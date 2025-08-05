//! Database completion provider trait
//! Defines the interface for database-specific completion implementations

use async_trait::async_trait;
use std::error::Error;

/// Information about a table for completion
#[derive(Debug, Clone)]
pub struct TableInfo {
    pub schema: Option<String>,
    pub name: String,
    pub table_type: TableType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TableType {
    Table,
    View,
    MaterializedView,
    ForeignTable,
}

/// Information about a column for completion
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub ordinal_position: i32,
}

/// Information about a function for completion
#[derive(Debug, Clone)]
pub struct FunctionInfo {
    pub schema: Option<String>,
    pub name: String,
    pub arguments: String,
    pub return_type: String,
}

/// Database-specific completion provider
#[async_trait]
pub trait CompletionProvider: Send + Sync {
    /// Get all accessible schemas
    async fn get_schemas(&self) -> Result<Vec<String>, Box<dyn Error>>;
    
    /// Get tables in a schema (or all schemas if None)
    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<TableInfo>, Box<dyn Error>>;
    
    /// Get columns for a table (with optional schema qualification)
    async fn get_columns(&self, schema: Option<&str>, table: &str) -> Result<Vec<ColumnInfo>, Box<dyn Error>>;
    
    /// Get database functions for completion
    async fn get_functions(&self, schema: Option<&str>) -> Result<Vec<FunctionInfo>, Box<dyn Error>>;
    
    /// Get database-specific keywords
    fn get_keywords(&self) -> Vec<&'static str> {
        // Default SQL keywords, can be overridden
        vec![
            "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES",
            "UPDATE", "SET", "DELETE", "JOIN", "INNER", "LEFT",
            "RIGHT", "FULL", "ON", "AS", "AND", "OR", "NOT",
            "IN", "EXISTS", "BETWEEN", "LIKE", "ORDER", "BY",
            "GROUP", "HAVING", "LIMIT", "OFFSET", "UNION",
            "CREATE", "TABLE", "INDEX", "VIEW", "DROP", "ALTER",
            "DISTINCT", "ALL", "CASE", "WHEN", "THEN", "ELSE", "END",
        ]
    }
    
    /// Get database-specific functions/operators
    fn get_builtin_functions(&self) -> Vec<&'static str> {
        // Default functions, can be overridden
        vec![
            "COUNT", "SUM", "AVG", "MAX", "MIN", "UPPER", "LOWER",
            "LENGTH", "TRIM", "SUBSTR", "SUBSTRING", "REPLACE",
            "CONCAT", "ABS", "ROUND", "CEIL", "FLOOR", "NOW",
            "CURRENT_DATE", "CURRENT_TIME", "CURRENT_TIMESTAMP",
            "CAST", "COALESCE", "NULLIF",
        ]
    }
    
    /// Check if a function name requires parentheses
    fn requires_parentheses(&self, function: &str) -> bool {
        // Most functions require parentheses, but some don't (like CURRENT_DATE)
        !matches!(
            function.to_uppercase().as_str(),
            "CURRENT_DATE" | "CURRENT_TIME" | "CURRENT_TIMESTAMP" | 
            "CURRENT_USER" | "SESSION_USER" | "USER"
        )
    }
}

/// Mock implementation for testing
#[cfg(test)]
pub struct MockCompletionProvider {
    pub schemas: Vec<String>,
    pub tables: Vec<TableInfo>,
    pub columns: Vec<ColumnInfo>,
}

#[cfg(test)]
#[async_trait]
impl CompletionProvider for MockCompletionProvider {
    async fn get_schemas(&self) -> Result<Vec<String>, Box<dyn Error>> {
        Ok(self.schemas.clone())
    }
    
    async fn get_tables(&self, _schema: Option<&str>) -> Result<Vec<TableInfo>, Box<dyn Error>> {
        Ok(self.tables.clone())
    }
    
    async fn get_columns(&self, _schema: Option<&str>, _table: &str) -> Result<Vec<ColumnInfo>, Box<dyn Error>> {
        Ok(self.columns.clone())
    }
    
    async fn get_functions(&self, _schema: Option<&str>) -> Result<Vec<FunctionInfo>, Box<dyn Error>> {
        Ok(vec![])
    }
}