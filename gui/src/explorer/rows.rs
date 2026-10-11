//! The explorer's flat row model.
//!
//! The tree is flattened into a `Vec<Row>` (only expanded nodes produce
//! children) and drawn through a virtualized list, so a schema with
//! thousands of tables never builds an element per table.

use crate::app_state::{LiveConnection, SchemaState};
use dbcrust_gui_core::schema::{self, ColumnFlags, ExplorerSection};
use dbcrust_gui_core::{ConnectionId, DatabaseType, TableDetails, TableEntry};
use gpui_kit::SharedString;
use std::collections::HashMap;
use std::sync::Arc;

/// Cache key of a table's description: the connection and the name it was
/// requested with (see `schema::describe_name`).
pub type DetailKey = (ConnectionId, String);

/// A table of a connection's (shared, immutable) table list.
#[derive(Clone)]
pub struct TableRef {
    tables: Arc<Vec<TableEntry>>,
    index: usize,
}

impl TableRef {
    pub fn entry(&self) -> &TableEntry {
        &self.tables[self.index]
    }
}

#[derive(Clone)]
pub enum RowKind {
    Connection,
    SchemaLoading,
    /// "Couldn't load tables" with a Retry; the reason follows as a
    /// [`RowKind::FailureDetail`].
    SchemaFailed {
        message: SharedString,
    },
    /// The first line of what went wrong (the full text is in a tooltip).
    FailureDetail {
        text: SharedString,
        full: SharedString,
    },
    /// "No tables" / "No tables match …"
    Note {
        text: SharedString,
    },
    Section {
        title: SharedString,
        count: usize,
        system: bool,
    },
    Group {
        title: SharedString,
        count: usize,
    },
    Table {
        table: TableRef,
        /// Catalog / system object (drawn dimmed).
        system: bool,
    },
    Column {
        table: TableRef,
        name: SharedString,
        data_type: SharedString,
        flags: ColumnFlags,
    },
    ColumnsLoading,
    /// "Couldn't load columns" with a Retry, then a [`RowKind::FailureDetail`].
    ColumnsFailed {
        key: String,
        message: SharedString,
    },
    ColumnsEmpty,
}

#[derive(Clone)]
pub struct Row {
    /// Stable, domain-derived id (connection id + schema + names).
    pub id: SharedString,
    pub connection: ConnectionId,
    pub depth: usize,
    pub kind: RowKind,
    pub expandable: bool,
    pub expanded: bool,
}

pub struct BuildContext<'a> {
    pub filter: &'a str,
    pub expansion: &'a HashMap<SharedString, bool>,
    pub errors: &'a HashMap<DetailKey, String>,
}

pub struct Built {
    pub rows: Vec<Row>,
    /// Expanded tables whose description is not cached yet.
    pub needs_details: Vec<(ConnectionId, String)>,
}

pub fn connection_row_id(id: ConnectionId) -> String {
    format!("{id}")
}

fn open(context: &BuildContext, id: &SharedString, default: bool, forced_by_filter: bool) -> bool {
    if forced_by_filter && !context.filter.trim().is_empty() {
        return true;
    }
    context.expansion.get(id).copied().unwrap_or(default)
}

pub fn build_rows(connections: &[LiveConnection], context: &BuildContext) -> Built {
    let mut built = Built {
        rows: Vec::new(),
        needs_details: Vec::new(),
    };
    for live in connections {
        build_connection(live, context, &mut built);
    }
    built
}

pub(super) fn object_noun(db_type: &DatabaseType) -> &'static str {
    match db_type {
        DatabaseType::MongoDB => "collections",
        DatabaseType::Elasticsearch => "indices",
        _ => "tables",
    }
}

fn build_connection(live: &LiveConnection, context: &BuildContext, built: &mut Built) {
    let connection = live.id();
    let db_type = &live.summary.db_type;
    let root_id: SharedString = connection_row_id(connection).into();
    let expanded = open(context, &root_id, true, true);
    built.rows.push(Row {
        id: root_id.clone(),
        connection,
        depth: 0,
        kind: RowKind::Connection,
        expandable: true,
        expanded,
    });
    if !expanded {
        return;
    }

    let status = |suffix: &str| -> SharedString { format!("{root_id}/{suffix}").into() };
    let tables = match &live.schema {
        SchemaState::Loading => {
            built.rows.push(Row {
                id: status("loading"),
                connection,
                depth: 1,
                kind: RowKind::SchemaLoading,
                expandable: false,
                expanded: false,
            });
            return;
        }
        SchemaState::Failed(message) => {
            built.rows.push(Row {
                id: status("failed"),
                connection,
                depth: 1,
                kind: RowKind::SchemaFailed {
                    message: message.clone().into(),
                },
                expandable: false,
                expanded: false,
            });
            built.rows.push(failure_detail(
                status("failed/detail"),
                connection,
                1,
                message,
            ));
            return;
        }
        SchemaState::Ready(tables) => tables,
    };

    let sections = schema::explorer_sections(db_type, tables, context.filter);
    if sections.is_empty() {
        let text = if context.filter.trim().is_empty() {
            format!("No {}", object_noun(db_type))
        } else {
            format!(
                "No {} match “{}”",
                object_noun(db_type),
                context.filter.trim()
            )
        };
        built.rows.push(Row {
            id: status("empty"),
            connection,
            depth: 1,
            kind: RowKind::Note { text: text.into() },
            expandable: false,
            expanded: false,
        });
        return;
    }

    for section in &sections {
        build_section(live, tables, section, &root_id, context, built);
    }
}

fn build_section(
    live: &LiveConnection,
    tables: &Arc<Vec<TableEntry>>,
    section: &ExplorerSection,
    root_id: &SharedString,
    context: &BuildContext,
    built: &mut Built,
) {
    let connection = live.id();
    let mut depth = 1;
    let mut section_id = root_id.clone();
    if let Some(title) = section.title() {
        section_id = format!("{root_id}/schema:{title}").into();
        let expanded = open(context, &section_id, !section.system, true);
        built.rows.push(Row {
            id: section_id.clone(),
            connection,
            depth,
            kind: RowKind::Section {
                title: title.into(),
                count: section.count,
                system: section.system,
            },
            expandable: true,
            expanded,
        });
        if !expanded {
            return;
        }
        depth += 1;
    }

    for group in &section.groups {
        let mut group_depth = depth;
        if let Some(label) = &group.label {
            let group_id: SharedString = format!("{section_id}/kind:{label}").into();
            let expanded = open(context, &group_id, true, true);
            built.rows.push(Row {
                id: group_id,
                connection,
                depth,
                kind: RowKind::Group {
                    title: label.clone().into(),
                    count: group.entries.len(),
                },
                expandable: true,
                expanded,
            });
            if !expanded {
                continue;
            }
            group_depth += 1;
        }
        for &index in &group.entries {
            let table = TableRef {
                tables: tables.clone(),
                index,
            };
            build_table(live, table, root_id, group_depth, context, built);
        }
    }
}

fn build_table(
    live: &LiveConnection,
    table: TableRef,
    root_id: &SharedString,
    depth: usize,
    context: &BuildContext,
    built: &mut Built,
) {
    let connection = live.id();
    let entry = table.entry();
    let key = schema::describe_name(&live.summary.db_type, entry);
    let table_id: SharedString = format!("{root_id}/table:{}", entry.qualified_name()).into();
    let expanded = context.expansion.get(&table_id).copied().unwrap_or(false);
    built.rows.push(Row {
        id: table_id.clone(),
        connection,
        depth,
        kind: RowKind::Table {
            table: table.clone(),
            system: schema::is_system_object(&live.summary.db_type, entry),
        },
        expandable: true,
        expanded,
    });
    if !expanded {
        return;
    }

    let child = |suffix: &str, kind: RowKind| Row {
        id: format!("{table_id}/{suffix}").into(),
        connection,
        depth: depth + 1,
        kind,
        expandable: false,
        expanded: false,
    };
    match live.details.get(&key) {
        Some(details) => push_columns(
            &mut built.rows,
            &table,
            details,
            &table_id,
            connection,
            depth + 1,
        ),
        None => match context.errors.get(&(connection, key.clone())) {
            Some(message) => {
                built.rows.push(child(
                    "failed",
                    RowKind::ColumnsFailed {
                        key,
                        message: message.clone().into(),
                    },
                ));
                built.rows.push(failure_detail(
                    format!("{table_id}/failed/detail").into(),
                    connection,
                    depth + 1,
                    message,
                ));
            }
            None => {
                built.needs_details.push((connection, key));
                built.rows.push(child("loading", RowKind::ColumnsLoading));
            }
        },
    }
}

/// The reason line under a failure row.
fn failure_detail(id: SharedString, connection: ConnectionId, depth: usize, message: &str) -> Row {
    Row {
        id,
        connection,
        depth,
        kind: RowKind::FailureDetail {
            text: failure_summary(message).into(),
            full: message.trim().to_string().into(),
        },
        expandable: false,
        expanded: false,
    }
}

/// The part of an error worth a row: its first non-empty line.
pub(super) fn failure_summary(message: &str) -> String {
    message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Unknown error")
        .to_string()
}

fn push_columns(
    rows: &mut Vec<Row>,
    table: &TableRef,
    details: &Arc<TableDetails>,
    table_id: &SharedString,
    connection: ConnectionId,
    depth: usize,
) {
    if details.columns.is_empty() {
        rows.push(Row {
            id: format!("{table_id}/empty").into(),
            connection,
            depth,
            kind: RowKind::ColumnsEmpty,
            expandable: false,
            expanded: false,
        });
        return;
    }
    let flags = schema::column_flags(details);
    for column in &details.columns {
        rows.push(Row {
            id: format!("{table_id}/col:{}", column.name).into(),
            connection,
            depth,
            kind: RowKind::Column {
                table: table.clone(),
                name: column.name.clone().into(),
                data_type: column.data_type.clone().into(),
                flags: flags.get(&column.name).copied().unwrap_or_default(),
            },
            expandable: false,
            expanded: false,
        });
    }
}

#[cfg(test)]
mod tests {
    // GPUI's `test` macro (glob-imported elsewhere) would shadow `#[test]`
    use std::prelude::v1::test;

    use super::*;

    #[test]
    fn a_failure_shows_its_first_line_only() {
        assert_eq!(
            failure_summary("\n  connection refused\n  at host:5432\n"),
            "connection refused"
        );
        assert_eq!(failure_summary("   "), "Unknown error");
        assert_eq!(failure_summary(""), "Unknown error");
    }

    #[test]
    fn the_detail_row_keeps_the_full_text_for_the_tooltip() {
        let row = failure_detail("x".into(), ConnectionId::next(), 2, "first\nsecond");
        let RowKind::FailureDetail { text, full } = row.kind else {
            panic!("a detail row");
        };
        assert_eq!(text.as_ref(), "first");
        assert_eq!(full.as_ref(), "first\nsecond");
        assert_eq!(row.depth, 2);
        assert!(!row.expandable);
    }
}
