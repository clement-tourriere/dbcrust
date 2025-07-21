//! Performance analyzer for database query plans
//! Provides standardized performance analysis across PostgreSQL, MySQL, and SQLite
use nu_ansi_term::Color;
use serde_json::Value as JsonValue;

/// Performance levels for color coding
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PerformanceLevel {
    Excellent,  // Green
    Good,       // Light Green
    Warning,    // Yellow
    Poor,       // Orange
    Critical,   // Red
}

impl PerformanceLevel {
    /// Get the color for this performance level
    pub fn color(&self) -> Color {
        match self {
            PerformanceLevel::Excellent => Color::Green,
            PerformanceLevel::Good => Color::LightGreen,
            PerformanceLevel::Warning => Color::Yellow,
            PerformanceLevel::Poor => Color::LightRed,
            PerformanceLevel::Critical => Color::Red,
        }
    }
    
    /// Get the emoji indicator for this performance level
    pub fn emoji(&self) -> &'static str {
        match self {
            PerformanceLevel::Excellent => "🟢",
            PerformanceLevel::Good => "🟢",
            PerformanceLevel::Warning => "🟡",
            PerformanceLevel::Poor => "🟠",
            PerformanceLevel::Critical => "🔴",
        }
    }
    
    /// Get the text description for this performance level
    pub fn description(&self) -> &'static str {
        match self {
            PerformanceLevel::Excellent => "EXCELLENT",
            PerformanceLevel::Good => "GOOD",
            PerformanceLevel::Warning => "WARNING",
            PerformanceLevel::Poor => "POOR",
            PerformanceLevel::Critical => "CRITICAL",
        }
    }
}

/// Performance metrics for a query operation
#[derive(Debug, Clone)]
pub struct PerformanceMetrics {
    pub operation_type: String,
    pub performance_level: PerformanceLevel,
    pub cost_score: f64,
    pub time_ms: Option<f64>,
    pub rows_examined: Option<u64>,
    pub rows_returned: Option<u64>,
    pub efficiency_percent: Option<f64>,
    pub recommendations: Vec<String>,
    pub warnings: Vec<String>,
}

impl PerformanceMetrics {
    pub fn new(operation_type: String) -> Self {
        Self {
            operation_type,
            performance_level: PerformanceLevel::Good,
            cost_score: 0.0,
            time_ms: None,
            rows_examined: None,
            rows_returned: None,
            efficiency_percent: None,
            recommendations: Vec::new(),
            warnings: Vec::new(),
        }
    }
    
    /// Calculate efficiency percentage from rows examined vs returned
    pub fn calculate_efficiency(&mut self) {
        if let (Some(examined), Some(returned)) = (self.rows_examined, self.rows_returned) {
            if examined > 0 {
                self.efficiency_percent = Some((returned as f64 / examined as f64) * 100.0);
            }
        }
    }
    
    /// Add a performance recommendation
    pub fn add_recommendation(&mut self, recommendation: String) {
        self.recommendations.push(recommendation);
    }
    
    /// Add a performance warning
    pub fn add_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }
}

/// Performance analyzer for different database types
pub struct PerformanceAnalyzer;

impl PerformanceAnalyzer {
    /// Analyze PostgreSQL EXPLAIN JSON output
    pub fn analyze_postgresql_plan(plan_json: &JsonValue) -> Vec<PerformanceMetrics> {
        let mut metrics = Vec::new();
        
        if let JsonValue::Array(plans) = plan_json {
            if let Some(plan) = plans.first() {
                if let Some(plan_obj) = plan.as_object() {
                    if let Some(plan_node) = plan_obj.get("Plan") {
                        Self::analyze_postgresql_node(plan_node, &mut metrics);
                    }
                }
            }
        }
        
        metrics
    }
    
    /// Recursively analyze PostgreSQL plan nodes
    fn analyze_postgresql_node(node: &JsonValue, metrics: &mut Vec<PerformanceMetrics>) {
        if let Some(node_obj) = node.as_object() {
            let mut metric = PerformanceMetrics::new(
                node_obj.get("Node Type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown")
                    .to_string()
            );
            
            // Extract timing information
            if let Some(JsonValue::Number(actual_time)) = node_obj.get("Actual Total Time") {
                metric.time_ms = actual_time.as_f64();
            }
            
            // Extract cost information
            if let Some(JsonValue::Number(total_cost)) = node_obj.get("Total Cost") {
                metric.cost_score = total_cost.as_f64().unwrap_or(0.0);
            }
            
            // Extract row information
            if let Some(JsonValue::Number(rows)) = node_obj.get("Actual Rows") {
                metric.rows_returned = rows.as_u64();
            }
            
            // Calculate performance level based on operation type and metrics
            metric.performance_level = Self::calculate_postgresql_performance_level(&metric, node_obj);
            
            // Add recommendations based on operation type
            Self::add_postgresql_recommendations(&mut metric, node_obj);
            
            metrics.push(metric);
            
            // Recursively analyze child plans
            if let Some(JsonValue::Array(plans)) = node_obj.get("Plans") {
                for plan in plans {
                    Self::analyze_postgresql_node(plan, metrics);
                }
            }
        }
    }
    
    /// Calculate performance level for PostgreSQL operations
    fn calculate_postgresql_performance_level(metric: &PerformanceMetrics, node_obj: &serde_json::Map<String, JsonValue>) -> PerformanceLevel {
        // Check for slow operations
        if metric.operation_type.contains("Seq Scan") {
            return PerformanceLevel::Warning;
        }
        
        // Check execution time thresholds
        if let Some(time) = metric.time_ms {
            if time > 1000.0 {
                return PerformanceLevel::Critical;
            } else if time > 100.0 {
                return PerformanceLevel::Warning;
            }
        }
        
        // Check cost thresholds
        if metric.cost_score > 10000.0 {
            return PerformanceLevel::Critical;
        } else if metric.cost_score > 1000.0 {
            return PerformanceLevel::Warning;
        }
        
        // Check row estimation accuracy
        if let (Some(JsonValue::Number(plan_rows)), Some(JsonValue::Number(actual_rows))) = 
            (node_obj.get("Plan Rows"), node_obj.get("Actual Rows")) {
            if let (Some(plan), Some(actual)) = (plan_rows.as_f64(), actual_rows.as_f64()) {
                if actual > 0.0 {
                    let ratio = plan / actual;
                    if !(0.1..=10.0).contains(&ratio) {
                        return PerformanceLevel::Poor;
                    }
                }
            }
        }
        
        PerformanceLevel::Good
    }
    
    /// Add PostgreSQL-specific recommendations
    fn add_postgresql_recommendations(metric: &mut PerformanceMetrics, node_obj: &serde_json::Map<String, JsonValue>) {
        match metric.operation_type.as_str() {
            "Seq Scan" => {
                metric.add_warning("Full table scan detected".to_string());
                if let Some(JsonValue::String(relation)) = node_obj.get("Relation Name") {
                    metric.add_recommendation(format!("Consider adding an index on table '{relation}'"));
                }
            },
            "Nested Loop" => {
                if let Some(time) = metric.time_ms {
                    if time > 100.0 {
                        metric.add_recommendation("Consider using a hash join instead of nested loop".to_string());
                    }
                }
            },
            "Sort" => {
                if let Some(JsonValue::String(sort_method)) = node_obj.get("Sort Method") {
                    if sort_method.contains("external") {
                        metric.add_warning("Sort spilled to disk".to_string());
                        metric.add_recommendation("Consider increasing work_mem".to_string());
                    }
                }
            },
            _ => {}
        }
    }
    
    /// Analyze MySQL EXPLAIN JSON output
    pub fn analyze_mysql_plan(plan_json: &JsonValue) -> Vec<PerformanceMetrics> {
        let mut metrics = Vec::new();
        
        if let JsonValue::Object(obj) = plan_json {
            if let Some(query_block) = obj.get("query_block") {
                Self::analyze_mysql_query_block(query_block, &mut metrics);
            }
        }
        
        metrics
    }
    
    /// Analyze MySQL query block
    fn analyze_mysql_query_block(query_block: &JsonValue, metrics: &mut Vec<PerformanceMetrics>) {
        if let Some(obj) = query_block.as_object() {
            // Analyze table information
            if let Some(table) = obj.get("table") {
                Self::analyze_mysql_table(table, metrics);
            }
            
            // Analyze nested loops
            if let Some(JsonValue::Array(nested_loop)) = obj.get("nested_loop") {
                for table in nested_loop {
                    if let Some(table_obj) = table.get("table") {
                        Self::analyze_mysql_table(table_obj, metrics);
                    }
                }
            }
        }
    }
    
    /// Analyze MySQL table information
    fn analyze_mysql_table(table: &JsonValue, metrics: &mut Vec<PerformanceMetrics>) {
        if let Some(obj) = table.as_object() {
            let table_name = obj.get("table_name")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown");
            
            let mut metric = PerformanceMetrics::new(format!("Table: {table_name}"));
            
            // Extract access type
            let access_type = obj.get("access_type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            
            // Extract cost information
            if let Some(cost_info) = obj.get("cost_info") {
                if let Some(cost_obj) = cost_info.as_object() {
                    if let Some(JsonValue::Number(read_cost)) = cost_obj.get("read_cost") {
                        metric.cost_score = read_cost.as_f64().unwrap_or(0.0);
                    }
                }
            }
            
            // Extract row information
            if let Some(JsonValue::Number(rows_examined)) = obj.get("rows_examined_per_scan") {
                metric.rows_examined = rows_examined.as_u64();
            }
            
            if let Some(JsonValue::Number(rows_produced)) = obj.get("rows_produced_per_join") {
                metric.rows_returned = rows_produced.as_u64();
            }
            
            metric.calculate_efficiency();
            
            // Calculate performance level
            metric.performance_level = Self::calculate_mysql_performance_level(&metric, access_type);
            
            // Add MySQL-specific recommendations
            Self::add_mysql_recommendations(&mut metric, access_type, obj);
            
            metrics.push(metric);
        }
    }
    
    /// Calculate performance level for MySQL operations
    fn calculate_mysql_performance_level(_metric: &PerformanceMetrics, access_type: &str) -> PerformanceLevel {
        // Check access type
        match access_type {
            "ALL" => PerformanceLevel::Critical,
            "index" => PerformanceLevel::Warning,
            "range" => PerformanceLevel::Good,
            "ref" => PerformanceLevel::Good,
            "eq_ref" => PerformanceLevel::Excellent,
            "const" | "system" => PerformanceLevel::Excellent,
            _ => PerformanceLevel::Good,
        }
    }
    
    /// Add MySQL-specific recommendations
    fn add_mysql_recommendations(metric: &mut PerformanceMetrics, access_type: &str, obj: &serde_json::Map<String, JsonValue>) {
        match access_type {
            "ALL" => {
                metric.add_warning("Full table scan detected".to_string());
                if let Some(JsonValue::String(table_name)) = obj.get("table_name") {
                    metric.add_recommendation(format!("Add an index to table '{table_name}'"));
                }
            },
            "index" => {
                metric.add_warning("Full index scan detected".to_string());
                metric.add_recommendation("Consider adding a more selective index".to_string());
            },
            _ => {}
        }
        
        // Check filtering efficiency
        if let Some(efficiency) = metric.efficiency_percent {
            if efficiency < 10.0 {
                metric.add_warning(format!("Poor filtering efficiency: {efficiency:.1}%"));
                metric.add_recommendation("Consider adding more selective WHERE conditions".to_string());
            }
        }
    }
    
    /// Analyze SQLite EXPLAIN QUERY PLAN output
    pub fn analyze_sqlite_plan(plan_rows: &[Vec<String>]) -> Vec<PerformanceMetrics> {
        let mut metrics = Vec::new();
        
        // Skip header row if present
        let data_rows = if plan_rows.len() > 1 && 
                          plan_rows[0].iter().any(|col| col.to_lowercase().contains("id") || col.to_lowercase().contains("detail")) {
            &plan_rows[1..]
        } else {
            plan_rows
        };
        
        for (i, row) in data_rows.iter().enumerate() {
            if row.len() >= 4 {
                let detail = &row[3];
                let mut metric = PerformanceMetrics::new(format!("Step {}", i + 1));
                
                // Calculate performance level based on operation
                metric.performance_level = Self::calculate_sqlite_performance_level(detail);
                
                // Add SQLite-specific recommendations
                Self::add_sqlite_recommendations(&mut metric, detail);
                
                metrics.push(metric);
            }
        }
        
        metrics
    }
    
    /// Calculate performance level for SQLite operations
    fn calculate_sqlite_performance_level(detail: &str) -> PerformanceLevel {
        let detail_lower = detail.to_lowercase();
        
        if detail_lower.contains("using covering index") {
            PerformanceLevel::Excellent
        } else if detail_lower.contains("using integer primary key") {
            PerformanceLevel::Excellent
        } else if detail_lower.contains("using index") {
            PerformanceLevel::Good
        } else if detail_lower.contains("search table") {
            PerformanceLevel::Good
        } else if detail_lower.contains("scan table") {
            PerformanceLevel::Critical
        } else if detail_lower.contains("using temporary b-tree") {
            PerformanceLevel::Warning
        } else {
            PerformanceLevel::Good
        }
    }
    
    /// Add SQLite-specific recommendations
    fn add_sqlite_recommendations(metric: &mut PerformanceMetrics, detail: &str) {
        let detail_lower = detail.to_lowercase();
        
        if detail_lower.contains("scan table") && !detail_lower.contains("using index") {
            metric.add_warning("Full table scan detected".to_string());
            metric.add_recommendation("Consider adding an index to improve performance".to_string());
        } else if detail_lower.contains("using temporary b-tree") {
            metric.add_warning("Temporary B-tree created for sorting".to_string());
            metric.add_recommendation("Consider adding an index to avoid sorting".to_string());
        }
    }
    
    /// Calculate overall performance score (0-100)
    pub fn calculate_overall_score(metrics: &[PerformanceMetrics]) -> u8 {
        if metrics.is_empty() {
            return 100;
        }
        
        let mut total_score = 0;
        for metric in metrics {
            let score = match metric.performance_level {
                PerformanceLevel::Excellent => 100,
                PerformanceLevel::Good => 80,
                PerformanceLevel::Warning => 60,
                PerformanceLevel::Poor => 40,
                PerformanceLevel::Critical => 20,
            };
            total_score += score;
        }
        
        (total_score / metrics.len()) as u8
    }
    
    /// Format performance metrics with colors and enhanced dashboard
    pub fn format_metrics_with_colors(metrics: &[PerformanceMetrics]) -> Vec<String> {
        let mut formatted = Vec::new();
        let overall_score = Self::calculate_overall_score(metrics);
        
        // Add header with overall score
        let score_color = if overall_score >= 80 {
            Color::Green
        } else if overall_score >= 60 {
            Color::Yellow
        } else {
            Color::Red
        };
        
        formatted.push(format!("📊 {}",
            score_color.bold().paint(format!("Query Performance Analysis (Score: {overall_score}/100)"))
        ));
        
        // Add performance dashboard summary
        let dashboard_summary = Self::generate_performance_dashboard(metrics, overall_score);
        for line in dashboard_summary {
            formatted.push(line);
        }
        
        formatted.push(String::new());
        formatted.push(format!("🔍 {}", Color::White.bold().paint("Detailed Operation Analysis:")));
        formatted.push(String::new());
        
        // Add individual metrics
        for (i, metric) in metrics.iter().enumerate() {
            let color = metric.performance_level.color();
            let emoji = metric.performance_level.emoji();
            
            formatted.push(format!("{} {} {} (Step {})",
                emoji,
                color.bold().paint(&metric.operation_type),
                color.paint(metric.performance_level.description()),
                i + 1
            ));
            
            // Add timing information if available
            if let Some(time) = metric.time_ms {
                let time_color = if time > 1000.0 {
                    Color::Red
                } else if time > 100.0 {
                    Color::Yellow
                } else {
                    Color::Green
                };
                formatted.push(format!("  ⏱️  Duration: {}", 
                    time_color.paint(format!("{time:.2} ms"))
                ));
            }
            
            // Add cost information
            if metric.cost_score > 0.0 {
                let cost_color = if metric.cost_score > 10000.0 {
                    Color::Red
                } else if metric.cost_score > 1000.0 {
                    Color::Yellow
                } else {
                    Color::Green
                };
                formatted.push(format!("  💰 Cost: {}", 
                    cost_color.paint(format!("{:.0}", metric.cost_score))
                ));
            }
            
            // Add row information
            if let Some(examined) = metric.rows_examined {
                formatted.push(format!("  🔍 Rows Examined: {}", Color::Blue.paint(format!("{examined}"))));
            }
            if let Some(returned) = metric.rows_returned {
                formatted.push(format!("  📤 Rows Returned: {}", Color::Blue.paint(format!("{returned}"))));
            }
            
            // Add efficiency information
            if let Some(efficiency) = metric.efficiency_percent {
                let efficiency_color = if efficiency >= 50.0 {
                    Color::Green
                } else if efficiency >= 10.0 {
                    Color::Yellow
                } else {
                    Color::Red
                };
                formatted.push(format!("  📊 Efficiency: {}",
                    efficiency_color.paint(format!("{efficiency:.1}%"))
                ));
            }
            
            // Add warnings
            for warning in &metric.warnings {
                formatted.push(format!("  🚨 {}", Color::Red.paint(warning)));
            }
            
            // Add recommendations
            for recommendation in &metric.recommendations {
                formatted.push(format!("  💡 {}", Color::Cyan.paint(recommendation)));
            }
            
            formatted.push(String::new()); // Empty line between metrics
        }
        
        // Add comprehensive recommendations
        let comprehensive_recommendations = Self::generate_comprehensive_recommendations(metrics, overall_score);
        if !comprehensive_recommendations.is_empty() {
            formatted.push(format!("🎯 {}", Color::White.bold().paint("Performance Optimization Summary:")));
            formatted.push(String::new());
            for recommendation in comprehensive_recommendations {
                formatted.push(recommendation);
            }
        }
        
        formatted
    }
    
    /// Generate performance dashboard summary
    pub fn generate_performance_dashboard(metrics: &[PerformanceMetrics], overall_score: u8) -> Vec<String> {
        let mut dashboard = Vec::new();
        
        dashboard.push(String::new());
        dashboard.push(format!("📈 {}", Color::White.bold().paint("Performance Dashboard:")));
        dashboard.push("─".repeat(50));
        
        // Performance level distribution
        let mut excellent_count = 0;
        let mut good_count = 0;
        let mut warning_count = 0;
        let mut poor_count = 0;
        let mut critical_count = 0;
        
        for metric in metrics {
            match metric.performance_level {
                PerformanceLevel::Excellent => excellent_count += 1,
                PerformanceLevel::Good => good_count += 1,
                PerformanceLevel::Warning => warning_count += 1,
                PerformanceLevel::Poor => poor_count += 1,
                PerformanceLevel::Critical => critical_count += 1,
            }
        }
        
        dashboard.push(format!("🟢 Excellent Operations: {}", Color::Green.paint(format!("{excellent_count}"))));
        dashboard.push(format!("🟢 Good Operations: {}", Color::LightGreen.paint(format!("{good_count}"))));
        if warning_count > 0 {
            dashboard.push(format!("🟡 Warning Operations: {}", Color::Yellow.paint(format!("{warning_count}"))));
        }
        if poor_count > 0 {
            dashboard.push(format!("🟠 Poor Operations: {}", Color::LightRed.paint(format!("{poor_count}"))));
        }
        if critical_count > 0 {
            dashboard.push(format!("🔴 Critical Operations: {}", Color::Red.paint(format!("{critical_count}"))));
        }
        
        // Performance statistics
        let total_warnings = metrics.iter().map(|m| m.warnings.len()).sum::<usize>();
        let total_recommendations = metrics.iter().map(|m| m.recommendations.len()).sum::<usize>();
        
        dashboard.push(String::new());
        dashboard.push(format!("📊 Total Operations: {}", metrics.len()));
        if total_warnings > 0 {
            dashboard.push(format!("⚠️  Total Warnings: {}", Color::Red.paint(format!("{total_warnings}"))));
        }
        if total_recommendations > 0 {
            dashboard.push(format!("💡 Optimization Opportunities: {}", Color::Cyan.paint(format!("{total_recommendations}"))));
        }
        
        // Performance grade
        let grade = if overall_score >= 90 {
            ("A+", Color::Green)
        } else if overall_score >= 80 {
            ("A", Color::Green)
        } else if overall_score >= 70 {
            ("B", Color::LightGreen)
        } else if overall_score >= 60 {
            ("C", Color::Yellow)
        } else if overall_score >= 50 {
            ("D", Color::LightRed)
        } else {
            ("F", Color::Red)
        };
        
        dashboard.push(String::new());
        dashboard.push(format!("🎓 Performance Grade: {}", grade.1.bold().paint(grade.0)));
        
        // Performance status
        let status = if overall_score >= 80 {
            ("✅ Excellent Performance", Color::Green)
        } else if overall_score >= 60 {
            ("⚠️  Needs Attention", Color::Yellow)
        } else {
            ("🚨 Requires Optimization", Color::Red)
        };
        
        dashboard.push(format!("🏆 Status: {}", status.1.bold().paint(status.0)));
        dashboard.push("─".repeat(50));
        
        dashboard
    }
    
    /// Generate comprehensive optimization recommendations
    pub fn generate_comprehensive_recommendations(metrics: &[PerformanceMetrics], overall_score: u8) -> Vec<String> {
        let mut recommendations = Vec::new();
        
        // Analyze patterns across all metrics
        let has_table_scans = metrics.iter().any(|m| 
            m.operation_type.contains("Seq Scan") || 
            m.operation_type.contains("scan table") ||
            m.warnings.iter().any(|w| w.contains("Full table scan"))
        );
        
        let has_slow_operations = metrics.iter().any(|m| 
            m.time_ms.is_some_and(|t| t > 100.0) ||
            m.cost_score > 1000.0
        );
        
        let has_inefficient_operations = metrics.iter().any(|m|
            m.efficiency_percent.is_some_and(|e| e < 10.0)
        );
        
        let has_sort_spill = metrics.iter().any(|m|
            m.warnings.iter().any(|w| w.contains("spilled to disk"))
        );
        
        // Priority recommendations based on overall score
        if overall_score < 60 {
            recommendations.push(format!("🚨 {}: This query has significant performance issues and requires immediate attention.",
                Color::Red.bold().paint("CRITICAL")
            ));
        } else if overall_score < 80 {
            recommendations.push(format!("⚠️  {}: This query has performance issues that should be addressed.",
                Color::Yellow.bold().paint("WARNING")
            ));
        }
        
        // Specific pattern-based recommendations
        if has_table_scans {
            recommendations.push(format!("🔍 {}: Add appropriate indexes to eliminate full table scans. Consider composite indexes for multi-column WHERE clauses.",
                Color::Cyan.paint("INDEX OPTIMIZATION")
            ));
        }
        
        if has_slow_operations {
            recommendations.push(format!("⏱️  {}: Review query structure and consider query rewriting, partitioning, or hardware upgrades.",
                Color::Cyan.paint("EXECUTION TIME")
            ));
        }
        
        if has_inefficient_operations {
            recommendations.push(format!("📊 {}: Improve WHERE clause selectivity and consider adding more specific filters.",
                Color::Cyan.paint("FILTER EFFICIENCY")
            ));
        }
        
        if has_sort_spill {
            recommendations.push(format!("💾 {}: Increase work_mem setting or add indexes to support ORDER BY clauses.",
                Color::Cyan.paint("MEMORY OPTIMIZATION")
            ));
        }
        
        // General recommendations based on score ranges
        match overall_score {
            90..=100 => {
                recommendations.push(format!("✨ {}: Excellent performance! Monitor regularly to maintain this level.",
                    Color::Green.paint("MONITORING")
                ));
            },
            80..=89 => {
                recommendations.push(format!("👍 {}: Good performance with minor optimization opportunities.",
                    Color::LightGreen.paint("FINE-TUNING")
                ));
            },
            60..=79 => {
                recommendations.push(format!("🔧 {}: Consider database statistics updates and index maintenance.",
                    Color::Yellow.paint("MAINTENANCE")
                ));
            },
            40..=59 => {
                recommendations.push(format!("🏗️  {}: Review database schema design and consider query restructuring.",
                    Color::LightRed.paint("RESTRUCTURING")
                ));
            },
            _ => {
                recommendations.push(format!("🆘 {}: Comprehensive performance review needed. Consider consulting a database specialist.",
                    Color::Red.paint("CRITICAL REVIEW")
                ));
            }
        }
        
        // Add general best practices if there are issues
        if overall_score < 80 {
            recommendations.push(String::new());
            recommendations.push(format!("📚 {}", Color::White.bold().paint("General Best Practices:")));
            recommendations.push("   • Update table statistics regularly (ANALYZE/UPDATE STATISTICS)".to_string());
            recommendations.push("   • Monitor index usage and remove unused indexes".to_string());
            recommendations.push("   • Consider query caching for frequently executed queries".to_string());
            recommendations.push("   • Review application-level caching strategies".to_string());
            recommendations.push("   • Use EXPLAIN ANALYZE for detailed execution metrics".to_string());
        }
        
        recommendations
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_performance_level_colors() {
        assert_eq!(PerformanceLevel::Excellent.color(), Color::Green);
        assert_eq!(PerformanceLevel::Critical.color(), Color::Red);
        assert_eq!(PerformanceLevel::Warning.color(), Color::Yellow);
    }
    
    #[test]
    fn test_performance_metrics_new() {
        let metric = PerformanceMetrics::new("Test Operation".to_string());
        assert_eq!(metric.operation_type, "Test Operation");
        assert_eq!(metric.performance_level, PerformanceLevel::Good);
        assert_eq!(metric.cost_score, 0.0);
    }
    
    #[test]
    fn test_calculate_efficiency() {
        let mut metric = PerformanceMetrics::new("Test".to_string());
        metric.rows_examined = Some(1000);
        metric.rows_returned = Some(100);
        metric.calculate_efficiency();
        assert_eq!(metric.efficiency_percent, Some(10.0));
    }
    
    #[test]
    fn test_mysql_performance_level_calculation() {
        let metric = PerformanceMetrics::new("Test".to_string());
        assert_eq!(PerformanceAnalyzer::calculate_mysql_performance_level(&metric, "ALL"), PerformanceLevel::Critical);
        assert_eq!(PerformanceAnalyzer::calculate_mysql_performance_level(&metric, "const"), PerformanceLevel::Excellent);
        assert_eq!(PerformanceAnalyzer::calculate_mysql_performance_level(&metric, "ref"), PerformanceLevel::Good);
    }
    
    #[test]
    fn test_sqlite_performance_level_calculation() {
        assert_eq!(PerformanceAnalyzer::calculate_sqlite_performance_level("SCAN TABLE users"), PerformanceLevel::Critical);
        assert_eq!(PerformanceAnalyzer::calculate_sqlite_performance_level("SEARCH TABLE users USING INDEX"), PerformanceLevel::Good);
        assert_eq!(PerformanceAnalyzer::calculate_sqlite_performance_level("SEARCH TABLE users USING COVERING INDEX"), PerformanceLevel::Excellent);
    }
    
    #[test]
    fn test_overall_score_calculation() {
        let metrics = vec![
            PerformanceMetrics {
                operation_type: "Test1".to_string(),
                performance_level: PerformanceLevel::Excellent,
                cost_score: 0.0,
                time_ms: None,
                rows_examined: None,
                rows_returned: None,
                efficiency_percent: None,
                recommendations: Vec::new(),
                warnings: Vec::new(),
            },
            PerformanceMetrics {
                operation_type: "Test2".to_string(),
                performance_level: PerformanceLevel::Good,
                cost_score: 0.0,
                time_ms: None,
                rows_examined: None,
                rows_returned: None,
                efficiency_percent: None,
                recommendations: Vec::new(),
                warnings: Vec::new(),
            },
        ];
        assert_eq!(PerformanceAnalyzer::calculate_overall_score(&metrics), 90);
    }
    
    #[test]
    fn test_performance_dashboard_generation() {
        let metrics = vec![
            PerformanceMetrics {
                operation_type: "Excellent Op".to_string(),
                performance_level: PerformanceLevel::Excellent,
                cost_score: 10.0,
                time_ms: Some(5.0),
                rows_examined: Some(100),
                rows_returned: Some(50),
                efficiency_percent: Some(50.0),
                recommendations: vec!["Test recommendation".to_string()],
                warnings: Vec::new(),
            },
            PerformanceMetrics {
                operation_type: "Critical Op".to_string(),
                performance_level: PerformanceLevel::Critical,
                cost_score: 15000.0,
                time_ms: Some(2000.0),
                rows_examined: Some(10000),
                rows_returned: Some(1),
                efficiency_percent: Some(0.01),
                recommendations: Vec::new(),
                warnings: vec!["Full table scan detected".to_string()],
            },
        ];
        
        let dashboard = PerformanceAnalyzer::generate_performance_dashboard(&metrics, 60);
        
        // Check that dashboard contains expected elements
        assert!(dashboard.iter().any(|line| line.contains("Performance Dashboard:")));
        assert!(dashboard.iter().any(|line| line.contains("Excellent Operations:")));
        assert!(dashboard.iter().any(|line| line.contains("Critical Operations:")));
        assert!(dashboard.iter().any(|line| line.contains("Total Operations: 2")));
        assert!(dashboard.iter().any(|line| line.contains("Total Warnings:")));
        assert!(dashboard.iter().any(|line| line.contains("Optimization Opportunities:")));
        assert!(dashboard.iter().any(|line| line.contains("Performance Grade:")));
        assert!(dashboard.iter().any(|line| line.contains("Needs Attention")));
    }
    
    #[test]
    fn test_comprehensive_recommendations() {
        let metrics = vec![
            PerformanceMetrics {
                operation_type: "Seq Scan".to_string(),
                performance_level: PerformanceLevel::Critical,
                cost_score: 12000.0,
                time_ms: Some(1500.0),
                rows_examined: Some(100000),
                rows_returned: Some(5),
                efficiency_percent: Some(0.005),
                recommendations: vec!["Add index".to_string()],
                warnings: vec!["Full table scan detected".to_string(), "Sort spilled to disk".to_string()],
            },
        ];
        
        let recommendations = PerformanceAnalyzer::generate_comprehensive_recommendations(&metrics, 30);
        
        // Check that comprehensive recommendations are generated
        assert!(recommendations.iter().any(|r| r.contains("CRITICAL")));
        assert!(recommendations.iter().any(|r| r.contains("INDEX OPTIMIZATION")));
        assert!(recommendations.iter().any(|r| r.contains("EXECUTION TIME")));
        assert!(recommendations.iter().any(|r| r.contains("FILTER EFFICIENCY")));
        assert!(recommendations.iter().any(|r| r.contains("MEMORY OPTIMIZATION")));
        assert!(recommendations.iter().any(|r| r.contains("CRITICAL REVIEW")));
        assert!(recommendations.iter().any(|r| r.contains("General Best Practices")));
    }
    
    #[test]
    fn test_excellent_performance_recommendations() {
        let metrics = vec![
            PerformanceMetrics {
                operation_type: "Index Scan".to_string(),
                performance_level: PerformanceLevel::Excellent,
                cost_score: 1.0,
                time_ms: Some(0.5),
                rows_examined: Some(10),
                rows_returned: Some(10),
                efficiency_percent: Some(100.0),
                recommendations: Vec::new(),
                warnings: Vec::new(),
            },
        ];
        
        let recommendations = PerformanceAnalyzer::generate_comprehensive_recommendations(&metrics, 95);
        
        // Check that monitoring recommendation is present for excellent performance
        assert!(recommendations.iter().any(|r| r.contains("MONITORING")));
        assert!(recommendations.iter().any(|r| r.contains("Excellent performance")));
        // Should not contain best practices section for excellent performance
        assert!(!recommendations.iter().any(|r| r.contains("General Best Practices")));
    }
    
    #[test]
    fn test_enhanced_formatting_output() {
        let metrics = vec![
            PerformanceMetrics {
                operation_type: "Test Operation".to_string(),
                performance_level: PerformanceLevel::Warning,
                cost_score: 500.0,
                time_ms: Some(150.0),
                rows_examined: Some(1000),
                rows_returned: Some(100),
                efficiency_percent: Some(10.0),
                recommendations: vec!["Test recommendation".to_string()],
                warnings: vec!["Test warning".to_string()],
            },
        ];
        
        let formatted = PerformanceAnalyzer::format_metrics_with_colors(&metrics);
        
        // Check that enhanced formatting includes all sections
        assert!(formatted.iter().any(|line| line.contains("Query Performance Analysis")));
        assert!(formatted.iter().any(|line| line.contains("Performance Dashboard:")));
        assert!(formatted.iter().any(|line| line.contains("Detailed Operation Analysis:")));
        assert!(formatted.iter().any(|line| line.contains("Performance Optimization Summary:")));
        assert!(formatted.iter().any(|line| line.contains("Duration:")));
        assert!(formatted.iter().any(|line| line.contains("Cost:")));
        assert!(formatted.iter().any(|line| line.contains("Rows Examined:")));
        assert!(formatted.iter().any(|line| line.contains("Rows Returned:")));
        assert!(formatted.iter().any(|line| line.contains("Efficiency:")));
    }
}