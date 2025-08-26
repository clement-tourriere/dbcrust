//! JSON data display implementation using the unified complex display system

use crate::complex_display::{
    ComplexDataDisplay, ComplexDataMetadata, ComplexDataParser, ComplexDisplayConfig,
};
use serde_json::Value;

/// JSON display adapter for the unified display system
pub struct JsonDisplayAdapter {
    pub value: Value,
    pub raw_json: String,
}

impl JsonDisplayAdapter {
    pub fn new(raw_json: String) -> Result<Self, serde_json::Error> {
        let value: Value = serde_json::from_str(&raw_json)?;
        Ok(Self { value, raw_json })
    }

    /// Count total number of elements/fields recursively
    fn count_elements(&self, value: &Value) -> usize {
        match value {
            Value::Object(map) => 1 + map.values().map(|v| self.count_elements(v)).sum::<usize>(),
            Value::Array(arr) => 1 + arr.iter().map(|v| self.count_elements(v)).sum::<usize>(),
            _ => 1,
        }
    }

    /// Calculate maximum depth of nested structure
    fn calculate_depth(&self, value: &Value) -> usize {
        match value {
            Value::Object(map) => {
                if map.is_empty() {
                    1
                } else {
                    1 + map
                        .values()
                        .map(|v| self.calculate_depth(v))
                        .max()
                        .unwrap_or(0)
                }
            }
            Value::Array(arr) => {
                if arr.is_empty() {
                    1
                } else {
                    1 + arr
                        .iter()
                        .map(|v| self.calculate_depth(v))
                        .max()
                        .unwrap_or(0)
                }
            }
            _ => 1,
        }
    }

    /// Check if structure has nested objects/arrays
    fn has_nested_structures(&self, value: &Value) -> bool {
        match value {
            Value::Object(map) => map
                .values()
                .any(|v| matches!(v, Value::Object(_) | Value::Array(_))),
            Value::Array(arr) => arr
                .iter()
                .any(|v| matches!(v, Value::Object(_) | Value::Array(_))),
            _ => false,
        }
    }

    /// Get schema information about the JSON structure
    fn get_schema_info(&self) -> String {
        match &self.value {
            Value::Object(map) => {
                let fields: Vec<String> = map.keys().take(5).cloned().collect();
                let field_summary = if map.len() > 5 {
                    format!("{}, ... ({} total)", fields.join(", "), map.len())
                } else {
                    fields.join(", ")
                };
                format!("Object {{ {} }}", field_summary)
            }
            Value::Array(arr) => {
                if arr.is_empty() {
                    "Array[]".to_string()
                } else {
                    let first_type = match &arr[0] {
                        Value::Object(_) => "Object",
                        Value::Array(_) => "Array",
                        Value::String(_) => "String",
                        Value::Number(_) => "Number",
                        Value::Bool(_) => "Boolean",
                        Value::Null => "Null",
                    };
                    format!("Array[{}; {}]", first_type, arr.len())
                }
            }
            Value::String(_) => "String".to_string(),
            Value::Number(_) => "Number".to_string(),
            Value::Bool(_) => "Boolean".to_string(),
            Value::Null => "Null".to_string(),
        }
    }

    /// Format JSON with indentation
    fn format_pretty_json(&self, value: &Value, indent: usize, max_width: usize) -> String {
        let indent_str = "  ".repeat(indent);
        let next_indent_str = "  ".repeat(indent + 1);

        match value {
            Value::Object(map) => {
                if map.is_empty() {
                    "{}".to_string()
                } else if map.len() == 1 && !self.has_nested_structures(value) {
                    // Inline small objects
                    let (key, val) = map.iter().next().unwrap();
                    format!(
                        "{{ \"{}\": {} }}",
                        key,
                        self.format_pretty_json(val, 0, max_width)
                    )
                } else {
                    let mut result = "{\n".to_string();
                    let items: Vec<_> = map.iter().collect();
                    for (i, (key, val)) in items.iter().enumerate() {
                        result.push_str(&format!(
                            "{}\"{}\": {}",
                            next_indent_str,
                            key,
                            self.format_pretty_json(val, indent + 1, max_width)
                        ));
                        if i < items.len() - 1 {
                            result.push(',');
                        }
                        result.push('\n');
                    }
                    result.push_str(&format!("{}}}", indent_str));
                    result
                }
            }
            Value::Array(arr) => {
                if arr.is_empty() {
                    "[]".to_string()
                } else if arr.len() <= 3 && !self.has_nested_structures(value) {
                    // Inline small arrays
                    let items: Vec<String> = arr
                        .iter()
                        .map(|v| self.format_pretty_json(v, 0, max_width))
                        .collect();
                    format!("[{}]", items.join(", "))
                } else {
                    let mut result = "[\n".to_string();
                    for (i, item) in arr.iter().enumerate() {
                        result.push_str(&format!(
                            "{}{}",
                            next_indent_str,
                            self.format_pretty_json(item, indent + 1, max_width)
                        ));
                        if i < arr.len() - 1 {
                            result.push(',');
                        }
                        result.push('\n');
                    }
                    result.push_str(&format!("{}]", indent_str));
                    result
                }
            }
            Value::String(s) => format!("\"{}\"", s),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => "null".to_string(),
        }
    }
}

impl ComplexDataDisplay for JsonDisplayAdapter {
    fn metadata(&self) -> ComplexDataMetadata {
        ComplexDataMetadata {
            data_type: "json".to_string(),
            size: self.count_elements(&self.value),
            depth: Some(self.calculate_depth(&self.value)),
            has_nested: self.has_nested_structures(&self.value),
            schema_info: Some(self.get_schema_info()),
        }
    }

    fn format_full(&self, config: &ComplexDisplayConfig) -> String {
        let metadata = self.metadata();
        let mut result = String::new();

        if config.show_dimensions {
            result.push_str(&format!(
                "JSON ({} elements, depth {}):\n",
                metadata.size,
                metadata.depth.unwrap_or(0)
            ));
        }

        result.push_str(&self.format_pretty_json(&self.value, 0, config.max_width));
        result
    }

    fn format_truncated(&self, config: &ComplexDisplayConfig) -> String {
        let metadata = self.metadata();

        // For large structures, show abbreviated version
        if metadata.size > config.truncation_length {
            match &self.value {
                Value::Object(map) => {
                    let shown_keys: Vec<_> = map.keys().take(config.truncation_length).collect();
                    let remaining = map.len().saturating_sub(config.truncation_length);

                    let mut result = "{ ".to_string();
                    for (i, key) in shown_keys.iter().enumerate() {
                        if i > 0 {
                            result.push_str(", ");
                        }
                        result.push_str(&format!("\"{}\": ...", key));
                    }
                    if remaining > 0 {
                        result.push_str(&format!(", ... {} more", remaining));
                    }
                    result.push_str(" }");

                    if config.show_dimensions {
                        result.push_str(&format!(" (size: {})", metadata.size));
                    }
                    result
                }
                Value::Array(arr) => {
                    let shown: Vec<String> = arr
                        .iter()
                        .take(config.truncation_length)
                        .map(|v| match v {
                            Value::Object(_) => "{...}".to_string(),
                            Value::Array(_) => "[...]".to_string(),
                            _ => serde_json::to_string(v).unwrap_or("...".to_string()),
                        })
                        .collect();

                    let remaining = arr.len().saturating_sub(config.truncation_length);
                    let mut result = format!("[{}", shown.join(", "));

                    if remaining > 0 {
                        result.push_str(&format!(", ... {} more", remaining));
                    }
                    result.push(']');

                    if config.show_dimensions {
                        result.push_str(&format!(" (size: {})", arr.len()));
                    }
                    result
                }
                _ => {
                    // For primitive values, just show them
                    let formatted = serde_json::to_string(&self.value)
                        .unwrap_or_else(|_| self.raw_json.clone());
                    if formatted.len() > 50 {
                        format!("{}... (truncated)", &formatted[..47])
                    } else {
                        formatted
                    }
                }
            }
        } else {
            self.format_full(config)
        }
    }

    fn format_summary(&self, _config: &ComplexDisplayConfig) -> String {
        let metadata = self.metadata();

        let type_info = match &self.value {
            Value::Object(map) => {
                let _field_types = map
                    .values()
                    .map(|v| match v {
                        Value::Object(_) => "Object",
                        Value::Array(_) => "Array",
                        Value::String(_) => "String",
                        Value::Number(_) => "Number",
                        Value::Bool(_) => "Boolean",
                        Value::Null => "Null",
                    })
                    .collect::<std::collections::HashSet<_>>();

                format!("Object with {} fields", map.len())
            }
            Value::Array(arr) => {
                let element_types = arr
                    .iter()
                    .map(|v| match v {
                        Value::Object(_) => "Object",
                        Value::Array(_) => "Array",
                        Value::String(_) => "String",
                        Value::Number(_) => "Number",
                        Value::Bool(_) => "Boolean",
                        Value::Null => "Null",
                    })
                    .collect::<std::collections::HashSet<_>>();

                format!(
                    "Array[{}] with {} elements",
                    element_types.into_iter().collect::<Vec<_>>().join("|"),
                    arr.len()
                )
            }
            _ => format!("{:?}", self.value),
        };

        format!(
            "JSON: {} | Size: {} elements | Depth: {} | Schema: {}",
            type_info,
            metadata.size,
            metadata.depth.unwrap_or(0),
            metadata
                .schema_info
                .unwrap_or_else(|| "Unknown".to_string())
        )
    }

    fn format_viz(&self, config: &ComplexDisplayConfig) -> String {
        let _metadata = self.metadata();

        // Create a simple tree-like visualization
        match &self.value {
            Value::Object(map) => {
                let mut lines = vec!["JSON Object:".to_string()];
                for (key, value) in map.iter().take(config.viz_width / 4) {
                    let value_desc = match value {
                        Value::Object(inner) => format!("Object({} fields)", inner.len()),
                        Value::Array(inner) => format!("Array[{}]", inner.len()),
                        Value::String(s) => {
                            format!("\"{}\"", if s.len() > 20 { &s[..17] } else { s })
                        }
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        Value::Null => "null".to_string(),
                    };
                    lines.push(format!("  {}: {}", key, value_desc));
                }
                if map.len() > config.viz_width / 4 {
                    lines.push(format!(
                        "  ... {} more fields",
                        map.len() - config.viz_width / 4
                    ));
                }
                lines.join("\n")
            }
            Value::Array(arr) => {
                let mut lines = vec![format!("JSON Array[{}]:", arr.len())];
                for (i, value) in arr.iter().take(config.viz_width / 6).enumerate() {
                    let value_desc = match value {
                        Value::Object(inner) => format!("Object({} fields)", inner.len()),
                        Value::Array(inner) => format!("Array[{}]", inner.len()),
                        Value::String(s) => {
                            format!("\"{}\"", if s.len() > 15 { &s[..12] } else { s })
                        }
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        Value::Null => "null".to_string(),
                    };
                    lines.push(format!("  [{}]: {}", i, value_desc));
                }
                if arr.len() > config.viz_width / 6 {
                    lines.push(format!(
                        "  ... {} more elements",
                        arr.len() - config.viz_width / 6
                    ));
                }
                lines.join("\n")
            }
            _ => format!(
                "JSON Value: {}",
                serde_json::to_string(&self.value).unwrap_or_else(|_| "invalid".to_string())
            ),
        }
    }
}

impl ComplexDataParser<JsonDisplayAdapter> for JsonDisplayAdapter {
    type Error = serde_json::Error;

    fn parse(raw_data: &str) -> Result<JsonDisplayAdapter, Self::Error> {
        JsonDisplayAdapter::new(raw_data.to_string())
    }

    fn validate(raw_data: &str) -> bool {
        serde_json::from_str::<Value>(raw_data).is_ok()
    }

    fn schema_info(raw_data: &str) -> Option<String> {
        if let Ok(value) = serde_json::from_str::<Value>(raw_data) {
            let adapter = JsonDisplayAdapter {
                value,
                raw_json: raw_data.to_string(),
            };
            Some(adapter.get_schema_info())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::complex_display::ComplexDisplayConfig;

    #[test]
    fn test_json_object_display() {
        let json_str = r#"{"name": "John", "age": 30, "city": "New York"}"#;
        let adapter = JsonDisplayAdapter::new(json_str.to_string()).unwrap();
        let config = ComplexDisplayConfig::default();

        // Test metadata
        let metadata = adapter.metadata();
        assert_eq!(metadata.data_type, "json");
        assert_eq!(metadata.size, 4); // Object + 3 fields
        assert!(metadata.schema_info.is_some());

        // Test different formats
        let full = adapter.format_full(&config);
        assert!(full.contains("John"));
        assert!(full.contains("30"));

        let summary = adapter.format_summary(&config);
        assert!(summary.contains("Object with 3 fields"));
        assert!(summary.contains("Size: 4 elements"));
    }

    #[test]
    fn test_json_array_display() {
        let json_str = r#"[1, 2, {"nested": true}, "string"]"#;
        let adapter = JsonDisplayAdapter::new(json_str.to_string()).unwrap();
        let config = ComplexDisplayConfig::default();

        let metadata = adapter.metadata();
        assert_eq!(metadata.data_type, "json");
        assert!(metadata.has_nested);

        let viz = adapter.format_viz(&config);
        assert!(viz.contains("JSON Array"));
        assert!(viz.contains("[0]:"));
    }

    #[test]
    fn test_json_truncation() {
        let json_str = r#"{"a": 1, "b": 2, "c": 3, "d": 4, "e": 5, "f": 6}"#;
        let adapter = JsonDisplayAdapter::new(json_str.to_string()).unwrap();
        let config = ComplexDisplayConfig {
            truncation_length: 3,
            show_dimensions: true,
            ..Default::default()
        };

        let truncated = adapter.format_truncated(&config);
        assert!(truncated.contains("more"));
        assert!(truncated.contains("size:"));
    }

    #[test]
    fn test_json_full_formatting() {
        let json_str = r#"{"user": {"name": "Alice", "details": {"age": 25}}}"#;
        let adapter = JsonDisplayAdapter::new(json_str.to_string()).unwrap();
        let config = ComplexDisplayConfig {
            show_dimensions: true,
            ..Default::default()
        };

        let full = adapter.format_full(&config);
        assert!(full.contains("JSON ("));
        assert!(full.contains("elements"));
        assert!(full.contains("depth"));
        assert!(full.contains("Alice"));
    }
}
