use regex::Regex;

/// Process a named query with parameter substitution.
/// Supports:
/// - Positional parameters: $1, $2, $3, etc.
/// - Raw aggregation: $* (all remaining arguments as is)
/// - String aggregation: $@ (all remaining arguments as quoted strings)
#[allow(dead_code)]
pub fn process_query(query: &str, args: &[&str]) -> String {
    let mut result = query.to_string();

    // Process positional parameters ($1, $2, etc.)
    let pos_pattern = Regex::new(r"\$(\d+)").unwrap();
    for cap in pos_pattern.captures_iter(query) {
        let pos: usize = cap[1].parse().unwrap();
        if pos > 0 && pos <= args.len() {
            // In regex, positions are 1-indexed but in args they're 0-indexed
            let replacement = args[pos - 1];
            result = result.replacen(&cap[0], replacement, 1);
        }
    }

    // Process raw aggregation ($*)
    if result.contains("$*") {
        let replacement = args.join(", ");
        result = result.replace("$*", &replacement);
    }

    // Process string aggregation ($@)
    if result.contains("$@") {
        // For string aggregation with combined parameters, we should not include
        // the positional parameters that were already used
        // We're only using the rest of the arguments after skipping the positional ones
        let used_positions = pos_pattern
            .captures_iter(query)
            .filter_map(|cap| cap[1].parse::<usize>().ok())
            .filter(|&pos| pos > 0 && pos <= args.len())
            .collect::<Vec<_>>();

        // Get max positional parameter
        let max_position = used_positions.iter().max().cloned().unwrap_or(0);

        // Use args starting after the last positional parameter
        let remaining_args = if max_position > 0 {
            &args[max_position..]
        } else {
            args
        };

        let quoted_args: Vec<String> = remaining_args
            .iter()
            .map(|arg| format!("'{}'", arg.replace('\'', "''")))
            .collect();

        let replacement = quoted_args.join(", ");
        result = result.replace("$@", &replacement);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_positional_parameters() {
        let query = "SELECT * FROM users WHERE name = '$1' AND age = $2";
        let args = vec!["John Doe", "30"];
        let processed = process_query(query, &args);
        assert_eq!(
            processed,
            "SELECT * FROM users WHERE name = 'John Doe' AND age = 30"
        );
    }

    #[test]
    fn test_raw_aggregation() {
        let query = "SELECT * FROM users WHERE age IN ($*)";
        let args = vec!["18", "21", "30"];
        let processed = process_query(query, &args);
        assert_eq!(processed, "SELECT * FROM users WHERE age IN (18, 21, 30)");
    }

    #[test]
    fn test_string_aggregation() {
        let query = "SELECT * FROM users WHERE category IN ($@)";
        let args = vec!["home user", "mobile user"];
        let processed = process_query(query, &args);
        assert_eq!(
            processed,
            "SELECT * FROM users WHERE category IN ('home user', 'mobile user')"
        );
    }

    #[test]
    fn test_combined_parameters() {
        let query = "SELECT * FROM users WHERE age = $1 AND category IN ($@)";
        let args = vec!["30", "home user", "mobile user"];
        let processed = process_query(query, &args);
        assert_eq!(
            processed,
            "SELECT * FROM users WHERE age = 30 AND category IN ('home user', 'mobile user')"
        );
    }

    #[test]
    fn test_no_parameters_passthrough() {
        let query = "SELECT * FROM users";
        let args: Vec<&str> = vec![];
        let processed = process_query(query, &args);
        assert_eq!(processed, "SELECT * FROM users");
    }

    #[test]
    fn test_unmatched_parameter_preserved() {
        let query = "SELECT * FROM users WHERE id = $3";
        let args = vec!["only_one"];
        let processed = process_query(query, &args);
        // $3 has no matching arg (only 1 arg provided), should remain as $3
        assert_eq!(processed, "SELECT * FROM users WHERE id = $3");
    }

    #[test]
    fn test_empty_args_with_parameters() {
        let query = "SELECT $1 FROM users";
        let args: Vec<&str> = vec![];
        let processed = process_query(query, &args);
        // No args provided, parameter stays
        assert_eq!(processed, "SELECT $1 FROM users");
    }

    #[test]
    fn test_single_positional_parameter() {
        let query = "SELECT * FROM $1";
        let args = vec!["users"];
        let processed = process_query(query, &args);
        assert_eq!(processed, "SELECT * FROM users");
    }

    #[test]
    fn test_raw_aggregation_single_arg() {
        let query = "SELECT * FROM users WHERE id IN ($*)";
        let args = vec!["42"];
        let processed = process_query(query, &args);
        assert_eq!(processed, "SELECT * FROM users WHERE id IN (42)");
    }

    #[test]
    fn test_string_aggregation_single_arg() {
        let query = "SELECT * FROM users WHERE name IN ($@)";
        let args = vec!["Alice"];
        let processed = process_query(query, &args);
        assert_eq!(processed, "SELECT * FROM users WHERE name IN ('Alice')");
    }

    #[test]
    fn test_special_characters_in_string_aggregation() {
        // Single quotes in args should be escaped for $@
        let query = "SELECT * FROM users WHERE name IN ($@)";
        let args = vec!["O'Brien"];
        let processed = process_query(query, &args);
        assert_eq!(processed, "SELECT * FROM users WHERE name IN ('O''Brien')");
    }

    #[test]
    fn test_multiple_positional_parameters() {
        let query = "SELECT * FROM $1 WHERE $2 = $3";
        let args = vec!["users", "id", "42"];
        let processed = process_query(query, &args);
        assert_eq!(processed, "SELECT * FROM users WHERE id = 42");
    }

    #[test]
    fn test_no_parameters_with_args_provided() {
        // Query has no parameters, args are provided but unused
        let query = "SELECT COUNT(*) FROM orders";
        let args = vec!["unused_arg"];
        let processed = process_query(query, &args);
        assert_eq!(processed, "SELECT COUNT(*) FROM orders");
    }

    #[test]
    fn test_raw_aggregation_empty_args() {
        let query = "SELECT * FROM users WHERE id IN ($*)";
        let args: Vec<&str> = vec![];
        let processed = process_query(query, &args);
        assert_eq!(processed, "SELECT * FROM users WHERE id IN ()");
    }
}
