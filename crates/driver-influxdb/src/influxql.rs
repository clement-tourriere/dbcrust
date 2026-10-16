//! InfluxQL's JSON replies (v1 `/query`, v2's 1.x-compatible `/query`) as one
//! table.
//!
//! With `chunked=true` a reply is a stream of JSON documents, each
//! `{"results":[{"statement_id":0,"series":[{"name","tags","columns","values"}]}]}`;
//! a long series continues in the next document (`"partial":true`). The sink
//! decodes documents as they arrive, so the read stops at the row budget.
//!
//! One statement's series become one table: a `name` column when the series
//! have several names, a column per tag key (`GROUP BY` tags), then the series
//! columns. A reply with several non-empty statements shows the first and says
//! so in a notice.

use crate::conn::{Flow, Sink};
use crate::table::Table;
use dbcrust_core::database::DatabaseError;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Default)]
struct Series {
    name: String,
    tags: BTreeMap<String, Option<String>>,
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
}

#[derive(Debug, Default)]
struct Statement {
    series: Vec<Series>,
}

impl Statement {
    fn row_count(&self) -> usize {
        self.series.iter().map(|series| series.rows.len()).sum()
    }
}

pub(crate) struct InfluxQlSink {
    pending: Vec<u8>,
    statements: BTreeMap<u64, Statement>,
    /// Warnings the server attached to its answer (`messages`).
    warnings: Vec<String>,
    max_rows: usize,
    rows: usize,
    truncated: bool,
}

impl InfluxQlSink {
    pub(crate) fn new(max_rows: usize) -> Self {
        Self {
            pending: Vec::new(),
            statements: BTreeMap::new(),
            warnings: Vec::new(),
            max_rows,
            rows: 0,
            truncated: false,
        }
    }

    /// Decode every complete document in `pending`, keeping a partial tail.
    fn drain_documents(&mut self) -> Result<Flow, DatabaseError> {
        let mut consumed = 0;
        let mut documents: Vec<Value> = Vec::new();
        {
            let mut stream =
                serde_json::Deserializer::from_slice(&self.pending).into_iter::<Value>();
            loop {
                match stream.next() {
                    Some(Ok(value)) => {
                        consumed = stream.byte_offset();
                        documents.push(value);
                    }
                    Some(Err(error)) if error.is_eof() => break,
                    Some(Err(error)) => {
                        return Err(DatabaseError::QueryError(format!(
                            "InfluxDB sent invalid JSON: {error}"
                        )));
                    }
                    None => {
                        consumed = self.pending.len();
                        break;
                    }
                }
            }
        }
        self.pending.drain(..consumed);
        for document in documents {
            if self.add_document(document)? == Flow::Stop {
                return Ok(Flow::Stop);
            }
        }
        Ok(Flow::Continue)
    }

    fn add_document(&mut self, document: Value) -> Result<Flow, DatabaseError> {
        if let Some(message) = document.get("error").and_then(Value::as_str) {
            return Err(query_error(message));
        }
        let Some(results) = document.get("results").and_then(Value::as_array) else {
            return Ok(Flow::Continue);
        };
        for result in results {
            if let Some(message) = result.get("error").and_then(Value::as_str) {
                return Err(query_error(message));
            }
            let id = result
                .get("statement_id")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            for message in result
                .get("messages")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                if let Some(text) = message.get("text").and_then(Value::as_str) {
                    let warning = dbcrust_core::password_sanitizer::redact_text(text);
                    if !self.warnings.contains(&warning) {
                        self.warnings.push(warning);
                    }
                }
            }
            let statement = self.statements.entry(id).or_default();
            let Some(all_series) = result.get("series").and_then(Value::as_array) else {
                continue;
            };
            for raw in all_series {
                let name = raw
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let tags: BTreeMap<String, Option<String>> = raw
                    .get("tags")
                    .and_then(Value::as_object)
                    .map(|tags| {
                        tags.iter()
                            .map(|(key, value)| (key.clone(), cell(value)))
                            .collect()
                    })
                    .unwrap_or_default();
                let columns: Vec<String> = raw
                    .get("columns")
                    .and_then(Value::as_array)
                    .map(|columns| {
                        columns
                            .iter()
                            .map(|c| c.as_str().unwrap_or_default().to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                // A long series continues in the next document
                let continues = statement
                    .series
                    .last()
                    .is_some_and(|last| last.name == name && last.tags == tags);
                if !continues {
                    statement.series.push(Series {
                        name,
                        tags,
                        columns,
                        rows: Vec::new(),
                    });
                }
                let target = statement.series.last_mut().expect("series pushed");
                for values in raw
                    .get("values")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                {
                    if self.rows >= self.max_rows {
                        self.truncated = true;
                        return Ok(Flow::Stop);
                    }
                    let row = values
                        .as_array()
                        .map(|cells| cells.iter().map(cell).collect())
                        .unwrap_or_default();
                    target.rows.push(row);
                    self.rows += 1;
                }
            }
        }
        Ok(Flow::Continue)
    }

    pub(crate) fn into_table(self) -> Table {
        let truncated = self.truncated;
        let mut shown: Option<Statement> = None;
        let mut hidden = 0usize;
        for (_, statement) in self.statements {
            if statement.row_count() == 0 && statement.series.is_empty() {
                continue;
            }
            if shown.is_none() {
                shown = Some(statement);
            } else {
                hidden += 1;
            }
        }
        let mut table = shown.map(table_from_statement).unwrap_or_default();
        table.truncated = truncated;
        // InfluxDB 1.x runs a write sent as GET and only warns: a statement
        // the classifier let through as a read must not go unnoticed
        for warning in self.warnings {
            if warning.contains("read only context") {
                table.notices.push(format!(
                    "the server treated a statement as a write sent over GET and ran it: {warning}"
                ));
            } else {
                table.notices.push(warning);
            }
        }
        if hidden > 0 {
            table.notices.push(format!(
                "{hidden} more statement result(s) were not shown: send one statement at a time"
            ));
        }
        table
    }
}

fn query_error(message: &str) -> DatabaseError {
    DatabaseError::QueryError(format!(
        "InfluxDB: {}",
        dbcrust_core::password_sanitizer::redact_text(message)
    ))
}

/// A JSON cell as text: strings as they are, `null` as NULL, numbers and
/// booleans in their JSON spelling, nested values as JSON.
fn cell(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        other => Some(other.to_string()),
    }
}

fn table_from_statement(statement: Statement) -> Table {
    let several_names = statement
        .series
        .iter()
        .map(|series| series.name.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        > 1;
    let tag_keys: Vec<String> = statement
        .series
        .iter()
        .flat_map(|series| series.tags.keys().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut columns: Vec<String> = Vec::new();
    if several_names {
        columns.push("name".to_string());
    }
    columns.extend(tag_keys.iter().cloned());
    let fixed = columns.len();
    for series in &statement.series {
        for column in &series.columns {
            // A tag key and a field of the same name share a column
            if !columns.contains(column) {
                columns.push(column.clone());
            }
        }
    }

    let mut rows = Vec::new();
    for series in &statement.series {
        let positions: Vec<usize> = series
            .columns
            .iter()
            .map(|column| {
                columns
                    .iter()
                    .position(|candidate| candidate == column)
                    .expect("column listed above")
            })
            .collect();
        for values in &series.rows {
            let mut row: Vec<Option<String>> = vec![None; columns.len()];
            if several_names {
                row[0] = Some(series.name.clone());
            }
            for (i, key) in tag_keys.iter().enumerate() {
                let slot = usize::from(several_names) + i;
                row[slot] = series.tags.get(key).cloned().flatten();
            }
            for (position, value) in positions.iter().zip(values) {
                // Keep a tag's value when the series column repeats its name
                if *position >= fixed || row[*position].is_none() {
                    row[*position] = value.clone();
                }
            }
            rows.push(row);
        }
    }
    Table {
        columns,
        rows,
        truncated: false,
        notices: Vec::new(),
    }
}

impl Sink for InfluxQlSink {
    fn feed(&mut self, bytes: &[u8]) -> Result<Flow, DatabaseError> {
        self.pending.extend_from_slice(bytes);
        self.drain_documents()
    }

    fn finish(&mut self) -> Result<(), DatabaseError> {
        self.drain_documents()?;
        // Whatever is left is not a complete document
        let leftover = self.pending.iter().any(|byte| !byte.is_ascii_whitespace());
        if leftover {
            return Err(DatabaseError::QueryError(
                "InfluxDB sent an incomplete JSON reply".to_string(),
            ));
        }
        Ok(())
    }
}

/// Parse a complete JSON reply (tests).
#[cfg(test)]
pub(crate) fn parse(json: &str, max_rows: usize) -> Result<Table, DatabaseError> {
    let mut sink = InfluxQlSink::new(max_rows);
    sink.feed(json.as_bytes())?;
    sink.finish()?;
    Ok(sink.into_table())
}
