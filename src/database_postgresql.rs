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
use sqlx::{Column, Row, TypeInfo};
use tracing::{debug, warn};

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
            .map(|t| format!("'{t}'"))
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
              AND t.typname IN ({type_list})
            ORDER BY t.typname, e.enumsortorder
            "#
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
            full_name: format!("{schema_name}.{table}"),
            columns,
            indexes,
            check_constraints,
            foreign_keys,
            referenced_by,
            nested_field_details: std::collections::HashMap::new(),
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

/// Format a Rust Vec as a PostgreSQL array string representation
/// e.g., vec!["a", "b", "c"] -> "{a,b,c}"
fn format_array_as_postgres<T: std::fmt::Display>(arr: &[T]) -> String {
    if arr.is_empty() {
        return "{}".to_string();
    }
    let elements: Vec<String> = arr
        .iter()
        .map(|v| {
            let s = v.to_string();
            // Quote strings that contain special characters
            if s.contains(',')
                || s.contains('"')
                || s.contains('{')
                || s.contains('}')
                || s.contains('\\')
                || s.contains(' ')
            {
                format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
            } else if s.is_empty() {
                "\"\"".to_string()
            } else {
                s
            }
        })
        .collect();
    format!("{{{}}}", elements.join(","))
}

/// Format a Rust Vec<Option<T>> as a PostgreSQL array string representation
/// Handles NULL elements within arrays
/// e.g., vec![Some("a"), None, Some("c")] -> "{a,NULL,c}"
fn format_option_array_as_postgres<T: std::fmt::Display>(arr: &[Option<T>]) -> String {
    if arr.is_empty() {
        return "{}".to_string();
    }
    let elements: Vec<String> = arr
        .iter()
        .map(|v| match v {
            Some(val) => {
                let s = val.to_string();
                // Quote strings that contain special characters
                if s.contains(',')
                    || s.contains('"')
                    || s.contains('{')
                    || s.contains('}')
                    || s.contains('\\')
                    || s.contains(' ')
                {
                    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
                } else if s.is_empty() {
                    "\"\"".to_string()
                } else {
                    s
                }
            }
            None => "NULL".to_string(),
        })
        .collect();
    format!("{{{}}}", elements.join(","))
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

    /// Format PostgreSQL value to string without display adapters (for raw JSON)
    fn format_raw_value(&self, row: &PgRow, column_index: usize) -> Result<String, DatabaseError> {
        use sqlx::TypeInfo;

        let column = row.column(column_index);
        let type_name = column.type_info().name();
        let type_name_upper = type_name.to_uppercase();

        debug!(
            "[PostgreSQL] Processing type '{}' for raw value (normalized: '{}')",
            type_name, type_name_upper
        );

        // Handle NULL values first
        if let Ok(value) = row.try_get::<Option<String>, _>(column_index) {
            if value.is_none() {
                return Ok("".to_string());
            }
        }

        // For JSON types, return raw JSON without display formatting
        if matches!(type_name_upper.as_str(), "JSON" | "JSONB") {
            return row
                .try_get::<serde_json::Value, _>(column_index)
                .map(|v| v.to_string()) // Raw JSON string
                .map_err(|e| DatabaseError::QueryError(e.to_string()));
        }

        // For other types, use the regular formatting function
        format_postgresql_value(row, column_index)
    }

    /// Execute a query and return raw values without display formatting (for EXPLAIN JSON)
    async fn execute_query_raw_json(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[PostgreSQLClient::execute_query_raw_json] Executing query for raw JSON");

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

        // Convert rows to strings WITHOUT display formatting for JSON
        for row in rows {
            let mut string_row = Vec::new();
            for i in 0..row.len() {
                // For EXPLAIN queries, we need raw JSON, not formatted display
                let value = self.format_raw_value(&row, i)?;
                string_row.push(value);
            }
            results.push(string_row);
        }

        debug!(
            "[PostgreSQLClient::execute_query_raw_json] Query completed with {} rows",
            results.len() - 1
        );
        Ok(results)
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
                let value = match format_postgresql_value(&row, i) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(
                            "[PostgreSQL] Failed to decode column '{}' (type: {}): {}",
                            row.column(i).name(),
                            row.column(i).type_info().name(),
                            e
                        );
                        "?error?".to_string()
                    }
                };
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
        let explain_sql = format!("EXPLAIN {sql}");
        let timeout_duration = std::time::Duration::from_secs(10); // Shorter timeout for tests

        match tokio::time::timeout(
            timeout_duration,
            sqlx::query(&explain_sql).fetch_all(&self.pool),
        )
        .await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(DatabaseError::QueryError(format!(
                "Query validation failed: {e}"
            ))),
            Err(_) => Err(DatabaseError::QueryError(
                "Query validation timed out".to_string(),
            )),
        }
    }

    async fn explain_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        let explain_sql = format!("EXPLAIN (FORMAT JSON) {sql}");
        let raw_results = self.execute_query_raw_json(&explain_sql).await?;
        self.format_explain_output(raw_results).await
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        let explain_sql = format!("EXPLAIN (FORMAT JSON) {sql}");
        self.execute_query_raw_json(&explain_sql).await
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
                DatabaseError::QueryError(format!("Failed to get PostgreSQL version: {e}"))
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

/// Format PostgreSQL INTERVAL from its components (microseconds, days, months)
/// This is a pure function that can be tested independently
fn format_interval_components(microseconds: i64, days: i32, months: i32) -> String {
    let mut parts = Vec::new();

    // Handle years and months
    if months != 0 {
        let years = months / 12;
        let remaining_months = months % 12;
        if years != 0 {
            parts.push(format!(
                "{} {}",
                years,
                if years.abs() == 1 { "year" } else { "years" }
            ));
        }
        if remaining_months != 0 {
            parts.push(format!(
                "{} {}",
                remaining_months,
                if remaining_months.abs() == 1 {
                    "mon"
                } else {
                    "mons"
                }
            ));
        }
    }

    // Handle days
    if days != 0 {
        parts.push(format!(
            "{} {}",
            days,
            if days.abs() == 1 { "day" } else { "days" }
        ));
    }

    // Handle time component (microseconds)
    if microseconds != 0 || parts.is_empty() {
        let is_negative = microseconds < 0;
        let abs_microseconds = microseconds.abs();
        let total_seconds = abs_microseconds / 1_000_000;
        let remaining_micros = abs_microseconds % 1_000_000;

        let hours = total_seconds / 3600;
        let minutes = (total_seconds % 3600) / 60;
        let seconds = total_seconds % 60;

        let sign = if is_negative { "-" } else { "" };

        if remaining_micros > 0 {
            // Format with fractional seconds
            let frac = format!("{:06}", remaining_micros);
            let frac_trimmed = frac.trim_end_matches('0');
            parts.push(format!(
                "{}{:02}:{:02}:{:02}.{}",
                sign, hours, minutes, seconds, frac_trimmed
            ));
        } else if hours != 0 || minutes != 0 || seconds != 0 || parts.is_empty() {
            parts.push(format!(
                "{}{:02}:{:02}:{:02}",
                sign, hours, minutes, seconds
            ));
        }
    }

    parts.join(" ")
}

/// Decode PostgreSQL INTERVAL type from raw binary format
/// PostgreSQL sends INTERVAL as 16 bytes in binary protocol:
/// - 8 bytes: microseconds (i64, big-endian)
/// - 4 bytes: days (i32, big-endian)
/// - 4 bytes: months (i32, big-endian)
fn decode_postgresql_interval(row: &PgRow, column_index: usize) -> Result<String, DatabaseError> {
    use sqlx::ValueRef;

    match row.try_get_raw(column_index) {
        Ok(value_ref) => {
            if value_ref.is_null() {
                return Ok(String::new());
            }

            // Try to get raw bytes
            match value_ref.as_bytes() {
                Ok(bytes) if bytes.len() == 16 => {
                    // Parse the 16-byte interval format
                    let microseconds =
                        i64::from_be_bytes(bytes[0..8].try_into().unwrap_or([0u8; 8]));
                    let days = i32::from_be_bytes(bytes[8..12].try_into().unwrap_or([0u8; 4]));
                    let months = i32::from_be_bytes(bytes[12..16].try_into().unwrap_or([0u8; 4]));

                    Ok(format_interval_components(microseconds, days, months))
                }
                Ok(_bytes) => {
                    // Not the expected 16-byte format, fall back to string representation
                    value_ref
                        .as_str()
                        .map(|s| s.to_string())
                        .or_else(|_| Ok("(interval)".to_string()))
                }
                Err(_) => {
                    // Try string representation as fallback
                    value_ref.as_str().map(|s| s.to_string()).map_err(|e| {
                        DatabaseError::QueryError(format!("Failed to decode INTERVAL: {e}"))
                    })
                }
            }
        }
        Err(e) => Err(DatabaseError::QueryError(format!(
            "Failed to get INTERVAL value: {e}"
        ))),
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
                            "Failed to get raw bytes for custom type '{type_name}': {e}"
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
                            Ok(format!("\\x{hex_representation}"))
                        }
                    }
                }
            }
        }
        Err(e) => Err(DatabaseError::QueryError(format!(
            "Failed to get raw value for custom type '{type_name}': {e}"
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

    // Handle NULL values first - but NOT for array types (they need special handling)
    // Array types end with "[]" and need Option<Vec<T>>, not Option<String>
    if !type_name_upper.ends_with("[]") {
        if let Ok(value) = row.try_get::<Option<String>, _>(column_index) {
            if value.is_none() {
                return Ok("".to_string());
            }
        }
    }

    // Match on normalized PostgreSQL type names and convert appropriately
    match type_name_upper.as_str() {
        // String types
        "TEXT" | "VARCHAR" | "CHAR" | "BPCHAR" | "NAME" | "CITEXT" => row
            .try_get::<String, _>(column_index)
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),

        // Integer types
        "INT2" | "SMALLINT" => row
            .try_get::<i16, _>(column_index)
            .map(|v| v.to_string())
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),
        "INT4" | "INTEGER" | "SERIAL" => row
            .try_get::<i32, _>(column_index)
            .map(|v| v.to_string())
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),
        "INT8" | "BIGINT" | "BIGSERIAL" => row
            .try_get::<i64, _>(column_index)
            .map(|v| v.to_string())
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),
        "OID" => row
            .try_get::<i32, _>(column_index)
            .map(|v| v.to_string())
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),

        // Floating point types
        "FLOAT4" | "REAL" => row
            .try_get::<f32, _>(column_index)
            .map(|v| v.to_string())
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),
        "FLOAT8" | "DOUBLE PRECISION" => row
            .try_get::<f64, _>(column_index)
            .map(|v| v.to_string())
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),
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
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),

        // Date and time types
        "TIMESTAMPTZ" => row
            .try_get::<chrono::DateTime<chrono::Utc>, _>(column_index)
            .map(|v| v.to_rfc3339())
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),
        "TIMESTAMP" => row
            .try_get::<chrono::NaiveDateTime, _>(column_index)
            .map(|v| v.to_string())
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),
        "DATE" => row
            .try_get::<chrono::NaiveDate, _>(column_index)
            .map(|v| v.to_string())
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),
        "TIME" => row
            .try_get::<chrono::NaiveTime, _>(column_index)
            .map(|v| v.to_string())
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),
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
            // PostgreSQL intervals - SQLx doesn't have built-in support for INTERVAL type
            // PostgreSQL sends INTERVAL as 16 bytes: 8 bytes microseconds, 4 bytes days, 4 bytes months
            decode_postgresql_interval(row, column_index)
        }

        // JSON types with complex display support
        "JSON" | "JSONB" => {
            let raw_value = row
                .try_get::<serde_json::Value, _>(column_index)
                .map(|v| v.to_string())
                .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name))?;

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
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),

        // Binary data types
        "BYTEA" => row
            .try_get::<Vec<u8>, _>(column_index)
            .map(|v| format!("\\x{}", hex::encode(v)))
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),

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
        // PostgreSQL arrays must be decoded using Vec<T> in SQLx, not String
        // IMPORTANT: Always use Option<Vec<T>> to handle NULL arrays properly
        t if t.ends_with("[]") => {
            // Get the base type by stripping the [] suffix
            let base_type = t.trim_end_matches("[]");

            // Normalize base type to uppercase for matching
            let base_type_upper = base_type.to_uppercase();

            // Try type-specific array decoding based on the base type
            // IMPORTANT: Use Vec<Option<T>> to handle arrays containing NULL elements
            // Then wrap in Option<> to handle the whole array being NULL
            match base_type_upper.as_str() {
                // String array types (most common - VARCHAR[], TEXT[], CHAR[], etc.)
                // Note: PostgreSQL reports "character varying" for VARCHAR
                "VARCHAR" | "CHARACTER VARYING" | "TEXT" | "CHAR" | "CHARACTER" | "BPCHAR"
                | "NAME" | "CITEXT" => {
                    // Use Vec<Option<String>> to handle NULL elements within the array
                    match row.try_get::<Option<Vec<Option<String>>>, _>(column_index) {
                        Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                        Ok(None) => Ok("".to_string()),
                        Err(_) => handle_custom_postgresql_type(row, column_index, type_name),
                    }
                }
                // Integer array types
                "INT2" | "SMALLINT" => {
                    match row.try_get::<Option<Vec<Option<i16>>>, _>(column_index) {
                        Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                        Ok(None) => Ok("".to_string()),
                        Err(_) => handle_custom_postgresql_type(row, column_index, type_name),
                    }
                }
                "INT4" | "INTEGER" | "SERIAL" => {
                    match row.try_get::<Option<Vec<Option<i32>>>, _>(column_index) {
                        Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                        Ok(None) => Ok("".to_string()),
                        Err(_) => handle_custom_postgresql_type(row, column_index, type_name),
                    }
                }
                "INT8" | "BIGINT" | "BIGSERIAL" => {
                    match row.try_get::<Option<Vec<Option<i64>>>, _>(column_index) {
                        Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                        Ok(None) => Ok("".to_string()),
                        Err(_) => handle_custom_postgresql_type(row, column_index, type_name),
                    }
                }
                // Float array types
                "FLOAT4" | "REAL" => {
                    match row.try_get::<Option<Vec<Option<f32>>>, _>(column_index) {
                        Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                        Ok(None) => Ok("".to_string()),
                        Err(_) => handle_custom_postgresql_type(row, column_index, type_name),
                    }
                }
                "FLOAT8" | "DOUBLE PRECISION" => {
                    match row.try_get::<Option<Vec<Option<f64>>>, _>(column_index) {
                        Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                        Ok(None) => Ok("".to_string()),
                        Err(_) => handle_custom_postgresql_type(row, column_index, type_name),
                    }
                }
                // Boolean array types
                "BOOL" | "BOOLEAN" => {
                    match row.try_get::<Option<Vec<Option<bool>>>, _>(column_index) {
                        Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                        Ok(None) => Ok("".to_string()),
                        Err(_) => handle_custom_postgresql_type(row, column_index, type_name),
                    }
                }
                // UUID array types
                "UUID" => {
                    match row.try_get::<Option<Vec<Option<sqlx::types::Uuid>>>, _>(column_index) {
                        Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                        Ok(None) => Ok("".to_string()),
                        Err(_) => handle_custom_postgresql_type(row, column_index, type_name),
                    }
                }
                // Numeric/Decimal array types
                "NUMERIC" | "DECIMAL" => {
                    match row.try_get::<Option<Vec<Option<sqlx::types::Decimal>>>, _>(column_index)
                    {
                        Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                        Ok(None) => Ok("".to_string()),
                        // For decimal arrays, try falling back to string array
                        Err(_) => {
                            match row.try_get::<Option<Vec<Option<String>>>, _>(column_index) {
                                Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                                Ok(None) => Ok("".to_string()),
                                Err(_) => {
                                    handle_custom_postgresql_type(row, column_index, type_name)
                                }
                            }
                        }
                    }
                }
                // JSON array types - JSON values can't be null inside arrays (use jsonb null)
                "JSON" | "JSONB" => {
                    match row.try_get::<Option<Vec<Option<serde_json::Value>>>, _>(column_index) {
                        Ok(Some(arr)) => {
                            let formatted: Vec<String> = arr
                                .iter()
                                .map(|v| match v {
                                    Some(val) => val.to_string(),
                                    None => "NULL".to_string(),
                                })
                                .collect();
                            Ok(format_array_as_postgres(&formatted))
                        }
                        Ok(None) => Ok("".to_string()),
                        Err(_) => handle_custom_postgresql_type(row, column_index, type_name),
                    }
                }
                // Timestamp array types
                "TIMESTAMP"
                | "TIMESTAMPTZ"
                | "TIMESTAMP WITH TIME ZONE"
                | "TIMESTAMP WITHOUT TIME ZONE" => {
                    match row.try_get::<Option<Vec<Option<chrono::DateTime<chrono::Utc>>>>, _>(
                        column_index,
                    ) {
                        Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                        Ok(None) => Ok("".to_string()),
                        Err(_) => {
                            // Try NaiveDateTime for timestamp without timezone
                            match row.try_get::<Option<Vec<Option<chrono::NaiveDateTime>>>, _>(
                                column_index,
                            ) {
                                Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                                Ok(None) => Ok("".to_string()),
                                Err(_) => {
                                    // Fallback to string array
                                    match row
                                        .try_get::<Option<Vec<Option<String>>>, _>(column_index)
                                    {
                                        Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                                        Ok(None) => Ok("".to_string()),
                                        Err(_) => handle_custom_postgresql_type(
                                            row,
                                            column_index,
                                            type_name,
                                        ),
                                    }
                                }
                            }
                        }
                    }
                }
                // Date array types
                "DATE" => {
                    match row.try_get::<Option<Vec<Option<chrono::NaiveDate>>>, _>(column_index) {
                        Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                        Ok(None) => Ok("".to_string()),
                        Err(_) => handle_custom_postgresql_type(row, column_index, type_name),
                    }
                }
                // Time array types
                "TIME" | "TIMETZ" => {
                    match row.try_get::<Option<Vec<Option<chrono::NaiveTime>>>, _>(column_index) {
                        Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                        Ok(None) => Ok("".to_string()),
                        Err(_) => {
                            match row.try_get::<Option<Vec<Option<String>>>, _>(column_index) {
                                Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                                Ok(None) => Ok("".to_string()),
                                Err(_) => {
                                    handle_custom_postgresql_type(row, column_index, type_name)
                                }
                            }
                        }
                    }
                }
                // Network types array - use string fallback since IpAddr array isn't directly supported
                "INET" | "CIDR" => {
                    match row.try_get::<Option<Vec<Option<String>>>, _>(column_index) {
                        Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                        Ok(None) => Ok("".to_string()),
                        Err(_) => handle_custom_postgresql_type(row, column_index, type_name),
                    }
                }
                // Bytea array
                "BYTEA" => match row.try_get::<Option<Vec<Option<Vec<u8>>>>, _>(column_index) {
                    Ok(Some(arr)) => {
                        let formatted: Vec<String> = arr
                            .iter()
                            .map(|bytes| match bytes {
                                Some(b) => format!("\\x{}", hex::encode(b)),
                                None => "NULL".to_string(),
                            })
                            .collect();
                        Ok(format_array_as_postgres(&formatted))
                    }
                    Ok(None) => Ok("".to_string()),
                    Err(_) => handle_custom_postgresql_type(row, column_index, type_name),
                },
                // Interval array - use raw value approach since SQLx doesn't have native INTERVAL support
                "INTERVAL" => {
                    // Use handle_custom_postgresql_type for the raw array value
                    handle_custom_postgresql_type(row, column_index, type_name)
                }
                // Default: try string array first, then fallback for unknown array types
                _ => {
                    // Always try Option<Vec<Option<String>>> to handle NULL elements and arrays
                    match row.try_get::<Option<Vec<Option<String>>>, _>(column_index) {
                        Ok(Some(arr)) => Ok(format_option_array_as_postgres(&arr)),
                        Ok(None) => Ok("".to_string()),
                        Err(_) => {
                            // Last resort: try to get raw bytes and format
                            match row.try_get::<Option<Vec<u8>>, _>(column_index) {
                                Ok(Some(bytes)) => {
                                    // Try to interpret as UTF-8 string
                                    match String::from_utf8(bytes.clone()) {
                                        Ok(s) => Ok(s),
                                        Err(_) => Ok(format!("\\x{}", hex::encode(bytes))),
                                    }
                                }
                                Ok(None) => Ok("".to_string()),
                                Err(e) => Err(DatabaseError::QueryError(format!(
                                    "Failed to decode array type '{}': {}",
                                    t, e
                                ))),
                            }
                        }
                    }
                }
            }
        }

        // Geometric types - these are complex, use raw value fallback for safety
        "POINT" | "LINE" | "LSEG" | "BOX" | "PATH" | "POLYGON" | "CIRCLE" => row
            .try_get::<String, _>(column_index)
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),

        // Range types - use raw value fallback for safety
        "INT4RANGE" | "INT8RANGE" | "NUMRANGE" | "TSRANGE" | "TSTZRANGE" | "DATERANGE" => row
            .try_get::<String, _>(column_index)
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),

        // XML type - use raw value fallback for safety
        "XML" => row
            .try_get::<String, _>(column_index)
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),

        // Bit string types - use raw value fallback for safety
        "BIT" | "VARBIT" => row
            .try_get::<String, _>(column_index)
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),

        // Money type - use raw value fallback for safety
        "MONEY" => row
            .try_get::<String, _>(column_index)
            .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name)),

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
                            "Failed to format {type_name_upper} type '{type_name}': {e}"
                        ))
                    }),
                "HALFVEC" => row
                    .try_get::<pgvector::HalfVector, _>(column_index)
                    .map(|v| formatter.format_half(v.as_slice()))
                    .map_err(|e| {
                        DatabaseError::QueryError(format!(
                            "Failed to format {type_name_upper} type '{type_name}': {e}"
                        ))
                    }),
                "SPARSEVEC" => row
                    .try_get::<pgvector::SparseVector, _>(column_index)
                    .map(|v| formatter.format_sparse(v.indices(), v.values()))
                    .map_err(|e| {
                        DatabaseError::QueryError(format!(
                            "Failed to format {type_name_upper} type '{type_name}': {e}"
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

            // Try String first, fallback to raw value extraction
            let raw_value = row
                .try_get::<String, _>(column_index)
                .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name))?;

            // Enhanced PostGIS processing with complex display support
            if raw_value.starts_with("01") || raw_value.starts_with("00") {
                // This looks like WKB (Well-Known Binary) format
                Ok(format!("WKB: {raw_value}"))
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
                .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name))
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
                        "LTREE" => s,                             // Path format is already readable
                        "LQUERY" => format!("Query: {s}"),        // Prefix to indicate it's a query
                        "LTXTQUERY" => format!("TextQuery: {s}"), // Prefix for text query
                        _ => s,
                    }
                })
                .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name))
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
                        format!("Cube: {s}")
                    }
                })
                .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name))
        }

        // Full-text search types - use raw value fallback for safety
        "TSVECTOR" | "TSQUERY" => {
            debug!(
                "[PostgreSQL] Processing full-text search type '{}' (normalized: '{}')",
                type_name, type_name_upper
            );
            row.try_get::<String, _>(column_index)
                .or_else(|_| handle_custom_postgresql_type(row, column_index, type_name))
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

    #[test]
    fn test_interval_formatting() {
        // Test zero interval
        assert_eq!(format_interval_components(0, 0, 0), "00:00:00");

        // Test hours only
        assert_eq!(
            format_interval_components(3600 * 1_000_000, 0, 0),
            "01:00:00"
        );

        // Test minutes and seconds
        assert_eq!(format_interval_components(90 * 1_000_000, 0, 0), "00:01:30");

        // Test days only
        assert_eq!(format_interval_components(0, 5, 0), "5 days");

        // Test single day
        assert_eq!(format_interval_components(0, 1, 0), "1 day");

        // Test months only
        assert_eq!(format_interval_components(0, 0, 3), "3 mons");

        // Test single month
        assert_eq!(format_interval_components(0, 0, 1), "1 mon");

        // Test years
        assert_eq!(format_interval_components(0, 0, 12), "1 year");
        assert_eq!(format_interval_components(0, 0, 24), "2 years");

        // Test years and months
        assert_eq!(format_interval_components(0, 0, 14), "1 year 2 mons");

        // Test complex interval: 1 year 2 months 3 days 04:05:06
        let microseconds = (4 * 3600 + 5 * 60 + 6) * 1_000_000;
        assert_eq!(
            format_interval_components(microseconds, 3, 14),
            "1 year 2 mons 3 days 04:05:06"
        );

        // Test fractional seconds
        let microseconds_with_frac = 1_500_000; // 1.5 seconds
        assert_eq!(
            format_interval_components(microseconds_with_frac, 0, 0),
            "00:00:01.5"
        );

        // Test more complex fractional seconds
        let microseconds_complex = 1_234_567; // 1.234567 seconds
        assert_eq!(
            format_interval_components(microseconds_complex, 0, 0),
            "00:00:01.234567"
        );

        // Test negative values (negative interval)
        // PostgreSQL represents negative intervals with negative components
        assert_eq!(
            format_interval_components(-3600 * 1_000_000, 0, 0),
            "-01:00:00"
        );
    }

    #[test]
    fn test_interval_edge_cases() {
        // Test large interval (like from age() function)
        // Example: 10 hours 23 minutes 45 seconds
        let microseconds = (10 * 3600 + 23 * 60 + 45) * 1_000_000;
        assert_eq!(format_interval_components(microseconds, 0, 0), "10:23:45");

        // Test interval with only time component (common from age())
        let age_microseconds = (2 * 3600 + 15 * 60 + 30) * 1_000_000; // 2h 15m 30s
        assert_eq!(
            format_interval_components(age_microseconds, 0, 0),
            "02:15:30"
        );

        // Test days and time
        let microseconds = (5 * 3600) * 1_000_000; // 5 hours
        assert_eq!(
            format_interval_components(microseconds, 2, 0),
            "2 days 05:00:00"
        );
    }

    #[test]
    fn test_all_builtin_types_have_format_handler() {
        // Verify that every type listed in is_builtin_postgresql_type is recognized
        let all_types = vec![
            // Numeric
            "SMALLINT",
            "INT2",
            "INTEGER",
            "INT4",
            "BIGINT",
            "INT8",
            "DECIMAL",
            "NUMERIC",
            "REAL",
            "FLOAT4",
            "DOUBLE PRECISION",
            "FLOAT8",
            "SMALLSERIAL",
            "SERIAL",
            "BIGSERIAL",
            "SERIAL2",
            "SERIAL4",
            "SERIAL8",
            "MONEY",
            "OID",
            // String
            "CHARACTER VARYING",
            "VARCHAR",
            "CHARACTER",
            "CHAR",
            "BPCHAR",
            "TEXT",
            "NAME",
            // Binary
            "BYTEA",
            // Date/time
            "TIMESTAMP",
            "TIMESTAMPTZ",
            "TIMESTAMP WITH TIME ZONE",
            "TIMESTAMP WITHOUT TIME ZONE",
            "DATE",
            "TIME",
            "TIMETZ",
            "TIME WITH TIME ZONE",
            "TIME WITHOUT TIME ZONE",
            "INTERVAL",
            // Boolean
            "BOOLEAN",
            "BOOL",
            // JSON
            "JSON",
            "JSONB",
            // Network
            "INET",
            "CIDR",
            "MACADDR",
            "MACADDR8",
            // UUID
            "UUID",
            // Geometric
            "POINT",
            "LINE",
            "LSEG",
            "BOX",
            "PATH",
            "POLYGON",
            "CIRCLE",
            // Range
            "INT4RANGE",
            "INT8RANGE",
            "NUMRANGE",
            "TSRANGE",
            "TSTZRANGE",
            "DATERANGE",
            // Bit
            "BIT",
            "VARBIT",
            // XML
            "XML",
            // FTS
            "TSVECTOR",
            "TSQUERY",
            // Extensions
            "VECTOR",
            "HALFVEC",
            "SPARSEVEC",
            "GEOMETRY",
            "GEOGRAPHY",
            "BOX2D",
            "BOX3D",
            "HSTORE",
            "LTREE",
            "LQUERY",
            "LTXTQUERY",
            "CUBE",
            "CITEXT",
        ];

        for type_name in &all_types {
            assert!(
                is_builtin_postgresql_type(type_name),
                "Type '{}' should be recognized as built-in",
                type_name
            );
        }

        // Also verify array variants
        for type_name in &all_types {
            let array_type = format!("{}[]", type_name);
            assert!(
                is_builtin_postgresql_type(&array_type),
                "Array type '{}' should be recognized as built-in",
                array_type
            );
        }
    }

    #[test]
    fn test_format_option_array_edge_cases() {
        // All NULLs
        let all_nulls: Vec<Option<i32>> = vec![None, None, None];
        assert_eq!(
            format_option_array_as_postgres(&all_nulls),
            "{NULL,NULL,NULL}"
        );

        // Mixed NULLs and values
        let mixed: Vec<Option<i32>> = vec![Some(1), None, Some(3)];
        assert_eq!(format_option_array_as_postgres(&mixed), "{1,NULL,3}");

        // Empty array
        let empty: Vec<Option<i32>> = vec![];
        assert_eq!(format_option_array_as_postgres(&empty), "{}");

        // Single NULL
        let single_null: Vec<Option<i32>> = vec![None];
        assert_eq!(format_option_array_as_postgres(&single_null), "{NULL}");

        // String arrays with special characters
        let special: Vec<Option<String>> = vec![
            Some("hello world".to_string()),
            Some("with,comma".to_string()),
            None,
            Some("normal".to_string()),
        ];
        assert_eq!(
            format_option_array_as_postgres(&special),
            "{\"hello world\",\"with,comma\",NULL,normal}"
        );
    }

    #[test]
    fn test_format_array_edge_cases() {
        let empty: Vec<String> = vec![];
        assert_eq!(format_array_as_postgres(&empty), "{}");

        let with_quotes: Vec<String> = vec!["has\"quote".to_string()];
        assert_eq!(format_array_as_postgres(&with_quotes), "{\"has\\\"quote\"}");

        let with_braces: Vec<String> = vec!["{nested}".to_string()];
        assert_eq!(format_array_as_postgres(&with_braces), "{\"{nested}\"}");

        // Single element, no special chars
        let simple: Vec<String> = vec!["hello".to_string()];
        assert_eq!(format_array_as_postgres(&simple), "{hello}");

        // Multiple elements
        let multi: Vec<String> = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(format_array_as_postgres(&multi), "{a,b,c}");

        // Empty string element
        let with_empty: Vec<String> = vec!["".to_string(), "a".to_string()];
        assert_eq!(format_array_as_postgres(&with_empty), "{\"\",a}");

        // Backslash in string
        let with_backslash: Vec<String> = vec!["back\\slash".to_string()];
        assert_eq!(
            format_array_as_postgres(&with_backslash),
            "{\"back\\\\slash\"}"
        );
    }

    #[test]
    fn test_no_hard_error_pattern_in_array_handlers() {
        // Regression guard: verify that the array type handling section
        // does not contain hard error returns that would crash on decode failure.
        // All array type Err arms should use handle_custom_postgresql_type fallback.
        let source = include_str!("database_postgresql.rs");

        // Find the array handling section between "Array types" comment and the closing
        // of the array match block. We look for the pattern within the array match arms.
        let array_section_start = source
            .find("// Array types - handle common array types")
            .expect("Should find array types section");
        let array_section_end = source[array_section_start..]
            .find("// Geometric types")
            .expect("Should find geometric types section");
        let array_section = &source[array_section_start..array_section_start + array_section_end];

        // The pattern "Err(e) => Err(DatabaseError::QueryError(e.to_string()))" should
        // NOT appear in the array handling section (except in the default fallback's
        // raw bytes error which is acceptable as an absolute last resort)
        let hard_error_pattern = "Err(e) => Err(DatabaseError::QueryError(e.to_string()))";
        let occurrences: Vec<_> = array_section.match_indices(hard_error_pattern).collect();

        // The only acceptable occurrence is in the default "_" arm's raw bytes fallback
        // which provides its own detailed error message
        assert!(
            occurrences.is_empty(),
            "Found {} hard error pattern(s) in array handlers that should use \
             handle_custom_postgresql_type fallback instead. \
             Occurrences at byte offsets: {:?}",
            occurrences.len(),
            occurrences.iter().map(|(i, _)| i).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn test_all_postgresql_types_decode_gracefully() {
        // Skip if no database available
        let database_url = match std::env::var("DATABASE_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!(
                    "Skipping test_all_postgresql_types_decode_gracefully: DATABASE_URL not set"
                );
                return;
            }
        };

        let connection_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };

        // Parse the URL to create a proper client
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(&database_url)
            .await;

        let pool = match pool {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Skipping: could not connect to database: {}", e);
                return;
            }
        };

        let metadata_provider = PostgreSQLMetadataProvider::new(pool.clone());
        let client = PostgreSQLClient {
            pool,
            connection_info,
            current_database: "test".to_string(),
            metadata_provider,
        };

        // Comprehensive type query - tests ALL scalar and array types using SELECT with casts
        let sql = r#"
            SELECT
                -- Scalar types
                1::int2 AS small_int,
                1::int4 AS integer_val,
                1::int8 AS big_int,
                1.5::float4 AS real_val,
                1.5::float8 AS double_val,
                1.23::numeric AS numeric_val,
                true::bool AS bool_val,
                'hello'::text AS text_val,
                'c'::char AS char_val,
                '2024-01-01'::date AS date_val,
                '2024-01-01 12:00:00'::timestamp AS timestamp_val,
                now()::timestamptz AS timestamptz_val,
                '12:00:00'::time AS time_val,
                '1 year 2 months'::interval AS interval_val,
                '{"key":"value"}'::json AS json_val,
                '{"key":"value"}'::jsonb AS jsonb_val,
                '192.168.1.1'::inet AS inet_val,
                '192.168.1.0/24'::cidr AS cidr_val,
                gen_random_uuid()::uuid AS uuid_val,
                '\xDEADBEEF'::bytea AS bytea_val,
                '(1,2)'::point AS point_val,
                '[1,10]'::int4range AS int4range_val,
                NULL::int4 AS null_int,
                NULL::text AS null_text,

                -- Array types (main focus)
                ARRAY[1,2,3]::int4[] AS int4_arr,
                ARRAY[1,2]::int2[] AS int2_arr,
                ARRAY[1,2]::int8[] AS int8_arr,
                ARRAY[1.5,2.5]::float4[] AS float4_arr,
                ARRAY[1.5,2.5]::float8[] AS float8_arr,
                ARRAY[true,false]::bool[] AS bool_arr,
                ARRAY['a','b']::text[] AS text_arr,
                ARRAY['a','b']::varchar[] AS varchar_arr,
                ARRAY['2024-01-01'::date]::date[] AS date_arr,
                ARRAY['2024-01-01 12:00:00'::timestamp]::timestamp[] AS timestamp_arr,
                ARRAY[now()]::timestamptz[] AS timestamptz_arr,
                ARRAY['12:00:00'::time]::time[] AS time_arr,
                ARRAY['{"a":1}'::jsonb]::jsonb[] AS jsonb_arr,
                ARRAY[gen_random_uuid()]::uuid[] AS uuid_arr,
                ARRAY[1.23::numeric]::numeric[] AS numeric_arr,
                ARRAY['\xDEAD'::bytea]::bytea[] AS bytea_arr,

                -- Edge cases
                ARRAY[NULL::int4] AS arr_with_null,
                NULL::int4[] AS null_array,
                '{}'::int4[] AS empty_array
        "#;

        let result = client.execute_query(sql).await;

        match result {
            Ok(rows) => {
                assert!(
                    rows.len() >= 2,
                    "Should have header + at least 1 data row, got {} rows",
                    rows.len()
                );

                let headers = &rows[0];
                let data = &rows[1];

                // Verify no column returned the error placeholder
                for (i, value) in data.iter().enumerate() {
                    assert_ne!(
                        value,
                        "?error?",
                        "Column '{}' (index {}) returned error placeholder. \
                         All types should decode gracefully.",
                        headers.get(i).unwrap_or(&format!("col_{}", i)),
                        i
                    );
                }

                // Verify specific known values
                assert_eq!(
                    data[headers.iter().position(|h| h == "small_int").unwrap()],
                    "1"
                );
                assert_eq!(
                    data[headers.iter().position(|h| h == "integer_val").unwrap()],
                    "1"
                );
                assert_eq!(
                    data[headers.iter().position(|h| h == "big_int").unwrap()],
                    "1"
                );
                assert_eq!(
                    data[headers.iter().position(|h| h == "bool_val").unwrap()],
                    "true"
                );
                assert_eq!(
                    data[headers.iter().position(|h| h == "text_val").unwrap()],
                    "hello"
                );

                // Verify NULL values are empty strings
                assert_eq!(
                    data[headers.iter().position(|h| h == "null_int").unwrap()],
                    ""
                );
                assert_eq!(
                    data[headers.iter().position(|h| h == "null_text").unwrap()],
                    ""
                );
                assert_eq!(
                    data[headers.iter().position(|h| h == "null_array").unwrap()],
                    ""
                );

                // Verify empty array
                assert_eq!(
                    data[headers.iter().position(|h| h == "empty_array").unwrap()],
                    "{}"
                );

                // Verify array with NULL element contains "NULL"
                let arr_with_null =
                    &data[headers.iter().position(|h| h == "arr_with_null").unwrap()];
                assert!(
                    arr_with_null.contains("NULL"),
                    "Array with NULL element should contain 'NULL', got: {}",
                    arr_with_null
                );

                eprintln!("All {} columns decoded successfully!", headers.len());
            }
            Err(e) => {
                panic!(
                    "Query should not fail with graceful degradation enabled: {}",
                    e
                );
            }
        }
    }

    #[tokio::test]
    async fn test_array_agg_with_nulls() {
        // Skip if no database available
        let database_url = match std::env::var("DATABASE_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("Skipping test_array_agg_with_nulls: DATABASE_URL not set");
                return;
            }
        };

        let connection_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };

        let pool = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(&database_url)
            .await;

        let pool = match pool {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Skipping: could not connect to database: {}", e);
                return;
            }
        };

        let metadata_provider = PostgreSQLMetadataProvider::new(pool.clone());
        let client = PostgreSQLClient {
            pool,
            connection_info,
            current_database: "test".to_string(),
            metadata_provider,
        };

        // Test ARRAY_AGG over NULL values - this is the exact scenario from the user's bug
        let sql = "SELECT ARRAY_AGG(x) AS agg_result FROM (SELECT NULL::int4 AS x) sub";

        let result = client.execute_query(sql).await;
        match result {
            Ok(rows) => {
                assert!(rows.len() >= 2, "Should have header + data row");
                let value = &rows[1][0];
                assert_ne!(
                    value, "?error?",
                    "ARRAY_AGG over NULL should not produce error placeholder"
                );
                eprintln!("ARRAY_AGG(NULL) result: {}", value);
            }
            Err(e) => {
                panic!("ARRAY_AGG query should not fail: {}", e);
            }
        }

        // Test ARRAY_AGG producing a real integer array
        let sql2 = "SELECT ARRAY_AGG(x) AS agg_ints FROM generate_series(1, 3) AS x";
        let result2 = client.execute_query(sql2).await;
        match result2 {
            Ok(rows) => {
                assert!(rows.len() >= 2);
                let value = &rows[1][0];
                assert_ne!(value, "?error?");
                // Should contain 1, 2, 3 in some array format
                assert!(
                    value.contains('1') && value.contains('2') && value.contains('3'),
                    "ARRAY_AGG should contain 1, 2, 3, got: {}",
                    value
                );
                eprintln!("ARRAY_AGG(1..3) result: {}", value);
            }
            Err(e) => {
                panic!("ARRAY_AGG query should not fail: {}", e);
            }
        }
    }
}
