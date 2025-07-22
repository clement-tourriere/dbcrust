//! SQL Context Analysis for better autocompletion
//! Provides context-aware SQL completion by analyzing the current SQL statement

use std::collections::HashMap;

/// SQL contexts for different clauses
#[derive(Debug, Clone, PartialEq)]
pub enum SqlContext {
    /// Inside a SELECT clause - suggest columns, *, aggregates
    SelectClause { from_tables: Vec<String> },
    /// Inside a FROM clause - suggest tables
    FromClause,
    /// Inside a WHERE clause - suggest columns from FROM tables
    WhereClause { from_tables: Vec<String> },
    /// Inside an ORDER BY clause - suggest columns from FROM tables
    OrderByClause { from_tables: Vec<String> },
    /// Inside a GROUP BY clause - suggest columns from FROM tables  
    GroupByClause { from_tables: Vec<String> },
    /// Inside a HAVING clause - suggest columns and aggregates
    HavingClause { from_tables: Vec<String> },
    /// After JOIN - suggest tables
    JoinClause,
    /// General context - suggest keywords, tables, etc.
    General,
}

/// Parse the SQL context based on the current line and cursor position
pub fn parse_sql_context(line: &str, cursor_pos: usize) -> SqlContext {
    let line_before_cursor = if cursor_pos <= line.len() {
        &line[..cursor_pos]
    } else {
        line
    };
    
    // Convert to uppercase for keyword matching
    let upper_line = line_before_cursor.to_uppercase();
    
    // Find the positions of key SQL keywords
    let keyword_positions = find_keyword_positions(&upper_line);
    
    // Determine current context based on the last keyword before cursor
    let current_context = determine_current_context(&keyword_positions, cursor_pos);
    
    match current_context {
        SqlContext::SelectClause { .. } => {
            // Parse FROM clause to get available tables
            let from_tables = extract_from_tables(line);
            SqlContext::SelectClause { from_tables }
        }
        SqlContext::WhereClause { .. } => {
            let from_tables = extract_from_tables(line);
            SqlContext::WhereClause { from_tables }
        }
        SqlContext::OrderByClause { .. } => {
            let from_tables = extract_from_tables(line);
            SqlContext::OrderByClause { from_tables }
        }
        SqlContext::GroupByClause { .. } => {
            let from_tables = extract_from_tables(line);
            SqlContext::GroupByClause { from_tables }
        }
        SqlContext::HavingClause { .. } => {
            let from_tables = extract_from_tables(line);
            SqlContext::HavingClause { from_tables }
        }
        other => other,
    }
}

/// Find positions of SQL keywords in the line
fn find_keyword_positions(upper_line: &str) -> HashMap<&'static str, Vec<usize>> {
    let keywords = ["SELECT", "FROM", "WHERE", "ORDER BY", "GROUP BY", "HAVING", "JOIN", "INNER JOIN", "LEFT JOIN", "RIGHT JOIN"];
    let mut positions = HashMap::new();
    
    for keyword in keywords {
        let keyword_positions: Vec<usize> = find_word_positions(upper_line, keyword);
        if !keyword_positions.is_empty() {
            positions.insert(keyword, keyword_positions);
        }
    }
    
    positions
}

/// Find all positions where a word/phrase appears as a complete word
fn find_word_positions(text: &str, word: &str) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut start = 0;
    
    while let Some(pos) = text[start..].find(word) {
        let absolute_pos = start + pos;
        
        // Check if it's a complete word (not part of another word)
        let is_word_start = absolute_pos == 0 || 
            !text.chars().nth(absolute_pos - 1).unwrap_or(' ').is_alphanumeric();
        let is_word_end = absolute_pos + word.len() == text.len() || 
            !text.chars().nth(absolute_pos + word.len()).unwrap_or(' ').is_alphanumeric();
        
        if is_word_start && is_word_end {
            positions.push(absolute_pos);
        }
        
        start = absolute_pos + 1;
    }
    
    positions
}

/// Determine the current SQL context based on keyword positions
fn determine_current_context(keyword_positions: &HashMap<&'static str, Vec<usize>>, cursor_pos: usize) -> SqlContext {
    let mut last_keyword: Option<(&str, usize)> = None;
    
    // Find the last keyword before the cursor position
    for (keyword, positions) in keyword_positions {
        for &pos in positions {
            if pos < cursor_pos {
                if let Some((_, last_pos)) = last_keyword {
                    if pos > last_pos {
                        last_keyword = Some((keyword, pos));
                    }
                } else {
                    last_keyword = Some((keyword, pos));
                }
            }
        }
    }
    
    match last_keyword {
        Some(("SELECT", _)) => SqlContext::SelectClause { from_tables: vec![] },
        Some(("FROM", _)) => SqlContext::FromClause,
        Some(("WHERE", _)) => SqlContext::WhereClause { from_tables: vec![] },
        Some(("ORDER BY", _)) => SqlContext::OrderByClause { from_tables: vec![] },
        Some(("GROUP BY", _)) => SqlContext::GroupByClause { from_tables: vec![] },
        Some(("HAVING", _)) => SqlContext::HavingClause { from_tables: vec![] },
        Some((keyword, _)) if keyword.contains("JOIN") => SqlContext::JoinClause,
        _ => SqlContext::General,
    }
}

/// Extract table names from the FROM clause
fn extract_from_tables(line: &str) -> Vec<String> {
    let upper_line = line.to_uppercase();
    let mut tables = Vec::new();
    
    // Find FROM keyword
    if let Some(from_pos) = upper_line.find(" FROM ") {
        let after_from = &line[from_pos + 6..]; // Skip " FROM "
        
        // Split by common SQL keywords that would end the FROM clause
        let from_clause = after_from
            .split_whitespace()
            .take_while(|word| {
                let upper_word = word.to_uppercase();
                !matches!(upper_word.as_str(), "WHERE" | "ORDER" | "GROUP" | "HAVING" | "LIMIT" | ";" | ")")
            })
            .collect::<Vec<_>>();
        
        // Extract table names (handle aliases, joins, etc.)
        let mut i = 0;
        while i < from_clause.len() {
            let word = from_clause[i].trim_end_matches(',');
            
            // Skip JOIN keywords
            if matches!(word.to_uppercase().as_str(), "JOIN" | "INNER" | "LEFT" | "RIGHT" | "FULL" | "CROSS" | "ON") {
                i += 1;
                continue;
            }
            
            // Skip ON conditions
            if word.to_uppercase() == "ON" {
                // Skip until we find the next table or end
                while i < from_clause.len() && 
                      !matches!(from_clause[i].to_uppercase().as_str(), "JOIN" | "INNER" | "LEFT" | "RIGHT") {
                    i += 1;
                }
                continue;
            }
            
            // This looks like a table name
            if !word.is_empty() && !word.to_uppercase().starts_with("ON") {
                let clean_table = word.trim_end_matches(',').trim();
                if !clean_table.is_empty() {
                    tables.push(clean_table.to_string());
                }
            }
            
            i += 1;
        }
    }
    
    tables
}

/// Get suggestions based on SQL context
pub fn get_context_suggestions(context: &SqlContext) -> Vec<&'static str> {
    match context {
        SqlContext::SelectClause { .. } => {
            // Suggest columns, *, and common aggregate functions
            vec!["*", "COUNT(", "SUM(", "AVG(", "MAX(", "MIN(", "DISTINCT"]
        }
        SqlContext::FromClause | SqlContext::JoinClause => {
            // Tables will be suggested by the main completion logic
            vec![]
        }
        SqlContext::WhereClause { .. } | 
        SqlContext::OrderByClause { .. } | 
        SqlContext::GroupByClause { .. } => {
            // Column names will be suggested based on from_tables
            vec![]
        }
        SqlContext::HavingClause { .. } => {
            // Suggest aggregate functions and column names
            vec!["COUNT(", "SUM(", "AVG(", "MAX(", "MIN("]
        }
        SqlContext::General => {
            // Default behavior - suggest keywords
            vec![]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_select_context() {
        let line = "SELECT ";
        let context = parse_sql_context(line, 7);
        assert!(matches!(context, SqlContext::SelectClause { .. }));
    }

    #[test]
    fn test_parse_where_context() {
        let line = "SELECT * FROM users WHERE ";
        let context = parse_sql_context(line, 26);
        assert!(matches!(context, SqlContext::WhereClause { .. }));
        
        if let SqlContext::WhereClause { from_tables } = context {
            assert!(from_tables.contains(&"users".to_string()));
        }
    }

    #[test]
    fn test_parse_from_context() {
        let line = "SELECT * FROM ";
        let context = parse_sql_context(line, 14);
        assert_eq!(context, SqlContext::FromClause);
    }

    #[test]
    fn test_extract_from_tables() {
        let line = "SELECT * FROM users u JOIN orders o ON u.id = o.user_id WHERE";
        let tables = extract_from_tables(line);
        assert!(tables.contains(&"users".to_string()));
        assert!(tables.contains(&"orders".to_string()));
    }

    #[test]
    fn test_extract_from_tables_with_aliases() {
        let line = "SELECT * FROM users AS u, orders AS o WHERE";
        let tables = extract_from_tables(line);
        assert!(tables.contains(&"users".to_string()));
        assert!(tables.contains(&"orders".to_string()));
    }

    #[test]
    fn test_get_select_suggestions() {
        let context = SqlContext::SelectClause { from_tables: vec![] };
        let suggestions = get_context_suggestions(&context);
        assert!(suggestions.contains(&"*"));
        assert!(suggestions.contains(&"COUNT("));
        assert!(suggestions.contains(&"SUM("));
    }
}