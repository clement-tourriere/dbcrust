//! PostgreSQL implementation of the database abstraction layer
use crate::complex_display::{
    ArrayDisplayAdapter, ComplexDataDisplay, ComplexDataType, ComplexTypeDetector,
    GenericComplexTypeDetector,
};
use crate::database::{ConnectionInfo, DatabaseClient, DatabaseError, MetadataProvider};
use crate::db::TableDetails;
use crate::geojson_display::GeoJsonDisplayAdapter;
use crate::json_display::JsonDisplayAdapter;
use crate::performance_analyzer::PerformanceAnalyzer;
use async_trait::async_trait;
use serde_json;
use sqlx::postgres::{PgPool, PgPoolOptions, PgRow};
use sqlx::{Column, Row};
use tracing::debug;

/// Check if a type name is a built-in PostgreSQL type
fn is_builtin_postgresql_type(type_name: &str) -> bool {
    // Common PostgreSQL built-in types
    matches!(
        type_name.to_uppercase().as_str(),
        // Numeric types
        "SMALLINT" | "INT2" | "INTEGER" | "INT4" | "BIGINT" | "INT8" |
        "DECIMAL" | "NUMERIC" | "REAL" | "FLOAT4" | "DOUBLE PRECISION" | "FLOAT8" |
        "SMALLSERIAL" | "SERIAL" | "BIGSERIAL" | "SERIAL2" | "SERIAL4" | "SERIAL8" |
        "MONEY" | "OID" |

        // String types
        "CHARACTER VARYING" | "VARCHAR" | "CHARACTER" | "CHAR" | "BPCHAR" |
        "TEXT" | "NAME" |

        // Binary types
        "BYTEA" |

        // Date/time types
        "TIMESTAMP" | "TIMESTAMPTZ" | "TIMESTAMP WITH TIME ZONE" |
        "TIMESTAMP WITHOUT TIME ZONE" | "DATE" | "TIME" | "TIMETZ" |
        "TIME WITH TIME ZONE" | "TIME WITHOUT TIME ZONE" | "INTERVAL" |

        // Boolean type
        "BOOLEAN" | "BOOL" |

        // JSON types
        "JSON" | "JSONB" |

        // Network types
        "INET" | "CIDR" | "MACADDR" | "MACADDR8" |

        // UUID type
        "UUID" |

        // Geometric types
        "POINT" | "LINE" | "LSEG" | "BOX" | "PATH" | "POLYGON" | "CIRCLE" |

        // Range types
        "INT4RANGE" | "INT8RANGE" | "NUMRANGE" | "TSRANGE" | "TSTZRANGE" | "DATERANGE" |

        // Bit string types
        "BIT" | "VARBIT" |

        // XML type
        "XML" |

        // Full-text search types
        "TSVECTOR" | "TSQUERY" |

        // Other common types
        "VOID" | "UNKNOWN" | "RECORD" |

        // Extension types that have special handling - NOT enums!
        // pgvector extension
        "VECTOR" | "HALFVEC" | "SPARSEVEC" |

        // PostGIS extension
        "GEOMETRY" | "GEOGRAPHY" | "BOX2D" | "BOX3D" |

        // PostgreSQL contrib extensions
        "HSTORE" | "LTREE" | "LQUERY" | "LTXTQUERY" | "CUBE" |

        // Other extensions commonly used
        "CITEXT" | "EARTHDISTANCE" | "ISN" | "SEG"
    ) || type_name.ends_with("[]") // Array types are also built-in
}

/// PostgreSQL metadata provider implementation
pub struct PostgreSQLMetadataProvider {
    pool: PgPool,
}

impl PostgreSQLMetadataProvider {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Get detailed column information including data types, nullability, and defaults
    async fn get_detailed_columns(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<Vec<crate::db::ColumnInfo>, DatabaseError> {
        let schema_name = schema.unwrap_or("public");

        let rows = sqlx::query(
            r#"
            SELECT
                a.attname as column_name,
                format_type(a.atttypid, a.atttypmod) as data_type,
                COALESCE(c.collname, '') as collation,
                NOT a.attnotnull as nullable,
                pg_get_expr(d.adbin, d.adrelid) as default_value
            FROM pg_attribute a
            INNER JOIN pg_class t ON a.attrelid = t.oid
            INNER JOIN pg_namespace n ON t.relnamespace = n.oid
            LEFT JOIN pg_attrdef d ON a.attrelid = d.adrelid AND a.attnum = d.adnum
            LEFT JOIN pg_collation c ON a.attcollation = c.oid AND a.attcollation <> 0
            WHERE n.nspname = $1
              AND t.relname = $2
              AND a.attnum > 0
              AND NOT a.attisdropped
            ORDER BY a.attnum
            "#,
        )
        .bind(schema_name)
        .bind(table)
        .fetch_all(&self.pool)
        .await?;

        let mut columns: Vec<crate::db::ColumnInfo> = rows
            .iter()
            .map(|row| crate::db::ColumnInfo {
                name: row.get::<String, _>("column_name"),
                data_type: row.get::<String, _>("data_type"),
                collation: row.get::<String, _>("collation"),
                nullable: row.get::<bool, _>("nullable"),
                default_value: row.get::<Option<String>, _>("default_value"),
                enum_values: None, // Will be populated below for enum types
            })
            .collect();

        // Collect all enum type names (types without modifiers like length)
        let enum_types: Vec<String> = columns
            .iter()
            .filter_map(|col| {
                let base_type = col
                    .data_type
                    .split('(') // Remove modifiers like character varying(200)
                    .next()
                    .unwrap_or(&col.data_type)
                    .trim();

                // Check if this looks like a custom type (not a built-in PostgreSQL type)
                // Built-in types typically have spaces or are well-known keywords
                if !is_builtin_postgresql_type(base_type) {
                    Some(base_type.to_string())
                } else {
                    None
                }
            })
            .collect::<std::collections::HashSet<_>>() // Deduplicate
            .into_iter()
            .collect();

        // Fetch enum values for all custom types found
        if !enum_types.is_empty() {
            debug!(
                "[PostgreSQLMetadataProvider::get_detailed_columns] Found potential enum types: {:?}",
                enum_types
            );

            match self
                .get_enum_values_for_types(&enum_types, schema_name)
                .await
            {
                Ok(enum_map) => {
                    // Update columns with enum values
                    for column in &mut columns {
                        let base_type = column
                            .data_type
                            .split('(')
                            .next()
                            .unwrap_or(&column.data_type)
                            .trim();

                        if let Some(enum_values) = enum_map.get(base_type) {
                            column.enum_values = Some(enum_values.clone());
                            debug!(
                                "[PostgreSQLMetadataProvider::get_detailed_columns] Set enum values for column '{}' type '{}': {:?}",
                                column.name, base_type, enum_values
                            );
                        }
                    }
                }
                Err(e) => {
                    debug!(
                        "[PostgreSQLMetadataProvider::get_detailed_columns] Failed to fetch enum values: {}",
                        e
                    );
                    // Continue without enum values rather than failing the entire operation
                }
            }
        }

        Ok(columns)
    }

    /// Get index information for a table
    async fn get_table_indexes(
        &self,
        table: &str,
        schema: &str,
    ) -> Result<Vec<crate::db::IndexInfo>, DatabaseError> {
        let rows = sqlx::query(
            r#"
            SELECT
                i.relname as index_name,
                CASE
                    WHEN ix.indisunique AND ix.indisprimary THEN 'PRIMARY KEY'
                    WHEN ix.indisunique THEN 'UNIQUE'
                    ELSE 'INDEX'
                END as index_type,
                ix.indisprimary as is_primary,
                ix.indisunique as is_unique,
                pg_get_expr(ix.indpred, ix.indrelid) as predicate,
                pg_get_indexdef(ix.indexrelid) as definition
            FROM pg_index ix
            INNER JOIN pg_class i ON i.oid = ix.indexrelid
            INNER JOIN pg_class t ON t.oid = ix.indrelid
            INNER JOIN pg_namespace n ON t.relnamespace = n.oid
            WHERE n.nspname = $1 AND t.relname = $2
            ORDER BY ix.indisprimary DESC, ix.indisunique DESC, i.relname
            "#,
        )
        .bind(schema)
        .bind(table)
        .fetch_all(&self.pool)
        .await?;

        let indexes: Vec<crate::db::IndexInfo> = rows
            .iter()
            .map(|row| crate::db::IndexInfo {
                name: row.get::<String, _>("index_name"),
                index_type: row.get::<String, _>("index_type"),
                is_primary: row.get::<bool, _>("is_primary"),
                is_unique: row.get::<bool, _>("is_unique"),
                predicate: row.get::<Option<String>, _>("predicate"),
                definition: row.get::<String, _>("definition"),
                constraint_def: None,
            })
            .collect();

        Ok(indexes)
    }

    /// Get foreign key constraints for a table
    async fn get_table_foreign_keys(
        &self,
        table: &str,
        schema: &str,
    ) -> Result<Vec<crate::db::ForeignKeyInfo>, DatabaseError> {
        let rows = sqlx::query(
            r#"
            SELECT
                c.conname as constraint_name,
                pg_get_constraintdef(c.oid) as definition
            FROM pg_constraint c
            INNER JOIN pg_class t ON c.conrelid = t.oid
            INNER JOIN pg_namespace n ON t.relnamespace = n.oid
            WHERE n.nspname = $1
              AND t.relname = $2
              AND c.contype = 'f'
            ORDER BY c.conname
            "#,
        )
        .bind(schema)
        .bind(table)
        .fetch_all(&self.pool)
        .await?;

        let foreign_keys: Vec<crate::db::ForeignKeyInfo> = rows
            .iter()
            .map(|row| crate::db::ForeignKeyInfo {
                name: row.get::<String, _>("constraint_name"),
                definition: row.get::<String, _>("definition"),
            })
            .collect();

        Ok(foreign_keys)
    }

    /// Get check constraints for a table
    async fn get_table_check_constraints(
        &self,
        table: &str,
        schema: &str,
    ) -> Result<Vec<crate::db::CheckConstraintInfo>, DatabaseError> {
        let rows = sqlx::query(
            r#"
            SELECT
                c.conname as constraint_name,
                pg_get_constraintdef(c.oid) as definition
            FROM pg_constraint c
            INNER JOIN pg_class t ON c.conrelid = t.oid
            INNER JOIN pg_namespace n ON t.relnamespace = n.oid
            WHERE n.nspname = $1
              AND t.relname = $2
              AND c.contype = 'c'
            ORDER BY c.conname
            "#,
        )
        .bind(schema)
        .bind(table)
        .fetch_all(&self.pool)
        .await?;

        let check_constraints: Vec<crate::db::CheckConstraintInfo> = rows
            .iter()
            .map(|row| crate::db::CheckConstraintInfo {
                name: row.get::<String, _>("constraint_name"),
                definition: row.get::<String, _>("definition"),
            })
            .collect();

        Ok(check_constraints)
    }

    /// Get enum values for all enum types used in the table
    async fn get_enum_values_for_types(
        &self,
        enum_types: &[String],
        schema: &str,
    ) -> Result<std::collections::HashMap<String, Vec<String>>, DatabaseError> {
        use std::collections::HashMap;

        if enum_types.is_empty() {
            return Ok(HashMap::new());
        }

        debug!(
            "[PostgreSQLMetadataProvider::get_enum_values_for_types] Fetching enum values for types: {:?} in schema: {}",
            enum_types, schema
        );

        // Build query to get enum values for all the specified types
        let type_list = enum_types
            .iter()
            .map(|t| format!("'{}'", t))
            .collect::<Vec<_>>()
            .join(",");

        let query = format!(
            r#"
            SELECT
                t.typname as enum_name,
                e.enumlabel as enum_value
            FROM pg_type t
            JOIN pg_enum e ON t.oid = e.enumtypid
            JOIN pg_namespace n ON t.typnamespace = n.oid
            WHERE n.nspname = $1
              AND t.typname IN ({})
            ORDER BY t.typname, e.enumsortorder
            "#,
            type_list
        );

        let rows = sqlx::query(&query)
            .bind(schema)
            .fetch_all(&self.pool)
            .await?;

        let mut enum_map = HashMap::new();
        for row in rows {
            let enum_name: String = row.get("enum_name");
            let enum_value: String = row.get("enum_value");

            enum_map
                .entry(enum_name)
                .or_insert_with(Vec::new)
                .push(enum_value);
        }

        debug!(
            "[PostgreSQLMetadataProvider::get_enum_values_for_types] Found enum values for {} types",
            enum_map.len()
        );

        Ok(enum_map)
    }

    /// Get tables that reference this table (reverse foreign keys)
    async fn get_table_referenced_by(
        &self,
        table: &str,
        schema: &str,
    ) -> Result<Vec<crate::db::ReferencedByInfo>, DatabaseError> {
        let rows = sqlx::query(
            r#"
            SELECT
                n.nspname as referencing_schema,
                t.relname as referencing_table,
                c.conname as constraint_name,
                pg_get_constraintdef(c.oid) as definition
            FROM pg_constraint c
            INNER JOIN pg_class t ON c.conrelid = t.oid
            INNER JOIN pg_namespace n ON t.relnamespace = n.oid
            INNER JOIN pg_class ref_t ON c.confrelid = ref_t.oid
            INNER JOIN pg_namespace ref_n ON ref_t.relnamespace = ref_n.oid
            WHERE ref_n.nspname = $1
              AND ref_t.relname = $2
              AND c.contype = 'f'
            ORDER BY n.nspname, t.relname, c.conname
            "#,
        )
        .bind(schema)
        .bind(table)
        .fetch_all(&self.pool)
        .await?;

        let referenced_by: Vec<crate::db::ReferencedByInfo> = rows
            .iter()
            .map(|row| crate::db::ReferencedByInfo {
                schema: row.get::<String, _>("referencing_schema"),
                table: row.get::<String, _>("referencing_table"),
                constraint_name: row.get::<String, _>("constraint_name"),
                definition: row.get::<String, _>("definition"),
            })
            .collect();

        Ok(referenced_by)
    }
}

#[async_trait]
impl MetadataProvider for PostgreSQLMetadataProvider {
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        debug!("[PostgreSQLMetadataProvider::get_schemas] Starting query");

        let rows = sqlx::query(
            r#"
            SELECT nspname as schema_name
            FROM pg_namespace
            WHERE nspname NOT LIKE 'pg_%'
              AND nspname NOT IN ('information_schema', 'pg_toast')
            ORDER BY nspname
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let schemas: Vec<String> = rows.iter().map(|row| row.get::<String, _>(0)).collect();

        debug!(
            "[PostgreSQLMetadataProvider::get_schemas] Found {} schemas",
            schemas.len()
        );
        Ok(schemas)
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        debug!(
            "[PostgreSQLMetadataProvider::get_tables] Starting query for schema: {:?}",
            schema
        );

        let query = if let Some(schema_name) = schema {
            sqlx::query(
                r#"
                SELECT c.relname as table_name
                FROM pg_class c
                INNER JOIN pg_namespace n ON c.relnamespace = n.oid
                WHERE c.relkind IN ('r', 'v', 'm', 'f', 'p')
                  AND n.nspname = $1
                ORDER BY c.relname
                "#,
            )
            .bind(schema_name)
        } else {
            sqlx::query(
                r#"
                SELECT c.relname as table_name
                FROM pg_class c
                INNER JOIN pg_namespace n ON c.relnamespace = n.oid
                WHERE c.relkind IN ('r', 'v', 'm', 'f', 'p')
                  AND n.nspname NOT LIKE 'pg_%'
                  AND n.nspname NOT IN ('information_schema', 'pg_toast')
                ORDER BY n.nspname, c.relname
                "#,
            )
        };

        let rows = query.fetch_all(&self.pool).await?;
        let tables: Vec<String> = rows.iter().map(|row| row.get::<String, _>(0)).collect();

        debug!(
            "[PostgreSQLMetadataProvider::get_tables] Found {} tables",
            tables.len()
        );
        Ok(tables)
    }

    async fn get_columns(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<Vec<String>, DatabaseError> {
        debug!(
            "[PostgreSQLMetadataProvider::get_columns] Starting query for table: '{}', schema: {:?}",
            table, schema
        );

        let schema_name = schema.unwrap_or("public");

        let rows = sqlx::query(
            r#"
            SELECT a.attname as column_name
            FROM pg_attribute a
            INNER JOIN pg_class c ON a.attrelid = c.oid
            INNER JOIN pg_namespace n ON c.relnamespace = n.oid
            WHERE n.nspname = $1
              AND c.relname = $2
              AND a.attnum > 0
              AND NOT a.attisdropped
            ORDER BY a.attnum
            "#,
        )
        .bind(schema_name)
        .bind(table)
        .fetch_all(&self.pool)
        .await?;

        let columns: Vec<String> = rows.iter().map(|row| row.get::<String, _>(0)).collect();

        debug!(
            "[PostgreSQLMetadataProvider::get_columns] Found {} columns",
            columns.len()
        );
        Ok(columns)
    }

    async fn get_functions(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        debug!(
            "[PostgreSQLMetadataProvider::get_functions] Starting query for schema: {:?}",
            schema
        );

        let query = if let Some(schema_name) = schema {
            sqlx::query(
                r#"
                SELECT p.proname as routine_name
                FROM pg_proc p
                INNER JOIN pg_namespace n ON p.pronamespace = n.oid
                WHERE p.prokind = 'f'
                  AND n.nspname = $1
                ORDER BY p.proname
                "#,
            )
            .bind(schema_name)
        } else {
            sqlx::query(
                r#"
                SELECT p.proname as routine_name
                FROM pg_proc p
                INNER JOIN pg_namespace n ON p.pronamespace = n.oid
                WHERE p.prokind = 'f'
                  AND n.nspname NOT LIKE 'pg_%'
                  AND n.nspname NOT IN ('information_schema', 'pg_toast')
                ORDER BY n.nspname, p.proname
                "#,
            )
        };

        let rows = query.fetch_all(&self.pool).await?;
        let functions: Vec<String> = rows.iter().map(|row| row.get::<String, _>(0)).collect();

        debug!(
            "[PostgreSQLMetadataProvider::get_functions] Found {} functions",
            functions.len()
        );
        Ok(functions)
    }

    async fn get_table_details(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<TableDetails, DatabaseError> {
        debug!(
            "[PostgreSQLMetadataProvider::get_table_details] Starting query for table: '{}', schema: {:?}",
            table, schema
        );

        let schema_name = schema.unwrap_or("public");

        // Get basic table information and columns
        let columns = self.get_detailed_columns(table, Some(schema_name)).await?;

        // Get indexes
        let indexes = self.get_table_indexes(table, schema_name).await?;

        // Get foreign keys
        let foreign_keys = self.get_table_foreign_keys(table, schema_name).await?;

        // Get check constraints
        let check_constraints = self.get_table_check_constraints(table, schema_name).await?;

        // Get referenced by information
        let referenced_by = self.get_table_referenced_by(table, schema_name).await?;

        let table_details = TableDetails {
            name: table.to_string(),
            schema: schema_name.to_string(),
            full_name: format!("{}.{}", schema_name, table),
            columns,
            indexes,
            check_constraints,
            foreign_keys,
            referenced_by,
        };

        debug!(
            "[PostgreSQLMetadataProvider::get_table_details] Successfully fetched details for table: '{}'",
            table
        );
        Ok(table_details)
    }

    fn supports_explain(&self) -> bool {
        true
    }

    fn default_schema(&self) -> Option<String> {
        Some("public".to_string())
    }
}

/// Format PostgreSQL complex values using appropriate display adapters
fn format_complex_postgresql_value(
    value: &str,
    detected_type: ComplexDataType,
) -> Result<String, DatabaseError> {
    let config = crate::complex_display::get_global_complex_config();

    match detected_type {
        ComplexDataType::Json => {
            if let Ok(adapter) = JsonDisplayAdapter::new(value.to_string()) {
                Ok(adapter.format(&config))
            } else {
                Ok(value.to_string())
            }
        }
        ComplexDataType::GeoJson => {
            if let Ok(adapter) = GeoJsonDisplayAdapter::new(value.to_string()) {
                Ok(adapter.format(&config))
            } else {
                Ok(value.to_string())
            }
        }
        ComplexDataType::Array => {
            // Parse PostgreSQL array format like {1,2,3} to Vec<String>
            let array_elements = parse_postgresql_array(value)?;
            let adapter = ArrayDisplayAdapter::new(array_elements);
            Ok(adapter.format(&config))
        }
        ComplexDataType::Vector => {
            // PostgreSQL vectors (pgvector) like [1,2,3] - parse as array
            let array_elements = parse_postgresql_vector(value)?;
            let adapter = ArrayDisplayAdapter::new(array_elements);
            Ok(adapter.format(&config))
        }
        _ => {
            // For Map and Tuple types, use default string representation
            Ok(value.to_string())
        }
    }
}

/// Parse PostgreSQL array format like {1,2,3} into Vec<String>
fn parse_postgresql_array(value: &str) -> Result<Vec<String>, DatabaseError> {
    if value.starts_with('{') && value.ends_with('}') {
        let inner = &value[1..value.len() - 1];
        if inner.is_empty() {
            return Ok(vec![]);
        }
        let elements: Vec<String> = inner
            .split(',')
            .map(|s| s.trim().trim_matches('"').to_string())
            .collect();
        Ok(elements)
    } else {
        Ok(vec![value.to_string()])
    }
}

/// Parse PostgreSQL vector format like [1,2,3] into Vec<String>
fn parse_postgresql_vector(value: &str) -> Result<Vec<String>, DatabaseError> {
    if value.starts_with('[') && value.ends_with(']') {
        let inner = &value[1..value.len() - 1];
        if inner.is_empty() {
            return Ok(vec![]);
        }
        let elements: Vec<String> = inner.split(',').map(|s| s.trim().to_string()).collect();
        Ok(elements)
    } else {
        Ok(vec![value.to_string()])
    }
}

/// PostgreSQL database client implementation
pub struct PostgreSQLClient {
    pool: PgPool,
    connection_info: ConnectionInfo,
    current_database: String,
    metadata_provider: PostgreSQLMetadataProvider,
}

impl PostgreSQLClient {
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        // Build PostgreSQL connection options
        let mut connect_options = sqlx::postgres::PgConnectOptions::new();

        if let Some(ref host) = connection_info.host {
            // Resolve *.localhost to 127.0.0.1 for connection, but preserve original hostname
            // for proxy routing via application_name or other mechanisms
            let (connection_host, preserve_original) =
                if host == "localhost" || host.ends_with(".localhost") {
                    ("127.0.0.1", true)
                } else {
                    (host.as_str(), false)
                };

            connect_options = connect_options.host(connection_host);

            // If we resolved a *.localhost domain, preserve the original hostname
            // in the application_name so proxies can route based on it
            if preserve_original && host != "localhost" {
                // Get existing application name or use default
                let app_name = connection_info
                    .options
                    .get("application_name")
                    .map(|name| format!("{name} (host: {host})"))
                    .unwrap_or_else(|| format!("dbcrust (host: {host})"));
                connect_options = connect_options.application_name(&app_name);
            }
        }

        if let Some(port) = connection_info.port {
            connect_options = connect_options.port(port);
        } else if let Some(default_port) = connection_info.default_port() {
            connect_options = connect_options.port(default_port);
        }

        if let Some(ref username) = connection_info.username {
            connect_options = connect_options.username(username);
        }

        if let Some(ref password) = connection_info.password {
            connect_options = connect_options.password(password);
        }

        let database_name = connection_info
            .database
            .clone()
            .unwrap_or_else(|| "postgres".to_string());
        connect_options = connect_options.database(&database_name);

        // Handle SSL mode from options
        if let Some(sslmode) = connection_info.options.get("sslmode") {
            let ssl_mode = match sslmode.as_str() {
                "disable" => sqlx::postgres::PgSslMode::Disable,
                "allow" => sqlx::postgres::PgSslMode::Allow,
                "prefer" => sqlx::postgres::PgSslMode::Prefer,
                "require" => sqlx::postgres::PgSslMode::Require,
                "verify-ca" => sqlx::postgres::PgSslMode::VerifyCa,
                "verify-full" => sqlx::postgres::PgSslMode::VerifyFull,
                _ => sqlx::postgres::PgSslMode::Prefer, // Default
            };
            connect_options = connect_options.ssl_mode(ssl_mode);
        }

        // Configure connection pool - don't connect yet for SSH tunnel scenarios
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .min_connections(0) // Don't pre-connect - wait for SSH tunnel
            .acquire_timeout(std::time::Duration::from_secs(15)) // Allow time for SSH tunnel establishment
            .idle_timeout(std::time::Duration::from_secs(300))
            .test_before_acquire(false) // Skip connection tests
            .connect_with(connect_options)
            .await
            .map_err(|e| DatabaseError::ConnectionError(e.to_string()))?;

        let metadata_provider = PostgreSQLMetadataProvider::new(pool.clone());

        Ok(Self {
            pool,
            connection_info,
            current_database: database_name,
            metadata_provider,
        })
    }

    /// Format PostgreSQL EXPLAIN JSON output for better readability
    async fn format_explain_output(
        &self,
        raw_results: Vec<Vec<String>>,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[PostgreSQLClient::format_explain_output] Formatting PostgreSQL EXPLAIN output");

        if raw_results.is_empty() {
            return Ok(vec![vec!["No query plan available".to_string()]]);
        }

        let mut formatted_results = Vec::new();
        formatted_results.push(vec!["PostgreSQL Query Plan".to_string()]);
        formatted_results.push(vec!["".to_string()]);

        // Process each row (usually just one row for JSON format)
        // Skip the first row which contains column headers
        for (i, row) in raw_results.iter().enumerate() {
            if i == 0 {
                // Skip header row
                continue;
            }

            let json_str = &row[0];
            debug!(
                "[PostgreSQLClient::format_explain_output] Attempting to parse JSON: {}",
                json_str
            );

            // Parse JSON
            match serde_json::from_str::<serde_json::Value>(json_str) {
                Ok(json) => {
                    // Use performance analyzer to get metrics
                    let performance_metrics = PerformanceAnalyzer::analyze_postgresql_plan(&json);

                    // Add performance summary header
                    let performance_summary =
                        PerformanceAnalyzer::format_metrics_with_colors(&performance_metrics);
                    for line in performance_summary {
                        formatted_results.push(vec![line]);
                    }

                    formatted_results.push(vec!["".to_string()]);
                    formatted_results.push(vec![
                        "💡 Use \\ecopy to copy the raw JSON plan to clipboard".to_string(),
                    ]);
                }
                Err(e) => {
                    debug!(
                        "[PostgreSQLClient::format_explain_output] JSON parse error: {}",
                        e
                    );
                    formatted_results.push(vec![format!("JSON Parse Error: {}", e)]);
                    formatted_results.push(vec![json_str.clone()]);
                }
            }
        }

        if formatted_results.len() <= 2 {
            formatted_results.push(vec!["No query plan information available".to_string()]);
        }

        Ok(formatted_results)
    }
}

#[async_trait]
impl DatabaseClient for PostgreSQLClient {
    async fn execute_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[PostgreSQLClient::execute_query] Executing query");

        // Add timeout to prevent hanging queries
        let timeout_duration = std::time::Duration::from_secs(30); // 30 seconds timeout
        let rows =
            match tokio::time::timeout(timeout_duration, sqlx::query(sql).fetch_all(&self.pool))
                .await
            {
                Ok(Ok(rows)) => rows,
                Ok(Err(e)) => return Err(DatabaseError::QueryError(e.to_string())),
                Err(_) => {
                    return Err(DatabaseError::QueryError(
                        "Query timed out after 30 seconds".to_string(),
                    ));
                }
            };

        if rows.is_empty() {
            return Ok(vec![]);
        }

        let mut results = Vec::new();

        // Get column names from the first row
        let first_row = &rows[0];
        let column_names: Vec<String> = (0..first_row.len())
            .map(|i| first_row.column(i).name().to_string())
            .collect();

        results.push(column_names);

        // Convert rows to strings
        for row in rows {
            let mut string_row = Vec::new();
            for i in 0..row.len() {
                let value = format_postgresql_value(&row, i)?;
                string_row.push(value);
            }
            results.push(string_row);
        }

        debug!(
            "[PostgreSQLClient::execute_query] Query completed with {} rows",
            results.len() - 1
        );
        Ok(results)
    }

    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError> {
        debug!("[PostgreSQLClient::test_query] Testing query for validation");
        // For PostgreSQL, we can use EXPLAIN to validate query syntax without executing it
        let explain_sql = format!("EXPLAIN {}", sql);
        let timeout_duration = std::time::Duration::from_secs(10); // Shorter timeout for tests

        match tokio::time::timeout(
            timeout_duration,
            sqlx::query(&explain_sql).fetch_all(&self.pool),
        )
        .await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(DatabaseError::QueryError(format!(
                "Query validation failed: {}",
                e
            ))),
            Err(_) => Err(DatabaseError::QueryError(
                "Query validation timed out".to_string(),
            )),
        }
    }

    async fn explain_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        let explain_sql = format!("EXPLAIN (FORMAT JSON) {sql}");
        let raw_results = self.execute_query(&explain_sql).await?;
        self.format_explain_output(raw_results).await
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        let explain_sql = format!("EXPLAIN (FORMAT JSON) {sql}");
        self.execute_query(&explain_sql).await
    }

    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        let query = r#"
            SELECT
                d.datname AS "Name",
                pg_get_userbyid(d.datdba) AS "Owner",
                pg_encoding_to_char(d.encoding) AS "Encoding",
                CASE WHEN d.datcollate = d.datctype THEN d.datcollate ELSE d.datcollate || '/' || d.datctype END AS "Collate",
                pg_size_pretty(pg_database_size(d.datname)) AS "Size"
            FROM
                pg_database d
            WHERE
                d.datistemplate = false
            ORDER BY
                d.datname
        "#;

        self.execute_query(query).await
    }

    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError> {
        // Create new connection info with updated database
        let mut new_connection_info = self.connection_info.clone();
        new_connection_info.database = Some(database.to_string());

        // Create new client with the updated connection
        let new_client = PostgreSQLClient::new(new_connection_info).await?;

        // Replace current connection
        *self = new_client;

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
        // Try a simple query to check if connection is still alive
        (sqlx::query("SELECT 1").fetch_one(&self.pool).await).is_ok()
    }

    async fn close(&mut self) -> Result<(), DatabaseError> {
        self.pool.close().await;
        Ok(())
    }

    async fn get_server_info(&self) -> Result<crate::database::ServerInfo, DatabaseError> {
        debug!("[PostgreSQLClient::get_server_info] Fetching server version information");

        // Query PostgreSQL version
        let version_query = "SELECT version()";
        let version_row = sqlx::query(version_query)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                DatabaseError::QueryError(format!("Failed to get PostgreSQL version: {}", e))
            })?;

        let version_string: String = version_row.get(0);
        debug!(
            "[PostgreSQLClient::get_server_info] Raw version string: {}",
            version_string
        );

        // Create ServerInfo for PostgreSQL
        let mut server_info = crate::database::ServerInfo::postgresql(version_string);

        // Add any additional PostgreSQL-specific information
        server_info.additional_info.insert(
            "current_database".to_string(),
            self.current_database.clone(),
        );

        // Try to get additional server information (non-critical, don't fail if these queries fail)
        if let Ok(uptime_row) = sqlx::query(
            "SELECT EXTRACT(EPOCH FROM (now() - pg_postmaster_start_time())) AS uptime_seconds",
        )
        .fetch_one(&self.pool)
        .await
        {
            if let Ok(uptime_seconds) = uptime_row.try_get::<f64, _>(0) {
                let uptime_days = (uptime_seconds / 86400.0).floor() as i32;
                server_info
                    .additional_info
                    .insert("uptime_days".to_string(), uptime_days.to_string());
            }
        }

        if let Ok(settings_row) =
            sqlx::query("SELECT setting FROM pg_settings WHERE name = 'max_connections'")
                .fetch_one(&self.pool)
                .await
        {
            if let Ok(max_connections) = settings_row.try_get::<String, _>(0) {
                server_info
                    .additional_info
                    .insert("max_connections".to_string(), max_connections);
            }
        }

        debug!("[PostgreSQLClient::get_server_info] Server info retrieved successfully");
        Ok(server_info)
    }
}

/// Handle custom PostgreSQL types (enums, composite types, domains) using raw value access
fn handle_custom_postgresql_type(
    row: &PgRow,
    column_index: usize,
    type_name: &str,
) -> Result<String, DatabaseError> {
    use sqlx::ValueRef;

    debug!(
        "[PostgreSQL] Attempting to handle custom type '{}' using raw value approach",
        type_name
    );

    // Try to get raw value reference
    match row.try_get_raw(column_index) {
        Ok(value_ref) => {
            // Check if value is NULL
            if value_ref.is_null() {
                debug!("[PostgreSQL] Custom type '{}' value is NULL", type_name);
                return Ok("".to_string());
            }

            // Try to get the value as string from raw bytes
            // PostgreSQL custom types are typically stored as text representations
            match value_ref.as_str() {
                Ok(text_value) => {
                    debug!(
                        "[PostgreSQL] Successfully converted custom type '{}' to string: '{}'",
                        type_name, text_value
                    );
                    Ok(text_value.to_string())
                }
                Err(_) => {
                    // If as_str() fails, try to decode as bytes and convert to UTF-8
                    let bytes = value_ref.as_bytes().map_err(|e| {
                        DatabaseError::QueryError(format!(
                            "Failed to get raw bytes for custom type '{}': {}",
                            type_name, e
                        ))
                    })?;

                    match std::str::from_utf8(bytes) {
                        Ok(utf8_str) => {
                            debug!(
                                "[PostgreSQL] Successfully decoded custom type '{}' from UTF-8 bytes: '{}'",
                                type_name, utf8_str
                            );
                            Ok(utf8_str.to_string())
                        }
                        Err(e) => {
                            debug!(
                                "[PostgreSQL] Failed to decode custom type '{}' as UTF-8: {}",
                                type_name, e
                            );
                            // As a last resort, represent as hex bytes
                            let hex_representation = hex::encode(bytes);
                            Ok(format!("\\x{}", hex_representation))
                        }
                    }
                }
            }
        }
        Err(e) => Err(DatabaseError::QueryError(format!(
            "Failed to get raw value for custom type '{}': {}",
            type_name, e
        ))),
    }
}

/// Format a PostgreSQL value to string representation
fn format_postgresql_value(row: &PgRow, column_index: usize) -> Result<String, DatabaseError> {
    use sqlx::TypeInfo;

    let column = row.column(column_index);
    let type_name = column.type_info().name();

    // Normalize type name to uppercase for consistent matching
    // PostgreSQL standard types come as uppercase (TEXT, VARCHAR)
    // Extension types come as lowercase (halfvec, geometry)
    let type_name_upper = type_name.to_uppercase();

    debug!(
        "[PostgreSQL] Processing type '{}' (normalized: '{}')",
        type_name, type_name_upper
    );

    // Handle NULL values first - try the most generic nullable type
    if let Ok(value) = row.try_get::<Option<String>, _>(column_index) {
        if value.is_none() {
            return Ok("".to_string());
        }
    }

    // Match on normalized PostgreSQL type names and convert appropriately
    match type_name_upper.as_str() {
        // String types
        "TEXT" | "VARCHAR" | "CHAR" | "BPCHAR" | "NAME" | "CITEXT" => row
            .try_get::<String, _>(column_index)
            .map_err(|e| DatabaseError::QueryError(e.to_string())),

        // Integer types
        "INT2" | "SMALLINT" => row
            .try_get::<i16, _>(column_index)
            .map(|v| v.to_string())
            .map_err(|e| DatabaseError::QueryError(e.to_string())),
        "INT4" | "INTEGER" | "SERIAL" => row
            .try_get::<i32, _>(column_index)
            .map(|v| v.to_string())
            .map_err(|e| DatabaseError::QueryError(e.to_string())),
        "INT8" | "BIGINT" | "BIGSERIAL" => row
            .try_get::<i64, _>(column_index)
            .map(|v| v.to_string())
            .map_err(|e| DatabaseError::QueryError(e.to_string())),
        "OID" => row
            .try_get::<i32, _>(column_index)
            .map(|v| v.to_string())
            .map_err(|e| DatabaseError::QueryError(e.to_string())),

        // Floating point types
        "FLOAT4" | "REAL" => row
            .try_get::<f32, _>(column_index)
            .map(|v| v.to_string())
            .map_err(|e| DatabaseError::QueryError(e.to_string())),
        "FLOAT8" | "DOUBLE PRECISION" => row
            .try_get::<f64, _>(column_index)
            .map(|v| v.to_string())
            .map_err(|e| DatabaseError::QueryError(e.to_string())),
        "NUMERIC" | "DECIMAL" => {
            row.try_get::<sqlx::types::Decimal, _>(column_index)
                .map(|v| v.to_string())
                .or_else(|_| {
                    // Fallback for numeric values that can't be represented as Decimal
                    row.try_get::<String, _>(column_index)
                })
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }

        // Boolean type
        "BOOL" | "BOOLEAN" => row
            .try_get::<bool, _>(column_index)
            .map(|v| v.to_string())
            .map_err(|e| DatabaseError::QueryError(e.to_string())),

        // Date and time types
        "TIMESTAMPTZ" => row
            .try_get::<chrono::DateTime<chrono::Utc>, _>(column_index)
            .map(|v| v.to_rfc3339())
            .map_err(|e| DatabaseError::QueryError(e.to_string())),
        "TIMESTAMP" => row
            .try_get::<chrono::NaiveDateTime, _>(column_index)
            .map(|v| v.to_string())
            .map_err(|e| DatabaseError::QueryError(e.to_string())),
        "DATE" => row
            .try_get::<chrono::NaiveDate, _>(column_index)
            .map(|v| v.to_string())
            .map_err(|e| DatabaseError::QueryError(e.to_string())),
        "TIME" => row
            .try_get::<chrono::NaiveTime, _>(column_index)
            .map(|v| v.to_string())
            .map_err(|e| DatabaseError::QueryError(e.to_string())),
        "TIMETZ" => {
            // PostgreSQL TIMETZ - for now treat as string since chrono doesn't have a direct equivalent
            row.try_get::<String, _>(column_index)
                .or_else(|_| {
                    // If string doesn't work, try as time and convert
                    row.try_get::<chrono::NaiveTime, _>(column_index)
                        .map(|v| v.to_string())
                })
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "INTERVAL" => {
            // PostgreSQL intervals - SQLx doesn't have built-in support, try as string
            row.try_get::<String, _>(column_index)
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }

        // JSON types with complex display support
        "JSON" | "JSONB" => {
            let raw_value = row
                .try_get::<serde_json::Value, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))?;

            // Use JsonDisplayAdapter for enhanced formatting
            if let Ok(adapter) = JsonDisplayAdapter::new(raw_value.clone()) {
                let config = crate::complex_display::get_global_complex_config();
                Ok(adapter.format(&config))
            } else {
                Ok(raw_value)
            }
        }

        // UUID type
        "UUID" => row
            .try_get::<sqlx::types::Uuid, _>(column_index)
            .map(|v| v.to_string())
            .map_err(|e| DatabaseError::QueryError(e.to_string())),

        // Binary data types
        "BYTEA" => row
            .try_get::<Vec<u8>, _>(column_index)
            .map(|v| format!("\\x{}", hex::encode(v)))
            .map_err(|e| DatabaseError::QueryError(e.to_string())),

        // Network address types
        "INET" | "CIDR" => {
            row.try_get::<std::net::IpAddr, _>(column_index)
                .map(|v| v.to_string())
                .or_else(|_| {
                    // Fallback to string if IP parsing fails
                    row.try_get::<String, _>(column_index)
                })
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "MACADDR" | "MACADDR8" => {
            // Try MAC address type if available, otherwise fallback to string
            row.try_get::<String, _>(column_index)
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }

        // Array types - handle common array types
        t if t.ends_with("[]") => {
            // For arrays, try to get as JSON first, then fallback to string
            match row.try_get::<serde_json::Value, _>(column_index) {
                Ok(json_val) => Ok(json_val.to_string()),
                Err(_) => {
                    // Fallback to string representation
                    row.try_get::<String, _>(column_index)
                        .map_err(|e| DatabaseError::QueryError(e.to_string()))
                }
            }
        }

        // Geometric types - these are complex, try as string
        "POINT" | "LINE" | "LSEG" | "BOX" | "PATH" | "POLYGON" | "CIRCLE" => row
            .try_get::<String, _>(column_index)
            .map_err(|e| DatabaseError::QueryError(e.to_string())),

        // Range types
        "INT4RANGE" | "INT8RANGE" | "NUMRANGE" | "TSRANGE" | "TSTZRANGE" | "DATERANGE" => row
            .try_get::<String, _>(column_index)
            .map_err(|e| DatabaseError::QueryError(e.to_string())),

        // XML type
        "XML" => row
            .try_get::<String, _>(column_index)
            .map_err(|e| DatabaseError::QueryError(e.to_string())),

        // Bit string types
        "BIT" | "VARBIT" => {
            // Bit strings as string representation
            row.try_get::<String, _>(column_index)
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }

        // Money type
        "MONEY" => {
            // Money as string representation
            row.try_get::<String, _>(column_index)
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }

        // Extension types - provide specific guidance for known extensions
        "VECTOR" | "HALFVEC" | "SPARSEVEC" => {
            debug!(
                "[PostgreSQL] Processing pgvector type '{}' (normalized: '{}')",
                type_name, type_name_upper
            );

            // Use VectorFormatter for smart vector display with global config
            let vector_config = crate::vector_display::get_global_vector_config();
            let formatter = crate::vector_display::VectorFormatter::new(&vector_config);

            // Use proper pgvector types (always available)
            match type_name_upper.as_str() {
                "VECTOR" => row
                    .try_get::<pgvector::Vector, _>(column_index)
                    .map(|v| {
                        let values: Vec<f32> = v.as_slice().to_vec();
                        formatter.format(&values)
                    })
                    .map_err(|e| {
                        DatabaseError::QueryError(format!(
                            "Failed to format {} type '{}': {}",
                            type_name_upper, type_name, e
                        ))
                    }),
                "HALFVEC" => row
                    .try_get::<pgvector::HalfVector, _>(column_index)
                    .map(|v| formatter.format_half(v.as_slice()))
                    .map_err(|e| {
                        DatabaseError::QueryError(format!(
                            "Failed to format {} type '{}': {}",
                            type_name_upper, type_name, e
                        ))
                    }),
                "SPARSEVEC" => row
                    .try_get::<pgvector::SparseVector, _>(column_index)
                    .map(|v| formatter.format_sparse(v.indices(), v.values()))
                    .map_err(|e| {
                        DatabaseError::QueryError(format!(
                            "Failed to format {} type '{}': {}",
                            type_name_upper, type_name, e
                        ))
                    }),
                _ => unreachable!(),
            }
        }

        "GEOMETRY" | "GEOGRAPHY" | "BOX2D" | "BOX3D" => {
            debug!(
                "[PostgreSQL] Processing PostGIS type '{}' (normalized: '{}')",
                type_name, type_name_upper
            );

            let raw_value = row
                .try_get::<String, _>(column_index)
                .map_err(|e| DatabaseError::QueryError(e.to_string()))?;

            // Enhanced PostGIS processing with complex display support
            if raw_value.starts_with("01") || raw_value.starts_with("00") {
                // This looks like WKB (Well-Known Binary) format
                Ok(format!("WKB: {}", raw_value))
            } else if GenericComplexTypeDetector::detect_type(&raw_value)
                == Some(ComplexDataType::GeoJson)
            {
                // Try to use GeoJsonDisplayAdapter for GeoJSON-like content
                if let Ok(adapter) = GeoJsonDisplayAdapter::new(raw_value.clone()) {
                    let config = crate::complex_display::get_global_complex_config();
                    Ok(adapter.format(&config))
                } else {
                    Ok(raw_value)
                }
            } else {
                // Use generic complex type detection for other formats
                if let Some(detected_type) = GenericComplexTypeDetector::detect_type(&raw_value) {
                    format_complex_postgresql_value(&raw_value, detected_type)
                } else {
                    Ok(raw_value)
                }
            }
        }

        "HSTORE" => {
            debug!(
                "[PostgreSQL] Processing hstore type '{}' (normalized: '{}')",
                type_name, type_name_upper
            );
            row.try_get::<String, _>(column_index)
                .map(|s| {
                    // Clean up hstore format for better readability
                    if s.starts_with('"') && s.ends_with('"') && s.len() > 1 {
                        // Remove outer quotes if present
                        s[1..s.len() - 1].to_string()
                    } else {
                        s
                    }
                    // Could add more hstore-specific formatting here
                })
                .map_err(|e| {
                    DatabaseError::QueryError(format!(
                        "Failed to format {} type '{}': {}",
                        type_name_upper, type_name, e
                    ))
                })
        }

        "LTREE" | "LQUERY" | "LTXTQUERY" => {
            debug!(
                "[PostgreSQL] Processing ltree type '{}' (normalized: '{}')",
                type_name, type_name_upper
            );
            row.try_get::<String, _>(column_index)
                .map(|s| {
                    // ltree paths are typically dot-separated, ensure they're readable
                    match type_name_upper.as_str() {
                        "LTREE" => s,                               // Path format is already readable
                        "LQUERY" => format!("Query: {}", s), // Prefix to indicate it's a query
                        "LTXTQUERY" => format!("TextQuery: {}", s), // Prefix for text query
                        _ => s,
                    }
                })
                .map_err(|e| {
                    DatabaseError::QueryError(format!(
                        "Failed to format {} type '{}': {}",
                        type_name_upper, type_name, e
                    ))
                })
        }

        "CUBE" => {
            debug!(
                "[PostgreSQL] Processing cube type '{}' (normalized: '{}')",
                type_name, type_name_upper
            );
            row.try_get::<String, _>(column_index)
                .map(|s| {
                    // Cube format is typically (lower_bounds, upper_bounds) or just (point)
                    // Clean up for better readability
                    if s.starts_with('(') && s.ends_with(')') {
                        s // Already formatted properly
                    } else {
                        format!("Cube: {}", s)
                    }
                })
                .map_err(|e| {
                    DatabaseError::QueryError(format!(
                        "Failed to format {} type '{}': {}",
                        type_name_upper, type_name, e
                    ))
                })
        }

        // Custom/composite types and unknown types - try as string with enhanced error messages
        _ => {
            // Check if this might be a known extension type we haven't implemented yet
            let extension_hint = if type_name_upper.contains("VECTOR") {
                " (possibly pgvector-related)"
            } else if type_name_upper.contains("GEOM") || type_name_upper.contains("GEOGRAPHY") {
                " (possibly PostGIS-related)"
            } else if type_name.contains("_") {
                " (possibly custom extension type)"
            } else {
                ""
            };

            debug!(
                "[PostgreSQL] Attempting string fallback for unknown type '{}' (normalized: '{}'){}",
                type_name, type_name_upper, extension_hint
            );

            // First try normal string conversion
            match row.try_get::<String, _>(column_index) {
                Ok(value) => Ok(value),
                Err(e) => {
                    // If string conversion fails, try to handle as custom type using raw value
                    debug!(
                        "[PostgreSQL] String conversion failed for type '{}', attempting raw value fallback: {}",
                        type_name, e
                    );

                    match handle_custom_postgresql_type(row, column_index, type_name) {
                        Ok(value) => {
                            debug!(
                                "[PostgreSQL] Successfully handled custom type '{}' using raw value approach",
                                type_name
                            );
                            Ok(value)
                        }
                        Err(raw_error) => {
                            debug!(
                                "[PostgreSQL] Raw value fallback also failed for type '{}': {}",
                                type_name, raw_error
                            );

                            // Return enhanced error message
                            Err(DatabaseError::QueryError(format!(
                                "Unable to format PostgreSQL type '{}'{}.{} Original error: {}. Raw value fallback error: {}",
                                type_name,
                                extension_hint,
                                if !extension_hint.is_empty() {
                                    " Consider enabling appropriate extension support or check if the extension is installed."
                                } else {
                                    " This may be a custom type (enum/composite/domain) that requires special handling."
                                },
                                e,
                                raw_error
                            )))
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::DatabaseType;
    use std::collections::HashMap;

    #[test]
    fn test_builtin_type_detection() {
        // Test that built-in PostgreSQL types are correctly identified
        assert!(is_builtin_postgresql_type("TEXT"));
        assert!(is_builtin_postgresql_type("INTEGER"));
        assert!(is_builtin_postgresql_type("TIMESTAMP"));
        assert!(is_builtin_postgresql_type("JSON"));
        assert!(is_builtin_postgresql_type("JSONB"));

        // Test that extension types are correctly identified as built-in (so not treated as enums)
        assert!(is_builtin_postgresql_type("VECTOR"));
        assert!(is_builtin_postgresql_type("HALFVEC"));
        assert!(is_builtin_postgresql_type("SPARSEVEC"));
        assert!(is_builtin_postgresql_type("GEOMETRY"));
        assert!(is_builtin_postgresql_type("GEOGRAPHY"));
        assert!(is_builtin_postgresql_type("HSTORE"));
        assert!(is_builtin_postgresql_type("LTREE"));
        assert!(is_builtin_postgresql_type("CUBE"));

        // Test that actual custom enum types are NOT identified as built-in
        assert!(!is_builtin_postgresql_type("userrole"));
        assert!(!is_builtin_postgresql_type("status_type"));
        assert!(!is_builtin_postgresql_type("custom_enum"));

        // Test array types
        assert!(is_builtin_postgresql_type("text[]"));
        assert!(is_builtin_postgresql_type("integer[]"));
    }

    #[tokio::test]
    async fn test_format_explain_output() {
        // Create a mock PostgreSQLClient for testing
        // Note: This test doesn't require a real database connection
        let raw_results = [
            vec!["QUERY PLAN".to_string()],  // Header row
            vec![r#"[{"Plan": {"Node Type": "Seq Scan", "Relation Name": "test_table", "Alias": "test_table", "Startup Cost": 0.00, "Total Cost": 10.00, "Plan Rows": 100, "Plan Width": 32}}]"#.to_string()],
        ];

        // We can't easily test the full format_explain_output without a real client,
        // but we can test the JSON parsing logic
        let json_str = &raw_results[1][0];
        let json_result = serde_json::from_str::<serde_json::Value>(json_str);

        assert!(
            json_result.is_ok(),
            "Should successfully parse EXPLAIN JSON output"
        );

        // Test that trying to parse the header row would fail
        let header_str = &raw_results[0][0];
        let header_result = serde_json::from_str::<serde_json::Value>(header_str);

        assert!(
            header_result.is_err(),
            "Should fail to parse header row as JSON"
        );
    }

    #[tokio::test]
    async fn test_postgresql_client_creation() {
        let connection_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: Some("localhost".to_string()),
            port: Some(5432),
            username: Some("postgres".to_string()),
            password: Some("test".to_string()),
            database: Some("postgres".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };

        // This test will fail if no PostgreSQL server is running, which is expected
        // In a real test environment, we'd use a test database or mock
        match PostgreSQLClient::new(connection_info).await {
            Ok(_) => {
                // Connection successful - this would happen in integration tests
            }
            Err(DatabaseError::ConnectionError(_)) => {
                // Expected when no test database is available
            }
            Err(e) => {
                panic!("Unexpected error: {e:?}");
            }
        }
    }
}
