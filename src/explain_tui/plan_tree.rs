//! Plan tree data structures for the TUI explain visualizer
//!
//! This module provides data structures to represent PostgreSQL EXPLAIN plans
//! as a hierarchical tree that can be rendered in the TUI.

use crate::performance_analyzer::PerformanceLevel;
use serde_json::Value as JsonValue;

/// A node in the query execution plan tree
#[derive(Debug, Clone)]
pub struct PlanNode {
    /// Unique identifier for this node (for tree widget)
    pub id: String,
    /// The type of operation (e.g., "Seq Scan", "Index Scan", "Hash Join")
    pub node_type: String,
    /// The relation/table being accessed (if applicable)
    pub relation_name: Option<String>,
    /// Schema name (if available)
    pub schema: Option<String>,
    /// The index being used (if applicable)
    pub index_name: Option<String>,
    /// Startup cost (cost before first row is returned)
    pub startup_cost: f64,
    /// Total cost (cost to return all rows)
    pub total_cost: f64,
    /// Estimated number of rows
    pub plan_rows: u64,
    /// Actual number of rows (if ANALYZE was used)
    pub actual_rows: Option<u64>,
    /// Actual time in milliseconds (if ANALYZE was used)
    pub actual_time_ms: Option<f64>,
    /// Actual startup time in milliseconds
    pub actual_startup_time_ms: Option<f64>,
    /// Number of loops executed
    pub actual_loops: Option<u64>,
    /// Filter condition applied
    pub filter: Option<String>,
    /// Index condition
    pub index_cond: Option<String>,
    /// Recheck condition (for bitmap scans)
    pub recheck_cond: Option<String>,
    /// Join filter
    pub join_filter: Option<String>,
    /// Hash condition
    pub hash_cond: Option<String>,
    /// Merge condition
    pub merge_cond: Option<String>,
    /// Sort key(s)
    pub sort_key: Vec<String>,
    /// Sort method (quicksort, top-N heapsort, external merge, etc.)
    pub sort_method: Option<String>,
    /// Sort space used
    pub sort_space_used: Option<u64>,
    /// Sort space type (Memory or Disk)
    pub sort_space_type: Option<String>,
    /// Group key(s) for aggregation
    pub group_key: Vec<String>,
    /// Output columns
    pub output: Vec<String>,
    /// Rows removed by filter
    pub rows_removed_by_filter: Option<u64>,
    /// Shared hit blocks
    pub shared_hit_blocks: Option<u64>,
    /// Shared read blocks
    pub shared_read_blocks: Option<u64>,
    /// Performance level for this node
    pub performance_level: PerformanceLevel,
    /// Warnings for this node
    pub warnings: Vec<String>,
    /// Recommendations for optimization
    pub recommendations: Vec<String>,
    /// Child nodes in the plan tree
    pub children: Vec<PlanNode>,
    /// Parent node type (for context in joins)
    pub parent_relationship: Option<String>,
    /// Workers planned (for parallel queries)
    pub workers_planned: Option<u64>,
    /// Workers launched (for parallel queries)
    pub workers_launched: Option<u64>,
    /// CTE name if this is a CTE scan
    pub cte_name: Option<String>,
    /// Subplan name
    pub subplan_name: Option<String>,
}

impl PlanNode {
    /// Create a new PlanNode with default values
    pub fn new(id: String, node_type: String) -> Self {
        Self {
            id,
            node_type,
            relation_name: None,
            schema: None,
            index_name: None,
            startup_cost: 0.0,
            total_cost: 0.0,
            plan_rows: 0,
            actual_rows: None,
            actual_time_ms: None,
            actual_startup_time_ms: None,
            actual_loops: None,
            filter: None,
            index_cond: None,
            recheck_cond: None,
            join_filter: None,
            hash_cond: None,
            merge_cond: None,
            sort_key: Vec::new(),
            sort_method: None,
            sort_space_used: None,
            sort_space_type: None,
            group_key: Vec::new(),
            output: Vec::new(),
            rows_removed_by_filter: Option::None,
            shared_hit_blocks: None,
            shared_read_blocks: None,
            performance_level: PerformanceLevel::Good,
            warnings: Vec::new(),
            recommendations: Vec::new(),
            children: Vec::new(),
            parent_relationship: None,
            workers_planned: None,
            workers_launched: None,
            cte_name: None,
            subplan_name: None,
        }
    }

    /// Get a display label for this node (used in tree view)
    pub fn display_label(&self) -> String {
        let mut label = self.node_type.clone();

        // Add relation/table name if available
        if let Some(ref relation) = self.relation_name {
            if let Some(ref schema) = self.schema {
                label = format!("{} on {}.{}", label, schema, relation);
            } else {
                label = format!("{} on {}", label, relation);
            }
        }

        // Add index name if available
        if let Some(ref index) = self.index_name {
            label = format!("{} using {}", label, index);
        }

        // Add CTE name if applicable
        if let Some(ref cte) = self.cte_name {
            label = format!("{} ({})", label, cte);
        }

        label
    }

    /// Get a short summary of costs/timing
    pub fn cost_summary(&self) -> String {
        if let Some(time) = self.actual_time_ms {
            format!("{:.2}ms", time)
        } else {
            format!("cost: {:.0}", self.total_cost)
        }
    }

    /// Get row count summary (estimated vs actual)
    pub fn rows_summary(&self) -> String {
        if let Some(actual) = self.actual_rows {
            let ratio = if self.plan_rows > 0 {
                actual as f64 / self.plan_rows as f64
            } else {
                1.0
            };
            if (0.8..=1.25).contains(&ratio) {
                format!("{} rows", actual)
            } else {
                format!("{} rows (est: {})", actual, self.plan_rows)
            }
        } else {
            format!("~{} rows", self.plan_rows)
        }
    }

    /// Check if this node has any warnings
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    /// Get the total number of nodes in this subtree
    pub fn node_count(&self) -> usize {
        1 + self.children.iter().map(|c| c.node_count()).sum::<usize>()
    }

    /// Calculate the maximum depth of this subtree
    pub fn max_depth(&self) -> usize {
        if self.children.is_empty() {
            1
        } else {
            1 + self
                .children
                .iter()
                .map(|c| c.max_depth())
                .max()
                .unwrap_or(0)
        }
    }

    /// Calculate total cost of this subtree
    pub fn total_subtree_cost(&self) -> f64 {
        self.total_cost
    }

    /// Calculate total time of this subtree (if available)
    pub fn total_subtree_time(&self) -> Option<f64> {
        self.actual_time_ms
    }

    /// Get all tables involved in this subtree
    pub fn get_tables(&self) -> Vec<String> {
        let mut tables = Vec::new();
        if let Some(ref relation) = self.relation_name {
            let full_name = if let Some(ref schema) = self.schema {
                format!("{}.{}", schema, relation)
            } else {
                relation.clone()
            };
            tables.push(full_name);
        }
        for child in &self.children {
            tables.extend(child.get_tables());
        }
        tables
    }

    /// Get all indexes used in this subtree
    pub fn get_indexes(&self) -> Vec<String> {
        let mut indexes = Vec::new();
        if let Some(ref index) = self.index_name {
            indexes.push(index.clone());
        }
        for child in &self.children {
            indexes.extend(child.get_indexes());
        }
        indexes
    }

    /// Count warnings in this subtree
    pub fn count_warnings(&self) -> usize {
        self.warnings.len()
            + self
                .children
                .iter()
                .map(|c| c.count_warnings())
                .sum::<usize>()
    }

    /// Count recommendations in this subtree
    pub fn count_recommendations(&self) -> usize {
        self.recommendations.len()
            + self
                .children
                .iter()
                .map(|c| c.count_recommendations())
                .sum::<usize>()
    }
}

/// Counter for generating unique node IDs
static NODE_ID_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn next_node_id() -> String {
    let id = NODE_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    format!("node_{}", id)
}

/// Reset the node ID counter (useful for testing)
pub fn reset_node_id_counter() {
    NODE_ID_COUNTER.store(0, std::sync::atomic::Ordering::SeqCst);
}

/// Parse a PostgreSQL EXPLAIN (FORMAT JSON) output into a PlanNode tree
pub fn parse_postgresql_plan(plan_json: &JsonValue) -> Option<PlanNode> {
    // Reset counter for consistent IDs
    reset_node_id_counter();

    // PostgreSQL EXPLAIN JSON is an array with one element containing the plan
    if let JsonValue::Array(plans) = plan_json {
        if let Some(plan) = plans.first() {
            if let Some(plan_obj) = plan.as_object() {
                if let Some(plan_node) = plan_obj.get("Plan") {
                    return parse_plan_node(plan_node);
                }
            }
        }
    }
    None
}

/// Recursively parse a plan node from JSON
fn parse_plan_node(node: &JsonValue) -> Option<PlanNode> {
    let node_obj = node.as_object()?;

    let node_type = node_obj
        .get("Node Type")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();

    let mut plan_node = PlanNode::new(next_node_id(), node_type.clone());

    // Extract basic properties
    plan_node.relation_name = node_obj
        .get("Relation Name")
        .and_then(|v| v.as_str())
        .map(String::from);
    plan_node.schema = node_obj
        .get("Schema")
        .and_then(|v| v.as_str())
        .map(String::from);
    plan_node.index_name = node_obj
        .get("Index Name")
        .and_then(|v| v.as_str())
        .map(String::from);

    // Cost information
    plan_node.startup_cost = node_obj
        .get("Startup Cost")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    plan_node.total_cost = node_obj
        .get("Total Cost")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    plan_node.plan_rows = node_obj
        .get("Plan Rows")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    // Actual execution statistics (from ANALYZE)
    plan_node.actual_rows = node_obj.get("Actual Rows").and_then(|v| v.as_u64());
    plan_node.actual_time_ms = node_obj.get("Actual Total Time").and_then(|v| v.as_f64());
    plan_node.actual_startup_time_ms = node_obj.get("Actual Startup Time").and_then(|v| v.as_f64());
    plan_node.actual_loops = node_obj.get("Actual Loops").and_then(|v| v.as_u64());

    // Filter and condition information
    plan_node.filter = node_obj
        .get("Filter")
        .and_then(|v| v.as_str())
        .map(String::from);
    plan_node.index_cond = node_obj
        .get("Index Cond")
        .and_then(|v| v.as_str())
        .map(String::from);
    plan_node.recheck_cond = node_obj
        .get("Recheck Cond")
        .and_then(|v| v.as_str())
        .map(String::from);
    plan_node.join_filter = node_obj
        .get("Join Filter")
        .and_then(|v| v.as_str())
        .map(String::from);
    plan_node.hash_cond = node_obj
        .get("Hash Cond")
        .and_then(|v| v.as_str())
        .map(String::from);
    plan_node.merge_cond = node_obj
        .get("Merge Cond")
        .and_then(|v| v.as_str())
        .map(String::from);

    // Sort information
    if let Some(JsonValue::Array(keys)) = node_obj.get("Sort Key") {
        plan_node.sort_key = keys
            .iter()
            .filter_map(|k| k.as_str().map(String::from))
            .collect();
    }
    plan_node.sort_method = node_obj
        .get("Sort Method")
        .and_then(|v| v.as_str())
        .map(String::from);
    plan_node.sort_space_used = node_obj.get("Sort Space Used").and_then(|v| v.as_u64());
    plan_node.sort_space_type = node_obj
        .get("Sort Space Type")
        .and_then(|v| v.as_str())
        .map(String::from);

    // Group key for aggregation
    if let Some(JsonValue::Array(keys)) = node_obj.get("Group Key") {
        plan_node.group_key = keys
            .iter()
            .filter_map(|k| k.as_str().map(String::from))
            .collect();
    }

    // Output columns
    if let Some(JsonValue::Array(cols)) = node_obj.get("Output") {
        plan_node.output = cols
            .iter()
            .filter_map(|c| c.as_str().map(String::from))
            .collect();
    }

    // Buffer usage
    plan_node.shared_hit_blocks = node_obj.get("Shared Hit Blocks").and_then(|v| v.as_u64());
    plan_node.shared_read_blocks = node_obj.get("Shared Read Blocks").and_then(|v| v.as_u64());
    plan_node.rows_removed_by_filter = node_obj
        .get("Rows Removed by Filter")
        .and_then(|v| v.as_u64());

    // Parallel query info
    plan_node.workers_planned = node_obj.get("Workers Planned").and_then(|v| v.as_u64());
    plan_node.workers_launched = node_obj.get("Workers Launched").and_then(|v| v.as_u64());

    // CTE and subplan names
    plan_node.cte_name = node_obj
        .get("CTE Name")
        .and_then(|v| v.as_str())
        .map(String::from);
    plan_node.subplan_name = node_obj
        .get("Subplan Name")
        .and_then(|v| v.as_str())
        .map(String::from);
    plan_node.parent_relationship = node_obj
        .get("Parent Relationship")
        .and_then(|v| v.as_str())
        .map(String::from);

    // Calculate performance level
    plan_node.performance_level = calculate_performance_level(&plan_node);

    // Add warnings and recommendations
    add_warnings_and_recommendations(&mut plan_node);

    // Parse child nodes
    if let Some(JsonValue::Array(plans)) = node_obj.get("Plans") {
        for child_node in plans {
            if let Some(child) = parse_plan_node(child_node) {
                plan_node.children.push(child);
            }
        }
    }

    Some(plan_node)
}

/// Calculate the performance level for a plan node
fn calculate_performance_level(node: &PlanNode) -> PerformanceLevel {
    // Sequential scan is usually a warning (unless on small tables)
    if node.node_type == "Seq Scan" {
        // If we have actual rows and it's a small table, it might be OK
        if let Some(rows) = node.actual_rows {
            if rows < 1000 {
                return PerformanceLevel::Good;
            }
        } else if node.plan_rows < 1000 {
            return PerformanceLevel::Good;
        }
        return PerformanceLevel::Warning;
    }

    // Check execution time thresholds
    if let Some(time) = node.actual_time_ms {
        if time > 1000.0 {
            return PerformanceLevel::Critical;
        } else if time > 100.0 {
            return PerformanceLevel::Poor;
        } else if time > 10.0 {
            return PerformanceLevel::Warning;
        }
    }

    // Check cost thresholds
    if node.total_cost > 10000.0 {
        return PerformanceLevel::Critical;
    } else if node.total_cost > 1000.0 {
        return PerformanceLevel::Warning;
    }

    // Check row estimation accuracy
    if let Some(actual) = node.actual_rows {
        if node.plan_rows > 0 && actual > 0 {
            let ratio = node.plan_rows as f64 / actual as f64;
            if !(0.1..=10.0).contains(&ratio) {
                return PerformanceLevel::Poor;
            }
        }
    }

    // Check for disk spill in sorting
    if let Some(ref space_type) = node.sort_space_type {
        if space_type == "Disk" {
            return PerformanceLevel::Warning;
        }
    }

    // Index scans and efficient operations
    if matches!(
        node.node_type.as_str(),
        "Index Scan" | "Index Only Scan" | "Bitmap Index Scan"
    ) {
        return PerformanceLevel::Excellent;
    }

    PerformanceLevel::Good
}

/// Add warnings and recommendations based on node analysis
fn add_warnings_and_recommendations(node: &mut PlanNode) {
    match node.node_type.as_str() {
        "Seq Scan" => {
            if node.plan_rows >= 1000 || node.actual_rows.unwrap_or(0) >= 1000 {
                node.warnings
                    .push("Full table scan on large table".to_string());
                if let Some(ref relation) = node.relation_name {
                    if let Some(ref filter) = node.filter {
                        // Try to extract column names for index suggestion
                        node.recommendations.push(format!(
                            "Consider adding an index on '{}' for filter: {}",
                            relation, filter
                        ));
                    } else {
                        node.recommendations.push(format!(
                            "Consider if all rows from '{}' are needed",
                            relation
                        ));
                    }
                }
            }
        }
        "Sort" => {
            if let Some(ref space_type) = node.sort_space_type {
                if space_type == "Disk" {
                    node.warnings.push("Sort spilled to disk".to_string());
                    node.recommendations
                        .push("Consider increasing work_mem".to_string());
                    if !node.sort_key.is_empty() {
                        node.recommendations.push(format!(
                            "Or add index on sort columns: {}",
                            node.sort_key.join(", ")
                        ));
                    }
                }
            }
        }
        "Hash Join" | "Nested Loop" | "Merge Join" => {
            if let Some(time) = node.actual_time_ms {
                if time > 100.0 {
                    node.warnings.push("Slow join operation".to_string());
                    node.recommendations
                        .push("Ensure join columns are indexed".to_string());
                }
            }
        }
        "Bitmap Heap Scan" => {
            if let Some(removed) = node.rows_removed_by_filter {
                if let Some(actual) = node.actual_rows {
                    if removed > actual * 10 {
                        node.warnings
                            .push("Many rows removed by recheck".to_string());
                        node.recommendations
                            .push("Consider a more selective index".to_string());
                    }
                }
            }
        }
        "Aggregate" | "HashAggregate" | "GroupAggregate" => {
            if let Some(time) = node.actual_time_ms {
                if time > 50.0 && !node.group_key.is_empty() {
                    node.recommendations.push(format!(
                        "Consider index on GROUP BY columns: {}",
                        node.group_key.join(", ")
                    ));
                }
            }
        }
        _ => {}
    }

    // Check for poor row estimates
    if let Some(actual) = node.actual_rows {
        if node.plan_rows > 0 {
            let ratio = actual as f64 / node.plan_rows as f64;
            if ratio > 10.0 || ratio < 0.1 {
                node.warnings.push(format!(
                    "Row estimate off by {:.0}x (est: {}, actual: {})",
                    ratio.max(1.0 / ratio),
                    node.plan_rows,
                    actual
                ));
                node.recommendations
                    .push("Run ANALYZE to update statistics".to_string());
            }
        }
    }
}

/// Statistics about the entire plan
#[derive(Debug, Clone)]
pub struct PlanStatistics {
    pub total_nodes: usize,
    pub max_depth: usize,
    pub total_cost: f64,
    pub total_time: Option<f64>,
    pub tables_involved: Vec<String>,
    pub indexes_used: Vec<String>,
    pub total_warnings: usize,
    pub total_recommendations: usize,
    pub has_seq_scans: bool,
    pub has_sort_spill: bool,
    pub performance_score: u8,
}

impl PlanStatistics {
    /// Calculate statistics from a plan tree
    pub fn from_plan(root: &PlanNode) -> Self {
        let tables = root.get_tables();
        let indexes = root.get_indexes();

        let has_seq_scans = Self::check_seq_scans(root);
        let has_sort_spill = Self::check_sort_spill(root);

        let performance_score = Self::calculate_score(root);

        Self {
            total_nodes: root.node_count(),
            max_depth: root.max_depth(),
            total_cost: root.total_subtree_cost(),
            total_time: root.total_subtree_time(),
            tables_involved: tables,
            indexes_used: indexes,
            total_warnings: root.count_warnings(),
            total_recommendations: root.count_recommendations(),
            has_seq_scans,
            has_sort_spill,
            performance_score,
        }
    }

    fn check_seq_scans(node: &PlanNode) -> bool {
        if node.node_type == "Seq Scan" && node.plan_rows >= 1000 {
            return true;
        }
        node.children.iter().any(Self::check_seq_scans)
    }

    fn check_sort_spill(node: &PlanNode) -> bool {
        if node.sort_space_type.as_deref() == Some("Disk") {
            return true;
        }
        node.children.iter().any(Self::check_sort_spill)
    }

    fn calculate_score(node: &PlanNode) -> u8 {
        let scores: Vec<u8> = Self::collect_scores(node);
        if scores.is_empty() {
            return 100;
        }
        (scores.iter().map(|&s| s as u32).sum::<u32>() / scores.len() as u32) as u8
    }

    fn collect_scores(node: &PlanNode) -> Vec<u8> {
        let mut scores = vec![match node.performance_level {
            PerformanceLevel::Excellent => 100,
            PerformanceLevel::Good => 80,
            PerformanceLevel::Warning => 60,
            PerformanceLevel::Poor => 40,
            PerformanceLevel::Critical => 20,
        }];
        for child in &node.children {
            scores.extend(Self::collect_scores(child));
        }
        scores
    }

    /// Get a letter grade based on the score
    pub fn grade(&self) -> &'static str {
        match self.performance_score {
            90..=100 => "A+",
            80..=89 => "A",
            70..=79 => "B",
            60..=69 => "C",
            50..=59 => "D",
            _ => "F",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_plan() {
        let plan_json = serde_json::json!([{
            "Plan": {
                "Node Type": "Seq Scan",
                "Relation Name": "users",
                "Schema": "public",
                "Startup Cost": 0.0,
                "Total Cost": 123.45,
                "Plan Rows": 1000,
                "Actual Rows": 950,
                "Actual Total Time": 12.5
            }
        }]);

        let plan = parse_postgresql_plan(&plan_json).unwrap();
        assert_eq!(plan.node_type, "Seq Scan");
        assert_eq!(plan.relation_name, Some("users".to_string()));
        assert_eq!(plan.schema, Some("public".to_string()));
        assert_eq!(plan.total_cost, 123.45);
        assert_eq!(plan.actual_rows, Some(950));
    }

    #[test]
    fn test_parse_nested_plan() {
        let plan_json = serde_json::json!([{
            "Plan": {
                "Node Type": "Hash Join",
                "Total Cost": 500.0,
                "Plan Rows": 100,
                "Hash Cond": "(a.id = b.a_id)",
                "Plans": [
                    {
                        "Node Type": "Seq Scan",
                        "Relation Name": "table_a",
                        "Total Cost": 100.0,
                        "Plan Rows": 50
                    },
                    {
                        "Node Type": "Hash",
                        "Total Cost": 200.0,
                        "Plan Rows": 200,
                        "Plans": [
                            {
                                "Node Type": "Seq Scan",
                                "Relation Name": "table_b",
                                "Total Cost": 150.0,
                                "Plan Rows": 200
                            }
                        ]
                    }
                ]
            }
        }]);

        let plan = parse_postgresql_plan(&plan_json).unwrap();
        assert_eq!(plan.node_type, "Hash Join");
        assert_eq!(plan.children.len(), 2);
        assert_eq!(plan.node_count(), 4);
        assert_eq!(plan.max_depth(), 3);
    }

    #[test]
    fn test_plan_statistics() {
        let plan_json = serde_json::json!([{
            "Plan": {
                "Node Type": "Index Scan",
                "Relation Name": "orders",
                "Index Name": "orders_pkey",
                "Total Cost": 50.0,
                "Plan Rows": 10,
                "Actual Rows": 10,
                "Actual Total Time": 0.5
            }
        }]);

        let plan = parse_postgresql_plan(&plan_json).unwrap();
        let stats = PlanStatistics::from_plan(&plan);

        assert_eq!(stats.total_nodes, 1);
        assert_eq!(stats.tables_involved, vec!["orders".to_string()]);
        assert_eq!(stats.indexes_used, vec!["orders_pkey".to_string()]);
        assert!(!stats.has_seq_scans);
    }
}
