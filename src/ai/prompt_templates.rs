//! Dialect-aware system prompts for AI text-to-SQL generation

use crate::database::{DatabaseType, DatabaseTypeExt};

/// Build the system prompt for text-to-SQL generation
pub fn build_system_prompt(db_type: &DatabaseType, schema_context: &str) -> String {
    let dialect_notes = get_dialect_notes(db_type);

    format!(
        r#"You are a SQL expert assistant for DBCrust, a multi-database CLI tool.
Your task is to generate SQL queries based on natural language descriptions.

RULES:
1. Output ONLY the SQL query — no explanations, no markdown fences, no comments.
2. Use the EXACT table and column names from the schema below.
3. Use the correct SQL dialect for {db_type}.
4. Add LIMIT 100 by default for SELECT queries unless the user specifies a limit.
5. Never generate destructive queries (DROP, TRUNCATE) unless explicitly asked.
6. If the query is ambiguous, make reasonable assumptions and generate the most likely query.
7. For follow-up queries, use context from the conversation to understand references like "those", "that table", "filter further", etc.

{dialect_notes}

DATABASE SCHEMA:
{schema_context}"#,
        db_type = db_type.display_name(),
    )
}

fn get_dialect_notes(db_type: &DatabaseType) -> String {
    match db_type {
        DatabaseType::PostgreSQL => r#"POSTGRESQL DIALECT NOTES:
- Use double quotes for identifiers with special characters
- JSONB operators: ->, ->>, #>, @>, ?, ?|, ?&
- Array operators: @>, <@, &&, ANY(), ALL()
- Use ILIKE for case-insensitive pattern matching
- String concatenation: || operator
- Date functions: NOW(), CURRENT_TIMESTAMP, DATE_TRUNC(), EXTRACT()
- Use ::type for casting (e.g., '2024-01-01'::date)
- Window functions fully supported
- CTEs with WITH clause supported
- LATERAL joins supported"#
            .to_string(),

        DatabaseType::MySQL => r#"MYSQL DIALECT NOTES:
- Use backticks for identifier quoting
- Use LIKE for pattern matching (case sensitivity depends on collation)
- String concatenation: CONCAT() function
- Date functions: NOW(), CURDATE(), DATE_FORMAT(), DATEDIFF()
- Use CAST(x AS type) for type conversion
- JSON operators: ->, ->>, JSON_EXTRACT()
- LIMIT with OFFSET syntax: LIMIT n OFFSET m
- No BOOLEAN type, use TINYINT(1)
- GROUP BY may require all non-aggregated columns"#
            .to_string(),

        DatabaseType::SQLite => r#"SQLITE DIALECT NOTES:
- Minimal type system: TEXT, INTEGER, REAL, BLOB
- Use double quotes for identifiers
- String concatenation: || operator
- Date functions: date(), time(), datetime(), strftime()
- No native BOOLEAN: use 0/1
- LIMIT/OFFSET syntax supported
- No RIGHT JOIN or FULL OUTER JOIN
- CTEs with WITH clause supported
- Use PRAGMA for database info"#
            .to_string(),

        DatabaseType::ClickHouse => r#"CLICKHOUSE DIALECT NOTES:
- Use double quotes or backticks for identifiers
- Strongly typed system with many numeric types
- Use toDate(), toDateTime() for date conversions
- Array functions: arrayJoin(), groupArray(), arrayMap()
- Use FINAL keyword for ReplacingMergeTree queries
- No standard UPDATE/DELETE (use ALTER TABLE ... UPDATE/DELETE)
- Aggregation functions: uniq(), quantile(), topK()
- SAMPLE clause for approximate queries
- Use FORMAT for output format control"#
            .to_string(),

        DatabaseType::MongoDB => r#"MONGODB QUERY NOTES:
- Queries use MongoDB aggregation pipeline syntax
- Use $match, $group, $sort, $project, $limit stages
- Field access: use dot notation for nested fields
- Comparison: $eq, $ne, $gt, $gte, $lt, $lte, $in, $nin
- Logical: $and, $or, $not, $nor
- String: $regex for pattern matching
- Generate aggregation pipelines as JSON arrays"#
            .to_string(),

        DatabaseType::Elasticsearch => r#"ELASTICSEARCH QUERY NOTES:
- Use Elasticsearch SQL syntax
- MATCH() function for full-text search
- LIKE for pattern matching
- Aggregations via GROUP BY
- DESCRIBE for index mapping
- SHOW TABLES for index listing
- Limited JOIN support"#
            .to_string(),

        // DataFusion file formats (Parquet, CSV, JSON, DuckDB)
        _ => r#"DATAFUSION SQL NOTES:
- Standard SQL dialect (PostgreSQL-compatible)
- Aggregate: COUNT, SUM, AVG, MIN, MAX, STDDEV, VAR, MEDIAN
- Window: ROW_NUMBER, RANK, DENSE_RANK, LAG, LEAD
- String: CONCAT, UPPER, LOWER, SUBSTRING, TRIM, REPLACE
- Date: NOW, DATE_TRUNC, EXTRACT, TO_TIMESTAMP
- Array: ARRAY_AGG, ARRAY_LENGTH
- Type conversion: CAST, TRY_CAST
- File names used as table names (dots/dashes → underscores)"#
            .to_string(),
    }
}
