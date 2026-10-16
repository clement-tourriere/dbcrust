//! Browsing: databases/buckets, measurements, tag and field keys.
//!
//! Each API generation answers in its own language — v1 `SHOW …`, v2 the Flux
//! `schema` package (looking back `schema_window`, 30 days unless the option
//! says otherwise, because Flux schema queries are time-bounded), v3
//! `information_schema` — and every name interpolated into a query is quoted
//! for that language.

use crate::exec::Engine;
use crate::table::Table;
use async_trait::async_trait;
use dbcrust_core::database::{
    DatabaseError, DatabaseType, DatabaseTypeExt, InfluxLanguage, MetadataProvider,
};
use dbcrust_core::model::{ColumnInfo, TableDetails};
use reqwest::Method;
use std::sync::Arc;
use std::time::Instant;

/// Flux string literal: `\`, `"` and `${` escaped (an unescaped `${` would
/// start an interpolation).
pub(crate) fn flux_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace("${", "\\${");
    format!("\"{escaped}\"")
}

/// InfluxQL double-quoted identifier.
pub(crate) fn influxql_ident(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

/// SQL single-quoted literal (v3).
fn sql_literal(value: &str) -> String {
    format!("'{}'", dbcrust_core::database::escape_sql_string(value))
}

/// Tag keys Flux's schema functions report that are not user columns.
const FLUX_INTERNAL_KEYS: &[&str] = &["_start", "_stop", "_measurement", "_field"];

/// The Flux functions worth completing.
pub(crate) const FLUX_FUNCTIONS: &[&str] = &[
    "from",
    "range",
    "filter",
    "map",
    "keep",
    "drop",
    "rename",
    "group",
    "ungroup",
    "sort",
    "limit",
    "tail",
    "first",
    "last",
    "min",
    "max",
    "mean",
    "median",
    "sum",
    "count",
    "spread",
    "stddev",
    "mode",
    "distinct",
    "unique",
    "top",
    "bottom",
    "quantile",
    "integral",
    "derivative",
    "difference",
    "elapsed",
    "movingAverage",
    "cumulativeSum",
    "aggregateWindow",
    "window",
    "duplicate",
    "fill",
    "pivot",
    "join",
    "union",
    "yield",
    "set",
    "reduce",
    "contains",
    "buckets",
    "now",
    "duration",
    "float",
    "int",
    "string",
    "uint",
    "bool",
    "time",
    "length",
    "schema.measurements",
    "schema.tagKeys",
    "schema.tagValues",
    "schema.fieldKeys",
    "schema.measurementTagKeys",
    "schema.measurementFieldKeys",
    "date.truncate",
    "strings.toUpper",
    "strings.toLower",
    "regexp.compile",
    "array.from",
];

/// A few DataFusion SQL functions for v3 completion.
const SQL_FUNCTIONS: &[&str] = &[
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "DATE_BIN",
    "DATE_TRUNC",
    "NOW",
    "INTERVAL",
    "COALESCE",
    "NULLIF",
    "SELECTOR_FIRST",
    "SELECTOR_LAST",
    "SELECTOR_MAX",
    "SELECTOR_MIN",
    "LOWER",
    "UPPER",
    "CONCAT",
    "SUBSTR",
    "ROUND",
    "ABS",
];

/// Metadata of one InfluxDB connection.
pub struct InfluxMetadataProvider {
    engine: Arc<Engine>,
}

impl InfluxMetadataProvider {
    pub(crate) fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }

    fn database_or(&self, schema: Option<&str>) -> Result<String, DatabaseError> {
        schema
            .map(str::to_string)
            .or_else(|| self.engine.database())
            .ok_or_else(|| {
                DatabaseError::MetadataError(
                    "no database selected: connect to one (/<database> in the URL, or \\c <database>)"
                        .to_string(),
                )
            })
    }

    /// The values of `column` (or the first column) of a metadata reply.
    fn values(table: &Table, column: &str) -> Vec<String> {
        let position = table
            .columns
            .iter()
            .position(|name| name == column)
            .unwrap_or(0);
        table
            .rows
            .iter()
            .filter_map(|row| row.get(position).cloned().flatten())
            .collect()
    }

    /// The databases (v1), buckets (v2) or databases (v3) as a table.
    pub(crate) async fn databases_table(&self) -> Result<Table, DatabaseError> {
        match self.engine.version {
            2 => {
                self.engine
                    .run(InfluxLanguage::Flux, "buckets()", None)
                    .await
            }
            3 => self.v3_databases().await,
            _ => {
                self.engine
                    .run(InfluxLanguage::InfluxQl, "SHOW DATABASES", None)
                    .await
            }
        }
    }

    /// v3 lists databases on a management endpoint, not through SQL.
    async fn v3_databases(&self) -> Result<Table, DatabaseError> {
        let started = Instant::now();
        let request = self
            .engine
            .http
            .request(Method::GET, "/api/v3/configure/database")
            .query(&[("format", "json")]);
        let response = self.engine.http.send(request, started).await?;
        let status = response.status();
        if !status.is_success() {
            let message = crate::conn::error_message(response, started).await;
            return Err(crate::conn::status_error(status, &message, false));
        }
        let body = response
            .text()
            .await
            .map_err(|error| crate::conn::transport_error(self.engine.http.base(), error))?;
        let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|error| {
            DatabaseError::MetadataError(format!("InfluxDB sent an invalid database list: {error}"))
        })?;
        let names = parsed
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| match entry {
                serde_json::Value::String(name) => Some(name.clone()),
                serde_json::Value::Object(fields) => fields
                    .values()
                    .find_map(|value| value.as_str().map(str::to_string)),
                _ => None,
            })
            .map(|name| vec![Some(name)])
            .collect();
        Ok(Table {
            columns: vec!["name".to_string()],
            rows: names,
            ..Table::default()
        })
    }

    /// Tag keys and field keys (name, type) of a measurement.
    async fn measurement_keys(
        &self,
        measurement: &str,
        database: &str,
    ) -> Result<(Vec<String>, Vec<(String, String)>), DatabaseError> {
        match self.engine.version {
            2 => {
                let window = &self.engine.schema_window;
                let query = |function: &str| {
                    format!(
                        "import \"influxdata/influxdb/schema\"\nschema.{function}(bucket: {}, measurement: {}, start: {window})",
                        flux_string(database),
                        flux_string(measurement)
                    )
                };
                let tags = self
                    .engine
                    .run(InfluxLanguage::Flux, &query("measurementTagKeys"), None)
                    .await?;
                let fields = self
                    .engine
                    .run(InfluxLanguage::Flux, &query("measurementFieldKeys"), None)
                    .await?;
                let tags = Self::values(&tags, "_value")
                    .into_iter()
                    .filter(|key| !FLUX_INTERNAL_KEYS.contains(&key.as_str()))
                    .collect();
                // Flux reports field names only
                let fields = Self::values(&fields, "_value")
                    .into_iter()
                    .map(|name| (name, "field".to_string()))
                    .collect();
                Ok((tags, fields))
            }
            3 => {
                let query = format!(
                    "SELECT column_name, data_type FROM information_schema.columns WHERE table_schema = 'iox' AND table_name = {} ORDER BY ordinal_position",
                    sql_literal(measurement)
                );
                let table = self
                    .engine
                    .run(InfluxLanguage::Sql, &query, Some(database))
                    .await?;
                // v3 does not tell tags from fields: dictionary columns are tags
                let mut tags = Vec::new();
                let mut fields = Vec::new();
                for row in &table.rows {
                    let name = row.first().cloned().flatten().unwrap_or_default();
                    let kind = row.get(1).cloned().flatten().unwrap_or_default();
                    if name == "time" {
                        continue;
                    }
                    if kind.starts_with("Dictionary") {
                        tags.push(name);
                    } else {
                        fields.push((name, kind));
                    }
                }
                Ok((tags, fields))
            }
            _ => {
                let from = influxql_ident(measurement);
                let tags = self
                    .engine
                    .run(
                        InfluxLanguage::InfluxQl,
                        &format!("SHOW TAG KEYS FROM {from}"),
                        Some(database),
                    )
                    .await?;
                let fields = self
                    .engine
                    .run(
                        InfluxLanguage::InfluxQl,
                        &format!("SHOW FIELD KEYS FROM {from}"),
                        Some(database),
                    )
                    .await?;
                let tags = Self::values(&tags, "tagKey");
                let kinds = Self::values(&fields, "fieldType");
                let fields = Self::values(&fields, "fieldKey")
                    .into_iter()
                    .zip(kinds.into_iter().chain(std::iter::repeat(String::new())))
                    .collect();
                Ok((tags, fields))
            }
        }
    }
}

#[async_trait]
impl MetadataProvider for InfluxMetadataProvider {
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        let table = self.databases_table().await?;
        let mut names = Self::values(&table, "name");
        if self.engine.version == 2 {
            // _monitoring / _tasks are internal
            names.retain(|name| !name.starts_with('_'));
        }
        Ok(names)
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        let database = self.database_or(schema).ok();
        let table = match self.engine.version {
            2 => {
                let bucket = self.database_or(schema)?;
                let query = format!(
                    "import \"influxdata/influxdb/schema\"\nschema.measurements(bucket: {}, start: {})",
                    flux_string(&bucket),
                    self.engine.schema_window
                );
                self.engine.run(InfluxLanguage::Flux, &query, None).await?
            }
            3 => {
                self.engine
                    .run(
                        InfluxLanguage::Sql,
                        "SELECT table_name FROM information_schema.tables WHERE table_schema = 'iox' ORDER BY table_name",
                        Some(&self.database_or(schema)?),
                    )
                    .await?
            }
            _ => {
                let query = match schema {
                    Some(name) => format!("SHOW MEASUREMENTS ON {}", influxql_ident(name)),
                    None => "SHOW MEASUREMENTS".to_string(),
                };
                self.engine
                    .run(InfluxLanguage::InfluxQl, &query, database.as_deref())
                    .await?
            }
        };
        let column = if self.engine.version == 2 {
            "_value"
        } else {
            "name"
        };
        Ok(Self::values(&table, column))
    }

    async fn get_columns(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<Vec<String>, DatabaseError> {
        let database = self.database_or(schema)?;
        let (tags, fields) = self.measurement_keys(table, &database).await?;
        let mut columns = vec!["time".to_string()];
        columns.extend(tags);
        columns.extend(fields.into_iter().map(|(name, _)| name));
        Ok(columns)
    }

    async fn get_functions(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        Ok(match self.engine.language {
            InfluxLanguage::Flux => FLUX_FUNCTIONS.iter().map(|f| (*f).to_string()).collect(),
            InfluxLanguage::Sql => SQL_FUNCTIONS.iter().map(|f| (*f).to_string()).collect(),
            InfluxLanguage::InfluxQl => DatabaseType::InfluxDB
                .sql_functions()
                .iter()
                .map(|f| (*f).to_string())
                .collect(),
        })
    }

    async fn get_table_details(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<TableDetails, DatabaseError> {
        let database = self.database_or(schema)?;
        let (tags, fields) = self.measurement_keys(table, &database).await?;
        let column = |name: String, data_type: String, nullable: bool| ColumnInfo {
            name,
            data_type,
            collation: String::new(),
            nullable,
            default_value: None,
            enum_values: None,
        };
        let mut columns = vec![column("time".to_string(), "timestamp".to_string(), false)];
        columns.extend(
            tags.into_iter()
                .map(|name| column(name, "tag (string)".to_string(), true)),
        );
        columns.extend(fields.into_iter().map(|(name, kind)| {
            let label = if kind.is_empty() {
                "field".to_string()
            } else if kind == "field" {
                kind
            } else {
                format!("field ({kind})")
            };
            column(name, label, true)
        }));
        Ok(TableDetails {
            name: table.to_string(),
            schema: database,
            full_name: table.to_string(),
            columns,
            indexes: Vec::new(),
            check_constraints: Vec::new(),
            foreign_keys: Vec::new(),
            referenced_by: Vec::new(),
            nested_field_details: Default::default(),
        })
    }

    fn supports_explain(&self) -> bool {
        self.engine.version == 3
    }

    fn default_schema(&self) -> Option<String> {
        self.engine.database()
    }
}
