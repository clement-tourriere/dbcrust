//! Plan view model: turns the raw EXPLAIN output of any engine into
//! something a tree view can draw.
//!
//! - PostgreSQL `EXPLAIN (FORMAT JSON)`: a full tree (costs, estimated vs
//!   actual rows, timings, buffers) built on `dbcrust::explain_tui`.
//! - SQLite `EXPLAIN QUERY PLAN`: a tree from parent ids, with scans,
//!   searches and temporary B-trees classified.
//! - MySQL: `FORMAT=JSON` blocks as a tree, the classic table as flat nodes.
//! - DataFusion: one titled text block per plan type.
//! - Anything else: JSON pretty-printed, or the cells as text.
//!
//! Everything here is pure and unit-tested; the GPUI view only draws it.

use crate::types::ExplainRaw;
use dbcrust::database::DatabaseType;
use dbcrust::explain_tui::{PlanNode, PlanStatistics, parse_postgresql_plan};
use dbcrust::performance_analyzer::PerformanceLevel;
use serde_json::Value;

/// A row estimate this many times off (either way) is flagged.
pub const ESTIMATE_MISMATCH_FACTOR: f64 = 10.0;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// How concerning a node is, for the marker color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Good,
    Neutral,
    Warning,
    Critical,
}

/// What a node does, for its icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// Reads a whole table.
    Scan,
    /// Uses an index (scan, search, lookup).
    IndexScan,
    Join,
    Sort,
    Aggregate,
    /// Subquery, CTE, materialization.
    Subquery,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub key: String,
    pub value: String,
    /// Render the value in the monospace font (expressions, identifiers).
    pub code: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PropertyGroup {
    pub title: &'static str,
    pub items: Vec<Property>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanNodeModel {
    /// Position in [`PlanTree::nodes`] (depth-first order).
    pub index: usize,
    pub depth: usize,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// Last child of its parent (draws `└` instead of `├`).
    pub is_last: bool,
    /// For depth `d >= 1`, `d - 1` flags: whether the ancestor at depth
    /// `1..d` has later siblings (a vertical guide line is drawn in that
    /// column).
    pub guides: Vec<bool>,
    pub title: String,
    pub relation: Option<String>,
    pub index_name: Option<String>,
    /// Short condition / key summary shown after the name.
    pub detail: Option<String>,
    pub kind: NodeKind,
    pub severity: Severity,
    pub startup_cost: Option<f64>,
    pub total_cost: Option<f64>,
    /// `total_cost` minus the children's, clamped at zero.
    pub exclusive_cost: Option<f64>,
    pub plan_rows: Option<u64>,
    pub actual_rows: Option<u64>,
    /// Per-loop time in milliseconds.
    pub actual_time_ms: Option<f64>,
    pub actual_loops: Option<u64>,
    /// Time spent in this node alone (all loops), clamped at zero.
    pub exclusive_time_ms: Option<f64>,
    /// Share (0..=1) of the plan's exclusive time (when analyzed) or cost.
    pub share: f64,
    /// How many times the row estimate is off, when above
    /// [`ESTIMATE_MISMATCH_FACTOR`].
    pub estimate_factor: Option<f64>,
    pub warnings: Vec<String>,
    pub recommendations: Vec<String>,
    pub properties: Vec<PropertyGroup>,
}

impl PlanNodeModel {
    /// `Seq Scan on orders using idx`, for lists and tooltips.
    pub fn label(&self) -> String {
        let mut label = self.title.clone();
        if let Some(relation) = &self.relation {
            label.push_str(&format!(" on {relation}"));
        }
        if let Some(index) = &self.index_name {
            label.push_str(&format!(" using {index}"));
        }
        label
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanTree {
    /// Depth-first order; roots have depth 0.
    pub nodes: Vec<PlanNodeModel>,
    pub has_costs: bool,
    pub has_actuals: bool,
}

/// One thing worth a look in the plan: an issue on a node and the advice
/// that goes with it. Unique per node and issue, so the same finding never
/// shows up twice.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanObservation {
    pub node: usize,
    /// The node, as in `Seq Scan on orders`.
    pub label: String,
    /// `Neutral` for a suggestion that has no warning behind it.
    pub severity: Severity,
    pub issue: String,
    pub advice: Vec<String>,
}

/// `items` without repeats (ignoring case and surrounding whitespace),
/// first occurrence wins, empty entries dropped.
fn distinct(items: &[String]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for item in items {
        let item = item.trim();
        let key = item.to_lowercase();
        if item.is_empty() || seen.contains(&key) {
            continue;
        }
        seen.push(key);
        out.push(item.to_string());
    }
    out
}

impl PlanTree {
    /// Every warning and suggestion of the plan, in plan order, one entry
    /// per node and issue. Warnings and recommendations are paired up when
    /// there are as many of each; otherwise the node's advice goes with its
    /// first warning. A node with suggestions but no warning gets one
    /// `Neutral` entry per suggestion.
    pub fn observations(&self) -> Vec<PlanObservation> {
        let mut out = Vec::new();
        for node in &self.nodes {
            let warnings = distinct(&node.warnings);
            let advice = distinct(&node.recommendations);
            let label = node.label();
            if warnings.is_empty() {
                out.extend(advice.into_iter().map(|issue| PlanObservation {
                    node: node.index,
                    label: label.clone(),
                    severity: Severity::Neutral,
                    issue,
                    advice: Vec::new(),
                }));
                continue;
            }
            let paired = warnings.len() == advice.len();
            for (position, issue) in warnings.iter().enumerate() {
                let advice = if paired {
                    vec![advice[position].clone()]
                } else if position == 0 {
                    advice.clone()
                } else {
                    Vec::new()
                };
                out.push(PlanObservation {
                    node: node.index,
                    label: label.clone(),
                    severity: node.severity.max(Severity::Warning),
                    issue: issue.clone(),
                    advice,
                });
            }
        }
        out
    }

    /// First node with the highest exclusive share: the "hot spot".
    pub fn hottest(&self) -> Option<usize> {
        self.nodes
            .iter()
            .filter(|node| node.share > 0.0)
            .max_by(|a, b| a.share.total_cmp(&b.share))
            .map(|node| node.index)
    }
}

/// Heuristic grade: PostgreSQL uses `PlanStatistics` from DBCrust, other
/// engines a simple penalty score. Always shown as "heuristic".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanGrade {
    pub letter: String,
    pub score: u8,
}

impl PlanGrade {
    fn from_score(score: u8) -> Self {
        let letter = match score {
            90..=100 => "A+",
            80..=89 => "A",
            70..=79 => "B",
            60..=69 => "C",
            50..=59 => "D",
            _ => "F",
        };
        Self {
            letter: letter.to_string(),
            score,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlanSummary {
    pub total_cost: Option<f64>,
    pub plan_rows: Option<u64>,
    pub actual_rows: Option<u64>,
    pub planning_ms: Option<f64>,
    pub execution_ms: Option<f64>,
    pub node_count: usize,
    /// Warnings found on the nodes (see [`PlanTree::observations`] for the
    /// de-duplicated list the view shows).
    pub warning_count: usize,
    /// Distinct observations: what the side panel lists.
    pub observation_count: usize,
    pub full_scans: usize,
    pub grade: Option<PlanGrade>,
    pub tables: Vec<String>,
    pub indexes: Vec<String>,
}

/// A timing shown in the plan header, named for what it really measures.
#[derive(Debug, Clone, PartialEq)]
pub struct TimingStat {
    pub label: &'static str,
    pub value: String,
    pub tooltip: &'static str,
}

impl TimingStat {
    fn new(label: &'static str, ms: f64, tooltip: &'static str) -> Self {
        Self {
            label,
            value: format_ms(ms),
            tooltip,
        }
    }
}

/// The timings of a plan, honestly labelled. `elapsed_ms` is how long the
/// request took end to end: without `ANALYZE` that is only the time to
/// collect the plan (the statement did not run); with it, the statement ran
/// and the engine's own execution figure, when it reports one, is what
/// counts.
pub fn timing_stats(analyzed: bool, elapsed_ms: f64, summary: &PlanSummary) -> Vec<TimingStat> {
    let mut stats = Vec::new();
    if analyzed {
        match summary.execution_ms {
            Some(execution) => {
                stats.push(TimingStat::new(
                    "Execution",
                    execution,
                    "Measured by the engine while it ran the statement (EXPLAIN ANALYZE)",
                ));
                if let Some(planning) = summary.planning_ms {
                    stats.push(TimingStat::new(
                        "Planning",
                        planning,
                        "Time the engine spent planning the statement",
                    ));
                }
                stats.push(TimingStat::new(
                    "Round trip",
                    elapsed_ms,
                    "Request to answer, including the connection and running the statement",
                ));
            }
            None => stats.push(TimingStat::new(
                "Ran and explained in",
                elapsed_ms,
                "The statement was executed to measure the plan; this is the whole request",
            )),
        }
    } else {
        stats.push(TimingStat::new(
            "Plan collected in",
            elapsed_ms,
            "Time to get this plan from the engine. The statement was not executed.",
        ));
        if let Some(planning) = summary.planning_ms {
            stats.push(TimingStat::new(
                "Planning",
                planning,
                "Time the engine spent planning the statement",
            ));
        }
    }
    stats
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanSection {
    pub title: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlanBody {
    Tree(PlanTree),
    /// One titled monospace block per plan type (DataFusion).
    Sections(Vec<PlanSection>),
    /// A plan JSON we could not turn into a tree.
    Json(String),
    /// A plain grid (unrecognised multi-column output).
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Text(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanModel {
    pub body: PlanBody,
    pub summary: PlanSummary,
    /// What "Copy plan" copies and the Raw view shows: the JSON document
    /// (for explain.dalibo.com and friends) or the cells as text.
    pub raw: String,
    pub raw_is_json: bool,
    /// Something the user should know about this engine's plan.
    pub note: Option<String>,
}

impl PlanModel {
    /// Build the model for `raw`. Never fails: unrecognised output falls
    /// back to text.
    pub fn from_raw(raw: &ExplainRaw) -> PlanModel {
        let built = match raw.db_type {
            DatabaseType::PostgreSQL => postgres_model(raw),
            DatabaseType::SQLite | DatabaseType::LibSql => sqlite_model(raw),
            DatabaseType::MySQL => mysql_model(raw),
            _ => None,
        };
        built
            .or_else(|| sections_model(raw))
            .or_else(|| json_model(raw))
            .unwrap_or_else(|| text_model(raw))
    }

    pub fn tree(&self) -> Option<&PlanTree> {
        match &self.body {
            PlanBody::Tree(tree) => Some(tree),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Formatting helpers (shared with the view)
// ---------------------------------------------------------------------------

/// `1234` → `1,234`; `12_345_678` → `12.3M`.
pub fn format_count(value: u64) -> String {
    if value >= 10_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        let digits = value.to_string();
        let mut out = String::new();
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        out
    } else {
        value.to_string()
    }
}

/// Planner cost units, compact: `0.43`, `128.5`, `12.3k`.
pub fn format_cost(value: f64) -> String {
    if value >= 1_000_000.0 {
        format!("{:.1}M", value / 1_000_000.0)
    } else if value >= 10_000.0 {
        format!("{:.1}k", value / 1_000.0)
    } else if value >= 100.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

/// Milliseconds: `0.12 ms`, `12.3 ms`, `1.23 s`, `2m 5s`.
pub fn format_ms(ms: f64) -> String {
    if ms < 1.0 {
        format!("{ms:.2} ms")
    } else if ms < 100.0 {
        format!("{ms:.1} ms")
    } else if ms < 1_000.0 {
        format!("{ms:.0} ms")
    } else if ms < 60_000.0 {
        format!("{:.2} s", ms / 1_000.0)
    } else {
        let seconds = (ms / 1_000.0).round() as u64;
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}

// ---------------------------------------------------------------------------
// Tree assembly
// ---------------------------------------------------------------------------

/// A node before depths, indices and shares are known.
#[derive(Default)]
struct Draft {
    title: String,
    relation: Option<String>,
    index_name: Option<String>,
    detail: Option<String>,
    kind: Option<NodeKind>,
    severity: Option<Severity>,
    startup_cost: Option<f64>,
    total_cost: Option<f64>,
    /// Exclusive cost when the engine reports it directly.
    own_cost: Option<f64>,
    plan_rows: Option<u64>,
    actual_rows: Option<u64>,
    actual_time_ms: Option<f64>,
    actual_loops: Option<u64>,
    warnings: Vec<String>,
    recommendations: Vec<String>,
    properties: Vec<PropertyGroup>,
    children: Vec<Draft>,
}

fn assemble(roots: Vec<Draft>) -> PlanTree {
    let mut nodes: Vec<PlanNodeModel> = Vec::new();
    let count = roots.len();
    for (position, root) in roots.into_iter().enumerate() {
        flatten(root, None, 0, position + 1 == count, &[], &mut nodes);
    }

    // exclusive cost / time
    let child_sums: Vec<(f64, f64)> = nodes
        .iter()
        .map(|node| {
            node.children
                .iter()
                .fold((0.0, 0.0), |(cost, time), &child| {
                    let child = &nodes[child];
                    (
                        cost + child.total_cost.unwrap_or(0.0),
                        time + total_time(child).unwrap_or(0.0),
                    )
                })
        })
        .collect();
    for (node, (child_cost, child_time)) in nodes.iter_mut().zip(child_sums) {
        if node.exclusive_cost.is_none() {
            node.exclusive_cost = node.total_cost.map(|total| (total - child_cost).max(0.0));
        }
        node.exclusive_time_ms = total_time(node).map(|total| (total - child_time).max(0.0));
    }

    let has_costs = nodes.iter().any(|node| node.total_cost.is_some());
    let has_actuals = nodes.iter().any(|node| node.actual_time_ms.is_some());

    let weight = |node: &PlanNodeModel| -> f64 {
        if has_actuals {
            node.exclusive_time_ms.unwrap_or(0.0)
        } else {
            node.exclusive_cost.unwrap_or(0.0)
        }
    };
    let total: f64 = nodes.iter().map(weight).sum();
    if total > 0.0 {
        let shares: Vec<f64> = nodes.iter().map(|node| weight(node) / total).collect();
        for (node, share) in nodes.iter_mut().zip(shares) {
            node.share = share;
        }
    }

    // estimate mismatch
    for node in &mut nodes {
        if node.actual_loops == Some(0) {
            continue;
        }
        if let (Some(estimated), Some(actual)) = (node.plan_rows, node.actual_rows) {
            let e = estimated.max(1) as f64;
            let a = actual.max(1) as f64;
            let factor = (a / e).max(e / a);
            if factor > ESTIMATE_MISMATCH_FACTOR {
                node.estimate_factor = Some(factor);
                node.warnings.push(format!(
                    "Row estimate off by {factor:.0}x (estimated {}, actual {})",
                    format_count(estimated),
                    format_count(actual)
                ));
                node.recommendations.push(
                    "Refresh the table statistics (ANALYZE) so the planner can estimate better"
                        .to_string(),
                );
                node.severity = node.severity.max(Severity::Warning);
            }
        }
    }

    PlanTree {
        nodes,
        has_costs,
        has_actuals,
    }
}

fn total_time(node: &PlanNodeModel) -> Option<f64> {
    node.actual_time_ms
        .map(|time| time * node.actual_loops.unwrap_or(1) as f64)
}

fn flatten(
    draft: Draft,
    parent: Option<usize>,
    depth: usize,
    is_last: bool,
    parent_guides: &[bool],
    nodes: &mut Vec<PlanNodeModel>,
) -> usize {
    let index = nodes.len();
    // guides of this node: its parent's guides, plus (for depth >= 2) the
    // parent's own "has later siblings" flag
    let guides: Vec<bool> = if depth >= 2 {
        let mut guides = parent_guides.to_vec();
        guides.push(parent.map(|p| !nodes[p].is_last).unwrap_or(false));
        guides
    } else {
        Vec::new()
    };
    let kind = draft.kind.unwrap_or(NodeKind::Other);
    let severity = draft
        .severity
        .unwrap_or(Severity::Neutral)
        .max(if draft.warnings.is_empty() {
            Severity::Good
        } else {
            Severity::Warning
        });
    nodes.push(PlanNodeModel {
        index,
        depth,
        parent,
        children: Vec::new(),
        is_last,
        guides: guides.clone(),
        title: draft.title,
        relation: draft.relation,
        index_name: draft.index_name,
        detail: draft.detail,
        kind,
        severity,
        startup_cost: draft.startup_cost,
        total_cost: draft.total_cost,
        exclusive_cost: draft.own_cost,
        plan_rows: draft.plan_rows,
        actual_rows: draft.actual_rows,
        actual_time_ms: draft.actual_time_ms,
        actual_loops: draft.actual_loops,
        exclusive_time_ms: None,
        share: 0.0,
        estimate_factor: None,
        warnings: draft.warnings,
        recommendations: draft.recommendations,
        properties: draft.properties,
    });
    let count = draft.children.len();
    let mut child_indices = Vec::with_capacity(count);
    for (position, child) in draft.children.into_iter().enumerate() {
        let child_index = flatten(
            child,
            Some(index),
            depth + 1,
            position + 1 == count,
            &guides,
            nodes,
        );
        child_indices.push(child_index);
    }
    nodes[index].children = child_indices;
    index
}

fn summarize(tree: &PlanTree) -> PlanSummary {
    let mut summary = PlanSummary {
        node_count: tree.nodes.len(),
        warning_count: tree.nodes.iter().map(|node| node.warnings.len()).sum(),
        observation_count: tree.observations().len(),
        ..PlanSummary::default()
    };
    let roots: Vec<&PlanNodeModel> = tree
        .nodes
        .iter()
        .filter(|node| node.parent.is_none())
        .collect();
    if tree.has_costs {
        summary.total_cost = Some(roots.iter().filter_map(|node| node.total_cost).sum());
    }
    summary.plan_rows = roots.first().and_then(|node| node.plan_rows);
    summary.actual_rows = roots.first().and_then(|node| node.actual_rows);
    for node in &tree.nodes {
        if node.kind == NodeKind::Scan && node.severity >= Severity::Warning {
            summary.full_scans += 1;
        }
        if let Some(relation) = &node.relation
            && !summary.tables.contains(relation)
        {
            summary.tables.push(relation.clone());
        }
        if let Some(index) = &node.index_name
            && !summary.indexes.contains(index)
        {
            summary.indexes.push(index.clone());
        }
    }
    summary
}

/// A rough score for engines without DBCrust's own plan statistics: start
/// at 100 and subtract per finding (full scans weigh most, a temporary
/// sort least).
fn heuristic_grade(tree: &PlanTree) -> PlanGrade {
    let penalty: u32 = tree
        .nodes
        .iter()
        .map(|node| match (node.severity, node.kind) {
            (Severity::Critical, _) => 30,
            (Severity::Warning, NodeKind::Sort) => 5,
            (Severity::Warning, NodeKind::Scan) => 20,
            (Severity::Warning, NodeKind::Subquery) => 15,
            (Severity::Warning, _) => 10,
            _ => 0,
        })
        .sum();
    PlanGrade::from_score(100u32.saturating_sub(penalty).max(20) as u8)
}

/// What to say about a full scan. A scan alone does not make a bad plan:
/// small tables are cheapest to read whole, so the advice is conditional.
fn scan_advice(table: &str) -> String {
    format!(
        "Fine while {table} is small. If it is large and the query's filter is selective, an index on the filtered or joined columns may help"
    )
}

const FILESORT_ADVICE: &str =
    "On large results, an index matching the ORDER BY could avoid the filesort";

fn property(key: &str, value: impl Into<String>, code: bool) -> Property {
    Property {
        key: key.to_string(),
        value: value.into(),
        code,
    }
}

// ---------------------------------------------------------------------------
// PostgreSQL
// ---------------------------------------------------------------------------

fn postgres_model(raw: &ExplainRaw) -> Option<PlanModel> {
    let cell = raw.rows.get(1)?.first()?;
    let document: Value = serde_json::from_str(cell.trim()).ok()?;
    let root_plan = parse_postgresql_plan(&document)?;
    let raw_root = document.get(0)?.get("Plan")?;

    let draft = pg_draft(&root_plan, raw_root);
    let tree = assemble(vec![draft]);
    let mut summary = summarize(&tree);
    let top = document.get(0)?;
    summary.planning_ms = top.get("Planning Time").and_then(Value::as_f64);
    summary.execution_ms = top.get("Execution Time").and_then(Value::as_f64);
    let statistics = PlanStatistics::from_plan(&root_plan);
    summary.grade = Some(PlanGrade {
        letter: statistics.grade().to_string(),
        score: statistics.performance_score,
    });

    let pretty = serde_json::to_string_pretty(&document).unwrap_or_else(|_| cell.clone());
    Some(PlanModel {
        body: PlanBody::Tree(tree),
        summary,
        raw: pretty,
        raw_is_json: true,
        note: None,
    })
}

fn pg_draft(node: &PlanNode, raw: &Value) -> Draft {
    let object = raw.as_object();
    let get = |key: &str| object.and_then(|o| o.get(key));
    let text = |key: &str| get(key).and_then(Value::as_str).map(str::to_string);
    let number = |key: &str| get(key).and_then(Value::as_u64);

    let parallel = get("Parallel Aware")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut title = node.node_type.clone();
    if parallel && !title.starts_with("Parallel") && !title.starts_with("Gather") {
        title = format!("Parallel {title}");
    }

    let relation = node.relation_name.as_ref().map(|name| match &node.schema {
        Some(schema) if schema != "public" => format!("{schema}.{name}"),
        _ => name.clone(),
    });

    let mut detail_parts: Vec<String> = Vec::new();
    if let Some(join) = text("Join Type")
        .filter(|_| node.node_type.contains("Join") || node.node_type == "Nested Loop")
    {
        detail_parts.push(format!("{} join", join.to_lowercase()));
    }
    if let Some(cond) = [
        &node.hash_cond,
        &node.merge_cond,
        &node.join_filter,
        &node.index_cond,
        &node.filter,
    ]
    .into_iter()
    .flatten()
    .next()
    {
        detail_parts.push(cond.clone());
    }
    if !node.sort_key.is_empty() {
        detail_parts.push(node.sort_key.join(", "));
    } else if !node.group_key.is_empty() {
        detail_parts.push(format!("by {}", node.group_key.join(", ")));
    }
    if let Some(strategy) = text("Strategy").filter(|_| node.node_type.contains("Aggregate")) {
        detail_parts.insert(0, strategy.to_lowercase());
    }
    let detail = if detail_parts.is_empty() {
        None
    } else {
        Some(detail_parts.join(" · "))
    };

    let kind = match node.node_type.as_str() {
        "Seq Scan" => NodeKind::Scan,
        t if t.contains("Index") || t == "Bitmap Heap Scan" => NodeKind::IndexScan,
        t if t.ends_with("Join") || t == "Nested Loop" => NodeKind::Join,
        t if t.contains("Sort") => NodeKind::Sort,
        t if t.contains("Aggregate") || t == "WindowAgg" || t == "Group" => NodeKind::Aggregate,
        "CTE Scan" | "Subquery Scan" | "Materialize" | "Memoize" | "WorkTable Scan" => {
            NodeKind::Subquery
        }
        t if t.contains("Scan") => NodeKind::Scan,
        _ => NodeKind::Other,
    };
    let severity = match node.performance_level {
        PerformanceLevel::Excellent => Severity::Good,
        PerformanceLevel::Good => Severity::Neutral,
        PerformanceLevel::Warning | PerformanceLevel::Poor => Severity::Warning,
        PerformanceLevel::Critical => Severity::Critical,
    };

    // DBCrust's own "row estimate" warning is replaced by the shared one
    let mut warnings: Vec<String> = node
        .warnings
        .iter()
        .filter(|warning| !warning.starts_with("Row estimate off"))
        .cloned()
        .collect();
    let mut recommendations: Vec<String> = node
        .recommendations
        .iter()
        .filter(|rec| rec.as_str() != "Run ANALYZE to update statistics")
        .cloned()
        .collect();

    if let (Some(removed), loops) = (
        number("Rows Removed by Filter"),
        node.actual_loops.unwrap_or(1),
    ) {
        let produced = node.actual_rows.unwrap_or(0);
        if removed > 1_000 && removed > produced.saturating_mul(10) {
            warnings.push(format!(
                "Filter discards {} rows to return {}{}",
                format_count(removed),
                format_count(produced),
                if loops > 1 { " (per loop)" } else { "" }
            ));
            recommendations.push(
                "If this filter runs often, an index matching it may avoid reading the discarded rows"
                    .to_string(),
            );
        }
    }
    if let (Some(planned), Some(launched)) = (node.workers_planned, node.workers_launched)
        && launched < planned
    {
        warnings.push(format!(
            "Only {launched} of {planned} parallel workers were launched"
        ));
    }

    Draft {
        title,
        relation,
        index_name: node.index_name.clone(),
        detail,
        kind: Some(kind),
        severity: Some(severity),
        startup_cost: Some(node.startup_cost),
        total_cost: Some(node.total_cost),
        own_cost: None,
        plan_rows: Some(node.plan_rows),
        actual_rows: node.actual_rows,
        actual_time_ms: node.actual_time_ms,
        actual_loops: node.actual_loops,
        warnings,
        recommendations,
        properties: pg_properties(raw),
        children: {
            let raw_children: Vec<&Value> = raw
                .get("Plans")
                .and_then(Value::as_array)
                .map(|plans| plans.iter().filter(|plan| plan.is_object()).collect())
                .unwrap_or_default();
            node.children
                .iter()
                .zip(raw_children)
                .map(|(child, raw_child)| pg_draft(child, raw_child))
                .collect()
        },
    }
}

/// Property groups for the details panel, in a stable, meaningful order.
fn pg_properties(raw: &Value) -> Vec<PropertyGroup> {
    const GROUPS: &[(&str, &[&str])] = &[
        (
            "Operation",
            &[
                "Node Type",
                "Parent Relationship",
                "Join Type",
                "Strategy",
                "Partial Mode",
                "Scan Direction",
                "Subplan Name",
                "CTE Name",
                "Parallel Aware",
                "Async Capable",
                "Inner Unique",
            ],
        ),
        (
            "Access",
            &[
                "Relation Name",
                "Schema",
                "Alias",
                "Index Name",
                "Index Cond",
                "Recheck Cond",
                "TID Cond",
                "Filter",
                "Join Filter",
                "Hash Cond",
                "Merge Cond",
                "Sort Key",
                "Presorted Key",
                "Group Key",
                "Function Name",
                "Output",
                "Rows Removed by Filter",
                "Rows Removed by Join Filter",
                "Rows Removed by Index Recheck",
                "Heap Fetches",
                "Exact Heap Blocks",
                "Lossy Heap Blocks",
            ],
        ),
        (
            "Estimates",
            &["Startup Cost", "Total Cost", "Plan Rows", "Plan Width"],
        ),
        (
            "Actual",
            &[
                "Actual Startup Time",
                "Actual Total Time",
                "Actual Rows",
                "Actual Loops",
                "Workers Planned",
                "Workers Launched",
                "Sort Method",
                "Sort Space Used",
                "Sort Space Type",
                "Hash Buckets",
                "Hash Batches",
                "Original Hash Batches",
                "Peak Memory Usage",
                "Cache Hits",
                "Cache Misses",
            ],
        ),
    ];
    const CODE_KEYS: &[&str] = &[
        "Index Cond",
        "Recheck Cond",
        "TID Cond",
        "Filter",
        "Join Filter",
        "Hash Cond",
        "Merge Cond",
        "Sort Key",
        "Presorted Key",
        "Group Key",
        "Output",
        "Relation Name",
        "Schema",
        "Alias",
        "Index Name",
        "Function Name",
        "CTE Name",
        "Subplan Name",
    ];

    let Some(object) = raw.as_object() else {
        return Vec::new();
    };
    let mut used: Vec<&str> = vec!["Plans"];
    let mut groups: Vec<PropertyGroup> = Vec::new();

    for (title, keys) in GROUPS {
        let items: Vec<Property> = keys
            .iter()
            .filter_map(|key| {
                object.get(*key).map(|value| {
                    used.push(key);
                    property(key, format_pg_value(key, value), CODE_KEYS.contains(key))
                })
            })
            .collect();
        if !items.is_empty() {
            groups.push(PropertyGroup { title, items });
        }
    }

    let mut buffers: Vec<Property> = Vec::new();
    let mut other: Vec<Property> = Vec::new();
    for (key, value) in object {
        if used.contains(&key.as_str()) {
            continue;
        }
        let item = property(key, format_pg_value(key, value), false);
        if key.starts_with("Shared ")
            || key.starts_with("Local ")
            || key.starts_with("Temp ")
            || key.starts_with("I/O ")
            || key.starts_with("WAL ")
        {
            buffers.push(item);
        } else {
            other.push(item);
        }
    }
    if !buffers.is_empty() {
        groups.push(PropertyGroup {
            title: "Buffers",
            items: buffers,
        });
    }
    if !other.is_empty() {
        groups.push(PropertyGroup {
            title: "Other",
            items: other,
        });
    }
    groups
}

fn format_pg_value(key: &str, value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(flag) => if *flag { "yes" } else { "no" }.to_string(),
        Value::Number(number) => {
            if key.ends_with("Time") {
                format_ms(number.as_f64().unwrap_or(0.0))
            } else if key.ends_with("Cost") {
                format!("{:.2}", number.as_f64().unwrap_or(0.0))
            } else if let Some(integer) = number.as_u64() {
                let formatted = format_count(integer);
                if key == "Sort Space Used" || key == "Peak Memory Usage" {
                    format!("{formatted} kB")
                } else {
                    formatted
                }
            } else {
                number.to_string()
            }
        }
        Value::Array(items) if items.iter().all(Value::is_string) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// SQLite
// ---------------------------------------------------------------------------

fn sqlite_model(raw: &ExplainRaw) -> Option<PlanModel> {
    let data = raw.rows.get(1..)?;
    if data.is_empty() || data.iter().any(|row| row.len() < 2) {
        return None;
    }
    struct Row {
        id: i64,
        parent: i64,
        detail: String,
    }
    let mut parsed: Vec<Row> = Vec::new();
    for row in data {
        let id = row.first()?.trim().parse().ok()?;
        let parent = row.get(1)?.trim().parse().ok()?;
        let detail = row.last()?.trim().to_string();
        parsed.push(Row { id, parent, detail });
    }

    fn build(parent: i64, rows: &[Row], visited: &mut Vec<i64>) -> Vec<Draft> {
        rows.iter()
            .filter(|row| row.parent == parent && !visited.contains(&row.id))
            .map(|row| row.id)
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|id| {
                if visited.contains(&id) {
                    return None;
                }
                visited.push(id);
                let row = rows.iter().find(|row| row.id == id)?;
                let mut draft = sqlite_draft(&row.detail);
                draft.children = build(id, rows, visited);
                Some(draft)
            })
            .collect()
    }

    let ids: Vec<i64> = parsed.iter().map(|row| row.id).collect();
    let mut visited = Vec::new();
    let mut roots: Vec<Draft> = Vec::new();
    // top-level parents: those not pointing at another row
    let mut root_parents: Vec<i64> = parsed
        .iter()
        .map(|row| row.parent)
        .filter(|parent| !ids.contains(parent))
        .collect();
    root_parents.dedup();
    for parent in root_parents {
        roots.extend(build(parent, &parsed, &mut visited));
    }
    if roots.is_empty() {
        return None;
    }

    let tree = assemble(roots);
    let mut summary = summarize(&tree);
    summary.grade = Some(heuristic_grade(&tree));

    let raw_text = raw
        .rows
        .iter()
        .skip(1)
        .map(|row| row.last().cloned().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    Some(PlanModel {
        body: PlanBody::Tree(tree),
        summary,
        raw: raw_text,
        raw_is_json: false,
        note: Some("SQLite reports the access strategy but no costs or row estimates.".to_string()),
    })
}

fn sqlite_draft(detail: &str) -> Draft {
    let upper = detail.to_uppercase();
    let mut draft = Draft {
        detail: None,
        properties: vec![PropertyGroup {
            title: "Plan",
            items: vec![property("Detail", detail, true)],
        }],
        ..Draft::default()
    };

    let (verb, rest) = match upper.split_whitespace().next() {
        Some(verb @ ("SCAN" | "SEARCH")) => (verb, detail.trim()[verb.len()..].trim_start()),
        _ => ("", detail),
    };

    if !verb.is_empty() {
        let rest = rest
            .strip_prefix("TABLE ")
            .or_else(|| rest.strip_prefix("table "))
            .unwrap_or(rest);
        let (target, using) = match find_ci(rest, " USING ") {
            Some(at) => (rest[..at].trim(), Some(rest[at + " USING ".len()..].trim())),
            None => (rest.trim(), None),
        };
        let upper_using = using.map(str::to_uppercase).unwrap_or_default();
        let table = target
            .split_whitespace()
            .next()
            .unwrap_or(target)
            .to_string();
        let target_upper = target.to_uppercase();
        let is_virtual =
            target_upper.starts_with("SUBQUERY") || target_upper.starts_with("CONSTANT ROW");
        if !is_virtual {
            draft.relation = Some(table.clone());
        } else {
            draft.relation = Some(target.to_string());
        }

        let index_name = using.and_then(|using| {
            let stripped = using
                .strip_prefix("AUTOMATIC COVERING INDEX")
                .or_else(|| using.strip_prefix("COVERING INDEX"))
                .or_else(|| using.strip_prefix("INDEX"))?;
            let name = stripped.split_whitespace().next()?;
            (!name.starts_with('(')).then(|| name.to_string())
        });
        draft.index_name = index_name;
        draft.detail = using
            .and_then(|using| using.find('(').map(|at| using[at..].to_string()))
            .filter(|text| !text.is_empty());

        if verb == "SCAN" {
            if is_virtual {
                draft.title = "Scan".to_string();
                draft.kind = Some(NodeKind::Subquery);
                draft.severity = Some(Severity::Neutral);
            } else if upper_using.contains("INDEX") {
                draft.title = if upper_using.contains("COVERING") {
                    "Covering index scan"
                } else {
                    "Index scan"
                }
                .to_string();
                draft.kind = Some(NodeKind::IndexScan);
                draft.severity = Some(Severity::Neutral);
            } else {
                draft.title = "Full table scan".to_string();
                draft.kind = Some(NodeKind::Scan);
                draft.severity = Some(Severity::Warning);
                draft.warnings.push(format!("Reads every row of {table}"));
                draft.recommendations.push(scan_advice(&table));
            }
        } else if upper_using.contains("AUTOMATIC") {
            draft.title = "Automatic index".to_string();
            draft.kind = Some(NodeKind::IndexScan);
            draft.severity = Some(Severity::Warning);
            draft.warnings.push(format!(
                "SQLite builds a temporary index on {table} for this query"
            ));
            draft.recommendations.push(format!(
                "If this query runs often, a permanent index on the join or filter columns of {table} may avoid rebuilding it each time"
            ));
        } else if upper_using.contains("PRIMARY KEY") {
            draft.title = "Primary key lookup".to_string();
            draft.kind = Some(NodeKind::IndexScan);
            draft.severity = Some(Severity::Good);
        } else if upper_using.contains("INDEX") {
            draft.title = if upper_using.contains("COVERING") {
                "Covering index search"
            } else {
                "Index search"
            }
            .to_string();
            draft.kind = Some(NodeKind::IndexScan);
            draft.severity = Some(Severity::Good);
        } else {
            draft.title = "Search".to_string();
            draft.kind = Some(NodeKind::Scan);
            draft.severity = Some(Severity::Neutral);
        }
        return draft;
    }

    if upper.starts_with("USE TEMP B-TREE") {
        let purpose = detail
            .trim()
            .get("USE TEMP B-TREE FOR ".len()..)
            .unwrap_or("sorting")
            .to_lowercase();
        draft.title = "Temporary B-tree".to_string();
        draft.detail = Some(purpose.clone());
        draft.kind = Some(NodeKind::Sort);
        draft.severity = Some(Severity::Warning);
        draft
            .warnings
            .push(format!("Builds a temporary B-tree for {purpose}"));
        draft.recommendations.push(
            "Fine for small results. For large ones, an index that matches this step could avoid it, at the cost of slower writes"
                .to_string(),
        );
    } else if upper.contains("CORRELATED") {
        draft.title = "Correlated subquery".to_string();
        draft.detail = Some(detail.to_lowercase());
        draft.kind = Some(NodeKind::Subquery);
        draft.severity = Some(Severity::Warning);
        draft
            .warnings
            .push("Runs once for every row of the outer query".to_string());
        draft.recommendations.push(
            "If the outer query returns many rows, rewriting it as a join or indexing the subquery's lookup column may help"
                .to_string(),
        );
    } else if upper.contains("SUBQUERY") {
        draft.title = "Subquery".to_string();
        draft.detail = Some(detail.to_lowercase());
        draft.kind = Some(NodeKind::Subquery);
        draft.severity = Some(Severity::Neutral);
    } else if upper.starts_with("MATERIALIZE") || upper.starts_with("CO-ROUTINE") {
        let (title, name) = detail.split_once(' ').unwrap_or((detail, ""));
        draft.title = title.to_string();
        draft.relation = Some(name.trim().to_string()).filter(|name| !name.is_empty());
        draft.kind = Some(NodeKind::Subquery);
        draft.severity = Some(Severity::Neutral);
    } else if upper.starts_with("MULTI-INDEX") {
        draft.title = "Multi-index OR".to_string();
        draft.kind = Some(NodeKind::IndexScan);
        draft.severity = Some(Severity::Good);
    } else {
        draft.title = capitalize(&detail.to_lowercase());
        draft.kind = Some(NodeKind::Other);
        draft.severity = Some(Severity::Neutral);
    }
    draft
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .to_ascii_uppercase()
        .find(&needle.to_ascii_uppercase())
}

// ---------------------------------------------------------------------------
// MySQL
// ---------------------------------------------------------------------------

fn mysql_model(raw: &ExplainRaw) -> Option<PlanModel> {
    let header = raw.rows.first()?;
    let data = raw.rows.get(1..)?;
    if data.is_empty() {
        return None;
    }

    // FORMAT=JSON: one cell holding the document (other JSON shapes are
    // left to the generic JSON view)
    if data.len() == 1 && data[0].len() == 1 && data[0][0].trim_start().starts_with(['{', '[']) {
        return mysql_json_model(&data[0][0]);
    }

    // classic EXPLAIN: one row per table access
    let column = |name: &str| {
        header
            .iter()
            .position(|c| c.trim().eq_ignore_ascii_case(name))
    };
    let table_at = column("table")?;
    let type_at = column("type");
    let key_at = column("key");
    let rows_at = column("rows");
    let extra_at = column("extra");
    let roots: Vec<Draft> = data
        .iter()
        .map(|row| {
            let cell = |index: Option<usize>| {
                index
                    .and_then(|i| row.get(i))
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty() && v != "NULL")
            };
            let access = cell(type_at).unwrap_or_default();
            let mut draft = mysql_access_draft(
                &access,
                cell(Some(table_at)),
                cell(key_at),
                cell(rows_at).and_then(|v| v.parse().ok()),
            );
            if let Some(extra) = cell(extra_at) {
                mysql_extra(&mut draft, &extra);
            }
            draft.properties = vec![PropertyGroup {
                title: "Plan",
                items: header
                    .iter()
                    .zip(row.iter())
                    .map(|(key, value)| property(key, value.as_str(), false))
                    .collect(),
            }];
            draft
        })
        .collect();
    let tree = assemble(roots);
    let mut summary = summarize(&tree);
    summary.grade = Some(heuristic_grade(&tree));
    let raw_text = std::iter::once(header.join("\t"))
        .chain(data.iter().map(|row| row.join("\t")))
        .collect::<Vec<_>>()
        .join("\n");
    Some(PlanModel {
        body: PlanBody::Tree(tree),
        summary,
        raw: raw_text,
        raw_is_json: false,
        note: Some("MySQL returned the classic EXPLAIN table (no JSON plan).".to_string()),
    })
}

fn mysql_json_model(text: &str) -> Option<PlanModel> {
    let document: Value = serde_json::from_str(text.trim()).ok()?;
    let block = document.get("query_block")?;
    let query_cost = block
        .pointer("/cost_info/query_cost")
        .and_then(mysql_number);
    let mut roots = mysql_block(block);
    if roots.is_empty() {
        return None;
    }
    if let (Some(cost), Some(first)) = (query_cost, roots.first_mut()) {
        first.total_cost = first.total_cost.or(Some(cost));
    }
    let tree = assemble(roots);
    let mut summary = summarize(&tree);
    summary.total_cost = query_cost.or(summary.total_cost);
    summary.grade = Some(heuristic_grade(&tree));
    Some(PlanModel {
        body: PlanBody::Tree(tree),
        summary,
        raw: serde_json::to_string_pretty(&document).unwrap_or_else(|_| text.to_string()),
        raw_is_json: true,
        note: None,
    })
}

fn mysql_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn mysql_access_draft(
    access: &str,
    table: Option<String>,
    key: Option<String>,
    rows: Option<u64>,
) -> Draft {
    let (title, kind, severity) = match access.to_lowercase().as_str() {
        "all" => ("Full table scan", NodeKind::Scan, Severity::Warning),
        "index" => ("Full index scan", NodeKind::IndexScan, Severity::Warning),
        "range" => ("Index range scan", NodeKind::IndexScan, Severity::Good),
        "ref" | "ref_or_null" | "index_merge" | "fulltext" => {
            ("Index lookup", NodeKind::IndexScan, Severity::Good)
        }
        "eq_ref" => ("Unique key lookup", NodeKind::IndexScan, Severity::Good),
        "const" | "system" => ("Constant row", NodeKind::IndexScan, Severity::Good),
        "" => ("Table access", NodeKind::Scan, Severity::Neutral),
        _ => ("Table access", NodeKind::Scan, Severity::Neutral),
    };
    let mut draft = Draft {
        title: title.to_string(),
        relation: table.clone(),
        index_name: key,
        detail: Some(access.to_string())
            .filter(|a| !a.is_empty())
            .map(|a| format!("type {a}")),
        kind: Some(kind),
        severity: Some(severity),
        plan_rows: rows,
        ..Draft::default()
    };
    if access.eq_ignore_ascii_case("all") {
        let name = table.unwrap_or_else(|| "the table".to_string());
        draft.warnings.push(format!("Reads every row of {name}"));
        draft.recommendations.push(scan_advice(&name));
    }
    draft
}

fn mysql_extra(draft: &mut Draft, extra: &str) {
    let lower = extra.to_lowercase();
    if lower.contains("using filesort") {
        draft
            .warnings
            .push("Sorts rows outside an index (filesort)".to_string());
        draft.recommendations.push(FILESORT_ADVICE.to_string());
    }
    if lower.contains("using temporary") {
        draft.warnings.push("Uses a temporary table".to_string());
    }
    if lower.contains("using join buffer") {
        draft
            .warnings
            .push("Joins without a usable index (join buffer)".to_string());
        draft.recommendations.push(
            "Indexing the join column of the joined table may avoid the join buffer".to_string(),
        );
    }
    if draft.detail.is_none() {
        draft.detail = Some(extra.to_string());
    } else if let Some(detail) = &mut draft.detail {
        detail.push_str(&format!(" · {extra}"));
    }
}

/// Convert one MySQL `query_block` (or any operation object) into drafts.
fn mysql_block(block: &Value) -> Vec<Draft> {
    let Some(object) = block.as_object() else {
        return Vec::new();
    };
    let mut out: Vec<Draft> = Vec::new();

    // operations that wrap the rest of the block
    for (key, title, kind) in [
        ("ordering_operation", "Sort", NodeKind::Sort),
        ("grouping_operation", "Group", NodeKind::Aggregate),
        ("duplicates_removal", "Distinct", NodeKind::Aggregate),
        ("buffer_result", "Buffer result", NodeKind::Other),
        ("windowing", "Window", NodeKind::Aggregate),
    ] {
        if let Some(inner) = object.get(key) {
            let mut draft = Draft {
                title: title.to_string(),
                kind: Some(kind),
                severity: Some(Severity::Neutral),
                children: mysql_block(inner),
                ..Draft::default()
            };
            if inner.get("using_filesort").and_then(Value::as_bool) == Some(true) {
                draft
                    .warnings
                    .push("Sorts rows outside an index (filesort)".to_string());
                draft.recommendations.push(FILESORT_ADVICE.to_string());
            }
            if inner.get("using_temporary_table").and_then(Value::as_bool) == Some(true) {
                draft.warnings.push("Uses a temporary table".to_string());
            }
            out.push(draft);
        }
    }

    if let Some(Value::Array(items)) = object.get("nested_loop") {
        let children: Vec<Draft> = items.iter().flat_map(mysql_block).collect();
        if children.len() > 1 {
            out.push(Draft {
                title: "Nested loop join".to_string(),
                kind: Some(NodeKind::Join),
                severity: Some(Severity::Neutral),
                children,
                ..Draft::default()
            });
        } else {
            out.extend(children);
        }
    }

    if let Some(table) = object.get("table") {
        out.push(mysql_table_draft(table));
    }

    if let Some(union) = object.get("union_result") {
        let children: Vec<Draft> = union
            .get("query_specifications")
            .and_then(Value::as_array)
            .map(|specs| {
                specs
                    .iter()
                    .filter_map(|spec| spec.get("query_block"))
                    .flat_map(mysql_block)
                    .collect()
            })
            .unwrap_or_default();
        out.push(Draft {
            title: "Union".to_string(),
            kind: Some(NodeKind::Other),
            severity: Some(Severity::Neutral),
            children,
            ..Draft::default()
        });
    }

    if let Some(Value::Array(subqueries)) = object
        .get("optimized_away_subqueries")
        .or_else(|| object.get("attached_subqueries"))
    {
        for subquery in subqueries {
            let dependent = subquery.get("dependent").and_then(Value::as_bool) == Some(true);
            let mut draft = Draft {
                title: if dependent {
                    "Dependent subquery"
                } else {
                    "Subquery"
                }
                .to_string(),
                kind: Some(NodeKind::Subquery),
                severity: Some(if dependent {
                    Severity::Warning
                } else {
                    Severity::Neutral
                }),
                children: subquery
                    .get("query_block")
                    .map(mysql_block)
                    .unwrap_or_default(),
                ..Draft::default()
            };
            if dependent {
                draft
                    .warnings
                    .push("Runs once for every row of the outer query".to_string());
            }
            out.push(draft);
        }
    }

    if out.is_empty() {
        if let Some(message) = object.get("message").and_then(Value::as_str) {
            out.push(Draft {
                title: message.to_string(),
                kind: Some(NodeKind::Other),
                severity: Some(Severity::Neutral),
                ..Draft::default()
            });
        } else if object.contains_key("select_id") {
            out.push(Draft {
                title: "Query block".to_string(),
                kind: Some(NodeKind::Other),
                severity: Some(Severity::Neutral),
                ..Draft::default()
            });
        }
    }
    out
}

fn mysql_table_draft(table: &Value) -> Draft {
    let text = |key: &str| table.get(key).and_then(Value::as_str).map(str::to_string);
    let access = text("access_type").unwrap_or_default();
    let mut draft = mysql_access_draft(
        &access,
        text("table_name"),
        text("key"),
        table.get("rows_examined_per_scan").and_then(Value::as_u64),
    );
    draft.total_cost = table
        .pointer("/cost_info/prefix_cost")
        .and_then(mysql_number);
    draft.own_cost = {
        let read = table.pointer("/cost_info/read_cost").and_then(mysql_number);
        let eval = table.pointer("/cost_info/eval_cost").and_then(mysql_number);
        match (read, eval) {
            (None, None) => None,
            (r, e) => Some(r.unwrap_or(0.0) + e.unwrap_or(0.0)),
        }
    };
    draft.actual_rows = None;
    if let Some(condition) = text("attached_condition").or_else(|| text("index_condition")) {
        draft.detail = Some(condition);
    }
    if let Some(filtered) = table.get("filtered").and_then(mysql_number)
        && filtered < 10.0
        && access.eq_ignore_ascii_case("all")
    {
        draft.warnings.push(format!(
            "Only {filtered:.0}% of the scanned rows survive the filter"
        ));
    }
    if let Some(object) = table.as_object() {
        let items: Vec<Property> = object
            .iter()
            .filter(|(key, _)| !matches!(key.as_str(), "cost_info" | "materialized_from_subquery"))
            .map(|(key, value)| {
                let value = match value {
                    Value::String(text) => text.clone(),
                    Value::Array(items) if items.iter().all(Value::is_string) => items
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", "),
                    other => other.to_string(),
                };
                property(
                    key,
                    value,
                    matches!(
                        key.as_str(),
                        "attached_condition" | "index_condition" | "used_columns" | "possible_keys"
                    ),
                )
            })
            .collect();
        let mut groups = vec![PropertyGroup {
            title: "Access",
            items,
        }];
        if let Some(Value::Object(costs)) = table.get("cost_info") {
            groups.push(PropertyGroup {
                title: "Cost",
                items: costs
                    .iter()
                    .map(|(key, value)| {
                        property(
                            key,
                            value
                                .as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| value.to_string()),
                            false,
                        )
                    })
                    .collect(),
            });
        }
        draft.properties = groups;
    }
    if let Some(sub) = table.pointer("/materialized_from_subquery/query_block") {
        draft.children = mysql_block(sub);
    }
    draft
}

// ---------------------------------------------------------------------------
// Fallback bodies
// ---------------------------------------------------------------------------

/// `(plan_type, plan)` tables (DataFusion and friends): one block per row.
fn sections_model(raw: &ExplainRaw) -> Option<PlanModel> {
    let header = raw.rows.first()?;
    let data = raw.rows.get(1..)?;
    let datafusion = matches!(
        raw.db_type,
        DatabaseType::Parquet | DatabaseType::CSV | DatabaseType::JSON
    );
    // DataFusion answers `(plan_type, plan)`, DuckDB `(explain_key, explain_value)`
    let titled = header.first().is_some_and(|name| {
        let name = name.trim();
        name.eq_ignore_ascii_case("plan_type") || name.eq_ignore_ascii_case("explain_key")
    });
    if header.len() != 2 || data.is_empty() || !(datafusion || titled) {
        return None;
    }
    let sections: Vec<PlanSection> = data
        .iter()
        .map(|row| PlanSection {
            title: row.first().cloned().unwrap_or_default(),
            text: row.get(1).cloned().unwrap_or_default(),
        })
        .collect();
    let raw_text = sections
        .iter()
        .map(|section| format!("{}\n{}", section.title, section.text))
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(PlanModel {
        body: PlanBody::Sections(sections),
        summary: PlanSummary::default(),
        raw: raw_text,
        raw_is_json: false,
        note: None,
    })
}

/// A single cell holding JSON (Elasticsearch DSL, unknown JSON plans).
fn json_model(raw: &ExplainRaw) -> Option<PlanModel> {
    let data = raw.rows.get(1..)?;
    if data.len() != 1 || data[0].len() != 1 {
        return None;
    }
    let value: Value = serde_json::from_str(data[0][0].trim()).ok()?;
    if !(value.is_object() || value.is_array()) {
        return None;
    }
    let pretty = serde_json::to_string_pretty(&value).ok()?;
    Some(PlanModel {
        body: PlanBody::Json(pretty.clone()),
        summary: PlanSummary::default(),
        raw: pretty,
        raw_is_json: true,
        note: None,
    })
}

fn text_model(raw: &ExplainRaw) -> PlanModel {
    let header = raw.rows.first().cloned().unwrap_or_default();
    let data: Vec<Vec<String>> = raw.rows.iter().skip(1).cloned().collect();
    let single_column = header.len() <= 1 && data.iter().all(|row| row.len() <= 1);
    let (body, text) = if single_column {
        let text = data
            .iter()
            .map(|row| row.first().cloned().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n");
        (PlanBody::Text(text.clone()), text)
    } else {
        let text = raw
            .rows
            .iter()
            .map(|row| row.join("\t"))
            .collect::<Vec<_>>()
            .join("\n");
        (PlanBody::Table { header, rows: data }, text)
    };
    PlanModel {
        body,
        summary: PlanSummary::default(),
        raw: text,
        raw_is_json: false,
        note: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::time::Duration;

    fn raw(db_type: DatabaseType, rows: &[&[&str]], analyzed: bool) -> ExplainRaw {
        ExplainRaw {
            db_type,
            sql: "select 1".into(),
            rows: rows
                .iter()
                .map(|row| row.iter().map(|cell| cell.to_string()).collect())
                .collect(),
            analyzed,
            elapsed: Duration::from_millis(3),
        }
    }

    const PG_ANALYZED: &str = r#"[{
      "Plan": {
        "Node Type": "Hash Join", "Join Type": "Inner", "Parallel Aware": false,
        "Startup Cost": 100.0, "Total Cost": 1000.0, "Plan Rows": 100, "Plan Width": 40,
        "Actual Startup Time": 1.0, "Actual Total Time": 50.0, "Actual Rows": 5000, "Actual Loops": 1,
        "Hash Cond": "(o.customer_id = c.id)",
        "Shared Hit Blocks": 12, "Shared Read Blocks": 3,
        "Plans": [
          {"Node Type": "Seq Scan", "Parent Relationship": "Outer", "Relation Name": "orders", "Schema": "public", "Alias": "o",
           "Startup Cost": 0.0, "Total Cost": 700.0, "Plan Rows": 20000, "Plan Width": 20,
           "Actual Startup Time": 0.1, "Actual Total Time": 30.0, "Actual Rows": 20000, "Actual Loops": 1,
           "Filter": "(status = 'paid'::text)", "Rows Removed by Filter": 500000},
          {"Node Type": "Hash", "Parent Relationship": "Inner",
           "Startup Cost": 80.0, "Total Cost": 80.0, "Plan Rows": 1000, "Plan Width": 20,
           "Actual Startup Time": 5.0, "Actual Total Time": 5.0, "Actual Rows": 1000, "Actual Loops": 1,
           "Plans": [
             {"Node Type": "Index Scan", "Parent Relationship": "Outer", "Relation Name": "customers", "Schema": "analytics", "Index Name": "customers_pkey",
              "Startup Cost": 0.1, "Total Cost": 80.0, "Plan Rows": 1000, "Plan Width": 20,
              "Actual Startup Time": 0.1, "Actual Total Time": 4.0, "Actual Rows": 1000, "Actual Loops": 1,
              "Index Cond": "(id > 0)"}
           ]}
        ]
      },
      "Planning Time": 0.4, "Execution Time": 51.2
    }]"#;

    fn pg_model() -> PlanModel {
        PlanModel::from_raw(&raw(
            DatabaseType::PostgreSQL,
            &[&["QUERY PLAN"], &[PG_ANALYZED]],
            true,
        ))
    }

    #[test]
    fn postgres_plan_becomes_a_tree_with_summary() {
        let model = pg_model();
        let tree = model.tree().expect("tree");
        assert_eq!(tree.nodes.len(), 4);
        assert!(tree.has_costs && tree.has_actuals);
        let titles: Vec<_> = tree
            .nodes
            .iter()
            .map(|n| (n.depth, n.title.as_str()))
            .collect();
        assert_eq!(
            titles,
            vec![
                (0, "Hash Join"),
                (1, "Seq Scan"),
                (1, "Hash"),
                (2, "Index Scan")
            ]
        );
        assert_eq!(tree.nodes[0].children, vec![1, 2]);
        assert_eq!(tree.nodes[1].relation.as_deref(), Some("orders"));
        assert_eq!(
            tree.nodes[3].relation.as_deref(),
            Some("analytics.customers")
        );
        assert_eq!(tree.nodes[3].index_name.as_deref(), Some("customers_pkey"));
        assert_eq!(
            tree.nodes[0].detail.as_deref(),
            Some("inner join · (o.customer_id = c.id)")
        );

        let summary = &model.summary;
        assert_eq!(summary.total_cost, Some(1000.0));
        assert_eq!(summary.plan_rows, Some(100));
        assert_eq!(summary.actual_rows, Some(5000));
        assert_eq!(summary.planning_ms, Some(0.4));
        assert_eq!(summary.execution_ms, Some(51.2));
        assert_eq!(summary.node_count, 4);
        assert!(summary.grade.is_some());
        assert_eq!(summary.tables, vec!["orders", "analytics.customers"]);
        assert!(model.raw_is_json && model.raw.contains("\"Planning Time\""));
    }

    #[test]
    fn exclusive_shares_use_time_when_analyzed_and_sum_to_one() {
        let tree = pg_model().tree().cloned().expect("tree");
        // join: 50 - (30 + 5) = 15ms; seq scan 30; hash 5 - 4 = 1; index scan 4
        let exclusive: Vec<_> = tree
            .nodes
            .iter()
            .map(|n| n.exclusive_time_ms.unwrap_or(-1.0))
            .collect();
        assert_eq!(exclusive, vec![15.0, 30.0, 1.0, 4.0]);
        let total: f64 = tree.nodes.iter().map(|n| n.share).sum();
        assert!((total - 1.0).abs() < 1e-9, "{total}");
        assert_eq!(tree.hottest(), Some(1));
        assert!((tree.nodes[1].share - 0.6).abs() < 1e-9);
    }

    #[test]
    fn cost_shares_apply_without_analyze_and_clamp_at_zero() {
        let plan = r#"[{"Plan": {"Node Type": "Limit", "Startup Cost": 0.0, "Total Cost": 10.0, "Plan Rows": 5, "Plan Width": 4,
            "Plans": [{"Node Type": "Seq Scan", "Relation Name": "t", "Startup Cost": 0.0, "Total Cost": 500.0, "Plan Rows": 5, "Plan Width": 4}]}}]"#;
        let model = PlanModel::from_raw(&raw(
            DatabaseType::PostgreSQL,
            &[&["QUERY PLAN"], &[plan]],
            false,
        ));
        let tree = model.tree().expect("tree");
        assert!(!tree.has_actuals);
        // the limit costs less than its child: exclusive cost clamps to zero
        assert_eq!(tree.nodes[0].exclusive_cost, Some(0.0));
        assert_eq!(tree.nodes[1].exclusive_cost, Some(500.0));
        assert!((tree.nodes[1].share - 1.0).abs() < 1e-9);
        assert_eq!(model.summary.execution_ms, None);
    }

    #[test]
    fn estimate_mismatch_over_ten_times_is_flagged() {
        let tree = pg_model().tree().cloned().expect("tree");
        // join: estimated 100, actual 5000 -> 50x
        let join = &tree.nodes[0];
        assert_eq!(join.estimate_factor.map(|f| f.round() as u64), Some(50));
        assert!(
            join.warnings
                .iter()
                .any(|w| w.starts_with("Row estimate off by 50x")),
            "{:?}",
            join.warnings
        );
        assert_eq!(join.severity, Severity::Warning);
        // seq scan: estimated 20000, actual 20000 -> fine, but its filter discards 500000 rows
        assert_eq!(tree.nodes[1].estimate_factor, None);
        assert!(
            tree.nodes[1]
                .warnings
                .iter()
                .any(|w| w.contains("Filter discards 500,000 rows")),
            "{:?}",
            tree.nodes[1].warnings
        );
        // only one row-estimate warning per node (DBCrust's duplicate is dropped)
        assert_eq!(
            join.warnings
                .iter()
                .filter(|w| w.starts_with("Row estimate"))
                .count(),
            1
        );
        assert_eq!(
            pg_model().summary.warning_count,
            tree.nodes.iter().map(|n| n.warnings.len()).sum::<usize>()
        );
    }

    #[test]
    fn guides_and_last_flags_draw_connectors() {
        let tree = pg_model().tree().cloned().expect("tree");
        // depth 1 nodes carry no guides; the index scan (depth 2) sits under
        // the last child (Hash), so no vertical line continues for column 0
        assert!(tree.nodes[1].guides.is_empty() && !tree.nodes[1].is_last);
        assert!(tree.nodes[2].is_last);
        assert_eq!(tree.nodes[3].guides, vec![false]);
        assert!(tree.nodes[3].is_last);

        // a middle child's subtree keeps its vertical guide
        let plan = r#"[{"Plan": {"Node Type": "Append", "Startup Cost": 0.0, "Total Cost": 3.0, "Plan Rows": 1, "Plan Width": 4,
            "Plans": [
              {"Node Type": "Sort", "Startup Cost": 0.0, "Total Cost": 1.0, "Plan Rows": 1, "Plan Width": 4,
               "Plans": [{"Node Type": "Seq Scan", "Relation Name": "a", "Startup Cost": 0.0, "Total Cost": 1.0, "Plan Rows": 1, "Plan Width": 4}]},
              {"Node Type": "Seq Scan", "Relation Name": "b", "Startup Cost": 0.0, "Total Cost": 1.0, "Plan Rows": 1, "Plan Width": 4}
            ]}}]"#;
        let model = PlanModel::from_raw(&raw(
            DatabaseType::PostgreSQL,
            &[&["QUERY PLAN"], &[plan]],
            false,
        ));
        let tree = model.tree().expect("tree");
        assert_eq!(tree.nodes[2].depth, 2);
        assert_eq!(tree.nodes[2].guides, vec![true]);
    }

    #[test]
    fn postgres_properties_are_grouped() {
        let tree = pg_model().tree().cloned().expect("tree");
        let scan = &tree.nodes[1];
        let titles: Vec<_> = scan.properties.iter().map(|g| g.title).collect();
        assert_eq!(titles, vec!["Operation", "Access", "Estimates", "Actual"]);
        let access = &scan.properties[1];
        assert!(access.items.iter().any(|p| p.key == "Filter" && p.code));
        let actual = &scan.properties[3];
        assert!(
            actual
                .items
                .iter()
                .any(|p| p.key == "Actual Total Time" && p.value == "30.0 ms")
        );
        // buffers appear for the join
        assert!(
            tree.nodes[0]
                .properties
                .iter()
                .any(|g| g.title == "Buffers")
        );
    }

    #[test]
    fn invalid_postgres_json_falls_back_to_text() {
        let model = PlanModel::from_raw(&raw(
            DatabaseType::PostgreSQL,
            &[
                &["QUERY PLAN"],
                &["Seq Scan on t  (cost=0.00..1.00 rows=1 width=4)"],
            ],
            false,
        ));
        assert!(matches!(model.body, PlanBody::Text(ref text) if text.contains("Seq Scan")));
        assert!(model.tree().is_none());
    }

    fn sqlite_plan(details: &[(&str, &str, &str)]) -> PlanModel {
        let mut rows: Vec<Vec<String>> = vec![vec![
            "id".into(),
            "parent".into(),
            "notused".into(),
            "detail".into(),
        ]];
        rows.extend(details.iter().map(|(id, parent, detail)| {
            vec![
                id.to_string(),
                parent.to_string(),
                "0".to_string(),
                detail.to_string(),
            ]
        }));
        PlanModel::from_raw(&ExplainRaw {
            db_type: DatabaseType::SQLite,
            sql: "select".into(),
            rows,
            analyzed: false,
            elapsed: Duration::ZERO,
        })
    }

    #[test]
    fn sqlite_plan_is_classified() {
        let model = sqlite_plan(&[
            ("2", "0", "SCAN customers"),
            (
                "5",
                "0",
                "SEARCH orders USING INDEX orders_customer_idx (customer_id=?)",
            ),
            ("9", "0", "USE TEMP B-TREE FOR ORDER BY"),
        ]);
        let tree = model.tree().expect("tree");
        assert_eq!(tree.nodes.len(), 3);
        assert!(!tree.has_costs && !tree.has_actuals);

        let scan = &tree.nodes[0];
        assert_eq!(
            (scan.title.as_str(), scan.kind, scan.severity),
            ("Full table scan", NodeKind::Scan, Severity::Warning)
        );
        assert_eq!(scan.relation.as_deref(), Some("customers"));
        assert_eq!(scan.warnings, vec!["Reads every row of customers"]);

        let search = &tree.nodes[1];
        assert_eq!(
            (search.title.as_str(), search.kind, search.severity),
            ("Index search", NodeKind::IndexScan, Severity::Good)
        );
        assert_eq!(search.relation.as_deref(), Some("orders"));
        assert_eq!(search.index_name.as_deref(), Some("orders_customer_idx"));
        assert_eq!(search.detail.as_deref(), Some("(customer_id=?)"));
        assert!(search.warnings.is_empty());

        let sort = &tree.nodes[2];
        assert_eq!(
            (sort.title.as_str(), sort.kind, sort.severity),
            ("Temporary B-tree", NodeKind::Sort, Severity::Warning)
        );
        assert_eq!(sort.detail.as_deref(), Some("order by"));

        assert_eq!(model.summary.full_scans, 1);
        assert_eq!(model.summary.warning_count, 2);
        let grade = model.summary.grade.expect("grade");
        assert_eq!(grade.score, 75);
        assert_eq!(model.raw.lines().count(), 3);
        assert!(model.note.is_some());
    }

    #[rstest]
    #[case(
        "SCAN orders USING INDEX orders_status_idx",
        "Index scan",
        Severity::Neutral
    )]
    #[case(
        "SCAN orders USING COVERING INDEX orders_status_idx",
        "Covering index scan",
        Severity::Neutral
    )]
    #[case(
        "SEARCH c USING INTEGER PRIMARY KEY (rowid=?)",
        "Primary key lookup",
        Severity::Good
    )]
    #[case(
        "SEARCH t USING COVERING INDEX t_a (a>?)",
        "Covering index search",
        Severity::Good
    )]
    #[case(
        "SEARCH t USING AUTOMATIC COVERING INDEX (a=?)",
        "Automatic index",
        Severity::Warning
    )]
    #[case("SCAN TABLE orders", "Full table scan", Severity::Warning)]
    #[case("SCAN SUBQUERY 2", "Scan", Severity::Neutral)]
    #[case(
        "CORRELATED SCALAR SUBQUERY 1",
        "Correlated subquery",
        Severity::Warning
    )]
    #[case("SCALAR SUBQUERY 3", "Subquery", Severity::Neutral)]
    #[case("USE TEMP B-TREE FOR GROUP BY", "Temporary B-tree", Severity::Warning)]
    #[case("MULTI-INDEX OR", "Multi-index OR", Severity::Good)]
    fn sqlite_details(#[case] detail: &str, #[case] title: &str, #[case] severity: Severity) {
        let model = sqlite_plan(&[("1", "0", detail)]);
        let node = &model.tree().expect("tree").nodes[0];
        assert_eq!(
            (node.title.as_str(), node.severity),
            (title, severity),
            "{detail}"
        );
    }

    #[test]
    fn sqlite_parent_ids_build_depth() {
        let model = sqlite_plan(&[
            ("2", "0", "SCAN customers"),
            ("5", "2", "CORRELATED SCALAR SUBQUERY 1"),
            (
                "6",
                "5",
                "SEARCH orders USING INDEX orders_customer_idx (customer_id=?)",
            ),
            ("9", "0", "SCAN products"),
        ]);
        let tree = model.tree().expect("tree");
        let shape: Vec<_> = tree.nodes.iter().map(|n| (n.depth, n.parent)).collect();
        assert_eq!(
            shape,
            vec![(0, None), (1, Some(0)), (2, Some(1)), (0, None)]
        );
        assert_eq!(
            tree.nodes[2].guides,
            Vec::<bool>::new()
                .into_iter()
                .chain([false])
                .collect::<Vec<_>>()
        );
        assert!(!tree.nodes[0].is_last && tree.nodes[3].is_last);
    }

    #[test]
    fn sqlite_without_rows_or_ids_falls_back() {
        let empty = PlanModel::from_raw(&raw(
            DatabaseType::SQLite,
            &[&["id", "parent", "notused", "detail"]],
            false,
        ));
        assert!(empty.tree().is_none());
        let odd = PlanModel::from_raw(&raw(
            DatabaseType::SQLite,
            &[&["QUERY PLAN"], &["whatever"]],
            false,
        ));
        assert!(matches!(odd.body, PlanBody::Text(_)));
    }

    #[test]
    fn mysql_json_plan_is_a_tree() {
        let plan = r#"{"query_block": {"select_id": 1, "cost_info": {"query_cost": "12.50"},
          "ordering_operation": {"using_filesort": true,
            "nested_loop": [
              {"table": {"table_name": "customers", "access_type": "ALL", "rows_examined_per_scan": 1000, "filtered": "5.00",
                         "cost_info": {"read_cost": "10.00", "eval_cost": "100.00", "prefix_cost": "110.00", "data_read_per_join": "1M"},
                         "used_columns": ["id", "name"]}},
              {"table": {"table_name": "orders", "access_type": "ref", "key": "idx_customer", "rows_examined_per_scan": 3,
                         "cost_info": {"read_cost": "2.00", "eval_cost": "1.00", "prefix_cost": "400.00"}}}
            ]}}}"#;
        let model = PlanModel::from_raw(&raw(DatabaseType::MySQL, &[&["EXPLAIN"], &[plan]], false));
        let tree = model.tree().expect("tree");
        let shape: Vec<_> = tree
            .nodes
            .iter()
            .map(|n| (n.depth, n.title.as_str()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (0, "Sort"),
                (1, "Nested loop join"),
                (2, "Full table scan"),
                (2, "Index lookup")
            ]
        );
        assert!(
            tree.nodes[0]
                .warnings
                .iter()
                .any(|w| w.contains("filesort"))
        );
        assert_eq!(tree.nodes[2].relation.as_deref(), Some("customers"));
        assert_eq!(tree.nodes[3].index_name.as_deref(), Some("idx_customer"));
        assert_eq!(model.summary.total_cost, Some(12.5));
        assert!(model.summary.full_scans >= 1);
        assert!(model.raw_is_json);
        // explicit exclusive cost (read + eval) drives the share
        assert_eq!(tree.nodes[2].exclusive_cost, Some(110.0));
    }

    #[test]
    fn mysql_classic_table_becomes_flat_nodes() {
        let model = PlanModel::from_raw(&raw(
            DatabaseType::MySQL,
            &[
                &[
                    "id",
                    "select_type",
                    "table",
                    "partitions",
                    "type",
                    "possible_keys",
                    "key",
                    "key_len",
                    "ref",
                    "rows",
                    "filtered",
                    "Extra",
                ],
                &[
                    "1",
                    "SIMPLE",
                    "orders",
                    "NULL",
                    "ALL",
                    "NULL",
                    "NULL",
                    "NULL",
                    "NULL",
                    "5000",
                    "10.00",
                    "Using where; Using filesort",
                ],
                &[
                    "1",
                    "SIMPLE",
                    "customers",
                    "NULL",
                    "eq_ref",
                    "PRIMARY",
                    "PRIMARY",
                    "4",
                    "shop.orders.customer_id",
                    "1",
                    "100.00",
                    "NULL",
                ],
            ],
            false,
        ));
        let tree = model.tree().expect("tree");
        assert_eq!(tree.nodes.len(), 2);
        assert_eq!(tree.nodes[0].title, "Full table scan");
        assert_eq!(tree.nodes[0].plan_rows, Some(5000));
        assert!(
            tree.nodes[0]
                .warnings
                .iter()
                .any(|w| w.contains("filesort"))
        );
        assert_eq!(tree.nodes[1].title, "Unique key lookup");
        assert_eq!(tree.nodes[1].index_name.as_deref(), Some("PRIMARY"));
        assert!(model.note.is_some());
        assert!(model.raw.starts_with("id\tselect_type"));
    }

    #[test]
    fn datafusion_plans_are_sections() {
        let model = PlanModel::from_raw(&raw(
            DatabaseType::Parquet,
            &[
                &["plan_type", "plan"],
                &["logical_plan", "Projection: a\n  TableScan: t"],
                &["physical_plan", "ProjectionExec: expr=[a]\n  ParquetExec"],
            ],
            false,
        ));
        let PlanBody::Sections(sections) = &model.body else {
            panic!("expected sections, got {:?}", model.body);
        };
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title, "logical_plan");
        assert!(sections[1].text.contains("ParquetExec"));
        assert!(model.raw.contains("physical_plan\nProjectionExec"));
    }

    #[test]
    fn duckdb_plans_are_sections() {
        let model = PlanModel::from_raw(&raw(
            DatabaseType::DuckDB,
            &[
                &["explain_key", "explain_value"],
                &["physical_plan", "┌──────┐\n│ SEQ_SCAN │\n└──────┘\n"],
            ],
            false,
        ));
        let PlanBody::Sections(sections) = &model.body else {
            panic!("expected sections, got {:?}", model.body);
        };
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "physical_plan");
        assert!(sections[0].text.contains("SEQ_SCAN"));
    }

    #[test]
    fn json_and_text_fallbacks() {
        let json = PlanModel::from_raw(&raw(
            DatabaseType::Elasticsearch,
            &[
                &["Translated Query"],
                &[r#"{"query":{"match_all":{}},"size":10}"#],
            ],
            false,
        ));
        assert!(matches!(&json.body, PlanBody::Json(pretty) if pretty.contains("\n  \"query\"")));
        assert!(json.raw_is_json);

        let text = PlanModel::from_raw(&raw(
            DatabaseType::ClickHouse,
            &[&["explain"], &["SELECT 1"], &["FROM t"]],
            false,
        ));
        assert_eq!(text.body, PlanBody::Text("SELECT 1\nFROM t".into()));

        let grid = PlanModel::from_raw(&raw(
            DatabaseType::MongoDB,
            &[&["Stage", "Detail"], &["COLLSCAN", "users"]],
            false,
        ));
        assert!(matches!(grid.body, PlanBody::Table { ref rows, .. } if rows.len() == 1));
        assert_eq!(grid.raw, "Stage\tDetail\nCOLLSCAN\tusers");

        let empty = PlanModel::from_raw(&raw(DatabaseType::ClickHouse, &[], false));
        assert_eq!(empty.body, PlanBody::Text(String::new()));
    }

    #[rstest]
    #[case(0, "0")]
    #[case(999, "999")]
    #[case(1_000, "1,000")]
    #[case(1_234_567, "1,234,567")]
    #[case(12_345_678, "12.3M")]
    fn counts(#[case] value: u64, #[case] expected: &str) {
        assert_eq!(format_count(value), expected);
    }

    #[rstest]
    #[case(0.43, "0.43")]
    #[case(128.5, "128")]
    #[case(12_345.0, "12.3k")]
    #[case(2_500_000.0, "2.5M")]
    fn costs(#[case] value: f64, #[case] expected: &str) {
        assert_eq!(format_cost(value), expected);
    }

    #[rstest]
    #[case(0.123, "0.12 ms")]
    #[case(12.34, "12.3 ms")]
    #[case(456.7, "457 ms")]
    #[case(1234.0, "1.23 s")]
    #[case(125_000.0, "2m 5s")]
    fn durations(#[case] value: f64, #[case] expected: &str) {
        assert_eq!(format_ms(value), expected);
    }

    #[test]
    fn observations_list_nodes_with_findings() {
        let tree = pg_model().tree().cloned().expect("tree");
        let observations = tree.observations();
        let mut nodes: Vec<usize> = observations.iter().map(|o| o.node).collect();
        nodes.dedup();
        assert_eq!(nodes, vec![0, 1]);
        // one entry per distinct issue
        let issues: std::collections::HashSet<_> =
            observations.iter().map(|o| (o.node, &o.issue)).collect();
        assert_eq!(issues.len(), observations.len());
        assert!(observations[0].label.starts_with("Hash Join"));
        assert!(!observations[0].advice.is_empty());
        assert_eq!(pg_model().summary.observation_count, observations.len());
    }

    fn node_with(warnings: &[&str], recommendations: &[&str]) -> PlanTree {
        let draft = Draft {
            title: "Seq Scan".into(),
            relation: Some("t".into()),
            warnings: warnings.iter().map(|w| w.to_string()).collect(),
            recommendations: recommendations.iter().map(|r| r.to_string()).collect(),
            ..Draft::default()
        };
        assemble(vec![draft])
    }

    #[test]
    fn repeated_findings_on_a_node_collapse_into_one_observation() {
        let tree = node_with(
            &[
                "Reads every row of t",
                " reads every row of T ",
                "Reads every row of t",
            ],
            &["Add an index", "add an index"],
        );
        let observations = tree.observations();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].issue, "Reads every row of t");
        assert_eq!(observations[0].advice, vec!["Add an index"]);
        assert_eq!(observations[0].severity, Severity::Warning);
    }

    #[test]
    fn warnings_and_advice_pair_up_only_when_counts_match() {
        let paired = node_with(&["a", "b"], &["fix a", "fix b"]).observations();
        assert_eq!(paired[0].advice, vec!["fix a"]);
        assert_eq!(paired[1].advice, vec!["fix b"]);

        let unpaired = node_with(&["a", "b"], &["only one"]).observations();
        assert_eq!(unpaired[0].advice, vec!["only one"]);
        assert!(unpaired[1].advice.is_empty());
    }

    #[test]
    fn advice_without_a_warning_is_a_neutral_suggestion() {
        let observations =
            node_with(&[], &["Consider an index", "Consider an index"]).observations();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].severity, Severity::Neutral);
        assert_eq!(observations[0].issue, "Consider an index");
        assert!(observations[0].advice.is_empty());
    }

    #[test]
    fn the_same_finding_on_different_nodes_is_kept_per_node() {
        let tree = assemble(vec![
            Draft {
                title: "Seq Scan".into(),
                warnings: vec!["Reads every row of a".into()],
                ..Draft::default()
            },
            Draft {
                title: "Seq Scan".into(),
                warnings: vec!["Reads every row of a".into()],
                ..Draft::default()
            },
        ]);
        let nodes: Vec<usize> = tree.observations().iter().map(|o| o.node).collect();
        assert_eq!(nodes, vec![0, 1]);
    }

    #[test]
    fn full_scan_advice_is_conditional_not_prescriptive() {
        let model = sqlite_plan(&[("2", "0", "SCAN customers")]);
        let tree = model.tree().expect("tree");
        let advice = &tree.observations()[0].advice[0];
        assert!(advice.contains("If it is large"), "{advice}");
        assert!(advice.contains("may help"), "{advice}");
        assert!(!advice.starts_with("Add an index"), "{advice}");
    }

    #[test]
    fn plan_collection_time_is_not_called_execution_time() {
        let summary = PlanSummary::default();
        let stats = timing_stats(false, 0.43, &summary);
        assert_eq!(stats.len(), 1);
        assert_eq!(
            (stats[0].label, stats[0].value.as_str()),
            ("Plan collected in", "0.43 ms")
        );
        assert!(stats[0].tooltip.contains("not executed"));
    }

    #[test]
    fn analyzed_plans_report_the_engine_execution_time() {
        let model = pg_model();
        let stats = timing_stats(true, 60.0, &model.summary);
        let labels: Vec<_> = stats.iter().map(|s| s.label).collect();
        assert_eq!(labels, vec!["Execution", "Planning", "Round trip"]);
        assert_eq!(stats[0].value, "51.2 ms");
        assert!(!labels.contains(&"Plan collected in"));
    }

    #[test]
    fn analyzed_without_engine_timings_names_the_whole_request() {
        let stats = timing_stats(true, 12.0, &PlanSummary::default());
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].label, "Ran and explained in");
    }
}
