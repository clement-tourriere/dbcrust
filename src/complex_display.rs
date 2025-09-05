//! Unified display system for complex PostgreSQL data types
//!
//! This module provides a consistent trait-based approach for displaying
//! complex data types like vectors, JSON, GeoJSON, arrays, and other
//! PostgreSQL extension types with multiple display modes.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::{Arc, RwLock};

/// Detected complex data types that can use specialized display
#[derive(Debug, Clone, PartialEq)]
pub enum ComplexDataType {
    /// JSON object or array
    Json,
    /// GeoJSON geographic data
    GeoJson,
    /// Vector of numbers (from pgvector or similar)
    Vector,
    /// Array of values (from ClickHouse, MongoDB, etc.)
    Array,
    /// Key-value map/object (from ClickHouse Map, etc.)
    Map,
    /// Tuple of heterogeneous values (from ClickHouse)
    Tuple,
}

/// Display modes available for complex data types
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComplexDisplayMode {
    /// Show raw/full content
    Full,
    /// Show abbreviated content with truncation
    Truncated,
    /// Show statistical or structural summary
    Summary,
    /// Show interactive visualization
    Viz,
}

impl fmt::Display for ComplexDisplayMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ComplexDisplayMode::Full => write!(f, "full"),
            ComplexDisplayMode::Truncated => write!(f, "truncated"),
            ComplexDisplayMode::Summary => write!(f, "summary"),
            ComplexDisplayMode::Viz => write!(f, "viz"),
        }
    }
}

impl ComplexDisplayMode {
    pub fn from_str(s: &str) -> Option<ComplexDisplayMode> {
        match s.to_lowercase().as_str() {
            "full" => Some(ComplexDisplayMode::Full),
            "truncated" | "trunc" => Some(ComplexDisplayMode::Truncated),
            "summary" | "stats" => Some(ComplexDisplayMode::Summary),
            "viz" => Some(ComplexDisplayMode::Viz),
            _ => None,
        }
    }

    pub fn all_modes() -> Vec<&'static str> {
        vec!["full", "truncated", "summary", "viz"]
    }
}

/// Configuration for complex data display
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ComplexDisplayConfig {
    /// Default display mode
    pub display_mode: ComplexDisplayMode,
    /// Maximum elements to show in truncated mode
    pub truncation_length: usize,
    /// Width for visualization modes
    pub viz_width: usize,
    /// Whether to show metadata/statistics
    pub show_metadata: bool,
    /// Threshold for auto-switching to truncated mode
    pub size_threshold: usize,
    /// Show size/dimension information
    pub show_dimensions: bool,
    /// Elements per row in full mode
    pub full_elements_per_row: usize,
    /// Maximum width for display
    pub max_width: usize,
    /// Show row/field numbers in full mode
    pub full_show_numbers: bool,
    /// Whether to pretty-print JSON (false = compact, true = formatted)
    pub json_pretty_print: bool,
}

impl Default for ComplexDisplayConfig {
    fn default() -> Self {
        Self {
            display_mode: ComplexDisplayMode::Truncated,
            truncation_length: 5,
            viz_width: 40,
            show_metadata: false,
            size_threshold: 20,
            show_dimensions: true,
            full_elements_per_row: 8,
            max_width: 80,
            full_show_numbers: true,
            json_pretty_print: false,
        }
    }
}

/// Global complex display configuration
static GLOBAL_COMPLEX_CONFIG: std::sync::OnceLock<Arc<RwLock<ComplexDisplayConfig>>> =
    std::sync::OnceLock::new();

/// Set the global complex display configuration
pub fn set_global_complex_config(config: ComplexDisplayConfig) {
    let global_config = GLOBAL_COMPLEX_CONFIG
        .get_or_init(|| Arc::new(RwLock::new(ComplexDisplayConfig::default())));

    if let Ok(mut global) = global_config.write() {
        *global = config;
    }
}

/// Get the global complex display configuration
pub fn get_global_complex_config() -> ComplexDisplayConfig {
    let global_config = GLOBAL_COMPLEX_CONFIG
        .get_or_init(|| Arc::new(RwLock::new(ComplexDisplayConfig::default())));

    global_config
        .read()
        .map(|config| config.clone())
        .unwrap_or_default()
}

/// Metadata about complex data structure
#[derive(Debug, Clone)]
pub struct ComplexDataMetadata {
    pub data_type: String,
    pub size: usize,
    pub depth: Option<usize>,
    pub has_nested: bool,
    pub schema_info: Option<String>,
}

/// Core trait for displaying complex PostgreSQL data types
pub trait ComplexDataDisplay {
    /// Get metadata about the data structure
    fn metadata(&self) -> ComplexDataMetadata;

    /// Format in full mode (complete data)
    fn format_full(&self, config: &ComplexDisplayConfig) -> String;

    /// Format in truncated mode (abbreviated)
    fn format_truncated(&self, config: &ComplexDisplayConfig) -> String;

    /// Format as summary (statistics/structure info)
    fn format_summary(&self, config: &ComplexDisplayConfig) -> String;

    /// Format as visualization (charts, graphs, etc.)
    fn format_viz(&self, config: &ComplexDisplayConfig) -> String;

    /// Main formatting method that delegates based on mode
    fn format(&self, config: &ComplexDisplayConfig) -> String {
        let metadata = self.metadata();
        let effective_mode = get_effective_mode(&config.display_mode, &metadata, config);

        match effective_mode {
            ComplexDisplayMode::Full => self.format_full(config),
            ComplexDisplayMode::Truncated => self.format_truncated(config),
            ComplexDisplayMode::Summary => self.format_summary(config),
            ComplexDisplayMode::Viz => self.format_viz(config),
        }
    }
}

/// Determine effective display mode based on data size and configuration
fn get_effective_mode(
    requested_mode: &ComplexDisplayMode,
    _metadata: &ComplexDataMetadata,
    _config: &ComplexDisplayConfig,
) -> ComplexDisplayMode {
    // Always respect the user's explicit mode choice
    // The user can choose to override size-based auto-switching by explicitly setting a mode
    requested_mode.clone()
}

/// Helper trait for data types that can be parsed from strings
pub trait ComplexDataParser<T> {
    type Error;

    /// Parse raw string data into structured format
    fn parse(raw_data: &str) -> Result<T, Self::Error>;

    /// Validate that the data is well-formed
    fn validate(raw_data: &str) -> bool;

    /// Get schema information if available
    fn schema_info(raw_data: &str) -> Option<String>;
}

/// Trait for detecting complex data types from raw string values
pub trait ComplexTypeDetector {
    /// Detect if a value represents a complex data type
    fn detect_type(value: &str) -> Option<ComplexDataType>;

    /// Check if a column should use complex display based on name and value
    fn should_use_complex_display(column_name: &str, value: &str) -> bool;
}

/// Generic implementation of complex type detection
pub struct GenericComplexTypeDetector;

impl ComplexTypeDetector for GenericComplexTypeDetector {
    fn detect_type(value: &str) -> Option<ComplexDataType> {
        let trimmed = value.trim();

        // Empty or null values
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
            return None;
        }

        // Handle curly brace syntax
        if trimmed.starts_with('{') && trimmed.ends_with('}') {
            // Check for PostgreSQL array format first
            if is_postgresql_array_like(trimmed) {
                return Some(ComplexDataType::Array);
            }
            // Check for GeoJSON patterns
            if is_geojson_like(trimmed) {
                return Some(ComplexDataType::GeoJson);
            }
            // Default to JSON object
            return Some(ComplexDataType::Json);
        }

        // JSON array detection
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            // Vector detection - array of numbers
            if is_vector_like(trimmed) {
                return Some(ComplexDataType::Vector);
            }
            return Some(ComplexDataType::Array);
        }

        // Check for vector representations without brackets (space/comma separated numbers)
        if is_bare_vector_like(trimmed) {
            return Some(ComplexDataType::Vector);
        }

        None
    }

    fn should_use_complex_display(column_name: &str, value: &str) -> bool {
        let column_lower = column_name.to_lowercase();

        // Column name hints
        let has_hint = column_lower.contains("json")
            || column_lower.contains("geojson")
            || column_lower.contains("vector")
            || column_lower.contains("array")
            || column_lower.contains("coordinates")
            || column_lower.contains("geometry")
            || column_lower.contains("geom")  // Also match shorter "geom"
            || column_lower.contains("location");

        // Always check value content
        let detected_type = Self::detect_type(value);

        has_hint || detected_type.is_some()
    }
}

/// Check if a JSON string looks like GeoJSON
fn is_geojson_like(json_str: &str) -> bool {
    // Look for GeoJSON type indicators
    json_str.contains(r#""type""#)
        && (json_str.contains(r#""Feature""#)
            || json_str.contains(r#""FeatureCollection""#)
            || json_str.contains(r#""Point""#)
            || json_str.contains(r#""LineString""#)
            || json_str.contains(r#""Polygon""#)
            || json_str.contains(r#""MultiPoint""#)
            || json_str.contains(r#""MultiLineString""#)
            || json_str.contains(r#""MultiPolygon""#)
            || json_str.contains(r#""GeometryCollection""#))
}

/// Check if an array string looks like a vector (array of numbers)
fn is_vector_like(array_str: &str) -> bool {
    // Remove brackets and split by comma
    let inner = array_str.trim_start_matches('[').trim_end_matches(']');
    if inner.trim().is_empty() {
        return false;
    }

    let parts: Vec<&str> = inner.split(',').collect();
    if parts.len() < 2 {
        return false;
    }

    // Check if all parts are numbers
    parts.iter().all(|part| part.trim().parse::<f64>().is_ok())
}

/// Check if a bare string looks like a vector (space or comma separated numbers)
fn is_bare_vector_like(value: &str) -> bool {
    if value.len() > 200 {
        // Long values are likely vectors
        // Try space separation first
        let parts: Vec<&str> = value.split_whitespace().collect();
        if parts.len() > 10 {
            return parts
                .iter()
                .take(10)
                .all(|part| part.parse::<f64>().is_ok());
        }

        // Try comma separation
        let parts: Vec<&str> = value.split(',').collect();
        if parts.len() > 10 {
            return parts
                .iter()
                .take(10)
                .all(|part| part.trim().parse::<f64>().is_ok());
        }
    }

    false
}

/// Check if a string looks like a PostgreSQL array (e.g., {1,2,3} or {"a","b","c"})
fn is_postgresql_array_like(value: &str) -> bool {
    if !value.starts_with('{') || !value.ends_with('}') {
        return false;
    }

    let inner = value.trim_start_matches('{').trim_end_matches('}').trim();

    // Empty array
    if inner.is_empty() {
        return true;
    }

    // PostgreSQL arrays don't contain JSON object syntax like key-value pairs
    // If it has colons, it's likely a JSON object
    if inner.contains(':') {
        return false;
    }

    // Check if it looks like comma-separated values (typical array format)
    let parts: Vec<&str> = inner.split(',').collect();
    if parts.len() > 1 {
        return true;
    }

    // Single element arrays are also valid
    true
}

/// Adapter to convert vector display to the new unified system
pub struct VectorDisplayAdapter<'a> {
    pub values: &'a [f32],
}

impl<'a> ComplexDataDisplay for VectorDisplayAdapter<'a> {
    fn metadata(&self) -> ComplexDataMetadata {
        ComplexDataMetadata {
            data_type: "vector".to_string(),
            size: self.values.len(),
            depth: None,
            has_nested: false,
            schema_info: Some(format!("f32[{}]", self.values.len())),
        }
    }

    fn format_full(&self, config: &ComplexDisplayConfig) -> String {
        use crate::vector_display::{VectorDisplayConfig, VectorDisplayMode, VectorFormatter};

        // Convert to vector display config with full mode
        let vec_config = VectorDisplayConfig {
            display_mode: VectorDisplayMode::Full,
            full_elements_per_row: config.full_elements_per_row,
            full_show_row_numbers: config.full_show_numbers,
            show_dimensions: config.show_dimensions,
            ..Default::default()
        };

        let formatter = VectorFormatter::new(&vec_config);
        formatter.format(self.values)
    }

    fn format_truncated(&self, config: &ComplexDisplayConfig) -> String {
        let len = self.values.len();
        let trunc_len = config.truncation_length;

        let content = if len <= trunc_len * 2 + 3 {
            let elements: Vec<String> = self.values.iter().map(|v| format!("{:.3}", v)).collect();
            format!("[{}]", elements.join(","))
        } else {
            let start: Vec<String> = self.values[..trunc_len]
                .iter()
                .map(|v| format!("{:.3}", v))
                .collect();
            let end: Vec<String> = self.values[len - trunc_len..]
                .iter()
                .map(|v| format!("{:.3}", v))
                .collect();
            format!("[{}, ..., {}]", start.join(","), end.join(","))
        };

        if config.show_dimensions {
            format!("{} ({}d)", content, len)
        } else {
            content
        }
    }

    fn format_summary(&self, _config: &ComplexDisplayConfig) -> String {
        use crate::vector_display::VectorStats;
        let stats = VectorStats::compute(self.values);

        format!(
            "Vector({}d): [{:.3}..{:.3}] μ={:.3}±{:.3} norm={:.3}\n\
             Percentiles: Q1={:.3} Q2={:.3} Q3={:.3} | IQR={:.3}\n\
             Distribution: {:.1}% non-zero ({}) | CV={:.3} | range={:.3}",
            stats.dimensions,
            stats.min,
            stats.max,
            stats.mean,
            stats.std_dev,
            stats.l2_norm,
            stats.percentile_25,
            stats.percentile_50,
            stats.percentile_75,
            stats.percentile_75 - stats.percentile_25,
            stats.sparsity * 100.0,
            stats.non_zero_count,
            stats.coefficient_of_variation,
            stats.range
        )
    }

    fn format_viz(&self, config: &ComplexDisplayConfig) -> String {
        use crate::vector_display::{VectorDisplayConfig, VectorDisplayMode, VectorFormatter};

        // Convert to vector display config with viz mode
        let vec_config = VectorDisplayConfig {
            display_mode: VectorDisplayMode::Viz,
            viz_width: config.viz_width,
            show_dimensions: config.show_dimensions,
            show_statistics: config.show_metadata,
            ..Default::default()
        };

        let formatter = VectorFormatter::new(&vec_config);
        formatter.format(self.values)
    }
}

/// Adapter for displaying arrays of mixed data types
pub struct ArrayDisplayAdapter {
    pub elements: Vec<String>,
    pub element_type_hint: Option<String>,
}

impl ArrayDisplayAdapter {
    pub fn new(elements: Vec<String>) -> Self {
        Self {
            elements,
            element_type_hint: None,
        }
    }

    pub fn with_type_hint(mut self, type_hint: String) -> Self {
        self.element_type_hint = Some(type_hint);
        self
    }

    /// Try to parse from JSON array string
    pub fn from_json_string(json_str: &str) -> Result<Self, serde_json::Error> {
        let array: Vec<serde_json::Value> = serde_json::from_str(json_str)?;
        let elements: Vec<String> = array
            .into_iter()
            .map(|v| match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            })
            .collect();

        Ok(Self::new(elements))
    }
}

impl ComplexDataDisplay for ArrayDisplayAdapter {
    fn metadata(&self) -> ComplexDataMetadata {
        // Analyze element types
        let mut type_counts = std::collections::HashMap::new();
        for element in &self.elements {
            let element_type = if element.parse::<f64>().is_ok() {
                "Number"
            } else if element == "true" || element == "false" {
                "Boolean"
            } else if element.starts_with('{') && element.ends_with('}') {
                "Object"
            } else if element.starts_with('[') && element.ends_with(']') {
                "Array"
            } else {
                "String"
            };

            *type_counts.entry(element_type).or_insert(0) += 1;
        }

        let type_summary = if type_counts.len() == 1 {
            format!(
                "{}[{}]",
                type_counts.keys().next().unwrap(),
                self.elements.len()
            )
        } else {
            let types: Vec<String> = type_counts
                .into_iter()
                .map(|(t, c)| {
                    if c == 1 {
                        t.to_string()
                    } else {
                        format!("{}×{}", t, c)
                    }
                })
                .collect();
            format!("Mixed[{}]", types.join("|"))
        };

        ComplexDataMetadata {
            data_type: "array".to_string(),
            size: self.elements.len(),
            depth: Some(1),
            has_nested: self
                .elements
                .iter()
                .any(|e| e.starts_with('{') || e.starts_with('[')),
            schema_info: Some(type_summary),
        }
    }

    fn format_full(&self, config: &ComplexDisplayConfig) -> String {
        if self.elements.is_empty() {
            return "[]".to_string();
        }

        let mut result = String::new();

        if config.show_dimensions {
            let metadata = self.metadata();
            result.push_str(&format!(
                "Array ({}):\n",
                metadata
                    .schema_info
                    .unwrap_or_else(|| format!("{} elements", self.elements.len()))
            ));
        }

        // Use matrix-style layout for better readability
        let elements_per_row = config.full_elements_per_row.min(5); // Arrays get fewer per row

        for (i, chunk) in self.elements.chunks(elements_per_row).enumerate() {
            if config.full_show_numbers {
                result.push_str(&format!("[{:3}]: ", i * elements_per_row));
            }

            let formatted_elements: Vec<String> = chunk
                .iter()
                .map(|element| {
                    if element.len() > 20 {
                        format!("{}...", &element[..17])
                    } else {
                        element.clone()
                    }
                })
                .collect();

            result.push_str(&formatted_elements.join("  "));
            result.push('\n');
        }

        result.trim_end().to_string()
    }

    fn format_truncated(&self, config: &ComplexDisplayConfig) -> String {
        let len = self.elements.len();
        let trunc_len = config.truncation_length;

        if len <= trunc_len * 2 + 3 {
            let elements: Vec<String> = self
                .elements
                .iter()
                .map(|e| {
                    if e.len() > 15 {
                        format!("{}...", &e[..12])
                    } else {
                        e.clone()
                    }
                })
                .collect();
            let mut result = format!("[{}]", elements.join(", "));
            if config.show_dimensions {
                result.push_str(&format!(" ({})", len));
            }
            result
        } else {
            let start: Vec<String> = self.elements[..trunc_len]
                .iter()
                .map(|e| {
                    if e.len() > 15 {
                        format!("{}...", &e[..12])
                    } else {
                        e.clone()
                    }
                })
                .collect();
            let end: Vec<String> = self.elements[len - trunc_len..]
                .iter()
                .map(|e| {
                    if e.len() > 15 {
                        format!("{}...", &e[..12])
                    } else {
                        e.clone()
                    }
                })
                .collect();
            let mut result = format!(
                "[{}, ... {} more ..., {}]",
                start.join(", "),
                len - trunc_len * 2,
                end.join(", ")
            );
            if config.show_dimensions {
                result.push_str(&format!(" ({})", len));
            }
            result
        }
    }

    fn format_summary(&self, _config: &ComplexDisplayConfig) -> String {
        let metadata = self.metadata();
        format!(
            "Array: {} | Elements: {} | Schema: {} | Nested: {}",
            metadata.schema_info.unwrap_or_else(|| "Mixed".to_string()),
            metadata.size,
            if metadata.has_nested { "Yes" } else { "No" },
            if metadata.has_nested {
                "Contains objects/arrays"
            } else {
                "Flat structure"
            }
        )
    }

    fn format_viz(&self, config: &ComplexDisplayConfig) -> String {
        if self.elements.is_empty() {
            return "Empty Array".to_string();
        }

        let mut lines = vec![format!(
            "Array Visualization ({} elements):",
            self.elements.len()
        )];
        let max_items = (config.viz_width / 8).max(5).min(10);

        for (i, element) in self.elements.iter().take(max_items).enumerate() {
            let display_value = if element.len() > 15 {
                format!("{}...", &element[..12])
            } else {
                element.clone()
            };

            lines.push(format!("  [{:2}] {}", i, display_value));
        }

        if self.elements.len() > max_items {
            lines.push(format!(
                "  ... {} more elements",
                self.elements.len() - max_items
            ));
        }

        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_complex_display_mode_parsing() {
        assert_eq!(
            ComplexDisplayMode::from_str("full"),
            Some(ComplexDisplayMode::Full)
        );
        assert_eq!(
            ComplexDisplayMode::from_str("truncated"),
            Some(ComplexDisplayMode::Truncated)
        );
        assert_eq!(
            ComplexDisplayMode::from_str("summary"),
            Some(ComplexDisplayMode::Summary)
        );
        assert_eq!(
            ComplexDisplayMode::from_str("viz"),
            Some(ComplexDisplayMode::Viz)
        );
        assert_eq!(ComplexDisplayMode::from_str("invalid"), None);
    }

    #[test]
    fn test_vector_adapter_integration() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let adapter = VectorDisplayAdapter { values: &values };
        let config = ComplexDisplayConfig::default();

        // Test metadata
        let metadata = adapter.metadata();
        assert_eq!(metadata.data_type, "vector");
        assert_eq!(metadata.size, 5);
        assert!(!metadata.has_nested);

        // Test different formats
        let full = adapter.format_full(&config);
        assert!(full.contains("1.000"));
        assert!(full.contains("(5d)"));

        let truncated = adapter.format_truncated(&config);
        assert!(truncated.contains("1.000"));
        assert!(truncated.contains("(5d)"));

        let summary = adapter.format_summary(&config);
        assert!(summary.contains("Vector(5d):"));
        assert!(summary.contains("Percentiles:"));
    }

    #[test]
    fn test_effective_mode_switching() {
        let metadata = ComplexDataMetadata {
            data_type: "test".to_string(),
            size: 50,
            depth: None,
            has_nested: false,
            schema_info: None,
        };

        let config = ComplexDisplayConfig {
            size_threshold: 20,
            ..Default::default()
        };

        // Should respect user's explicit Full mode choice (no auto-switching)
        let effective = get_effective_mode(&ComplexDisplayMode::Full, &metadata, &config);
        assert_eq!(effective, ComplexDisplayMode::Full);

        // Should preserve other modes
        let effective = get_effective_mode(&ComplexDisplayMode::Summary, &metadata, &config);
        assert_eq!(effective, ComplexDisplayMode::Summary);
    }

    #[test]
    fn test_complex_type_detection() {
        // Test JSON detection
        let json_str = r#"{"name": "John", "age": 30}"#;
        assert_eq!(
            GenericComplexTypeDetector::detect_type(json_str),
            Some(ComplexDataType::Json)
        );

        // Test vector detection (numeric arrays are detected as vectors)
        let numeric_array_str = "[1, 2, 3, 4, 5]";
        assert_eq!(
            GenericComplexTypeDetector::detect_type(numeric_array_str),
            Some(ComplexDataType::Vector)
        );

        // Test actual array detection (mixed content)
        let mixed_array_str = r#"["hello", 123, true]"#;
        assert_eq!(
            GenericComplexTypeDetector::detect_type(mixed_array_str),
            Some(ComplexDataType::Array)
        );

        // Test PostgreSQL array format
        let pg_array_str = "{1,2,3,4,5}";
        assert_eq!(
            GenericComplexTypeDetector::detect_type(pg_array_str),
            Some(ComplexDataType::Array)
        );

        // Test GeoJSON detection
        let geojson_str = r#"{"type": "Point", "coordinates": [125.6, 10.1]}"#;
        assert_eq!(
            GenericComplexTypeDetector::detect_type(geojson_str),
            Some(ComplexDataType::GeoJson)
        );
    }

    #[test]
    fn test_column_name_detection() {
        // Test vector column detection
        assert!(GenericComplexTypeDetector::should_use_complex_display(
            "embedding",
            "[0.1,0.2,0.3]"
        ));
        assert!(GenericComplexTypeDetector::should_use_complex_display(
            "vector_column",
            "[1,2,3]"
        ));

        // Test spatial column detection
        assert!(GenericComplexTypeDetector::should_use_complex_display(
            "geom",
            "POINT(1 2)"
        ));
        assert!(GenericComplexTypeDetector::should_use_complex_display(
            "location",
            "POLYGON((0 0,1 0,1 1,0 1,0 0))"
        ));

        // Test JSON column detection
        assert!(GenericComplexTypeDetector::should_use_complex_display(
            "metadata",
            r#"{"key": "value"}"#
        ));
        assert!(GenericComplexTypeDetector::should_use_complex_display(
            "config",
            r#"{"enabled": true}"#
        ));
    }

    #[test]
    fn test_global_config_management() {
        // Test setting and getting global config
        let config = ComplexDisplayConfig {
            display_mode: ComplexDisplayMode::Full,
            truncation_length: 10,
            viz_width: 60,
            show_metadata: true,
            size_threshold: 50,
            show_dimensions: false,
            full_elements_per_row: 12,
            max_width: 120,
            full_show_numbers: false,
            json_pretty_print: true,
        };

        set_global_complex_config(config.clone());
        let retrieved_config = get_global_complex_config();

        assert_eq!(retrieved_config.display_mode, ComplexDisplayMode::Full);
        assert_eq!(retrieved_config.truncation_length, 10);
        assert_eq!(retrieved_config.viz_width, 60);
        assert_eq!(retrieved_config.show_metadata, true);
    }

    #[test]
    fn test_array_display_adapter() {
        let elements = vec![
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string(),
            "5".to_string(),
        ];
        let adapter = ArrayDisplayAdapter::new(elements);
        let config = ComplexDisplayConfig::default();

        // Test basic functionality
        assert_eq!(adapter.metadata().size, 5);
        assert_eq!(adapter.metadata().data_type, "array");

        // Test format methods exist (implementation details tested in integration)
        let _truncated = adapter.format_truncated(&config);
        let _full = adapter.format_full(&config);
        let _summary = adapter.format_summary(&config);
        let _viz = adapter.format_viz(&config);
    }
}
