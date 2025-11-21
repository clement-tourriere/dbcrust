//! SQL dialect providers for database-specific SQL generation
//!
//! Each database system has different SQL syntax, functions, and capabilities.
//! This module provides trait-based abstractions for generating database-specific SQL.

use crate::ai_sql::schema::SchemaContext;
use crate::database::{DatabaseType, DatabaseTypeExt};

/// SQL features supported by a database
#[derive(Debug, Clone)]
pub struct SqlFeatures {
    pub supports_cte: bool,
    pub supports_window_functions: bool,
    pub supports_json_operators: bool,
    pub supports_arrays: bool,
    pub supports_full_text_search: bool,
    pub supports_recursive_cte: bool,
    pub supports_lateral_join: bool,
    pub supports_materialized_views: bool,
    pub max_identifier_length: usize,
    pub case_sensitive_identifiers: bool,
    pub requires_table_aliases_in_subqueries: bool,
}

impl Default for SqlFeatures {
    fn default() -> Self {
        Self {
            supports_cte: false,
            supports_window_functions: false,
            supports_json_operators: false,
            supports_arrays: false,
            supports_full_text_search: false,
            supports_recursive_cte: false,
            supports_lateral_join: false,
            supports_materialized_views: false,
            max_identifier_length: 63,
            case_sensitive_identifiers: false,
            requires_table_aliases_in_subqueries: false,
        }
    }
}

/// Trait for database-specific SQL dialect handling
pub trait SqlDialectProvider: Send + Sync {
    /// Get the database type this dialect handles
    fn database_type(&self) -> DatabaseType;

    /// Get the display name for this dialect
    fn dialect_name(&self) -> &str;

    /// Generate system prompt for AI (teaches the AI about this SQL dialect)
    fn system_prompt(&self) -> String;

    /// Format schema context for this database (how to present tables/columns to AI)
    fn format_schema_context(&self, schema: &SchemaContext) -> String;

    /// Get SQL features supported by this database
    fn features(&self) -> SqlFeatures;

    /// Get available functions and operators as a formatted string for AI context
    fn available_functions(&self) -> String;

    /// Validate SQL syntax for this database (basic validation)
    fn validate_sql(&self, sql: &str) -> Result<(), String>;

    /// Get query optimization hints specific to this database
    fn optimization_hints(&self) -> Vec<String>;

    /// Get example queries that demonstrate this database's SQL dialect
    fn example_queries(&self) -> Vec<(String, String)>; // (description, sql)

    /// Get common query patterns for this database
    fn common_patterns(&self) -> Vec<(String, String)>; // (pattern name, template)

    /// Format a CREATE TABLE statement for schema understanding
    fn format_create_table(&self, table_name: &str, columns: &[(String, String)]) -> String;

    /// Quote identifier (table name, column name) according to database rules
    fn quote_identifier(&self, identifier: &str) -> String;

    /// Get the date/time function for "now"
    fn now_function(&self) -> &str;

    /// Get the string concatenation operator or function
    fn concat_operator(&self) -> &str;

    /// Get limit clause syntax
    fn limit_clause(&self, limit: u32) -> String;
}

/// PostgreSQL dialect implementation
pub struct PostgreSQLDialect;

impl SqlDialectProvider for PostgreSQLDialect {
    fn database_type(&self) -> DatabaseType {
        DatabaseType::PostgreSQL
    }

    fn dialect_name(&self) -> &str {
        "PostgreSQL"
    }

    fn system_prompt(&self) -> String {
        r#"You are an expert PostgreSQL SQL query generator. Generate efficient, idiomatic PostgreSQL queries.

IMPORTANT RULES:
1. Generate ONLY the SQL query - no explanations, no markdown, no additional text
2. Do not wrap the query in markdown code blocks (no ```sql or ```)
3. The query should be ready to execute directly
4. Use PostgreSQL-specific features when appropriate

PostgreSQL SQL Features Available:
- Common Table Expressions (CTEs) with WITH clause
- Recursive CTEs for hierarchical queries
- Window functions: ROW_NUMBER(), RANK(), DENSE_RANK(), LAG(), LEAD(), FIRST_VALUE(), LAST_VALUE()
- JSON/JSONB operators: ->, ->>, #>, #>>, @>, <@, ?, ?&, ?|
- JSON functions: json_build_object(), jsonb_agg(), jsonb_build_object()
- Array functions: array_agg(), unnest(), array_length(), ANY(), ALL()
- String aggregation: string_agg(expression, delimiter)
- Full-text search: to_tsvector(), to_tsquery(), plainto_tsquery(), @@
- Advanced date/time: date_trunc(), extract(), age(), now(), INTERVAL
- LATERAL joins for correlated subqueries
- DISTINCT ON for first-row-per-group queries
- RETURNING clause for INSERT/UPDATE/DELETE
- Pattern matching: LIKE, ILIKE, SIMILAR TO, ~ (regex)
- Case-insensitive collations with ILIKE
- Generate series: generate_series()

Date/Time Functions:
- NOW() or CURRENT_TIMESTAMP for current date/time
- date_trunc('month', timestamp) for truncating to month
- INTERVAL '1 day', INTERVAL '3 months' for date arithmetic
- EXTRACT(field FROM timestamp) for extracting parts

Best Practices:
1. Use CTEs for complex queries to improve readability
2. Prefer window functions over self-joins when possible
3. Use EXPLAIN ANALYZE for performance analysis
4. Index recommendations: mention in comments if query would benefit from indexes
5. Use parameterized queries for user input (placeholders: $1, $2, etc.)
6. Quote identifiers with double quotes only when necessary (mixed case, reserved words)
7. Use table aliases for clarity in joins
8. Prefer INNER JOIN to implicit joins in WHERE clause

Common Patterns:
- Running totals: SUM(amount) OVER (ORDER BY date)
- Ranking: ROW_NUMBER() OVER (PARTITION BY category ORDER BY score DESC)
- Pagination: OFFSET n LIMIT m
- Upsert: INSERT ... ON CONFLICT DO UPDATE

Example PostgreSQL Query:
SELECT u.name, u.email, COUNT(o.id) as order_count
FROM users u
LEFT JOIN orders o ON u.id = o.user_id
WHERE u.created_at >= NOW() - INTERVAL '1 year'
GROUP BY u.id, u.name, u.email
ORDER BY order_count DESC
LIMIT 10;

Remember: Output ONLY the SQL query, nothing else."#.to_string()
    }

    fn format_schema_context(&self, schema: &SchemaContext) -> String {
        let mut context = format!(
            "Database: {} (PostgreSQL)\nCurrent Schema: {}\n\n",
            schema.current_database,
            schema.current_schema.as_deref().unwrap_or("public")
        );

        context.push_str("Available Tables:\n");
        for table in &schema.tables {
            context.push_str(&format!("\nTable: {}\n", table.name));
            if let Some(schema_name) = &table.schema {
                context.push_str(&format!("  Schema: {}\n", schema_name));
            }
            context.push_str("  Columns:\n");
            for col in &table.columns {
                let mut col_info = format!("    - {} ({})", col.name, col.data_type);
                if col.is_primary_key {
                    col_info.push_str(" PRIMARY KEY");
                }
                if !col.nullable {
                    col_info.push_str(" NOT NULL");
                }
                if let Some((ref_table, ref_col)) = &col.references {
                    col_info.push_str(&format!(" REFERENCES {}.{}", ref_table, ref_col));
                }
                context.push_str(&format!("{}\n", col_info));
            }

            if !table.indexes.is_empty() {
                context.push_str("  Indexes:\n");
                for idx in &table.indexes {
                    context.push_str(&format!("    - {}\n", idx));
                }
            }
        }

        if !schema.relationships.is_empty() {
            context.push_str("\nForeign Key Relationships:\n");
            for rel in &schema.relationships {
                context.push_str(&format!(
                    "  {}.{} -> {}.{}\n",
                    rel.from_table, rel.from_column, rel.to_table, rel.to_column
                ));
            }
        }

        context
    }

    fn features(&self) -> SqlFeatures {
        SqlFeatures {
            supports_cte: true,
            supports_window_functions: true,
            supports_json_operators: true,
            supports_arrays: true,
            supports_full_text_search: true,
            supports_recursive_cte: true,
            supports_lateral_join: true,
            supports_materialized_views: true,
            max_identifier_length: 63,
            case_sensitive_identifiers: false,
            requires_table_aliases_in_subqueries: false,
        }
    }

    fn available_functions(&self) -> String {
        let functions = DatabaseType::PostgreSQL.sql_functions();
        format!(
            "PostgreSQL Functions:\n{}",
            functions
                .iter()
                .map(|f| format!("  - {}", f))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }

    fn validate_sql(&self, sql: &str) -> Result<(), String> {
        let sql_upper = sql.to_uppercase();

        // Basic validation - check for dangerous operations
        if sql_upper.contains("DROP TABLE") || sql_upper.contains("DROP DATABASE") {
            return Err("DROP statements are not allowed".to_string());
        }

        if sql_upper.contains("TRUNCATE") {
            return Err("TRUNCATE statements are not allowed".to_string());
        }

        // Check for basic syntax
        if sql.trim().is_empty() {
            return Err("Empty SQL query".to_string());
        }

        Ok(())
    }

    fn optimization_hints(&self) -> Vec<String> {
        vec![
            "Use indexes on columns in WHERE, JOIN, and ORDER BY clauses".to_string(),
            "Consider using partial indexes for filtered queries".to_string(),
            "Use EXPLAIN ANALYZE to understand query performance".to_string(),
            "Window functions are more efficient than self-joins".to_string(),
            "CTEs can help the planner optimize complex queries".to_string(),
            "Use LIMIT to restrict result sets in development".to_string(),
        ]
    }

    fn example_queries(&self) -> Vec<(String, String)> {
        vec![
            (
                "Top N per group using window function".to_string(),
                "SELECT * FROM (SELECT *, ROW_NUMBER() OVER (PARTITION BY category ORDER BY score DESC) as rn FROM products) t WHERE rn <= 5".to_string(),
            ),
            (
                "Running total with window function".to_string(),
                "SELECT date, amount, SUM(amount) OVER (ORDER BY date) as running_total FROM transactions".to_string(),
            ),
            (
                "Aggregation with JSON output".to_string(),
                "SELECT category, jsonb_agg(jsonb_build_object('name', name, 'price', price)) as products FROM items GROUP BY category".to_string(),
            ),
            (
                "Date range filtering".to_string(),
                "SELECT * FROM orders WHERE created_at >= NOW() - INTERVAL '30 days'".to_string(),
            ),
        ]
    }

    fn common_patterns(&self) -> Vec<(String, String)> {
        vec![
            ("Pagination".to_string(), "SELECT * FROM {table} ORDER BY {column} LIMIT {limit} OFFSET {offset}".to_string()),
            ("Top N".to_string(), "SELECT * FROM {table} ORDER BY {column} DESC LIMIT {n}".to_string()),
            ("Date filtering".to_string(), "WHERE {date_column} >= NOW() - INTERVAL '{n} days'".to_string()),
            ("Count distinct".to_string(), "SELECT COUNT(DISTINCT {column}) FROM {table}".to_string()),
        ]
    }

    fn format_create_table(&self, table_name: &str, columns: &[(String, String)]) -> String {
        let mut ddl = format!("CREATE TABLE {} (\n", self.quote_identifier(table_name));
        let col_defs: Vec<String> = columns
            .iter()
            .map(|(name, typ)| format!("  {} {}", self.quote_identifier(name), typ))
            .collect();
        ddl.push_str(&col_defs.join(",\n"));
        ddl.push_str("\n);");
        ddl
    }

    fn quote_identifier(&self, identifier: &str) -> String {
        // Only quote if necessary (contains special chars, reserved words, or mixed case)
        if identifier.contains(char::is_uppercase)
            || identifier.contains('-')
            || identifier.contains(' ')
        {
            format!("\"{}\"", identifier.replace('"', "\"\""))
        } else {
            identifier.to_string()
        }
    }

    fn now_function(&self) -> &str {
        "NOW()"
    }

    fn concat_operator(&self) -> &str {
        "||"
    }

    fn limit_clause(&self, limit: u32) -> String {
        format!("LIMIT {}", limit)
    }
}

/// MySQL dialect implementation
pub struct MySQLDialect;

impl SqlDialectProvider for MySQLDialect {
    fn database_type(&self) -> DatabaseType {
        DatabaseType::MySQL
    }

    fn dialect_name(&self) -> &str {
        "MySQL"
    }

    fn system_prompt(&self) -> String {
        r#"You are an expert MySQL SQL query generator. Generate efficient, idiomatic MySQL queries.

IMPORTANT RULES:
1. Generate ONLY the SQL query - no explanations, no markdown, no additional text
2. Do not wrap the query in markdown code blocks (no ```sql or ```)
3. The query should be ready to execute directly
4. Use MySQL-specific features when appropriate

MySQL SQL Features Available:
- Window functions (MySQL 8.0+): ROW_NUMBER(), RANK(), DENSE_RANK(), LAG(), LEAD()
- CTEs with WITH clause (MySQL 8.0+)
- JSON functions: JSON_OBJECT(), JSON_ARRAY(), JSON_EXTRACT(), ->>, ->
- String functions: CONCAT(), GROUP_CONCAT(), SUBSTRING()
- Date/time functions: NOW(), CURRENT_DATE(), DATE_FORMAT(), STR_TO_DATE()
- FROM_UNIXTIME() for timestamp conversion
- Aggregate functions: COUNT(), SUM(), AVG(), MIN(), MAX()

Date/Time Functions:
- NOW() or CURRENT_TIMESTAMP for current date/time
- DATE_FORMAT(date, format) for formatting dates
- STR_TO_DATE(string, format) for parsing dates
- INTERVAL for date arithmetic: DATE_SUB(NOW(), INTERVAL 30 DAY)

Best Practices:
1. Use backticks for identifiers with special characters or reserved words
2. Prefer INNER JOIN to implicit joins in WHERE clause
3. Use prepared statements for user input (placeholders: ?)
4. Index columns used in WHERE, JOIN, and ORDER BY
5. Use LIMIT for pagination and result restriction

Common Patterns:
- Pagination: LIMIT offset, count
- Top N: ORDER BY column DESC LIMIT n
- Date filtering: WHERE date_column >= DATE_SUB(NOW(), INTERVAL 30 DAY)

Example MySQL Query:
SELECT u.name, u.email, COUNT(o.id) as order_count
FROM users u
LEFT JOIN orders o ON u.id = o.user_id
WHERE u.created_at >= DATE_SUB(NOW(), INTERVAL 1 YEAR)
GROUP BY u.id, u.name, u.email
ORDER BY order_count DESC
LIMIT 10;

Remember: Output ONLY the SQL query, nothing else."#.to_string()
    }

    fn format_schema_context(&self, schema: &SchemaContext) -> String {
        let mut context = format!("Database: {} (MySQL)\n\n", schema.current_database);

        context.push_str("Available Tables:\n");
        for table in &schema.tables {
            context.push_str(&format!("\nTable: `{}`\n", table.name));
            context.push_str("  Columns:\n");
            for col in &table.columns {
                let mut col_info = format!("    - {} ({})", col.name, col.data_type);
                if col.is_primary_key {
                    col_info.push_str(" PRIMARY KEY");
                }
                if !col.nullable {
                    col_info.push_str(" NOT NULL");
                }
                context.push_str(&format!("{}\n", col_info));
            }
        }

        context
    }

    fn features(&self) -> SqlFeatures {
        SqlFeatures {
            supports_cte: true,              // MySQL 8.0+
            supports_window_functions: true, // MySQL 8.0+
            supports_json_operators: true,
            supports_arrays: false,
            supports_full_text_search: true,
            supports_recursive_cte: true, // MySQL 8.0+
            supports_lateral_join: false,
            supports_materialized_views: false,
            max_identifier_length: 64,
            case_sensitive_identifiers: false, // Depends on config, but generally no
            requires_table_aliases_in_subqueries: true,
        }
    }

    fn available_functions(&self) -> String {
        let functions = DatabaseType::MySQL.sql_functions();
        format!(
            "MySQL Functions:\n{}",
            functions
                .iter()
                .map(|f| format!("  - {}", f))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }

    fn validate_sql(&self, sql: &str) -> Result<(), String> {
        let sql_upper = sql.to_uppercase();

        if sql_upper.contains("DROP TABLE") || sql_upper.contains("DROP DATABASE") {
            return Err("DROP statements are not allowed".to_string());
        }

        if sql.trim().is_empty() {
            return Err("Empty SQL query".to_string());
        }

        Ok(())
    }

    fn optimization_hints(&self) -> Vec<String> {
        vec![
            "Use indexes on columns in WHERE, JOIN, and ORDER BY clauses".to_string(),
            "Consider covering indexes to avoid table lookups".to_string(),
            "Use EXPLAIN to understand query execution plan".to_string(),
        ]
    }

    fn example_queries(&self) -> Vec<(String, String)> {
        vec![
            (
                "Top N with LIMIT".to_string(),
                "SELECT * FROM products ORDER BY price DESC LIMIT 10".to_string(),
            ),
            (
                "Date filtering".to_string(),
                "SELECT * FROM orders WHERE created_at >= DATE_SUB(NOW(), INTERVAL 30 DAY)"
                    .to_string(),
            ),
        ]
    }

    fn common_patterns(&self) -> Vec<(String, String)> {
        vec![
            (
                "Pagination".to_string(),
                "SELECT * FROM {table} ORDER BY {column} LIMIT {offset}, {limit}".to_string(),
            ),
            (
                "Top N".to_string(),
                "SELECT * FROM {table} ORDER BY {column} DESC LIMIT {n}".to_string(),
            ),
        ]
    }

    fn format_create_table(&self, table_name: &str, columns: &[(String, String)]) -> String {
        let mut ddl = format!("CREATE TABLE `{}` (\n", table_name);
        let col_defs: Vec<String> = columns
            .iter()
            .map(|(name, typ)| format!("  `{}` {}", name, typ))
            .collect();
        ddl.push_str(&col_defs.join(",\n"));
        ddl.push_str("\n);");
        ddl
    }

    fn quote_identifier(&self, identifier: &str) -> String {
        format!("`{}`", identifier.replace('`', "``"))
    }

    fn now_function(&self) -> &str {
        "NOW()"
    }

    fn concat_operator(&self) -> &str {
        "CONCAT"
    }

    fn limit_clause(&self, limit: u32) -> String {
        format!("LIMIT {}", limit)
    }
}

/// Factory function to create appropriate dialect provider
pub fn create_dialect_provider(db_type: DatabaseType) -> Box<dyn SqlDialectProvider> {
    match db_type {
        DatabaseType::PostgreSQL => Box::new(PostgreSQLDialect),
        DatabaseType::MySQL => Box::new(MySQLDialect),
        // TODO: Add more dialect implementations
        _ => Box::new(PostgreSQLDialect), // Fallback to PostgreSQL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_postgresql_features() {
        let dialect = PostgreSQLDialect;
        let features = dialect.features();

        assert!(features.supports_cte);
        assert!(features.supports_window_functions);
        assert!(features.supports_json_operators);
        assert!(features.supports_arrays);
    }

    #[test]
    fn test_postgresql_quote_identifier() {
        let dialect = PostgreSQLDialect;

        assert_eq!(dialect.quote_identifier("users"), "users");
        assert_eq!(dialect.quote_identifier("Users"), "\"Users\"");
        assert_eq!(dialect.quote_identifier("user-name"), "\"user-name\"");
    }

    #[test]
    fn test_mysql_quote_identifier() {
        let dialect = MySQLDialect;

        assert_eq!(dialect.quote_identifier("users"), "`users`");
        assert_eq!(dialect.quote_identifier("user-name"), "`user-name`");
    }

    #[test]
    fn test_sql_validation() {
        let dialect = PostgreSQLDialect;

        assert!(dialect.validate_sql("SELECT * FROM users").is_ok());
        assert!(dialect.validate_sql("DROP TABLE users").is_err());
        assert!(dialect.validate_sql("").is_err());
    }
}
