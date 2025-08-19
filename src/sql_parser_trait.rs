//! Database-specific SQL parser trait system
//! Provides a trait-based approach to SQL parsing that accounts for database-specific syntax

use crate::database::DatabaseType;
use crate::sql_parser::{SqlClause, SqlContext};
use async_trait::async_trait;

/// Database-specific context information that supplements the generic SqlContext
#[derive(Debug, Clone)]
pub enum DatabaseSpecificContext {
    /// PostgreSQL-specific context
    PostgreSQL {
        /// JSON/JSONB operators found in the query
        json_operators: Vec<String>,
        /// Array access patterns
        array_accesses: Vec<String>,
        /// Window function context
        has_window_functions: bool,
        /// CTE (Common Table Expression) context
        has_cte: bool,
    },
    /// MySQL-specific context
    MySQL {
        /// Backtick-quoted identifiers
        backtick_identifiers: Vec<String>,
        /// MySQL-specific operators
        mysql_operators: Vec<String>,
        /// Storage engine hints
        storage_engine_context: bool,
    },
    /// SQLite-specific context
    SQLite {
        /// PRAGMA statements
        pragma_context: Option<String>,
        /// Virtual table context
        virtual_table_context: bool,
        /// WITHOUT ROWID context
        without_rowid_context: bool,
    },
    /// Generic context for unknown database types
    Generic,
}

/// Enhanced SQL context that includes database-specific information
#[derive(Debug, Clone)]
pub struct EnhancedSqlContext {
    /// Base SQL context from generic parser
    pub base_context: SqlContext,
    /// Database-specific context information
    pub database_context: DatabaseSpecificContext,
    /// Database type for this context
    pub database_type: DatabaseType,
}

/// Database-specific keyword categories
#[derive(Debug, Clone, PartialEq)]
pub enum KeywordCategory {
    /// DDL keywords (CREATE, ALTER, DROP)
    DDL,
    /// DML keywords (SELECT, INSERT, UPDATE, DELETE)
    DML,
    /// Functions
    Functions,
    /// Operators
    Operators,
    /// Data types
    DataTypes,
    /// System functions
    SystemFunctions,
    /// Aggregate functions
    AggregateFunctions,
    /// Window functions
    WindowFunctions,
}

/// Database-specific SQL parser engine trait
#[async_trait]
pub trait SqlParserEngine: Send + Sync {
    /// Get the database type this parser handles
    fn database_type(&self) -> DatabaseType;

    /// Parse SQL at cursor position with database-specific logic
    fn parse_at_cursor(&self, sql: &str, cursor_pos: usize) -> EnhancedSqlContext;

    /// Get database-specific keywords by category
    fn get_keywords_by_category(&self, category: KeywordCategory) -> Vec<&'static str>;

    /// Get database-specific functions
    fn get_functions(&self) -> Vec<&'static str>;

    /// Get database-specific operators
    fn get_operators(&self) -> Vec<&'static str>;

    /// Get database-specific data types
    fn get_data_types(&self) -> Vec<&'static str>;

    /// Check if a keyword is valid in the current context
    fn is_keyword_valid_in_context(&self, keyword: &str, context: &EnhancedSqlContext) -> bool;

    /// Get context-specific suggestions
    fn get_context_suggestions(
        &self,
        context: &EnhancedSqlContext,
        current_word: &str,
    ) -> Vec<String>;

    /// Parse database-specific operators at cursor position
    fn parse_operators_at_cursor(&self, sql: &str, cursor_pos: usize) -> Vec<String>;

    /// Check if the parser can handle advanced syntax features
    fn supports_advanced_features(&self) -> bool {
        true
    }

    /// Get database-specific completion hints
    fn get_completion_hints(&self, context: &EnhancedSqlContext) -> Vec<CompletionHint>;
}

/// Completion hint with database-specific information
#[derive(Debug, Clone)]
pub struct CompletionHint {
    /// The suggestion text
    pub text: String,
    /// Description of what this suggestion does
    pub description: String,
    /// Category of this suggestion
    pub category: CompletionHintCategory,
    /// Whether this suggestion requires parentheses
    pub requires_parentheses: bool,
    /// Database-specific priority (higher = more important)
    pub priority: u8,
}

/// Categories for completion hints
#[derive(Debug, Clone, PartialEq)]
pub enum CompletionHintCategory {
    Keyword,
    Function,
    Operator,
    DataType,
    TableName,
    ColumnName,
    SchemaName,
    DatabaseSpecific,
}

/// Parser factory for creating database-specific parsers
pub struct SqlParserFactory;

impl SqlParserFactory {
    /// Create a parser for the specified database type
    pub fn create_parser(database_type: DatabaseType) -> Box<dyn SqlParserEngine> {
        match database_type {
            DatabaseType::PostgreSQL => {
                Box::new(crate::sql_parser_postgresql::PostgreSQLParser::new())
            }
            DatabaseType::MySQL => Box::new(crate::sql_parser_mysql::MySQLParser::new()),
            DatabaseType::SQLite => Box::new(crate::sql_parser_sqlite::SQLiteParser::new()),
            DatabaseType::ClickHouse => {
                // Use PostgreSQL parser for now as ClickHouse SQL is similar
                Box::new(crate::sql_parser_postgresql::PostgreSQLParser::new())
            }
        }
    }

    /// Create a generic parser that falls back to basic SQL parsing
    pub fn create_generic_parser() -> Box<dyn SqlParserEngine> {
        Box::new(GenericSqlParser::new())
    }
}

/// Generic SQL parser that falls back to the existing parser logic
pub struct GenericSqlParser;

impl Default for GenericSqlParser {
    fn default() -> Self {
        Self::new()
    }
}

impl GenericSqlParser {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SqlParserEngine for GenericSqlParser {
    fn database_type(&self) -> DatabaseType {
        // Return PostgreSQL as a reasonable default
        DatabaseType::PostgreSQL
    }

    fn parse_at_cursor(&self, sql: &str, cursor_pos: usize) -> EnhancedSqlContext {
        let base_context = crate::sql_parser::parse_sql_at_cursor(sql, cursor_pos);

        EnhancedSqlContext {
            base_context,
            database_context: DatabaseSpecificContext::Generic,
            database_type: DatabaseType::PostgreSQL,
        }
    }

    fn get_keywords_by_category(&self, category: KeywordCategory) -> Vec<&'static str> {
        match category {
            KeywordCategory::DDL => vec!["CREATE", "ALTER", "DROP"],
            KeywordCategory::DML => vec!["SELECT", "INSERT", "UPDATE", "DELETE"],
            KeywordCategory::Functions => vec!["COUNT", "SUM", "AVG", "MAX", "MIN"],
            KeywordCategory::Operators => vec!["AND", "OR", "NOT", "IN", "EXISTS"],
            KeywordCategory::DataTypes => vec!["TEXT", "INTEGER", "BOOLEAN", "DATE"],
            KeywordCategory::SystemFunctions => vec!["NOW", "CURRENT_DATE", "CURRENT_TIME"],
            KeywordCategory::AggregateFunctions => vec!["COUNT", "SUM", "AVG", "MAX", "MIN"],
            KeywordCategory::WindowFunctions => vec!["ROW_NUMBER", "RANK", "DENSE_RANK"],
        }
    }

    fn get_functions(&self) -> Vec<&'static str> {
        vec![
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
            "NOW",
            "CURRENT_DATE",
            "CURRENT_TIME",
            "CURRENT_TIMESTAMP",
        ]
    }

    fn get_operators(&self) -> Vec<&'static str> {
        vec![
            "=", "!=", "<>", "<", ">", "<=", ">=", "AND", "OR", "NOT", "IN", "LIKE", "BETWEEN",
        ]
    }

    fn get_data_types(&self) -> Vec<&'static str> {
        vec![
            "TEXT",
            "INTEGER",
            "REAL",
            "BOOLEAN",
            "DATE",
            "TIME",
            "TIMESTAMP",
        ]
    }

    fn is_keyword_valid_in_context(&self, _keyword: &str, _context: &EnhancedSqlContext) -> bool {
        true // Generic parser accepts all keywords
    }

    fn get_context_suggestions(
        &self,
        _context: &EnhancedSqlContext,
        _current_word: &str,
    ) -> Vec<String> {
        vec![] // Generic parser doesn't provide specific suggestions
    }

    fn parse_operators_at_cursor(&self, _sql: &str, _cursor_pos: usize) -> Vec<String> {
        vec![] // Generic parser doesn't parse specific operators
    }

    fn get_completion_hints(&self, context: &EnhancedSqlContext) -> Vec<CompletionHint> {
        let mut hints = Vec::new();

        // Add basic hints based on the current clause
        match context.base_context.current_clause {
            SqlClause::Select => {
                hints.push(CompletionHint {
                    text: "*".to_string(),
                    description: "Select all columns".to_string(),
                    category: CompletionHintCategory::Keyword,
                    requires_parentheses: false,
                    priority: 9,
                });
                hints.push(CompletionHint {
                    text: "DISTINCT".to_string(),
                    description: "Select distinct values".to_string(),
                    category: CompletionHintCategory::Keyword,
                    requires_parentheses: false,
                    priority: 8,
                });
            }
            SqlClause::From => {
                hints.push(CompletionHint {
                    text: "JOIN".to_string(),
                    description: "Join tables".to_string(),
                    category: CompletionHintCategory::Keyword,
                    requires_parentheses: false,
                    priority: 8,
                });
            }
            _ => {}
        }

        hints
    }
}

/// Utility functions for database-specific parsing
pub mod parsing_utils {
    use super::*;

    /// Check if a character is a valid identifier character for the database type
    pub fn is_identifier_char(ch: char, database_type: DatabaseType) -> bool {
        match database_type {
            DatabaseType::PostgreSQL => ch.is_alphanumeric() || ch == '_' || ch == '$',
            DatabaseType::MySQL => ch.is_alphanumeric() || ch == '_' || ch == '$',
            DatabaseType::SQLite => ch.is_alphanumeric() || ch == '_',
            DatabaseType::ClickHouse => ch.is_alphanumeric() || ch == '_',
        }
    }

    /// Check if an identifier needs quoting for the database type
    pub fn needs_quoting(identifier: &str, database_type: DatabaseType) -> bool {
        if identifier.is_empty() {
            return true;
        }

        let first_char = identifier.chars().next().unwrap();
        if !first_char.is_alphabetic() && first_char != '_' {
            return true;
        }

        // Check for reserved keywords (simplified check)
        let upper_identifier = identifier.to_uppercase();
        match database_type {
            DatabaseType::PostgreSQL => {
                matches!(
                    upper_identifier.as_str(),
                    "SELECT" | "FROM" | "WHERE" | "ORDER" | "GROUP"
                )
            }
            DatabaseType::MySQL => {
                matches!(
                    upper_identifier.as_str(),
                    "SELECT" | "FROM" | "WHERE" | "ORDER" | "GROUP" | "LIMIT"
                )
            }
            DatabaseType::SQLite => {
                matches!(
                    upper_identifier.as_str(),
                    "SELECT" | "FROM" | "WHERE" | "ORDER" | "GROUP"
                )
            }
            DatabaseType::ClickHouse => {
                matches!(
                    upper_identifier.as_str(),
                    "SELECT" | "FROM" | "WHERE" | "ORDER" | "GROUP" | "LIMIT"
                )
            }
        }
    }

    /// Get the appropriate quote character for identifiers
    pub fn get_quote_char(database_type: DatabaseType) -> char {
        match database_type {
            DatabaseType::PostgreSQL => '"',
            DatabaseType::MySQL => '`',
            DatabaseType::SQLite => '"',
            DatabaseType::ClickHouse => '`', // ClickHouse uses backticks like MySQL
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generic_parser_creation() {
        let parser = GenericSqlParser::new();
        assert_eq!(parser.database_type(), DatabaseType::PostgreSQL);
    }

    #[test]
    fn test_generic_parser_keywords() {
        let parser = GenericSqlParser::new();
        let ddl_keywords = parser.get_keywords_by_category(KeywordCategory::DDL);
        assert!(ddl_keywords.contains(&"CREATE"));
        assert!(ddl_keywords.contains(&"ALTER"));
        assert!(ddl_keywords.contains(&"DROP"));
    }

    #[test]
    fn test_parsing_utils_identifier_char() {
        use parsing_utils::*;

        assert!(is_identifier_char('a', DatabaseType::PostgreSQL));
        assert!(is_identifier_char('_', DatabaseType::PostgreSQL));
        assert!(is_identifier_char('$', DatabaseType::PostgreSQL));
        assert!(!is_identifier_char(' ', DatabaseType::PostgreSQL));

        assert!(is_identifier_char('a', DatabaseType::MySQL));
        assert!(is_identifier_char('_', DatabaseType::MySQL));
        assert!(is_identifier_char('$', DatabaseType::MySQL));

        assert!(is_identifier_char('a', DatabaseType::SQLite));
        assert!(is_identifier_char('_', DatabaseType::SQLite));
        assert!(!is_identifier_char('$', DatabaseType::SQLite));
    }

    #[test]
    fn test_parsing_utils_needs_quoting() {
        use parsing_utils::*;

        assert!(!needs_quoting("table_name", DatabaseType::PostgreSQL));
        assert!(needs_quoting("select", DatabaseType::PostgreSQL));
        assert!(needs_quoting("123table", DatabaseType::PostgreSQL));
    }

    #[test]
    fn test_parsing_utils_quote_char() {
        use parsing_utils::*;

        assert_eq!(get_quote_char(DatabaseType::PostgreSQL), '"');
        assert_eq!(get_quote_char(DatabaseType::MySQL), '`');
        assert_eq!(get_quote_char(DatabaseType::SQLite), '"');
    }
}
