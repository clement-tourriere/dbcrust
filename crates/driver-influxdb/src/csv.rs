//! Incremental CSV records over a byte stream (`csv-core`): a response is
//! parsed while it arrives so the read can stop at the row budget.

use crate::conn::Flow;
use csv_core::{ReadRecordResult, Reader, ReaderBuilder, Terminator};
use dbcrust_core::database::DatabaseError;

/// Splits a CSV byte stream into records of text fields. Empty lines produce
/// no record; quoted fields may contain delimiters and line breaks.
pub(crate) struct RecordReader {
    reader: Reader,
    out: Vec<u8>,
    ends: Vec<usize>,
    out_len: usize,
    ends_len: usize,
}

type OnRecord<'a> = dyn FnMut(Vec<String>) -> Result<Flow, DatabaseError> + 'a;

impl RecordReader {
    pub(crate) fn new() -> Self {
        Self {
            reader: ReaderBuilder::new().terminator(Terminator::CRLF).build(),
            out: vec![0; 4096],
            ends: vec![0; 64],
            out_len: 0,
            ends_len: 0,
        }
    }

    /// Feed `input`, calling `on_record` for every record it completes. A
    /// record cut by the end of `input` is kept for the next call.
    pub(crate) fn feed(
        &mut self,
        mut input: &[u8],
        on_record: &mut OnRecord<'_>,
    ) -> Result<Flow, DatabaseError> {
        // An empty slice means end of input to csv-core: never pass one here
        if input.is_empty() {
            return Ok(Flow::Continue);
        }
        self.drive(&mut input, on_record)
    }

    /// The stream ended: flush a last record that has no line terminator.
    pub(crate) fn finish(&mut self, on_record: &mut OnRecord<'_>) -> Result<Flow, DatabaseError> {
        self.drive(&mut &[][..], on_record)
    }

    fn drive(
        &mut self,
        input: &mut &[u8],
        on_record: &mut OnRecord<'_>,
    ) -> Result<Flow, DatabaseError> {
        let at_end = input.is_empty();
        loop {
            let (result, read, written, ends) = self.reader.read_record(
                input,
                &mut self.out[self.out_len..],
                &mut self.ends[self.ends_len..],
            );
            *input = &input[read..];
            self.out_len += written;
            self.ends_len += ends;
            match result {
                ReadRecordResult::InputEmpty => return Ok(Flow::Continue),
                ReadRecordResult::OutputFull => {
                    let grown = self.out.len() * 2;
                    self.out.resize(grown, 0);
                }
                ReadRecordResult::OutputEndsFull => {
                    let grown = self.ends.len() * 2;
                    self.ends.resize(grown, 0);
                }
                ReadRecordResult::Record => {
                    let mut fields = Vec::with_capacity(self.ends_len);
                    let mut start = 0;
                    for end in &self.ends[..self.ends_len] {
                        fields.push(String::from_utf8_lossy(&self.out[start..*end]).into_owned());
                        start = *end;
                    }
                    self.out_len = 0;
                    self.ends_len = 0;
                    if on_record(fields)? == Flow::Stop {
                        return Ok(Flow::Stop);
                    }
                    if at_end && input.is_empty() {
                        // csv-core reports the flushed record, then End
                        continue;
                    }
                }
                ReadRecordResult::End => return Ok(Flow::Continue),
            }
        }
    }
}

/// A plain CSV reply (header row, then data rows) as one table — the v3 query
/// endpoints. An empty cell is NULL.
pub(crate) struct CsvTableSink {
    reader: RecordReader,
    state: CsvTableState,
}

struct CsvTableState {
    columns: Option<Vec<String>>,
    rows: Vec<Vec<Option<String>>>,
    max_rows: usize,
    truncated: bool,
}

impl CsvTableState {
    fn push_record(&mut self, record: Vec<String>) -> Result<Flow, DatabaseError> {
        let Some(columns) = &self.columns else {
            self.columns = Some(record);
            return Ok(Flow::Continue);
        };
        if self.rows.len() >= self.max_rows {
            self.truncated = true;
            return Ok(Flow::Stop);
        }
        let mut row: Vec<Option<String>> = record
            .into_iter()
            .map(|cell| (!cell.is_empty()).then_some(cell))
            .collect();
        row.resize(columns.len(), None);
        self.rows.push(row);
        Ok(Flow::Continue)
    }
}

impl CsvTableSink {
    pub(crate) fn new(max_rows: usize) -> Self {
        Self {
            reader: RecordReader::new(),
            state: CsvTableState {
                columns: None,
                rows: Vec::new(),
                max_rows,
                truncated: false,
            },
        }
    }

    pub(crate) fn into_table(self) -> crate::table::Table {
        crate::table::Table {
            columns: self.state.columns.unwrap_or_default(),
            rows: self.state.rows,
            truncated: self.state.truncated,
            notices: Vec::new(),
        }
    }
}

impl crate::conn::Sink for CsvTableSink {
    fn feed(&mut self, bytes: &[u8]) -> Result<Flow, DatabaseError> {
        let state = &mut self.state;
        self.reader
            .feed(bytes, &mut |record| state.push_record(record))
    }

    fn finish(&mut self) -> Result<(), DatabaseError> {
        let state = &mut self.state;
        self.reader
            .finish(&mut |record| state.push_record(record))
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(chunks: &[&str]) -> Vec<Vec<String>> {
        let mut reader = RecordReader::new();
        let mut out: Vec<Vec<String>> = Vec::new();
        for chunk in chunks {
            reader
                .feed(chunk.as_bytes(), &mut |record| {
                    out.push(record);
                    Ok(Flow::Continue)
                })
                .unwrap();
        }
        reader
            .finish(&mut |record| {
                out.push(record);
                Ok(Flow::Continue)
            })
            .unwrap();
        out
    }

    fn row(fields: &[&str]) -> Vec<String> {
        fields.iter().map(|f| (*f).to_string()).collect()
    }

    #[test]
    fn quoted_fields_keep_delimiters_quotes_and_line_breaks() {
        assert_eq!(
            records(&["a,\"b,c\",\"d\"\"e\"\r\n\"line\nbreak\",,x\r\n"]),
            vec![row(&["a", "b,c", "d\"e"]), row(&["line\nbreak", "", "x"])]
        );
    }

    #[test]
    fn a_record_cut_between_chunks_is_completed() {
        assert_eq!(
            records(&["a,b", "c\r", "\n1,\"2", "3\"\r\n"]),
            vec![row(&["a", "bc"]), row(&["1", "23"])]
        );
    }

    #[test]
    fn blank_lines_make_no_record_and_the_last_line_needs_no_terminator() {
        assert_eq!(
            records(&["a,b\r\n\r\n\r\nc,d"]),
            vec![row(&["a", "b"]), row(&["c", "d"])]
        );
    }

    #[test]
    fn long_fields_and_many_columns_grow_the_buffers() {
        let long = "x".repeat(10_000);
        let line = (0..200)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let got = records(&[&format!("{long},1\r\n{line}\r\n")]);
        assert_eq!(got[0], vec![long, "1".to_string()]);
        assert_eq!(got[1].len(), 200);
    }

    #[test]
    fn utf8_split_across_chunks_survives() {
        let text = "héllo,wörld\r\n";
        let bytes = text.as_bytes();
        let (first, second) = bytes.split_at(2); // inside "é"
        let mut reader = RecordReader::new();
        let mut out = Vec::new();
        for part in [first, second] {
            reader
                .feed(part, &mut |record| {
                    out.push(record);
                    Ok(Flow::Continue)
                })
                .unwrap();
        }
        assert_eq!(out, vec![row(&["héllo", "wörld"])]);
    }

    #[test]
    fn stop_ends_the_feed() {
        let mut reader = RecordReader::new();
        let mut seen = 0;
        let flow = reader
            .feed(b"a\r\nb\r\nc\r\n", &mut |_| {
                seen += 1;
                Ok(if seen == 2 {
                    Flow::Stop
                } else {
                    Flow::Continue
                })
            })
            .unwrap();
        assert_eq!((flow, seen), (Flow::Stop, 2));
    }
}
