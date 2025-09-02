//! Elasticsearch implementation of the database abstraction layer
use crate::complex_display::ComplexDisplayConfig;
use crate::database::{
    ConnectionInfo, DatabaseClient, DatabaseError, DatabaseTypeExt, MetadataProvider, ServerInfo,
};
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
            .map_err(|e| DatabaseError::QueryError(format!("Failed to list indices: {}", e)))?;

        let body: Value = response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse indices response: {}", e))
        })?;

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
            .map_err(|e| DatabaseError::QueryError(format!("Failed to list indices: {}", e)))?;

        let body: Value = response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse indices response: {}", e))
        })?;

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
        // Get mapping for the index to extract field names
        let response = self
            .client
            .indices()
            .get_mapping(IndicesGetMappingParts::Index(&[table]))
            .send()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to get mapping: {}", e)))?;

        let body: Value = response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse mapping response: {}", e))
        })?;

        let mut columns = Vec::new();

        // Navigate through the mapping structure
        if let Some(index_mapping) = body.get(table) {
            if let Some(mappings) = index_mapping.get("mappings") {
                if let Some(properties) = mappings.get("properties") {
                    if let Some(props) = properties.as_object() {
                        for (field_name, _field_def) in props {
                            columns.push(field_name.clone());
                        }
                    }
                }
            }
        }

        // If no mapping found, try to get sample documents to infer fields
        if columns.is_empty() {
            let search_response = self
                .client
                .search(SearchParts::Index(&[table]))
                .body(json!({
                    "size": 1,
                    "query": {
                        "match_all": {}
                    }
                }))
                .send()
                .await
                .map_err(|e| {
                    DatabaseError::QueryError(format!("Failed to get sample document: {}", e))
                })?;

            let search_body: Value = search_response.json().await.map_err(|e| {
                DatabaseError::QueryError(format!("Failed to parse search response: {}", e))
            })?;

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
            .map_err(|e| DatabaseError::QueryError(format!("Failed to get mapping: {}", e)))?;

        let mapping_body: Value = mapping_response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse mapping response: {}", e))
        })?;

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
                    format!("{}.{}", prefix, field_name)
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
                                    name: format!("{}.{}", full_field_name, sub_field_name),
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
        if field_def
            .get("index")
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
            == false
        {
            capabilities.retain(|&c| c != "filter" && c != "search");
            capabilities.push("no-index");
        }

        // Check if doc_values is disabled (affects aggregation and sorting)
        if field_def
            .get("doc_values")
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
            == false
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
                .map_or(false, |v| v == "true")
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
            url_string.push_str(&format!(":{}", port));
        } else if let Some(default_port) = connection_info.database_type.default_port() {
            url_string.push_str(&format!(":{}", default_port));
        }

        debug!(
            "[ElasticsearchClient::new] Connecting to URL: {}",
            url_string
        );

        // Parse URL
        let url = Url::parse(&url_string).map_err(|e| {
            DatabaseError::ConnectionError(format!("Invalid Elasticsearch URL: {}", e))
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
            .map_or(false, |v| v == "false")
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
            DatabaseError::ConnectionError(format!(
                "Failed to build Elasticsearch transport: {}",
                e
            ))
        })?;

        let client = Elasticsearch::new(transport);

        // Test connection
        let _info_response = client.info().send().await.map_err(|e| {
            DatabaseError::ConnectionError(format!("Failed to connect to Elasticsearch: {}", e))
        })?;

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
            .map_err(|e| DatabaseError::QueryError(format!("Regex error: {}", e)))?;

        let result = table_regex.replace_all(sql, |caps: &regex::Captures| {
            let keyword = &caps[1];
            let table_name = &caps[2];

            // Clean any display hints first
            let clean_name = Self::clean_table_name(table_name);

            // Auto-quote if needed and not already quoted
            if Self::needs_quoting(&clean_name) && !Self::is_already_quoted(&clean_name) {
                format!("{} \"{}\"", keyword, clean_name)
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
            .map_err(|e| DatabaseError::QueryError(format!("Failed to get mapping: {}", e)))?;

        let body: Value = response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse mapping response: {}", e))
        })?;

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
                    format!("{}.{}", prefix, field_name)
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
                DatabaseError::QueryError(format!("Failed to get sample document: {}", e))
            })?;

        let search_body: Value = search_response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse search response: {}", e))
        })?;

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
    fn identify_array_fields(
        &self,
        value: &Value,
        prefix: &str,
        array_fields: &mut HashSet<String>,
    ) {
        match value {
            Value::Object(obj) => {
                for (key, val) in obj {
                    let full_key = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{}.{}", prefix, key)
                    };

                    if val.is_array() {
                        array_fields.insert(full_key.clone());
                    } else {
                        self.identify_array_fields(val, &full_key, array_fields);
                    }
                }
            }
            _ => {}
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
            let rewritten_query = sql.replacen("SELECT *", &format!("SELECT {}", columns_list), 1);

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

    /// Execute SQL query via Elasticsearch SQL API
    async fn execute_sql_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!(
            "[ElasticsearchClient::execute_sql_query] Executing SQL: {}",
            sql
        );

        // First, automatically quote table names in SQL
        let auto_quoted_sql = Self::auto_quote_table_names_in_sql(sql)?;
        if auto_quoted_sql != sql {
            debug!(
                "[ElasticsearchClient::execute_sql_query] Auto-quoted table names: {} -> {}",
                sql, auto_quoted_sql
            );
        }

        // Fix quoting for Elasticsearch SQL API (backticks to double quotes)
        let mut final_sql = Self::fix_elasticsearch_sql_quoting(&auto_quoted_sql);
        if final_sql != auto_quoted_sql {
            debug!(
                "[ElasticsearchClient::execute_sql_query] Fixed SQL quoting: {} -> {}",
                auto_quoted_sql, final_sql
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
            .map_err(|e| DatabaseError::QueryError(format!("SQL query failed: {}", e)))?;

        let body: Value = response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse SQL response: {}", e))
        })?;

        debug!(
            "[ElasticsearchClient::execute_sql_query] Response body: {:?}",
            body
        );

        // Check for errors in response
        if let Some(error) = body.get("error") {
            let error_msg = if let Some(reason) = error.get("reason").and_then(|r| r.as_str()) {
                if reason.contains("backquoted identifiers not supported") {
                    format!(
                        "Elasticsearch SQL Error: {}. Hint: Use double quotes (\") instead of backticks (`) for identifiers with special characters.",
                        reason
                    )
                } else if reason.contains("mismatched input")
                    && (reason.contains("-") || reason.contains("."))
                {
                    format!(
                        "Elasticsearch SQL Error: {}. Hint: Index names with hyphens, dots, or special characters must be quoted with double quotes (\").",
                        reason
                    )
                } else {
                    format!("Elasticsearch SQL Error: {}", reason)
                }
            } else {
                format!("Elasticsearch SQL Error: {:?}", error)
            };
            return Err(DatabaseError::QueryError(error_msg));
        }

        // Parse SQL API response format
        let mut results = Vec::new();

        if let Some(columns) = body.get("columns") {
            if let Some(cols) = columns.as_array() {
                let mut header = Vec::new();
                for col in cols {
                    if let Some(name) = col.get("name").and_then(|n| n.as_str()) {
                        header.push(name.to_string());
                    }
                }
                results.push(header);
            }
        }

        if let Some(rows) = body.get("rows") {
            if let Some(rows_array) = rows.as_array() {
                debug!(
                    "[ElasticsearchClient::execute_sql_query] Found {} rows",
                    rows_array.len()
                );
                for row in rows_array {
                    if let Some(row_array) = row.as_array() {
                        let mut row_strings = Vec::new();
                        for cell in row_array {
                            row_strings.push(self.format_elasticsearch_value(cell));
                        }
                        results.push(row_strings);
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
            results.len()
        );

        // If we have headers but no data rows, add a message row to indicate empty result
        if results.len() == 1 && !results.is_empty() {
            debug!(
                "[ElasticsearchClient::execute_sql_query] Query returned headers but no data rows"
            );
            // Add a message row to show the empty result explicitly
            let column_count = results[0].len();
            if column_count > 0 {
                let mut empty_message_row = vec!["(0 rows)".to_string()];
                // Pad with empty strings for remaining columns
                for _ in 1..column_count {
                    empty_message_row.push("".to_string());
                }
                results.push(empty_message_row);
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
            if !results.is_empty() {
                let column_count = results[0].len();
                let message = format!(
                    "Note: {} array fields excluded: {}",
                    excluded_fields.len(),
                    excluded_fields.join(", ")
                );

                let mut info_row = vec![message];
                // Pad with empty strings for remaining columns
                for _ in 1..column_count {
                    info_row.push("".to_string());
                }
                results.push(info_row);
            }
        }

        Ok(results)
    }

    /// Format Elasticsearch values for display
    fn format_elasticsearch_value(&self, value: &Value) -> String {
        match value {
            Value::Null => "NULL".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            Value::Array(_) | Value::Object(_) => {
                // Use complex display configuration for JSON formatting
                let json_str = if self.complex_display_config.json_pretty_print {
                    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
                } else {
                    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
                };

                // Apply display mode formatting
                match &self.complex_display_config.display_mode {
                    crate::complex_display::ComplexDisplayMode::Truncated => {
                        let max_len = self.complex_display_config.max_width;
                        if json_str.len() > max_len {
                            format!("{}...", &json_str[..max_len])
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
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        let cmd_upper = command.trim().to_uppercase();

        if cmd_upper.starts_with("SHOW TABLES") || cmd_upper.starts_with("SHOW INDICES") {
            return self.list_indices().await;
        }

        if cmd_upper.starts_with("DESCRIBE ") || cmd_upper.starts_with("DESC ") {
            let table_name = command.trim().split_whitespace().nth(1).unwrap_or("*");
            return self.describe_index(table_name).await;
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
            .map_err(|e| DatabaseError::QueryError(format!("Failed to list indices: {}", e)))?;

        let body: Value = response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse indices response: {}", e))
        })?;

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
            .map_err(|e| DatabaseError::QueryError(format!("Failed to get mapping: {}", e)))?;

        let body: Value = response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse mapping response: {}", e))
        })?;

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
                    format!("{}.{}", prefix, field_name)
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
        debug!(
            "[ElasticsearchClient::execute_query] Executing query: {}",
            query
        );

        if query.trim().is_empty() {
            return Ok(vec![vec!["No query provided".to_string()]]);
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

        // Use SQL translate API to validate query without executing
        let _response = self
            .client
            .sql()
            .translate()
            .body(json!({
                "query": sql
            }))
            .send()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Query validation failed: {}", e)))?;

        Ok(())
    }

    async fn explain_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!(
            "[ElasticsearchClient::explain_query] Explaining query: {}",
            sql
        );

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
            .map_err(|e| DatabaseError::QueryError(format!("Query translation failed: {}", e)))?;

        let body: Value = response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse translation response: {}", e))
        })?;

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
                DatabaseError::ConnectionError(format!("Failed to check index existence: {}", e))
            })?;

        if exists_response.status_code().as_u16() != 200 {
            return Err(DatabaseError::ConnectionError(format!(
                "Index '{}' does not exist",
                database
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
                DatabaseError::QueryError(format!("Failed to get server info: {}", e))
            })?;

        let body: Value = response.json().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to parse server info: {}", e))
        })?;

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
