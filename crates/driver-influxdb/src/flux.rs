//! Flux's annotated CSV (`POST /api/v2/query`, `Accept: application/csv`) as one
//! table.
//!
//! A response is a sequence of table blocks, each made of annotation rows
//! (`#datatype`, `#group`, `#default`), a header row and data rows; a block
//! starts again whenever a schema changes, and tables of the same schema share
//! a block (told apart by their `table` column). Every row begins with an empty
//! annotation cell, which is dropped.
//!
//! The blocks are unioned into one table: columns in first-seen order, `NULL`
//! where a block has no such column. An empty cell is `NULL` unless the
//! block's `#default` row gives the column a value (CSV cannot tell an empty
//! string from a null). The `result` column is hidden while it only says the
//! default `_result`, and `table` while the reply has a single table; both come
//! back as soon as they carry information.

use crate::conn::{Flow, Sink};
use crate::csv::RecordReader;
use crate::table::Table;
use dbcrust_core::database::DatabaseError;
use std::collections::{HashMap, HashSet};

/// One block's header: where its columns land in the unioned table.
struct Block {
    /// Position of each block column in the unioned column list.
    positions: Vec<usize>,
    /// `#default` value per block column (`None` = no default).
    defaults: Vec<Option<String>>,
    /// An error table (`error`, `reference`) instead of data.
    is_error: bool,
}

pub(crate) struct FluxSink {
    reader: RecordReader,
    state: FluxState,
}

struct FluxState {
    columns: Vec<String>,
    index: HashMap<String, usize>,
    rows: Vec<Vec<Option<String>>>,
    /// Annotation rows seen since the last header: (kind, cells).
    annotations: Vec<(String, Vec<String>)>,
    block: Option<Block>,
    blocks: usize,
    /// Distinct (block, `table` cell) pairs: how many tables the reply has.
    tables: HashSet<(usize, String)>,
    max_rows: usize,
    truncated: bool,
}

impl FluxSink {
    pub(crate) fn new(max_rows: usize) -> Self {
        Self {
            reader: RecordReader::new(),
            state: FluxState::new(max_rows),
        }
    }

    /// The unioned table, with `result` / `table` hidden when they say nothing.
    pub(crate) fn into_table(self) -> Table {
        self.state.into_table()
    }
}

impl FluxState {
    fn new(max_rows: usize) -> Self {
        Self {
            columns: Vec::new(),
            index: HashMap::new(),
            rows: Vec::new(),
            annotations: Vec::new(),
            block: None,
            blocks: 0,
            tables: HashSet::new(),
            max_rows,
            truncated: false,
        }
    }

    fn column_position(&mut self, name: &str) -> usize {
        if let Some(position) = self.index.get(name) {
            return *position;
        }
        let position = self.columns.len();
        self.columns.push(name.to_string());
        self.index.insert(name.to_string(), position);
        position
    }

    fn push_record(&mut self, record: Vec<String>) -> Result<Flow, DatabaseError> {
        let first = record.first().map(String::as_str).unwrap_or_default();
        if let Some(kind) = first.strip_prefix('#') {
            // A new annotation block after a header starts a new table block
            if self.block.is_some() {
                self.block = None;
                self.annotations.clear();
            }
            self.annotations
                .push((kind.to_ascii_lowercase(), record[1..].to_vec()));
            return Ok(Flow::Continue);
        }

        if self.block.is_none() {
            // The header row
            let names = &record[record.len().min(1)..];
            let is_error = names == ["error", "reference"];
            let defaults = self
                .annotations
                .iter()
                .find(|(kind, _)| kind == "default")
                .map(|(_, cells)| cells.clone())
                .unwrap_or_default();
            let positions = names
                .iter()
                .map(|name| self.column_position(name))
                .collect();
            let defaults = (0..names.len())
                .map(|i| defaults.get(i).filter(|value| !value.is_empty()).cloned())
                .collect();
            self.block = Some(Block {
                positions,
                defaults,
                is_error,
            });
            self.blocks += 1;
            return Ok(Flow::Continue);
        }

        let block_index = self.blocks;
        let block = self.block.as_ref().expect("header seen");
        let cells = &record[record.len().min(1)..];
        if block.is_error {
            let message = cells.first().cloned().unwrap_or_default();
            return Err(DatabaseError::QueryError(format!(
                "InfluxDB (Flux): {}",
                dbcrust_core::password_sanitizer::redact_text(&message)
            )));
        }
        if self.rows.len() >= self.max_rows {
            self.truncated = true;
            return Ok(Flow::Stop);
        }
        let mut row: Vec<Option<String>> = vec![None; self.columns.len()];
        for (i, position) in block.positions.iter().enumerate() {
            let cell = cells.get(i).map(String::as_str).unwrap_or_default();
            row[*position] = if cell.is_empty() {
                block.defaults[i].clone()
            } else {
                Some(cell.to_string())
            };
        }
        if let Some(table_position) = self.index.get("table")
            && let Some(Some(id)) = row.get(*table_position)
        {
            self.tables.insert((block_index, id.clone()));
        }
        self.rows.push(row);
        Ok(Flow::Continue)
    }

    fn into_table(self) -> Table {
        let width = self.columns.len();
        let hidden: Vec<usize> = ["result", "table"]
            .iter()
            .filter_map(|name| {
                let position = *self.index.get(*name)?;
                let says_nothing = if *name == "result" {
                    self.rows.iter().all(|row| {
                        matches!(row.get(position), None | Some(None))
                            || row[position].as_deref() == Some("_result")
                    })
                } else {
                    self.tables.len() <= 1
                };
                says_nothing.then_some(position)
            })
            .collect();
        let keep = |position: &usize| !hidden.contains(position);
        let columns = self
            .columns
            .iter()
            .enumerate()
            .filter(|(position, _)| keep(position))
            .map(|(_, name)| name.clone())
            .collect();
        let rows = self
            .rows
            .into_iter()
            .map(|mut row| {
                row.resize(width, None);
                row.into_iter()
                    .enumerate()
                    .filter(|(position, _)| keep(position))
                    .map(|(_, cell)| cell)
                    .collect()
            })
            .collect();
        Table {
            columns,
            rows,
            truncated: self.truncated,
            notices: Vec::new(),
        }
    }
}

impl Sink for FluxSink {
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

/// Parse a complete annotated CSV document (metadata queries, tests).
#[cfg(test)]
pub(crate) fn parse(csv: &str, max_rows: usize) -> Result<Table, DatabaseError> {
    let mut sink = FluxSink::new(max_rows);
    sink.feed(csv.as_bytes())?;
    sink.finish()?;
    Ok(sink.into_table())
}
