//! SQLite-specific SQL parser implementation
//! Handles SQLite-specific syntax, operators, and functions

use crate::database::DatabaseType;
use crate::sql_parser::{SqlClause, StatementType};
use crate::sql_parser_trait::{
    SqlParserEngine, EnhancedSqlContext, DatabaseSpecificContext, KeywordCategory, 
    CompletionHint, CompletionHintCategory
};
use async_trait::async_trait;

/// SQLite-specific SQL parser
pub struct SQLiteParser {}

impl SQLiteParser {
    pub fn new() -> Self {
        Self {}
    }
    
    /// Parse SQLite-specific operators at the cursor position
    fn parse_sqlite_operators(&self, sql: &str, cursor_pos: usize) -> Vec<String> {
        let mut operators = Vec::new();
        
        // Look for SQLite-specific operators around the cursor
        let start = cursor_pos.saturating_sub(10);
        let end = std::cmp::min(cursor_pos + 10, sql.len());
        let context = &sql[start..end];
        
        // SQLite-specific operators
        if context.to_uppercase().contains("GLOB") {
            operators.push("GLOB".to_string());
        }
        if context.to_uppercase().contains("MATCH") {
            operators.push("MATCH".to_string());
        }
        if context.to_uppercase().contains("REGEXP") {
            operators.push("REGEXP".to_string());
        }
        
        // JSON operators (SQLite 3.45.0+)
        if context.contains("->") || context.contains("->>") {
            operators.extend_from_slice(&["->".to_string(), "->>".to_string()]);
        }
        
        // Concatenation operator
        if context.contains("||") {
            operators.push("||".to_string());
        }
        
        operators
    }
    
    /// Detect SQLite-specific syntax patterns
    fn detect_sqlite_patterns(&self, sql: &str) -> DatabaseSpecificContext {
        let mut pragma_context = None;
        let virtual_table_context;
        let without_rowid_context;
        
        let upper_sql = sql.to_uppercase();
        
        // Check for PRAGMA statements
        if upper_sql.contains("PRAGMA") {
            // Extract the PRAGMA statement
            if let Some(pragma_start) = upper_sql.find("PRAGMA") {
                let pragma_part = &sql[pragma_start..];
                if let Some(pragma_end) = pragma_part.find(';') {
                    pragma_context = Some(pragma_part[..pragma_end].to_string());
                } else {
                    pragma_context = Some(pragma_part.to_string());
                }
            }
        }
        
        // Check for virtual table context
        virtual_table_context = upper_sql.contains("VIRTUAL TABLE") || 
                               upper_sql.contains("USING FTS") ||
                               upper_sql.contains("USING RTREE");
        
        // Check for WITHOUT ROWID context
        without_rowid_context = upper_sql.contains("WITHOUT ROWID");
        
        DatabaseSpecificContext::SQLite {
            pragma_context,
            virtual_table_context,
            without_rowid_context,
        }
    }
    
    /// Get SQLite-specific completion hints based on context
    fn get_sqlite_hints(&self, context: &EnhancedSqlContext) -> Vec<CompletionHint> {
        let mut hints = Vec::new();
        
        // Add hints based on current clause
        match context.base_context.current_clause {
            SqlClause::Select => {
                // SQLite-specific functions
                hints.push(CompletionHint {
                    text: "GROUP_CONCAT(".to_string(),
                    description: "Concatenate values from multiple rows".to_string(),
                    category: CompletionHintCategory::Function,
                    requires_parentheses: false,
                    priority: 8,
                });
                
                hints.push(CompletionHint {
                    text: "IFNULL(".to_string(),
                    description: "Return alternative value if NULL".to_string(),
                    category: CompletionHintCategory::Function,
                    requires_parentheses: false,
                    priority: 8,
                });
                
                hints.push(CompletionHint {
                    text: "TYPEOF(".to_string(),
                    description: "Get the type of a value".to_string(),
                    category: CompletionHintCategory::Function,
                    requires_parentheses: false,
                    priority: 7,
                });
                
                // Date functions
                hints.push(CompletionHint {
                    text: "DATETIME(".to_string(),
                    description: "Format date and time".to_string(),
                    category: CompletionHintCategory::Function,
                    requires_parentheses: false,
                    priority: 7,
                });
                
                hints.push(CompletionHint {
                    text: "STRFTIME(".to_string(),
                    description: "Format date with custom format string".to_string(),
                    category: CompletionHintCategory::Function,
                    requires_parentheses: false,
                    priority: 7,
                });
                
                // JSON functions
                hints.push(CompletionHint {
                    text: "JSON_EXTRACT(".to_string(),
                    description: "Extract JSON value".to_string(),
                    category: CompletionHintCategory::Function,
                    requires_parentheses: false,
                    priority: 6,
                });
            }
            SqlClause::Where => {
                // SQLite-specific operators
                hints.push(CompletionHint {
                    text: "GLOB".to_string(),
                    description: "Pattern matching with wildcards (* and ?)".to_string(),
                    category: CompletionHintCategory::Operator,
                    requires_parentheses: false,
                    priority: 7,
                });
                
                hints.push(CompletionHint {
                    text: "MATCH".to_string(),
                    description: "Full-text search matching".to_string(),
                    category: CompletionHintCategory::Operator,
                    requires_parentheses: false,
                    priority: 6,
                });
                
                hints.push(CompletionHint {
                    text: "REGEXP".to_string(),
                    description: "Regular expression matching".to_string(),
                    category: CompletionHintCategory::Operator,
                    requires_parentheses: false,
                    priority: 6,
                });
            }
            _ => {}
        }
        
        // Add PRAGMA suggestions if appropriate
        if context.base_context.current_clause == SqlClause::Unknown {
            hints.push(CompletionHint {
                text: "PRAGMA".to_string(),
                description: "SQLite configuration command".to_string(),
                category: CompletionHintCategory::DatabaseSpecific,
                requires_parentheses: false,
                priority: 6,
            });
        }
        
        // Add WITHOUT ROWID for CREATE TABLE
        if context.base_context.statement_type == StatementType::CreateTable {
            hints.push(CompletionHint {
                text: "WITHOUT ROWID".to_string(),
                description: "Create table without implicit rowid column".to_string(),
                category: CompletionHintCategory::DatabaseSpecific,
                requires_parentheses: false,
                priority: 6,
            });
        }
        
        hints
    }
}

#[async_trait]
impl SqlParserEngine for SQLiteParser {
    fn database_type(&self) -> DatabaseType {
        DatabaseType::SQLite
    }
    
    fn parse_at_cursor(&self, sql: &str, cursor_pos: usize) -> EnhancedSqlContext {
        // Start with the base SQL parsing
        let base_context = crate::sql_parser::parse_sql_at_cursor(sql, cursor_pos);
        
        // Add SQLite-specific parsing
        let database_context = self.detect_sqlite_patterns(sql);
        
        EnhancedSqlContext {
            base_context,
            database_context,
            database_type: DatabaseType::SQLite,
        }
    }
    
    fn get_keywords_by_category(&self, category: KeywordCategory) -> Vec<&'static str> {
        match category {
            KeywordCategory::DDL => vec![
                "CREATE", "ALTER", "DROP", "PRAGMA", "VACUUM", "ANALYZE", "REINDEX"
            ],
            KeywordCategory::DML => vec![
                "SELECT", "INSERT", "UPDATE", "DELETE", "REPLACE", "ATTACH", "DETACH"
            ],
            KeywordCategory::Functions => vec![
                "ABS", "COALESCE", "IFNULL", "LENGTH", "LOWER", "UPPER", "TRIM",
                "SUBSTR", "SUBSTRING", "REPLACE", "ROUND", "TYPEOF", "PRINTF",
                "QUOTE", "RANDOM", "RANDOMBLOB", "ZEROBLOB", "HEX", "UNHEX"
            ],
            KeywordCategory::Operators => vec![
                "AND", "OR", "NOT", "IN", "EXISTS", "BETWEEN", "LIKE", "GLOB",
                "MATCH", "REGEXP", "IS", "NULL", "ISNULL", "NOTNULL"
            ],
            KeywordCategory::DataTypes => vec![
                "INTEGER", "REAL", "TEXT", "BLOB", "NUMERIC", "BOOLEAN", "DATE", "DATETIME"
            ],
            KeywordCategory::SystemFunctions => vec![
                "CHANGES", "LAST_INSERT_ROWID", "SQLITE_VERSION", "SQLITE_SOURCE_ID",
                "SQLITE_COMPILEOPTION_GET", "SQLITE_COMPILEOPTION_USED", "TOTAL_CHANGES"
            ],
            KeywordCategory::AggregateFunctions => vec![
                "COUNT", "SUM", "AVG", "MAX", "MIN", "GROUP_CONCAT", "TOTAL"
            ],
            KeywordCategory::WindowFunctions => vec![
                "ROW_NUMBER", "RANK", "DENSE_RANK", "PERCENT_RANK", "CUME_DIST",
                "NTILE", "LAG", "LEAD", "FIRST_VALUE", "LAST_VALUE", "NTH_VALUE"
            ],
        }
    }
    
    fn get_functions(&self) -> Vec<&'static str> {
        vec![
            // Core functions
            "ABS", "CHANGES", "CHAR", "COALESCE", "GLOB", "HEX", "IFNULL", "INSTR",
            "LENGTH", "LIKE", "LOAD_EXTENSION", "LOWER", "LTRIM", "MAX", "MIN",
            "NULLIF", "PRINTF", "QUOTE", "RANDOM", "RANDOMBLOB", "REPLACE", "ROUND",
            "RTRIM", "SOUNDEX", "SQLITE_COMPILEOPTION_GET", "SQLITE_COMPILEOPTION_USED",
            "SQLITE_SOURCE_ID", "SQLITE_VERSION", "SUBSTR", "SUBSTRING", "TOTAL_CHANGES",
            "TRIM", "TYPEOF", "UNICODE", "UPPER", "ZEROBLOB",
            
            // Date/time functions
            "DATE", "TIME", "DATETIME", "JULIANDAY", "STRFTIME", "UNIXEPOCH",
            
            // Mathematical functions
            "ACOS", "ACOSH", "ASIN", "ASINH", "ATAN", "ATAN2", "ATANH", "CEIL",
            "CEILING", "COS", "COSH", "DEGREES", "EXP", "FLOOR", "LN", "LOG",
            "LOG10", "LOG2", "MOD", "PI", "POW", "POWER", "RADIANS", "SIGN",
            "SIN", "SINH", "SQRT", "TAN", "TANH", "TRUNC",
            
            // JSON functions
            "JSON", "JSON_ARRAY", "JSON_ARRAY_LENGTH", "JSON_EXTRACT", "JSON_INSERT",
            "JSON_OBJECT", "JSON_PATCH", "JSON_REMOVE", "JSON_REPLACE", "JSON_SET",
            "JSON_TYPE", "JSON_VALID", "JSON_QUOTE", "JSON_GROUP_ARRAY", "JSON_GROUP_OBJECT",
            
            // Aggregate functions
            "AVG", "COUNT", "GROUP_CONCAT", "MAX", "MIN", "SUM", "TOTAL",
        ]
    }
    
    fn get_operators(&self) -> Vec<&'static str> {
        vec![
            // Standard operators
            "=", "==", "!=", "<>", "<", ">", "<=", ">=", "AND", "OR", "NOT",
            "IN", "LIKE", "BETWEEN", "IS", "NULL", "ISNULL", "NOTNULL",
            
            // SQLite-specific operators
            "GLOB", "MATCH", "REGEXP", "||", "->", "->>",
        ]
    }
    
    fn get_data_types(&self) -> Vec<&'static str> {
        vec![
            "INTEGER", "REAL", "TEXT", "BLOB", "NUMERIC", "BOOLEAN", "DATE", "DATETIME"
        ]
    }
    
    fn is_keyword_valid_in_context(&self, keyword: &str, context: &EnhancedSqlContext) -> bool {
        let upper_keyword = keyword.to_uppercase();
        
        // Context-specific validation
        match context.base_context.current_clause {
            SqlClause::Select => {
                // In SELECT clause, allow column functions and operators
                matches!(upper_keyword.as_str(),
                    "DISTINCT" | "ALL" | "*" | "AS" | "FROM" |
                    "COUNT" | "SUM" | "AVG" | "MAX" | "MIN" |
                    "GROUP_CONCAT" | "IFNULL" | "COALESCE" | "TYPEOF" |
                    "LENGTH" | "SUBSTR" | "UPPER" | "LOWER" | "TRIM"
                )
            }
            SqlClause::From => {
                // In FROM clause, allow table-related keywords
                matches!(upper_keyword.as_str(),
                    "JOIN" | "INNER" | "LEFT" | "RIGHT" | "FULL" | "OUTER" |
                    "CROSS" | "ON" | "USING" | "WHERE" | "GROUP" |
                    "ORDER" | "LIMIT" | "OFFSET" | "UNION" | "INTERSECT" | "EXCEPT"
                )
            }
            SqlClause::Where => {
                // In WHERE clause, allow conditional operators
                matches!(upper_keyword.as_str(),
                    "AND" | "OR" | "NOT" | "IN" | "EXISTS" | "BETWEEN" |
                    "LIKE" | "GLOB" | "MATCH" | "REGEXP" | "IS" | "NULL" |
                    "ISNULL" | "NOTNULL" | "GROUP" | "ORDER" | "LIMIT" | "OFFSET"
                )
            }
            _ => true, // Allow all keywords in other contexts
        }
    }
    
    fn get_context_suggestions(&self, context: &EnhancedSqlContext, current_word: &str) -> Vec<String> {
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
        
        // Add SQLite-specific operators if appropriate
        if context.base_context.current_clause == SqlClause::Where {
            let operators = self.get_operators();
            for op in operators {
                if op.to_lowercase().starts_with(&lower_word) {
                    suggestions.push(op.to_string());
                }
            }
        }
        
        // Add PRAGMA suggestions if starting with "PRAGMA"
        if lower_word.starts_with("pragma") {
            let pragma_keywords = [
                "PRAGMA table_info", "PRAGMA index_info", "PRAGMA foreign_key_list",
                "PRAGMA table_list", "PRAGMA database_list", "PRAGMA schema_version",
                "PRAGMA user_version", "PRAGMA journal_mode", "PRAGMA synchronous",
                "PRAGMA cache_size", "PRAGMA temp_store", "PRAGMA locking_mode",
                "PRAGMA auto_vacuum", "PRAGMA integrity_check", "PRAGMA quick_check",
            ];
            
            for pragma in &pragma_keywords {
                if pragma.to_lowercase().starts_with(&lower_word) {
                    suggestions.push(pragma.to_string());
                }
            }
        }
        
        suggestions
    }
    
    fn parse_operators_at_cursor(&self, sql: &str, cursor_pos: usize) -> Vec<String> {
        self.parse_sqlite_operators(sql, cursor_pos)
    }
    
    fn get_completion_hints(&self, context: &EnhancedSqlContext) -> Vec<CompletionHint> {
        let mut hints = self.get_sqlite_hints(context);
        
        // Add database-specific hints based on context
        if let DatabaseSpecificContext::SQLite { 
            pragma_context, 
            virtual_table_context, 
            without_rowid_context 
        } = &context.database_context {
            
            // If PRAGMA context is present, suggest common PRAGMA commands
            if pragma_context.is_some() {
                hints.push(CompletionHint {
                    text: "table_info(".to_string(),
                    description: "Get table structure information".to_string(),
                    category: CompletionHintCategory::DatabaseSpecific,
                    requires_parentheses: false,
                    priority: 8,
                });
                
                hints.push(CompletionHint {
                    text: "foreign_key_list(".to_string(),
                    description: "List foreign keys for a table".to_string(),
                    category: CompletionHintCategory::DatabaseSpecific,
                    requires_parentheses: false,
                    priority: 7,
                });
            }
            
            // If virtual table context is present, suggest FTS operators
            if *virtual_table_context {
                hints.push(CompletionHint {
                    text: "MATCH".to_string(),
                    description: "Full-text search operator".to_string(),
                    category: CompletionHintCategory::Operator,
                    requires_parentheses: false,
                    priority: 9,
                });
            }
            
            // If WITHOUT ROWID context is present, suggest related keywords
            if *without_rowid_context {
                hints.push(CompletionHint {
                    text: "PRIMARY KEY".to_string(),
                    description: "Required for WITHOUT ROWID tables".to_string(),
                    category: CompletionHintCategory::Keyword,
                    requires_parentheses: false,
                    priority: 9,
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
    fn test_sqlite_parser_creation() {
        let parser = SQLiteParser::new();
        assert_eq!(parser.database_type(), DatabaseType::SQLite);
        // Test that we can create the parser successfully
        let ddl_keywords = parser.get_keywords_by_category(KeywordCategory::DDL);
        assert!(ddl_keywords.contains(&"PRAGMA"));
    }
    
    #[test]
    fn test_sqlite_operator_detection() {
        let parser = SQLiteParser::new();
        let operators = parser.parse_sqlite_operators("SELECT * FROM table WHERE name GLOB 'test*'", 35);
        assert!(operators.contains(&"GLOB".to_string()));
    }
    
    #[test]
    fn test_sqlite_pattern_detection() {
        let parser = SQLiteParser::new();
        let sql = "PRAGMA table_info(users); CREATE VIRTUAL TABLE docs USING fts5(content);";
        let context = parser.detect_sqlite_patterns(sql);
        
        if let DatabaseSpecificContext::SQLite { 
            pragma_context, 
            virtual_table_context, 
            .. 
        } = context {
            assert!(pragma_context.is_some());
            assert!(virtual_table_context);
        } else {
            panic!("Expected SQLite context");
        }
    }
    
    #[test]
    fn test_sqlite_keywords_by_category() {
        let parser = SQLiteParser::new();
        let functions = parser.get_keywords_by_category(KeywordCategory::Functions);
        assert!(functions.contains(&"IFNULL"));
        assert!(functions.contains(&"TYPEOF"));
        assert!(functions.contains(&"PRINTF"));
    }
    
    #[test]
    fn test_sqlite_without_rowid_detection() {
        let parser = SQLiteParser::new();
        let sql = "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT) WITHOUT ROWID";
        let context = parser.detect_sqlite_patterns(sql);
        
        if let DatabaseSpecificContext::SQLite { without_rowid_context, .. } = context {
            assert!(without_rowid_context);
        } else {
            panic!("Expected SQLite context");
        }
    }
    
    #[test]
    fn test_sqlite_pragma_suggestions() {
        let parser = SQLiteParser::new();
        let context = EnhancedSqlContext {
            base_context: crate::sql_parser::parse_sql_at_cursor("PRAGMA ", 7),
            database_context: DatabaseSpecificContext::SQLite {
                pragma_context: Some("PRAGMA ".to_string()),
                virtual_table_context: false,
                without_rowid_context: false,
            },
            database_type: DatabaseType::SQLite,
        };
        
        let suggestions = parser.get_context_suggestions(&context, "pragma t");
        assert!(suggestions.iter().any(|s| s.contains("table_info")));
    }
}