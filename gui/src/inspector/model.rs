//! What the inspector shows, shaped once when a description arrives.

use dbcrust_gui_core::schema::{self, ColumnOutline, ForeignKey, IndexOutline};
use dbcrust_gui_core::{DatabaseType, TableDetails, TableEntry};
use std::sync::Arc;

/// A row of the Columns tab: a column, or one nested field below it.
pub(super) enum ColumnLine {
    Column(Box<ColumnOutline>),
    Nested { text: String },
}

pub(super) struct ForeignKeyLine {
    pub name: String,
    pub definition: String,
    pub parsed: Option<ForeignKey>,
}

pub(super) struct ReferenceLine {
    /// `schema.table` of the referencing table.
    pub table: String,
    pub schema: String,
    pub name: String,
    pub constraint: String,
    pub definition: String,
    pub parsed: Option<ForeignKey>,
}

/// A described table, with every list the tabs need already computed.
pub(super) struct Described {
    pub details: Arc<TableDetails>,
    pub columns: Vec<ColumnLine>,
    pub indexes: Vec<IndexOutline>,
    pub foreign_keys: Vec<ForeignKeyLine>,
    pub references: Vec<ReferenceLine>,
    pub ddl: Option<String>,
}

impl Described {
    pub fn new(db_type: &DatabaseType, _table: &TableEntry, details: Arc<TableDetails>) -> Self {
        let outlines = schema::column_outlines(db_type, &details);
        let mut columns = Vec::with_capacity(outlines.len());
        for outline in outlines {
            let nested = outline.nested.clone();
            columns.push(ColumnLine::Column(Box::new(outline)));
            columns.extend(nested.into_iter().map(|text| ColumnLine::Nested { text }));
        }
        let foreign_keys = details
            .foreign_keys
            .iter()
            .map(|fk| ForeignKeyLine {
                name: fk.name.clone(),
                definition: fk.definition.clone(),
                parsed: schema::parse_foreign_key(&fk.name, &fk.definition),
            })
            .collect();
        let references = details
            .referenced_by
            .iter()
            .map(|reference| ReferenceLine {
                table: if reference.schema.is_empty() {
                    reference.table.clone()
                } else {
                    format!("{}.{}", reference.schema, reference.table)
                },
                schema: reference.schema.clone(),
                name: reference.table.clone(),
                constraint: reference.constraint_name.clone(),
                definition: reference.definition.clone(),
                parsed: schema::parse_foreign_key(
                    &reference.constraint_name,
                    &reference.definition,
                ),
            })
            .collect();
        let ddl = schema::supports_ddl(db_type).then(|| schema::ddl_summary(db_type, &details));
        Self {
            indexes: schema::index_outlines(&details),
            details,
            columns,
            foreign_keys,
            references,
            ddl,
        }
    }

    pub fn summary(&self, db_type: &DatabaseType) -> String {
        schema::describe_counts(db_type, &self.details)
    }
}
