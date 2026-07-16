//! White Dragon HTTP API implementation of the database abstraction layer.
//!
//! White Dragon accepts both its qualifier language and SQL dialect through
//! `POST /v1/search`. DBCrust exposes both search front-ends as a regular query
//! backend and discovers its exact query schema through `GET /v1/schema`.

use crate::database::{
    ConnectionInfo, DatabaseClient, DatabaseError, MetadataProvider, ServerInfo,
    StructuredQueryResult, query_timeout,
};
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

const DEFAULT_PORT: u16 = 7700;
const METADATA_TIMEOUT: Duration = Duration::from_secs(60);
const SCHEMA_API_VERSION: u32 = 3;
const HIT_COLUMNS: [&str; 6] = ["split_id", "doc_id", "id", "score", "snippet", "index"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct WhiteDragonColumn {
    name: String,
    data_type: String,
    nullable: bool,
    stored: bool,
    capabilities: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    hits: Vec<SearchHit>,
    #[serde(default)]
    partial: bool,
    #[serde(default)]
    partial_splits: Vec<String>,
    #[serde(default)]
    aggs: Option<AggregationResponse>,
}

#[derive(Debug, Deserialize)]
struct SearchHit {
    split_id: String,
    doc_id: u32,
    id: String,
    score: u64,
    #[serde(default)]
    fields: BTreeMap<String, Value>,
    snippet: String,
    index: String,
}

#[derive(Debug, Deserialize)]
struct AggregationResponse {
    #[serde(default)]
    buckets: Vec<AggregationBucket>,
    total_matches: u64,
    #[serde(default)]
    approximate: bool,
}

#[derive(Debug, Deserialize)]
struct AggregationBucket {
    value: String,
    count: u64,
}

#[derive(Debug, Deserialize)]
struct SearchSchemaResponse {
    version: u32,
    tables: Vec<String>,
    functions: Vec<String>,
    #[serde(default)]
    qualifier_controls: Vec<String>,
    schemas: Vec<SearchSchema>,
}

#[derive(Debug, Deserialize)]
struct SearchSchema {
    fields: Vec<SearchSchemaField>,
    indexes: Vec<SearchSchemaIndex>,
    #[serde(default)]
    default_search: Vec<String>,
    #[serde(default)]
    sort: Option<SearchSchemaSort>,
    #[serde(default)]
    operational: SearchOperationalFields,
}

#[derive(Debug, Deserialize)]
struct SearchSchemaField {
    name: String,
    field_type: String,
    stored: bool,
    columnar: bool,
}

#[derive(Debug, Deserialize)]
struct SearchSchemaIndex {
    name: String,
    field: String,
    kind: String,
    #[serde(default)]
    transform: Vec<Value>,
    case_fold: bool,
    tokenizer: String,
}

#[derive(Debug, Deserialize)]
struct SearchSchemaSort {
    field: String,
    direction: String,
}

#[derive(Debug, Default, Deserialize)]
struct SearchOperationalFields {
    time: Option<String>,
    namespace: Option<String>,
    tags: Option<String>,
}

/// Metadata for White Dragon's logical and mapping-defined document fields.
///
/// The authoritative `/v1/schema` response is cached for the connection so
/// browsing, completion, and empty result sets all use the exact split schema.
pub struct WhiteDragonMetadataProvider {
    client: Client,
    base_url: String,
    username: Option<String>,
    password: Option<String>,
    schema: RwLock<Option<Arc<SearchSchemaResponse>>>,
}

impl WhiteDragonMetadataProvider {
    fn new(client: Client, base_url: String, connection_info: &ConnectionInfo) -> Self {
        Self {
            client,
            base_url,
            username: connection_info.username.clone(),
            password: connection_info.password.clone(),
            schema: RwLock::new(None),
        }
    }

    fn request(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let request = if let Some(username) = self.username.as_deref() {
            request.basic_auth(username, self.password.as_deref())
        } else {
            request
        };
        request.timeout(METADATA_TIMEOUT)
    }

    async fn schema(&self) -> Result<Arc<SearchSchemaResponse>, DatabaseError> {
        if let Some(schema) = self.schema.read().await.as_ref() {
            return Ok(Arc::clone(schema));
        }
        let schema = Arc::new(self.fetch_schema().await?);
        let mut cached = self.schema.write().await;
        if cached.is_none() {
            *cached = Some(Arc::clone(&schema));
        }
        cached.as_ref().map(Arc::clone).ok_or_else(|| {
            DatabaseError::MetadataError("White Dragon schema cache is empty".to_string())
        })
    }

    async fn fetch_schema(&self) -> Result<SearchSchemaResponse, DatabaseError> {
        let response = self
            .request(self.client.get(format!("{}/v1/schema", self.base_url)))
            .send()
            .await
            .map_err(|error| {
                DatabaseError::MetadataError(format!("White Dragon schema request failed: {error}"))
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            DatabaseError::MetadataError(format!(
                "Failed to read White Dragon schema response: {error}"
            ))
        })?;
        if !status.is_success() {
            let message = if status == StatusCode::NOT_FOUND {
                "White Dragon does not expose /v1/schema; upgrade to a server with schema introspection"
                    .to_string()
            } else {
                error_message(status, &body)
            };
            return Err(DatabaseError::MetadataError(message));
        }
        let schema: SearchSchemaResponse = serde_json::from_str(&body).map_err(|error| {
            DatabaseError::MetadataError(format!(
                "Failed to decode White Dragon schema response: {error}"
            ))
        })?;
        if schema.version != SCHEMA_API_VERSION {
            return Err(DatabaseError::MetadataError(format!(
                "Unsupported White Dragon schema version {}; expected version {SCHEMA_API_VERSION}",
                schema.version
            )));
        }
        Ok(schema)
    }

    async fn columns_for_table(
        &self,
        table: &str,
    ) -> Result<Vec<WhiteDragonColumn>, DatabaseError> {
        let schema = self.schema().await?;
        canonical_table(&schema, table)?;
        Ok(merged_schema_columns(&schema))
    }

    async fn completion_columns(&self, table: &str) -> Result<Vec<String>, DatabaseError> {
        let schema = self.schema().await?;
        canonical_table(&schema, table)?;
        let mut names = merged_schema_columns(&schema)
            .into_iter()
            .map(|field| field.name)
            .collect::<BTreeSet<_>>();
        names.extend(
            schema
                .schemas
                .iter()
                .flat_map(|schema| schema.indexes.iter())
                .map(|index| index.name.clone()),
        );
        Ok(names.into_iter().collect())
    }

    async fn stored_field_names(&self) -> Result<Vec<String>, DatabaseError> {
        let schema = self.schema().await?;
        Ok(merged_schema_columns(&schema)
            .into_iter()
            .filter(|field| field.stored)
            .map(|field| field.name)
            .collect())
    }
}

#[derive(Default)]
struct MergedField {
    data_types: BTreeSet<String>,
    stored: bool,
    columnar: bool,
    capabilities: BTreeSet<String>,
}

fn merged_schema_columns(schema: &SearchSchemaResponse) -> Vec<WhiteDragonColumn> {
    let mut fields = BTreeMap::<String, MergedField>::new();
    for current in &schema.schemas {
        for field in &current.fields {
            let merged = fields.entry(field.name.clone()).or_default();
            merged
                .data_types
                .insert(schema_data_type(&field.field_type));
            merged.stored |= field.stored;
            merged.columnar |= field.columnar;
            if field.stored {
                merged.capabilities.insert("stored".to_string());
            }
            if field.columnar {
                merged.capabilities.insert("columnar".to_string());
            }
        }
        for index in &current.indexes {
            fields
                .entry(index.field.clone())
                .or_default()
                .capabilities
                .insert("indexed".to_string());
        }
        if let Some(sort) = &current.sort {
            fields
                .entry(sort.field.clone())
                .or_default()
                .capabilities
                .insert(format!("sort {}", sort.direction));
        }
        for (role, field) in [
            ("time", current.operational.time.as_deref()),
            ("namespace", current.operational.namespace.as_deref()),
            ("tags", current.operational.tags.as_deref()),
        ] {
            if let Some(field) = field {
                fields
                    .entry(field.to_string())
                    .or_default()
                    .capabilities
                    .insert(format!("operational {role}"));
            }
        }
    }
    fields
        .into_iter()
        .map(|(name, field)| WhiteDragonColumn {
            name,
            data_type: if field.data_types.len() == 1 {
                field
                    .data_types
                    .into_iter()
                    .next()
                    .expect("one merged field type")
            } else {
                format!(
                    "VARIANT<{}>",
                    field.data_types.into_iter().collect::<Vec<_>>().join(", ")
                )
            },
            // Mapping v2's embedded schema does not retain the ingest-only
            // `required` bit, so clients must conservatively allow NULL.
            nullable: true,
            stored: field.stored,
            capabilities: field.capabilities.into_iter().collect(),
        })
        .collect()
}

fn schema_data_type(field_type: &str) -> String {
    match field_type {
        "text" => "TEXT[]",
        "keyword" => "KEYWORD[]",
        "i64" => "I64",
        "u64" => "U64",
        "f64" => "F64",
        "bool" => "BOOL",
        "datetime" => "DATETIME",
        "json" => "JSON",
        other => return other.to_ascii_uppercase(),
    }
    .to_string()
}

fn canonical_table<'a>(
    schema: &'a SearchSchemaResponse,
    table: &str,
) -> Result<&'a str, DatabaseError> {
    let table = table.trim_matches('"');
    schema
        .tables
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(table))
        .map(String::as_str)
        .ok_or_else(|| {
            DatabaseError::MetadataError(format!(
                "Unknown White Dragon table '{table}'; available tables: {}",
                schema.tables.join(", ")
            ))
        })
}

fn schema_qualifiers(schema: &SearchSchemaResponse) -> Vec<String> {
    let mut qualifiers = schema
        .qualifier_controls
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    for current in &schema.schemas {
        let mut indexed_fields = BTreeSet::new();
        let mut fields_with_sub_indexes = BTreeSet::new();
        for index in &current.indexes {
            qualifiers.insert(index.name.clone());
            indexed_fields.insert(index.field.clone());
            if index.name != index.field {
                fields_with_sub_indexes.insert(index.field.clone());
            }
        }
        qualifiers.extend(
            fields_with_sub_indexes
                .intersection(&indexed_fields)
                .map(|field| format!("{field}.*")),
        );
    }
    qualifiers.into_iter().collect()
}

#[derive(Default)]
struct MergedIndex {
    kinds: BTreeSet<String>,
    definitions: BTreeSet<String>,
}

fn schema_indexes(schema: &SearchSchemaResponse) -> Vec<crate::db::IndexInfo> {
    let mut indexes = BTreeMap::<String, MergedIndex>::new();
    for current in &schema.schemas {
        for index in &current.indexes {
            let merged = indexes.entry(index.name.clone()).or_default();
            merged.kinds.insert(index.kind.to_ascii_uppercase());
            let mut definition = format!("ON documents ({})", index.field);
            if current.default_search.contains(&index.name) {
                definition.push_str(" DEFAULT SEARCH");
            }
            if index.case_fold {
                definition.push_str(" CASE FOLD");
            }
            if index.kind == "token" {
                definition.push_str(&format!(" TOKENIZER {}", index.tokenizer));
            }
            if !index.transform.is_empty() {
                definition.push_str(&format!(
                    " TRANSFORM {}",
                    serde_json::to_string(&index.transform).unwrap_or_else(|_| "[]".to_string())
                ));
            }
            merged.definitions.insert(definition);
        }
    }
    indexes
        .into_iter()
        .map(|(name, index)| crate::db::IndexInfo {
            name,
            index_type: index.kinds.into_iter().collect::<Vec<_>>().join(" | "),
            is_primary: false,
            is_unique: false,
            predicate: None,
            definition: index
                .definitions
                .into_iter()
                .collect::<Vec<_>>()
                .join(" OR "),
            constraint_def: None,
        })
        .collect()
}

#[async_trait]
impl MetadataProvider for WhiteDragonMetadataProvider {
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        Ok(vec!["default".to_string()])
    }

    async fn get_tables(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        Ok(self.schema().await?.tables.clone())
    }

    async fn get_columns(
        &self,
        table: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<String>, DatabaseError> {
        self.completion_columns(table).await
    }

    async fn get_functions(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        Ok(self.schema().await?.functions.clone())
    }

    async fn get_table_details(
        &self,
        table: &str,
        _schema: Option<&str>,
    ) -> Result<crate::db::TableDetails, DatabaseError> {
        let schema = self.schema().await?;
        let canonical_table = canonical_table(&schema, table)?.to_string();
        Ok(crate::db::TableDetails {
            name: canonical_table.clone(),
            schema: "default".to_string(),
            full_name: canonical_table.clone(),
            columns: self
                .columns_for_table(&canonical_table)
                .await?
                .into_iter()
                .map(|column| crate::db::ColumnInfo {
                    name: column.name,
                    data_type: column.data_type,
                    collation: column.capabilities.join(", "),
                    nullable: column.nullable,
                    default_value: None,
                    enum_values: None,
                })
                .collect(),
            indexes: schema_indexes(&schema),
            check_constraints: Vec::new(),
            foreign_keys: Vec::new(),
            referenced_by: Vec::new(),
            nested_field_details: HashMap::new(),
        })
    }

    async fn get_search_qualifiers(&self) -> Result<Vec<String>, DatabaseError> {
        let schema = self.schema().await?;
        Ok(schema_qualifiers(&schema))
    }

    fn supports_explain(&self) -> bool {
        false
    }

    fn default_schema(&self) -> Option<String> {
        Some("default".to_string())
    }
}

/// Client for a running White Dragon server.
pub struct WhiteDragonClient {
    client: Client,
    connection_info: ConnectionInfo,
    base_url: String,
    metadata_provider: WhiteDragonMetadataProvider,
}

impl WhiteDragonClient {
    /// Connect to a White Dragon HTTP endpoint and verify that it is ready.
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        let base_url = base_url(&connection_info)?;
        let client = Client::builder()
            .build()
            .map_err(|error| DatabaseError::ConnectionError(error.to_string()))?;
        let metadata_provider =
            WhiteDragonMetadataProvider::new(client.clone(), base_url.clone(), &connection_info);
        let instance = Self {
            client,
            connection_info,
            base_url,
            metadata_provider,
        };
        instance.check_ready().await?;
        // Require and cache the authoritative schema so browsing, completion,
        // and empty result sets cannot silently fall back to sampled fields.
        instance.metadata_provider.schema().await?;
        Ok(instance)
    }

    async fn check_ready(&self) -> Result<(), DatabaseError> {
        let response = self
            .request(self.client.get(format!("{}/readyz", self.base_url)))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|error| {
                DatabaseError::ConnectionError(format!(
                    "Could not reach White Dragon at {}: {error}",
                    self.base_url
                ))
            })?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(DatabaseError::ConnectionError(format!(
                "White Dragon at {} is not ready (HTTP {})",
                self.base_url,
                response.status()
            )))
        }
    }

    fn request(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let request = if let Some(username) = self.connection_info.username.as_deref() {
            request.basic_auth(username, self.connection_info.password.as_deref())
        } else {
            request
        };
        if let Some(timeout) = query_timeout() {
            request.timeout(timeout + Duration::from_secs(1))
        } else {
            request
        }
    }

    async fn execute(&self, query: &str) -> Result<StructuredQueryResult, DatabaseError> {
        let prepared = prepare_search_request(query)?;
        let stored_fields = self.metadata_provider.stored_field_names().await?;
        validate_hit_projection(prepared.hit_projection.as_deref(), &stored_fields)?;
        let response = self
            .request(
                self.client
                    .post(format!("{}/v1/search", self.base_url))
                    .json(&prepared.body),
            )
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    DatabaseError::QueryError(format!("White Dragon query timed out: {error}"))
                } else {
                    DatabaseError::QueryError(format!("White Dragon request failed: {error}"))
                }
            })?;

        let status = response.status();
        let body = response.text().await.map_err(|error| {
            DatabaseError::QueryError(format!("Failed to read White Dragon response: {error}"))
        })?;
        if !status.is_success() {
            return Err(DatabaseError::QueryError(error_message(status, &body)));
        }
        let response: SearchResponse = serde_json::from_str(&body).map_err(|error| {
            DatabaseError::QueryError(format!("Failed to decode White Dragon response: {error}"))
        })?;
        Ok(response_to_result_with_group(
            prepared.group_column,
            prepared.hit_projection.as_deref(),
            response,
            &stored_fields,
        ))
    }
}

#[derive(Debug)]
struct PreparedSearchRequest {
    body: Value,
    group_column: Option<String>,
    hit_projection: Option<Vec<String>>,
}

#[derive(Debug, PartialEq, Eq)]
struct SqlHitProjection {
    fields: Vec<String>,
    rewritten_query: String,
}

fn prepare_search_request(query: &str) -> Result<PreparedSearchRequest, DatabaseError> {
    let timeout_ms =
        query_timeout().map(|timeout| u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX));
    if !query.trim_start().starts_with('{') {
        let projection = sql_hit_projection(query);
        return Ok(PreparedSearchRequest {
            body: json!({
                "q": projection
                    .as_ref()
                    .map_or(query, |projection| projection.rewritten_query.as_str()),
                "timeout_ms": timeout_ms
            }),
            group_column: selected_group_column(query),
            hit_projection: projection.map(|projection| projection.fields),
        });
    }

    let mut body: Value = serde_json::from_str(query).map_err(|error| {
        DatabaseError::QueryError(format!("Invalid White Dragon search request JSON: {error}"))
    })?;
    let object = body.as_object_mut().ok_or_else(|| {
        DatabaseError::QueryError("White Dragon search request must be a JSON object".to_string())
    })?;
    let q = object
        .get("q")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            DatabaseError::QueryError(
                "White Dragon search request requires a string 'q' field".to_string(),
            )
        })?
        .to_string();
    let projection = sql_hit_projection(&q);
    if let Some(projection) = &projection {
        object.insert("q".to_string(), json!(projection.rewritten_query));
    }
    if !object.contains_key("timeout_ms") {
        object.insert("timeout_ms".to_string(), json!(timeout_ms));
    }
    let group_column = selected_group_column(&q).or_else(|| structured_group_column(object));
    Ok(PreparedSearchRequest {
        body,
        group_column,
        hit_projection: projection.map(|projection| projection.fields),
    })
}

/// White Dragon's compact SQL currently accepts only `SELECT *` for hit
/// queries. DBCrust exposes normal stored-field projection by expanding the
/// server query and projecting the typed hit fields locally.
fn sql_hit_projection(query: &str) -> Option<SqlHitProjection> {
    let leading_whitespace = query.len() - query.trim_start().len();
    let select_end = leading_whitespace.checked_add("SELECT".len())?;
    if !query
        .get(leading_whitespace..select_end)?
        .eq_ignore_ascii_case("SELECT")
        || !query
            .get(select_end..)?
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
    {
        return None;
    }

    let from_start = find_top_level_from(query, select_end)?;
    let select_list = query.get(select_end..from_start)?.trim();
    let fields = split_projection_fields(select_list)?;
    if fields.is_empty() {
        return None;
    }

    let mut rewritten_query = String::with_capacity(query.len() + 2);
    rewritten_query.push_str(query.get(..select_end)?);
    rewritten_query.push_str(" * ");
    rewritten_query.push_str(query.get(from_start..)?);
    Some(SqlHitProjection {
        fields,
        rewritten_query,
    })
}

fn find_top_level_from(query: &str, start: usize) -> Option<usize> {
    let bytes = query.as_bytes();
    let mut index = start;
    let mut depth = 0usize;
    let mut quote = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(delimiter) = quote {
            if byte == delimiter {
                if bytes.get(index + 1) == Some(&delimiter) {
                    index += 2;
                    continue;
                }
                quote = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' => {
                quote = Some(byte);
                index += 1;
            }
            b'(' => {
                depth += 1;
                index += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            _ if depth == 0 && (byte.is_ascii_alphabetic() || byte == b'_') => {
                let word_start = index;
                index += 1;
                while bytes
                    .get(index)
                    .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                {
                    index += 1;
                }
                if query
                    .get(word_start..index)
                    .is_some_and(|word| word.eq_ignore_ascii_case("FROM"))
                {
                    return Some(word_start);
                }
            }
            _ => index += 1,
        }
    }
    None
}

fn split_projection_fields(select_list: &str) -> Option<Vec<String>> {
    let bytes = select_list.as_bytes();
    let mut fields = Vec::new();
    let mut field_start = 0usize;
    let mut index = 0usize;
    let mut quote = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(delimiter) = quote {
            if byte == delimiter {
                if bytes.get(index + 1) == Some(&delimiter) {
                    index += 2;
                    continue;
                }
                quote = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b',' => {
                fields.push(parse_projection_field(
                    select_list.get(field_start..index)?.trim(),
                )?);
                field_start = index + 1;
            }
            b'(' | b')' => return None,
            _ => {}
        }
        index += 1;
    }
    if quote.is_some() {
        return None;
    }
    fields.push(parse_projection_field(
        select_list.get(field_start..)?.trim(),
    )?);
    Some(fields)
}

fn parse_projection_field(field: &str) -> Option<String> {
    if field.starts_with('"') {
        if !field.ends_with('"') || field.len() < 2 {
            return None;
        }
        let inner = field.get(1..field.len() - 1)?;
        let decoded = inner.replace("\"\"", "\"");
        return (!decoded.is_empty()).then_some(decoded);
    }
    let valid = !field.is_empty()
        && field.split('.').all(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
                && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
        });
    valid.then(|| field.to_string())
}

fn validate_hit_projection(
    projection: Option<&[String]>,
    stored_fields: &[String],
) -> Result<(), DatabaseError> {
    let Some(projection) = projection else {
        return Ok(());
    };
    if let Some(field) = projection
        .iter()
        .find(|field| !stored_fields.contains(field))
    {
        return Err(DatabaseError::QueryError(format!(
            "White Dragon field '{field}' is not a stored source field; available stored fields: {}",
            stored_fields.join(", ")
        )));
    }
    Ok(())
}

fn structured_group_column(request: &serde_json::Map<String, Value>) -> Option<String> {
    let aggregation = request.get("aggregation")?.as_object()?;
    if let Some(terms) = aggregation.get("terms").and_then(Value::as_object) {
        return terms
            .get("field")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    let histogram = aggregation
        .get("date_histogram")
        .and_then(Value::as_object)?;
    let field = histogram.get("field")?.as_str()?;
    let interval = histogram.get("interval")?.as_str()?;
    Some(format!("{interval}({field})"))
}

fn base_url(connection_info: &ConnectionInfo) -> Result<String, DatabaseError> {
    let host = connection_info.host.as_deref().ok_or_else(|| {
        DatabaseError::InvalidUrl(
            "White Dragon URL requires a host, for example white-dragon://127.0.0.1:7700"
                .to_string(),
        )
    })?;
    let protocol = if connection_info.use_tls {
        "https"
    } else {
        "http"
    };
    let port = connection_info.port.unwrap_or(DEFAULT_PORT);
    Ok(format!("{protocol}://{host}:{port}"))
}

fn error_message(status: StatusCode, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| body.trim().to_string());
    if detail.is_empty() {
        format!("White Dragon returned HTTP {status}")
    } else {
        format!("White Dragon returned HTTP {status}: {detail}")
    }
}

#[cfg(test)]
fn response_to_result(query: &str, response: SearchResponse) -> StructuredQueryResult {
    let prepared = prepare_search_request(query).expect("test query should prepare");
    response_to_result_with_group(
        prepared.group_column,
        prepared.hit_projection.as_deref(),
        response,
        &[],
    )
}

fn response_to_result_with_group(
    group_column: Option<String>,
    hit_projection: Option<&[String]>,
    response: SearchResponse,
    schema_stored_fields: &[String],
) -> StructuredQueryResult {
    let SearchResponse {
        hits,
        partial,
        partial_splits,
        aggs,
    } = response;

    if let Some(aggs) = aggs {
        if aggs.approximate || partial {
            tracing::warn!(
                approximate = aggs.approximate,
                partial,
                partial_splits = ?partial_splits,
                "White Dragon aggregation is incomplete"
            );
        }

        if aggs.buckets.is_empty() {
            return match group_column {
                Some(group_column) => StructuredQueryResult {
                    columns: vec![group_column, "count".to_string()],
                    rows: Vec::new(),
                },
                None => StructuredQueryResult {
                    columns: vec!["count".to_string()],
                    rows: vec![vec![Some(aggs.total_matches.to_string())]],
                },
            };
        }

        return StructuredQueryResult {
            columns: vec![
                group_column.unwrap_or_else(|| "value".to_string()),
                "count".to_string(),
            ],
            rows: aggs
                .buckets
                .into_iter()
                .map(|bucket| vec![Some(bucket.value), Some(bucket.count.to_string())])
                .collect(),
        };
    }

    if partial {
        tracing::warn!(
            partial_splits = ?partial_splits,
            "White Dragon returned partial search results"
        );
    }
    if let Some(hit_projection) = hit_projection {
        return StructuredQueryResult {
            columns: hit_projection.to_vec(),
            rows: hits
                .into_iter()
                .map(|hit| {
                    hit_projection
                        .iter()
                        .map(|field| hit.fields.get(field).and_then(json_value_to_cell))
                        .collect()
                })
                .collect(),
        };
    }
    let field_columns = result_field_columns(&hits, schema_stored_fields);
    let mut columns = HIT_COLUMNS[..4]
        .iter()
        .map(|column| (*column).to_string())
        .collect::<Vec<_>>();
    columns.extend(
        field_columns
            .iter()
            .map(|(_, display_name)| display_name.clone()),
    );
    columns.extend(HIT_COLUMNS[4..].iter().map(|column| (*column).to_string()));

    StructuredQueryResult {
        columns,
        rows: hits
            .into_iter()
            .map(|hit| {
                let mut row = vec![
                    Some(hit.split_id),
                    Some(hit.doc_id.to_string()),
                    Some(hit.id),
                    Some(hit.score.to_string()),
                ];
                row.extend(
                    field_columns
                        .iter()
                        .map(|(name, _)| hit.fields.get(name).and_then(json_value_to_cell)),
                );
                row.extend([Some(hit.snippet), Some(hit.index)]);
                row
            })
            .collect(),
    }
}

fn result_field_columns(
    hits: &[SearchHit],
    schema_stored_fields: &[String],
) -> Vec<(String, String)> {
    let names = schema_stored_fields
        .iter()
        .cloned()
        .chain(hits.iter().flat_map(|hit| hit.fields.keys().cloned()))
        .collect::<BTreeSet<_>>();
    names
        .into_iter()
        .map(|name| {
            let display_name = if HIT_COLUMNS.contains(&name.as_str()) {
                format!("fields.{name}")
            } else {
                name.clone()
            };
            (name, display_name)
        })
        .collect()
}

fn json_value_to_cell(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::Array(_) | Value::Object(_) => Some(value.to_string()),
    }
}

fn selected_group_column(sql: &str) -> Option<String> {
    let trimmed = sql.trim_start();
    let select = trimmed.get(..6)?;
    if !select.eq_ignore_ascii_case("select") {
        return None;
    }
    trimmed[6..]
        .split_once(',')
        .map(|(expression, _)| expression.trim())
        .filter(|expression| !expression.is_empty())
        .map(str::to_string)
        .or_else(|| group_by_column(sql))
}

fn group_by_column(sql: &str) -> Option<String> {
    let context = crate::sql_parser::SqlParser::new(sql.to_string()).parse_at_cursor(sql.len());
    let tokens = context
        .tokens
        .iter()
        .filter(|token| token.token_type != crate::sql_parser::TokenType::Whitespace)
        .collect::<Vec<_>>();
    let group = tokens.windows(2).position(|pair| {
        pair[0].value.eq_ignore_ascii_case("GROUP") && pair[1].value.eq_ignore_ascii_case("BY")
    })?;
    let start = tokens[group + 1].end;
    let mut depth = 0usize;
    let mut end = sql.len();
    for token in tokens.iter().skip(group + 2) {
        match token.value.as_str() {
            "(" => depth += 1,
            ")" => depth = depth.saturating_sub(1),
            ";" if depth == 0 => {
                end = token.start;
                break;
            }
            _ if depth == 0
                && (token.value.eq_ignore_ascii_case("ORDER")
                    || token.value.eq_ignore_ascii_case("LIMIT")) =>
            {
                end = token.start;
                break;
            }
            _ => {}
        }
    }
    let expression = sql.get(start..end)?.trim();
    (!expression.is_empty()).then(|| expression.to_string())
}

#[async_trait]
impl DatabaseClient for WhiteDragonClient {
    async fn execute_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        Ok(self.execute(sql).await?.into_display_rows("NULL"))
    }

    async fn execute_query_structured(
        &self,
        sql: &str,
    ) -> Result<StructuredQueryResult, DatabaseError> {
        self.execute(sql).await
    }

    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError> {
        self.execute(sql).await.map(|_| ())
    }

    async fn explain_query(&self, _sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        Err(DatabaseError::FeatureNotSupported {
            database_type: self.connection_info.database_type.clone(),
            feature: "EXPLAIN".to_string(),
        })
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        self.explain_query(sql).await
    }

    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        Ok(vec![
            vec!["Database".to_string()],
            vec!["documents".to_string()],
        ])
    }

    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError> {
        if database.eq_ignore_ascii_case("documents") || database.eq_ignore_ascii_case("default") {
            Ok(())
        } else {
            Err(DatabaseError::FeatureNotSupported {
                database_type: self.connection_info.database_type.clone(),
                feature: format!("switching to database '{database}'"),
            })
        }
    }

    fn get_current_database(&self) -> String {
        "documents".to_string()
    }

    fn get_connection_info(&self) -> &ConnectionInfo {
        &self.connection_info
    }

    fn get_metadata_provider(&self) -> &dyn MetadataProvider {
        &self.metadata_provider
    }

    async fn is_connected(&self) -> bool {
        self.check_ready().await.is_ok()
    }

    async fn close(&mut self) -> Result<(), DatabaseError> {
        Ok(())
    }

    async fn get_server_info(&self) -> Result<ServerInfo, DatabaseError> {
        let response = self
            .request(self.client.get(format!("{}/healthz", self.base_url)))
            .send()
            .await
            .map_err(|error| DatabaseError::ConnectionError(error.to_string()))?;
        if !response.status().is_success() {
            return Err(DatabaseError::ConnectionError(format!(
                "White Dragon health check returned HTTP {}",
                response.status()
            )));
        }
        let health: Value = response.json().await.map_err(|error| {
            DatabaseError::ConnectionError(format!(
                "Failed to decode White Dragon health response: {error}"
            ))
        })?;
        let schema_version = self.metadata_provider.schema().await?.version;
        let mut info = ServerInfo::new(
            "White Dragon".to_string(),
            format!("HTTP API v1 / schema v{schema_version}"),
        );
        info.supports_transactions = false;
        info.supports_roles = false;
        for key in ["status", "splits", "docs"] {
            if let Some(value) = health.get(key) {
                info.additional_info
                    .insert(key.to_string(), value.to_string());
            }
        }
        Ok(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::DatabaseType;

    fn connection_info() -> ConnectionInfo {
        ConnectionInfo {
            database_type: DatabaseType::WhiteDragon,
            host: Some("search.example".to_string()),
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
            use_tls: false,
        }
    }

    fn search_hit() -> SearchHit {
        SearchHit {
            split_id: "01".to_string(),
            doc_id: 7,
            id: "abc".to_string(),
            score: 8,
            fields: BTreeMap::new(),
            snippet: "Rust search".to_string(),
            index: "headline".to_string(),
        }
    }

    fn schema_field(
        name: &str,
        field_type: &str,
        stored: bool,
        columnar: bool,
    ) -> SearchSchemaField {
        SearchSchemaField {
            name: name.to_string(),
            field_type: field_type.to_string(),
            stored,
            columnar,
        }
    }

    fn schema_index(name: &str, field: &str, kind: &str, case_fold: bool) -> SearchSchemaIndex {
        SearchSchemaIndex {
            name: name.to_string(),
            field: field.to_string(),
            kind: kind.to_string(),
            transform: Vec::new(),
            case_fold,
            tokenizer: "simple".to_string(),
        }
    }

    fn schema_response() -> SearchSchemaResponse {
        SearchSchemaResponse {
            version: 3,
            tables: vec!["documents".to_string()],
            functions: vec![
                "COUNT".to_string(),
                "YEAR".to_string(),
                "MONTH".to_string(),
                "DAY".to_string(),
            ],
            qualifier_controls: vec!["case".to_string()],
            schemas: vec![
                SearchSchema {
                    fields: vec![
                        schema_field("attributes", "json", true, false),
                        schema_field("body", "text", true, false),
                        schema_field("hidden", "text", false, false),
                        schema_field("priority", "i64", true, true),
                        schema_field("visibility", "keyword", true, true),
                    ],
                    indexes: vec![
                        schema_index("body", "body", "sparse_ngram", true),
                        schema_index("body.tokens", "body", "token", true),
                        schema_index("priority", "priority", "range", false),
                        schema_index("visibility", "visibility", "exact", false),
                    ],
                    default_search: vec!["body".to_string()],
                    sort: Some(SearchSchemaSort {
                        field: "priority".to_string(),
                        direction: "desc".to_string(),
                    }),
                    operational: SearchOperationalFields {
                        time: None,
                        namespace: Some("visibility".to_string()),
                        tags: None,
                    },
                },
                SearchSchema {
                    fields: vec![
                        schema_field("priority", "f64", true, true),
                        schema_field("rating", "f64", true, true),
                        schema_field("visibility", "keyword", true, true),
                    ],
                    indexes: vec![
                        schema_index("priority", "priority", "range", false),
                        schema_index("rating", "rating", "range", false),
                        schema_index("visibility", "visibility", "exact", false),
                    ],
                    default_search: Vec::new(),
                    sort: None,
                    operational: SearchOperationalFields::default(),
                },
            ],
        }
    }

    #[test]
    fn builds_http_base_url_with_default_port() {
        assert_eq!(
            base_url(&connection_info()).unwrap(),
            "http://search.example:7700"
        );
    }

    #[test]
    fn builds_https_base_url() {
        let mut info = connection_info();
        info.use_tls = true;
        info.port = Some(443);
        assert_eq!(base_url(&info).unwrap(), "https://search.example:443");
    }

    #[tokio::test]
    async fn schema_v3_feeds_fields_indexes_and_qualifier_completion() {
        let provider = WhiteDragonMetadataProvider::new(
            Client::new(),
            "http://search.example:7700".to_string(),
            &connection_info(),
        );
        *provider.schema.write().await = Some(Arc::new(schema_response()));

        assert_eq!(provider.get_tables(None).await.unwrap(), ["documents"]);
        let qualifiers = provider.get_search_qualifiers().await.unwrap();
        for expected in [
            "body",
            "body.*",
            "body.tokens",
            "case",
            "priority",
            "rating",
            "visibility",
        ] {
            assert!(qualifiers.contains(&expected.to_string()), "{expected}");
        }
        assert!(!qualifiers.contains(&"attributes".to_string()));
        assert!(!qualifiers.contains(&"hidden".to_string()));

        let completion = provider.get_columns("documents", None).await.unwrap();
        assert!(completion.contains(&"attributes".to_string()));
        assert!(completion.contains(&"body.tokens".to_string()));

        let details = provider.get_table_details("documents", None).await.unwrap();
        assert_eq!(details.columns[0].name, "attributes");
        assert_eq!(details.columns[0].data_type, "JSON");
        let priority = details
            .columns
            .iter()
            .find(|column| column.name == "priority")
            .unwrap();
        assert_eq!(priority.data_type, "VARIANT<F64, I64>");
        assert!(priority.nullable);
        assert!(priority.collation.contains("columnar"));
        assert!(priority.collation.contains("sort desc"));
        assert!(
            details
                .columns
                .iter()
                .find(|column| column.name == "visibility")
                .unwrap()
                .collation
                .contains("operational namespace")
        );
        assert!(
            details
                .indexes
                .iter()
                .any(|index| index.name == "body.tokens" && index.index_type == "TOKEN")
        );
        assert!(
            provider
                .get_table_details("patches", None)
                .await
                .unwrap_err()
                .to_string()
                .contains("available tables: documents")
        );
    }

    #[test]
    fn maps_every_wdsp_v6_field_type_without_losing_width_or_signedness() {
        let cases = [
            ("text", "TEXT[]"),
            ("keyword", "KEYWORD[]"),
            ("i64", "I64"),
            ("u64", "U64"),
            ("f64", "F64"),
            ("bool", "BOOL"),
            ("datetime", "DATETIME"),
            ("json", "JSON"),
        ];
        for (source, expected) in cases {
            assert_eq!(schema_data_type(source), expected);
        }
    }

    #[test]
    fn deserializes_current_schema_v3_wire_contract() {
        let schema: SearchSchemaResponse = serde_json::from_value(json!({
            "version": 3,
            "tables": ["documents"],
            "functions": ["COUNT", "YEAR", "MONTH", "DAY"],
            "qualifier_controls": ["case"],
            "schemas": [{
                "fields": [{
                    "name": "body",
                    "field_type": "text",
                    "stored": true,
                    "columnar": false
                }],
                "indexes": [{
                    "name": "body.tokens",
                    "field": "body",
                    "kind": "token",
                    "transform": [],
                    "case_fold": true,
                    "tokenizer": "simple"
                }],
                "default_search": ["body.tokens"],
                "sort": null,
                "operational": {"time": null, "namespace": null, "tags": null}
            }]
        }))
        .unwrap();

        assert_eq!(schema.version, 3);
        assert_eq!(merged_schema_columns(&schema)[0].data_type, "TEXT[]");
        assert_eq!(
            schema_qualifiers(&schema),
            ["body.*", "body.tokens", "case"]
        );
    }

    #[test]
    fn prepares_structured_named_field_search_request() {
        let prepared = prepare_search_request(
            r#"{
                "q": "headline:Dragon",
                "limit": 3,
                "sort": {"named_field": "rating", "direction": "desc"}
            }"#,
        )
        .unwrap();

        assert_eq!(prepared.body["q"], "headline:Dragon");
        assert_eq!(prepared.body["limit"], 3);
        assert_eq!(prepared.body["sort"]["named_field"], "rating");
        assert!(prepared.body.get("timeout_ms").is_some());
        assert_eq!(prepared.group_column, None);
        assert_eq!(prepared.hit_projection, None);
    }

    #[test]
    fn stored_field_projection_is_expanded_for_white_dragon() {
        let prepared = prepare_search_request(
            r"Select patch from documents where patch.code ~ '\btoto\b' limit 1",
        )
        .unwrap();

        assert_eq!(
            prepared.body["q"],
            r"Select * from documents where patch.code ~ '\btoto\b' limit 1"
        );
        assert_eq!(prepared.hit_projection, Some(vec!["patch".to_string()]));
        assert_eq!(prepared.group_column, None);
    }

    #[test]
    fn multiple_and_quoted_stored_fields_preserve_select_order() {
        let prepared =
            prepare_search_request(r#"SELECT repo, "commit_sha" FROM documents LIMIT 2"#).unwrap();

        assert_eq!(prepared.body["q"], "SELECT * FROM documents LIMIT 2");
        assert_eq!(
            prepared.hit_projection,
            Some(vec!["repo".to_string(), "commit_sha".to_string()])
        );
    }

    #[test]
    fn aggregate_select_lists_are_not_rewritten() {
        for query in [
            "SELECT COUNT(*) FROM documents",
            "SELECT visibility, COUNT(*) FROM documents GROUP BY visibility",
            "SELECT month(published), COUNT(*) FROM documents GROUP BY month(published)",
            "SELECT * FROM documents",
        ] {
            let prepared = prepare_search_request(query).unwrap();
            assert_eq!(prepared.body["q"], query);
            assert_eq!(prepared.hit_projection, None);
        }
    }

    #[test]
    fn structured_sql_projection_is_expanded_without_losing_policies() {
        let prepared = prepare_search_request(
            r#"{
                "q": "SELECT headline, priority FROM documents LIMIT 3",
                "sort": {"named_field": "rating", "direction": "desc"}
            }"#,
        )
        .unwrap();

        assert_eq!(prepared.body["q"], "SELECT * FROM documents LIMIT 3");
        assert_eq!(prepared.body["sort"]["named_field"], "rating");
        assert_eq!(
            prepared.hit_projection,
            Some(vec!["headline".to_string(), "priority".to_string()])
        );
    }

    #[test]
    fn projection_requires_stored_source_fields() {
        let fields = ["patch".to_string(), "repo".to_string()];
        validate_hit_projection(Some(&["patch".to_string()]), &fields).unwrap();
        let error =
            validate_hit_projection(Some(&["patch.code".to_string()]), &fields).unwrap_err();
        assert!(error.to_string().contains("not a stored source field"));
    }

    #[test]
    fn structured_aggregations_preserve_generic_group_columns() {
        let terms = prepare_search_request(
            r#"{
                "q": "headline:Dragon",
                "aggregation": {
                    "terms": {"field": "visibility", "size": 10}
                }
            }"#,
        )
        .unwrap();
        assert_eq!(terms.group_column.as_deref(), Some("visibility"));

        let histogram = prepare_search_request(
            r#"{
                "q": "headline:Dragon",
                "aggregation": {
                    "date_histogram": {
                        "field": "published",
                        "interval": "month",
                        "size": 12
                    }
                }
            }"#,
        )
        .unwrap();
        assert_eq!(histogram.group_column.as_deref(), Some("month(published)"));
    }

    #[test]
    fn rejects_malformed_structured_search_request() {
        let error = prepare_search_request(r#"{"limit": 3}"#).unwrap_err();
        assert!(error.to_string().contains("requires a string 'q' field"));
    }

    #[test]
    fn maps_generic_hit_fields_to_a_flat_table() {
        let result = response_to_result(
            "SELECT * FROM documents WHERE headline MATCH 'rust'",
            SearchResponse {
                hits: vec![SearchHit {
                    split_id: "01".to_string(),
                    doc_id: 7,
                    id: "abc".to_string(),
                    score: 8,
                    fields: BTreeMap::from([
                        ("attributes".to_string(), json!({"reviewed": true})),
                        ("headline".to_string(), json!(["Rust search"])),
                        ("priority".to_string(), json!(7)),
                    ]),
                    snippet: "Rust search".to_string(),
                    index: "headline".to_string(),
                }],
                partial: false,
                partial_splits: Vec::new(),
                aggs: None,
            },
        );
        assert_eq!(
            result.columns,
            [
                "split_id",
                "doc_id",
                "id",
                "score",
                "attributes",
                "headline",
                "priority",
                "snippet",
                "index",
            ]
        );
        assert_eq!(result.rows[0][4].as_deref(), Some(r#"{"reviewed":true}"#));
        assert_eq!(result.rows[0][5].as_deref(), Some(r#"["Rust search"]"#));
        assert_eq!(result.rows[0][6].as_deref(), Some("7"));
        assert_eq!(result.rows[0][7].as_deref(), Some("Rust search"));
        assert_eq!(result.rows[0][8].as_deref(), Some("headline"));
    }

    #[test]
    fn empty_search_keeps_all_stored_schema_fields() {
        let result = response_to_result_with_group(
            None,
            None,
            SearchResponse {
                hits: Vec::new(),
                partial: false,
                partial_splits: Vec::new(),
                aggs: None,
            },
            &["priority".to_string(), "visibility".to_string()],
        );

        assert_eq!(&result.columns[4..6], ["priority", "visibility"]);
        assert!(result.rows.is_empty());
    }

    #[test]
    fn projected_hits_contain_only_selected_fields_in_select_order() {
        let mut first = search_hit();
        first.fields = BTreeMap::from([
            ("patch".to_string(), json!(["+toto();"])),
            ("repo".to_string(), json!(["example/repo"])),
        ]);
        let second = search_hit();
        let result = response_to_result(
            "SELECT repo, patch FROM documents WHERE patch.code MATCH 'toto'",
            SearchResponse {
                hits: vec![first, second],
                partial: false,
                partial_splits: Vec::new(),
                aggs: None,
            },
        );

        assert_eq!(result.columns, ["repo", "patch"]);
        assert_eq!(result.rows[0][0].as_deref(), Some(r#"["example/repo"]"#));
        assert_eq!(result.rows[0][1].as_deref(), Some(r#"["+toto();"]"#));
        assert_eq!(result.rows[1], [None, None]);
    }

    #[test]
    fn generic_fields_preserve_missing_values_as_null() {
        let mut first = search_hit();
        first.fields = BTreeMap::from([("priority".to_string(), json!(8))]);
        let mut second = search_hit();
        second.doc_id = 8;
        second.fields = BTreeMap::from([("rating".to_string(), json!(4.8))]);

        let result = response_to_result(
            "SELECT * FROM documents",
            SearchResponse {
                hits: vec![first, second],
                partial: false,
                partial_splits: Vec::new(),
                aggs: None,
            },
        );

        assert_eq!(&result.columns[4..6], ["priority", "rating"]);
        assert_eq!(result.rows[0][4].as_deref(), Some("8"));
        assert_eq!(result.rows[0][5], None);
        assert_eq!(result.rows[1][4], None);
        assert_eq!(result.rows[1][5].as_deref(), Some("4.8"));
    }

    #[test]
    fn mapped_field_names_colliding_with_hit_envelope_are_unambiguous() {
        let mut hit = search_hit();
        hit.fields = BTreeMap::from([
            ("index".to_string(), json!(["mapped-index"])),
            ("score".to_string(), json!(42)),
        ]);
        let result = response_to_result(
            "SELECT * FROM documents",
            SearchResponse {
                hits: vec![hit],
                partial: false,
                partial_splits: Vec::new(),
                aggs: None,
            },
        );
        assert!(result.columns.contains(&"fields.index".to_string()));
        assert!(result.columns.contains(&"fields.score".to_string()));
    }

    #[test]
    fn deserializes_current_generic_wire_response() {
        let response: SearchResponse = serde_json::from_value(json!({
            "hits": [{
                "split_id": "00000000",
                "doc_id": 3,
                "id": "abc",
                "score": 42,
                "fields": {"priority": 8, "visibility": ["public"]},
                "snippet": "White Dragon",
                "index": "headline"
            }],
            "stats": {"partial": true},
            "partial": true,
            "partial_splits": ["00000001"]
        }))
        .unwrap();

        assert!(response.partial);
        assert_eq!(response.partial_splits, ["00000001"]);
        assert_eq!(response.hits[0].id, "abc");
        assert_eq!(response.hits[0].fields["priority"], json!(8));
        assert_eq!(response.hits[0].index, "headline");
    }

    #[test]
    fn maps_grouped_aggregation_to_sql_shaped_table() {
        let result = response_to_result(
            "SELECT visibility, COUNT(*) FROM documents GROUP BY visibility",
            SearchResponse {
                hits: Vec::new(),
                partial: false,
                partial_splits: Vec::new(),
                aggs: Some(AggregationResponse {
                    buckets: vec![AggregationBucket {
                        value: "public".to_string(),
                        count: 3,
                    }],
                    total_matches: 3,
                    approximate: false,
                }),
            },
        );
        assert_eq!(result.columns, ["visibility", "count"]);
        assert_eq!(result.rows[0][1].as_deref(), Some("3"));
    }

    #[test]
    fn empty_grouped_aggregation_has_grouped_columns_and_no_rows() {
        let result = response_to_result(
            "SELECT COUNT(*) FROM documents GROUP BY month(published)",
            SearchResponse {
                hits: Vec::new(),
                partial: false,
                partial_splits: Vec::new(),
                aggs: Some(AggregationResponse {
                    buckets: Vec::new(),
                    total_matches: 0,
                    approximate: false,
                }),
            },
        );
        assert_eq!(result.columns, ["month(published)", "count"]);
        assert!(result.rows.is_empty());
    }

    #[test]
    fn count_aggregation_keeps_its_single_total_row() {
        let result = response_to_result(
            "SELECT COUNT(*) FROM documents",
            SearchResponse {
                hits: Vec::new(),
                partial: false,
                partial_splits: Vec::new(),
                aggs: Some(AggregationResponse {
                    buckets: Vec::new(),
                    total_matches: 12,
                    approximate: false,
                }),
            },
        );
        assert_eq!(result.columns, ["count"]);
        assert_eq!(result.rows, [[Some("12".to_string())]]);
    }
}
