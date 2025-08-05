//! MySQL-specific SQL parser implementation
//! Handles MySQL-specific syntax, operators, and functions

use crate::database::DatabaseType;
use crate::sql_parser::{SqlClause, StatementType};
use crate::sql_parser_trait::{
    SqlParserEngine, EnhancedSqlContext, DatabaseSpecificContext, KeywordCategory, 
    CompletionHint, CompletionHintCategory
};
use async_trait::async_trait;

/// MySQL-specific SQL parser
pub struct MySQLParser {}

impl MySQLParser {
    pub fn new() -> Self {
        Self {}
    }
    
    /// Parse MySQL-specific operators at the cursor position
    fn parse_mysql_operators(&self, sql: &str, cursor_pos: usize) -> Vec<String> {
        let mut operators = Vec::new();
        
        // Look for MySQL-specific operators around the cursor
        let start = cursor_pos.saturating_sub(10);
        let end = std::cmp::min(cursor_pos + 10, sql.len());
        let context = &sql[start..end];
        
        // MySQL-specific operators
        if context.contains("<=>") {
            operators.push("<=>".to_string()); // NULL-safe equal operator
        }
        if context.contains("<<") || context.contains(">>") {
            operators.extend_from_slice(&["<<".to_string(), ">>".to_string()]); // Bit shift operators
        }
        if context.contains("&") || context.contains("|") || context.contains("^") {
            operators.extend_from_slice(&["&".to_string(), "|".to_string(), "^".to_string()]); // Bitwise operators
        }
        if context.contains("REGEXP") || context.contains("RLIKE") {
            operators.extend_from_slice(&["REGEXP".to_string(), "RLIKE".to_string()]);
        }
        if context.contains("NOT REGEXP") || context.contains("NOT RLIKE") {
            operators.extend_from_slice(&["NOT REGEXP".to_string(), "NOT RLIKE".to_string()]);
        }
        if context.contains("SOUNDS LIKE") {
            operators.push("SOUNDS LIKE".to_string());
        }
        
        // JSON operators (MySQL 5.7+)
        if context.contains("->") || context.contains("->>") {
            operators.extend_from_slice(&["->".to_string(), "->>".to_string()]);
        }
        
        operators
    }
    
    /// Detect MySQL-specific syntax patterns
    fn detect_mysql_patterns(&self, sql: &str) -> DatabaseSpecificContext {
        let mut backtick_identifiers = Vec::new();
        let mut mysql_operators = Vec::new();
        let storage_engine_context;
        
        // Check for backtick-quoted identifiers
        let mut chars = sql.chars().peekable();
        let mut in_backticks = false;
        let mut current_identifier = String::new();
        
        while let Some(ch) = chars.next() {
            if ch == '`' {
                if in_backticks {
                    // End of backtick identifier
                    if !current_identifier.is_empty() {
                        backtick_identifiers.push(format!("`{}`", current_identifier));
                        current_identifier.clear();
                    }
                    in_backticks = false;
                } else {
                    // Start of backtick identifier
                    in_backticks = true;
                }
            } else if in_backticks {
                current_identifier.push(ch);
            }
        }
        
        // Check for MySQL-specific operators
        let mysql_ops = ["<=>", "<<", ">>", "&", "|", "^", "REGEXP", "RLIKE"];
        for op in &mysql_ops {
            if sql.to_uppercase().contains(op) {
                mysql_operators.push(op.to_string());
            }
        }
        
        // Check for storage engine context
        storage_engine_context = sql.to_uppercase().contains("ENGINE=") || 
                                sql.to_uppercase().contains("TYPE=");
        
        DatabaseSpecificContext::MySQL {
            backtick_identifiers,
            mysql_operators,
            storage_engine_context,
        }
    }
    
    /// Get MySQL-specific completion hints based on context
    fn get_mysql_hints(&self, context: &EnhancedSqlContext) -> Vec<CompletionHint> {
        let mut hints = Vec::new();
        
        // Add hints based on current clause
        match context.base_context.current_clause {
            SqlClause::Select => {
                // MySQL-specific functions
                hints.push(CompletionHint {
                    text: "GROUP_CONCAT(".to_string(),
                    description: "Concatenate values from multiple rows".to_string(),
                    category: CompletionHintCategory::Function,
                    requires_parentheses: false,
                    priority: 8,
                });
                
                hints.push(CompletionHint {
                    text: "IF(".to_string(),
                    description: "Conditional expression".to_string(),
                    category: CompletionHintCategory::Function,
                    requires_parentheses: false,
                    priority: 7,
                });
                
                hints.push(CompletionHint {
                    text: "IFNULL(".to_string(),
                    description: "Return alternative value if NULL".to_string(),
                    category: CompletionHintCategory::Function,
                    requires_parentheses: false,
                    priority: 8,
                });
                
                // JSON functions
                hints.push(CompletionHint {
                    text: "JSON_EXTRACT(".to_string(),
                    description: "Extract JSON value".to_string(),
                    category: CompletionHintCategory::Function,
                    requires_parentheses: false,
                    priority: 7,
                });
                
                // Date functions
                hints.push(CompletionHint {
                    text: "FROM_UNIXTIME(".to_string(),
                    description: "Convert Unix timestamp to datetime".to_string(),
                    category: CompletionHintCategory::Function,
                    requires_parentheses: false,
                    priority: 7,
                });
            }
            SqlClause::Where => {
                // MySQL-specific operators
                hints.push(CompletionHint {
                    text: "<=>".to_string(),
                    description: "NULL-safe equal operator".to_string(),
                    category: CompletionHintCategory::Operator,
                    requires_parentheses: false,
                    priority: 7,
                });
                
                hints.push(CompletionHint {
                    text: "REGEXP".to_string(),
                    description: "Regular expression pattern matching".to_string(),
                    category: CompletionHintCategory::Operator,
                    requires_parentheses: false,
                    priority: 8,
                });
                
                hints.push(CompletionHint {
                    text: "RLIKE".to_string(),
                    description: "Regular expression pattern matching (synonym for REGEXP)".to_string(),
                    category: CompletionHintCategory::Operator,
                    requires_parentheses: false,
                    priority: 7,
                });
                
                hints.push(CompletionHint {
                    text: "SOUNDS LIKE".to_string(),
                    description: "Phonetic similarity comparison".to_string(),
                    category: CompletionHintCategory::Operator,
                    requires_parentheses: false,
                    priority: 6,
                });
            }
            SqlClause::Insert => {
                hints.push(CompletionHint {
                    text: "ON DUPLICATE KEY UPDATE".to_string(),
                    description: "Handle duplicate key conflicts".to_string(),
                    category: CompletionHintCategory::Keyword,
                    requires_parentheses: false,
                    priority: 8,
                });
                
                hints.push(CompletionHint {
                    text: "IGNORE".to_string(),
                    description: "Ignore duplicate key errors".to_string(),
                    category: CompletionHintCategory::Keyword,
                    requires_parentheses: false,
                    priority: 7,
                });
            }
            _ => {}
        }
        
        // Add storage engine hints if in CREATE TABLE context
        if context.base_context.statement_type == StatementType::CreateTable {
            hints.push(CompletionHint {
                text: "ENGINE=InnoDB".to_string(),
                description: "InnoDB storage engine".to_string(),
                category: CompletionHintCategory::DatabaseSpecific,
                requires_parentheses: false,
                priority: 8,
            });
            
            hints.push(CompletionHint {
                text: "ENGINE=MyISAM".to_string(),
                description: "MyISAM storage engine".to_string(),
                category: CompletionHintCategory::DatabaseSpecific,
                requires_parentheses: false,
                priority: 7,
            });
        }
        
        hints
    }
}

#[async_trait]
impl SqlParserEngine for MySQLParser {
    fn database_type(&self) -> DatabaseType {
        DatabaseType::MySQL
    }
    
    fn parse_at_cursor(&self, sql: &str, cursor_pos: usize) -> EnhancedSqlContext {
        // Start with the base SQL parsing
        let base_context = crate::sql_parser::parse_sql_at_cursor(sql, cursor_pos);
        
        // Add MySQL-specific parsing
        let database_context = self.detect_mysql_patterns(sql);
        
        EnhancedSqlContext {
            base_context,
            database_context,
            database_type: DatabaseType::MySQL,
        }
    }
    
    fn get_keywords_by_category(&self, category: KeywordCategory) -> Vec<&'static str> {
        match category {
            KeywordCategory::DDL => vec![
                "CREATE", "ALTER", "DROP", "TRUNCATE", "RENAME", "COMMENT"
            ],
            KeywordCategory::DML => vec![
                "SELECT", "INSERT", "UPDATE", "DELETE", "REPLACE", "LOAD", "DATA", "INFILE"
            ],
            KeywordCategory::Functions => vec![
                "IFNULL", "IF", "COALESCE", "NULLIF", "GREATEST", "LEAST",
                "CONCAT", "CONCAT_WS", "GROUP_CONCAT", "SUBSTRING", "LEFT", "RIGHT",
                "UPPER", "LOWER", "TRIM", "LENGTH", "LOCATE", "REPLACE"
            ],
            KeywordCategory::Operators => vec![
                "AND", "OR", "NOT", "IN", "EXISTS", "BETWEEN", "LIKE", "REGEXP",
                "RLIKE", "IS", "NULL", "SOUNDS", "LIKE"
            ],
            KeywordCategory::DataTypes => vec![
                "TINYINT", "SMALLINT", "MEDIUMINT", "INT", "INTEGER", "BIGINT",
                "DECIMAL", "NUMERIC", "FLOAT", "DOUBLE", "REAL", "BIT", "BOOLEAN",
                "DATE", "DATETIME", "TIMESTAMP", "TIME", "YEAR", "CHAR", "VARCHAR",
                "BINARY", "VARBINARY", "TINYBLOB", "BLOB", "MEDIUMBLOB", "LONGBLOB",
                "TINYTEXT", "TEXT", "MEDIUMTEXT", "LONGTEXT", "ENUM", "SET", "JSON"
            ],
            KeywordCategory::SystemFunctions => vec![
                "NOW", "CURDATE", "CURTIME", "CURRENT_DATE", "CURRENT_TIME",
                "CURRENT_TIMESTAMP", "DATABASE", "USER", "VERSION", "CONNECTION_ID",
                "LAST_INSERT_ID", "FOUND_ROWS", "ROW_COUNT"
            ],
            KeywordCategory::AggregateFunctions => vec![
                "COUNT", "SUM", "AVG", "MAX", "MIN", "GROUP_CONCAT", "BIT_AND",
                "BIT_OR", "BIT_XOR", "STD", "STDDEV", "STDDEV_POP", "STDDEV_SAMP",
                "VAR_POP", "VAR_SAMP", "VARIANCE"
            ],
            KeywordCategory::WindowFunctions => vec![
                "ROW_NUMBER", "RANK", "DENSE_RANK", "PERCENT_RANK", "CUME_DIST",
                "NTILE", "LAG", "LEAD", "FIRST_VALUE", "LAST_VALUE", "NTH_VALUE"
            ],
        }
    }
    
    fn get_functions(&self) -> Vec<&'static str> {
        vec![
            // Standard SQL functions
            "COUNT", "SUM", "AVG", "MAX", "MIN", "UPPER", "LOWER", "LENGTH",
            "TRIM", "SUBSTR", "SUBSTRING", "REPLACE", "CONCAT", "ABS", "ROUND",
            "CEIL", "CEILING", "FLOOR", "COALESCE", "NULLIF", "GREATEST", "LEAST",
            
            // MySQL-specific functions
            "IFNULL", "IF", "GROUP_CONCAT", "CONCAT_WS", "LEFT", "RIGHT",
            "REVERSE", "REPEAT", "INSERT", "LCASE", "UCASE", "LTRIM", "RTRIM",
            "LPAD", "RPAD", "STRCMP", "SOUNDEX", "SPACE", "LOCATE", "POSITION",
            "INSTR", "FIND_IN_SET", "FIELD", "ELT", "MAKE_SET", "EXPORT_SET",
            "QUOTE", "UNHEX", "HEX", "BIN", "OCT", "CONV", "INET_ATON", "INET_NTOA",
            
            // Date/time functions
            "NOW", "CURDATE", "CURTIME", "UNIX_TIMESTAMP", "FROM_UNIXTIME",
            "DATE_ADD", "DATE_SUB", "DATEDIFF", "DATE_FORMAT", "STR_TO_DATE",
            "YEAR", "MONTH", "DAY", "HOUR", "MINUTE", "SECOND", "DAYOFWEEK",
            "DAYOFMONTH", "DAYOFYEAR", "WEEK", "WEEKDAY", "MONTHNAME", "DAYNAME",
            
            // JSON functions
            "JSON_ARRAY", "JSON_OBJECT", "JSON_EXTRACT", "JSON_CONTAINS",
            "JSON_KEYS", "JSON_SEARCH", "JSON_TYPE", "JSON_VALID", "JSON_LENGTH",
            
            // Mathematical functions
            "MOD", "POW", "POWER", "SQRT", "EXP", "LN", "LOG", "LOG10", "LOG2",
            "SIN", "COS", "TAN", "ASIN", "ACOS", "ATAN", "ATAN2", "DEGREES",
            "RADIANS", "PI", "RAND", "SIGN", "TRUNCATE",
        ]
    }
    
    fn get_operators(&self) -> Vec<&'static str> {
        vec![
            // Standard operators
            "=", "!=", "<>", "<", ">", "<=", ">=", "AND", "OR", "NOT",
            "IN", "LIKE", "BETWEEN", "IS", "NULL",
            
            // MySQL-specific operators
            "<=>", "<<", ">>", "&", "|", "^", "~", "REGEXP", "RLIKE",
            "NOT REGEXP", "NOT RLIKE", "SOUNDS LIKE", "->", "->>",
            "DIV", "MOD", "XOR",
        ]
    }
    
    fn get_data_types(&self) -> Vec<&'static str> {
        vec![
            "TINYINT", "SMALLINT", "MEDIUMINT", "INT", "INTEGER", "BIGINT",
            "DECIMAL", "DEC", "NUMERIC", "FLOAT", "DOUBLE", "REAL", "BIT",
            "BOOLEAN", "BOOL", "SERIAL", "DATE", "DATETIME", "TIMESTAMP",
            "TIME", "YEAR", "CHAR", "VARCHAR", "BINARY", "VARBINARY",
            "TINYBLOB", "BLOB", "MEDIUMBLOB", "LONGBLOB", "TINYTEXT", "TEXT",
            "MEDIUMTEXT", "LONGTEXT", "ENUM", "SET", "JSON", "GEOMETRY",
            "POINT", "LINESTRING", "POLYGON", "MULTIPOINT", "MULTILINESTRING",
            "MULTIPOLYGON", "GEOMETRYCOLLECTION",
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
                    "GROUP_CONCAT" | "IF" | "IFNULL" | "COALESCE" |
                    "CONCAT" | "CONCAT_WS" | "SUBSTRING" | "LEFT" | "RIGHT"
                )
            }
            SqlClause::From => {
                // In FROM clause, allow table-related keywords
                matches!(upper_keyword.as_str(),
                    "JOIN" | "INNER" | "LEFT" | "RIGHT" | "FULL" | "OUTER" |
                    "CROSS" | "STRAIGHT_JOIN" | "ON" | "USING" | "WHERE" |
                    "GROUP" | "ORDER" | "LIMIT" | "OFFSET" | "UNION"
                )
            }
            SqlClause::Where => {
                // In WHERE clause, allow conditional operators
                matches!(upper_keyword.as_str(),
                    "AND" | "OR" | "NOT" | "IN" | "EXISTS" | "BETWEEN" |
                    "LIKE" | "REGEXP" | "RLIKE" | "SOUNDS" | "IS" | "NULL" |
                    "GROUP" | "ORDER" | "LIMIT" | "OFFSET"
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
        
        // Add MySQL-specific operators if appropriate
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
        self.parse_mysql_operators(sql, cursor_pos)
    }
    
    fn get_completion_hints(&self, context: &EnhancedSqlContext) -> Vec<CompletionHint> {
        let mut hints = self.get_mysql_hints(context);
        
        // Add database-specific hints based on context
        if let DatabaseSpecificContext::MySQL { 
            mysql_operators, 
            storage_engine_context, 
            .. 
        } = &context.database_context {
            
            // If MySQL operators are present, suggest more MySQL-specific functions
            if !mysql_operators.is_empty() {
                hints.push(CompletionHint {
                    text: "SOUNDEX(".to_string(),
                    description: "Get phonetic representation".to_string(),
                    category: CompletionHintCategory::Function,
                    requires_parentheses: false,
                    priority: 6,
                });
            }
            
            // If storage engine context is present, suggest storage engine options
            if *storage_engine_context {
                hints.push(CompletionHint {
                    text: "AUTO_INCREMENT".to_string(),
                    description: "Set auto increment starting value".to_string(),
                    category: CompletionHintCategory::DatabaseSpecific,
                    requires_parentheses: false,
                    priority: 7,
                });
                
                hints.push(CompletionHint {
                    text: "CHARSET".to_string(),
                    description: "Set character set".to_string(),
                    category: CompletionHintCategory::DatabaseSpecific,
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
    fn test_mysql_parser_creation() {
        let parser = MySQLParser::new();
        assert_eq!(parser.database_type(), DatabaseType::MySQL);
        // Test that we can create the parser successfully
        let functions = parser.get_functions();
        assert!(functions.contains(&"GROUP_CONCAT"));
    }
    
    #[test]
    fn test_mysql_operator_detection() {
        let parser = MySQLParser::new();
        let operators = parser.parse_mysql_operators("SELECT * FROM table WHERE a <=> NULL", 35);
        assert!(operators.contains(&"<=>".to_string()));
    }
    
    #[test]
    fn test_mysql_pattern_detection() {
        let parser = MySQLParser::new();
        let sql = "CREATE TABLE `users` (id INT PRIMARY KEY) ENGINE=InnoDB";
        let context = parser.detect_mysql_patterns(sql);
        
        if let DatabaseSpecificContext::MySQL { 
            backtick_identifiers, 
            storage_engine_context, 
            .. 
        } = context {
            assert!(!backtick_identifiers.is_empty());
            assert!(storage_engine_context);
        } else {
            panic!("Expected MySQL context");
        }
    }
    
    #[test]
    fn test_mysql_keywords_by_category() {
        let parser = MySQLParser::new();
        let functions = parser.get_keywords_by_category(KeywordCategory::Functions);
        assert!(functions.contains(&"IFNULL"));
        assert!(functions.contains(&"GROUP_CONCAT"));
        assert!(functions.contains(&"CONCAT_WS"));
    }
    
    #[test]
    fn test_mysql_backtick_detection() {
        let parser = MySQLParser::new();
        let sql = "SELECT `column_name` FROM `table_name` WHERE `id` = 1";
        let context = parser.detect_mysql_patterns(sql);
        
        if let DatabaseSpecificContext::MySQL { backtick_identifiers, .. } = context {
            assert_eq!(backtick_identifiers.len(), 3);
            assert!(backtick_identifiers.contains(&"`column_name`".to_string()));
            assert!(backtick_identifiers.contains(&"`table_name`".to_string()));
            assert!(backtick_identifiers.contains(&"`id`".to_string()));
        } else {
            panic!("Expected MySQL context");
        }
    }
}