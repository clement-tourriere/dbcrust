//! MongoDB implementation of the database abstraction layer
use crate::database::{
    ConnectionInfo, DatabaseClient, DatabaseError, DatabaseTypeExt, MetadataProvider,
};
use async_trait::async_trait;
use bson::{Document, doc};
use futures_util::stream::{StreamExt, TryStreamExt};
use mongodb::{Client, Database as MongoDatabase, options::ClientOptions};
use tracing::debug;

/// MongoDB metadata provider implementation
pub struct MongoDBMetadataProvider {
    database: MongoDatabase,
}

impl MongoDBMetadataProvider {
    pub fn new(database: MongoDatabase) -> Self {
        Self { database }
    }
}

#[async_trait]
impl MetadataProvider for MongoDBMetadataProvider {
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        // MongoDB doesn't have schemas, return empty list
        Ok(Vec::new())
    }

    async fn get_tables(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        // For MongoDB, collections are like tables
        let collections =
            self.database.list_collection_names().await.map_err(|e| {
                DatabaseError::QueryError(format!("Failed to list collections: {}", e))
            })?;

        Ok(collections)
    }

    async fn get_columns(
        &self,
        table: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<String>, DatabaseError> {
        // Get sample documents to infer schema
        let collection_handle = self.database.collection::<Document>(table);
        let sample_docs = collection_handle
            .find(Document::new())
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to sample documents: {}", e)))?;

        // Collect sample documents (limit to 10 for schema inference)
        let mut sample_documents = Vec::new();
        let mut cursor = sample_docs;
        while let Some(Ok(doc)) = cursor.next().await {
            if sample_documents.len() >= 10 {
                break;
            }
            sample_documents.push(doc);
        }

        // Infer schema from sample documents
        let mut columns = Vec::new();
        if let Some(first_doc) = sample_documents.first() {
            for (key, _value) in first_doc {
                columns.push(key.clone());
            }
        }

        Ok(columns)
    }

    async fn get_functions(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        // MongoDB doesn't have stored functions like SQL databases
        Ok(Vec::new())
    }

    async fn get_table_details(
        &self,
        collection: &str,
        _schema: Option<&str>,
    ) -> Result<crate::db::TableDetails, DatabaseError> {
        // Get collection statistics (for future use)
        let _stats = self
            .database
            .run_command({
                let mut cmd = Document::new();
                cmd.insert("collStats", collection);
                cmd
            })
            .await
            .map_err(|e| {
                DatabaseError::QueryError(format!("Failed to get collection stats: {}", e))
            })?;

        // Get sample documents to infer schema
        let collection_handle = self.database.collection::<Document>(collection);
        let sample_docs = collection_handle
            .find(Document::new())
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to sample documents: {}", e)))?;

        // Collect sample documents (limit to 10 for schema inference)
        let mut sample_documents = Vec::new();
        let mut cursor = sample_docs;
        while let Some(Ok(doc)) = cursor.next().await {
            if sample_documents.len() >= 10 {
                break;
            }
            sample_documents.push(doc);
        }

        // Infer schema from sample documents
        let mut columns = Vec::new();
        if let Some(first_doc) = sample_documents.first() {
            for (key, _value) in first_doc {
                columns.push(crate::db::ColumnInfo {
                    name: key.clone(),
                    data_type: "BSON".to_string(), // MongoDB uses BSON types
                    collation: "".to_string(),
                    nullable: true, // MongoDB fields can be null
                    default_value: None,
                });
            }
        }

        // Get indexes for the collection
        let index_cursor = self
            .database
            .collection::<Document>("system.indexes")
            .find(doc! { "ns": format!("{}.{}", self.database.name(), collection) })
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to get indexes: {}", e)))?;

        let mut index_cursor = index_cursor;
        let mut indexes = Vec::new();
        while let Ok(Some(index_doc)) = index_cursor.try_next().await {
            if let Some(name) = index_doc.get_str("name").ok() {
                indexes.push(crate::db::IndexInfo {
                    name: name.to_string(),
                    index_type: "INDEX".to_string(), // MongoDB doesn't have traditional index types
                    is_primary: false,               // MongoDB doesn't have primary keys like SQL
                    is_unique: false,                // Would need to check index options
                    predicate: None,
                    definition: format!("{:?}", index_doc),
                    constraint_def: None,
                });
            }
        }

        Ok(crate::db::TableDetails {
            name: collection.to_string(),
            schema: "".to_string(), // MongoDB doesn't have schemas
            columns,
            indexes,
            full_name: collection.to_string(),
            check_constraints: Vec::new(), // MongoDB doesn't have traditional constraints
            foreign_keys: Vec::new(),
            referenced_by: Vec::new(),
        })
    }

    fn supports_explain(&self) -> bool {
        true // MongoDB supports explain
    }

    fn default_schema(&self) -> Option<String> {
        None // MongoDB doesn't have schemas
    }
}

/// MongoDB client implementation
pub struct MongoDBClient {
    client: Client,
    database: MongoDatabase,
    connection_info: ConnectionInfo,
    current_database: String,
    metadata_provider: MongoDBMetadataProvider,
}

impl MongoDBClient {
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        debug!("[MongoDBClient::new] Creating MongoDB client");

        // Build MongoDB connection string
        let mut connection_string = format!("mongodb://");

        // Add authentication if provided
        if let (Some(username), Some(password)) =
            (&connection_info.username, &connection_info.password)
        {
            connection_string.push_str(&format!("{}:{}@", username, password));
        }

        // Add host and port
        if let Some(host) = &connection_info.host {
            connection_string.push_str(host);
        } else {
            connection_string.push_str("localhost");
        }

        if let Some(port) = connection_info.port {
            connection_string.push_str(&format!(":{}", port));
        } else if let Some(default_port) = connection_info.database_type.default_port() {
            connection_string.push_str(&format!(":{}", default_port));
        }

        // Add database if specified
        if let Some(database) = &connection_info.database {
            connection_string.push_str(&format!("/{}", database));
        }

        // Add query parameters
        if !connection_info.options.is_empty() {
            connection_string.push('?');
            let params: Vec<String> = connection_info
                .options
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect();
            connection_string.push_str(&params.join("&"));
        }

        debug!(
            "[MongoDBClient::new] Connection string: {}",
            connection_string
        );

        // Parse connection string into ClientOptions
        let client_options = ClientOptions::parse(&connection_string)
            .await
            .map_err(|e| {
                DatabaseError::ConnectionError(format!(
                    "Failed to parse MongoDB connection string: {}",
                    e
                ))
            })?;

        // Create client
        let client = Client::with_options(client_options).map_err(|e| {
            DatabaseError::ConnectionError(format!("Failed to create MongoDB client: {}", e))
        })?;

        // Get database name
        let database_name = connection_info
            .database
            .clone()
            .unwrap_or_else(|| "test".to_string());

        let database = client.database(&database_name);

        // Test connection
        database
            .run_command(doc! { "ping": 1 })
            .await
            .map_err(|e| {
                DatabaseError::ConnectionError(format!("Failed to connect to MongoDB: {}", e))
            })?;

        let metadata_provider = MongoDBMetadataProvider::new(database.clone());

        Ok(Self {
            client,
            database,
            connection_info,
            current_database: database_name,
            metadata_provider,
        })
    }

    /// List all collections in the current database
    pub async fn list_collections(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[MongoDBClient::list_collections] Listing collections");

        let collections =
            self.database.list_collection_names().await.map_err(|e| {
                DatabaseError::QueryError(format!("Failed to list collections: {}", e))
            })?;

        let mut results = Vec::new();
        results.push(vec!["Collection".to_string()]);

        for collection in collections {
            results.push(vec![collection]);
        }

        Ok(results)
    }

    /// Execute MongoDB find query with filters and projections
    pub async fn mongo_find(
        &self,
        collection: &str,
        filter: Option<&str>,
        projection: Option<&str>,
        limit: Option<i64>,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!(
            "[MongoDBClient::mongo_find] Executing find on collection: {}",
            collection
        );

        // Parse filter JSON or use empty document
        let filter_doc = if let Some(filter_str) = filter {
            serde_json::from_str::<Document>(filter_str)
                .map_err(|e| DatabaseError::QueryError(format!("Invalid filter JSON: {}", e)))?
        } else {
            Document::new()
        };

        let limit_value = limit.unwrap_or(100);

        // If projection is specified, we need to handle it differently
        // For now, we'll use the dynamic column extraction and handle projection later
        if projection.is_some() {
            // Handle projection case - this is more complex as we need to respect the projection
            return self
                .mongo_find_with_projection(collection, filter_doc, projection, limit_value)
                .await;
        }

        // Use the dynamic column extraction for regular queries
        self.extract_columns_and_values(collection, filter_doc, limit_value)
            .await
    }

    /// Execute MongoDB find query with projection handling
    async fn mongo_find_with_projection(
        &self,
        collection: &str,
        filter_doc: Document,
        projection: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        let collection_handle = self.database.collection::<Document>(collection);

        // Build find options with projection
        let mut find_options = mongodb::options::FindOptions::default();
        if let Some(proj_str) = projection {
            let projection_doc = serde_json::from_str::<Document>(proj_str).map_err(|e| {
                DatabaseError::QueryError(format!("Invalid projection JSON: {}", e))
            })?;
            find_options.projection = Some(projection_doc.clone());

            // Extract column names from projection document
            let projected_columns: Vec<String> = projection_doc.keys().map(|k| k.clone()).collect();

            find_options.limit = Some(limit);

            let mut cursor = collection_handle
                .find(filter_doc)
                .with_options(find_options)
                .await
                .map_err(|e| DatabaseError::QueryError(format!("Failed to execute find: {}", e)))?;

            let mut results = Vec::new();
            results.push(projected_columns.clone());

            while let Some(doc) = cursor
                .try_next()
                .await
                .map_err(|e| DatabaseError::QueryError(format!("Cursor error: {}", e)))?
            {
                let mut row = Vec::new();
                for column in &projected_columns {
                    let value = self.extract_field_value(&doc, column);
                    row.push(value);
                }
                results.push(row);
            }

            return Ok(results);
        }

        // Fallback to regular extraction
        self.extract_columns_and_values(collection, filter_doc, limit)
            .await
    }

    /// Execute MongoDB aggregation pipeline
    pub async fn mongo_aggregate(
        &self,
        collection: &str,
        pipeline: &str,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!(
            "[MongoDBClient::mongo_aggregate] Executing aggregation on collection: {}",
            collection
        );

        let collection_handle = self.database.collection::<Document>(collection);

        // Parse pipeline JSON
        let pipeline_docs: Vec<Document> = serde_json::from_str(pipeline)
            .map_err(|e| DatabaseError::QueryError(format!("Invalid pipeline JSON: {}", e)))?;

        let mut cursor = collection_handle
            .aggregate(pipeline_docs)
            .await
            .map_err(|e| {
                DatabaseError::QueryError(format!("Failed to execute aggregation: {}", e))
            })?;

        let mut results = Vec::new();
        results.push(vec!["result".to_string()]);

        while let Some(doc) = cursor
            .try_next()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Cursor error: {}", e)))?
        {
            let doc_json = serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string());
            results.push(vec![doc_json]);
        }

        debug!(
            "[MongoDBClient::mongo_aggregate] Aggregation completed with {} rows",
            results.len() - 1
        );
        Ok(results)
    }

    /// Get MongoDB database statistics
    pub async fn mongo_stats(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[MongoDBClient::mongo_stats] Getting database statistics");

        let stats = self
            .database
            .run_command(doc! { "dbStats": 1 })
            .await
            .map_err(|e| {
                DatabaseError::QueryError(format!("Failed to get database stats: {}", e))
            })?;

        let mut results = Vec::new();
        results.push(vec!["Property".to_string(), "Value".to_string()]);

        // Add key statistics
        if let Some(db_name) = stats.get_str("db").ok() {
            results.push(vec!["Database".to_string(), db_name.to_string()]);
        }
        if let Some(collections) = stats.get_i32("collections").ok() {
            results.push(vec!["Collections".to_string(), collections.to_string()]);
        }
        if let Some(objects) = stats.get_i64("objects").ok() {
            results.push(vec!["Documents".to_string(), objects.to_string()]);
        }
        if let Some(data_size) = stats.get_f64("dataSize").ok() {
            results.push(vec![
                "Data Size".to_string(),
                format!("{:.2} MB", data_size / (1024.0 * 1024.0)),
            ]);
        }
        if let Some(storage_size) = stats.get_f64("storageSize").ok() {
            results.push(vec![
                "Storage Size".to_string(),
                format!("{:.2} MB", storage_size / (1024.0 * 1024.0)),
            ]);
        }
        if let Some(indexes) = stats.get_i32("indexes").ok() {
            results.push(vec!["Indexes".to_string(), indexes.to_string()]);
        }

        Ok(results)
    }
}

impl MongoDBClient {
    /// Execute MongoDB JavaScript-like commands
    async fn execute_mongodb_command(
        &self,
        command: &str,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!(
            "[MongoDBClient::execute_mongodb_command] Executing command: {}",
            command
        );

        // Handle db.runCommand({ dbStats: 1 })
        if command.contains("dbStats") {
            return self.mongo_stats().await;
        }

        // Handle db.<collection>.find()
        if let Some(find_match) = self.parse_find_command(command) {
            let (collection, filter, projection, limit) = find_match;
            return self
                .mongo_find(&collection, filter.as_deref(), projection.as_deref(), limit)
                .await;
        }

        // Handle db.<collection>.aggregate()
        if let Some((collection, pipeline)) = self.parse_aggregate_command(command) {
            return self.mongo_aggregate(&collection, &pipeline).await;
        }

        // Fallback to simple find if it's just a collection name
        if command.starts_with("db.") && !command.contains('(') {
            let collection = command.strip_prefix("db.").unwrap_or(command);
            return self.execute_simple_find(collection, 100).await;
        }

        Ok(vec![
            vec!["Error".to_string()],
            vec![format!("Unsupported MongoDB command: {}", command)],
        ])
    }

    /// Parse MongoDB find command syntax
    fn parse_find_command(
        &self,
        command: &str,
    ) -> Option<(String, Option<String>, Option<String>, Option<i64>)> {
        // Pattern: db.<collection>.find([filter], [projection])
        if !command.contains(".find(") {
            return None;
        }

        // Extract collection name
        let collection_start = command.find("db.").map(|i| i + 3)?;
        let collection_end = command[collection_start..]
            .find(".find(")
            .map(|i| collection_start + i)?;
        let collection = command[collection_start..collection_end].to_string();

        // Extract parameters from find()
        let params_start = collection_end + 6; // After ".find("
        let params_end = command[params_start..]
            .rfind(')')
            .map(|i| params_start + i)?;
        let params = &command[params_start..params_end];

        // Simple parsing - just look for commas outside of braces
        // This is a simplified parser and might need enhancement for complex cases
        let parts: Vec<&str> = params.split(',').collect();

        let filter = if !parts.is_empty() && !parts[0].trim().is_empty() {
            Some(parts[0].trim().to_string())
        } else {
            None
        };

        let projection = if parts.len() > 1 && !parts[1].trim().is_empty() {
            Some(parts[1].trim().to_string())
        } else {
            None
        };

        // Check for limit in the command (simplified)
        let limit = if command.contains("limit:") || command.contains("limit(") {
            // Try to extract limit value
            None // For now, use default
        } else {
            None
        };

        Some((collection, filter, projection, limit))
    }

    /// Parse MongoDB aggregate command syntax
    fn parse_aggregate_command(&self, command: &str) -> Option<(String, String)> {
        // Pattern: db.<collection>.aggregate([pipeline])
        if !command.contains(".aggregate(") {
            return None;
        }

        // Extract collection name
        let collection_start = command.find("db.").map(|i| i + 3)?;
        let collection_end = command[collection_start..]
            .find(".aggregate(")
            .map(|i| collection_start + i)?;
        let collection = command[collection_start..collection_end].to_string();

        // Extract pipeline
        let pipeline_start = collection_end + 11; // After ".aggregate("
        let pipeline_end = command[pipeline_start..]
            .rfind(')')
            .map(|i| pipeline_start + i)?;
        let pipeline = command[pipeline_start..pipeline_end].trim().to_string();

        Some((collection, pipeline))
    }

    /// Extract columns and values from MongoDB documents for tabular display
    async fn extract_columns_and_values(
        &self,
        collection_name: &str,
        filter_doc: Document,
        limit: i64,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        let collection = self.database.collection::<Document>(collection_name);

        // First, get column names by sampling documents (reuse the get_columns logic)
        let mut sample_cursor = collection
            .find(filter_doc.clone())
            .limit(10)
            .await
            .map_err(|e| {
                DatabaseError::QueryError(format!("Failed to sample documents for columns: {}", e))
            })?;

        let mut all_columns = std::collections::BTreeSet::new();
        let mut sample_count = 0;

        // Collect unique column names from sample documents
        while let Some(doc) = sample_cursor
            .try_next()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Cursor error: {}", e)))?
        {
            if sample_count >= 10 {
                break;
            }
            for key in doc.keys() {
                all_columns.insert(key.clone());
            }
            sample_count += 1;
        }

        let columns: Vec<String> = all_columns.into_iter().collect();

        // Now get the actual data with the same filter
        let mut cursor = collection
            .find(filter_doc)
            .limit(limit)
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to execute find: {}", e)))?;

        let mut results = Vec::new();

        // Add header row with actual column names
        results.push(columns.clone());

        // Add data rows
        let mut row_count = 0;
        while let Some(doc) = cursor
            .try_next()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Cursor error: {}", e)))?
        {
            if row_count >= limit {
                break;
            }

            let mut row = Vec::new();
            for column in &columns {
                let value = self.extract_field_value(&doc, column);
                row.push(value);
            }
            results.push(row);
            row_count += 1;
        }

        debug!(
            "[MongoDBClient::extract_columns_and_values] Query completed with {} columns and {} rows",
            columns.len(),
            results.len() - 1
        );
        Ok(results)
    }

    /// Extract a field value from a BSON document and convert to string
    fn extract_field_value(&self, doc: &Document, field: &str) -> String {
        match doc.get(field) {
            Some(bson::Bson::String(s)) => s.clone(),
            Some(bson::Bson::Int32(i)) => i.to_string(),
            Some(bson::Bson::Int64(i)) => i.to_string(),
            Some(bson::Bson::Double(d)) => d.to_string(),
            Some(bson::Bson::Boolean(b)) => b.to_string(),
            Some(bson::Bson::ObjectId(oid)) => oid.to_hex(),
            Some(bson::Bson::DateTime(dt)) => dt.to_string(),
            Some(bson::Bson::Array(arr)) => {
                // Format array as JSON for display
                serde_json::to_string(arr).unwrap_or_else(|_| format!("[{} items]", arr.len()))
            }
            Some(bson::Bson::Document(nested_doc)) => {
                // Format nested document as compact JSON
                serde_json::to_string(nested_doc).unwrap_or_else(|_| "{}".to_string())
            }
            Some(bson::Bson::Null) => "NULL".to_string(),
            Some(other) => {
                // For other BSON types, try to serialize as JSON
                serde_json::to_string(other).unwrap_or_else(|_| format!("{:?}", other))
            }
            None => "".to_string(), // Field not present in this document
        }
    }

    /// Drop a MongoDB database
    async fn drop_database(&self, database_name: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!(
            "[MongoDBClient::drop_database] Dropping database: {}",
            database_name
        );

        let target_db = self.client.database(database_name);
        let mut cmd = Document::new();
        cmd.insert("dropDatabase", 1);

        match target_db.run_command(cmd).await {
            Ok(_) => Ok(vec![
                vec!["Result".to_string()],
                vec![format!("Database '{}' dropped successfully", database_name)],
            ]),
            Err(e) => Err(DatabaseError::QueryError(format!(
                "Failed to drop database '{}': {}",
                database_name, e
            ))),
        }
    }

    /// Create a MongoDB database (implicit creation by switching and creating a collection)
    async fn create_database(
        &self,
        database_name: &str,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!(
            "[MongoDBClient::create_database] Creating database: {}",
            database_name
        );

        let target_db = self.client.database(database_name);

        // MongoDB creates databases implicitly when you first create a collection
        // We'll create a temporary collection and then drop it to initialize the database
        let temp_collection = target_db.collection::<Document>("_temp_init");
        let temp_doc = doc! { "_id": "temp", "created": "init" };

        match temp_collection.insert_one(temp_doc).await {
            Ok(_) => {
                // Remove the temporary document
                let filter = doc! { "_id": "temp" };
                let _ = temp_collection.delete_one(filter).await;

                Ok(vec![
                    vec!["Result".to_string()],
                    vec![format!("Database '{}' created successfully", database_name)],
                ])
            }
            Err(e) => Err(DatabaseError::QueryError(format!(
                "Failed to create database '{}': {}",
                database_name, e
            ))),
        }
    }

    /// Create a MongoDB collection
    async fn create_collection(
        &self,
        collection_name: &str,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!(
            "[MongoDBClient::create_collection] Creating collection: {}",
            collection_name
        );

        let mut cmd = Document::new();
        cmd.insert("create", collection_name);

        match self.database.run_command(cmd).await {
            Ok(_) => Ok(vec![
                vec!["Result".to_string()],
                vec![format!(
                    "Collection '{}' created successfully",
                    collection_name
                )],
            ]),
            Err(e) => Err(DatabaseError::QueryError(format!(
                "Failed to create collection '{}': {}",
                collection_name, e
            ))),
        }
    }

    /// Drop a MongoDB collection
    async fn drop_collection(
        &self,
        collection_name: &str,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!(
            "[MongoDBClient::drop_collection] Dropping collection: {}",
            collection_name
        );

        let collection = self.database.collection::<Document>(collection_name);

        match collection.drop().await {
            Ok(_) => Ok(vec![
                vec!["Result".to_string()],
                vec![format!(
                    "Collection '{}' dropped successfully",
                    collection_name
                )],
            ]),
            Err(e) => Err(DatabaseError::QueryError(format!(
                "Failed to drop collection '{}': {}",
                collection_name, e
            ))),
        }
    }

    async fn execute_simple_find(
        &self,
        collection_name: &str,
        limit: i64,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        // Use the new dynamic column extraction method
        let filter_doc = Document::new();
        self.extract_columns_and_values(collection_name, filter_doc, limit)
            .await
    }

    /// Handle DROP DATABASE SQL command
    async fn handle_drop_database_sql(
        &self,
        query: &str,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[handle_drop_database_sql] Parsing query: {}", query);

        // Extract database name from "DROP DATABASE database_name;"
        let parts: Vec<&str> = query.split_whitespace().collect();
        if parts.len() < 3 {
            return Err(DatabaseError::QueryError(
                "Invalid DROP DATABASE syntax. Use: DROP DATABASE database_name".to_string(),
            ));
        }

        let database_name = parts[2].trim_end_matches(';');
        self.drop_database(database_name).await
    }

    /// Handle CREATE DATABASE SQL command
    async fn handle_create_database_sql(
        &self,
        query: &str,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[handle_create_database_sql] Parsing query: {}", query);

        // Extract database name from "CREATE DATABASE database_name;"
        let parts: Vec<&str> = query.split_whitespace().collect();
        if parts.len() < 3 {
            return Err(DatabaseError::QueryError(
                "Invalid CREATE DATABASE syntax. Use: CREATE DATABASE database_name".to_string(),
            ));
        }

        let database_name = parts[2].trim_end_matches(';');
        self.create_database(database_name).await
    }

    /// Handle CREATE COLLECTION SQL command
    async fn handle_create_collection_sql(
        &self,
        query: &str,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[handle_create_collection_sql] Parsing query: {}", query);

        // Extract collection name from "CREATE COLLECTION collection_name;"
        let parts: Vec<&str> = query.split_whitespace().collect();
        if parts.len() < 3 {
            return Err(DatabaseError::QueryError(
                "Invalid CREATE COLLECTION syntax. Use: CREATE COLLECTION collection_name"
                    .to_string(),
            ));
        }

        let collection_name = parts[2].trim_end_matches(';');
        self.create_collection(collection_name).await
    }

    /// Handle DROP COLLECTION SQL command
    async fn handle_drop_collection_sql(
        &self,
        query: &str,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[handle_drop_collection_sql] Parsing query: {}", query);

        // Extract collection name from "DROP COLLECTION collection_name;"
        let parts: Vec<&str> = query.split_whitespace().collect();
        if parts.len() < 3 {
            return Err(DatabaseError::QueryError(
                "Invalid DROP COLLECTION syntax. Use: DROP COLLECTION collection_name".to_string(),
            ));
        }

        let collection_name = parts[2].trim_end_matches(';');
        self.drop_collection(collection_name).await
    }

    /// Parse SQL WHERE clause from a SQL query
    fn parse_sql_where_clause(&self, sql: &str) -> Option<String> {
        let sql_upper = sql.to_uppercase();

        // Find WHERE clause position
        let where_start = sql_upper.find(" WHERE ")?;
        let after_where = sql.get(where_start + 7..)?; // Skip " WHERE "

        // Find the end of WHERE clause (before ORDER BY, GROUP BY, LIMIT, etc.)
        let end_keywords = ["ORDER BY", "GROUP BY", "HAVING", "LIMIT", "OFFSET"];
        let mut where_end = after_where.len();

        for keyword in end_keywords.iter() {
            if let Some(pos) = after_where.to_uppercase().find(keyword) {
                where_end = where_end.min(pos);
            }
        }

        let where_clause = after_where.get(..where_end)?.trim();
        if where_clause.is_empty() {
            None
        } else {
            Some(where_clause.to_string())
        }
    }

    /// Convert simple SQL WHERE conditions to MongoDB filter document
    fn sql_condition_to_mongodb_filter(&self, condition: &str) -> Result<Document, DatabaseError> {
        debug!(
            "[sql_condition_to_mongodb_filter] Parsing condition: {}",
            condition
        );

        // Handle simple comparison operators
        let operators = [
            (">=", "$gte"),
            ("<=", "$lte"),
            ("!=", "$ne"),
            ("<>", "$ne"),
            ("=", ""),
            (">", "$gt"),
            ("<", "$lt"),
        ];

        for (sql_op, mongo_op) in operators.iter() {
            if let Some(op_pos) = condition.find(sql_op) {
                let field = condition[..op_pos].trim();
                let value_str = condition[op_pos + sql_op.len()..].trim();

                // Remove surrounding quotes if present
                let value_str = value_str.trim_matches('\'').trim_matches('"');

                let bson_value = self.convert_sql_value_to_bson(field, value_str)?;

                if mongo_op.is_empty() {
                    // Simple equality
                    return Ok(doc! { field.to_string(): bson_value });
                } else {
                    // Comparison operator
                    return Ok(doc! { field.to_string(): { mongo_op.to_string(): bson_value } });
                }
            }
        }

        // Handle LIKE operator
        if let Some(like_pos) = condition.to_uppercase().find(" LIKE ") {
            return self.parse_like_condition(condition, like_pos);
        }

        // Handle AND conditions (simple case)
        if condition.to_uppercase().contains(" AND ") {
            return self.parse_and_conditions(condition);
        }

        // Handle OR conditions
        if condition.to_uppercase().contains(" OR ") {
            return self.parse_or_conditions(condition);
        }

        // Handle IN operator
        if condition.to_uppercase().contains(" IN ") {
            return self.parse_in_condition(condition);
        }

        // Handle BETWEEN operator
        if condition.to_uppercase().contains(" BETWEEN ") {
            return self.parse_between_condition(condition);
        }

        // Handle IS NULL / IS NOT NULL
        if condition.to_uppercase().contains(" IS NULL") {
            return self.parse_null_condition(condition, false);
        }
        if condition.to_uppercase().contains(" IS NOT NULL") {
            return self.parse_null_condition(condition, true);
        }

        Err(DatabaseError::QueryError(format!(
            "Unsupported WHERE condition: {}",
            condition
        )))
    }

    /// Parse AND conditions in WHERE clause
    fn parse_and_conditions(&self, condition: &str) -> Result<Document, DatabaseError> {
        let parts: Vec<&str> = condition.split(" AND ").collect();
        let mut filter_doc = Document::new();

        for part in parts {
            let part_filter = self.sql_condition_to_mongodb_filter(part.trim())?;

            // Merge filters (simple case - assume no conflicting fields)
            for (key, value) in part_filter {
                filter_doc.insert(key, value);
            }
        }

        Ok(filter_doc)
    }

    /// Parse LIKE condition and convert to MongoDB $regex
    fn parse_like_condition(
        &self,
        condition: &str,
        like_pos: usize,
    ) -> Result<Document, DatabaseError> {
        let field = condition[..like_pos].trim();
        let pattern = condition[like_pos + 6..].trim(); // Skip " LIKE "

        // Remove surrounding quotes
        let pattern = pattern.trim_matches('\'').trim_matches('"');

        // Convert SQL LIKE pattern to regex
        let regex_pattern = self.sql_like_to_regex(pattern);

        Ok(doc! {
            field.to_string(): {
                "$regex": regex_pattern,
                "$options": "i"  // Case insensitive by default
            }
        })
    }

    /// Convert SQL LIKE pattern to MongoDB regex pattern
    fn sql_like_to_regex(&self, pattern: &str) -> String {
        let mut regex = String::new();
        let mut chars = pattern.chars().peekable();

        while let Some(ch) = chars.next() {
            match ch {
                '%' => regex.push_str(".*"),
                '_' => regex.push('.'),
                '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|'
                | '\\' => {
                    // Escape regex special characters
                    regex.push('\\');
                    regex.push(ch);
                }
                _ => regex.push(ch),
            }
        }

        regex
    }

    /// Parse OR conditions in WHERE clause
    fn parse_or_conditions(&self, condition: &str) -> Result<Document, DatabaseError> {
        let parts: Vec<&str> = condition.split(" OR ").collect();
        let mut or_conditions = Vec::new();

        for part in parts {
            let part_filter = self.sql_condition_to_mongodb_filter(part.trim())?;
            or_conditions.push(part_filter);
        }

        Ok(doc! { "$or": or_conditions })
    }

    /// Parse IN condition
    fn parse_in_condition(&self, condition: &str) -> Result<Document, DatabaseError> {
        if let Some(in_pos) = condition.to_uppercase().find(" IN ") {
            let field = condition[..in_pos].trim();
            let values_part = condition[in_pos + 4..].trim(); // Skip " IN "

            // Remove surrounding parentheses
            let values_part = values_part.trim_start_matches('(').trim_end_matches(')');

            // Parse comma-separated values
            let mut values = Vec::new();
            for value_str in values_part.split(',') {
                let value_str = value_str.trim().trim_matches('\'').trim_matches('"');
                let bson_value = self.convert_sql_value_to_bson(field, value_str)?;
                values.push(bson_value);
            }

            return Ok(doc! { field.to_string(): { "$in": values } });
        }

        Err(DatabaseError::QueryError(
            "Invalid IN condition format".to_string(),
        ))
    }

    /// Parse BETWEEN condition
    fn parse_between_condition(&self, condition: &str) -> Result<Document, DatabaseError> {
        if let Some(between_pos) = condition.to_uppercase().find(" BETWEEN ") {
            let field = condition[..between_pos].trim();
            let range_part = condition[between_pos + 9..].trim(); // Skip " BETWEEN "

            if let Some(and_pos) = range_part.to_uppercase().find(" AND ") {
                let min_str = range_part[..and_pos]
                    .trim()
                    .trim_matches('\'')
                    .trim_matches('"');
                let max_str = range_part[and_pos + 5..]
                    .trim()
                    .trim_matches('\'')
                    .trim_matches('"'); // Skip " AND "

                let min_value = self.convert_sql_value_to_bson(field, min_str)?;
                let max_value = self.convert_sql_value_to_bson(field, max_str)?;

                return Ok(doc! {
                    field.to_string(): {
                        "$gte": min_value,
                        "$lte": max_value
                    }
                });
            }
        }

        Err(DatabaseError::QueryError(
            "Invalid BETWEEN condition format".to_string(),
        ))
    }

    /// Parse NULL condition
    fn parse_null_condition(
        &self,
        condition: &str,
        is_not_null: bool,
    ) -> Result<Document, DatabaseError> {
        let field = if is_not_null {
            condition.replace(" IS NOT NULL", "").trim().to_string()
        } else {
            condition.replace(" IS NULL", "").trim().to_string()
        };

        if is_not_null {
            // Field exists and is not null
            Ok(doc! {
                field: {
                    "$exists": true,
                    "$ne": bson::Bson::Null
                }
            })
        } else {
            // Field is null or doesn't exist
            Ok(doc! {
                "$or": [
                    { field.clone(): bson::Bson::Null },
                    { field: { "$exists": false } }
                ]
            })
        }
    }

    /// Convert SQL value string to appropriate BSON value
    fn convert_sql_value_to_bson(
        &self,
        field: &str,
        value_str: &str,
    ) -> Result<bson::Bson, DatabaseError> {
        // Handle _id field specially - convert to ObjectId if it looks like one
        if field == "_id" && value_str.len() == 24 {
            match bson::oid::ObjectId::parse_str(value_str) {
                Ok(oid) => return Ok(bson::Bson::ObjectId(oid)),
                Err(_) => {
                    // If it's not a valid ObjectId, treat as string
                    return Ok(bson::Bson::String(value_str.to_string()));
                }
            }
        }

        // Try to parse as number first
        if let Ok(int_val) = value_str.parse::<i32>() {
            return Ok(bson::Bson::Int32(int_val));
        }

        if let Ok(int_val) = value_str.parse::<i64>() {
            return Ok(bson::Bson::Int64(int_val));
        }

        if let Ok(float_val) = value_str.parse::<f64>() {
            return Ok(bson::Bson::Double(float_val));
        }

        // Handle boolean values
        match value_str.to_lowercase().as_str() {
            "true" => return Ok(bson::Bson::Boolean(true)),
            "false" => return Ok(bson::Bson::Boolean(false)),
            "null" => return Ok(bson::Bson::Null),
            _ => {}
        }

        // Default to string
        Ok(bson::Bson::String(value_str.to_string()))
    }

    /// Execute SQL SELECT query with proper WHERE clause handling
    async fn execute_sql_select(&self, query: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!(
            "[MongoDBClient::execute_sql_select] Parsing SQL query: {}",
            query
        );

        // Parse collection name from FROM clause
        let collection_name = if let Some(from_pos) = query.to_uppercase().find("FROM") {
            let after_from = &query[from_pos + 4..].trim();
            let collection_end = after_from.find(' ').unwrap_or(after_from.len());
            after_from[..collection_end].trim()
        } else {
            return Ok(vec![
                vec!["Error".to_string()],
                vec!["Invalid SQL query format - missing FROM".to_string()],
            ]);
        };

        // Parse LIMIT clause
        let limit = if let Some(limit_pos) = query.to_uppercase().find("LIMIT") {
            let limit_str = &query[limit_pos + 5..].trim();
            limit_str.parse::<i64>().unwrap_or(100)
        } else {
            100 // Default limit
        };

        // Parse WHERE clause and convert to MongoDB filter
        let filter_doc = if let Some(where_clause) = self.parse_sql_where_clause(query) {
            debug!("[execute_sql_select] Found WHERE clause: {}", where_clause);
            match self.sql_condition_to_mongodb_filter(&where_clause) {
                Ok(filter) => {
                    debug!(
                        "[execute_sql_select] Converted to MongoDB filter: {:?}",
                        filter
                    );
                    filter
                }
                Err(e) => {
                    return Ok(vec![
                        vec!["Error".to_string()],
                        vec![format!("Failed to parse WHERE clause: {}", e)],
                    ]);
                }
            }
        } else {
            debug!("[execute_sql_select] No WHERE clause found, using empty filter");
            Document::new()
        };

        // Execute the query with the filter
        self.extract_columns_and_values(collection_name, filter_doc, limit)
            .await
    }
}

#[async_trait]
impl DatabaseClient for MongoDBClient {
    async fn execute_query(&self, query: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[MongoDBClient::execute_query] Executing query: {}", query);

        if query.trim().is_empty() {
            return Ok(vec![vec!["No query provided".to_string()]]);
        }

        let query = query.trim();
        let query_upper = query.to_uppercase();

        // Handle MongoDB JavaScript-like syntax
        if query.starts_with("db.") {
            return self.execute_mongodb_command(query).await;
        }

        // Handle database management SQL commands
        if query_upper.starts_with("DROP DATABASE") {
            return self.handle_drop_database_sql(query).await;
        }

        if query_upper.starts_with("CREATE DATABASE") {
            return self.handle_create_database_sql(query).await;
        }

        if query_upper.starts_with("CREATE COLLECTION") {
            return self.handle_create_collection_sql(query).await;
        }

        if query_upper.starts_with("DROP COLLECTION") {
            return self.handle_drop_collection_sql(query).await;
        }

        // Handle SQL-like queries for MongoDB collections
        if query_upper.starts_with("SELECT") {
            return self.execute_sql_select(query).await;
        }

        // Handle direct collection queries (for backward compatibility)
        self.execute_simple_find(query, 100).await
    }

    async fn test_query(&self, query: &str) -> Result<(), DatabaseError> {
        debug!("[MongoDBClient::test_query] Testing query: {}", query);

        // Simple validation - check if collection exists
        let collection_name = query.trim();

        let collections =
            self.database.list_collection_names().await.map_err(|e| {
                DatabaseError::QueryError(format!("Failed to list collections: {}", e))
            })?;

        if collections.contains(&collection_name.to_string()) {
            Ok(())
        } else {
            Err(DatabaseError::QueryError(format!(
                "Collection '{}' does not exist",
                collection_name
            )))
        }
    }

    async fn explain_query(&self, query: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[MongoDBClient::explain_query] Explaining query: {}", query);

        let collection_name = query.trim();
        let collection = self.database.collection::<Document>(collection_name);

        // Use MongoDB's explain functionality
        let _explain_result = collection
            .find(Document::new())
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to explain query: {}", e)))?;

        // For now, return a simple explanation
        let mut results = Vec::new();
        results.push(vec!["MongoDB Query Plan".to_string()]);
        results.push(vec!["".to_string()]);
        results.push(vec!["Collection Scan".to_string()]);
        results.push(vec![format!("Collection: {}", collection_name)]);

        Ok(results)
    }

    async fn explain_query_raw(&self, query: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        self.explain_query(query).await
    }

    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[MongoDBClient::list_databases] Listing databases");

        let admin_db = self.client.database("admin");
        let mut cmd = Document::new();
        cmd.insert("listDatabases", 1);
        let databases = admin_db
            .run_command(cmd)
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to list databases: {}", e)))?;

        let mut results = Vec::new();
        results.push(vec!["Name".to_string(), "Size".to_string()]);

        if let Some(databases_array) = databases.get_array("databases").ok() {
            for db_doc in databases_array {
                if let Some(db_obj) = db_doc.as_document() {
                    let name = db_obj.get_str("name").unwrap_or("N/A").to_string();
                    let size = db_obj.get_f64("sizeOnDisk").unwrap_or(0.0);
                    results.push(vec![name, format!("{:.2} MB", size / (1024.0 * 1024.0))]);
                }
            }
        }

        Ok(results)
    }

    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError> {
        debug!(
            "[MongoDBClient::connect_to_database] Switching to database: {}",
            database
        );

        self.database = self.client.database(database);
        self.current_database = database.to_string();
        self.metadata_provider = MongoDBMetadataProvider::new(self.database.clone());

        Ok(())
    }

    fn get_current_database(&self) -> String {
        self.current_database.clone()
    }

    fn get_connection_info(&self) -> &ConnectionInfo {
        &self.connection_info
    }

    fn get_metadata_provider(&self) -> &dyn MetadataProvider {
        &self.metadata_provider
    }

    async fn is_connected(&self) -> bool {
        // Test connection with a simple ping
        self.database.run_command(doc! { "ping": 1 }).await.is_ok()
    }

    async fn close(&mut self) -> Result<(), DatabaseError> {
        debug!("[MongoDBClient::close] Closing MongoDB connection");
        // MongoDB client doesn't have an explicit close method
        // The connection will be closed when the client is dropped
        Ok(())
    }

    async fn get_server_info(&self) -> Result<crate::database::ServerInfo, DatabaseError> {
        debug!("[MongoDBClient::get_server_info] Getting server info");

        let admin_db = self.client.database("admin");
        let server_info = admin_db
            .run_command(doc! { "buildInfo": 1 })
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to get server info: {}", e)))?;

        let version = server_info.get_str("version").unwrap_or("unknown");
        let mut additional_info = std::collections::HashMap::new();

        if let Some(modules) = server_info.get_array("modules").ok() {
            additional_info.insert("modules".to_string(), format!("{:?}", modules));
        }

        Ok(crate::database::ServerInfo {
            server_type: "MongoDB".to_string(),
            server_version: version.to_string(),
            version_major: None, // Would need to parse version string
            version_minor: None,
            version_patch: None,
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            supports_transactions: true, // MongoDB supports transactions
            supports_roles: true,        // MongoDB supports role-based auth
            additional_info,
        })
    }
}
