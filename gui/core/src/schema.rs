//! Schema metadata models: parsing DBCrust's tabular metadata output and
//! building safe identifiers for generated SQL.

use crate::types::{TableDetails, TableEntry, TableKind};
use dbcrust::database::DatabaseType;
use dbcrust::db::{ColumnInfo, IndexInfo};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

/// Parse `Database::list_tables` output (header row `Schema, Name, Type,
/// Owner`, then one row per object). Columns are located by header name so
/// engine-specific orderings keep working.
pub fn parse_table_list(db_type: &DatabaseType, rows: Vec<Vec<String>>) -> Vec<TableEntry> {
    let mut rows = rows.into_iter();
    let Some(header) = rows.next() else {
        return Vec::new();
    };
    let find = |name: &str| {
        header
            .iter()
            .position(|column| column.trim().eq_ignore_ascii_case(name))
    };
    let name_column = find("name").or_else(|| find("table")).unwrap_or(0);
    let schema_column = find("schema");
    let kind_column = find("type").or_else(|| find("kind"));
    let owner_column = find("owner");

    let mut entries: Vec<TableEntry> = rows
        .filter_map(|row| {
            let name = row.get(name_column)?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            let cell = |index: Option<usize>| {
                index
                    .and_then(|index| row.get(index))
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("unknown"))
            };
            let kind = match cell(kind_column) {
                Some(label) => TableKind::from_label(&label),
                None if matches!(db_type, DatabaseType::MongoDB) => TableKind::Collection,
                None if matches!(db_type, DatabaseType::Elasticsearch) => TableKind::Index,
                None => TableKind::Table,
            };
            Some(TableEntry {
                schema: cell(schema_column),
                name,
                kind,
                owner: cell(owner_column),
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        is_system_object(db_type, a)
            .cmp(&is_system_object(db_type, b))
            .then_with(|| a.schema.cmp(&b.schema))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

/// Parse `Database::list_databases` output (header row first) into names.
pub fn parse_database_list(rows: Vec<Vec<String>>) -> Vec<String> {
    let mut rows = rows.into_iter();
    let Some(header) = rows.next() else {
        return Vec::new();
    };
    let name_column = header
        .iter()
        .position(|column| {
            let column = column.trim();
            column.eq_ignore_ascii_case("name") || column.eq_ignore_ascii_case("database")
        })
        .unwrap_or(0);
    rows.filter_map(|row| row.get(name_column).map(|name| name.trim().to_string()))
        .filter(|name| !name.is_empty())
        .collect()
}

/// Quote an identifier for `db_type` only when needed (keeps generated SQL
/// readable): lower-case-safe names stay bare, everything else is quoted
/// with the engine's quote character, doubling embedded quotes.
pub fn quote_identifier(db_type: &DatabaseType, identifier: &str) -> String {
    let backtick = matches!(db_type, DatabaseType::MySQL | DatabaseType::ClickHouse);
    let brackets = matches!(db_type, DatabaseType::SqlServer);
    let bare = !identifier.is_empty()
        && !is_reserved_word(identifier)
        && identifier.chars().enumerate().all(|(i, c)| {
            let first_ok = c == '_' || c.is_ascii_alphabetic();
            let rest_ok = first_ok || c.is_ascii_digit() || c == '$';
            let case_ok = !matches!(db_type, DatabaseType::PostgreSQL | DatabaseType::Redshift)
                || !c.is_ascii_uppercase();
            (if i == 0 { first_ok } else { rest_ok }) && case_ok
        });
    if bare {
        identifier.to_string()
    } else if backtick {
        format!("`{}`", identifier.replace('`', "``"))
    } else if brackets {
        format!("[{}]", identifier.replace(']', "]]"))
    } else {
        format!("\"{}\"", identifier.replace('"', "\"\""))
    }
}

/// `schema.table` reference for generated SQL. The schema is omitted when
/// it is the engine's default (`public`, `main`) or absent.
pub fn table_reference(db_type: &DatabaseType, entry: &TableEntry) -> String {
    let table = quote_identifier(db_type, &entry.name);
    match entry.schema.as_deref() {
        Some(schema) if !is_default_schema(db_type, schema) => {
            format!("{}.{table}", quote_identifier(db_type, schema))
        }
        _ => table,
    }
}

/// The schema objects live in when none is given.
pub fn is_default_schema(db_type: &DatabaseType, schema: &str) -> bool {
    match db_type {
        DatabaseType::PostgreSQL | DatabaseType::Redshift => schema == "public",
        DatabaseType::SQLite | DatabaseType::DuckDB | DatabaseType::LibSql => schema == "main",
        DatabaseType::SqlServer => schema == "dbo",
        DatabaseType::MongoDB | DatabaseType::Elasticsearch => true,
        // A measurement belongs to the connected database; `"db"."x"` would
        // name a retention policy in InfluxQL
        DatabaseType::InfluxDB => true,
        _ => false,
    }
}

fn is_reserved_word(identifier: &str) -> bool {
    const RESERVED: &[&str] = &[
        "all",
        "and",
        "as",
        "asc",
        "by",
        "case",
        "check",
        "column",
        "constraint",
        "create",
        "cross",
        "default",
        "delete",
        "desc",
        "distinct",
        "drop",
        "else",
        "end",
        "exists",
        "foreign",
        "from",
        "full",
        "group",
        "having",
        "in",
        "index",
        "inner",
        "insert",
        "into",
        "is",
        "join",
        "key",
        "left",
        "like",
        "limit",
        "not",
        "null",
        "offset",
        "on",
        "or",
        "order",
        "outer",
        "primary",
        "references",
        "right",
        "select",
        "set",
        "table",
        "then",
        "to",
        "union",
        "unique",
        "update",
        "user",
        "using",
        "values",
        "when",
        "where",
        "with",
    ];
    RESERVED.contains(&identifier.to_ascii_lowercase().as_str())
}

/// Engine catalog/system objects, listed after user objects.
pub fn is_system_object(db_type: &DatabaseType, entry: &TableEntry) -> bool {
    let name = entry.name.to_lowercase();
    let schema = entry.schema.as_deref().unwrap_or("").to_lowercase();
    match db_type {
        DatabaseType::PostgreSQL => {
            schema == "pg_catalog"
                || schema.starts_with("information_schema")
                || name.starts_with("pg_")
        }
        DatabaseType::Redshift => {
            schema == "pg_catalog"
                || schema == "pg_internal"
                || schema.starts_with("information_schema")
                || name.starts_with("pg_")
                || name.starts_with("stl_")
                || name.starts_with("stv_")
                || name.starts_with("svv_")
                || name.starts_with("svl_")
        }
        DatabaseType::MySQL => matches!(
            schema.as_str(),
            "information_schema" | "performance_schema" | "mysql" | "sys"
        ),
        DatabaseType::SQLite | DatabaseType::LibSql => name.starts_with("sqlite_"),
        DatabaseType::ClickHouse => schema == "system" || schema == "information_schema",
        DatabaseType::MongoDB => name.starts_with("system."),
        DatabaseType::Elasticsearch => name.starts_with('.'),
        DatabaseType::DuckDB => name.starts_with("duckdb_") || name.starts_with("sqlite_"),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Generated SQL
// ---------------------------------------------------------------------------

/// Rows fetched by "Select top 100" and the inspector's data preview.
pub const PREVIEW_ROWS: usize = 100;

/// The name to hand to `describe_table` for `entry`: `schema.table` outside
/// the engine's default schema, with parts quoted when they contain a dot or
/// a quote (so the core does not split them). Used as the cache key too, so
/// every view that describes a table shares one entry.
pub fn describe_name(db_type: &DatabaseType, entry: &TableEntry) -> String {
    let part = |value: &str| {
        if value.contains('.') || value.contains('"') {
            format!("\"{}\"", value.replace('"', "\"\""))
        } else {
            value.to_string()
        }
    };
    match entry.schema.as_deref() {
        Some(schema) if !is_default_schema(db_type, schema) => {
            format!("{}.{}", part(schema), part(&entry.name))
        }
        _ => part(&entry.name),
    }
}

/// A Redis key as one `redis-cli` argument: double-quoted, `\` and `"` escaped.
fn redis_key(name: &str) -> String {
    format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The read command that previews a Redis key of the entry's type (the
/// explorer's "kind" is the key's `TYPE`), bounded to `limit` elements.
fn redis_preview(entry: &TableEntry, limit: usize) -> String {
    let key = redis_key(&entry.name);
    let last = limit.saturating_sub(1);
    match entry.kind.label() {
        "string" => format!("GET {key}"),
        "hash" => format!("HSCAN {key} 0 COUNT {limit}"),
        "list" => format!("LRANGE {key} 0 {last}"),
        "set" => format!("SSCAN {key} 0 COUNT {limit}"),
        "zset" => format!("ZRANGE {key} 0 {last} WITHSCORES"),
        "stream" => format!("XRANGE {key} - + COUNT {limit}"),
        _ => format!("TYPE {key}"),
    }
}

/// The command that sizes a Redis key of the entry's type.
fn redis_size(entry: &TableEntry) -> String {
    let key = redis_key(&entry.name);
    let command = match entry.kind.label() {
        "string" => "STRLEN",
        "hash" => "HLEN",
        "list" => "LLEN",
        "set" => "SCARD",
        "zset" => "ZCARD",
        "stream" => "XLEN",
        _ => "TTL",
    };
    format!("{command} {key}")
}

/// `SELECT * FROM <table> LIMIT n;` (SQL Server: `SELECT TOP (n) * FROM …`;
/// MongoDB: `db.<collection>.find({})`; Redis: the read command for the key's
/// type).
pub fn select_star_sql(db_type: &DatabaseType, entry: &TableEntry, limit: usize) -> String {
    match db_type {
        DatabaseType::MongoDB => format!("db.{}.find({{}})", entry.name),
        DatabaseType::Redis => redis_preview(entry, limit),
        DatabaseType::SqlServer => format!(
            "SELECT TOP ({limit}) * FROM {};",
            table_reference(db_type, entry)
        ),
        _ => format!(
            "SELECT * FROM {} LIMIT {limit};",
            table_reference(db_type, entry)
        ),
    }
}

/// `SELECT col, … FROM <table> LIMIT n;` with one column per line.
pub fn select_columns_sql(
    db_type: &DatabaseType,
    entry: &TableEntry,
    columns: &[String],
    limit: usize,
) -> String {
    if columns.is_empty() || matches!(db_type, DatabaseType::MongoDB | DatabaseType::Redis) {
        return select_star_sql(db_type, entry, limit);
    }
    let list = columns
        .iter()
        .map(|column| format!("    {}", quote_identifier(db_type, column)))
        .collect::<Vec<_>>()
        .join(",\n");
    if matches!(db_type, DatabaseType::SqlServer) {
        return format!(
            "SELECT TOP ({limit})\n{list}\nFROM {};",
            table_reference(db_type, entry)
        );
    }
    format!(
        "SELECT\n{list}\nFROM {}\nLIMIT {limit};",
        table_reference(db_type, entry)
    )
}

/// `SELECT COUNT(*) FROM <table>;` (MongoDB: a `$count` aggregation).
pub fn count_sql(db_type: &DatabaseType, entry: &TableEntry) -> String {
    match db_type {
        DatabaseType::MongoDB => {
            format!("db.{}.aggregate([{{\"$count\": \"count\"}}])", entry.name)
        }
        DatabaseType::Redis => redis_size(entry),
        _ => format!("SELECT COUNT(*) FROM {};", table_reference(db_type, entry)),
    }
}

/// The number a `count_sql` statement returned: the first cell of the first
/// row, as `1234`, `1,234` or inside a document (`{"count": 1234}`).
pub fn parse_count(rows: &[Vec<Option<String>>]) -> Option<u64> {
    let cell = rows.first()?.first()?.as_deref()?.trim();
    let plain: String = cell
        .chars()
        .filter(|c| !matches!(c, ',' | '_' | ' ' | '\u{202f}' | '\u{a0}'))
        .collect();
    if let Ok(count) = plain.parse::<u64>() {
        return Some(count);
    }
    // a float rendering of an integral count (`1234.0`)
    if let Ok(float) = plain.parse::<f64>()
        && float >= 0.0
        && float.fract() == 0.0
        && float < u64::MAX as f64
    {
        return Some(float as u64);
    }
    // inside a document: the first run of digits after the last `:`
    if !cell.contains(':') {
        return None;
    }
    let tail = cell.rsplit(':').next().unwrap_or(cell);
    let digits: String = tail
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// `1234567` → `1,234,567`.
pub fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Engines whose SQL dialect takes `INSERT` through DBCrust.
pub fn supports_insert(db_type: &DatabaseType) -> bool {
    matches!(
        db_type,
        DatabaseType::PostgreSQL
            | DatabaseType::Redshift
            | DatabaseType::MySQL
            | DatabaseType::SQLite
            | DatabaseType::LibSql
            | DatabaseType::ClickHouse
    )
}

/// A fill-in-the-blanks `INSERT` with one column per line. `None` for
/// engines without SQL inserts (files, MongoDB, Elasticsearch, …).
pub fn insert_template_sql(
    db_type: &DatabaseType,
    entry: &TableEntry,
    columns: &[ColumnInfo],
) -> Option<String> {
    if !supports_insert(db_type) || columns.is_empty() {
        return None;
    }
    let names = columns
        .iter()
        .map(|column| format!("    {}", quote_identifier(db_type, &column.name)))
        .collect::<Vec<_>>()
        .join(",\n");
    let last = columns.len() - 1;
    let values = columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let separator = if index == last { " " } else { "," };
            let mut note = column.data_type.clone();
            if !column.nullable {
                note.push_str(", not null");
            }
            if let Some(default) = column.default_value.as_deref().filter(|d| !d.is_empty()) {
                note.push_str(&format!(", default {default}"));
            }
            format!(
                "    {}{separator} -- {}: {note}",
                placeholder_value(column),
                column.name
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "INSERT INTO {} (\n{names}\n)\nVALUES (\n{values}\n);",
        table_reference(db_type, entry)
    ))
}

/// A starting value for an `INSERT` template, by column type.
fn placeholder_value(column: &ColumnInfo) -> &'static str {
    if column.nullable || column.default_value.is_some() {
        return "NULL";
    }
    let ty = column.data_type.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| ty.contains(needle));
    if has(&["bool"]) {
        "FALSE"
    } else if has(&[
        "int", "serial", "numeric", "decimal", "float", "double", "real", "money",
    ]) {
        "0"
    } else if has(&[
        "char", "text", "uuid", "json", "date", "time", "string", "enum",
    ]) {
        "''"
    } else {
        "NULL"
    }
}

// ---------------------------------------------------------------------------
// Constraint parsing and DDL
// ---------------------------------------------------------------------------

/// A foreign key parsed from DBCrust's `FOREIGN KEY (a, b) REFERENCES
/// t(x, y) [ON DELETE …]` definition text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKey {
    pub name: String,
    pub columns: Vec<String>,
    /// The referenced table, as written by the engine (may be qualified).
    pub target: String,
    pub target_columns: Vec<String>,
    /// Trailing clauses (`ON DELETE CASCADE`, …), empty when absent.
    pub actions: String,
}

impl ForeignKey {
    /// `a, b → table(x, y)`
    pub fn summary(&self) -> String {
        format!(
            "{} → {}({})",
            self.columns.join(", "),
            self.target,
            self.target_columns.join(", ")
        )
    }
}

/// Parse a foreign key definition. `None` when it does not follow the
/// `FOREIGN KEY (…) REFERENCES t(…)` shape (the raw text is still shown).
pub fn parse_foreign_key(name: &str, definition: &str) -> Option<ForeignKey> {
    let lower = definition.to_ascii_lowercase();
    let key_at = lower.find("foreign key")?;
    let (columns, columns_end) = balanced_group(definition, key_at + "foreign key".len())?;
    let references_at = lower[columns_end..].find("references")? + columns_end;
    let after_references = references_at + "references".len();
    let open = definition[after_references..].find('(')? + after_references;
    let target = definition[after_references..open].trim().to_string();
    let (target_columns, target_end) = balanced_group(definition, open)?;
    Some(ForeignKey {
        name: name.to_string(),
        columns: split_top_level(&columns),
        target,
        target_columns: split_top_level(&target_columns),
        actions: definition[target_end..].trim().to_string(),
    })
}

/// The foreign keys of a table that could be parsed.
pub fn foreign_keys(details: &TableDetails) -> Vec<ForeignKey> {
    details
        .foreign_keys
        .iter()
        .filter_map(|fk| parse_foreign_key(&fk.name, &fk.definition))
        .collect()
}

/// Column names of an index, from its `definition` (a full `CREATE INDEX`
/// statement on PostgreSQL, `(a, b)` elsewhere). Expressions stay verbatim.
pub fn index_columns(index: &IndexInfo) -> Vec<String> {
    let definition = index.definition.as_str();
    let lower = definition.to_ascii_lowercase();
    let from = lower
        .find(" using ")
        .map(|at| at + " using ".len())
        .unwrap_or(0);
    balanced_group(definition, from)
        .map(|(list, _)| split_top_level(&list))
        .unwrap_or_default()
}

/// Columns of the primary key, empty when none is reported. (SQLite does not
/// report the implicit `INTEGER PRIMARY KEY` rowid alias as an index.)
pub fn primary_key_columns(details: &TableDetails) -> Vec<String> {
    details
        .indexes
        .iter()
        .find(|index| index.is_primary)
        .map(index_columns)
        .unwrap_or_default()
}

/// Per-column markers shown in the explorer and the inspector.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ColumnFlags {
    pub primary_key: bool,
    pub foreign_key: bool,
    pub unique: bool,
    pub not_null: bool,
}

/// Markers for every column of `details`, keyed by column name.
pub fn column_flags(details: &TableDetails) -> HashMap<String, ColumnFlags> {
    let primary = primary_key_columns(details);
    let foreign: Vec<String> = foreign_keys(details)
        .into_iter()
        .flat_map(|fk| fk.columns)
        .collect();
    let unique: Vec<String> = details
        .indexes
        .iter()
        .filter(|index| index.is_unique && !index.is_primary)
        .map(index_columns)
        .filter(|columns| columns.len() == 1)
        .flatten()
        .collect();
    details
        .columns
        .iter()
        .map(|column| {
            let flags = ColumnFlags {
                primary_key: primary.contains(&column.name),
                foreign_key: foreign.contains(&column.name),
                unique: unique.contains(&column.name),
                not_null: !column.nullable,
            };
            (column.name.clone(), flags)
        })
        .collect()
}

/// Engines for which a `CREATE TABLE` reconstruction makes sense.
pub fn supports_ddl(db_type: &DatabaseType) -> bool {
    !matches!(db_type, DatabaseType::MongoDB | DatabaseType::Elasticsearch)
}

/// A readable `CREATE TABLE` (primary key, unique, foreign keys, checks)
/// followed by `CREATE INDEX` statements, rebuilt from the metadata DBCrust
/// reports. It is a summary for reading and copying, not an exact dump:
/// engine-specific storage options are not included.
pub fn ddl_summary(db_type: &DatabaseType, details: &TableDetails) -> String {
    let entry = TableEntry {
        schema: Some(details.schema.clone()).filter(|schema| !schema.is_empty()),
        name: details.name.clone(),
        kind: TableKind::Table,
        owner: None,
    };
    let reference = table_reference(db_type, &entry);

    // (line, trailing comment)
    let mut lines: Vec<(String, Option<String>)> = Vec::new();
    for column in &details.columns {
        let mut line = format!(
            "{} {}",
            quote_identifier(db_type, &column.name),
            column.data_type
        );
        if !column.nullable {
            line.push_str(" NOT NULL");
        }
        if let Some(default) = column.default_value.as_deref().filter(|d| !d.is_empty()) {
            line.push_str(&format!(" DEFAULT {default}"));
        }
        let note = column
            .enum_values
            .as_ref()
            .filter(|values| !values.is_empty())
            .map(|values| format!("enum: {}", values.join(", ")));
        lines.push((line, note));
    }

    let quoted_list = |columns: &[String]| {
        columns
            .iter()
            .map(|column| quote_identifier(db_type, column))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let mut trailing_indexes: Vec<String> = Vec::new();
    for index in &details.indexes {
        let columns = index_columns(index);
        if index.is_primary {
            if !columns.is_empty() {
                lines.push((format!("PRIMARY KEY ({})", quoted_list(&columns)), None));
            }
        } else if index.name.starts_with("sqlite_autoindex_") {
            if !columns.is_empty() {
                lines.push((format!("UNIQUE ({})", quoted_list(&columns)), None));
            }
        } else if index
            .definition
            .trim_start()
            .to_ascii_uppercase()
            .starts_with("CREATE ")
        {
            trailing_indexes.push(format!(
                "{};",
                index.definition.trim().trim_end_matches(';')
            ));
        } else if !columns.is_empty() {
            let mut statement = format!(
                "CREATE {}INDEX {} ON {reference} ({})",
                if index.is_unique { "UNIQUE " } else { "" },
                quote_identifier(db_type, &index.name),
                columns.join(", ")
            );
            if let Some(predicate) = index.predicate.as_deref().filter(|p| !p.is_empty()) {
                statement.push_str(&format!(" WHERE {predicate}"));
            }
            statement.push(';');
            trailing_indexes.push(statement);
        }
    }

    for fk in &details.foreign_keys {
        lines.push((
            format!(
                "CONSTRAINT {} {}",
                quote_identifier(db_type, &fk.name),
                fk.definition.trim()
            ),
            None,
        ));
    }
    for check in &details.check_constraints {
        let definition = check.definition.trim();
        let body = if definition.to_ascii_uppercase().starts_with("CHECK") {
            definition.to_string()
        } else {
            format!("CHECK ({definition})")
        };
        lines.push((
            format!(
                "CONSTRAINT {} {body}",
                quote_identifier(db_type, &check.name)
            ),
            None,
        ));
    }

    let last = lines.len().saturating_sub(1);
    let body = lines
        .iter()
        .enumerate()
        .map(|(index, (line, note))| {
            let comma = if index == last { "" } else { "," };
            let note = note
                .as_ref()
                .map(|note| format!(" -- {note}"))
                .unwrap_or_default();
            format!("    {line}{comma}{note}")
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut ddl = format!("CREATE TABLE {reference} (\n{body}\n);");
    for statement in trailing_indexes {
        ddl.push_str("\n\n");
        ddl.push_str(&statement);
    }
    ddl
}

// ---------------------------------------------------------------------------
// Explorer model
// ---------------------------------------------------------------------------

/// Case-insensitive match of every whitespace-separated term of `filter`
/// against the table name or its qualified name. An empty filter matches.
pub fn matches_filter(entry: &TableEntry, filter: &str) -> bool {
    let terms = filter_terms(filter);
    if terms.is_empty() {
        return true;
    }
    let name = entry.name.to_lowercase();
    let qualified = entry.qualified_name().to_lowercase();
    terms
        .iter()
        .all(|term| name.contains(term) || qualified.contains(term))
}

fn filter_terms(filter: &str) -> Vec<String> {
    filter.split_whitespace().map(str::to_lowercase).collect()
}

/// Byte ranges of `text` to highlight for `filter` (ASCII case-insensitive,
/// merged and sorted). Ranges always fall on character boundaries.
pub fn highlight_ranges(text: &str, filter: &str) -> Vec<Range<usize>> {
    let haystack = text.to_ascii_lowercase();
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for term in filter_terms(filter) {
        let term = term.to_ascii_lowercase();
        if term.is_empty() {
            continue;
        }
        let mut from = 0;
        while let Some(found) = haystack[from..].find(&term) {
            let start = from + found;
            let end = start + term.len();
            if text.is_char_boundary(start) && text.is_char_boundary(end) {
                ranges.push(start..end);
            }
            from = end;
        }
    }
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// Objects of one kind inside a section (a "Tables (12)" row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplorerGroup {
    /// Header text; `None` when the section has a single kind (no header).
    pub label: Option<String>,
    /// Indices into the entry slice given to [`explorer_sections`].
    pub entries: Vec<usize>,
}

/// A schema (or the flat list, or the system objects) in the explorer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplorerSection {
    /// `None`: the flat user list (no header) — or the "System" section when
    /// `system` is set and the engine has no schemas.
    pub schema: Option<String>,
    pub system: bool,
    /// Number of objects in the section after filtering.
    pub count: usize,
    pub groups: Vec<ExplorerGroup>,
}

impl ExplorerSection {
    /// Header text, or `None` for the flat user list.
    pub fn title(&self) -> Option<String> {
        match (&self.schema, self.system) {
            (Some(schema), _) => Some(schema.clone()),
            (None, true) => Some("System".to_string()),
            (None, false) => None,
        }
    }
}

/// Plural group label for a kind on this engine ("Tables", "Collections",
/// "Indices" for Elasticsearch, …).
pub fn kind_group_label(db_type: &DatabaseType, kind: &TableKind) -> String {
    match kind {
        TableKind::Table => "Tables".to_string(),
        TableKind::Collection => "Collections".to_string(),
        TableKind::Index => {
            if matches!(db_type, DatabaseType::Elasticsearch) {
                "Indices".to_string()
            } else {
                "Indexes".to_string()
            }
        }
        TableKind::View => "Views".to_string(),
        TableKind::MaterializedView => "Materialized views".to_string(),
        TableKind::ForeignTable => "Foreign tables".to_string(),
        TableKind::Other(label) => {
            let mut chars = label.chars();
            let capitalized = match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            };
            format!("{capitalized}s")
        }
    }
}

fn kind_rank(kind: &TableKind) -> u8 {
    match kind {
        TableKind::Table => 0,
        TableKind::Collection => 1,
        TableKind::Index => 2,
        TableKind::View => 3,
        TableKind::MaterializedView => 4,
        TableKind::ForeignTable => 5,
        TableKind::Other(_) => 6,
    }
}

/// Lay `entries` out for the explorer tree.
///
/// - Schemas become sections when the engine reports more than one
///   non-system schema; otherwise the user objects form one flat section.
/// - System objects (catalogs, `sqlite_*`, …) go last, in their own sections.
/// - Inside a section, objects are grouped by kind when it holds several.
/// - `filter` keeps matching objects only (see [`matches_filter`]); the
///   section structure is decided on the unfiltered list so it does not jump
///   while typing.
pub fn explorer_sections(
    db_type: &DatabaseType,
    entries: &[TableEntry],
    filter: &str,
) -> Vec<ExplorerSection> {
    let by_schema = {
        let mut schemas: Vec<&str> = entries
            .iter()
            .filter(|entry| !is_system_object(db_type, entry))
            .filter_map(|entry| entry.schema.as_deref())
            .collect();
        schemas.sort_unstable();
        schemas.dedup();
        schemas.len() > 1
    };

    let mut order: Vec<(Option<String>, bool)> = Vec::new();
    let mut members: HashMap<(Option<String>, bool), Vec<usize>> = HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        if !matches_filter(entry, filter) {
            continue;
        }
        let system = is_system_object(db_type, entry);
        // Schema headers only exist when the engine has several user
        // schemas; otherwise system objects share one "System" section.
        let schema = if by_schema {
            entry.schema.clone()
        } else {
            None
        };
        let key = (schema, system);
        if !members.contains_key(&key) {
            order.push(key.clone());
        }
        members.entry(key).or_default().push(index);
    }
    // User sections first (default schema first, then alphabetical), then
    // system sections.
    order.sort_by(|(a_schema, a_system), (b_schema, b_system)| {
        let is_default = |schema: &Option<String>| {
            schema
                .as_deref()
                .is_some_and(|s| is_default_schema(db_type, s))
        };
        a_system
            .cmp(b_system)
            .then_with(|| is_default(b_schema).cmp(&is_default(a_schema)))
            .then_with(|| a_schema.cmp(b_schema))
    });

    order
        .into_iter()
        .map(|key| {
            let indices = members.remove(&key).unwrap_or_default();
            let (schema, system) = key;
            let mut kinds: Vec<&TableKind> = Vec::new();
            for &index in &indices {
                if !kinds.contains(&&entries[index].kind) {
                    kinds.push(&entries[index].kind);
                }
            }
            kinds.sort_by_key(|kind| kind_rank(kind));
            let groups = if kinds.len() <= 1 {
                vec![ExplorerGroup {
                    label: None,
                    entries: indices.clone(),
                }]
            } else {
                kinds
                    .into_iter()
                    .map(|kind| ExplorerGroup {
                        label: Some(kind_group_label(db_type, kind)),
                        entries: indices
                            .iter()
                            .copied()
                            .filter(|&index| &entries[index].kind == kind)
                            .collect(),
                    })
                    .collect()
            };
            ExplorerSection {
                schema,
                system,
                count: indices.len(),
                groups,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// View classification
// ---------------------------------------------------------------------------

/// DBCrust's `list_tables` reports every relation as a table. These catalog
/// queries return `schema, name` rows of the objects that are really views
/// (and materialized views), so a caller with a connection can reclassify
/// them with [`mark_kinds`]. Engines without a cheap catalog query get none.
pub fn view_queries(db_type: &DatabaseType) -> Vec<(TableKind, &'static str)> {
    match db_type {
        DatabaseType::PostgreSQL => vec![
            (
                TableKind::View,
                "SELECT n.nspname, c.relname FROM pg_catalog.pg_class c \
                 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE c.relkind = 'v'",
            ),
            (
                TableKind::MaterializedView,
                "SELECT n.nspname, c.relname FROM pg_catalog.pg_class c \
                 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE c.relkind = 'm'",
            ),
            (
                TableKind::ForeignTable,
                "SELECT n.nspname, c.relname FROM pg_catalog.pg_class c \
                 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE c.relkind = 'f'",
            ),
        ],
        DatabaseType::MySQL => vec![(
            TableKind::View,
            "SELECT table_schema, table_name FROM information_schema.views \
             WHERE table_schema = DATABASE()",
        )],
        DatabaseType::SQLite | DatabaseType::LibSql => vec![(
            TableKind::View,
            "SELECT 'main', name FROM sqlite_master WHERE type = 'view'",
        )],
        // Late-binding views are in pg_views too
        DatabaseType::Redshift => vec![(
            TableKind::View,
            "SELECT schemaname, viewname FROM pg_views \
             WHERE schemaname NOT IN ('pg_catalog', 'information_schema', 'pg_internal')",
        )],
        DatabaseType::ClickHouse => vec![
            (
                TableKind::View,
                "SELECT database, name FROM system.tables \
                 WHERE engine = 'View' AND database = currentDatabase()",
            ),
            (
                TableKind::MaterializedView,
                "SELECT database, name FROM system.tables \
                 WHERE engine = 'MaterializedView' AND database = currentDatabase()",
            ),
        ],
        _ => Vec::new(),
    }
}

/// Give the entries named by `rows` (`schema, name` data rows of a
/// [`view_queries`] result) the kind `kind`. Rows that match nothing, and
/// entries that are not plain tables, are left alone.
pub fn mark_kinds(
    db_type: &DatabaseType,
    entries: &mut [TableEntry],
    kind: &TableKind,
    rows: &[Vec<String>],
) {
    for row in rows {
        let (Some(schema), Some(name)) = (row.first(), row.get(1)) else {
            continue;
        };
        let (schema, name) = (schema.trim(), name.trim());
        if let Some(entry) = entries.iter_mut().find(|entry| {
            entry.name == name
                && entry.kind == TableKind::Table
                && match entry.schema.as_deref() {
                    Some(existing) => {
                        existing == schema
                            || (is_default_schema(db_type, existing)
                                && is_default_schema(db_type, schema))
                    }
                    None => true,
                }
        }) {
            entry.kind = kind.clone();
        }
    }
}

// ---------------------------------------------------------------------------
// Table outline (inspector)
// ---------------------------------------------------------------------------

/// One column of a table, ready to display.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnOutline {
    pub position: usize,
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub default_value: Option<String>,
    pub flags: ColumnFlags,
    pub enum_values: Vec<String>,
    /// Elasticsearch field capabilities (searchable, aggregatable, …).
    pub capabilities: Vec<String>,
    /// Collation, on engines that report one.
    pub collation: Option<String>,
    /// `table(column)` this column references, when it is a foreign key.
    pub references: Option<String>,
    /// The referenced table alone, as the engine wrote it (possibly
    /// qualified): what [`find_table`] resolves to open its inspector.
    pub reference_table: Option<String>,
    /// Nested field descriptions (MongoDB documents, struct columns).
    pub nested: Vec<String>,
}

/// Columns of `details` with their markers, enum values and nested fields.
pub fn column_outlines(db_type: &DatabaseType, details: &TableDetails) -> Vec<ColumnOutline> {
    let flags = column_flags(details);
    let references: HashMap<String, (String, String)> = foreign_keys(details)
        .into_iter()
        .flat_map(|fk| {
            fk.columns
                .iter()
                .enumerate()
                .map(|(index, column)| {
                    let target = fk
                        .target_columns
                        .get(index)
                        .map(|target| format!("{}({target})", fk.target))
                        .unwrap_or_else(|| fk.target.clone());
                    (column.clone(), (target, fk.target.clone()))
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let elasticsearch = matches!(db_type, DatabaseType::Elasticsearch);
    details
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let collation = column.collation.trim();
            let (capabilities, collation) = if elasticsearch {
                (
                    collation
                        .split(',')
                        .map(|item| item.trim().to_string())
                        .filter(|item| !item.is_empty())
                        .collect(),
                    None,
                )
            } else {
                (
                    Vec::new(),
                    Some(collation.to_string()).filter(|c| !c.is_empty()),
                )
            };
            ColumnOutline {
                position: index + 1,
                name: column.name.clone(),
                data_type: column.data_type.clone(),
                nullable: column.nullable,
                default_value: column.default_value.clone().filter(|d| !d.is_empty()),
                flags: flags.get(&column.name).copied().unwrap_or_default(),
                enum_values: column.enum_values.clone().unwrap_or_default(),
                capabilities,
                collation,
                references: references.get(&column.name).map(|(text, _)| text.clone()),
                reference_table: references.get(&column.name).map(|(_, table)| table.clone()),
                nested: details
                    .nested_field_details
                    .get(&column.name)
                    .cloned()
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// One index of a table, ready to display.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexOutline {
    pub name: String,
    pub primary: bool,
    pub unique: bool,
    /// Access method (`btree`, `hash`, …) as the engine reports it.
    pub method: String,
    pub columns: Vec<String>,
    /// The full `CREATE INDEX` statement, when the engine reports one.
    pub statement: Option<String>,
    pub predicate: Option<String>,
}

pub fn index_outlines(details: &TableDetails) -> Vec<IndexOutline> {
    details
        .indexes
        .iter()
        .map(|index| {
            let statement = index
                .definition
                .trim_start()
                .to_ascii_uppercase()
                .starts_with("CREATE ")
                .then(|| index.definition.trim().to_string());
            IndexOutline {
                name: index.name.clone(),
                primary: index.is_primary,
                unique: index.is_unique,
                method: index.index_type.clone(),
                columns: index_columns(index),
                statement,
                predicate: index.predicate.clone().filter(|p| !p.is_empty()),
            }
        })
        .collect()
}

/// `12 columns · 3 indexes · 2 foreign keys · referenced by 4`, leaving out
/// what the engine does not report or the table does not have.
pub fn describe_counts(db_type: &DatabaseType, details: &TableDetails) -> String {
    let plural = |count: usize, one: &str, many: &str| {
        format!("{count} {}", if count == 1 { one } else { many })
    };
    let noun = match db_type {
        DatabaseType::MongoDB | DatabaseType::Elasticsearch => ("field", "fields"),
        _ => ("column", "columns"),
    };
    let mut parts = vec![plural(details.columns.len(), noun.0, noun.1)];
    if !details.indexes.is_empty() {
        parts.push(plural(details.indexes.len(), "index", "indexes"));
    }
    if !details.foreign_keys.is_empty() {
        parts.push(plural(
            details.foreign_keys.len(),
            "foreign key",
            "foreign keys",
        ));
    }
    if !details.check_constraints.is_empty() {
        parts.push(plural(details.check_constraints.len(), "check", "checks"));
    }
    if !details.referenced_by.is_empty() {
        parts.push(format!("referenced by {}", details.referenced_by.len()));
    }
    parts.join(" · ")
}

/// Whether the engine reports incoming foreign keys. (SQLite, MySQL and
/// ClickHouse describe only the outgoing ones.)
pub fn reports_references(db_type: &DatabaseType) -> bool {
    matches!(db_type, DatabaseType::PostgreSQL | DatabaseType::Redshift)
}

/// The tables of one schema snapshot by name, to resolve many foreign key
/// targets (the table inspector draws one link per foreign key and
/// reference, every frame) without scanning the whole list for each.
/// Resolves exactly like [`find_table`]; build a new one when the snapshot
/// changes ([`TableLookup::is_for`]).
pub struct TableLookup {
    tables: Arc<Vec<TableEntry>>,
    /// Positions in `tables`, in list order, by `TableEntry::name`.
    by_name: HashMap<String, Vec<usize>>,
}

impl TableLookup {
    pub fn new(tables: Arc<Vec<TableEntry>>) -> Self {
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, table) in tables.iter().enumerate() {
            by_name.entry(table.name.clone()).or_default().push(index);
        }
        Self { tables, by_name }
    }

    /// Whether this lookup was built from `tables` (the same snapshot).
    pub fn is_for(&self, tables: &Arc<Vec<TableEntry>>) -> bool {
        Arc::ptr_eq(&self.tables, tables)
    }

    /// [`find_table`] over the snapshot.
    pub fn find(
        &self,
        db_type: &DatabaseType,
        target: &str,
        from_schema: Option<&str>,
    ) -> Option<&TableEntry> {
        let parts = split_qualified(target);
        let (schema, name) = target_parts(&parts)?;
        let named = self.by_name.get(name)?;
        pick_table(
            db_type,
            named.iter().map(|&index| &self.tables[index]),
            schema,
            from_schema,
        )
    }
}

/// Resolve a foreign key target (`customers`, `public.customers`,
/// `"Orders"`) against the table list of a connection.
pub fn find_table<'a>(
    db_type: &DatabaseType,
    tables: &'a [TableEntry],
    target: &str,
    from_schema: Option<&str>,
) -> Option<&'a TableEntry> {
    let parts = split_qualified(target);
    let (schema, name) = target_parts(&parts)?;
    pick_table(
        db_type,
        tables.iter().filter(|entry| entry.name == name),
        schema,
        from_schema,
    )
}

/// The schema (if qualified) and the name of a split target.
fn target_parts(parts: &[String]) -> Option<(Option<&str>, &str)> {
    match parts {
        [] => None,
        [name] => Some((None, name.as_str())),
        [.., schema, name] => Some((Some(schema.as_str()), name.as_str())),
    }
}

/// Among the tables called like the target (in list order): the one in the
/// target's schema; unqualified, the one in the referencing table's schema,
/// else in a default schema, else the first.
fn pick_table<'a>(
    db_type: &DatabaseType,
    named: impl Iterator<Item = &'a TableEntry>,
    schema: Option<&str>,
    from_schema: Option<&str>,
) -> Option<&'a TableEntry> {
    let named: Vec<&TableEntry> = named.collect();
    match schema {
        Some(schema) => named
            .into_iter()
            .find(|entry| entry.schema.as_deref() == Some(schema)),
        None => named
            .iter()
            .find(|entry| entry.schema.as_deref() == from_schema)
            .or_else(|| {
                named.iter().find(|entry| {
                    entry
                        .schema
                        .as_deref()
                        .is_none_or(|schema| is_default_schema(db_type, schema))
                })
            })
            .or(named.first())
            .copied(),
    }
}

/// `a.b."c.d"` → `[a, b, c.d]`
fn split_qualified(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in text.trim().chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => current.push(c),
            (None, '"' | '`') => quote = Some(c),
            (None, '.') => parts.push(std::mem::take(&mut current)),
            (None, c) => current.push(c),
        }
    }
    parts.push(current);
    parts.into_iter().filter(|part| !part.is_empty()).collect()
}

// ---------------------------------------------------------------------------
// Small text helpers
// ---------------------------------------------------------------------------

/// The text between the first `(` at or after `from` and its matching `)`,
/// plus the index just past the `)`.
fn balanced_group(text: &str, from: usize) -> Option<(String, usize)> {
    let open = text.get(from..)?.find('(')? + from;
    let mut depth = 0usize;
    let mut in_quote: Option<char> = None;
    for (offset, c) in text[open..].char_indices() {
        match (in_quote, c) {
            (Some(quote), c) if c == quote => in_quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'' | '`') => in_quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = open + offset;
                    return Some((text[open + 1..end].to_string(), end + 1));
                }
            }
            _ => {}
        }
    }
    None
}

/// Split on commas outside parentheses and quotes; identifiers lose their
/// surrounding quotes.
fn split_top_level(list: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut in_quote: Option<char> = None;
    let mut current = String::new();
    for c in list.chars() {
        match (in_quote, c) {
            (Some(quote), c) if c == quote => {
                in_quote = None;
                current.push(c);
            }
            (Some(_), _) => current.push(c),
            (None, '"' | '\'' | '`') => {
                in_quote = Some(c);
                current.push(c);
            }
            (None, '(') => {
                depth += 1;
                current.push(c);
            }
            (None, ')') => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            (None, ',') if depth == 0 => {
                parts.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    parts.push(current);
    parts
        .into_iter()
        .map(|part| unquote_identifier(part.trim()))
        .filter(|part| !part.is_empty())
        .collect()
}

fn unquote_identifier(text: &str) -> String {
    for quote in ['"', '`'] {
        if let Some(inner) = text
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner.replace(&format!("{quote}{quote}"), &quote.to_string());
        }
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(data: &[&[&str]]) -> Vec<Vec<String>> {
        data.iter()
            .map(|row| row.iter().map(|cell| cell.to_string()).collect())
            .collect()
    }

    #[test]
    fn parses_tables_by_header_and_sorts_system_last() {
        let entries = parse_table_list(
            &DatabaseType::PostgreSQL,
            rows(&[
                &["Schema", "Name", "Type", "Owner"],
                &["public", "users", "table", "app"],
                &["pg_catalog", "pg_class", "table", "postgres"],
                &["analytics", "Orders", "view", "app"],
            ]),
        );
        let names: Vec<_> = entries.iter().map(|e| e.qualified_name()).collect();
        assert_eq!(
            names,
            vec!["analytics.Orders", "public.users", "pg_catalog.pg_class"]
        );
        assert_eq!(entries[0].kind, TableKind::View);
        assert_eq!(entries[1].owner.as_deref(), Some("app"));
    }

    #[test]
    fn parses_database_names() {
        let names = parse_database_list(rows(&[
            &["Name", "Owner", "Encoding"],
            &["app", "app", "UTF8"],
            &["postgres", "postgres", "UTF8"],
        ]));
        assert_eq!(names, vec!["app", "postgres"]);
    }

    #[test]
    fn identifiers_are_quoted_only_when_needed() {
        let pg = DatabaseType::PostgreSQL;
        assert_eq!(quote_identifier(&pg, "users"), "users");
        assert_eq!(quote_identifier(&pg, "Users"), "\"Users\"");
        assert_eq!(quote_identifier(&pg, "order"), "\"order\"");
        assert_eq!(quote_identifier(&pg, "a\"b"), "\"a\"\"b\"");
        assert_eq!(
            quote_identifier(&DatabaseType::MySQL, "my table"),
            "`my table`"
        );
        assert_eq!(quote_identifier(&DatabaseType::SQLite, "Orders"), "Orders");
    }

    #[test]
    fn table_references_skip_default_schemas() {
        let entry = |schema: &str, name: &str| TableEntry {
            schema: Some(schema.into()),
            name: name.into(),
            kind: TableKind::Table,
            owner: None,
        };
        let pg = DatabaseType::PostgreSQL;
        assert_eq!(table_reference(&pg, &entry("public", "users")), "users");
        assert_eq!(
            table_reference(&pg, &entry("analytics", "Events")),
            "analytics.\"Events\""
        );
        assert_eq!(
            table_reference(&DatabaseType::SQLite, &entry("main", "orders")),
            "orders"
        );
    }

    #[test]
    fn empty_output_is_empty() {
        assert!(parse_table_list(&DatabaseType::SQLite, Vec::new()).is_empty());
        assert!(parse_database_list(Vec::new()).is_empty());
    }

    // --- generated SQL, constraints, DDL and explorer model ---

    use dbcrust::db::{
        CheckConstraintInfo, ColumnInfo, ForeignKeyInfo, IndexInfo, ReferencedByInfo,
    };
    use rstest::rstest;

    fn table(schema: Option<&str>, name: &str, kind: TableKind) -> TableEntry {
        TableEntry {
            schema: schema.map(str::to_string),
            name: name.into(),
            kind,
            owner: None,
        }
    }

    fn column(name: &str, data_type: &str, nullable: bool, default: Option<&str>) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: data_type.into(),
            collation: String::new(),
            nullable,
            default_value: default.map(str::to_string),
            enum_values: None,
        }
    }

    fn index(name: &str, definition: &str, primary: bool, unique: bool) -> IndexInfo {
        IndexInfo {
            name: name.into(),
            index_type: "btree".into(),
            is_primary: primary,
            is_unique: unique,
            predicate: None,
            definition: definition.into(),
            constraint_def: None,
        }
    }

    /// A SQLite-flavoured `orders` table.
    fn orders_details() -> TableDetails {
        TableDetails {
            name: "orders".into(),
            schema: "main".into(),
            full_name: "main.orders".into(),
            columns: vec![
                column("id", "INTEGER", false, None),
                column("customer_id", "INTEGER", false, None),
                column("status", "TEXT", false, Some("'pending'")),
                column("shipped_at", "TEXT", true, None),
            ],
            indexes: vec![
                index("orders_customer_idx", "(customer_id)", false, false),
                index("sqlite_autoindex_orders_1", "(id)", true, true),
            ],
            check_constraints: vec![CheckConstraintInfo {
                name: "status_check".into(),
                definition: "status IN ('pending','paid')".into(),
            }],
            foreign_keys: vec![ForeignKeyInfo {
                name: "fk_orders_customer_id".into(),
                definition: "FOREIGN KEY (customer_id) REFERENCES customers(id)".into(),
            }],
            referenced_by: vec![ReferencedByInfo {
                schema: "main".into(),
                table: "order_items".into(),
                constraint_name: "fk".into(),
                definition: "FOREIGN KEY (order_id) REFERENCES orders(id)".into(),
            }],
            nested_field_details: Default::default(),
        }
    }

    #[rstest]
    #[case(DatabaseType::PostgreSQL, Some("public"), "users", "users")]
    #[case(
        DatabaseType::PostgreSQL,
        Some("analytics"),
        "events",
        "analytics.events"
    )]
    #[case(
        DatabaseType::PostgreSQL,
        Some("analytics"),
        "a.b",
        "analytics.\"a.b\""
    )]
    #[case(DatabaseType::SQLite, Some("main"), "orders", "orders")]
    #[case(DatabaseType::SQLite, None, "we.ird", "\"we.ird\"")]
    #[case(DatabaseType::MySQL, Some("shop"), "orders", "shop.orders")]
    #[case(DatabaseType::Elasticsearch, None, "logs-2024.01", "\"logs-2024.01\"")]
    fn describe_names(
        #[case] db_type: DatabaseType,
        #[case] schema: Option<&str>,
        #[case] name: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(
            describe_name(&db_type, &table(schema, name, TableKind::Table)),
            expected
        );
    }

    #[rstest]
    #[case(
        DatabaseType::PostgreSQL,
        Some("analytics"),
        "Events",
        "SELECT * FROM analytics.\"Events\" LIMIT 100;",
        "SELECT COUNT(*) FROM analytics.\"Events\";"
    )]
    #[case(
        DatabaseType::SQLite,
        Some("main"),
        "orders",
        "SELECT * FROM orders LIMIT 100;",
        "SELECT COUNT(*) FROM orders;"
    )]
    #[case(
        DatabaseType::MySQL,
        Some("shop"),
        "order items",
        "SELECT * FROM shop.`order items` LIMIT 100;",
        "SELECT COUNT(*) FROM shop.`order items`;"
    )]
    #[case(
        DatabaseType::MongoDB,
        None,
        "users",
        "db.users.find({})",
        "db.users.aggregate([{\"$count\": \"count\"}])"
    )]
    fn select_and_count(
        #[case] db_type: DatabaseType,
        #[case] schema: Option<&str>,
        #[case] name: &str,
        #[case] select: &str,
        #[case] count: &str,
    ) {
        let entry = table(schema, name, TableKind::Table);
        assert_eq!(select_star_sql(&db_type, &entry, PREVIEW_ROWS), select);
        assert_eq!(count_sql(&db_type, &entry), count);
    }

    #[rstest::rstest]
    #[case("string", "GET \"k\"", "STRLEN \"k\"")]
    #[case("hash", "HSCAN \"k\" 0 COUNT 100", "HLEN \"k\"")]
    #[case("list", "LRANGE \"k\" 0 99", "LLEN \"k\"")]
    #[case("set", "SSCAN \"k\" 0 COUNT 100", "SCARD \"k\"")]
    #[case("zset", "ZRANGE \"k\" 0 99 WITHSCORES", "ZCARD \"k\"")]
    #[case("stream", "XRANGE \"k\" - + COUNT 100", "XLEN \"k\"")]
    #[case("ReJSON-RL", "TYPE \"k\"", "TTL \"k\"")]
    fn redis_keys_preview_with_the_command_of_their_type(
        #[case] kind: &str,
        #[case] select: &str,
        #[case] count: &str,
    ) {
        let entry = table(None, "k", TableKind::from_label(kind));
        assert_eq!(
            select_star_sql(&DatabaseType::Redis, &entry, PREVIEW_ROWS),
            select
        );
        assert_eq!(count_sql(&DatabaseType::Redis, &entry), count);
    }

    #[test]
    fn redis_key_names_are_quoted_for_the_command_line() {
        let entry = table(None, "a \"b\" \\c", TableKind::from_label("string"));
        assert_eq!(
            select_star_sql(&DatabaseType::Redis, &entry, PREVIEW_ROWS),
            "GET \"a \\\"b\\\" \\\\c\""
        );
        // The explorer's column picker has nothing to pick: it previews the key
        assert_eq!(
            select_columns_sql(&DatabaseType::Redis, &entry, &["x".to_string()], 5),
            "GET \"a \\\"b\\\" \\\\c\""
        );
    }

    #[test]
    fn sql_server_previews_use_top_and_brackets() {
        let sql_server = DatabaseType::SqlServer;
        let entry = |schema: &str, name: &str| TableEntry {
            schema: Some(schema.into()),
            name: name.into(),
            kind: TableKind::Table,
            owner: None,
        };
        assert_eq!(
            select_star_sql(&sql_server, &entry("dbo", "Orders"), 100),
            "SELECT TOP (100) * FROM Orders;"
        );
        assert_eq!(
            select_star_sql(&sql_server, &entry("Sales", "Order Items"), 100),
            "SELECT TOP (100) * FROM Sales.[Order Items];"
        );
        assert_eq!(
            select_columns_sql(
                &sql_server,
                &entry("dbo", "T"),
                &["id".to_string(), "a]b".to_string()],
                5
            ),
            "SELECT TOP (5)\n    id,\n    [a]]b]\nFROM T;"
        );
        assert_eq!(
            count_sql(&sql_server, &entry("Sales", "Orders")),
            "SELECT COUNT(*) FROM Sales.Orders;"
        );
    }

    #[test]
    fn select_columns_lists_one_column_per_line() {
        let entry = table(Some("main"), "orders", TableKind::Table);
        let columns = vec!["id".to_string(), "Order Total".to_string()];
        assert_eq!(
            select_columns_sql(&DatabaseType::SQLite, &entry, &columns, 50),
            "SELECT\n    id,\n    \"Order Total\"\nFROM orders\nLIMIT 50;"
        );
        assert_eq!(
            select_columns_sql(&DatabaseType::SQLite, &entry, &[], 50),
            "SELECT * FROM orders LIMIT 50;"
        );
    }

    #[test]
    fn insert_template_uses_types_and_notes() {
        let details = orders_details();
        let entry = table(Some("main"), "orders", TableKind::Table);
        let sql = insert_template_sql(&DatabaseType::SQLite, &entry, &details.columns)
            .expect("sqlite supports inserts");
        assert_eq!(
            sql,
            "INSERT INTO orders (\n    id,\n    customer_id,\n    status,\n    shipped_at\n)\nVALUES (\n    0, -- id: INTEGER, not null\n    0, -- customer_id: INTEGER, not null\n    NULL, -- status: TEXT, not null, default 'pending'\n    NULL  -- shipped_at: TEXT\n);"
        );
        assert!(insert_template_sql(&DatabaseType::MongoDB, &entry, &details.columns).is_none());
        assert!(insert_template_sql(&DatabaseType::Parquet, &entry, &details.columns).is_none());
        assert!(insert_template_sql(&DatabaseType::SQLite, &entry, &[]).is_none());
    }

    #[rstest]
    #[case(
        "FOREIGN KEY (customer_id) REFERENCES customers(id)",
        &["customer_id"], "customers", &["id"], ""
    )]
    #[case(
        "FOREIGN KEY (a, b) REFERENCES public.items (x, y) ON DELETE CASCADE",
        &["a", "b"], "public.items", &["x", "y"], "ON DELETE CASCADE"
    )]
    #[case(
        "FOREIGN KEY (\"Order Id\") REFERENCES \"Orders\"(\"id\")",
        &["Order Id"], "\"Orders\"", &["id"], ""
    )]
    fn parses_foreign_keys(
        #[case] definition: &str,
        #[case] columns: &[&str],
        #[case] target: &str,
        #[case] target_columns: &[&str],
        #[case] actions: &str,
    ) {
        let fk = parse_foreign_key("fk", definition).expect("parses");
        assert_eq!(fk.columns, columns);
        assert_eq!(fk.target, target);
        assert_eq!(fk.target_columns, target_columns);
        assert_eq!(fk.actions, actions);
    }

    #[test]
    fn unparseable_foreign_keys_are_skipped() {
        assert!(parse_foreign_key("x", "CHECK (a > 0)").is_none());
        assert!(parse_foreign_key("x", "FOREIGN KEY (a) REFERENCES").is_none());
        assert!(parse_foreign_key("x", "FOREIGN KEY (a").is_none());
    }

    #[rstest]
    #[case("CREATE UNIQUE INDEX orders_pkey ON public.orders USING btree (id)", &["id"])]
    #[case("CREATE INDEX i ON public.t USING btree (a, b DESC) WHERE (c > 1)", &["a", "b DESC"])]
    #[case("CREATE INDEX i ON t USING btree (lower(email), \"Name\")", &["lower(email)", "Name"])]
    #[case("(customer_id, ordered_at)", &["customer_id", "ordered_at"])]
    #[case("", &[])]
    fn index_column_lists(#[case] definition: &str, #[case] expected: &[&str]) {
        assert_eq!(
            index_columns(&index("i", definition, false, false)),
            expected
        );
    }

    #[test]
    fn column_markers() {
        let details = orders_details();
        assert_eq!(primary_key_columns(&details), vec!["id"]);
        let flags = column_flags(&details);
        assert!(flags["id"].primary_key && flags["id"].not_null);
        assert!(flags["customer_id"].foreign_key && !flags["customer_id"].primary_key);
        assert!(!flags["shipped_at"].not_null);
        assert_eq!(
            foreign_keys(&details)[0].summary(),
            "customer_id → customers(id)"
        );
    }

    #[test]
    fn ddl_includes_keys_constraints_and_indexes() {
        let ddl = ddl_summary(&DatabaseType::SQLite, &orders_details());
        assert_eq!(
            ddl,
            "CREATE TABLE orders (\n    id INTEGER NOT NULL,\n    customer_id INTEGER NOT NULL,\n    status TEXT NOT NULL DEFAULT 'pending',\n    shipped_at TEXT,\n    PRIMARY KEY (id),\n    CONSTRAINT fk_orders_customer_id FOREIGN KEY (customer_id) REFERENCES customers(id),\n    CONSTRAINT status_check CHECK (status IN ('pending','paid'))\n);\n\nCREATE INDEX orders_customer_idx ON orders (customer_id);"
        );
    }

    #[test]
    fn ddl_reuses_postgres_index_statements_and_qualifies_schema() {
        let mut details = orders_details();
        details.schema = "shop".into();
        details.indexes = vec![
            index(
                "orders_pkey",
                "CREATE UNIQUE INDEX orders_pkey ON shop.orders USING btree (id)",
                true,
                true,
            ),
            index(
                "orders_status_idx",
                "CREATE INDEX orders_status_idx ON shop.orders USING btree (status)",
                false,
                false,
            ),
        ];
        details.columns[2].enum_values = Some(vec!["pending".into(), "paid".into()]);
        let ddl = ddl_summary(&DatabaseType::PostgreSQL, &details);
        assert!(ddl.starts_with("CREATE TABLE shop.orders ("), "{ddl}");
        assert!(ddl.contains("    PRIMARY KEY (id),"), "{ddl}");
        assert!(
            ddl.contains("status TEXT NOT NULL DEFAULT 'pending', -- enum: pending, paid"),
            "{ddl}"
        );
        assert!(
            ddl.ends_with("CREATE INDEX orders_status_idx ON shop.orders USING btree (status);"),
            "{ddl}"
        );
        assert!(
            !ddl.contains("orders_pkey"),
            "primary key index is a constraint: {ddl}"
        );
    }

    #[test]
    fn ddl_of_a_table_without_columns_is_still_valid_text() {
        let mut details = orders_details();
        details.columns.clear();
        details.indexes.clear();
        details.check_constraints.clear();
        details.foreign_keys.clear();
        assert_eq!(
            ddl_summary(&DatabaseType::SQLite, &details),
            "CREATE TABLE orders (\n\n);"
        );
    }

    #[rstest]
    #[case("", true)]
    #[case("ord", true)]
    #[case("ORD", true)]
    #[case("sales ord", true)]
    #[case("sales.ord", true)]
    #[case("ord cust", false)]
    #[case("nope", false)]
    fn filter_matching(#[case] filter: &str, #[case] expected: bool) {
        let entry = table(Some("sales"), "orders", TableKind::Table);
        assert_eq!(matches_filter(&entry, filter), expected);
    }

    #[test]
    fn highlights_are_merged_and_boundary_safe() {
        assert_eq!(
            highlight_ranges("order_items", "ord ITEM"),
            vec![0..3, 6..10]
        );
        assert_eq!(highlight_ranges("order_items", "or ord"), vec![0..3]);
        assert_eq!(highlight_ranges("café_orders", "é"), vec![3..5]);
        assert_eq!(highlight_ranges("café_orders", "ord"), vec![6..9]);
        assert!(highlight_ranges("anything", "").is_empty());
    }

    fn names(entries: &[TableEntry], group: &ExplorerGroup) -> Vec<String> {
        group
            .entries
            .iter()
            .map(|&i| entries[i].qualified_name())
            .collect()
    }

    #[test]
    fn single_schema_engines_stay_flat_with_kind_groups() {
        let entries = parse_table_list(
            &DatabaseType::SQLite,
            rows(&[
                &["Schema", "Name", "Type", "Owner"],
                &["main", "orders", "table", ""],
                &["main", "customers", "table", ""],
                &["main", "revenue", "view", ""],
                &["main", "sqlite_sequence", "table", ""],
            ]),
        );
        let sections = explorer_sections(&DatabaseType::SQLite, &entries, "");
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title(), None);
        assert_eq!(sections[0].count, 3);
        let labels: Vec<_> = sections[0].groups.iter().map(|g| g.label.clone()).collect();
        assert_eq!(
            labels,
            vec![Some("Tables".to_string()), Some("Views".to_string())]
        );
        assert_eq!(
            names(&entries, &sections[0].groups[0]),
            vec!["main.customers", "main.orders"]
        );
        assert!(sections[1].system);
        assert_eq!(sections[1].title(), Some("System".to_string()));
        assert_eq!(sections[1].groups[0].label, None);
    }

    #[test]
    fn several_schemas_become_sections_system_last() {
        let entries = parse_table_list(
            &DatabaseType::PostgreSQL,
            rows(&[
                &["Schema", "Name", "Type", "Owner"],
                &["public", "users", "table", ""],
                &["analytics", "events", "table", ""],
                &["analytics", "daily", "view", ""],
                &["pg_catalog", "pg_class", "table", ""],
            ]),
        );
        let sections = explorer_sections(&DatabaseType::PostgreSQL, &entries, "");
        let titles: Vec<_> = sections.iter().map(|s| s.title()).collect();
        assert_eq!(
            titles,
            vec![
                Some("public".into()),
                Some("analytics".into()),
                Some("pg_catalog".into())
            ]
        );
        assert!(sections[2].system);

        // filtering keeps the structure but drops empty sections
        let filtered = explorer_sections(&DatabaseType::PostgreSQL, &entries, "event");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].title(), Some("analytics".into()));
        assert_eq!(filtered[0].count, 1);
        assert_eq!(filtered[0].groups[0].label, None);
    }

    #[test]
    fn one_user_schema_is_flat_even_if_not_default() {
        let entries = vec![
            table(Some("shop"), "orders", TableKind::Table),
            table(Some("shop"), "items", TableKind::Table),
        ];
        let sections = explorer_sections(&DatabaseType::MySQL, &entries, "");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title(), None);
        assert_eq!(sections[0].count, 2);
    }

    #[test]
    fn collections_and_indices_get_engine_labels() {
        assert_eq!(
            kind_group_label(&DatabaseType::MongoDB, &TableKind::Collection),
            "Collections"
        );
        assert_eq!(
            kind_group_label(&DatabaseType::Elasticsearch, &TableKind::Index),
            "Indices"
        );
        assert_eq!(
            kind_group_label(
                &DatabaseType::PostgreSQL,
                &TableKind::Other("sequence".into())
            ),
            "Sequences"
        );
        assert_eq!(
            kind_group_label(&DatabaseType::PostgreSQL, &TableKind::MaterializedView),
            "Materialized views"
        );
    }

    #[rstest::rstest]
    #[case("1234", Some(1234))]
    #[case(" 1,234 ", Some(1234))]
    #[case("1234.0", Some(1234))]
    #[case("{\"count\": 77}", Some(77))]
    #[case("{ count: 5 }", Some(5))]
    #[case("many", None)]
    #[case("-3", None)]
    fn counts_are_read_from_the_first_cell(#[case] cell: &str, #[case] expected: Option<u64>) {
        assert_eq!(parse_count(&[vec![Some(cell.to_string())]]), expected);
    }

    #[test]
    fn a_missing_count_cell_is_none() {
        assert_eq!(parse_count(&[]), None);
        assert_eq!(parse_count(&[vec![]]), None);
        assert_eq!(parse_count(&[vec![None]]), None);
    }

    #[rstest::rstest]
    #[case(0, "0")]
    #[case(999, "999")]
    #[case(1_000, "1,000")]
    #[case(1_234_567, "1,234,567")]
    #[case(12_345_678_901, "12,345,678,901")]
    fn digits_are_grouped_by_three(#[case] value: u64, #[case] expected: &str) {
        assert_eq!(group_digits(value), expected);
    }

    #[test]
    fn outlines_combine_markers_enums_and_capabilities() {
        let mut details = orders_details();
        details.columns[2].enum_values = Some(vec!["pending".into(), "paid".into()]);
        details.columns[3].collation = "en_US".into();
        details
            .nested_field_details
            .insert("status".into(), vec!["code: string".into()]);
        let outlines = column_outlines(&DatabaseType::PostgreSQL, &details);
        assert_eq!(outlines.len(), 4);
        assert_eq!(outlines[0].position, 1);
        assert!(outlines[0].flags.primary_key);
        assert_eq!(outlines[1].references.as_deref(), Some("customers(id)"));
        assert_eq!(outlines[1].reference_table.as_deref(), Some("customers"));
        assert_eq!(outlines[0].reference_table, None);
        assert_eq!(outlines[2].enum_values, vec!["pending", "paid"]);
        assert_eq!(outlines[2].nested, vec!["code: string"]);
        assert_eq!(outlines[2].default_value.as_deref(), Some("'pending'"));
        assert_eq!(outlines[3].collation.as_deref(), Some("en_US"));

        details.columns[3].collation = "searchable, aggregatable".into();
        let es = column_outlines(&DatabaseType::Elasticsearch, &details);
        assert_eq!(es[3].capabilities, vec!["searchable", "aggregatable"]);
        assert_eq!(es[3].collation, None);
    }

    #[test]
    fn index_outlines_keep_full_statements_only() {
        let mut details = orders_details();
        details.indexes.push(index(
            "orders_pg",
            "CREATE INDEX orders_pg ON public.orders USING btree (status)",
            false,
            false,
        ));
        let outlines = index_outlines(&details);
        assert_eq!(outlines[0].columns, vec!["customer_id"]);
        assert_eq!(outlines[0].statement, None);
        assert!(outlines[1].primary && outlines[1].unique);
        assert_eq!(outlines[2].columns, vec!["status"]);
        assert!(
            outlines[2]
                .statement
                .as_deref()
                .is_some_and(|s| s.starts_with("CREATE INDEX"))
        );
    }

    #[test]
    fn counts_skip_what_is_absent() {
        let details = orders_details();
        assert_eq!(
            describe_counts(&DatabaseType::SQLite, &details),
            "4 columns · 2 indexes · 1 foreign key · 1 check · referenced by 1"
        );
        let mut bare = orders_details();
        bare.indexes.clear();
        bare.foreign_keys.clear();
        bare.check_constraints.clear();
        bare.referenced_by.clear();
        bare.columns.truncate(1);
        assert_eq!(
            describe_counts(&DatabaseType::Elasticsearch, &bare),
            "1 field"
        );
        assert!(reports_references(&DatabaseType::PostgreSQL));
        assert!(!reports_references(&DatabaseType::SQLite));
    }

    #[rstest]
    #[case("customers", None, Some("public.customers"))]
    #[case("analytics.customers", None, Some("analytics.customers"))]
    #[case("\"Orders\"", None, Some("public.Orders"))]
    #[case("customers", Some("analytics"), Some("analytics.customers"))]
    #[case("missing", None, None)]
    fn finds_foreign_key_targets(
        #[case] target: &str,
        #[case] from: Option<&str>,
        #[case] expected: Option<&str>,
    ) {
        let tables = vec![
            table(Some("analytics"), "customers", TableKind::Table),
            table(Some("public"), "customers", TableKind::Table),
            table(Some("public"), "Orders", TableKind::Table),
        ];
        let found = find_table(&DatabaseType::PostgreSQL, &tables, target, from)
            .map(|entry| entry.qualified_name());
        assert_eq!(found.as_deref(), expected);
        let lookup = TableLookup::new(Arc::new(tables));
        let looked_up = lookup
            .find(&DatabaseType::PostgreSQL, target, from)
            .map(|entry| entry.qualified_name());
        assert_eq!(looked_up, found);
    }

    #[test]
    fn the_lookup_resolves_like_a_scan() {
        let tables = Arc::new(vec![
            table(None, "orders", TableKind::Table),
            table(Some("main"), "orders", TableKind::Table),
            table(Some("sales"), "orders", TableKind::Table),
            table(Some("public"), "orders", TableKind::Table),
            table(Some("sales"), "Line.Items", TableKind::Table),
            table(Some("Sales"), "items", TableKind::View),
            table(Some("public"), "items", TableKind::Table),
        ]);
        let lookup = TableLookup::new(tables.clone());
        assert!(lookup.is_for(&tables));
        assert!(!lookup.is_for(&Arc::new(tables.as_ref().clone())));
        let targets = [
            "orders",
            "sales.orders",
            "\"sales\".\"orders\"",
            "db.sales.orders",
            "missing.orders",
            "\"Line.Items\"",
            "sales.\"Line.Items\"",
            "items",
            "Sales.items",
            "ITEMS",
            "",
            ".",
        ];
        for db_type in [
            DatabaseType::PostgreSQL,
            DatabaseType::SQLite,
            DatabaseType::MySQL,
        ] {
            for target in targets {
                for from in [None, Some("sales"), Some("main"), Some("Sales")] {
                    assert_eq!(
                        lookup.find(&db_type, target, from),
                        find_table(&db_type, &tables, target, from),
                        "{db_type:?} {target} from {from:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn views_are_reclassified_from_catalog_rows() {
        let mut entries = parse_table_list(
            &DatabaseType::SQLite,
            rows(&[
                &["Schema", "Name", "Type", "Owner"],
                &["main", "orders", "table", "unknown"],
                &["main", "revenue_by_month", "table", "unknown"],
            ]),
        );
        assert_eq!(entries[0].owner, None);
        let queries = view_queries(&DatabaseType::SQLite);
        assert_eq!(queries.len(), 1);
        mark_kinds(
            &DatabaseType::SQLite,
            &mut entries,
            &queries[0].0,
            &rows(&[&["main", "revenue_by_month"], &["main", "ghost"], &["x"]]),
        );
        let kinds: Vec<_> = entries
            .iter()
            .map(|e| (e.name.as_str(), e.kind.clone()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("orders", TableKind::Table),
                ("revenue_by_month", TableKind::View)
            ]
        );
        assert!(view_queries(&DatabaseType::Parquet).is_empty());
        assert_eq!(view_queries(&DatabaseType::PostgreSQL).len(), 3);
    }
}
