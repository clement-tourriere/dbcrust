//! Vector visualization system for PostgreSQL extension types
//!
//! This module provides smart formatting for large vectors from PostgreSQL extensions
//! like pgvector, with multiple display modes optimized for different use cases.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, RwLock};

/// Global vector display configuration
static GLOBAL_VECTOR_CONFIG: std::sync::OnceLock<Arc<RwLock<VectorDisplayConfig>>> =
    std::sync::OnceLock::new();

/// Set the global vector display configuration
pub fn set_global_vector_config(config: VectorDisplayConfig) {
    let global_config =
        GLOBAL_VECTOR_CONFIG.get_or_init(|| Arc::new(RwLock::new(VectorDisplayConfig::default())));

    if let Ok(mut global) = global_config.write() {
        *global = config;
    }
}

/// Get the global vector display configuration
pub fn get_global_vector_config() -> VectorDisplayConfig {
    let global_config =
        GLOBAL_VECTOR_CONFIG.get_or_init(|| Arc::new(RwLock::new(VectorDisplayConfig::default())));

    global_config
        .read()
        .map(|config| config.clone())
        .unwrap_or_default()
}

/// Display modes for vector visualization
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VectorDisplayMode {
    /// Show all elements in matrix-style layout
    Full,
    /// Show first/last N elements with ellipsis
    Truncated,
    /// Show statistical summary
    Summary,
    /// Interactive visualization
    Viz,
}

impl fmt::Display for VectorDisplayMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VectorDisplayMode::Full => write!(f, "full"),
            VectorDisplayMode::Truncated => write!(f, "truncated"),
            VectorDisplayMode::Summary => write!(f, "summary"),
            VectorDisplayMode::Viz => write!(f, "viz"),
        }
    }
}

impl FromStr for VectorDisplayMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "full" => Ok(VectorDisplayMode::Full),
            "truncated" | "trunc" => Ok(VectorDisplayMode::Truncated),
            "summary" | "stats" => Ok(VectorDisplayMode::Summary),
            "viz" => Ok(VectorDisplayMode::Viz),
            _ => Err(format!(
                "Invalid vector display mode: '{}'. Valid modes: full, truncated, summary, viz",
                s
            )),
        }
    }
}

impl VectorDisplayMode {
    pub fn all_modes() -> Vec<&'static str> {
        vec!["full", "truncated", "summary", "viz"]
    }
}

/// Configuration for vector display formatting
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VectorDisplayConfig {
    /// Default display mode for vectors
    pub display_mode: VectorDisplayMode,
    /// Number of elements to show at start/end when truncated
    pub truncation_length: usize,
    /// Width of ASCII visualization
    pub viz_width: usize,
    /// Whether to show summary statistics with other modes
    pub show_statistics: bool,
    /// Automatically switch to truncated mode above this dimension count
    pub dimension_threshold: usize,
    /// Show dimension count in all modes
    pub show_dimensions: bool,
    /// Number of elements per row in full mode matrix layout
    pub full_elements_per_row: usize,
    /// Whether to show row numbers in full mode matrix layout
    pub full_show_row_numbers: bool,
}

impl Default for VectorDisplayConfig {
    fn default() -> Self {
        Self {
            display_mode: VectorDisplayMode::Truncated,
            truncation_length: 5,
            viz_width: 40,
            show_statistics: false,
            dimension_threshold: 20,
            show_dimensions: true,
            full_elements_per_row: 8,
            full_show_row_numbers: true,
        }
    }
}

/// Vector statistics for summary display
#[derive(Debug, Clone)]
pub struct VectorStats {
    pub dimensions: usize,
    pub min: f32,
    pub max: f32,
    pub mean: f32,
    pub std_dev: f32,
    pub l2_norm: f32,
    pub non_zero_count: usize,
    // Enhanced distribution information
    pub percentile_25: f32,
    pub percentile_50: f32, // median
    pub percentile_75: f32,
    pub zero_ratio: f32,
    pub sparsity: f32, // ratio of non-zero elements
    pub range: f32,
    pub coefficient_of_variation: f32,
}

impl VectorStats {
    pub fn compute(values: &[f32]) -> Self {
        let dimensions = values.len();

        if dimensions == 0 {
            return Self {
                dimensions: 0,
                min: 0.0,
                max: 0.0,
                mean: 0.0,
                std_dev: 0.0,
                l2_norm: 0.0,
                non_zero_count: 0,
                percentile_25: 0.0,
                percentile_50: 0.0,
                percentile_75: 0.0,
                zero_ratio: 0.0,
                sparsity: 0.0,
                range: 0.0,
                coefficient_of_variation: 0.0,
            };
        }

        let min = values.iter().fold(f32::INFINITY, |a, &b| a.min(b));
        let max = values.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
        let sum: f32 = values.iter().sum();
        let mean = sum / dimensions as f32;

        let variance = values.iter().map(|&x| (x - mean).powi(2)).sum::<f32>() / dimensions as f32;
        let std_dev = variance.sqrt();

        let l2_norm = values.iter().map(|&x| x * x).sum::<f32>().sqrt();
        let non_zero_count = values.iter().filter(|&&x| x != 0.0).count();

        // Calculate percentiles (requires sorted values)
        let mut sorted_values = values.to_vec();
        sorted_values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let percentile_25 = Self::percentile(&sorted_values, 0.25);
        let percentile_50 = Self::percentile(&sorted_values, 0.50); // median
        let percentile_75 = Self::percentile(&sorted_values, 0.75);

        // Calculate additional metrics
        let zero_count = dimensions - non_zero_count;
        let zero_ratio = zero_count as f32 / dimensions as f32;
        let sparsity = non_zero_count as f32 / dimensions as f32;
        let range = max - min;
        let coefficient_of_variation = if mean != 0.0 {
            std_dev / mean.abs()
        } else {
            0.0
        };

        Self {
            dimensions,
            min,
            max,
            mean,
            std_dev,
            l2_norm,
            non_zero_count,
            percentile_25,
            percentile_50,
            percentile_75,
            zero_ratio,
            sparsity,
            range,
            coefficient_of_variation,
        }
    }

    /// Calculate percentile from sorted values
    fn percentile(sorted_values: &[f32], p: f32) -> f32 {
        if sorted_values.is_empty() {
            return 0.0;
        }

        if sorted_values.len() == 1 {
            return sorted_values[0];
        }

        let index = p * (sorted_values.len() - 1) as f32;
        let lower = index.floor() as usize;
        let upper = index.ceil() as usize;

        if lower == upper {
            sorted_values[lower]
        } else {
            let weight = index - lower as f32;
            sorted_values[lower] * (1.0 - weight) + sorted_values[upper] * weight
        }
    }
}

/// ASCII visualization characters for display
const SPARKLINE_CHARS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Main vector formatter with multiple display modes
pub struct VectorFormatter<'a> {
    config: &'a VectorDisplayConfig,
}

impl<'a> VectorFormatter<'a> {
    pub fn new(config: &'a VectorDisplayConfig) -> Self {
        Self { config }
    }

    /// Format vector values according to configuration
    pub fn format(&self, values: &[f32]) -> String {
        let stats = VectorStats::compute(values);
        let effective_mode = self.get_effective_mode(&stats);

        match effective_mode {
            VectorDisplayMode::Full => self.format_full(values, &stats),
            VectorDisplayMode::Truncated => self.format_truncated(values, &stats),
            VectorDisplayMode::Summary => self.format_summary(&stats),
            VectorDisplayMode::Viz => self.format_viz(values, &stats),
        }
    }

    /// Format half precision vectors
    pub fn format_half(&self, values: &[half::f16]) -> String {
        let f32_values: Vec<f32> = values.iter().map(|&x| x.to_f32()).collect();
        self.format(&f32_values)
    }

    /// Format sparse vectors
    pub fn format_sparse(&self, indices: &[i32], values: &[f32]) -> String {
        let pairs: Vec<String> = indices
            .iter()
            .zip(values.iter())
            .map(|(i, v)| format!("{i}:{v:.3}"))
            .collect();

        let content = if pairs.len() > self.config.truncation_length * 2 {
            let start_pairs = &pairs[..self.config.truncation_length];
            let end_pairs = &pairs[pairs.len() - self.config.truncation_length..];
            format!(
                "{{{}, ..., {}}}",
                start_pairs.join(","),
                end_pairs.join(",")
            )
        } else {
            format!("{{{}}}", pairs.join(","))
        };

        if self.config.show_dimensions {
            format!("{} ({} non-zero)", content, pairs.len())
        } else {
            content
        }
    }

    /// Determine effective display mode based on vector size and configuration
    fn get_effective_mode(&self, stats: &VectorStats) -> &VectorDisplayMode {
        if stats.dimensions > self.config.dimension_threshold
            && self.config.display_mode == VectorDisplayMode::Full
        {
            &VectorDisplayMode::Truncated
        } else {
            &self.config.display_mode
        }
    }

    /// Format vector in full mode (matrix-style layout with all elements)
    fn format_full(&self, values: &[f32], stats: &VectorStats) -> String {
        if values.is_empty() {
            return if self.config.show_dimensions {
                "[] (0d)".to_string()
            } else {
                "[]".to_string()
            };
        }

        let elements_per_row = self.config.full_elements_per_row;
        let show_row_numbers = self.config.full_show_row_numbers;

        // Calculate formatting width based on the data
        let max_val_str_len = values
            .iter()
            .map(|&v| format!("{v:.3}").len())
            .max()
            .unwrap_or(6);
        let element_width = max_val_str_len.max(6); // Minimum 6 chars

        let mut result = Vec::new();

        // Add header with dimension info if enabled
        if self.config.show_dimensions {
            result.push(format!("Vector({}d):", stats.dimensions));
        }

        // Process rows
        for (row_idx, chunk) in values.chunks(elements_per_row).enumerate() {
            let mut row = String::new();

            // Add row number if enabled
            if show_row_numbers {
                let start_idx = row_idx * elements_per_row;
                row.push_str(&format!("[{start_idx:3}]: "));
            }

            // Add elements in this row
            let formatted_elements: Vec<String> = chunk
                .iter()
                .map(|&val| format!("{val:>element_width$.3}"))
                .collect();

            row.push_str(&formatted_elements.join(" "));
            result.push(row);
        }

        result.join("\n")
    }

    /// Format vector in truncated mode
    fn format_truncated(&self, values: &[f32], stats: &VectorStats) -> String {
        let len = values.len();
        let trunc_len = self.config.truncation_length;

        let content = if len <= trunc_len * 2 + 3 {
            // Show all if not much longer than truncation would be
            let elements: Vec<String> = values.iter().map(|v| format!("{v:.3}")).collect();
            format!("[{}]", elements.join(","))
        } else {
            // Show start and end with ellipsis
            let start: Vec<String> = values[..trunc_len]
                .iter()
                .map(|v| format!("{v:.3}"))
                .collect();
            let end: Vec<String> = values[len - trunc_len..]
                .iter()
                .map(|v| format!("{v:.3}"))
                .collect();
            format!("[{}, ..., {}]", start.join(","), end.join(","))
        };

        if self.config.show_dimensions {
            format!("{} ({}d)", content, stats.dimensions)
        } else {
            content
        }
    }

    /// Format vector as summary statistics
    fn format_summary(&self, stats: &VectorStats) -> String {
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
            // Second line - percentiles and IQR
            stats.percentile_25,
            stats.percentile_50,
            stats.percentile_75,
            stats.percentile_75 - stats.percentile_25, // IQR
            // Third line - distribution info
            stats.sparsity * 100.0,
            stats.non_zero_count,
            stats.coefficient_of_variation,
            stats.range
        )
    }

    /// Format vector as ASCII visualization
    fn format_viz(&self, values: &[f32], stats: &VectorStats) -> String {
        if values.is_empty() {
            return "[] (0d)".to_string();
        }

        let viz = self.generate_viz(values);

        let base = format!("[{viz}]");

        let mut result = if self.config.show_dimensions {
            format!("{} ({}d)", base, stats.dimensions)
        } else {
            base
        };

        if self.config.show_statistics {
            result.push_str(&format!(" μ={:.2} σ={:.2}", stats.mean, stats.std_dev));
        }

        result
    }

    /// Generate ASCII visualization from values
    fn generate_viz(&self, values: &[f32]) -> String {
        if values.is_empty() {
            return String::new();
        }

        let target_width = self.config.viz_width.min(values.len());

        // Subsample if vector is longer than target width
        let sampled: Vec<f32> = if values.len() > target_width {
            let step = values.len() as f32 / target_width as f32;
            (0..target_width)
                .map(|i| {
                    let idx = (i as f32 * step) as usize;
                    values[idx.min(values.len() - 1)]
                })
                .collect()
        } else {
            values.to_vec()
        };

        // Find range for normalization
        let min_val = sampled.iter().fold(f32::INFINITY, |a, &b| a.min(b));
        let max_val = sampled.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
        let range = max_val - min_val;

        if range == 0.0 {
            // All values are the same
            return SPARKLINE_CHARS[SPARKLINE_CHARS.len() / 2]
                .to_string()
                .repeat(sampled.len());
        }

        // Map values to visualization characters
        sampled
            .iter()
            .map(|&val| {
                let normalized = (val - min_val) / range;
                let char_idx = (normalized * (SPARKLINE_CHARS.len() - 1) as f32).round() as usize;
                SPARKLINE_CHARS[char_idx.min(SPARKLINE_CHARS.len() - 1)]
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_stats_computation() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let stats = VectorStats::compute(&values);

        assert_eq!(stats.dimensions, 5);
        assert_eq!(stats.min, 1.0);
        assert_eq!(stats.max, 5.0);
        assert_eq!(stats.mean, 3.0);
        assert_eq!(stats.non_zero_count, 5);
        assert!((stats.l2_norm - 7.416).abs() < 0.01); // sqrt(55)
    }

    #[test]
    fn test_vector_display_mode_parsing() {
        assert_eq!("full".parse(), Ok(VectorDisplayMode::Full));
        assert_eq!("truncated".parse(), Ok(VectorDisplayMode::Truncated));
        assert_eq!("trunc".parse(), Ok(VectorDisplayMode::Truncated));
        assert_eq!("summary".parse(), Ok(VectorDisplayMode::Summary));
        assert_eq!("viz".parse(), Ok(VectorDisplayMode::Viz));
        assert!("invalid".parse::<VectorDisplayMode>().is_err());
    }

    #[test]
    fn test_truncated_formatting() {
        let config = VectorDisplayConfig {
            display_mode: VectorDisplayMode::Truncated,
            truncation_length: 2,
            show_dimensions: true,
            ..Default::default()
        };
        let formatter = VectorFormatter::new(&config);

        let large_vector: Vec<f32> = (0..10).map(|i| i as f32).collect();
        let result = formatter.format(&large_vector);

        assert!(result.contains("[0.000,1.000, ..., 8.000,9.000]"));
        assert!(result.contains("(10d)"));
    }

    #[test]
    fn test_viz_generation() {
        let config = VectorDisplayConfig {
            display_mode: VectorDisplayMode::Viz,
            viz_width: 5,
            show_dimensions: true,
            show_statistics: false,
            ..Default::default()
        };
        let formatter = VectorFormatter::new(&config);

        let values = vec![0.0, 0.25, 0.5, 0.75, 1.0];
        let result = formatter.format(&values);

        assert!(result.starts_with('['));
        assert!(result.contains("] (5d)"));
        // Should contain various viz characters
        let viz_part = result.split('[').nth(1).unwrap().split(']').next().unwrap();
        assert_eq!(viz_part.chars().count(), 5);
    }

    #[test]
    fn test_summary_formatting() {
        let config = VectorDisplayConfig {
            display_mode: VectorDisplayMode::Summary,
            ..Default::default()
        };
        let formatter = VectorFormatter::new(&config);

        let values = vec![1.0, 2.0, 3.0];
        let result = formatter.format(&values);

        assert!(result.starts_with("Vector(3d):"));
        assert!(result.contains("[1.000..3.000]"));
        assert!(result.contains("μ=2.000"));
        assert!(result.contains("Q1="));
        assert!(result.contains("Q2="));
        assert!(result.contains("Q3="));
        assert!(result.contains("non-zero"));
        assert!(result.contains("CV="));
    }

    #[test]
    fn test_enhanced_summary_with_distribution_info() {
        let config = VectorDisplayConfig {
            display_mode: VectorDisplayMode::Summary,
            ..Default::default()
        };
        let formatter = VectorFormatter::new(&config);

        // Test with a vector that has interesting distribution properties
        let values = vec![0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 10.0]; // Some zeros, some spread
        let result = formatter.format(&values);

        println!("Enhanced summary output:\n{}", result);

        // Check basic structure
        assert!(result.starts_with("Vector(8d):"));

        // Check range format [min..max]
        assert!(result.contains("[0.000..10.000]"));

        // Check percentiles are shown
        assert!(result.contains("Percentiles:"));
        assert!(result.contains("Q1="));
        assert!(result.contains("Q2="));
        assert!(result.contains("Q3="));
        assert!(result.contains("IQR="));

        // Check distribution info
        assert!(result.contains("Distribution:"));
        assert!(result.contains("% non-zero"));
        assert!(result.contains("CV="));
        assert!(result.contains("range="));

        // Verify the summary is multi-line
        assert!(result.contains('\n'));
        let lines: Vec<&str> = result.split('\n').collect();
        assert_eq!(lines.len(), 3, "Summary should have exactly 3 lines");
    }

    #[test]
    fn test_full_mode_matrix_formatting() {
        let config = VectorDisplayConfig {
            display_mode: VectorDisplayMode::Full,
            full_elements_per_row: 4,
            full_show_row_numbers: true,
            show_dimensions: true,
            ..Default::default()
        };
        let formatter = VectorFormatter::new(&config);

        // Test with a vector that will span multiple rows
        let values: Vec<f32> = (0..10).map(|i| i as f32 * 0.5).collect();
        let result = formatter.format(&values);

        println!("Full mode matrix output:\n{}", result);

        // Check that it contains header
        assert!(result.starts_with("Vector(10d):"));

        // Check that it contains row numbers
        assert!(result.contains("[  0]:"));
        assert!(result.contains("[  4]:"));
        assert!(result.contains("[  8]:"));

        // Check that values are formatted with proper precision
        assert!(result.contains("0.000"));
        assert!(result.contains("0.500"));
        assert!(result.contains("4.500"));

        // Verify it's multi-line
        assert!(result.contains('\n'));
        let lines: Vec<&str> = result.split('\n').collect();
        assert!(lines.len() >= 3, "Should have header + multiple rows");
    }

    #[test]
    fn test_full_mode_without_row_numbers() {
        let config = VectorDisplayConfig {
            display_mode: VectorDisplayMode::Full,
            full_elements_per_row: 3,
            full_show_row_numbers: false,
            show_dimensions: false,
            ..Default::default()
        };
        let formatter = VectorFormatter::new(&config);

        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let result = formatter.format(&values);

        println!("Full mode without row numbers:\n{}", result);

        // Should not contain dimension header or row numbers
        assert!(!result.contains("Vector("));
        assert!(!result.contains("["));

        // Should still contain the values formatted nicely
        assert!(result.contains("1.000"));
        assert!(result.contains("2.000"));
        assert!(result.contains("5.000"));

        // Should be multi-line (5 elements, 3 per row = 2 rows)
        let lines: Vec<&str> = result.split('\n').collect();
        assert_eq!(lines.len(), 2, "Should have exactly 2 rows");
    }

    #[test]
    fn test_sparse_vector_formatting() {
        let config = VectorDisplayConfig::default();
        let formatter = VectorFormatter::new(&config);

        let indices = vec![0, 5, 12];
        let values = vec![1.0, 2.0, 0.5];
        let result = formatter.format_sparse(&indices, &values);

        assert!(result.contains("{0:1.000,5:2.000,12:0.500}"));
        assert!(result.contains("(3 non-zero)"));
    }
}
