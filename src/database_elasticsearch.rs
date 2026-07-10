//! Elasticsearch implementation of the database abstraction layer
use crate::complex_display::ComplexDisplayConfig;
use crate::database::{
    ConnectionInfo, DatabaseClient, DatabaseError, DatabaseTypeExt, MetadataProvider, ServerInfo,
    StructuredQueryResult,
};
use crate::regex_operators::{RegexTarget, translate_regex_operators};
use async_trait::async_trait;
use elasticsearch::{
    Elasticsearch, SearchParts,
    auth::Credentials,
    cat::CatIndicesParts,
    cert::CertificateValidation,
    http::{
        Url,
        transport::{SingleNodeConnectionPool, TransportBuilder},
    },
    indices::{IndicesExistsParts, IndicesGetMappingParts},
};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use tracing::{debug, info};

fn elasticsearch_error_reason(body: &Value) -> Option<String> {
    let error = body.get("error")?;
    if let Some(reason) = error.as_str() {
        return Some(reason.to_string());
    }
    // Mapping responses are keyed by index name, so an index literally named
    // "error" produces a top-level "error" key on a SUCCESSFUL response. Only
    // treat the value as an error when it carries the documented error shape.
    let error_object = error.as_object()?;
    if !(error_object.contains_key("reason")
        || error_object.contains_key("type")
        || error_object.contains_key("root_cause"))
    {
        return None;
    }
    if let Some(reason) = error.get("reason").and_then(Value::as_str) {
        return Some(reason.to_string());
    }
    if let Some(reason) = error
        .get("root_cause")
        .and_then(Value::as_array)
        .and_then(|causes| causes.first())
        .and_then(|cause| cause.get("reason"))
        .and_then(Value::as_str)
    {
        return Some(reason.to_string());
    }
    error
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| Some(error.to_string()))
}

fn ensure_elasticsearch_success(body: &Value, operation: &str) -> Result<(), DatabaseError> {
    if let Some(reason) = elasticsearch_error_reason(body) {
        Err(DatabaseError::QueryError(format!(
            "Elasticsearch {operation} failed: {reason}"
        )))
    } else {
        Ok(())
    }
}

/// Elasticsearch metadata provider implementation
pub struct ElasticsearchMetadataProvider {
    client: Elasticsearch,
    default_index: Option<String>,
}

impl ElasticsearchMetadataProvider {
    pub fn new(client: Elasticsearch, default_index: Option<String>) -> Self {
        Self {
            client,
            default_index,
        }
    }
}

#[async_trait]
impl MetadataProvider for ElasticsearchMetadataProvider {
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        // Elasticsearch doesn't have traditional schemas, return index patterns
        let response = self
            .client
            .cat()
            .indices(CatIndicesParts::None)
            .format("json")
            .send()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to list indices: {e}")))?;

        let body: Value = response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse indices response: {e}"))
        })?;
        ensure_elasticsearch_success(&body, "index listing")?;

        let mut schemas = Vec::new();
        if let Some(indices) = body.as_array() {
            for index in indices {
                if let Some(index_name) = index.get("index").and_then(|v| v.as_str()) {
                    // Extract index patterns (everything before the first number or date)
                    let parts: Vec<&str> = index_name.split('-').collect();
                    if !parts.is_empty() {
                        let pattern = format!("{}*", parts[0]);
                        if !schemas.contains(&pattern) {
                            schemas.push(pattern);
                        }
                    }
                }
            }
        }

        if schemas.is_empty() {
            schemas.push("*".to_string()); // Default to match all indices
        }

        Ok(schemas)
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        // For Elasticsearch, indices are like tables
        let index_pattern = schema.unwrap_or("*");

        let response = self
            .client
            .cat()
            .indices(CatIndicesParts::Index(&[index_pattern]))
            .format("json")
            .send()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to list indices: {e}")))?;

        let body: Value = response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse indices response: {e}"))
        })?;
        ensure_elasticsearch_success(&body, "index listing")?;

        let mut tables = Vec::new();
        if let Some(indices) = body.as_array() {
            for index in indices {
                if let Some(index_name) = index.get("index").and_then(|v| v.as_str()) {
                    // Just return the clean index name - the completion system will handle quoting
                    tables.push(index_name.to_string());
                }
            }
        }

        Ok(tables)
    }

    async fn get_columns(
        &self,
        table: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<String>, DatabaseError> {
        let clean_table = Self::clean_mapping_target(table);

        // Get mappings for the index, alias, or wildcard and extract nested
        // fields and multi-fields (for example `message.keyword`). Mapping
        // responses are keyed by the concrete backing index, which is not
        // necessarily the name that was requested.
        let response = self
            .client
            .indices()
            .get_mapping(IndicesGetMappingParts::Index(&[&clean_table]))
            .send()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to get mapping: {e}")))?;

        let body: Value = response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse mapping response: {e}"))
        })?;
        ensure_elasticsearch_success(&body, "mapping lookup")?;

        let mut columns = Self::completion_columns_from_mapping(&body);

        // If no mapping found, try to get sample documents to infer fields.
        if columns.is_empty() {
            let search_response = self
                .client
                .search(SearchParts::Index(&[&clean_table]))
                .body(json!({
                    "size": 1,
                    "query": {
                        "match_all": {}
                    }
                }))
                .send()
                .await
                .map_err(|e| {
                    DatabaseError::QueryError(format!("Failed to get sample document: {e}"))
                })?;

            let search_body: Value = search_response.json().await.map_err(|e| {
                DatabaseError::QueryError(format!("Failed to parse search response: {e}"))
            })?;
            ensure_elasticsearch_success(&search_body, "sample document lookup")?;

            if let Some(hits) = search_body
                .get("hits")
                .and_then(|h| h.get("hits"))
                .and_then(|h| h.as_array())
            {
                if let Some(first_hit) = hits.first() {
                    if let Some(source) = first_hit.get("_source").and_then(|s| s.as_object()) {
                        for field_name in source.keys() {
                            columns.push(field_name.clone());
                        }
                    }
                }
            }
        }

        columns.sort();
        columns.dedup();
        Ok(columns)
    }

    async fn get_functions(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        // Return Elasticsearch SQL functions
        Ok(vec![
            "COUNT".to_string(),
            "SUM".to_string(),
            "AVG".to_string(),
            "MIN".to_string(),
            "MAX".to_string(),
            "MATCH".to_string(),
            "QUERY".to_string(),
            "SCORE".to_string(),
            "DATE_HISTOGRAM".to_string(),
            "TERMS".to_string(),
            "CARDINALITY".to_string(),
        ])
    }

    async fn get_table_details(
        &self,
        table: &str,
        _schema: Option<&str>,
    ) -> Result<crate::db::TableDetails, DatabaseError> {
        // Clean the table name (remove display hints) and handle quoting
        let clean_table_name = ElasticsearchClient::clean_table_name(table);
        debug!(
            "[ElasticsearchMetadataProvider::get_table_details] Processing table: '{}' -> '{}'",
            table, clean_table_name
        );

        // Get index statistics and mapping details
        let mapping_response = self
            .client
            .indices()
            .get_mapping(IndicesGetMappingParts::Index(&[&clean_table_name]))
            .send()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to get mapping: {e}")))?;

        let mapping_body: Value = mapping_response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse mapping response: {e}"))
        })?;
        ensure_elasticsearch_success(&mapping_body, "mapping lookup")?;

        let mut columns = Vec::new();

        if let Some(index_mapping) = mapping_body.get(&clean_table_name) {
            if let Some(mappings) = index_mapping.get("mappings") {
                if let Some(properties) = mappings.get("properties") {
                    // Extract ALL fields recursively (nested fields, multi-fields, etc.)
                    self.extract_all_fields_for_table_details(properties, "", &mut columns);
                }
            }
        }

        // Get index statistics
        let stats_response = self
            .client
            .cat()
            .indices(CatIndicesParts::Index(&[&clean_table_name]))
            .format("json")
            .send()
            .await;

        let mut additional_info = HashMap::new();
        if let Ok(stats_resp) = stats_response {
            if let Ok(stats_body) = stats_resp.json::<Value>().await {
                if let Some(indices) = stats_body.as_array() {
                    if let Some(index_stats) = indices.first() {
                        if let Some(doc_count) = index_stats.get("docs.count") {
                            additional_info
                                .insert("document_count".to_string(), doc_count.to_string());
                        }
                        if let Some(store_size) = index_stats.get("store.size") {
                            additional_info
                                .insert("store_size".to_string(), store_size.to_string());
                        }
                        if let Some(health) = index_stats.get("health") {
                            additional_info.insert("health".to_string(), health.to_string());
                        }
                    }
                }
            }
        }

        Ok(crate::db::TableDetails {
            name: clean_table_name.clone(),
            schema: "".to_string(),
            full_name: clean_table_name,
            columns,
            indexes: Vec::new(), // Elasticsearch doesn't have traditional indexes
            check_constraints: Vec::new(),
            foreign_keys: Vec::new(),
            referenced_by: Vec::new(),
            nested_field_details: std::collections::HashMap::new(),
        })
    }

    fn supports_explain(&self) -> bool {
        true
    }

    fn default_schema(&self) -> Option<String> {
        self.default_index.clone().or_else(|| Some("*".to_string()))
    }
}

/// Elasticsearch database client implementation
pub struct ElasticsearchClient {
    client: Elasticsearch,
    connection_info: ConnectionInfo,
    current_index: String,
    metadata_provider: ElasticsearchMetadataProvider,
    complex_display_config: ComplexDisplayConfig,
}

impl ElasticsearchMetadataProvider {
    fn clean_mapping_target(table: &str) -> String {
        let clean = ElasticsearchClient::clean_table_name(table.trim());
        if clean.len() >= 2
            && ((clean.starts_with('"') && clean.ends_with('"'))
                || (clean.starts_with('`') && clean.ends_with('`')))
        {
            clean[1..clean.len() - 1].to_string()
        } else {
            clean
        }
    }

    fn completion_columns_from_mapping(mapping_body: &Value) -> Vec<String> {
        let mut columns = Vec::new();
        if let Some(indices) = mapping_body.as_object() {
            for index_mapping in indices.values() {
                if let Some(properties) = index_mapping
                    .get("mappings")
                    .and_then(|mappings| mappings.get("properties"))
                {
                    Self::extract_completion_fields(properties, "", &mut columns);
                }
            }
        }
        columns.sort();
        columns.dedup();
        columns
    }

    fn extract_completion_fields(properties: &Value, prefix: &str, columns: &mut Vec<String>) {
        let Some(properties) = properties.as_object() else {
            return;
        };

        for (field_name, field_definition) in properties {
            let full_name = if prefix.is_empty() {
                field_name.clone()
            } else {
                format!("{prefix}.{field_name}")
            };

            let field_type = field_definition.get("type").and_then(Value::as_str);
            if !matches!(field_type, Some("object" | "nested")) && field_type.is_some() {
                columns.push(full_name.clone());
            }

            if let Some(multi_fields) = field_definition.get("fields").and_then(Value::as_object) {
                for sub_field_name in multi_fields.keys() {
                    columns.push(format!("{full_name}.{sub_field_name}"));
                }
            }

            if let Some(nested_properties) = field_definition.get("properties") {
                Self::extract_completion_fields(nested_properties, &full_name, columns);
            }
        }
    }

    /// Extract all fields recursively including nested fields and multi-fields
    fn extract_all_fields_for_table_details(
        &self,
        properties: &Value,
        prefix: &str,
        columns: &mut Vec<crate::db::ColumnInfo>,
    ) {
        if let Some(props) = properties.as_object() {
            for (field_name, field_def) in props {
                let full_field_name = if prefix.is_empty() {
                    field_name.clone()
                } else {
                    format!("{prefix}.{field_name}")
                };

                // Add the main field if it has a type
                if let Some(field_type) = field_def.get("type").and_then(|t| t.as_str()) {
                    // Determine field capabilities based on type and mapping
                    let (enhanced_type, capabilities) =
                        self.analyze_field_capabilities(field_type, field_def);

                    columns.push(crate::db::ColumnInfo {
                        name: full_field_name.clone(),
                        data_type: enhanced_type,
                        collation: capabilities, // Store capabilities info (will be displayed as "Capabilities")
                        nullable: true,          // Elasticsearch fields can be null
                        default_value: None,
                        enum_values: None, // Elasticsearch doesn't have native enum support
                    });
                }

                // Handle multi-fields (e.g., field.keyword, field.text)
                if let Some(fields) = field_def.get("fields") {
                    if let Some(fields_obj) = fields.as_object() {
                        for (sub_field_name, sub_field_def) in fields_obj {
                            if let Some(sub_field_type) =
                                sub_field_def.get("type").and_then(|t| t.as_str())
                            {
                                let (enhanced_type, capabilities) =
                                    self.analyze_field_capabilities(sub_field_type, sub_field_def);

                                columns.push(crate::db::ColumnInfo {
                                    name: format!("{full_field_name}.{sub_field_name}"),
                                    data_type: enhanced_type,
                                    collation: capabilities,
                                    nullable: true,
                                    default_value: None,
                                    enum_values: None, // Elasticsearch doesn't have native enum support
                                });
                            }
                        }
                    }
                }

                // Recursively handle nested object properties
                if let Some(nested_properties) = field_def.get("properties") {
                    self.extract_all_fields_for_table_details(
                        nested_properties,
                        &full_field_name,
                        columns,
                    );
                }
            }
        }
    }

    /// Analyze field capabilities based on type and mapping properties
    fn analyze_field_capabilities(&self, field_type: &str, field_def: &Value) -> (String, String) {
        let mut capabilities = Vec::new();

        // All fields are selectable unless they're nested/object without a type
        capabilities.push("select");

        // Determine what operations this field supports
        match field_type {
            "keyword" => {
                capabilities.push("filter");
                capabilities.push("group");
                capabilities.push("agg");
                capabilities.push("sort");
            }
            "text" => {
                capabilities.push("search");
                // Check if it has doc_values for aggregation (rare for text fields)
                if field_def
                    .get("doc_values")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    capabilities.push("agg");
                    capabilities.push("sort");
                }
                // Text fields can be filtered with term queries but not efficiently
                capabilities.push("filter*");
            }
            "long" | "integer" | "short" | "byte" | "double" | "float" | "half_float"
            | "scaled_float" => {
                capabilities.push("filter");
                capabilities.push("group");
                capabilities.push("agg");
                capabilities.push("sort");
                capabilities.push("math");
            }
            "date" => {
                capabilities.push("filter");
                capabilities.push("group");
                capabilities.push("agg");
                capabilities.push("sort");
                capabilities.push("range");
            }
            "boolean" => {
                capabilities.push("filter");
                capabilities.push("group");
                capabilities.push("agg");
            }
            "geo_point" | "geo_shape" => {
                capabilities.push("geo");
                capabilities.push("filter");
            }
            "nested" => {
                capabilities.retain(|&c| c != "select"); // Nested fields need special syntax
                capabilities.push("nested");
            }
            "object" => {
                capabilities.retain(|&c| c != "select"); // Object fields are not directly selectable
                capabilities.push("object");
            }
            "ip" => {
                capabilities.push("filter");
                capabilities.push("group");
                capabilities.push("agg");
                capabilities.push("ip-range");
            }
            _ => {
                capabilities.push("basic");
            }
        }

        // Check if indexing is disabled
        if !field_def
            .get("index")
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
        {
            capabilities.retain(|&c| c != "filter" && c != "search");
            capabilities.push("no-index");
        }

        // Check if doc_values is disabled (affects aggregation and sorting)
        if !field_def
            .get("doc_values")
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
        {
            capabilities.retain(|&c| c != "agg" && c != "sort" && c != "group");
            capabilities.push("no-docval");
        }

        let capabilities_str = if capabilities.is_empty() {
            "none".to_string()
        } else {
            capabilities.join(",")
        };

        (field_type.to_string(), capabilities_str)
    }
}

impl ElasticsearchClient {
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        debug!("[ElasticsearchClient::new] Creating client for connection");

        // Build Elasticsearch URL
        let mut url_string = String::new();

        // Handle scheme
        if connection_info.options.contains_key("ssl")
            && connection_info
                .options
                .get("ssl")
                .is_some_and(|v| v == "true")
        {
            url_string.push_str("https://");
        } else {
            url_string.push_str("http://");
        }

        // Add host and port with *.localhost resolution
        let (connection_host, original_host) = if let Some(host) = &connection_info.host {
            // Resolve *.localhost to 127.0.0.1 for connection, but preserve original
            if host == "localhost" || host.ends_with(".localhost") {
                ("127.0.0.1", Some(host.clone()))
            } else {
                (host.as_str(), None)
            }
        } else {
            ("localhost", None)
        };
        url_string.push_str(connection_host);

        if let Some(port) = connection_info.port {
            url_string.push_str(&format!(":{port}"));
        } else if let Some(default_port) = connection_info.database_type.default_port() {
            url_string.push_str(&format!(":{default_port}"));
        }

        debug!(
            "[ElasticsearchClient::new] Connecting to URL: {}",
            url_string
        );

        // Parse URL
        let url = Url::parse(&url_string).map_err(|e| {
            DatabaseError::ConnectionError(format!("Invalid Elasticsearch URL: {e}"))
        })?;

        // Create connection pool
        let conn_pool = SingleNodeConnectionPool::new(url);
        let mut transport_builder = TransportBuilder::new(conn_pool);

        // Handle authentication
        if let (Some(username), Some(password)) =
            (&connection_info.username, &connection_info.password)
        {
            transport_builder =
                transport_builder.auth(Credentials::Basic(username.clone(), password.clone()));
        }

        // Handle SSL verification
        if connection_info
            .options
            .get("verify_certs")
            .is_some_and(|v| v == "false")
        {
            transport_builder = transport_builder.cert_validation(CertificateValidation::None);
        }

        // If we resolved a *.localhost domain, add the original hostname as a header
        // for proxy routing (but exclude plain "localhost")
        if let Some(ref original) = original_host {
            if original != "localhost" {
                use elasticsearch::http::headers::HeaderMap;
                use elasticsearch::http::headers::HeaderValue;
                let mut default_headers = HeaderMap::new();
                if let Ok(host_value) = HeaderValue::from_str(original) {
                    default_headers.insert("X-Original-Host", host_value);
                }
                transport_builder = transport_builder.headers(default_headers);
            }
        }

        let transport = transport_builder.build().map_err(|e| {
            DatabaseError::ConnectionError(format!("Failed to build Elasticsearch transport: {e}"))
        })?;

        let client = Elasticsearch::new(transport);

        // Test both transport and HTTP/API success. The generated client
        // returns an HTTP response for authentication failures, so `send()`
        // succeeding alone does not mean the connection is usable.
        let info_response = client.info().send().await.map_err(|e| {
            DatabaseError::ConnectionError(format!("Failed to connect to Elasticsearch: {e}"))
        })?;
        let status = info_response.status_code().as_u16();
        let info_body: Value = info_response.json().await.map_err(|e| {
            DatabaseError::ConnectionError(format!(
                "Failed to parse Elasticsearch server response: {e}"
            ))
        })?;
        if !(200..300).contains(&status) || info_body.get("error").is_some() {
            let reason = elasticsearch_error_reason(&info_body)
                .unwrap_or_else(|| format!("server returned HTTP {status}"));
            return Err(DatabaseError::ConnectionError(format!(
                "Elasticsearch connection rejected: {reason}"
            )));
        }

        debug!("[ElasticsearchClient::new] Connection successful");

        let current_index = connection_info
            .database
            .clone()
            .unwrap_or_else(|| "*".to_string());

        let metadata_provider =
            ElasticsearchMetadataProvider::new(client.clone(), Some(current_index.clone()));

        // Initialize complex display configuration
        let complex_display_config = ComplexDisplayConfig::elasticsearch_default();

        Ok(Self {
            client,
            connection_info,
            current_index,
            metadata_provider,
            complex_display_config,
        })
    }

    /// Check if an index name needs quoting for SQL queries
    pub fn needs_quoting(name: &str) -> bool {
        // Elasticsearch identifiers need quoting if they contain special characters
        name.contains('-')
            || name.contains('.')
            || name.contains(':')
            || name.contains(' ')
            || name.contains('@')
            || name.contains('#')
            || name.starts_with(char::is_numeric)
    }

    /// Check if a table name is already quoted
    fn is_already_quoted(name: &str) -> bool {
        (name.starts_with('"') && name.ends_with('"'))
            || (name.starts_with('`') && name.ends_with('`'))
    }

    /// Remove display hints from table name (e.g., "table (use \"table\")")
    fn clean_table_name(name: &str) -> String {
        if let Some(pos) = name.find(" (use ") {
            name[..pos].to_string()
        } else {
            name.to_string()
        }
    }

    /// Automatically quote table names in SQL queries
    fn auto_quote_table_names_in_sql(sql: &str) -> Result<String, DatabaseError> {
        // Regex to match table names after FROM, JOIN, UPDATE, INTO keywords
        let table_regex = Regex::new(r"(?i)\b(FROM|JOIN|UPDATE|INTO)\s+([^\s;,()]+)")
            .map_err(|e| DatabaseError::QueryError(format!("Regex error: {e}")))?;

        let result = table_regex.replace_all(sql, |caps: &regex::Captures| {
            let keyword = &caps[1];
            let table_name = &caps[2];

            // Clean any display hints first
            let clean_name = Self::clean_table_name(table_name);

            // Auto-quote if needed and not already quoted
            if Self::needs_quoting(&clean_name) && !Self::is_already_quoted(&clean_name) {
                format!("{keyword} \"{clean_name}\"")
            } else {
                caps[0].to_string()
            }
        });

        Ok(result.into_owned())
    }

    /// Extract index name from a SELECT query
    fn extract_index_name_from_query(sql: &str) -> Option<String> {
        let sql_upper = sql.to_uppercase();
        if let Some(from_pos) = sql_upper.find("FROM") {
            let after_from = &sql[from_pos + 4..].trim();
            let parts: Vec<&str> = after_from.split_whitespace().collect();
            if !parts.is_empty() {
                let raw_index_name = parts[0];
                // Clean and unquote the index name
                let index_name = raw_index_name.trim_matches('"').trim_matches('`');
                let clean_name = Self::clean_table_name(index_name);
                return Some(clean_name);
            }
        }
        None
    }

    /// Check if query is a SELECT * pattern
    fn is_select_star_query(sql: &str) -> bool {
        let sql_trimmed = sql.trim();
        let sql_upper = sql_trimmed.to_uppercase();
        sql_upper.starts_with("SELECT *")
            || sql_upper.starts_with("SELECT\t*")
            || sql_upper.starts_with("SELECT\n*")
    }

    /// Get field mapping information for an index
    async fn get_field_mappings(
        &self,
        index_name: &str,
    ) -> Result<HashMap<String, String>, DatabaseError> {
        let response = self
            .client
            .indices()
            .get_mapping(IndicesGetMappingParts::Index(&[index_name]))
            .send()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to get mapping: {e}")))?;

        let body: Value = response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse mapping response: {e}"))
        })?;
        ensure_elasticsearch_success(&body, "mapping lookup")?;

        let mut field_types = HashMap::new();

        // Navigate through the mapping structure
        if let Some(index_mapping) = body.get(index_name) {
            if let Some(mappings) = index_mapping.get("mappings") {
                if let Some(properties) = mappings.get("properties") {
                    self.extract_field_types(properties, "", &mut field_types);
                }
            }
        }

        Ok(field_types)
    }

    /// Recursively extract field types from mapping properties
    #[allow(clippy::only_used_in_recursion)]
    fn extract_field_types(
        &self,
        properties: &Value,
        prefix: &str,
        field_types: &mut HashMap<String, String>,
    ) {
        if let Some(props) = properties.as_object() {
            for (field_name, field_def) in props {
                let full_field_name = if prefix.is_empty() {
                    field_name.clone()
                } else {
                    format!("{prefix}.{field_name}")
                };

                if let Some(field_type) = field_def.get("type").and_then(|t| t.as_str()) {
                    field_types.insert(full_field_name.clone(), field_type.to_string());
                }

                // Handle nested objects
                if let Some(nested_properties) = field_def.get("properties") {
                    self.extract_field_types(nested_properties, &full_field_name, field_types);
                }
            }
        }
    }

    /// Determine which fields are safe to query (non-array, non-nested)
    async fn get_safe_queryable_fields(
        &self,
        index_name: &str,
    ) -> Result<Vec<String>, DatabaseError> {
        let field_mappings = self.get_field_mappings(index_name).await?;

        // Get a sample document to identify which fields might be arrays
        let search_response = self
            .client
            .search(SearchParts::Index(&[index_name]))
            .body(json!({
                "size": 1,
                "query": {
                    "match_all": {}
                }
            }))
            .send()
            .await
            .map_err(|e| {
                DatabaseError::QueryError(format!("Failed to get sample document: {e}"))
            })?;

        let search_body: Value = search_response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse search response: {e}"))
        })?;
        ensure_elasticsearch_success(&search_body, "sample document lookup")?;

        let mut safe_fields = Vec::new();
        let mut potentially_array_fields = HashSet::new();

        // Analyze sample document for array fields
        if let Some(hits) = search_body
            .get("hits")
            .and_then(|h| h.get("hits"))
            .and_then(|h| h.as_array())
        {
            if let Some(first_hit) = hits.first() {
                if let Some(source) = first_hit.get("_source") {
                    self.identify_array_fields(source, "", &mut potentially_array_fields);
                }
            }
        }

        // Build list of safe fields (non-array, non-nested)
        for (field_name, field_type) in field_mappings {
            // Skip nested and object fields
            if field_type == "nested" || field_type == "object" {
                continue;
            }

            // Skip fields identified as arrays in the sample document
            if potentially_array_fields.contains(&field_name) {
                continue;
            }

            safe_fields.push(field_name);
        }

        // Sort for consistent output
        safe_fields.sort();
        Ok(safe_fields)
    }

    /// Recursively identify array fields from a document
    #[allow(clippy::only_used_in_recursion)]
    fn identify_array_fields(
        &self,
        value: &Value,
        prefix: &str,
        array_fields: &mut HashSet<String>,
    ) {
        if let Value::Object(obj) = value {
            for (key, val) in obj {
                let full_key = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };

                if val.is_array() {
                    array_fields.insert(full_key.clone());
                } else {
                    self.identify_array_fields(val, &full_key, array_fields);
                }
            }
        }
    }

    /// Rewrite SELECT * query to use safe fields only
    async fn rewrite_select_star_query(
        &self,
        sql: &str,
    ) -> Result<(String, Vec<String>), DatabaseError> {
        if let Some(index_name) = Self::extract_index_name_from_query(sql) {
            let safe_fields = self.get_safe_queryable_fields(&index_name).await?;

            if safe_fields.is_empty() {
                return Err(DatabaseError::QueryError(
                    "No queryable fields found in index (all fields may be arrays or nested)"
                        .to_string(),
                ));
            }

            let columns_list = safe_fields.join(", ");
            let select_star = Regex::new(r"(?is)^(\s*SELECT)\s+\*")
                .map_err(|e| DatabaseError::QueryError(format!("Regex error: {e}")))?;
            let rewritten_query = select_star
                .replace(sql, |captures: &regex::Captures| {
                    format!("{} {columns_list}", &captures[1])
                })
                .into_owned();

            // Get list of excluded fields for user info
            let field_mappings = self.get_field_mappings(&index_name).await?;
            let all_fields: HashSet<String> = field_mappings.keys().cloned().collect();
            let safe_fields_set: HashSet<String> = safe_fields.iter().cloned().collect();
            let excluded_fields: Vec<String> =
                all_fields.difference(&safe_fields_set).cloned().collect();

            Ok((rewritten_query, excluded_fields))
        } else {
            Err(DatabaseError::QueryError(
                "Could not extract index name from query".to_string(),
            ))
        }
    }

    /// Fix SQL query to use proper Elasticsearch quoting (double quotes instead of backticks)
    fn fix_elasticsearch_sql_quoting(sql: &str) -> String {
        // Replace backticks with double quotes for Elasticsearch SQL API
        sql.replace('`', "\"")
    }

    fn prepare_sql_query(sql: &str) -> Result<String, DatabaseError> {
        let auto_quoted_sql = Self::auto_quote_table_names_in_sql(sql)?;
        let normalized_sql = Self::fix_elasticsearch_sql_quoting(&auto_quoted_sql);
        translate_regex_operators(&normalized_sql, RegexTarget::ElasticsearchSql)
    }

    /// Interpret a `_sql/translate` response as a validation verdict.
    ///
    /// The translate API cannot produce a DSL for command statements
    /// (SHOW TABLES, DESCRIBE) or queries Elasticsearch executes locally
    /// (e.g. `SELECT 1`) and reports "Cannot generate a query" for them —
    /// those statements execute fine, so they count as valid.
    fn translate_validation_result(body: &Value) -> Result<(), DatabaseError> {
        match elasticsearch_error_reason(body) {
            Some(reason) if reason.contains("Cannot generate a query") => Ok(()),
            Some(reason) => Err(DatabaseError::QueryError(format!(
                "Elasticsearch query validation failed: {reason}"
            ))),
            None => Ok(()),
        }
    }

    /// Execute SQL query via Elasticsearch SQL API
    async fn execute_sql_query(&self, sql: &str) -> Result<StructuredQueryResult, DatabaseError> {
        debug!(
            "[ElasticsearchClient::execute_sql_query] Executing SQL: {}",
            sql
        );

        // Normalize table quoting consistently for execution, validation, and
        // explain. This is especially important for generated GUI queries.
        let mut final_sql = Self::prepare_sql_query(sql)?;
        if final_sql != sql {
            debug!(
                "[ElasticsearchClient::execute_sql_query] Normalized SQL: {} -> {}",
                sql, final_sql
            );
        }

        // Handle SELECT * queries by rewriting them to exclude array fields
        let mut excluded_fields = Vec::new();
        if Self::is_select_star_query(&final_sql) {
            debug!(
                "[ElasticsearchClient::execute_sql_query] Detected SELECT * query, checking for array fields"
            );

            match self.rewrite_select_star_query(&final_sql).await {
                Ok((rewritten_query, excluded)) => {
                    if !excluded.is_empty() {
                        debug!(
                            "[ElasticsearchClient::execute_sql_query] Rewritten query to exclude {} array fields",
                            excluded.len()
                        );
                        final_sql = rewritten_query;
                        excluded_fields = excluded;
                    } else {
                        debug!(
                            "[ElasticsearchClient::execute_sql_query] No array fields found, using original SELECT *"
                        );
                    }
                }
                Err(e) => {
                    debug!(
                        "[ElasticsearchClient::execute_sql_query] Failed to rewrite SELECT * query: {}",
                        e
                    );
                    // Continue with original query and let Elasticsearch handle the error
                }
            }
        }

        let response = self
            .client
            .sql()
            .query()
            .format("json") // Format as URL parameter
            .body(json!({
                "query": final_sql,
                "fetch_size": 1000
            }))
            .send()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("SQL query failed: {e}")))?;

        let body: Value = response
            .json()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to parse SQL response: {e}")))?;

        debug!(
            "[ElasticsearchClient::execute_sql_query] Response body: {:?}",
            body
        );

        // Check for errors in response
        if let Some(error) = body.get("error") {
            let error_msg = if let Some(reason) = error.get("reason").and_then(|r| r.as_str()) {
                if reason.contains("backquoted identifiers not supported") {
                    format!(
                        "Elasticsearch SQL Error: {reason}. Hint: Use double quotes (\") instead of backticks (`) for identifiers with special characters."
                    )
                } else if reason.contains("mismatched input")
                    && (reason.contains("-") || reason.contains("."))
                {
                    format!(
                        "Elasticsearch SQL Error: {reason}. Hint: Index names with hyphens, dots, or special characters must be quoted with double quotes (\")."
                    )
                } else {
                    format!("Elasticsearch SQL Error: {reason}")
                }
            } else {
                format!("Elasticsearch SQL Error: {error:?}")
            };
            return Err(DatabaseError::QueryError(error_msg));
        }

        // Parse SQL API response format
        let mut columns: Vec<String> = Vec::new();
        let mut header_present = false;

        if let Some(body_columns) = body.get("columns") {
            if let Some(cols) = body_columns.as_array() {
                header_present = true;
                for col in cols {
                    if let Some(name) = col.get("name").and_then(|n| n.as_str()) {
                        columns.push(name.to_string());
                    }
                }
            }
        }

        let mut rows: Vec<Vec<Option<String>>> = Vec::new();

        if let Some(body_rows) = body.get("rows") {
            if let Some(rows_array) = body_rows.as_array() {
                debug!(
                    "[ElasticsearchClient::execute_sql_query] Found {} rows",
                    rows_array.len()
                );
                for row in rows_array {
                    if let Some(row_array) = row.as_array() {
                        let mut row_cells = Vec::new();
                        for cell in row_array {
                            row_cells.push(Self::elasticsearch_value_to_cell(
                                cell,
                                &self.complex_display_config,
                            ));
                        }
                        rows.push(row_cells);
                    }
                }
            } else {
                debug!("[ElasticsearchClient::execute_sql_query] No rows array found");
            }
        } else {
            debug!("[ElasticsearchClient::execute_sql_query] No 'rows' field in response");
        }

        debug!(
            "[ElasticsearchClient::execute_sql_query] Returning {} result rows (including header)",
            rows.len() + usize::from(header_present)
        );

        // If we have headers but no data rows, add a message row to indicate empty result
        if header_present && rows.is_empty() {
            debug!(
                "[ElasticsearchClient::execute_sql_query] Query returned headers but no data rows"
            );
            // Add a message row to show the empty result explicitly
            let column_count = columns.len();
            if column_count > 0 {
                let mut empty_message_row = vec![Some("(0 rows)".to_string())];
                // Pad with empty strings for remaining columns
                for _ in 1..column_count {
                    empty_message_row.push(Some(String::new()));
                }
                rows.push(empty_message_row);
            }
        }

        // Add informational message about excluded array fields
        if !excluded_fields.is_empty() {
            info!(
                "Note: SELECT * excluded {} array/nested fields: {}",
                excluded_fields.len(),
                excluded_fields.join(", ")
            );

            // Add the message as a comment row at the end
            if header_present || !rows.is_empty() {
                let column_count = if header_present {
                    columns.len()
                } else {
                    rows[0].len()
                };
                let message = format!(
                    "Note: {} array fields excluded: {}",
                    excluded_fields.len(),
                    excluded_fields.join(", ")
                );

                let mut info_row = vec![Some(message)];
                // Pad with empty strings for remaining columns
                for _ in 1..column_count {
                    info_row.push(Some(String::new()));
                }
                rows.push(info_row);
            }
        }

        Ok(StructuredQueryResult { columns, rows })
    }

    /// Map one SQL-API cell onto a display cell, preserving SQL NULL-ness.
    ///
    /// `None` means the server sent a JSON `null` (SQL NULL); a literal
    /// `"NULL"` string value stays `Some("NULL")` so the two remain
    /// distinguishable for structured consumers.
    fn elasticsearch_value_to_cell(value: &Value, config: &ComplexDisplayConfig) -> Option<String> {
        match value {
            Value::Null => None,
            other => Some(Self::format_elasticsearch_value(other, config)),
        }
    }

    /// Format non-NULL Elasticsearch values for display
    fn format_elasticsearch_value(value: &Value, config: &ComplexDisplayConfig) -> String {
        match value {
            Value::Null => "NULL".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            Value::Array(_) | Value::Object(_) => {
                // Use complex display configuration for JSON formatting
                let json_str = if config.json_pretty_print {
                    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
                } else {
                    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
                };

                // Apply display mode formatting
                match &config.display_mode {
                    crate::complex_display::ComplexDisplayMode::Truncated => {
                        let max_len = config.max_width;
                        if json_str.len() > max_len {
                            format!(
                                "{}...",
                                crate::complex_display::truncate_str_bytes(&json_str, max_len)
                            )
                        } else {
                            json_str
                        }
                    }
                    crate::complex_display::ComplexDisplayMode::Summary => {
                        // Show just the type and size for summary mode
                        match value {
                            Value::Array(arr) => format!("[Array: {} items]", arr.len()),
                            Value::Object(obj) => format!("{{Object: {} fields}}", obj.len()),
                            _ => json_str,
                        }
                    }
                    _ => json_str, // Full mode or other modes show complete JSON
                }
            }
        }
    }

    /// Check if query is a supported SQL query
    fn is_sql_query(&self, query: &str) -> bool {
        let query_upper = query.trim().to_uppercase();
        query_upper.starts_with("SELECT")
            || query_upper.starts_with("SHOW")
            || query_upper.starts_with("DESCRIBE")
            || query_upper.starts_with("EXPLAIN")
    }

    /// Handle Elasticsearch-specific commands
    async fn handle_elasticsearch_command(
        &self,
        command: &str,
    ) -> Result<StructuredQueryResult, DatabaseError> {
        let cmd_upper = command.trim().to_uppercase();

        if cmd_upper.starts_with("SHOW TABLES") || cmd_upper.starts_with("SHOW INDICES") {
            return Ok(StructuredQueryResult::from_display_rows(
                self.list_indices().await?,
            ));
        }

        if cmd_upper.starts_with("DESCRIBE ") || cmd_upper.starts_with("DESC ") {
            let table_name = command.split_whitespace().nth(1).unwrap_or("*");
            return Ok(StructuredQueryResult::from_display_rows(
                self.describe_index(table_name).await?,
            ));
        }

        // Default to SQL execution
        self.execute_sql_query(command).await
    }

    /// List all Elasticsearch indices
    async fn list_indices(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[ElasticsearchClient::list_indices] Listing indices");

        let response = self
            .client
            .cat()
            .indices(CatIndicesParts::None)
            .format("json")
            .send()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to list indices: {e}")))?;

        let body: Value = response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse indices response: {e}"))
        })?;
        ensure_elasticsearch_success(&body, "index listing")?;

        let mut results = Vec::new();
        results.push(vec![
            "Index".to_string(),
            "Health".to_string(),
            "Status".to_string(),
            "Documents".to_string(),
            "Size".to_string(),
        ]);

        if let Some(indices) = body.as_array() {
            for index in indices {
                let index_name = index
                    .get("index")
                    .and_then(|v| v.as_str())
                    .unwrap_or("N/A")
                    .to_string();

                let health = index
                    .get("health")
                    .and_then(|v| v.as_str())
                    .unwrap_or("N/A")
                    .to_string();

                let status = index
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("N/A")
                    .to_string();

                let docs_count = index
                    .get("docs.count")
                    .and_then(|v| v.as_str())
                    .unwrap_or("N/A")
                    .to_string();

                let store_size = index
                    .get("store.size")
                    .and_then(|v| v.as_str())
                    .unwrap_or("N/A")
                    .to_string();

                results.push(vec![index_name, health, status, docs_count, store_size]);
            }
        }

        Ok(results)
    }

    /// Describe an Elasticsearch index (show its mapping)
    async fn describe_index(&self, index_name: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        // Clean the index name to remove any display hints
        let clean_index_name = Self::clean_table_name(index_name);
        debug!(
            "[ElasticsearchClient::describe_index] Describing index: '{}' -> '{}'",
            index_name, clean_index_name
        );

        let response = self
            .client
            .indices()
            .get_mapping(IndicesGetMappingParts::Index(&[&clean_index_name]))
            .send()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to get mapping: {e}")))?;

        let body: Value = response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse mapping response: {e}"))
        })?;
        ensure_elasticsearch_success(&body, "mapping lookup")?;

        let mut results = Vec::new();
        results.push(vec![
            "Field".to_string(),
            "Type".to_string(),
            "SQL Compatible".to_string(),
            "Index".to_string(),
        ]);

        // Get sample document to identify array fields
        let mut array_fields = HashSet::new();
        let search_response = self
            .client
            .search(SearchParts::Index(&[&clean_index_name]))
            .body(json!({
                "size": 1,
                "query": {
                    "match_all": {}
                }
            }))
            .send()
            .await;

        if let Ok(search_response) = search_response {
            if let Ok(search_body) = search_response.json::<Value>().await {
                if let Some(hits) = search_body
                    .get("hits")
                    .and_then(|h| h.get("hits"))
                    .and_then(|h| h.as_array())
                {
                    if let Some(first_hit) = hits.first() {
                        if let Some(source) = first_hit.get("_source") {
                            self.identify_array_fields(source, "", &mut array_fields);
                        }
                    }
                }
            }
        }

        // Parse mapping structure
        if let Some(index_mapping) = body.get(&clean_index_name) {
            if let Some(mappings) = index_mapping.get("mappings") {
                if let Some(properties) = mappings.get("properties") {
                    self.describe_properties_recursive(properties, "", &array_fields, &mut results);
                }
            }
        }

        Ok(results)
    }

    /// Recursively describe properties from mapping
    #[allow(clippy::only_used_in_recursion)]
    fn describe_properties_recursive(
        &self,
        properties: &Value,
        prefix: &str,
        array_fields: &HashSet<String>,
        results: &mut Vec<Vec<String>>,
    ) {
        if let Some(props) = properties.as_object() {
            for (field_name, field_def) in props {
                let full_field_name = if prefix.is_empty() {
                    field_name.clone()
                } else {
                    format!("{prefix}.{field_name}")
                };

                let field_type = field_def
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("object");

                let indexed = field_def
                    .get("index")
                    .and_then(|i| i.as_bool())
                    .map(|b| b.to_string())
                    .unwrap_or_else(|| "true".to_string());

                // Determine SQL compatibility
                let sql_compatible = if field_type == "nested" || field_type == "object" {
                    "No (nested/object)".to_string()
                } else if array_fields.contains(&full_field_name) {
                    "No (array)".to_string()
                } else {
                    "Yes".to_string()
                };

                results.push(vec![
                    full_field_name.clone(),
                    field_type.to_string(),
                    sql_compatible,
                    indexed,
                ]);

                // Handle nested objects
                if let Some(nested_properties) = field_def.get("properties") {
                    self.describe_properties_recursive(
                        nested_properties,
                        &full_field_name,
                        array_fields,
                        results,
                    );
                }
            }
        }
    }
}

#[async_trait]
impl DatabaseClient for ElasticsearchClient {
    async fn execute_query(&self, query: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        // "NULL" is this backend's historical display sentinel for JSON null.
        Ok(self
            .execute_query_structured(query)
            .await?
            .into_display_rows("NULL"))
    }

    async fn execute_query_structured(
        &self,
        query: &str,
    ) -> Result<StructuredQueryResult, DatabaseError> {
        debug!(
            "[ElasticsearchClient::execute_query] Executing query: {}",
            query
        );

        if query.trim().is_empty() {
            return Ok(StructuredQueryResult::from_display_rows(vec![vec![
                "No query provided".to_string(),
            ]]));
        }

        let query = query.trim();

        // Handle Elasticsearch-specific commands or SQL queries
        if self.is_sql_query(query) {
            self.execute_sql_query(query).await
        } else {
            self.handle_elasticsearch_command(query).await
        }
    }

    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError> {
        debug!("[ElasticsearchClient::test_query] Testing query: {}", sql);

        let sql = Self::prepare_sql_query(sql)?;
        // Use SQL translate API to validate query without executing
        let response = self
            .client
            .sql()
            .translate()
            .body(json!({
                "query": sql
            }))
            .send()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Query validation failed: {e}")))?;
        let body: Value = response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse validation response: {e}"))
        })?;
        Self::translate_validation_result(&body)
    }

    async fn explain_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!(
            "[ElasticsearchClient::explain_query] Explaining query: {}",
            sql
        );

        let sql = Self::prepare_sql_query(sql)?;
        // Use SQL translate API to show the underlying Elasticsearch query
        let response = self
            .client
            .sql()
            .translate()
            .body(json!({
                "query": sql
            }))
            .send()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Query translation failed: {e}")))?;

        let body: Value = response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse translation response: {e}"))
        })?;
        ensure_elasticsearch_success(&body, "query translation")?;

        let mut results = Vec::new();
        results.push(vec!["Elasticsearch Query".to_string()]);

        let formatted_query =
            serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string());
        results.push(vec![formatted_query]);

        Ok(results)
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        self.explain_query(sql).await
    }

    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        // For Elasticsearch, list indices as databases
        self.list_indices().await
    }

    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError> {
        debug!(
            "[ElasticsearchClient::connect_to_database] Switching to index: {}",
            database
        );

        // Check if index exists
        let exists_response = self
            .client
            .indices()
            .exists(IndicesExistsParts::Index(&[database]))
            .send()
            .await
            .map_err(|e| {
                DatabaseError::ConnectionError(format!("Failed to check index existence: {e}"))
            })?;

        if exists_response.status_code().as_u16() != 200 {
            return Err(DatabaseError::ConnectionError(format!(
                "Index '{database}' does not exist"
            )));
        }

        self.current_index = database.to_string();

        // Update metadata provider with new default index
        self.metadata_provider = ElasticsearchMetadataProvider::new(
            self.client.clone(),
            Some(self.current_index.clone()),
        );

        Ok(())
    }

    fn get_current_database(&self) -> String {
        self.current_index.clone()
    }

    fn get_connection_info(&self) -> &ConnectionInfo {
        &self.connection_info
    }

    fn get_metadata_provider(&self) -> &dyn MetadataProvider {
        &self.metadata_provider
    }

    async fn is_connected(&self) -> bool {
        self.client.info().send().await.is_ok()
    }

    async fn close(&mut self) -> Result<(), DatabaseError> {
        debug!("[ElasticsearchClient::close] Closing connection");
        // Elasticsearch client doesn't need explicit closing
        Ok(())
    }

    async fn get_server_info(&self) -> Result<ServerInfo, DatabaseError> {
        let response =
            self.client.info().send().await.map_err(|e| {
                DatabaseError::QueryError(format!("Failed to get server info: {e}"))
            })?;

        let body: Value = response
            .json()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to parse server info: {e}")))?;

        let version = body
            .get("version")
            .and_then(|v| v.get("number"))
            .and_then(|n| n.as_str())
            .unwrap_or("Unknown")
            .to_string();

        let cluster_name = body
            .get("cluster_name")
            .and_then(|n| n.as_str())
            .unwrap_or("elasticsearch")
            .to_string();

        let mut info = ServerInfo::new("Elasticsearch".to_string(), version);
        info.supports_transactions = false; // Elasticsearch doesn't support transactions
        info.supports_roles = true; // Elasticsearch supports role-based security
        info.additional_info
            .insert("cluster_name".to_string(), cluster_name);
        info.parse_version_numbers();

        Ok(info)
    }
}

impl ComplexDisplayConfig {
    pub fn elasticsearch_default() -> Self {
        Self {
            display_mode: crate::complex_display::ComplexDisplayMode::Truncated,
            truncation_length: 10,
            viz_width: 80,
            show_metadata: false,
            size_threshold: 50,
            show_dimensions: false,
            full_elements_per_row: 5,
            max_width: 200,
            full_show_numbers: false,
            json_pretty_print: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ElasticsearchClient, ElasticsearchMetadataProvider, elasticsearch_error_reason,
        ensure_elasticsearch_success,
    };
    use crate::complex_display::ComplexDisplayConfig;
    use rstest::rstest;
    use serde_json::{Value, json};

    #[rstest]
    #[case(json!(null), None)] // JSON null -> SQL NULL
    #[case(json!("NULL"), Some("NULL"))] // a literal "NULL" string stays text
    #[case(json!(""), Some(""))] // empty string is not NULL
    #[case(json!(42), Some("42"))]
    #[case(json!(true), Some("true"))]
    fn sql_cells_keep_null_distinct_from_null_text(
        #[case] value: Value,
        #[case] expected: Option<&str>,
    ) {
        let config = ComplexDisplayConfig::elasticsearch_default();
        assert_eq!(
            ElasticsearchClient::elasticsearch_value_to_cell(&value, &config),
            expected.map(str::to_string)
        );
    }

    #[test]
    fn completion_columns_include_nested_and_multi_fields() {
        let mapping = json!({
            "logs-2026.07-000001": {
                "mappings": {
                    "properties": {
                        "message": {
                            "type": "text",
                            "fields": { "keyword": { "type": "keyword" } }
                        },
                        "host": {
                            "properties": {
                                "name": { "type": "keyword" }
                            }
                        },
                        "events": {
                            "type": "nested",
                            "properties": {
                                "code": { "type": "integer" }
                            }
                        }
                    }
                }
            }
        });

        assert_eq!(
            ElasticsearchMetadataProvider::completion_columns_from_mapping(&mapping),
            vec![
                "events.code".to_string(),
                "host.name".to_string(),
                "message".to_string(),
                "message.keyword".to_string(),
            ]
        );
    }

    #[test]
    fn mapping_target_removes_sql_identifier_quotes() {
        assert_eq!(
            ElasticsearchMetadataProvider::clean_mapping_target("\"logs-2026.07\""),
            "logs-2026.07"
        );
        assert_eq!(
            ElasticsearchMetadataProvider::clean_mapping_target("`logs-current`"),
            "logs-current"
        );
    }

    #[test]
    fn api_errors_are_not_treated_as_success() {
        let body = json!({
            "error": {
                "type": "security_exception",
                "reason": "missing authentication credentials"
            },
            "status": 401
        });

        assert_eq!(
            elasticsearch_error_reason(&body).as_deref(),
            Some("missing authentication credentials")
        );
        assert!(ensure_elasticsearch_success(&body, "connection").is_err());
        assert!(ensure_elasticsearch_success(&json!({ "name": "node-1" }), "connection").is_ok());
    }

    #[test]
    fn index_named_error_is_not_an_api_error() {
        // GET /error/_mapping succeeds with a body keyed by the index name.
        let mapping_response = json!({
            "error": {
                "mappings": {
                    "properties": { "message": { "type": "text" } }
                }
            }
        });

        assert_eq!(elasticsearch_error_reason(&mapping_response), None);
        assert!(ensure_elasticsearch_success(&mapping_response, "mapping lookup").is_ok());
    }

    #[test]
    fn translate_validation_accepts_untranslatable_commands() {
        // SHOW TABLES / DESCRIBE / SELECT 1 execute fine but cannot be
        // translated to a query DSL; that must not fail validation.
        let planning_error = json!({
            "error": {
                "type": "planning_exception",
                "reason": "Cannot generate a query DSL for a special SQL command"
            },
            "status": 400
        });
        assert!(ElasticsearchClient::translate_validation_result(&planning_error).is_ok());

        let real_error = json!({
            "error": {
                "type": "verification_exception",
                "reason": "Unknown column [nope]"
            },
            "status": 400
        });
        assert!(ElasticsearchClient::translate_validation_result(&real_error).is_err());
        assert!(
            ElasticsearchClient::translate_validation_result(&json!({ "size": 10, "query": {} }))
                .is_ok()
        );
    }

    #[test]
    fn sql_preparation_quotes_elasticsearch_indices() {
        assert_eq!(
            ElasticsearchClient::prepare_sql_query("select * from logs-2026.07").unwrap(),
            "select * from \"logs-2026.07\""
        );
        assert_eq!(
            ElasticsearchClient::prepare_sql_query("EXPLAIN SELECT * FROM `logs-current`").unwrap(),
            "EXPLAIN SELECT * FROM \"logs-current\""
        );
    }
}
