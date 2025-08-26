//! Unified display system for complex PostgreSQL data types
//!
//! This module provides a consistent trait-based approach for displaying
//! complex data types like vectors, JSON, GeoJSON, arrays, and other
//! PostgreSQL extension types with multiple display modes.

use serde::{Deserialize, Serialize};
use std::fmt;

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
        }
    }
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
    metadata: &ComplexDataMetadata,
    config: &ComplexDisplayConfig,
) -> ComplexDisplayMode {
    // Auto-switch to truncated mode for large data structures
    if metadata.size > config.size_threshold && requested_mode == &ComplexDisplayMode::Full {
        ComplexDisplayMode::Truncated
    } else {
        requested_mode.clone()
    }
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

        // Should auto-switch from Full to Truncated for large data
        let effective = get_effective_mode(&ComplexDisplayMode::Full, &metadata, &config);
        assert_eq!(effective, ComplexDisplayMode::Truncated);

        // Should preserve other modes
        let effective = get_effective_mode(&ComplexDisplayMode::Summary, &metadata, &config);
        assert_eq!(effective, ComplexDisplayMode::Summary);
    }
}
