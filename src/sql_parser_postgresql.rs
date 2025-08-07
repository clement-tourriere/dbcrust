//! PostgreSQL-specific SQL parser implementation
//! Handles PostgreSQL-specific syntax, operators, and functions

use crate::database::DatabaseType;
use crate::sql_parser::SqlClause;
use crate::sql_parser_trait::{
    CompletionHint, CompletionHintCategory, DatabaseSpecificContext, EnhancedSqlContext,
    KeywordCategory, SqlParserEngine,
};
use async_trait::async_trait;

/// PostgreSQL-specific SQL parser
pub struct PostgreSQLParser {}

impl Default for PostgreSQLParser {
    fn default() -> Self {
        Self::new()
    }
}

impl PostgreSQLParser {
    pub fn new() -> Self {
        Self {}
    }

    /// Parse PostgreSQL-specific operators at the cursor position
    fn parse_postgresql_operators(&self, sql: &str, cursor_pos: usize) -> Vec<String> {
        let mut operators = Vec::new();

        // Look for PostgreSQL-specific operators around the cursor
        let start = cursor_pos.saturating_sub(10);
        let end = std::cmp::min(cursor_pos + 10, sql.len());
        let context = &sql[start..end];

        // JSON operators
        if context.contains("->") || context.contains("->>") {
            operators.extend_from_slice(&["->".to_string(), "->>".to_string()]);
        }
        if context.contains("#>") || context.contains("#>>") {
            operators.extend_from_slice(&["#>".to_string(), "#>>".to_string()]);
        }
        if context.contains("@>") || context.contains("<@") {
            operators.extend_from_slice(&["@>".to_string(), "<@".to_string()]);
        }
        if context.contains("?") || context.contains("?|") || context.contains("?&") {
            operators.extend_from_slice(&["?".to_string(), "?|".to_string(), "?&".to_string()]);
        }

        // Array operators
        if context.contains("&&") {
            operators.push("&&".to_string());
        }
        if context.contains("||") {
            operators.push("||".to_string());
        }

        // Range operators
        if context.contains("@>") || context.contains("<@") {
            operators.extend_from_slice(&["@>".to_string(), "<@".to_string()]);
        }
        if context.contains("&&") {
            operators.push("&&".to_string());
        }

        // Text search operators
        if context.contains("@@") {
            operators.push("@@".to_string());
        }
        if context.contains("@@@") {
            operators.push("@@@".to_string());
        }

        operators
    }

    /// Detect PostgreSQL-specific syntax patterns
    fn detect_postgresql_patterns(&self, sql: &str) -> DatabaseSpecificContext {
        let mut json_operators = Vec::new();
        let mut array_accesses = Vec::new();

        // Check for JSON operators
        let json_ops = ["->", "->>", "#>", "#>>", "@>", "<@", "?", "?|", "?&"];
        for op in &json_ops {
            if sql.contains(op) {
                json_operators.push(op.to_string());
            }
        }

        // Check for array access patterns
        if sql.contains("[") && sql.contains("]") {
            // Simple heuristic: look for patterns like column[1] or array[1:3]
            let chars = sql.chars().peekable();
            let mut current_word = String::new();

            for ch in chars {
                if ch.is_alphanumeric() || ch == '_' {
                    current_word.push(ch);
                } else if ch == '[' && !current_word.is_empty() {
                    array_accesses.push(format!("{current_word}[...]"));
                    current_word.clear();
                } else {
                    current_word.clear();
                }
            }
        }

        // Check for window functions
        let window_keywords = ["OVER", "PARTITION BY", "ORDER BY", "ROWS", "RANGE"];
        let has_window_functions = window_keywords
            .iter()
            .any(|keyword| sql.to_uppercase().contains(keyword));

        // Check for CTEs
        let has_cte = sql.to_uppercase().contains("WITH")
            && (sql.to_uppercase().contains("AS (") || sql.to_uppercase().contains("AS("));

        DatabaseSpecificContext::PostgreSQL {
            json_operators,
            array_accesses,
            has_window_functions,
            has_cte,
        }
    }

    /// Get PostgreSQL-specific completion hints based on context
    fn get_postgresql_hints(&self, context: &EnhancedSqlContext) -> Vec<CompletionHint> {
        let mut hints = Vec::new();

        // Add hints based on current clause
        match context.base_context.current_clause {
            SqlClause::Select => {
                // JSON functions
                hints.push(CompletionHint {
                    text: "JSON_BUILD_OBJECT(".to_string(),
                    description: "Build a JSON object from key-value pairs".to_string(),
                    category: CompletionHintCategory::Function,
                    requires_parentheses: false,
                    priority: 7,
                });

                hints.push(CompletionHint {
                    text: "ARRAY_AGG(".to_string(),
                    description: "Aggregate values into an array".to_string(),
                    category: CompletionHintCategory::Function,
                    requires_parentheses: false,
                    priority: 8,
                });

                hints.push(CompletionHint {
                    text: "STRING_AGG(".to_string(),
                    description: "Concatenate strings with separator".to_string(),
                    category: CompletionHintCategory::Function,
                    requires_parentheses: false,
                    priority: 8,
                });

                // Window functions
                hints.push(CompletionHint {
                    text: "ROW_NUMBER() OVER (".to_string(),
                    description: "Assign unique row numbers".to_string(),
                    category: CompletionHintCategory::Function,
                    requires_parentheses: false,
                    priority: 7,
                });
            }
            SqlClause::Where => {
                // JSON operators
                hints.push(CompletionHint {
                    text: "->".to_string(),
                    description: "JSON field accessor (returns JSON)".to_string(),
                    category: CompletionHintCategory::Operator,
                    requires_parentheses: false,
                    priority: 8,
                });

                hints.push(CompletionHint {
                    text: "->>".to_string(),
                    description: "JSON field accessor (returns text)".to_string(),
                    category: CompletionHintCategory::Operator,
                    requires_parentheses: false,
                    priority: 8,
                });

                hints.push(CompletionHint {
                    text: "@>".to_string(),
                    description: "JSON/Array contains operator".to_string(),
                    category: CompletionHintCategory::Operator,
                    requires_parentheses: false,
                    priority: 7,
                });

                hints.push(CompletionHint {
                    text: "ILIKE".to_string(),
                    description: "Case-insensitive pattern matching".to_string(),
                    category: CompletionHintCategory::Operator,
                    requires_parentheses: false,
                    priority: 8,
                });
            }
            SqlClause::From => {
                hints.push(CompletionHint {
                    text: "LATERAL".to_string(),
                    description: "Allow reference to columns from preceding tables".to_string(),
                    category: CompletionHintCategory::Keyword,
                    requires_parentheses: false,
                    priority: 6,
                });
            }
            _ => {}
        }

        // Add CTE suggestions if appropriate
        if context.base_context.current_clause == SqlClause::Unknown {
            hints.push(CompletionHint {
                text: "WITH".to_string(),
                description: "Common Table Expression (CTE)".to_string(),
                category: CompletionHintCategory::Keyword,
                requires_parentheses: false,
                priority: 7,
            });
        }

        hints
    }
}

#[async_trait]
impl SqlParserEngine for PostgreSQLParser {
    fn database_type(&self) -> DatabaseType {
        DatabaseType::PostgreSQL
    }

    fn parse_at_cursor(&self, sql: &str, cursor_pos: usize) -> EnhancedSqlContext {
        // Start with the base SQL parsing
        let base_context = crate::sql_parser::parse_sql_at_cursor(sql, cursor_pos);

        // Add PostgreSQL-specific parsing
        let database_context = self.detect_postgresql_patterns(sql);

        EnhancedSqlContext {
            base_context,
            database_context,
            database_type: DatabaseType::PostgreSQL,
        }
    }

    fn get_keywords_by_category(&self, category: KeywordCategory) -> Vec<&'static str> {
        match category {
            KeywordCategory::DDL => vec![
                "CREATE", "ALTER", "DROP", "TRUNCATE", "COMMENT", "GRANT", "REVOKE",
            ],
            KeywordCategory::DML => vec![
                "SELECT",
                "INSERT",
                "UPDATE",
                "DELETE",
                "MERGE",
                "COPY",
                "RETURNING",
            ],
            KeywordCategory::Functions => vec![
                "COALESCE",
                "NULLIF",
                "GREATEST",
                "LEAST",
                "JSON_BUILD_OBJECT",
                "JSON_AGG",
                "ARRAY_AGG",
                "STRING_AGG",
                "UNNEST",
                "GENERATE_SERIES",
            ],
            KeywordCategory::Operators => vec![
                "AND", "OR", "NOT", "IN", "EXISTS", "BETWEEN", "LIKE", "ILIKE", "SIMILAR", "TO",
                "IS", "DISTINCT", "FROM",
            ],
            KeywordCategory::DataTypes => vec![
                "BIGINT",
                "INTEGER",
                "SMALLINT",
                "DECIMAL",
                "NUMERIC",
                "REAL",
                "DOUBLE",
                "PRECISION",
                "SERIAL",
                "BIGSERIAL",
                "MONEY",
                "TEXT",
                "VARCHAR",
                "CHAR",
                "BYTEA",
                "TIMESTAMP",
                "TIMESTAMPTZ",
                "DATE",
                "TIME",
                "TIMETZ",
                "INTERVAL",
                "BOOLEAN",
                "UUID",
                "JSON",
                "JSONB",
                "ARRAY",
                "INET",
                "CIDR",
                "MACADDR",
                "POINT",
                "LINE",
                "BOX",
                "PATH",
                "POLYGON",
                "CIRCLE",
            ],
            KeywordCategory::SystemFunctions => vec![
                "NOW",
                "CURRENT_DATE",
                "CURRENT_TIME",
                "CURRENT_TIMESTAMP",
                "CURRENT_USER",
                "SESSION_USER",
                "USER",
                "CURRENT_CATALOG",
                "CURRENT_SCHEMA",
                "VERSION",
                "PG_BACKEND_PID",
            ],
            KeywordCategory::AggregateFunctions => vec![
                "COUNT",
                "SUM",
                "AVG",
                "MAX",
                "MIN",
                "ARRAY_AGG",
                "STRING_AGG",
                "JSON_AGG",
                "JSONB_AGG",
                "BOOL_AND",
                "BOOL_OR",
                "BIT_AND",
                "BIT_OR",
                "STDDEV",
                "STDDEV_POP",
                "STDDEV_SAMP",
                "VARIANCE",
                "VAR_POP",
                "VAR_SAMP",
            ],
            KeywordCategory::WindowFunctions => vec![
                "ROW_NUMBER",
                "RANK",
                "DENSE_RANK",
                "PERCENT_RANK",
                "CUME_DIST",
                "NTILE",
                "LAG",
                "LEAD",
                "FIRST_VALUE",
                "LAST_VALUE",
                "NTH_VALUE",
            ],
        }
    }

    fn get_functions(&self) -> Vec<&'static str> {
        vec![
            // Standard SQL functions
            "COUNT",
            "SUM",
            "AVG",
            "MAX",
            "MIN",
            "UPPER",
            "LOWER",
            "LENGTH",
            "TRIM",
            "SUBSTR",
            "SUBSTRING",
            "REPLACE",
            "CONCAT",
            "ABS",
            "ROUND",
            "CEIL",
            "FLOOR",
            "COALESCE",
            "NULLIF",
            "GREATEST",
            "LEAST",
            // PostgreSQL-specific functions
            "STRING_AGG",
            "ARRAY_AGG",
            "UNNEST",
            "GENERATE_SERIES",
            "JSON_BUILD_OBJECT",
            "JSON_BUILD_ARRAY",
            "JSON_AGG",
            "JSONB_AGG",
            "JSONB_BUILD_OBJECT",
            "JSONB_BUILD_ARRAY",
            "JSON_EXTRACT_PATH",
            "JSON_EXTRACT_PATH_TEXT",
            "JSONB_EXTRACT_PATH",
            "JSONB_EXTRACT_PATH_TEXT",
            "AGE",
            "EXTRACT",
            "DATE_PART",
            "DATE_TRUNC",
            "TO_CHAR",
            "TO_DATE",
            "TO_TIMESTAMP",
            "TO_NUMBER",
            "FORMAT",
            "LEFT",
            "RIGHT",
            "REVERSE",
            "TRANSLATE",
            "OVERLAY",
            "REGEXP_REPLACE",
            "REGEXP_SPLIT_TO_ARRAY",
            "ARRAY_APPEND",
            "ARRAY_PREPEND",
            "ARRAY_CAT",
            "ARRAY_LENGTH",
            "ARRAY_POSITION",
            "ARRAY_REMOVE",
            "ARRAY_REPLACE",
        ]
    }

    fn get_operators(&self) -> Vec<&'static str> {
        vec![
            // Standard operators
            "=",
            "!=",
            "<>",
            "<",
            ">",
            "<=",
            ">=",
            "AND",
            "OR",
            "NOT",
            "IN",
            "LIKE",
            "BETWEEN",
            "IS",
            "NULL",
            // PostgreSQL-specific operators
            "->",
            "->>",
            "#>",
            "#>>",
            "@>",
            "<@",
            "?",
            "?|",
            "?&",
            "||",
            "&&",
            "@@",
            "@@@",
            "ILIKE",
            "SIMILAR TO",
            "~",
            "~*",
            "!~",
            "!~*",
            "<<",
            ">>",
            "&<",
            "&>",
            "<<|",
            "|>>",
            "<->",
            "@@",
            "~=",
            "@",
            "##",
        ]
    }

    fn get_data_types(&self) -> Vec<&'static str> {
        vec![
            "BIGINT",
            "INTEGER",
            "SMALLINT",
            "DECIMAL",
            "NUMERIC",
            "REAL",
            "DOUBLE PRECISION",
            "SERIAL",
            "BIGSERIAL",
            "SMALLSERIAL",
            "MONEY",
            "TEXT",
            "VARCHAR",
            "CHAR",
            "BYTEA",
            "TIMESTAMP",
            "TIMESTAMPTZ",
            "DATE",
            "TIME",
            "TIMETZ",
            "INTERVAL",
            "BOOLEAN",
            "UUID",
            "JSON",
            "JSONB",
            "ARRAY",
            "INET",
            "CIDR",
            "MACADDR",
            "MACADDR8",
            "POINT",
            "LINE",
            "LSEG",
            "BOX",
            "PATH",
            "POLYGON",
            "CIRCLE",
            "BIT",
            "VARBIT",
            "TSVECTOR",
            "TSQUERY",
            "XML",
            "PG_LSN",
            "TXID_SNAPSHOT",
        ]
    }

    fn is_keyword_valid_in_context(&self, keyword: &str, context: &EnhancedSqlContext) -> bool {
        let upper_keyword = keyword.to_uppercase();

        // Context-specific validation
        match context.base_context.current_clause {
            SqlClause::Select => {
                // In SELECT clause, allow column functions and operators
                matches!(
                    upper_keyword.as_str(),
                    "DISTINCT"
                        | "ALL"
                        | "*"
                        | "AS"
                        | "FROM"
                        | "COUNT"
                        | "SUM"
                        | "AVG"
                        | "MAX"
                        | "MIN"
                        | "STRING_AGG"
                        | "ARRAY_AGG"
                        | "JSON_AGG"
                        | "JSONB_AGG"
                        | "ROW_NUMBER"
                        | "RANK"
                        | "DENSE_RANK"
                        | "OVER"
                )
            }
            SqlClause::From => {
                // In FROM clause, allow table-related keywords
                matches!(
                    upper_keyword.as_str(),
                    "JOIN"
                        | "INNER"
                        | "LEFT"
                        | "RIGHT"
                        | "FULL"
                        | "OUTER"
                        | "CROSS"
                        | "LATERAL"
                        | "ON"
                        | "USING"
                        | "WHERE"
                        | "GROUP"
                        | "ORDER"
                        | "LIMIT"
                        | "OFFSET"
                        | "UNION"
                        | "INTERSECT"
                        | "EXCEPT"
                )
            }
            SqlClause::Where => {
                // In WHERE clause, allow conditional operators
                matches!(
                    upper_keyword.as_str(),
                    "AND"
                        | "OR"
                        | "NOT"
                        | "IN"
                        | "EXISTS"
                        | "BETWEEN"
                        | "LIKE"
                        | "ILIKE"
                        | "SIMILAR"
                        | "TO"
                        | "IS"
                        | "NULL"
                        | "DISTINCT"
                        | "FROM"
                        | "ANY"
                        | "SOME"
                        | "ALL"
                        | "GROUP"
                        | "ORDER"
                        | "LIMIT"
                        | "OFFSET"
                )
            }
            _ => true, // Allow all keywords in other contexts
        }
    }

    fn get_context_suggestions(
        &self,
        context: &EnhancedSqlContext,
        current_word: &str,
    ) -> Vec<String> {
        let mut suggestions = Vec::new();
        let lower_word = current_word.to_lowercase();

        // Get base keywords for the current clause
        let keywords = match context.base_context.current_clause {
            SqlClause::Select => self.get_keywords_by_category(KeywordCategory::Functions),
            SqlClause::Where => self.get_keywords_by_category(KeywordCategory::Operators),
            _ => vec![],
        };

        // Filter keywords that start with the current word
        for keyword in keywords {
            if keyword.to_lowercase().starts_with(&lower_word) {
                suggestions.push(keyword.to_string());
            }
        }

        // Add PostgreSQL-specific operators if appropriate
        if context.base_context.current_clause == SqlClause::Where {
            let operators = self.get_operators();
            for op in operators {
                if op.to_lowercase().starts_with(&lower_word) {
                    suggestions.push(op.to_string());
                }
            }
        }

        suggestions
    }

    fn parse_operators_at_cursor(&self, sql: &str, cursor_pos: usize) -> Vec<String> {
        self.parse_postgresql_operators(sql, cursor_pos)
    }

    fn get_completion_hints(&self, context: &EnhancedSqlContext) -> Vec<CompletionHint> {
        let mut hints = self.get_postgresql_hints(context);

        // Add database-specific hints based on context
        if let DatabaseSpecificContext::PostgreSQL {
            json_operators,
            has_window_functions,
            has_cte,
            ..
        } = &context.database_context
        {
            // If JSON operators are present, suggest more JSON functions
            if !json_operators.is_empty() {
                hints.push(CompletionHint {
                    text: "JSON_TYPEOF(".to_string(),
                    description: "Get JSON value type".to_string(),
                    category: CompletionHintCategory::Function,
                    requires_parentheses: false,
                    priority: 6,
                });
            }

            // If window functions are present, suggest window-related keywords
            if *has_window_functions {
                hints.push(CompletionHint {
                    text: "PARTITION BY".to_string(),
                    description: "Partition window function".to_string(),
                    category: CompletionHintCategory::Keyword,
                    requires_parentheses: false,
                    priority: 8,
                });
            }

            // If CTEs are present, suggest RECURSIVE
            if *has_cte {
                hints.push(CompletionHint {
                    text: "RECURSIVE".to_string(),
                    description: "Recursive Common Table Expression".to_string(),
                    category: CompletionHintCategory::Keyword,
                    requires_parentheses: false,
                    priority: 7,
                });
            }
        }

        hints
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_postgresql_parser_creation() {
        let parser = PostgreSQLParser::new();
        assert_eq!(parser.database_type(), DatabaseType::PostgreSQL);
        // Test that we can create the parser successfully
        let functions = parser.get_functions();
        assert!(functions.contains(&"JSON_BUILD_OBJECT"));
    }

    #[test]
    fn test_postgresql_json_operator_detection() {
        let parser = PostgreSQLParser::new();
        let operators = parser.parse_postgresql_operators("SELECT data -> 'key' FROM table", 12);
        assert!(operators.contains(&"->".to_string()));
    }

    #[test]
    fn test_postgresql_pattern_detection() {
        let parser = PostgreSQLParser::new();
        let sql = "WITH cte AS (SELECT json_data ->> 'name' FROM users) SELECT * FROM cte";
        let context = parser.detect_postgresql_patterns(sql);

        if let DatabaseSpecificContext::PostgreSQL {
            json_operators,
            has_cte,
            ..
        } = context
        {
            assert!(!json_operators.is_empty());
            assert!(has_cte);
        } else {
            panic!("Expected PostgreSQL context");
        }
    }

    #[test]
    fn test_postgresql_keywords_by_category() {
        let parser = PostgreSQLParser::new();
        let functions = parser.get_keywords_by_category(KeywordCategory::Functions);
        assert!(functions.contains(&"JSON_BUILD_OBJECT"));
        assert!(functions.contains(&"ARRAY_AGG"));
        assert!(functions.contains(&"STRING_AGG"));
    }

    #[test]
    fn test_postgresql_window_function_detection() {
        let parser = PostgreSQLParser::new();
        let sql = "SELECT ROW_NUMBER() OVER (PARTITION BY dept_id ORDER BY salary) FROM employees";
        let context = parser.detect_postgresql_patterns(sql);

        if let DatabaseSpecificContext::PostgreSQL {
            has_window_functions,
            ..
        } = context
        {
            assert!(has_window_functions);
        } else {
            panic!("Expected PostgreSQL context");
        }
    }
}
