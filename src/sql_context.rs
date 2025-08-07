//! SQL Context Analysis for better autocompletion
//! Provides context-aware SQL completion by analyzing the current SQL statement

use std::collections::HashMap;

/// Represents a table reference with optional alias and schema
#[derive(Debug, Clone, PartialEq)]
pub struct TableReference {
    pub table_name: String,
    pub alias: Option<String>,
    pub schema: Option<String>,
}

/// SQL contexts for different clauses
#[derive(Debug, Clone, PartialEq)]
pub enum SqlContext {
    /// Inside a SELECT clause - suggest columns, *, aggregates
    SelectClause { from_tables: Vec<TableReference> },
    /// Inside a FROM clause - suggest tables
    FromClause,
    /// Inside a WHERE clause - suggest columns from FROM tables
    WhereClause { from_tables: Vec<TableReference> },
    /// Inside an ORDER BY clause - suggest columns from FROM tables
    OrderByClause { from_tables: Vec<TableReference> },
    /// Inside a GROUP BY clause - suggest columns from FROM tables
    GroupByClause { from_tables: Vec<TableReference> },
    /// Inside a HAVING clause - suggest columns and aggregates
    HavingClause { from_tables: Vec<TableReference> },
    /// After JOIN - suggest tables
    JoinClause,
    /// General context - suggest keywords, tables, etc.
    General,
}

/// Parse the SQL context based on the current line and cursor position
pub fn parse_sql_context(line: &str, cursor_pos: usize) -> SqlContext {
    // We now use the full line for complete query analysis

    // Find keyword positions in the FULL line to understand complete query structure
    let full_upper_line = line.to_uppercase();
    let all_keyword_positions = find_keyword_positions(&full_upper_line);

    // Determine current context based on cursor position and available FROM tables
    let from_tables = extract_from_tables(line);

    // The smart context determination already includes FROM tables
    determine_current_context_smart(&all_keyword_positions, cursor_pos, &from_tables)
}

/// Find positions of SQL keywords in the line
fn find_keyword_positions(upper_line: &str) -> HashMap<&'static str, Vec<usize>> {
    let keywords = [
        "SELECT",
        "FROM",
        "WHERE",
        "ORDER BY",
        "GROUP BY",
        "HAVING",
        "JOIN",
        "INNER JOIN",
        "LEFT JOIN",
        "RIGHT JOIN",
    ];
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
        let is_word_start = absolute_pos == 0
            || !text
                .chars()
                .nth(absolute_pos - 1)
                .unwrap_or(' ')
                .is_alphanumeric();
        let is_word_end = absolute_pos + word.len() == text.len()
            || !text
                .chars()
                .nth(absolute_pos + word.len())
                .unwrap_or(' ')
                .is_alphanumeric();

        if is_word_start && is_word_end {
            positions.push(absolute_pos);
        }

        start = absolute_pos + 1;
    }

    positions
}

/// Smart context determination that considers cursor position AND available FROM tables
fn determine_current_context_smart(
    keyword_positions: &HashMap<&'static str, Vec<usize>>,
    cursor_pos: usize,
    from_tables: &[TableReference],
) -> SqlContext {
    let mut last_keyword_before_cursor: Option<(&str, usize)> = None;

    // Find the last keyword before the cursor position
    for (keyword, positions) in keyword_positions {
        for &pos in positions {
            if pos < cursor_pos {
                if let Some((_, last_pos)) = last_keyword_before_cursor {
                    if pos > last_pos {
                        last_keyword_before_cursor = Some((keyword, pos));
                    }
                } else {
                    last_keyword_before_cursor = Some((keyword, pos));
                }
            }
        }
    }

    // Check if query has FROM tables (indicates complete or partial query structure)
    let has_from_tables = !from_tables.is_empty();

    // Smart context determination
    match last_keyword_before_cursor {
        Some(("SELECT", _)) => {
            // If cursor is in SELECT but query has FROM tables, include them
            // This handles: "SELECT [CURSOR] FROM users_user" scenarios
            SqlContext::SelectClause {
                from_tables: from_tables.to_vec(),
            }
        }
        Some(("FROM", _)) => {
            // In FROM clause - suggest tables
            SqlContext::FromClause
        }
        Some(("WHERE", _)) => SqlContext::WhereClause {
            from_tables: from_tables.to_vec(),
        },
        Some(("ORDER BY", _)) => SqlContext::OrderByClause {
            from_tables: from_tables.to_vec(),
        },
        Some(("GROUP BY", _)) => SqlContext::GroupByClause {
            from_tables: from_tables.to_vec(),
        },
        Some(("HAVING", _)) => SqlContext::HavingClause {
            from_tables: from_tables.to_vec(),
        },
        Some((keyword, _)) if keyword.contains("JOIN") => SqlContext::JoinClause,
        _ => {
            // If no keyword before cursor but we have FROM tables, assume SELECT context
            // This handles edge cases where cursor is at very beginning
            if has_from_tables {
                SqlContext::SelectClause {
                    from_tables: from_tables.to_vec(),
                }
            } else {
                SqlContext::General
            }
        }
    }
}

/// Extract table references from the FROM clause, including aliases and schemas
pub fn extract_from_tables(line: &str) -> Vec<TableReference> {
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
                !matches!(
                    upper_word.as_str(),
                    "WHERE" | "ORDER" | "GROUP" | "HAVING" | "LIMIT" | ";" | ")"
                )
            })
            .collect::<Vec<_>>();

        // Extract table references (handle aliases, joins, etc.)
        let mut i = 0;
        while i < from_clause.len() {
            let word = from_clause[i].trim_end_matches(',');

            // Skip JOIN keywords and AS keyword
            if matches!(
                word.to_uppercase().as_str(),
                "JOIN" | "INNER" | "LEFT" | "RIGHT" | "FULL" | "CROSS" | "ON" | "AS"
            ) {
                i += 1;
                continue;
            }

            // Skip ON conditions
            if i > 0 && from_clause[i - 1].to_uppercase() == "ON" {
                // Skip until we find the next table or end
                while i < from_clause.len()
                    && !matches!(
                        from_clause[i].to_uppercase().as_str(),
                        "JOIN" | "INNER" | "LEFT" | "RIGHT" | ","
                    )
                {
                    i += 1;
                }
                continue;
            }

            // This looks like a table name
            if !word.is_empty() && !word.to_uppercase().starts_with("ON") {
                let clean_table = word.trim_end_matches(',').trim();
                if !clean_table.is_empty() {
                    // Check if it's a schema-qualified table
                    let (schema, table_name) = if clean_table.contains('.') {
                        let parts: Vec<&str> = clean_table.splitn(2, '.').collect();
                        (Some(parts[0].to_string()), parts[1].to_string())
                    } else {
                        (None, clean_table.to_string())
                    };

                    // Check for alias (next non-keyword word)
                    let mut alias = None;
                    if i + 1 < from_clause.len() {
                        let next_word = from_clause[i + 1].trim_end_matches(',');
                        let next_upper = next_word.to_uppercase();

                        // Check if next word is AS keyword
                        if next_upper == "AS" && i + 2 < from_clause.len() {
                            alias = Some(from_clause[i + 2].trim_end_matches(',').to_string());
                            i += 2; // Skip AS and alias
                        } else if !matches!(
                            next_upper.as_str(),
                            "JOIN"
                                | "INNER"
                                | "LEFT"
                                | "RIGHT"
                                | "FULL"
                                | "CROSS"
                                | "WHERE"
                                | "ORDER"
                                | "GROUP"
                                | "HAVING"
                                | ","
                                | "ON"
                        ) {
                            // Direct alias without AS keyword
                            alias = Some(next_word.to_string());
                            i += 1; // Skip alias
                        }
                    }

                    tables.push(TableReference {
                        table_name,
                        alias,
                        schema,
                    });
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
            vec![
                "*",
                "COUNT(",
                "SUM(",
                "AVG(",
                "MAX(",
                "MIN(",
                "DISTINCT",
                "CASE",
                "WHEN",
                "CAST(",
                "COALESCE(",
            ]
        }
        SqlContext::FromClause | SqlContext::JoinClause => {
            // Tables will be suggested by the main completion logic
            vec![]
        }
        SqlContext::WhereClause { .. }
        | SqlContext::OrderByClause { .. }
        | SqlContext::GroupByClause { .. } => {
            // Column names will be suggested based on from_tables
            vec![]
        }
        SqlContext::HavingClause { .. } => {
            // Suggest aggregate functions and column names
            vec![
                "COUNT(", "SUM(", "AVG(", "MAX(", "MIN(", "HAVING", "AND", "OR",
            ]
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
            assert!(from_tables.iter().any(|t| t.table_name == "users"));
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
        assert_eq!(tables.len(), 2);

        let users_ref = tables.iter().find(|t| t.table_name == "users").unwrap();
        assert_eq!(users_ref.alias, Some("u".to_string()));
        assert_eq!(users_ref.schema, None);

        let orders_ref = tables.iter().find(|t| t.table_name == "orders").unwrap();
        assert_eq!(orders_ref.alias, Some("o".to_string()));
        assert_eq!(orders_ref.schema, None);
    }

    #[test]
    fn test_extract_from_tables_with_aliases() {
        let line = "SELECT * FROM users AS u, orders AS o WHERE";
        let tables = extract_from_tables(line);
        assert_eq!(tables.len(), 2);

        let users_ref = tables.iter().find(|t| t.table_name == "users").unwrap();
        assert_eq!(users_ref.alias, Some("u".to_string()));

        let orders_ref = tables.iter().find(|t| t.table_name == "orders").unwrap();
        assert_eq!(orders_ref.alias, Some("o".to_string()));
    }

    #[test]
    fn test_extract_from_tables_with_schema() {
        let line = "SELECT * FROM public.users u, myschema.orders WHERE";
        let tables = extract_from_tables(line);
        assert_eq!(tables.len(), 2);

        let users_ref = tables.iter().find(|t| t.table_name == "users").unwrap();
        assert_eq!(users_ref.alias, Some("u".to_string()));
        assert_eq!(users_ref.schema, Some("public".to_string()));

        let orders_ref = tables.iter().find(|t| t.table_name == "orders").unwrap();
        assert_eq!(orders_ref.alias, None);
        assert_eq!(orders_ref.schema, Some("myschema".to_string()));
    }

    #[test]
    fn test_get_select_suggestions() {
        let context = SqlContext::SelectClause {
            from_tables: vec![],
        };
        let suggestions = get_context_suggestions(&context);
        assert!(suggestions.contains(&"*"));
        assert!(suggestions.contains(&"COUNT("));
        assert!(suggestions.contains(&"SUM("));
    }

    #[test]
    fn test_parse_select_context_with_cursor_moved_back() {
        // Test when user types "SELECT FROM users" then moves cursor back after SELECT
        let line = "SELECT  FROM users u";
        let context = parse_sql_context(line, 7); // Cursor after "SELECT "

        if let SqlContext::SelectClause { from_tables } = context {
            assert_eq!(from_tables.len(), 1);
            assert_eq!(from_tables[0].table_name, "users");
            assert_eq!(from_tables[0].alias, Some("u".to_string()));
        } else {
            panic!("Expected SelectClause context");
        }
    }

    #[test]
    fn test_extract_complex_from_clause() {
        // Test complex FROM clause with multiple tables, joins, and aliases
        let line = "SELECT * FROM public.users u LEFT JOIN orders o ON u.id = o.user_id JOIN products AS p ON p.id = o.product_id";
        let tables = extract_from_tables(line);

        assert_eq!(tables.len(), 3);

        // Check users table
        let users = tables.iter().find(|t| t.table_name == "users").unwrap();
        assert_eq!(users.schema, Some("public".to_string()));
        assert_eq!(users.alias, Some("u".to_string()));

        // Check orders table
        let orders = tables.iter().find(|t| t.table_name == "orders").unwrap();
        assert_eq!(orders.schema, None);
        assert_eq!(orders.alias, Some("o".to_string()));

        // Check products table
        let products = tables.iter().find(|t| t.table_name == "products").unwrap();
        assert_eq!(products.schema, None);
        assert_eq!(products.alias, Some("p".to_string()));
    }
}
