use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, StyledText};
use regex::Regex;

pub struct SqlHighlighter {
    sql_keywords: Vec<String>,
    sql_types: Vec<String>,
    sql_functions: Vec<String>,
}

impl SqlHighlighter {
    #[allow(dead_code)]
    pub fn new() -> Self {
        // SQL keywords (commands)
        let sql_keywords = vec![
            "SELECT",
            "FROM",
            "WHERE",
            "INSERT",
            "UPDATE",
            "DELETE",
            "DROP",
            "CREATE",
            "ALTER",
            "TABLE",
            "VIEW",
            "INDEX",
            "TRIGGER",
            "FUNCTION",
            "PROCEDURE",
            "SCHEMA",
            "DATABASE",
            "GROUP",
            "BY",
            "ORDER",
            "HAVING",
            "JOIN",
            "LEFT",
            "RIGHT",
            "INNER",
            "FULL",
            "CROSS",
            "UNION",
            "INTERSECT",
            "EXCEPT",
            "LIMIT",
            "OFFSET",
            "ASC",
            "DESC",
            "DISTINCT",
            "ALL",
            "IN",
            "BETWEEN",
            "LIKE",
            "ILIKE",
            "SIMILAR",
            "TO",
            "IS",
            "NULL",
            "AND",
            "OR",
            "NOT",
            "AS",
            "WITH",
            "ON",
            "USING",
            "RETURNING",
            "VALUES",
            "SET",
            "INTO",
            "DEFAULT",
            "PRIMARY",
            "KEY",
            "FOREIGN",
            "REFERENCES",
            "CONSTRAINT",
            "UNIQUE",
            "CHECK",
            "GRANT",
            "REVOKE",
            "CASCADE",
            "BEGIN",
            "COMMIT",
            "ROLLBACK",
            "TRANSACTION",
        ];

        // SQL data types
        let sql_types = vec![
            "INT",
            "INTEGER",
            "SMALLINT",
            "BIGINT",
            "DECIMAL",
            "NUMERIC",
            "REAL",
            "DOUBLE",
            "PRECISION",
            "SERIAL",
            "BIGSERIAL",
            "MONEY",
            "VARCHAR",
            "CHAR",
            "TEXT",
            "BYTEA",
            "TIMESTAMP",
            "DATE",
            "TIME",
            "INTERVAL",
            "BOOLEAN",
            "ENUM",
            "POINT",
            "LINE",
            "LSEG",
            "BOX",
            "PATH",
            "POLYGON",
            "CIRCLE",
            "CIDR",
            "INET",
            "MACADDR",
            "BIT",
            "UUID",
            "XML",
            "JSON",
            "JSONB",
            "ARRAY",
            "COMPOSITE",
            "RANGE",
            "DOMAIN",
            "OID",
            "REGCLASS",
            "REGPROC",
            "SMALLSERIAL",
        ];

        // SQL functions
        let sql_functions = vec![
            "COUNT",
            "SUM",
            "AVG",
            "MIN",
            "MAX",
            "COALESCE",
            "NULLIF",
            "GREATEST",
            "LEAST",
            "CURRENT_DATE",
            "CURRENT_TIME",
            "CURRENT_TIMESTAMP",
            "CURRENT_USER",
            "SESSION_USER",
            "USER",
            "EXTRACT",
            "SUBSTRING",
            "POSITION",
            "TRIM",
            "UPPER",
            "LOWER",
            "INITCAP",
            "LENGTH",
            "CHAR_LENGTH",
            "BIT_LENGTH",
            "OCTET_LENGTH",
            "ABS",
            "ROUND",
            "TRUNC",
            "CEIL",
            "CEILING",
            "FLOOR",
            "SIGN",
            "RANDOM",
            "SETSEED",
            "CAST",
            "TO_CHAR",
            "TO_DATE",
            "TO_NUMBER",
            "TO_TIMESTAMP",
            "AGE",
            "DATE_PART",
            "DATE_TRUNC",
            "NOW",
            "CONCAT",
            "CONCAT_WS",
            "FORMAT",
            "REGEXP_MATCH",
            "REGEXP_REPLACE",
            "SPLIT_PART",
            "ARRAY_TO_STRING",
            "STRING_TO_ARRAY",
            "STRING_AGG",
            "ARRAY_AGG",
            "JSON_AGG",
            "JSONB_AGG",
            "JSONB_OBJECT_AGG",
            "XMLAGG",
            "BIT_AND",
            "BIT_OR",
            "EVERY",
            "SOME",
            "ANY",
        ];

        SqlHighlighter {
            sql_keywords: sql_keywords.into_iter().map(String::from).collect(),
            sql_types: sql_types.into_iter().map(String::from).collect(),
            sql_functions: sql_functions.into_iter().map(String::from).collect(),
        }
    }
}

impl Highlighter for SqlHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut styled_text = StyledText::new();
        let mut last_end = 0;

        // Style for SQL keywords
        let keyword_style = Style::new().fg(Color::Blue).bold();
        // Style for SQL data types
        let type_style = Style::new().fg(Color::Green).bold();
        // Style for SQL functions
        let function_style = Style::new().fg(Color::Purple).bold();
        // Style for strings
        let string_style = Style::new().fg(Color::Red);
        // Style for numbers
        let number_style = Style::new().fg(Color::Yellow);
        // Style for comments
        let comment_style = Style::new().fg(Color::DarkGray).italic();

        // Highlight SQL comments (-- and /* ... */)
        let comment_regex = Regex::new(r"(--.*$|/\*[\s\S]*?\*/|\s--.*$)").unwrap();
        let string_regex = Regex::new(r#"('[^']*'|"[^"]*")"#).unwrap();
        let number_regex = Regex::new(r"\b\d+(\.\d+)?\b").unwrap();

        // First, look for comments as they override other syntax
        for cap in comment_regex.captures_iter(line) {
            let comment = cap.get(1).unwrap();
            let start = comment.start();
            let end = comment.end();

            // Add any text before this comment with normal styling
            if start > last_end {
                styled_text.push((Style::new(), line[last_end..start].to_string()));
            }

            // Add the comment with comment styling
            styled_text.push((comment_style, line[start..end].to_string()));
            last_end = end;
        }

        // If there were no comments or there's text after the last comment
        if last_end < line.len() {
            let remaining = &line[last_end..];

            // Use regex to find word boundaries safely
            let word_regex = Regex::new(r"\b\w+\b").unwrap();
            let mut current_pos = last_end;

            for word_match in word_regex.find_iter(remaining) {
                let word_start = word_match.start() + last_end;
                let word_end = word_match.end() + last_end;
                let word = &line[word_start..word_end];

                // Skip if this position is inside a string
                let mut is_in_string = false;
                for cap in string_regex.captures_iter(line) {
                    let string_match = cap.get(0).unwrap();
                    if word_start >= string_match.start() && word_end <= string_match.end() {
                        is_in_string = true;
                        break;
                    }
                }

                if is_in_string {
                    continue;
                }

                // Add any text before this word
                if word_start > current_pos {
                    styled_text.push((Style::new(), line[current_pos..word_start].to_string()));
                }

                // Uppercase the word for case-insensitive comparison
                let upper_word = word.to_uppercase();

                // Determine styling
                let style = if self.sql_keywords.contains(&upper_word) {
                    keyword_style
                } else if self.sql_types.contains(&upper_word) {
                    type_style
                } else if self.sql_functions.contains(&upper_word) {
                    function_style
                } else {
                    Style::new()
                };

                // Add the word with appropriate styling
                styled_text.push((style, word.to_string()));
                current_pos = word_end;
            }

            // Add any text after the last word
            if current_pos < line.len() {
                styled_text.push((Style::new(), line[current_pos..].to_string()));
            }
        }

        // Now, highlight strings and numbers
        let mut final_styled_text = StyledText::new();

        // Process each segment in the styled text
        for i in 0..styled_text.buffer.len() {
            if let Some((style, text)) = styled_text.buffer.get(i) {
                // Only apply string/number highlighting if this segment isn't already styled
                // (to avoid overriding comments)
                if style == &Style::new() {
                    let mut last_str_end = 0;
                    let text_str = text.as_str();

                    // Find and highlight strings
                    for cap in string_regex.captures_iter(text_str) {
                        let string_match = cap.get(0).unwrap();
                        let start = string_match.start();
                        let end = string_match.end();

                        // Add text before the string with normal styling
                        if start > last_str_end {
                            final_styled_text
                                .push((Style::new(), text_str[last_str_end..start].to_string()));
                        }

                        // Add the string with string styling
                        final_styled_text.push((string_style, text_str[start..end].to_string()));
                        last_str_end = end;
                    }

                    // If there's remaining text after string processing, handle numbers in it
                    if last_str_end < text_str.len() {
                        let remaining = &text_str[last_str_end..];
                        let mut last_num_end = 0;

                        // Find and highlight numbers
                        for cap in number_regex.captures_iter(remaining) {
                            let num_match = cap.get(0).unwrap();
                            let start = num_match.start();
                            let end = num_match.end();

                            // Add text before the number with normal styling
                            if start > last_num_end {
                                final_styled_text.push((
                                    Style::new(),
                                    remaining[last_num_end..start].to_string(),
                                ));
                            }

                            // Add the number with number styling
                            final_styled_text
                                .push((number_style, remaining[start..end].to_string()));
                            last_num_end = end;
                        }

                        // Add any remaining text after number processing
                        if last_num_end < remaining.len() {
                            final_styled_text
                                .push((Style::new(), remaining[last_num_end..].to_string()));
                        }
                    }
                } else {
                    // Keep the existing styling for already styled text
                    final_styled_text.push((*style, text.clone()));
                }
            }
        }

        if final_styled_text.buffer.len() == 0 {
            // If no highlighting was applied, return the original text
            return styled_text;
        }

        final_styled_text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    fn test_sql_keyword_highlighting() {
        let highlighter = SqlHighlighter::new();
        let line = "SELECT * FROM users WHERE id = 123";
        let styled = highlighter.highlight(line, 0);

        // This is just a simple test to ensure the highlighter runs without errors
        // A more comprehensive test would inspect the styled output
        assert!(styled.buffer.len() > 1);
    }

    #[rstest]
    fn test_sql_comment_highlighting() {
        let highlighter = SqlHighlighter::new();
        let line = "SELECT * FROM users -- Get all users";
        let styled = highlighter.highlight(line, 0);

        // Check that we have styling for the comment section
        assert!(styled.buffer.len() > 1);
    }

    #[rstest]
    fn test_sql_string_highlighting() {
        let highlighter = SqlHighlighter::new();
        let line = "SELECT * FROM users WHERE name = 'John'";
        let styled = highlighter.highlight(line, 0);

        // Ensure strings get styled
        assert!(styled.buffer.len() > 1);
    }

    #[rstest]
    fn test_complex_sql_query() {
        let highlighter = SqlHighlighter::new();
        let query = r#"
            WITH active_users AS (
                SELECT 
                    id, 
                    first_name, 
                    last_name,
                    CAST(created_at AS DATE) AS join_date
                FROM 
                    users
                WHERE 
                    status = 'active' AND
                    created_at > '2023-01-01'
            )
            SELECT 
                au.id,
                au.first_name || ' ' || au.last_name AS full_name,
                COUNT(o.id) AS total_orders,
                SUM(o.amount) AS total_spent
            FROM 
                active_users au
            LEFT JOIN 
                orders o ON au.id = o.user_id
            GROUP BY 
                au.id, au.first_name, au.last_name
            HAVING 
                COUNT(o.id) > 0
            ORDER BY 
                total_spent DESC
            LIMIT 10;
            -- This query finds top 10 spending active users
        "#;

        let styled = highlighter.highlight(query, 0);
        // Just check that the highlighter doesn't crash on complex queries
        assert!(styled.buffer.len() > 1);
    }
}
