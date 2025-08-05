//! Enhanced SQL Parser for Autocompletion
//! Provides comprehensive SQL parsing with full statement understanding

/// Represents a parsed SQL token with its position
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub value: String,
    pub start: usize,
    pub end: usize,
    pub token_type: TokenType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenType {
    Keyword,
    Identifier,
    Operator,
    Literal,
    Whitespace,
    Punctuation,
}

/// Represents different SQL statement types
#[derive(Debug, Clone, PartialEq)]
pub enum StatementType {
    Select,
    Insert,
    Update,
    Delete,
    CreateTable,
    AlterTable,
    DropTable,
    CreateIndex,
    Unknown,
}

/// Enhanced table reference with all necessary information
#[derive(Debug, Clone, PartialEq)]
pub struct TableRef {
    pub schema: Option<String>,
    pub table: String,
    pub alias: Option<String>,
    /// Position in the SQL statement
    pub position: usize,
}

/// Column reference with table qualification
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnRef {
    pub table: Option<String>,
    pub column: String,
    pub alias: Option<String>,
}

/// Represents the complete SQL context at a cursor position
#[derive(Debug, Clone)]
pub struct SqlContext {
    pub statement_type: StatementType,
    pub tokens: Vec<Token>,
    pub tables: Vec<TableRef>,
    pub columns: Vec<ColumnRef>,
    pub current_clause: SqlClause,
    pub cursor_token: Option<Token>,
    pub expecting: Vec<ExpectedElement>,
}

/// SQL clauses that affect completion behavior
#[derive(Debug, Clone, PartialEq)]
pub enum SqlClause {
    Select,
    From,
    Where,
    Join,
    On,
    GroupBy,
    Having,
    OrderBy,
    Insert,
    InsertColumns,
    InsertValues,
    Update,
    UpdateSet,
    Delete,
    Unknown,
}

/// What the parser expects at the current position
#[derive(Debug, Clone, PartialEq)]
pub enum ExpectedElement {
    Table,
    Column,
    Keyword(Vec<&'static str>),
    Value,
    Function,
    Operator,
    Identifier,
}

/// Main SQL parser
pub struct SqlParser {
    /// Full SQL text
    #[allow(dead_code)]
    text: String,
    /// Current position in text
    #[allow(dead_code)]
    pos: usize,
    /// All tokens
    tokens: Vec<Token>,
}

impl SqlParser {
    pub fn new(text: String) -> Self {
        let tokens = Self::tokenize(&text);
        Self {
            text,
            pos: 0,
            tokens,
        }
    }

    /// Parse SQL and return context at cursor position
    pub fn parse_at_cursor(&self, cursor_pos: usize) -> SqlContext {
        let statement_type = self.detect_statement_type();
        let tables = self.extract_tables();
        let columns = self.extract_columns();
        let current_clause = self.determine_clause_at_position(cursor_pos);
        let cursor_token = self.find_token_at_position(cursor_pos);
        let expecting = self.determine_expectations(&current_clause, &cursor_token, cursor_pos);

        SqlContext {
            statement_type,
            tokens: self.tokens.clone(),
            tables,
            columns,
            current_clause,
            cursor_token,
            expecting,
        }
    }

    /// Tokenize the SQL text
    fn tokenize(text: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut chars = text.char_indices().peekable();
        
        while let Some((start, ch)) = chars.next() {
            let token = match ch {
                ' ' | '\t' | '\n' | '\r' => {
                    // Whitespace
                    let mut end = start;
                    while let Some(&(pos, c)) = chars.peek() {
                        if c.is_whitespace() {
                            end = pos;
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    Token {
                        value: text[start..=end].to_string(),
                        start,
                        end: end + 1,
                        token_type: TokenType::Whitespace,
                    }
                }
                '\'' | '"' => {
                    // String literal
                    let quote = ch;
                    let mut end = start;
                    let mut escaped = false;
                    while let Some((pos, c)) = chars.next() {
                        end = pos;
                        if !escaped && c == quote {
                            break;
                        }
                        escaped = c == '\\' && !escaped;
                    }
                    Token {
                        value: text[start..=end].to_string(),
                        start,
                        end: end + 1,
                        token_type: TokenType::Literal,
                    }
                }
                '(' | ')' | ',' | ';' | '.' => {
                    // Punctuation
                    Token {
                        value: ch.to_string(),
                        start,
                        end: start + 1,
                        token_type: TokenType::Punctuation,
                    }
                }
                '=' | '<' | '>' | '!' => {
                    // Operators
                    let mut end = start;
                    if let Some(&(pos, next_ch)) = chars.peek() {
                        if (ch == '<' && (next_ch == '=' || next_ch == '>')) ||
                           (ch == '>' && next_ch == '=') ||
                           (ch == '!' && next_ch == '=') {
                            end = pos;
                            chars.next();
                        }
                    }
                    Token {
                        value: text[start..=end].to_string(),
                        start,
                        end: end + 1,
                        token_type: TokenType::Operator,
                    }
                }
                _ if ch.is_alphabetic() || ch == '_' => {
                    // Identifier or keyword
                    let mut end = start;
                    while let Some(&(pos, c)) = chars.peek() {
                        if c.is_alphanumeric() || c == '_' {
                            end = pos;
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    let value = text[start..=end].to_string();
                    let token_type = if Self::is_keyword(&value) {
                        TokenType::Keyword
                    } else {
                        TokenType::Identifier
                    };
                    Token {
                        value,
                        start,
                        end: end + 1,
                        token_type,
                    }
                }
                _ if ch.is_numeric() => {
                    // Number literal
                    let mut end = start;
                    let mut has_dot = false;
                    while let Some(&(pos, c)) = chars.peek() {
                        if c.is_numeric() || (c == '.' && !has_dot) {
                            if c == '.' {
                                has_dot = true;
                            }
                            end = pos;
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    Token {
                        value: text[start..=end].to_string(),
                        start,
                        end: end + 1,
                        token_type: TokenType::Literal,
                    }
                }
                _ => {
                    // Unknown character
                    Token {
                        value: ch.to_string(),
                        start,
                        end: start + 1,
                        token_type: TokenType::Punctuation,
                    }
                }
            };
            
            tokens.push(token);
        }
        
        tokens
    }

    fn is_keyword(word: &str) -> bool {
        matches!(
            word.to_uppercase().as_str(),
            "SELECT" | "FROM" | "WHERE" | "INSERT" | "INTO" | "VALUES" | 
            "UPDATE" | "SET" | "DELETE" | "JOIN" | "INNER" | "LEFT" | 
            "RIGHT" | "FULL" | "OUTER" | "ON" | "AS" | "AND" | "OR" | 
            "NOT" | "IN" | "EXISTS" | "BETWEEN" | "LIKE" | "ORDER" | 
            "BY" | "GROUP" | "HAVING" | "LIMIT" | "OFFSET" | "UNION" | 
            "INTERSECT" | "EXCEPT" | "CREATE" | "TABLE" | "INDEX" | 
            "VIEW" | "DROP" | "ALTER" | "ADD" | "COLUMN" | "CONSTRAINT" |
            "PRIMARY" | "KEY" | "FOREIGN" | "REFERENCES" | "UNIQUE" |
            "DEFAULT" | "NULL" | "CASCADE" | "RESTRICT" | "WITH" |
            "DISTINCT" | "ALL" | "CASE" | "WHEN" | "THEN" | "ELSE" | "END"
        )
    }

    fn detect_statement_type(&self) -> StatementType {
        for token in &self.tokens {
            if token.token_type == TokenType::Keyword {
                match token.value.to_uppercase().as_str() {
                    "SELECT" => return StatementType::Select,
                    "INSERT" => return StatementType::Insert,
                    "UPDATE" => return StatementType::Update,
                    "DELETE" => return StatementType::Delete,
                    "CREATE" => {
                        // Look for next keyword
                        if let Some(next) = self.find_next_keyword_after(&token) {
                            match next.value.to_uppercase().as_str() {
                                "TABLE" => return StatementType::CreateTable,
                                "INDEX" => return StatementType::CreateIndex,
                                _ => {}
                            }
                        }
                    }
                    "ALTER" => return StatementType::AlterTable,
                    "DROP" => return StatementType::DropTable,
                    _ => {}
                }
            }
        }
        StatementType::Unknown
    }

    fn find_next_keyword_after(&self, token: &Token) -> Option<&Token> {
        let mut found = false;
        for t in &self.tokens {
            if found && t.token_type == TokenType::Keyword {
                return Some(t);
            }
            if std::ptr::eq(t, token) {
                found = true;
            }
        }
        None
    }

    fn extract_tables(&self) -> Vec<TableRef> {
        let mut tables = Vec::new();
        let mut i = 0;
        
        while i < self.tokens.len() {
            let token = &self.tokens[i];
            
            if token.token_type == TokenType::Keyword {
                match token.value.to_uppercase().as_str() {
                    "FROM" | "JOIN" | "INTO" => {
                        // Look for table after these keywords
                        if let Some(table_ref) = self.parse_table_ref(i + 1) {
                            tables.push(table_ref);
                        }
                    }
                    "UPDATE" => {
                        // UPDATE directly followed by table
                        if let Some(table_ref) = self.parse_table_ref(i + 1) {
                            tables.push(table_ref);
                        }
                    }
                    _ => {}
                }
            }
            
            i += 1;
        }
        
        tables
    }

    fn parse_table_ref(&self, start_idx: usize) -> Option<TableRef> {
        let mut idx = start_idx;
        
        // Skip whitespace
        while idx < self.tokens.len() && self.tokens[idx].token_type == TokenType::Whitespace {
            idx += 1;
        }
        
        if idx >= self.tokens.len() {
            return None;
        }
        
        let first_token = &self.tokens[idx];
        if first_token.token_type != TokenType::Identifier {
            return None;
        }
        
        // Check for schema.table pattern
        let (schema, table, next_idx) = if idx + 2 < self.tokens.len() 
            && self.tokens[idx + 1].value == "." 
            && self.tokens[idx + 2].token_type == TokenType::Identifier {
            (Some(first_token.value.clone()), self.tokens[idx + 2].value.clone(), idx + 3)
        } else {
            (None, first_token.value.clone(), idx + 1)
        };
        
        // Look for alias
        let mut alias = None;
        let mut check_idx = next_idx;
        
        // Skip whitespace
        while check_idx < self.tokens.len() && self.tokens[check_idx].token_type == TokenType::Whitespace {
            check_idx += 1;
        }
        
        if check_idx < self.tokens.len() {
            let token = &self.tokens[check_idx];
            
            // Check for AS keyword
            if token.token_type == TokenType::Keyword && token.value.to_uppercase() == "AS" {
                check_idx += 1;
                // Skip whitespace
                while check_idx < self.tokens.len() && self.tokens[check_idx].token_type == TokenType::Whitespace {
                    check_idx += 1;
                }
                if check_idx < self.tokens.len() && self.tokens[check_idx].token_type == TokenType::Identifier {
                    alias = Some(self.tokens[check_idx].value.clone());
                }
            } else if token.token_type == TokenType::Identifier {
                // Direct alias without AS
                alias = Some(token.value.clone());
            }
        }
        
        Some(TableRef {
            schema,
            table,
            alias,
            position: first_token.start,
        })
    }

    fn extract_columns(&self) -> Vec<ColumnRef> {
        let columns = Vec::new();
        
        // This would parse column references from SELECT, SET clauses etc.
        // For now, keeping it simple
        
        columns
    }

    fn determine_clause_at_position(&self, cursor_pos: usize) -> SqlClause {
        let mut current_clause = SqlClause::Unknown;
        
        for token in &self.tokens {
            if token.start > cursor_pos {
                break;
            }
            
            if token.token_type == TokenType::Keyword {
                match token.value.to_uppercase().as_str() {
                    "SELECT" => current_clause = SqlClause::Select,
                    "FROM" => current_clause = SqlClause::From,
                    "WHERE" => current_clause = SqlClause::Where,
                    "JOIN" | "INNER" | "LEFT" | "RIGHT" | "FULL" => current_clause = SqlClause::Join,
                    "ON" => current_clause = SqlClause::On,
                    "GROUP" => {
                        // Check if followed by BY
                        if let Some(next) = self.find_next_non_whitespace_token(token) {
                            if next.value.to_uppercase() == "BY" {
                                current_clause = SqlClause::GroupBy;
                            }
                        }
                    }
                    "HAVING" => current_clause = SqlClause::Having,
                    "ORDER" => {
                        // Check if followed by BY
                        if let Some(next) = self.find_next_non_whitespace_token(token) {
                            if next.value.to_uppercase() == "BY" {
                                current_clause = SqlClause::OrderBy;
                            }
                        }
                    }
                    "INSERT" => current_clause = SqlClause::Insert,
                    "VALUES" => current_clause = SqlClause::InsertValues,
                    "UPDATE" => current_clause = SqlClause::Update,
                    "SET" => current_clause = SqlClause::UpdateSet,
                    "DELETE" => current_clause = SqlClause::Delete,
                    _ => {}
                }
            }
        }
        
        current_clause
    }

    fn find_next_non_whitespace_token(&self, after: &Token) -> Option<&Token> {
        let mut found = false;
        for token in &self.tokens {
            if found && token.token_type != TokenType::Whitespace {
                return Some(token);
            }
            if std::ptr::eq(token, after) {
                found = true;
            }
        }
        None
    }

    fn find_token_at_position(&self, pos: usize) -> Option<Token> {
        for token in &self.tokens {
            if pos >= token.start && pos <= token.end {
                return Some(token.clone());
            }
        }
        None
    }

    fn determine_expectations(
        &self, 
        clause: &SqlClause, 
        _cursor_token: &Option<Token>,
        cursor_pos: usize
    ) -> Vec<ExpectedElement> {
        let mut expectations = Vec::new();
        
        match clause {
            SqlClause::Select => {
                expectations.push(ExpectedElement::Column);
                expectations.push(ExpectedElement::Function);
                expectations.push(ExpectedElement::Keyword(vec!["DISTINCT", "ALL", "*"]));
            }
            SqlClause::From => {
                expectations.push(ExpectedElement::Table);
            }
            SqlClause::Where | SqlClause::On => {
                expectations.push(ExpectedElement::Column);
                expectations.push(ExpectedElement::Value);
                expectations.push(ExpectedElement::Operator);
            }
            SqlClause::Join => {
                expectations.push(ExpectedElement::Table);
            }
            SqlClause::UpdateSet => {
                // Check if we're before or after =
                if self.is_after_equals(cursor_pos) {
                    expectations.push(ExpectedElement::Value);
                    expectations.push(ExpectedElement::Column);
                    expectations.push(ExpectedElement::Function);
                } else {
                    expectations.push(ExpectedElement::Column);
                }
            }
            SqlClause::InsertColumns => {
                expectations.push(ExpectedElement::Column);
            }
            SqlClause::InsertValues => {
                expectations.push(ExpectedElement::Value);
                expectations.push(ExpectedElement::Function);
            }
            SqlClause::OrderBy | SqlClause::GroupBy => {
                expectations.push(ExpectedElement::Column);
            }
            _ => {
                // General expectations
                expectations.push(ExpectedElement::Keyword(vec![
                    "SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "ALTER", "DROP"
                ]));
            }
        }
        
        expectations
    }

    fn is_after_equals(&self, cursor_pos: usize) -> bool {
        // Look backwards from cursor for = sign
        for token in self.tokens.iter().rev() {
            if token.end > cursor_pos {
                continue;
            }
            if token.value == "=" {
                return true;
            }
            if token.token_type == TokenType::Punctuation && token.value == "," {
                return false;
            }
        }
        false
    }
}

/// Parse SQL at cursor position
pub fn parse_sql_at_cursor(sql: &str, cursor_pos: usize) -> SqlContext {
    let parser = SqlParser::new(sql.to_string());
    parser.parse_at_cursor(cursor_pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_simple_select() {
        let sql = "SELECT * FROM users";
        let tokens = SqlParser::tokenize(sql);
        
        assert_eq!(tokens[0].value, "SELECT");
        assert_eq!(tokens[0].token_type, TokenType::Keyword);
        assert_eq!(tokens[2].value, "*");
        assert_eq!(tokens[4].value, "FROM");
        assert_eq!(tokens[6].value, "users");
        assert_eq!(tokens[6].token_type, TokenType::Identifier);
    }

    #[test]
    fn test_detect_statement_types() {
        let test_cases = vec![
            ("SELECT * FROM users", StatementType::Select),
            ("INSERT INTO users VALUES", StatementType::Insert),
            ("UPDATE users SET name = 'test'", StatementType::Update),
            ("DELETE FROM users WHERE id = 1", StatementType::Delete),
            ("CREATE TABLE users", StatementType::CreateTable),
        ];

        for (sql, expected) in test_cases {
            let parser = SqlParser::new(sql.to_string());
            assert_eq!(parser.detect_statement_type(), expected);
        }
    }

    #[test]
    fn test_extract_tables() {
        let sql = "SELECT u.*, o.total FROM users u JOIN orders o ON u.id = o.user_id";
        let parser = SqlParser::new(sql.to_string());
        let tables = parser.extract_tables();

        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].table, "users");
        assert_eq!(tables[0].alias, Some("u".to_string()));
        assert_eq!(tables[1].table, "orders");
        assert_eq!(tables[1].alias, Some("o".to_string()));
    }

    #[test]
    fn test_update_context() {
        let sql = "UPDATE users SET name = ";
        let parser = SqlParser::new(sql.to_string());
        let context = parser.parse_at_cursor(sql.len());

        assert_eq!(context.statement_type, StatementType::Update);
        assert_eq!(context.current_clause, SqlClause::UpdateSet);
        assert_eq!(context.tables.len(), 1);
        assert_eq!(context.tables[0].table, "users");
        assert!(context.expecting.contains(&ExpectedElement::Value));
    }
}