//! Schema extraction for AI SQL generation
//!
//! Extracts database schema metadata to provide context for AI SQL generation.
//! Leverages existing MetadataProvider trait for multi-database support.

use crate::ai_sql::error::{AiError, AiResult};
use crate::database::DatabaseType;
use crate::db::Database;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::{debug, info};

/// Column metadata for AI context
#[derive(Debug, Clone)]
pub struct ColumnMetadata {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub is_primary_key: bool,
    pub is_foreign_key: bool,
    pub references: Option<(String, String)>, // (table, column)
    pub default_value: Option<String>,
}

/// Table metadata with columns and indexes
#[derive(Debug, Clone)]
pub struct TableMetadata {
    pub name: String,
    pub schema: Option<String>,
    pub columns: Vec<ColumnMetadata>,
    pub indexes: Vec<String>,
    pub row_count_estimate: Option<u64>,
    pub comment: Option<String>,
}

/// Foreign key relationship
#[derive(Debug, Clone)]
pub struct Relationship {
    pub from_table: String,
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
    pub constraint_name: Option<String>,
}

/// Complete schema context for AI SQL generation
#[derive(Debug, Clone)]
pub struct SchemaContext {
    pub database_type: DatabaseType,
    pub current_database: String,
    pub current_schema: Option<String>,
    pub tables: Vec<TableMetadata>,
    pub relationships: Vec<Relationship>,
    pub common_patterns: Vec<QueryPattern>,
}

/// Common query patterns discovered in the database
#[derive(Debug, Clone)]
pub struct QueryPattern {
    pub pattern_type: String, // "time_series", "user_activity", "e-commerce", etc.
    pub tables_involved: Vec<String>,
    pub description: String,
}

/// Schema extractor with caching
pub struct SchemaExtractor {
    cache: HashMap<String, SchemaContext>,
    cache_ttl_seconds: u64,
    last_cache_time: HashMap<String, std::time::Instant>,
}

impl SchemaExtractor {
    /// Create a new schema extractor
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            cache_ttl_seconds: 300, // 5 minutes default
            last_cache_time: HashMap::new(),
        }
    }

    /// Extract schema context from database
    pub async fn extract_context(
        &mut self,
        database: &Arc<Mutex<Database>>,
        query_hint: Option<&str>,
    ) -> AiResult<SchemaContext> {
        let mut db = database.lock().unwrap();
        let db_name = db.get_current_db();

        // Get database type from connection info
        let db_type = db
            .get_connection_info()
            .map(|info| info.database_type.clone())
            .ok_or_else(|| AiError::SchemaError("No connection info available".to_string()))?;

        // Check cache
        if let Some(cached) = self.get_cached_context(&db_name) {
            debug!("Using cached schema context for database: {}", db_name);
            return Ok(cached);
        }

        info!("Extracting schema context for database: {}", db_name);

        // Get current schema (not all databases have this concept)
        let current_schema = None; // TODO: Extract from connection info if needed

        // Extract tables (with optional filtering based on query hint)
        let table_names = if let Some(hint) = query_hint {
            self.extract_relevant_table_names(&mut db, hint).await?
        } else {
            self.extract_all_table_names(&mut db).await?
        };

        debug!("Found {} tables to analyze", table_names.len());

        // Extract metadata for each table
        let mut tables = Vec::new();
        for table_name in table_names {
            match self.extract_table_metadata(&mut db, &table_name, current_schema.as_deref()).await {
                Ok(table_meta) => tables.push(table_meta),
                Err(e) => {
                    debug!("Failed to extract metadata for table {}: {}", table_name, e);
                    // Continue with other tables
                }
            }
        }

        // Extract relationships
        let relationships = self.extract_relationships(&mut db, &tables).await?;

        // Detect common patterns
        let common_patterns = self.detect_patterns(&tables, &relationships);

        let context = SchemaContext {
            database_type: db_type,
            current_database: db_name.clone(),
            current_schema,
            tables,
            relationships,
            common_patterns,
        };

        // Cache the context
        self.cache_context(db_name, context.clone());

        Ok(context)
    }

    /// Extract all table names from database
    async fn extract_all_table_names(&self, db: &mut Database) -> AiResult<Vec<String>> {
        db.get_tables_and_views(None)
            .await
            .map_err(|e| AiError::SchemaError(format!("Failed to get tables: {}", e)))
    }

    /// Extract relevant table names based on query hint
    async fn extract_relevant_table_names(
        &self,
        db: &mut Database,
        hint: &str,
    ) -> AiResult<Vec<String>> {
        let all_tables = self.extract_all_table_names(db).await?;

        // Simple relevance scoring based on keyword matching
        let hint_lower = hint.to_lowercase();
        let keywords: Vec<&str> = hint_lower.split_whitespace().collect();

        let mut scored_tables: Vec<(String, usize)> = all_tables
            .into_iter()
            .map(|table| {
                let table_lower = table.to_lowercase();
                let score = keywords
                    .iter()
                    .filter(|&&keyword| table_lower.contains(keyword))
                    .count();
                (table, score)
            })
            .collect();

        // Sort by score descending
        scored_tables.sort_by(|a, b| b.1.cmp(&a.1));

        // Return tables with any relevance, or all if none match
        let relevant_tables: Vec<String> = scored_tables
            .into_iter()
            .filter(|(_, score)| *score > 0)
            .map(|(table, _)| table)
            .collect();

        if relevant_tables.is_empty() {
            // If no matches, return all tables (up to a limit)
            self.extract_all_table_names(db).await
        } else {
            Ok(relevant_tables)
        }
    }

    /// Extract metadata for a single table
    async fn extract_table_metadata(
        &self,
        db: &mut Database,
        table_name: &str,
        schema: Option<&str>,
    ) -> AiResult<TableMetadata> {
        // Get table details (indexes, foreign keys, etc.)
        let details = db
            .get_table_details(table_name)
            .await
            .map_err(|e| {
                AiError::SchemaError(format!(
                    "Failed to get table details for {}: {}",
                    table_name, e
                ))
            })?;

        // Convert column information
        let column_metadata: Vec<ColumnMetadata> = details
            .columns
            .iter()
            .map(|col| {
                ColumnMetadata {
                    name: col.name.clone(),
                    data_type: col.data_type.clone(),
                    nullable: col.nullable,
                    is_primary_key: false, // Will be determined from indexes
                    is_foreign_key: false, // Will be determined from foreign_keys
                    references: None,      // Will be populated from foreign_keys
                    default_value: col.default_value.clone(),
                }
            })
            .collect();

        // Convert indexes to string representations
        let index_strings: Vec<String> = details
            .indexes
            .iter()
            .map(|idx| {
                let mut parts = vec![idx.name.clone()];
                if idx.is_primary {
                    parts.push("PRIMARY KEY".to_string());
                } else if idx.is_unique {
                    parts.push("UNIQUE".to_string());
                }
                parts.push(idx.index_type.clone());
                parts.join(" ")
            })
            .collect();

        Ok(TableMetadata {
            name: table_name.to_string(),
            schema: schema.map(|s| s.to_string()),
            columns: column_metadata,
            indexes: index_strings,
            row_count_estimate: None,
            comment: None,
        })
    }

    /// Extract foreign key relationships
    async fn extract_relationships(
        &self,
        db: &mut Database,
        tables: &[TableMetadata],
    ) -> AiResult<Vec<Relationship>> {
        let relationships = Vec::new();

        for table in tables {
            // Get table details which include foreign keys
            match db.get_table_details(&table.name).await {
                Ok(details) => {
                    // Extract foreign key relationships from definitions
                    // ForeignKeyInfo only has name and definition, so we'd need to parse
                    // For now, we'll skip detailed relationship extraction
                    // TODO: Parse FK definitions to extract from/to columns
                    for _fk in &details.foreign_keys {
                        // Could parse fk.definition which contains:
                        // "FOREIGN KEY (column) REFERENCES other_table(other_column)"
                        // For now, we'll leave this as a TODO
                    }
                }
                Err(e) => {
                    debug!("Failed to get foreign keys for {}: {}", table.name, e);
                }
            }
        }

        Ok(relationships)
    }

    /// Detect common query patterns in the schema
    fn detect_patterns(
        &self,
        tables: &[TableMetadata],
        _relationships: &[Relationship],
    ) -> Vec<QueryPattern> {
        let mut patterns = Vec::new();

        // Detect time-series pattern (tables with timestamp columns)
        let time_series_tables: Vec<String> = tables
            .iter()
            .filter(|t| {
                t.columns.iter().any(|c| {
                    c.data_type.to_uppercase().contains("TIMESTAMP")
                        || c.data_type.to_uppercase().contains("DATETIME")
                        || c.name.to_lowercase().contains("created")
                        || c.name.to_lowercase().contains("updated")
                })
            })
            .map(|t| t.name.clone())
            .collect();

        if !time_series_tables.is_empty() {
            patterns.push(QueryPattern {
                pattern_type: "time_series".to_string(),
                tables_involved: time_series_tables,
                description: "Tables with temporal data suitable for time-based analysis".to_string(),
            });
        }

        // Detect user activity pattern
        let user_tables: Vec<String> = tables
            .iter()
            .filter(|t| {
                t.name.to_lowercase().contains("user")
                    || t.name.to_lowercase().contains("account")
            })
            .map(|t| t.name.clone())
            .collect();

        if !user_tables.is_empty() {
            patterns.push(QueryPattern {
                pattern_type: "user_activity".to_string(),
                tables_involved: user_tables,
                description: "User-related tables for activity analysis".to_string(),
            });
        }

        // Detect e-commerce pattern
        let has_orders = tables.iter().any(|t| t.name.to_lowercase().contains("order"));
        let has_products = tables
            .iter()
            .any(|t| t.name.to_lowercase().contains("product"));
        let has_customers = tables
            .iter()
            .any(|t| t.name.to_lowercase().contains("customer"));

        if has_orders && (has_products || has_customers) {
            let ecommerce_tables: Vec<String> = tables
                .iter()
                .filter(|t| {
                    let name_lower = t.name.to_lowercase();
                    name_lower.contains("order")
                        || name_lower.contains("product")
                        || name_lower.contains("customer")
                        || name_lower.contains("cart")
                })
                .map(|t| t.name.clone())
                .collect();

            patterns.push(QueryPattern {
                pattern_type: "e_commerce".to_string(),
                tables_involved: ecommerce_tables,
                description: "E-commerce related tables for sales analysis".to_string(),
            });
        }

        patterns
    }

    /// Get cached context if available and not expired
    fn get_cached_context(&self, db_name: &str) -> Option<SchemaContext> {
        if let Some(context) = self.cache.get(db_name) {
            if let Some(last_time) = self.last_cache_time.get(db_name) {
                let elapsed = last_time.elapsed().as_secs();
                if elapsed < self.cache_ttl_seconds {
                    return Some(context.clone());
                }
            }
        }
        None
    }

    /// Cache schema context
    fn cache_context(&mut self, db_name: String, context: SchemaContext) {
        self.cache.insert(db_name.clone(), context);
        self.last_cache_time
            .insert(db_name, std::time::Instant::now());
    }

    /// Clear the cache
    pub fn clear_cache(&mut self) {
        self.cache.clear();
        self.last_cache_time.clear();
    }
}

impl Default for SchemaExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_patterns() {
        let tables = vec![
            TableMetadata {
                name: "users".to_string(),
                schema: None,
                columns: vec![ColumnMetadata {
                    name: "created_at".to_string(),
                    data_type: "TIMESTAMP".to_string(),
                    nullable: false,
                    is_primary_key: false,
                    is_foreign_key: false,
                    references: None,
                    default_value: None,
                }],
                indexes: vec![],
                row_count_estimate: None,
                comment: None,
            },
            TableMetadata {
                name: "orders".to_string(),
                schema: None,
                columns: vec![],
                indexes: vec![],
                row_count_estimate: None,
                comment: None,
            },
        ];

        let extractor = SchemaExtractor::new();
        let patterns = extractor.detect_patterns(&tables, &[]);

        assert!(!patterns.is_empty());
        assert!(patterns
            .iter()
            .any(|p| p.pattern_type == "time_series"));
        assert!(patterns.iter().any(|p| p.pattern_type == "user_activity"));
    }
}
