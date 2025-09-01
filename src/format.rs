use crate::db::{ColumnFilteringInfo, TableDetails};
use chrono;
use prettytable::{Cell, Row, Table};

/// Format a data type with enum values if available
fn format_type_with_enum_values(data_type: &str, enum_values: &Option<Vec<String>>) -> String {
    match enum_values {
        Some(values) if !values.is_empty() => {
            // Format enum values in a readable way
            let values_str = values.join(", ");
            format!("{} ({})", data_type, values_str)
        }
        _ => data_type.to_string(),
    }
}

/// Safe formatting function to prevent "Formatting argument out of range" errors
fn safe_format_with_width(text: &str, width: usize, left_align: bool) -> String {
    if width == 0 {
        return text.to_string();
    }

    if text.len() >= width {
        text.to_string()
    } else {
        let padding = width - text.len();
        if left_align {
            format!("{}{}", text, " ".repeat(padding))
        } else {
            format!("{}{}", " ".repeat(padding), text)
        }
    }
}

#[allow(dead_code)]
pub fn format_query_results_expanded(data: &[Vec<String>]) -> Vec<Table> {
    let mut tables = Vec::new();

    if data.len() < 2 {
        // If there's only a header row or empty data, return empty tables vector
        return tables;
    }

    let header = &data[0]; // First row is header

    // For each data row, create a separate vertical table
    for (i, row) in data.iter().skip(1).enumerate() {
        let mut table = Table::new();

        // Add title row indicating record number
        table.add_row(Row::new(vec![
            Cell::new(&format!("Record {}", i + 1)),
            Cell::new(""),
        ]));

        // Add each field with its column name and value
        for (col_idx, col_name) in header.iter().enumerate() {
            // Make sure we don't go out of bounds
            if col_idx < row.len() {
                table.add_row(Row::new(vec![
                    Cell::new(col_name),
                    Cell::new(&row[col_idx]),
                ]));
            }
        }

        tables.push(table);
    }

    tables
}

#[allow(dead_code)]
pub fn format_query_results_psql(data: &[Vec<String>]) -> String {
    format_query_results_psql_with_info(data, None)
}

#[allow(dead_code)]
pub fn format_query_results_psql_with_info(
    data: &[Vec<String>],
    column_info: Option<&ColumnFilteringInfo>,
) -> String {
    // Use panic catching to handle any formatting errors gracefully
    let result = std::panic::catch_unwind(|| format_query_results_psql_internal(data, column_info));

    match result {
        Ok(formatted) => formatted,
        Err(_panic_info) => {
            eprintln!("PANIC caught in format_query_results_psql!");

            // Write detailed crash analysis
            let analysis = analyze_format_crash(data, "query_results_formatting");
            if let Err(e) = std::fs::write("dbcrust_format_crash.txt", &analysis) {
                eprintln!("Failed to write crash analysis: {e}");
            } else {
                eprintln!("Crash analysis written to dbcrust_format_crash.txt");
            }

            // Return a safe fallback representation
            if data.is_empty() {
                return "No data to display".to_string();
            }

            let mut fallback = String::new();
            fallback.push_str("=== FORMATTING ERROR - SAFE FALLBACK ===\n");
            fallback.push_str(&format!("Rows: {}\n", data.len()));
            if !data.is_empty() {
                fallback.push_str(&format!("Columns: {}\n", data[0].len()));
                fallback.push_str("Header: ");
                for (i, col) in data[0].iter().enumerate() {
                    if i > 0 {
                        fallback.push_str(", ");
                    }
                    fallback.push_str(&format!("\"{col}\""));
                }
                fallback.push('\n');
                fallback.push_str("First few rows (unformatted):\n");
                for (row_idx, row) in data.iter().skip(1).take(3).enumerate() {
                    fallback.push_str(&format!("Row {}: {:?}\n", row_idx + 1, row));
                }
            }
            fallback.push_str("See dbcrust_format_crash.txt for detailed analysis\n");
            fallback
        }
    }
}

fn format_query_results_psql_internal(
    data: &[Vec<String>],
    column_info: Option<&ColumnFilteringInfo>,
) -> String {
    if data.is_empty() {
        return String::new();
    }

    let header = &data[0];

    // Safety check: ensure header is not empty to prevent column width access errors
    if header.is_empty() {
        return String::new();
    }

    // Find the maximum number of columns across ALL rows (header + data)
    let max_cols = data.iter().map(|row| row.len()).max().unwrap_or(0);
    let header_cols = header.len();

    // Create an extended header if some rows have more columns than the original header
    let mut extended_header = header.clone();
    if max_cols > header_cols {
        for i in header_cols..max_cols {
            extended_header.push(format!("column_{}", i + 1));
        }
        eprintln!(
            "Info: Some rows have more columns than header. Extended header from {header_cols} to {max_cols} columns."
        );
    }

    // Validate data consistency with the extended header
    let mut has_inconsistencies = false;
    for (row_idx, row) in data.iter().enumerate() {
        if row.len() != max_cols && row.len() != header_cols {
            has_inconsistencies = true;
            eprintln!(
                "Info: Row {} has {} columns, table has {} columns. Will pad/truncate as needed.",
                row_idx,
                row.len(),
                max_cols
            );
        }
    }

    // If we detect inconsistencies, write analysis to a file (but don't treat as error)
    if has_inconsistencies {
        let analysis = analyze_format_crash(data, "data_consistency_info");
        if let Err(e) = std::fs::write("dbcrust_data_analysis.txt", &analysis) {
            eprintln!("Failed to write data analysis file: {e}");
        } else {
            eprintln!("Data structure analysis written to dbcrust_data_analysis.txt");
        }
    }

    // Find the maximum width needed for each column (using extended header)
    let mut col_widths = vec![0; max_cols];

    // Calculate widths for header columns (including extended ones)
    for (i, col_name) in extended_header.iter().enumerate() {
        col_widths[i] = col_name.len();
    }

    // Calculate widths for all data cells
    for row in data.iter() {
        for (i, cell) in row.iter().enumerate() {
            if i < col_widths.len() {
                col_widths[i] = col_widths[i].max(cell.len());
            }
            // Note: No warning here since we're handling dynamic column counts
        }
    }

    let mut result = String::new();

    // Add header row using extended header (left-aligned in psql)
    for (i, h) in extended_header.iter().enumerate() {
        if i > 0 {
            result.push_str(" | ");
        }
        result.push_str(&safe_format_with_width(h, col_widths[i], true));
    }
    result.push('\n');

    // Add separator line
    for (i, width) in col_widths.iter().enumerate() {
        if i > 0 {
            result.push_str("-+-");
        }
        result.push_str(&"-".repeat(*width));
    }
    result.push('\n');

    // Add data rows (skip header which is data[0])
    for row in data.iter().skip(1) {
        for i in 0..max_cols {
            if i > 0 {
                result.push_str(" | ");
            }

            let cell_value = if i < row.len() {
                &row[i]
            } else {
                "" // Empty string for missing columns
            };

            // Try to right-align numeric values, left-align text
            let is_numeric = !cell_value.is_empty()
                && cell_value
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '+');

            if is_numeric && !cell_value.is_empty() {
                result.push_str(&safe_format_with_width(cell_value, col_widths[i], false));
            } else {
                result.push_str(&safe_format_with_width(cell_value, col_widths[i], true));
            }
        }
        result.push('\n');
    }

    // Add row count
    let row_count = data.len() - 1;
    result.push_str(&format!(
        "({} {})\n",
        row_count,
        if row_count == 1 { "row" } else { "rows" }
    ));

    // Add column indicator if columns are filtered
    if let Some(info) = column_info {
        if info.is_filtered() {
            result.push_str(&format!(
                "📊 Displaying {} of {} columns: {}\n",
                info.displayed_columns,
                info.total_columns,
                info.filtered_column_names.join(", ")
            ));
            result.push_str("💡 Use \\clrcs to clear column selections or \\resetview to reset all view settings\n");
        }
    }

    result
}

#[allow(dead_code)]
pub fn analyze_format_crash(data: &[Vec<String>], query: &str) -> String {
    let mut analysis = String::new();

    analysis.push_str("=== DBCRUST DATA STRUCTURE ANALYSIS ===\n");
    analysis.push_str(&format!("Query: {query}\n"));
    analysis.push_str(&format!(
        "Timestamp: {}\n",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    ));
    analysis.push_str("=======================================\n\n");

    if data.is_empty() {
        analysis.push_str("ISSUE: Data array is completely empty\n");
        return analysis;
    }

    let header = &data[0];
    analysis.push_str(&format!("Header row: {} columns\n", header.len()));
    analysis.push_str(&format!("Header contents: {header:?}\n\n"));

    if header.is_empty() {
        analysis.push_str("ISSUE: Header row is empty\n");
        return analysis;
    }

    analysis.push_str("Data row analysis:\n");
    for (row_idx, row) in data.iter().enumerate() {
        if row_idx == 0 {
            continue; // Skip header
        }

        if row.len() != header.len() {
            analysis.push_str(&format!(
                "ROW {}: {} columns (MISMATCH! Expected {})\n",
                row_idx,
                row.len(),
                header.len()
            ));
            analysis.push_str(&format!("  Contents: {row:?}\n"));
        } else if row_idx < 5 {
            analysis.push_str(&format!("ROW {}: {} columns (OK)\n", row_idx, row.len()));
        }
    }

    let mismatched_rows: Vec<usize> = data
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, row)| row.len() != header.len())
        .map(|(idx, _)| idx)
        .collect();

    if !mismatched_rows.is_empty() {
        analysis.push_str(&format!(
            "\nFOUND {} MISMATCHED ROWS: {:?}\n",
            mismatched_rows.len(),
            mismatched_rows
        ));
    }

    // Check for any rows with suspiciously large column counts
    let max_cols = data.iter().map(|row| row.len()).max().unwrap_or(0);
    let min_cols = data.iter().map(|row| row.len()).min().unwrap_or(0);

    analysis.push_str(&format!("Column count range: {min_cols} to {max_cols}\n"));

    if max_cols > header.len() {
        analysis.push_str("INFO: Some rows have more columns than header!\n");
        analysis.push_str(&format!(
            "SOLUTION: Extended header from {} to {} columns with auto-generated names\n",
            header.len(),
            max_cols
        ));
    }

    if min_cols < header.len() {
        analysis.push_str("INFO: Some rows have fewer columns than header!\n");
        analysis.push_str("SOLUTION: Missing columns will be displayed as empty cells\n");
    }

    analysis.push_str("\n=== DATA HANDLING STRATEGY ===\n");
    analysis.push_str("1. Extended header to accommodate all columns\n");
    analysis.push_str("2. Rows with missing columns: padded with empty cells\n");
    analysis.push_str("3. Rows with extra columns: all values displayed\n");
    analysis.push_str("4. Consistent formatting applied across all rows\n");
    analysis.push_str("5. No data loss - all values are preserved and shown\n\n");

    if max_cols != header.len() {
        analysis.push_str("RESULT: Table will display all data with consistent formatting.\n");
        analysis.push_str("Extra columns will have auto-generated names (column_N).\n");
    } else {
        analysis.push_str("RESULT: Data structure is consistent, normal formatting applied.\n");
    }

    analysis
}

pub fn format_table_details(details: &TableDetails) -> String {
    let mut result = String::new();

    // Table header
    result.push_str(&format!("Table \"{}.{}\"\n", details.schema, details.name));

    // Detect database type based on schema patterns
    let is_sqlite = details.schema == "main";
    let is_mysql = details.schema != "main"
        && details.schema != "public"
        && !details.columns.is_empty()
        && details.columns[0].collation.is_empty();

    // Detect Elasticsearch by looking for capability-style collation values
    let is_elasticsearch = !details.columns.is_empty()
        && details.columns.iter().any(|c| {
            c.collation.contains("filter")
                || c.collation.contains("search")
                || c.collation.contains("select")
                || c.collation.contains("agg")
        });

    if is_sqlite {
        // SQLite-style format (3 columns: Column, Type, Modifiers)
        let mut col_widths = vec![0; 3]; // For Column, Type, Modifiers

        // Calculate column widths
        col_widths[0] = "Column".len().max(
            details
                .columns
                .iter()
                .map(|c| c.name.len())
                .max()
                .unwrap_or(0),
        );
        col_widths[1] = "Type".len().max(
            details
                .columns
                .iter()
                .map(|c| c.data_type.len())
                .max()
                .unwrap_or(0),
        );

        // Modifiers column combines nullable and default
        col_widths[2] = "Modifiers".len().max(
            details
                .columns
                .iter()
                .map(|c| {
                    let mut modifiers = Vec::new();
                    if !c.nullable {
                        modifiers.push("NOT NULL".to_string());
                    }
                    if let Some(ref default) = c.default_value {
                        if !default.is_empty() {
                            modifiers.push(format!("DEFAULT {default}"));
                        }
                    }
                    if modifiers.is_empty() {
                        1 // For the dash
                    } else {
                        modifiers.join(" ").len()
                    }
                })
                .max()
                .unwrap_or(1),
        );

        // Add padding
        for width in &mut col_widths {
            *width += 2;
        }

        // Header row
        result.push_str(&format!(
            "{:<width0$} | {:<width1$} | {:<width2$}\n",
            "Column",
            "Type",
            "Modifiers",
            width0 = col_widths[0],
            width1 = col_widths[1],
            width2 = col_widths[2]
        ));

        // Separator row
        result.push_str(&format!(
            "{}-+-{}-+-{}\n",
            "-".repeat(col_widths[0]),
            "-".repeat(col_widths[1]),
            "-".repeat(col_widths[2])
        ));

        // Data rows
        for col in &details.columns {
            let mut modifiers = Vec::new();
            if !col.nullable {
                modifiers.push("NOT NULL".to_string());
            }
            if let Some(ref default) = col.default_value {
                if !default.is_empty() {
                    modifiers.push(format!("DEFAULT {default}"));
                }
            }
            let modifiers_str = if modifiers.is_empty() {
                "-".to_string() // Use dash for empty modifiers for better visual consistency
            } else {
                modifiers.join(" ")
            };

            result.push_str(&format!(
                "{:<width0$} | {:<width1$} | {:<width2$}\n",
                col.name,
                col.data_type,
                modifiers_str,
                width0 = col_widths[0],
                width1 = col_widths[1],
                width2 = col_widths[2]
            ));
        }
    } else if is_mysql {
        // MySQL-style format (4 columns: Column, Type, Nullable, Default)
        let mut col_widths = vec![0; 4]; // For Column, Type, Nullable, Default

        // Start with header widths as minimums
        col_widths[0] = "Column".len();
        col_widths[1] = "Type".len();
        col_widths[2] = "Nullable".len();
        col_widths[3] = "Default".len();

        // Find maximum width for each column
        for col in &details.columns {
            col_widths[0] = col_widths[0].max(col.name.len());
            col_widths[1] = col_widths[1].max(col.data_type.len());
            col_widths[2] = col_widths[2].max(if col.nullable {
                "YES".len()
            } else {
                "NO".len()
            });
            col_widths[3] = col_widths[3].max(col.default_value.as_ref().map_or(4, |v| v.len())); // 4 for "NULL"
        }

        // Add some padding
        for width in &mut col_widths {
            *width += 2;
        }

        // Header row
        result.push_str(&format!(
            "{:<width0$} | {:<width1$} | {:<width2$} | {:<width3$}\n",
            "Column",
            "Type",
            "Nullable",
            "Default",
            width0 = col_widths[0],
            width1 = col_widths[1],
            width2 = col_widths[2],
            width3 = col_widths[3]
        ));

        // Separator row
        let sep_line = format!(
            "{}-+-{}-+-{}-+-{}\n",
            "-".repeat(col_widths[0]),
            "-".repeat(col_widths[1]),
            "-".repeat(col_widths[2]),
            "-".repeat(col_widths[3])
        );
        result.push_str(&sep_line);

        // Data rows
        for col in &details.columns {
            result.push_str(&format!(
                "{:<width0$} | {:<width1$} | {:<width2$} | {:<width3$}\n",
                col.name,
                col.data_type,
                if col.nullable { "YES" } else { "NO" },
                col.default_value.as_ref().unwrap_or(&"NULL".to_string()),
                width0 = col_widths[0],
                width1 = col_widths[1],
                width2 = col_widths[2],
                width3 = col_widths[3]
            ));
        }
    } else {
        // PostgreSQL-style format (5 columns with collation)
        let mut col_widths = vec![0; 5]; // For Column, Type, Collation, Nullable, Default

        // Start with header widths as minimums
        col_widths[0] = "Column".len();
        col_widths[1] = "Type".len();
        col_widths[2] = if is_elasticsearch {
            "Capabilities".len()
        } else {
            "Collation".len()
        };
        col_widths[3] = "Nullable".len();
        col_widths[4] = "Default".len();

        // Find maximum width for each column
        for col in &details.columns {
            col_widths[0] = col_widths[0].max(col.name.len());

            // Calculate width for type column including enum values if present
            let type_display = format_type_with_enum_values(&col.data_type, &col.enum_values);
            col_widths[1] = col_widths[1].max(type_display.len());

            col_widths[2] = col_widths[2].max(col.collation.len());
            col_widths[3] = col_widths[3].max(if col.nullable {
                "yes".len()
            } else {
                "not null".len()
            });
            col_widths[4] = col_widths[4].max(col.default_value.as_ref().map_or(0, |v| v.len()));
        }

        // Add some padding
        for width in &mut col_widths {
            *width += 2;
        }

        // Header row
        let collation_header = if is_elasticsearch {
            "Capabilities"
        } else {
            "Collation"
        };
        result.push_str(&format!(
            "{:<width0$} | {:<width1$} | {:<width2$} | {:<width3$} | {:<width4$}\n",
            "Column",
            "Type",
            collation_header,
            "Nullable",
            "Default",
            width0 = col_widths[0],
            width1 = col_widths[1],
            width2 = col_widths[2],
            width3 = col_widths[3],
            width4 = col_widths[4]
        ));

        // Separator row
        let sep_line = format!(
            "{}-+-{}-+-{}-+-{}-+-{}\n",
            "-".repeat(col_widths[0]),
            "-".repeat(col_widths[1]),
            "-".repeat(col_widths[2]),
            "-".repeat(col_widths[3]),
            "-".repeat(col_widths[4])
        );
        result.push_str(&sep_line);

        // Data rows
        for col in &details.columns {
            result.push_str(&format!(
                "{:<width0$} | {:<width1$} | {:<width2$} | {:<width3$} | {:<width4$}\n",
                col.name,
                format_type_with_enum_values(&col.data_type, &col.enum_values),
                col.collation,
                if col.nullable { "" } else { "not null" },
                col.default_value.as_ref().unwrap_or(&String::new()),
                width0 = col_widths[0],
                width1 = col_widths[1],
                width2 = col_widths[2],
                width3 = col_widths[3],
                width4 = col_widths[4]
            ));
        }
    }

    // Indexes
    if !details.indexes.is_empty() {
        result.push_str("Indexes:\n");

        for idx in &details.indexes {
            // Use the part of the definition after "USING" or the full definition if "USING" is not present.
            // This part usually contains the method and columns, e.g., "btree (column_name)".
            let def_part = idx
                .definition
                .split_once(" USING ")
                .map_or(&*idx.definition, |(_create_kw, def)| def.trim());

            let idx_type_display = if def_part
                .to_lowercase()
                .starts_with(&idx.index_type.to_lowercase())
            {
                // If def_part already starts with the index type (e.g. "btree (id)"), don't prepend idx.index_type
                String::new()
            } else {
                // Otherwise, include idx.index_type (e.g. for GIN, GIST if not in def_part directly)
                format!("{} ", idx.index_type)
            };

            let idx_desc = if idx.is_primary {
                format!(
                    "\"{}\" PRIMARY KEY, {}{}",
                    idx.name,
                    idx_type_display, // Potentially empty if type is in def_part
                    def_part
                )
            } else if idx.is_unique {
                format!(
                    "\"{}\" UNIQUE CONSTRAINT, {}{}",
                    idx.name, idx_type_display, def_part
                )
            } else {
                format!("\"{}\" {}{}", idx.name, idx_type_display, def_part)
            };

            // Add predicate (WHERE clause) if present
            let idx_line = if let Some(pred) = &idx.predicate {
                format!("    {idx_desc} WHERE {pred}")
            } else {
                format!("    {idx_desc}")
            };

            result.push_str(&format!("{idx_line}\n"));
        }
        result.push('\n'); // Add a blank line after the Indexes section if it's not empty
    }

    // Check constraints
    if !details.check_constraints.is_empty() {
        result.push_str("Check constraints:\n");
        for cc in &details.check_constraints {
            result.push_str(&format!("    \"{}\" {}\n", cc.name, cc.definition));
        }
        result.push('\n'); // Add a blank line after the Check constraints section if it's not empty
    }

    // Foreign keys
    if !details.foreign_keys.is_empty() {
        result.push_str("Foreign-key constraints:\n");

        for fk in &details.foreign_keys {
            result.push_str(&format!("    \"{}\" {}\n", fk.name, fk.definition));
        }
        result.push('\n'); // Add a blank line after the Foreign keys section if it's not empty
    }

    // Referenced by
    if !details.referenced_by.is_empty() {
        result.push_str("Referenced by:\n");

        for rf in &details.referenced_by {
            result.push_str(&format!(
                "    TABLE \"{}\".\"{}\" CONSTRAINT \"{}\" {}\n",
                rf.schema, rf.table, rf.constraint_name, rf.definition
            ));
        }
        result.push('\n'); // Add a blank line after the Referenced by section if it's not empty
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_with_inconsistent_columns() {
        // Test case that reproduces the type of data inconsistency that could cause crashes
        let test_data = vec![
            // Header with 10 columns (like historical_scan_historicalscan table)
            vec![
                "id".to_string(),
                "gg_created_at".to_string(),
                "gg_updated_at".to_string(),
                "type".to_string(),
                "company_id".to_string(),
                "status".to_string(),
                "commit_search_query".to_string(),
                "scan_informations".to_string(),
                "platform_account_id".to_string(),
                "recurrent".to_string(),
            ],
            // Normal row with correct number of columns
            vec![
                "1".to_string(),
                "2023-01-01 00:00:00+00".to_string(),
                "2023-01-01 00:00:00+00".to_string(),
                "full".to_string(),
                "123".to_string(),
                "completed".to_string(),
                "{}".to_string(),
                "{}".to_string(),
                "456".to_string(),
                "true".to_string(),
            ],
            // Problematic row with fewer columns (could happen with corrupted data)
            vec![
                "2".to_string(),
                "2023-01-02 00:00:00+00".to_string(),
                "2023-01-02 00:00:00+00".to_string(),
                "partial".to_string(),
                "124".to_string(),
                // Missing columns that could cause index out of bounds
            ],
            // Row with more columns than expected
            vec![
                "3".to_string(),
                "2023-01-03 00:00:00+00".to_string(),
                "2023-01-03 00:00:00+00".to_string(),
                "full".to_string(),
                "125".to_string(),
                "completed".to_string(),
                "{}".to_string(),
                "{}".to_string(),
                "789".to_string(),
                "false".to_string(),
                "extra_column".to_string(), // Extra column that shouldn't be there
            ],
        ];

        // This should not panic, even with inconsistent data
        let result = format_query_results_psql(&test_data);

        // Verify we get some result (even if it's a fallback)
        assert!(!result.is_empty(), "Should return some formatted output");
        assert!(result.contains("id"), "Should contain header information");

        // Verify that all actual data values are present in the output
        assert!(result.contains("1"), "Should contain row 1 id");
        assert!(result.contains("2"), "Should contain row 2 id");
        assert!(result.contains("3"), "Should contain row 3 id");
        assert!(
            result.contains("2023-01-01"),
            "Should contain row 1 timestamp"
        );
        assert!(
            result.contains("2023-01-02"),
            "Should contain row 2 timestamp"
        );
        assert!(
            result.contains("2023-01-03"),
            "Should contain row 3 timestamp"
        );
        assert!(result.contains("partial"), "Should contain row 2 type");
        assert!(
            result.contains("extra_column"),
            "Should contain extra column value"
        );

        // Verify that auto-generated column name appears for the extra column
        assert!(
            result.contains("column_11"),
            "Should contain auto-generated column name"
        );

        // Verify proper table structure with separators
        assert!(result.contains(" | "), "Should contain column separators");
        assert!(
            result.contains("---"),
            "Should contain header separator line"
        );

        // Count the number of rows (should have 3 data rows plus header and separator)
        let line_count = result.lines().count();
        assert!(
            line_count >= 5,
            "Should have at least 5 lines (header, separator, 3 data rows)"
        );

        println!(
            "Enhanced test completed successfully. Output length: {}",
            result.len()
        );
        println!("Formatted output:\n{result}");
    }

    #[test]
    fn test_safe_formatting_functions() {
        // Test our safe formatting function
        assert_eq!(safe_format_with_width("test", 0, true), "test");
        assert_eq!(safe_format_with_width("test", 10, true), "test      ");
        assert_eq!(safe_format_with_width("test", 10, false), "      test");
        assert_eq!(
            safe_format_with_width("toolongtext", 5, true),
            "toolongtext"
        );
    }

    #[test]
    fn test_empty_data_handling() {
        let empty_data: Vec<Vec<String>> = Vec::new();
        let result = format_query_results_psql(&empty_data);
        assert_eq!(result, "");
    }

    #[test]
    fn test_empty_header_handling() {
        let data_with_empty_header = vec![vec![]];
        let result = format_query_results_psql(&data_with_empty_header);
        assert_eq!(result, "");
    }

    #[test]
    fn test_column_filtering_info_display() {
        use crate::db::ColumnFilteringInfo;

        let test_data = vec![
            vec!["col1".to_string(), "col2".to_string()],
            vec!["val1".to_string(), "val2".to_string()],
        ];

        // Test with column filtering info
        let column_info = ColumnFilteringInfo::new(
            5, // total columns
            2, // displayed columns
            vec!["col1".to_string(), "col2".to_string()],
        );

        let result = format_query_results_psql_with_info(&test_data, Some(&column_info));

        // Verify the column indicator is present and appears after row count
        assert!(
            result.contains("📊 Displaying 2 of 5 columns"),
            "Should contain column indicator"
        );
        assert!(result.contains("col1, col2"), "Should contain column names");
        assert!(result.contains("(1 row)"), "Should contain row count");
        assert!(
            result.contains("💡 Use \\clrcs to clear"),
            "Should contain help message"
        );

        // Verify order: row count should come before column indicator
        let row_pos = result.find("(1 row)").unwrap();
        let col_pos = result.find("📊 Displaying").unwrap();
        assert!(
            row_pos < col_pos,
            "Row count should appear before column indicator"
        );

        // Test without column filtering info
        let result_no_info = format_query_results_psql_with_info(&test_data, None);
        assert!(
            !result_no_info.contains("📊"),
            "Should not contain column indicator when no filtering"
        );
        assert!(
            result_no_info.contains("(1 row)"),
            "Should still contain row count"
        );
    }
}
