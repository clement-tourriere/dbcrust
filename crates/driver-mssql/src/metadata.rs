//! Schema browsing from the catalog views of the current database
//! (`sys.schemas`, `sys.objects`, `sys.columns`, `sys.indexes`,
//! `sys.foreign_keys`, `sys.check_constraints`).
//!
//! Every statement is a constant: user-controlled names (schema, table) travel
//! as `@P1`/`@P2` parameters, never as SQL text. Objects are addressed through
//! `OBJECT_ID(@P1)` with a bracket-quoted `[schema].[table]` string.

use crate::conn::Shared;
use async_trait::async_trait;
use dbcrust_core::database::{DatabaseError, MetadataProvider};
use dbcrust_core::model::{
    CheckConstraintInfo, ColumnInfo, ForeignKeyInfo, IndexInfo, ReferencedByInfo, TableDetails,
};
use std::collections::HashMap;
use std::sync::Arc;

/// The schema an unqualified name belongs to when the login has no other.
pub(crate) const DEFAULT_SCHEMA: &str = "dbo";

const SCHEMAS_SQL: &str = "\
SELECT s.name
FROM sys.schemas AS s
WHERE s.schema_id < 16384
  AND s.name NOT IN (N'sys', N'INFORMATION_SCHEMA', N'guest')
ORDER BY s.name";

const TABLES_ALL_SQL: &str = "\
SELECT s.name AS schema_name, o.name AS object_name
FROM sys.objects AS o
JOIN sys.schemas AS s ON s.schema_id = o.schema_id
WHERE o.type IN ('U', 'V') AND o.is_ms_shipped = 0
ORDER BY s.name, o.name";

const TABLES_IN_SCHEMA_SQL: &str = "\
SELECT s.name AS schema_name, o.name AS object_name
FROM sys.objects AS o
JOIN sys.schemas AS s ON s.schema_id = o.schema_id
WHERE o.type IN ('U', 'V') AND o.is_ms_shipped = 0 AND s.name = @P1
ORDER BY o.name";

const FUNCTIONS_ALL_SQL: &str = "\
SELECT s.name AS schema_name, o.name AS object_name
FROM sys.objects AS o
JOIN sys.schemas AS s ON s.schema_id = o.schema_id
WHERE o.type IN ('FN', 'IF', 'TF', 'P') AND o.is_ms_shipped = 0
ORDER BY s.name, o.name";

const FUNCTIONS_IN_SCHEMA_SQL: &str = "\
SELECT s.name AS schema_name, o.name AS object_name
FROM sys.objects AS o
JOIN sys.schemas AS s ON s.schema_id = o.schema_id
WHERE o.type IN ('FN', 'IF', 'TF', 'P') AND o.is_ms_shipped = 0 AND s.name = @P1
ORDER BY o.name";

/// The schema that holds table `@P1`: the login's default schema when it has
/// one by that name, otherwise the first schema alphabetically.
const RESOLVE_SCHEMA_SQL: &str = "\
SELECT TOP (1) s.name
FROM sys.objects AS o
JOIN sys.schemas AS s ON s.schema_id = o.schema_id
WHERE o.name = @P1 AND o.type IN ('U', 'V')
ORDER BY CASE WHEN s.schema_id = SCHEMA_ID() THEN 0 ELSE 1 END, s.name";

const COLUMN_NAMES_SQL: &str = "\
SELECT c.name
FROM sys.columns AS c
WHERE c.object_id = OBJECT_ID(@P1)
ORDER BY c.column_id";

const COLUMNS_SQL: &str = "\
SELECT c.name,
       TYPE_NAME(c.user_type_id) AS type_name,
       c.max_length, c.precision, c.scale,
       c.is_nullable, c.is_identity, c.is_computed,
       c.collation_name,
       OBJECT_DEFINITION(c.default_object_id) AS default_definition,
       cc.definition AS computed_definition,
       CONVERT(nvarchar(40), ic.seed_value) AS seed_value,
       CONVERT(nvarchar(40), ic.increment_value) AS increment_value
FROM sys.columns AS c
LEFT JOIN sys.computed_columns AS cc
       ON cc.object_id = c.object_id AND cc.column_id = c.column_id
LEFT JOIN sys.identity_columns AS ic
       ON ic.object_id = c.object_id AND ic.column_id = c.column_id
WHERE c.object_id = OBJECT_ID(@P1)
ORDER BY c.column_id";

const INDEXES_SQL: &str = "\
SELECT i.name, i.type_desc, i.is_primary_key, i.is_unique, i.is_unique_constraint,
       i.filter_definition,
       STUFF((SELECT N', ' + QUOTENAME(c.name)
                     + CASE WHEN ic.is_descending_key = 1 THEN N' DESC' ELSE N'' END
              FROM sys.index_columns AS ic
              JOIN sys.columns AS c
                ON c.object_id = ic.object_id AND c.column_id = ic.column_id
              WHERE ic.object_id = i.object_id AND ic.index_id = i.index_id
                AND ic.is_included_column = 0
              ORDER BY ic.key_ordinal
              FOR XML PATH(N''), TYPE).value(N'.', N'nvarchar(max)'), 1, 2, N'') AS key_columns,
       STUFF((SELECT N', ' + QUOTENAME(c.name)
              FROM sys.index_columns AS ic
              JOIN sys.columns AS c
                ON c.object_id = ic.object_id AND c.column_id = ic.column_id
              WHERE ic.object_id = i.object_id AND ic.index_id = i.index_id
                AND ic.is_included_column = 1
              ORDER BY ic.index_column_id
              FOR XML PATH(N''), TYPE).value(N'.', N'nvarchar(max)'), 1, 2, N'') AS included_columns
FROM sys.indexes AS i
WHERE i.object_id = OBJECT_ID(@P1) AND i.type > 0 AND i.is_hypothetical = 0
ORDER BY i.is_primary_key DESC, i.is_unique_constraint DESC, i.name";

const CHECKS_SQL: &str = "\
SELECT cc.name, cc.definition
FROM sys.check_constraints AS cc
WHERE cc.parent_object_id = OBJECT_ID(@P1)
ORDER BY cc.name";

/// Outgoing foreign keys of `@P1` (columns of this table, then the referenced
/// table and its columns).
const FOREIGN_KEYS_SQL: &str = "\
SELECT fk.name,
       STUFF((SELECT N', ' + QUOTENAME(pc.name)
              FROM sys.foreign_key_columns AS fkc
              JOIN sys.columns AS pc
                ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id
              WHERE fkc.constraint_object_id = fk.object_id
              ORDER BY fkc.constraint_column_id
              FOR XML PATH(N''), TYPE).value(N'.', N'nvarchar(max)'), 1, 2, N'') AS columns_list,
       rs.name AS referenced_schema, rt.name AS referenced_table,
       STUFF((SELECT N', ' + QUOTENAME(rc.name)
              FROM sys.foreign_key_columns AS fkc
              JOIN sys.columns AS rc
                ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id
              WHERE fkc.constraint_object_id = fk.object_id
              ORDER BY fkc.constraint_column_id
              FOR XML PATH(N''), TYPE).value(N'.', N'nvarchar(max)'), 1, 2, N'') AS referenced_columns,
       fk.delete_referential_action_desc, fk.update_referential_action_desc
FROM sys.foreign_keys AS fk
JOIN sys.tables AS rt ON rt.object_id = fk.referenced_object_id
JOIN sys.schemas AS rs ON rs.schema_id = rt.schema_id
WHERE fk.parent_object_id = OBJECT_ID(@P1)
ORDER BY fk.name";

/// Foreign keys of other tables that point at `@P1`.
const REFERENCED_BY_SQL: &str = "\
SELECT fk.name, ps.name AS parent_schema, pt.name AS parent_table,
       STUFF((SELECT N', ' + QUOTENAME(pc.name)
              FROM sys.foreign_key_columns AS fkc
              JOIN sys.columns AS pc
                ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id
              WHERE fkc.constraint_object_id = fk.object_id
              ORDER BY fkc.constraint_column_id
              FOR XML PATH(N''), TYPE).value(N'.', N'nvarchar(max)'), 1, 2, N'') AS columns_list,
       STUFF((SELECT N', ' + QUOTENAME(rc.name)
              FROM sys.foreign_key_columns AS fkc
              JOIN sys.columns AS rc
                ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id
              WHERE fkc.constraint_object_id = fk.object_id
              ORDER BY fkc.constraint_column_id
              FOR XML PATH(N''), TYPE).value(N'.', N'nvarchar(max)'), 1, 2, N'') AS referenced_columns
FROM sys.foreign_keys AS fk
JOIN sys.tables AS pt ON pt.object_id = fk.parent_object_id
JOIN sys.schemas AS ps ON ps.schema_id = pt.schema_id
WHERE fk.referenced_object_id = OBJECT_ID(@P1)
ORDER BY ps.name, pt.name, fk.name";

/// `[name]` with `]` doubled, the T-SQL quoted identifier.
pub(crate) fn quote_name(name: &str) -> String {
    format!("[{}]", name.replace(']', "]]"))
}

/// `[schema].[table]`, the string `OBJECT_ID` resolves.
pub(crate) fn object_name(schema: &str, table: &str) -> String {
    format!("{}.{}", quote_name(schema), quote_name(table))
}

/// One part of a packed `schema.table` string: quoted only when a dot or
/// bracket in the name would make the packed form ambiguous (the splitter in
/// `dbcrust_core::model::metadata_table_parts` understands `[..]`).
fn pack_part(part: &str) -> String {
    if part.contains(['.', '[', ']']) {
        quote_name(part)
    } else {
        part.to_string()
    }
}

/// How an object is listed: unqualified inside the schema being browsed or in
/// `dbo`, `schema.name` otherwise.
pub(crate) fn pack_object(schema: &str, name: &str, browsing_schema: Option<&str>) -> String {
    if browsing_schema.is_some() || schema == DEFAULT_SCHEMA {
        pack_part(name)
    } else {
        format!("{}.{}", pack_part(schema), pack_part(name))
    }
}

/// `nvarchar(50)`, `decimal(18,2)`, `datetime2(3)`, `varbinary(max)`… from the
/// catalog's name, byte length, precision and scale.
pub(crate) fn format_sql_type(
    type_name: &str,
    max_length: i32,
    precision: i32,
    scale: i32,
) -> String {
    let lower = type_name.to_ascii_lowercase();
    match lower.as_str() {
        "nchar" | "nvarchar" => match max_length {
            -1 => format!("{type_name}(max)"),
            bytes => format!("{type_name}({})", bytes / 2),
        },
        "char" | "varchar" | "binary" | "varbinary" => match max_length {
            -1 => format!("{type_name}(max)"),
            bytes => format!("{type_name}({bytes})"),
        },
        "decimal" | "numeric" => format!("{type_name}({precision},{scale})"),
        "datetime2" | "time" | "datetimeoffset" => format!("{type_name}({scale})"),
        "float" if precision != 53 => format!("{type_name}({precision})"),
        _ => type_name.to_string(),
    }
}

/// A default constraint's definition without the redundant outer parentheses
/// SQL Server stores (`((0))` → `0`, `(getdate())` → `getdate()`).
pub(crate) fn clean_default(definition: &str) -> String {
    let mut text = definition.trim();
    while text.len() >= 2 && text.starts_with('(') && text.ends_with(')') && balanced_outer(text) {
        text = text[1..text.len() - 1].trim();
    }
    text.to_string()
}

/// Whether the first `(` of `text` closes at its last character.
fn balanced_outer(text: &str) -> bool {
    let mut depth = 0i32;
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return i == text.len() - 1;
                }
            }
            _ => {}
        }
    }
    false
}

/// The `CREATE INDEX` statement of an index, for `\d`.
pub(crate) fn index_definition(
    schema: &str,
    table: &str,
    name: &str,
    type_desc: &str,
    unique: bool,
    key_columns: &str,
    included: Option<&str>,
    filter: Option<&str>,
) -> String {
    let mut sql = format!(
        "CREATE {}{} INDEX {} ON {} ({key_columns})",
        if unique { "UNIQUE " } else { "" },
        type_desc,
        quote_name(name),
        object_name(schema, table),
    );
    if let Some(included) = included.filter(|c| !c.is_empty()) {
        sql.push_str(&format!(" INCLUDE ({included})"));
    }
    if let Some(filter) = filter.filter(|f| !f.is_empty()) {
        sql.push_str(&format!(" WHERE {filter}"));
    }
    sql
}

/// `FOREIGN KEY (a, b) REFERENCES [s].[t] (x, y) ON DELETE CASCADE`.
pub(crate) fn foreign_key_definition(
    columns: &str,
    referenced_schema: &str,
    referenced_table: &str,
    referenced_columns: &str,
    on_delete: &str,
    on_update: &str,
) -> String {
    let mut sql = format!(
        "FOREIGN KEY ({columns}) REFERENCES {} ({referenced_columns})",
        object_name(referenced_schema, referenced_table)
    );
    for (clause, action) in [("DELETE", on_delete), ("UPDATE", on_update)] {
        if !action.is_empty() && action != "NO_ACTION" {
            sql.push_str(&format!(" ON {clause} {}", action.replace('_', " ")));
        }
    }
    sql
}

type Cells = Vec<Option<String>>;

fn text(row: &Cells, index: usize) -> String {
    row.get(index).cloned().flatten().unwrap_or_default()
}

fn optional(row: &Cells, index: usize) -> Option<String> {
    row.get(index).cloned().flatten().filter(|v| !v.is_empty())
}

fn flag(row: &Cells, index: usize) -> bool {
    matches!(row.get(index), Some(Some(v)) if v == "1")
}

fn int(row: &Cells, index: usize) -> i32 {
    row.get(index)
        .and_then(|v| v.as_deref())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Catalog browsing over the client's session.
pub struct MssqlMetadataProvider {
    shared: Arc<Shared>,
}

impl MssqlMetadataProvider {
    pub(crate) fn new(shared: Arc<Shared>) -> Self {
        Self { shared }
    }

    /// The schema of `table` when the caller gave none.
    async fn resolve_schema(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<String, DatabaseError> {
        if let Some(schema) = schema {
            return Ok(schema.to_string());
        }
        let rows = self
            .shared
            .query_params(RESOLVE_SCHEMA_SQL, &[table])
            .await?;
        Ok(rows
            .first()
            .map(|row| text(row, 0))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| DEFAULT_SCHEMA.to_string()))
    }
}

#[async_trait]
impl MetadataProvider for MssqlMetadataProvider {
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        let rows = self.shared.query_params(SCHEMAS_SQL, &[]).await?;
        Ok(rows.iter().map(|row| text(row, 0)).collect())
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        let rows = match schema {
            Some(schema) => {
                self.shared
                    .query_params(TABLES_IN_SCHEMA_SQL, &[schema])
                    .await?
            }
            None => self.shared.query_params(TABLES_ALL_SQL, &[]).await?,
        };
        Ok(rows
            .iter()
            .map(|row| pack_object(&text(row, 0), &text(row, 1), schema))
            .collect())
    }

    async fn get_columns(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<Vec<String>, DatabaseError> {
        let schema = self.resolve_schema(table, schema).await?;
        let rows = self
            .shared
            .query_params(COLUMN_NAMES_SQL, &[&object_name(&schema, table)])
            .await?;
        Ok(rows.iter().map(|row| text(row, 0)).collect())
    }

    async fn get_functions(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        let rows = match schema {
            Some(schema) => {
                self.shared
                    .query_params(FUNCTIONS_IN_SCHEMA_SQL, &[schema])
                    .await?
            }
            None => self.shared.query_params(FUNCTIONS_ALL_SQL, &[]).await?,
        };
        Ok(rows
            .iter()
            .map(|row| pack_object(&text(row, 0), &text(row, 1), schema))
            .collect())
    }

    async fn get_table_details(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<TableDetails, DatabaseError> {
        let schema = self.resolve_schema(table, schema).await?;
        let object = object_name(&schema, table);
        let object = object.as_str();

        let column_rows = self.shared.query_params(COLUMNS_SQL, &[object]).await?;
        if column_rows.is_empty() {
            return Err(DatabaseError::MetadataError(format!(
                "Table or view {schema}.{table} was not found (or you cannot see it)"
            )));
        }
        let columns: Vec<ColumnInfo> = column_rows
            .iter()
            .map(|row| {
                let data_type =
                    format_sql_type(&text(row, 1), int(row, 2), int(row, 3), int(row, 4));
                let default_value = if flag(row, 6) {
                    Some(format!(
                        "IDENTITY({},{})",
                        optional(row, 11).unwrap_or_else(|| "1".to_string()),
                        optional(row, 12).unwrap_or_else(|| "1".to_string())
                    ))
                } else if flag(row, 7) {
                    optional(row, 10).map(|expression| format!("AS {expression}"))
                } else {
                    optional(row, 9).map(|definition| clean_default(&definition))
                };
                ColumnInfo {
                    name: text(row, 0),
                    data_type,
                    collation: text(row, 8),
                    nullable: flag(row, 5),
                    default_value,
                    enum_values: None,
                }
            })
            .collect();

        let indexes: Vec<IndexInfo> = self
            .shared
            .query_params(INDEXES_SQL, &[object])
            .await?
            .iter()
            .map(|row| {
                let (primary, unique_constraint) = (flag(row, 2), flag(row, 4));
                let keys = text(row, 6);
                let constraint_def = if primary {
                    Some(format!("PRIMARY KEY ({keys})"))
                } else if unique_constraint {
                    Some(format!("UNIQUE ({keys})"))
                } else {
                    None
                };
                IndexInfo {
                    name: text(row, 0),
                    index_type: text(row, 1),
                    is_primary: primary,
                    is_unique: flag(row, 3),
                    predicate: optional(row, 5),
                    definition: index_definition(
                        &schema,
                        table,
                        &text(row, 0),
                        &text(row, 1),
                        flag(row, 3),
                        &keys,
                        optional(row, 7).as_deref(),
                        optional(row, 5).as_deref(),
                    ),
                    constraint_def,
                }
            })
            .collect();

        let check_constraints = self
            .shared
            .query_params(CHECKS_SQL, &[object])
            .await?
            .iter()
            .map(|row| CheckConstraintInfo {
                name: text(row, 0),
                definition: text(row, 1),
            })
            .collect();

        let foreign_keys = self
            .shared
            .query_params(FOREIGN_KEYS_SQL, &[object])
            .await?
            .iter()
            .map(|row| ForeignKeyInfo {
                name: text(row, 0),
                definition: foreign_key_definition(
                    &text(row, 1),
                    &text(row, 2),
                    &text(row, 3),
                    &text(row, 4),
                    &text(row, 5),
                    &text(row, 6),
                ),
            })
            .collect();

        let referenced_by = self
            .shared
            .query_params(REFERENCED_BY_SQL, &[object])
            .await?
            .iter()
            .map(|row| ReferencedByInfo {
                schema: text(row, 1),
                table: text(row, 2),
                constraint_name: text(row, 0),
                definition: format!(
                    "FOREIGN KEY ({}) REFERENCES {} ({})",
                    text(row, 3),
                    object_name(&schema, table),
                    text(row, 4)
                ),
            })
            .collect();

        Ok(TableDetails {
            name: table.to_string(),
            schema: schema.clone(),
            full_name: format!("{schema}.{table}"),
            columns,
            indexes,
            check_constraints,
            foreign_keys,
            referenced_by,
            nested_field_details: HashMap::new(),
        })
    }

    /// `SET SHOWPLAN_XML` is available on every supported server.
    fn supports_explain(&self) -> bool {
        true
    }

    fn default_schema(&self) -> Option<String> {
        Some(DEFAULT_SCHEMA.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("Orders", "[Orders]")]
    #[case("Order Items", "[Order Items]")]
    #[case("a]b", "[a]]b]")]
    #[case("]", "[]]]")]
    #[case("x'; DROP TABLE t; --", "[x'; DROP TABLE t; --]")]
    fn names_are_bracket_quoted(#[case] name: &str, #[case] expected: &str) {
        assert_eq!(quote_name(name), expected);
    }

    #[test]
    fn object_names_quote_both_parts() {
        assert_eq!(object_name("dbo", "Orders"), "[dbo].[Orders]");
        assert_eq!(object_name("a.b", "c]d"), "[a.b].[c]]d]");
    }

    #[rstest]
    #[case("dbo", "Orders", None, "Orders")]
    #[case("Sales", "Orders", None, "Sales.Orders")]
    #[case("Sales", "Orders", Some("Sales"), "Orders")]
    #[case("Sales", "Order.Items", None, "Sales.[Order.Items]")]
    #[case("odd.schema", "t", None, "[odd.schema].t")]
    #[case("dbo", "a]b", None, "[a]]b]")]
    fn packed_names_round_trip_through_the_splitter(
        #[case] schema: &str,
        #[case] name: &str,
        #[case] browsing: Option<&str>,
        #[case] expected: &str,
    ) {
        use dbcrust_core::database::DatabaseType;
        let packed = pack_object(schema, name, browsing);
        assert_eq!(packed, expected);
        let (split_schema, split_table) =
            dbcrust_core::model::metadata_table_parts(&packed, &DatabaseType::SqlServer);
        assert_eq!(split_table, name);
        if schema != DEFAULT_SCHEMA && browsing.is_none() {
            assert_eq!(split_schema.as_deref(), Some(schema));
        } else {
            assert_eq!(split_schema, None);
        }
    }

    #[rstest]
    #[case("int", 4, 10, 0, "int")]
    #[case("nvarchar", 100, 0, 0, "nvarchar(50)")]
    #[case("nvarchar", -1, 0, 0, "nvarchar(max)")]
    #[case("varchar", 255, 0, 0, "varchar(255)")]
    #[case("varchar", -1, 0, 0, "varchar(max)")]
    #[case("nchar", 20, 0, 0, "nchar(10)")]
    #[case("varbinary", -1, 0, 0, "varbinary(max)")]
    #[case("binary", 16, 0, 0, "binary(16)")]
    #[case("decimal", 9, 18, 2, "decimal(18,2)")]
    #[case("numeric", 5, 10, 0, "numeric(10,0)")]
    #[case("datetime2", 8, 27, 7, "datetime2(7)")]
    #[case("time", 5, 16, 3, "time(3)")]
    #[case("datetimeoffset", 10, 34, 7, "datetimeoffset(7)")]
    #[case("float", 8, 53, 0, "float")]
    #[case("float", 4, 24, 0, "float(24)")]
    #[case("uniqueidentifier", 16, 0, 0, "uniqueidentifier")]
    #[case("geography", -1, 0, 0, "geography")]
    fn type_names_carry_their_size(
        #[case] type_name: &str,
        #[case] max_length: i32,
        #[case] precision: i32,
        #[case] scale: i32,
        #[case] expected: &str,
    ) {
        assert_eq!(
            format_sql_type(type_name, max_length, precision, scale),
            expected
        );
    }

    #[rstest]
    #[case("((0))", "0")]
    #[case("(getdate())", "getdate()")]
    #[case("('x')", "'x'")]
    #[case("((1)+(2))", "(1)+(2)")]
    #[case("(NEXT VALUE FOR [dbo].[seq])", "NEXT VALUE FOR [dbo].[seq]")]
    #[case("getdate()", "getdate()")]
    #[case("(a)+(b)", "(a)+(b)")]
    fn defaults_lose_their_outer_parentheses(#[case] raw: &str, #[case] expected: &str) {
        assert_eq!(clean_default(raw), expected);
    }

    #[test]
    fn index_definitions_read_like_ddl() {
        assert_eq!(
            index_definition(
                "dbo",
                "Orders",
                "IX_Orders_Date",
                "NONCLUSTERED",
                false,
                "[Date] DESC",
                Some("[Total]"),
                Some("([Total]>(0))")
            ),
            "CREATE NONCLUSTERED INDEX [IX_Orders_Date] ON [dbo].[Orders] ([Date] DESC) INCLUDE ([Total]) WHERE ([Total]>(0))"
        );
        assert_eq!(
            index_definition("Sales", "O", "PK_O", "CLUSTERED", true, "[Id]", None, None),
            "CREATE UNIQUE CLUSTERED INDEX [PK_O] ON [Sales].[O] ([Id])"
        );
    }

    #[test]
    fn foreign_key_definitions_name_actions_other_than_the_default() {
        assert_eq!(
            foreign_key_definition(
                "[CustomerId]",
                "dbo",
                "Customers",
                "[Id]",
                "NO_ACTION",
                "NO_ACTION"
            ),
            "FOREIGN KEY ([CustomerId]) REFERENCES [dbo].[Customers] ([Id])"
        );
        assert_eq!(
            foreign_key_definition("[A], [B]", "x", "y", "[A], [B]", "CASCADE", "SET_NULL"),
            "FOREIGN KEY ([A], [B]) REFERENCES [x].[y] ([A], [B]) ON DELETE CASCADE ON UPDATE SET NULL"
        );
    }

    /// Names reach the server as `@P1`/`@P2` parameters; none of the catalog
    /// statements may take user text any other way.
    #[test]
    fn catalog_statements_are_constants_with_parameter_placeholders() {
        for sql in [
            SCHEMAS_SQL,
            TABLES_ALL_SQL,
            TABLES_IN_SCHEMA_SQL,
            FUNCTIONS_ALL_SQL,
            FUNCTIONS_IN_SCHEMA_SQL,
            RESOLVE_SCHEMA_SQL,
            COLUMN_NAMES_SQL,
            COLUMNS_SQL,
            INDEXES_SQL,
            CHECKS_SQL,
            FOREIGN_KEYS_SQL,
            REFERENCED_BY_SQL,
        ] {
            assert!(!sql.contains('{'), "format placeholder left in {sql}");
            let params = sql.matches("@P").count();
            assert!(params <= 1, "{sql}");
        }
        for sql in [
            TABLES_IN_SCHEMA_SQL,
            FUNCTIONS_IN_SCHEMA_SQL,
            RESOLVE_SCHEMA_SQL,
        ] {
            assert!(sql.contains("@P1"));
        }
        for sql in [
            COLUMN_NAMES_SQL,
            COLUMNS_SQL,
            INDEXES_SQL,
            CHECKS_SQL,
            FOREIGN_KEYS_SQL,
            REFERENCED_BY_SQL,
        ] {
            assert!(sql.contains("OBJECT_ID(@P1)"));
        }
    }

    /// The statements are read-only by the driver's own guard (metadata runs
    /// even on `--read-only` sessions).
    #[test]
    fn catalog_statements_pass_the_read_only_guard() {
        use dbcrust_core::database::DatabaseType;
        for sql in [
            SCHEMAS_SQL,
            TABLES_ALL_SQL,
            TABLES_IN_SCHEMA_SQL,
            FUNCTIONS_ALL_SQL,
            FUNCTIONS_IN_SCHEMA_SQL,
            RESOLVE_SCHEMA_SQL,
            COLUMN_NAMES_SQL,
            COLUMNS_SQL,
            INDEXES_SQL,
            CHECKS_SQL,
            FOREIGN_KEYS_SQL,
            REFERENCED_BY_SQL,
        ] {
            let verdict = dbcrust_core::safety::check_read_only(sql, &DatabaseType::SqlServer);
            assert!(verdict.is_ok(), "{verdict:?}\n{sql}");
        }
    }
}
