"""
Consolidated performance report formatter for Django requests.

Produces a single, clear performance report block that shows developers
everything they need to know at a glance — grades, metrics, critical
issues, slow queries with EXPLAIN insights, and actionable fixes.

Design principles:
  - ONE log call per request (no scattered lines)
  - Visual hierarchy: grade -> metrics -> critical -> slow -> warnings -> hints
  - Most critical items first
  - Actionable fixes, not just descriptions
  - File:line locations for IDE clickability
"""

import logging
from dataclasses import dataclass, field
from typing import List, Dict, Optional, Any, TYPE_CHECKING

if TYPE_CHECKING:
    from .analyzer import AnalysisResult

logger = logging.getLogger("dbcrust.performance")


# ---------------------------------------------------------------------------
# Data classes
# ---------------------------------------------------------------------------

@dataclass
class SlowQueryInfo:
    """Information about a slow query with optional EXPLAIN insights."""
    sql: str
    duration_ms: float
    tables: List[str]
    # EXPLAIN insights (optional -- populated when EXPLAIN is available)
    explain_plan_type: Optional[str] = None    # "Seq Scan on orders", "Index Scan", etc.
    explain_rows_examined: Optional[int] = None
    explain_suggestion: Optional[str] = None   # Raw EXPLAIN-based suggestion
    explain_django_fix: Optional[str] = None   # Django-specific fix


@dataclass
class IssueInfo:
    """
    Bridges a ``DetectedPattern`` into the report.

    Each issue maps to one visual block in the CRITICAL / WARNINGS / HINTS
    sections of the rendered report.
    """
    severity: str               # "critical", "high", "medium", "low"
    label: str                  # Human-readable type, e.g. "N+1 Query"
    description: str
    affected_queries_count: int = 0
    recommendation: Optional[str] = None
    code_suggestion: Optional[str] = None
    code_locations: List[str] = field(default_factory=list)
    sql_example: Optional[str] = None


@dataclass
class RequestPerformanceReport:
    """All data needed to format a performance report for one request."""
    # Request info
    method: str = "?"
    path: str = "?"
    view_name: Optional[str] = None
    status_code: Optional[int] = None

    # Metrics
    total_queries: int = 0
    db_time_ms: float = 0.0
    request_time_ms: float = 0.0
    duplicate_queries: int = 0

    # Queries by type
    selects: int = 0
    inserts: int = 0
    updates: int = 0
    deletes: int = 0

    # Issues by severity bucket (typed IssueInfo objects)
    critical_issues: List[IssueInfo] = field(default_factory=list)
    warnings: List[IssueInfo] = field(default_factory=list)
    hints: List[IssueInfo] = field(default_factory=list)

    # Slow queries (with optional EXPLAIN)
    slow_queries: List[SlowQueryInfo] = field(default_factory=list)

    # Grade
    grade: str = "A"
    grade_emoji: str = "✅"


# ---------------------------------------------------------------------------
# Pattern type -> human-readable label
# ---------------------------------------------------------------------------

_PATTERN_TYPE_LABELS: Dict[str, str] = {
    "n_plus_one": "N+1 Query",
    "missing_select_related": "Missing select_related",
    "missing_prefetch_related": "Missing prefetch_related",
    "inefficient_count": "Inefficient Count",
    "missing_only": "Missing .only()/.defer()",
    "large_result_set": "Large Result Set",
    "unnecessary_ordering": "Unnecessary Ordering",
    "subqueries_in_loops": "Subqueries in Loops",
    "missing_database_indexes": "Missing DB Index",
    "inefficient_aggregation": "Inefficient Aggregation",
    "missing_bulk_operations": "Missing Bulk Operations",
    "inefficient_exists_check": "Inefficient Exists Check",
    "missing_select_for_update": "Missing select_for_update",
    "transaction_issues": "Transaction Issue",
    "connection_pool_exhaustion": "Connection Pool Exhaustion",
    "inefficient_distinct": "Inefficient DISTINCT",
    "missing_values_values_list": "Missing .values()/.values_list()",
    "redundant_queries": "Redundant Queries",
    "missing_query_caching": "Missing Query Caching",
}


def _pattern_type_label(pattern_type: str) -> str:
    """Return a human-readable label for a pattern type string."""
    if pattern_type in _PATTERN_TYPE_LABELS:
        return _PATTERN_TYPE_LABELS[pattern_type]
    # Fallback: title-case with underscores replaced
    return pattern_type.replace("_", " ").title()


# ---------------------------------------------------------------------------
# Builder: AnalysisResult -> RequestPerformanceReport
# ---------------------------------------------------------------------------

def build_report_from_analysis(
    result: "AnalysisResult",
    *,
    method: str = "?",
    path: str = "?",
    view_name: Optional[str] = None,
    status_code: Optional[int] = None,
    request_time_ms: float = 0.0,
    slow_queries: Optional[List[SlowQueryInfo]] = None,
) -> RequestPerformanceReport:
    """
    Build a :class:`RequestPerformanceReport` from an
    :class:`~analyzer.AnalysisResult` produced by the middleware.

    This is the single bridge between the *analyzer world* (``DetectedPattern``,
    ``AnalysisResult``) and the *report world* (``IssueInfo``,
    ``RequestPerformanceReport``).

    Args:
        result: The analysis result from ``DjangoAnalyzer``.
        method: HTTP method (GET, POST, ...).
        path: Request path.
        view_name: Resolved Django view name (optional).
        status_code: HTTP response status code (optional).
        request_time_ms: Wall-clock request time in milliseconds.
        slow_queries: Pre-built slow-query info list from
            :class:`~slow_query_analyzer.SlowQueryAnalyzer` (optional).

    Returns:
        A fully populated ``RequestPerformanceReport`` ready for
        :func:`format_performance_report`.
    """
    db_time_ms = result.total_duration * 1000

    # -- query-type counts --------------------------------------------------
    qbt = result.queries_by_type
    selects = qbt.get("SELECT", 0)
    inserts = qbt.get("INSERT", 0)
    updates = qbt.get("UPDATE", 0)
    deletes = qbt.get("DELETE", 0)

    # -- bucket patterns into severity lists --------------------------------
    critical_issues: List[IssueInfo] = []
    warning_issues: List[IssueInfo] = []
    hint_issues: List[IssueInfo] = []

    for pattern in result.detected_patterns:
        info = IssueInfo(
            severity=pattern.severity,
            label=_pattern_type_label(pattern.pattern_type),
            description=pattern.description,
            affected_queries_count=len(pattern.affected_queries),
            recommendation=pattern.recommendation,
            code_suggestion=pattern.code_suggestion,
            code_locations=list(pattern.code_locations) if pattern.code_locations else [],
            sql_example=(
                pattern.query_examples[0] if pattern.query_examples else None
            ),
        )

        if pattern.severity == "critical":
            critical_issues.append(info)
        elif pattern.severity in ("high", "medium"):
            warning_issues.append(info)
        else:
            hint_issues.append(info)

    # -- grade --------------------------------------------------------------
    grade, grade_emoji = calculate_grade(
        total_queries=result.total_queries,
        db_time_ms=db_time_ms,
        critical_count=len(critical_issues),
        high_count=len([i for i in warning_issues if i.severity == "high"]),
        duplicate_count=result.duplicate_queries,
    )

    return RequestPerformanceReport(
        method=method,
        path=path,
        view_name=view_name,
        status_code=status_code,
        total_queries=result.total_queries,
        db_time_ms=db_time_ms,
        request_time_ms=request_time_ms,
        duplicate_queries=result.duplicate_queries,
        selects=selects,
        inserts=inserts,
        updates=updates,
        deletes=deletes,
        critical_issues=critical_issues,
        warnings=warning_issues,
        hints=hint_issues,
        slow_queries=slow_queries or [],
        grade=grade,
        grade_emoji=grade_emoji,
    )


# ---------------------------------------------------------------------------
# Grading
# ---------------------------------------------------------------------------

def calculate_grade(
    total_queries: int,
    db_time_ms: float,
    critical_count: int,
    high_count: int,
    duplicate_count: int,
) -> tuple:
    """
    Calculate a performance grade (A-F) from request metrics.

    Returns:
        ``(grade_letter, emoji)``
    """
    score = 100

    # --- query count -------------------------------------------------------
    if total_queries > 50:
        score -= 40
    elif total_queries > 30:
        score -= 25
    elif total_queries > 15:
        score -= 15
    elif total_queries > 5:
        score -= 5

    # --- db time -----------------------------------------------------------
    if db_time_ms > 500:
        score -= 35
    elif db_time_ms > 250:
        score -= 20
    elif db_time_ms > 100:
        score -= 10
    elif db_time_ms > 50:
        score -= 5

    # --- issue severity ----------------------------------------------------
    score -= critical_count * 20
    score -= high_count * 8

    # --- duplicates --------------------------------------------------------
    if duplicate_count > 10:
        score -= 10
    elif duplicate_count > 5:
        score -= 5
    elif duplicate_count > 0:
        score -= 2

    # --- map to letter -----------------------------------------------------
    if score >= 90:
        return "A", "✅"
    elif score >= 75:
        return "B", "🟢"
    elif score >= 55:
        return "C", "🟡"
    elif score >= 35:
        return "D", "🟠"
    else:
        return "F", "🔴"


# ---------------------------------------------------------------------------
# Formatter
# ---------------------------------------------------------------------------

def format_performance_report(report: RequestPerformanceReport) -> str:
    """
    Render a consolidated performance report as a **single string**.

    Example output::

        ━━━ DBCrust 🔴 Grade F ━━━ GET /api/orders/ ━━━
            View: orders.views.OrderListView

        📊 Queries: 47 | DB: 342ms | Total: 520ms | Dupes: 12

        🚨 CRITICAL (2)
          ❌ N+1 Query: 23x queries on "order_items" WHERE order_id=%s
             -> .prefetch_related('items')
             📍 orders/views.py:45

        🐌 SLOW QUERIES (1)
          🔴 89ms | SELECT ... FROM orders WHERE status=%s ORDER BY ...
             EXPLAIN: Seq Scan on orders -- 50,000 rows
             -> Meta.indexes = [Index(fields=['status', '-created_at'])]

        ⚠️  WARNINGS (2)
          ⚡ Redundant Queries: same query executed 5x
             -> Cache result or restructure code

        💡 HINTS
          * Use .only('id','name','status') -- fetching 18 unused fields

        ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    """
    lines: list[str] = []
    separator = "━" * 72

    # -- header -------------------------------------------------------------
    header = f"DBCrust {report.grade_emoji} Grade {report.grade}"
    path_info = f"{report.method} {report.path}"
    lines.append("")
    lines.append(f"━━━ {header} ━━━ {path_info} ━━━")
    if report.view_name:
        lines.append(f"    View: {report.view_name}")

    # -- metrics bar --------------------------------------------------------
    parts = [
        f"Queries: {report.total_queries}",
        f"DB: {report.db_time_ms:.0f}ms",
        f"Total: {report.request_time_ms:.0f}ms",
    ]
    if report.duplicate_queries > 0:
        parts.append(f"Dupes: {report.duplicate_queries}")

    # query-type breakdown (compact)
    type_parts = []
    if report.selects:
        type_parts.append(f"{report.selects}R")
    if report.inserts:
        type_parts.append(f"{report.inserts}W")
    if report.updates:
        type_parts.append(f"{report.updates}U")
    if report.deletes:
        type_parts.append(f"{report.deletes}D")
    if type_parts:
        parts.append("/".join(type_parts))

    lines.append("")
    lines.append(f"📊 {' │ '.join(parts)}")

    # -- critical issues ----------------------------------------------------
    if report.critical_issues:
        lines.append("")
        lines.append(f"🚨 CRITICAL ({len(report.critical_issues)})")
        for issue in report.critical_issues:
            _append_issue(lines, issue, icon="❌")

    # -- slow queries -------------------------------------------------------
    if report.slow_queries:
        lines.append("")
        lines.append(f"🐌 SLOW QUERIES ({len(report.slow_queries)})")
        for sq in report.slow_queries:
            _append_slow_query(lines, sq)

    # -- warnings -----------------------------------------------------------
    if report.warnings:
        lines.append("")
        lines.append(f"⚠️  WARNINGS ({len(report.warnings)})")
        for issue in report.warnings:
            _append_issue(lines, issue, icon="⚡")

    # -- hints --------------------------------------------------------------
    if report.hints:
        lines.append("")
        lines.append("💡 HINTS")
        for hint in report.hints:
            _append_hint(lines, hint)

    # -- footer -------------------------------------------------------------
    lines.append("")
    lines.append(separator)

    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------

def _append_issue(lines: list, issue: IssueInfo, icon: str = "⚡"):
    """Render one issue block (2-4 lines)."""
    # main line
    if issue.affected_queries_count > 1:
        lines.append(f"  {icon} {issue.label}: {issue.affected_queries_count}x {issue.description}")
    else:
        lines.append(f"  {icon} {issue.label}: {issue.description}")

    # fix (prefer code_suggestion, fall back to recommendation)
    fix = issue.code_suggestion or issue.recommendation
    if fix:
        lines.append(f"     -> {_oneliner(fix, 66)}")

    # location (clickable in most IDEs)
    if issue.code_locations:
        lines.append(f"     📍 {issue.code_locations[0]}")

    # sql example
    if issue.sql_example:
        lines.append(f"     SQL: {_truncate_sql(issue.sql_example, 62)}")


def _append_hint(lines: list, hint: IssueInfo):
    """Render one hint line."""
    fix = hint.code_suggestion or hint.recommendation
    if fix and fix != hint.description:
        lines.append(f"  * {hint.description}")
        lines.append(f"    -> {_oneliner(fix, 70)}")
    else:
        lines.append(f"  * {hint.description}")


def _append_slow_query(lines: list, sq: SlowQueryInfo):
    """Render one slow-query block with optional EXPLAIN."""
    # severity icon by duration
    if sq.duration_ms >= 200:
        icon = "🔴"
    elif sq.duration_ms >= 100:
        icon = "🟠"
    else:
        icon = "🟡"

    sql_display = _truncate_sql(sq.sql, 55)
    lines.append(f"  {icon} {sq.duration_ms:.0f}ms │ {sql_display}")

    # EXPLAIN insight
    if sq.explain_plan_type:
        explain_parts = [sq.explain_plan_type]
        if sq.explain_rows_examined and sq.explain_rows_examined > 0:
            explain_parts.append(f"{sq.explain_rows_examined:,} rows")
        lines.append(f"     EXPLAIN: {' — '.join(explain_parts)}")

    # suggestion (prefer Django-specific, fall back to generic)
    fix = sq.explain_django_fix or sq.explain_suggestion
    if fix:
        lines.append(f"     -> {_oneliner(fix, 66)}")


def _truncate_sql(sql: str, max_length: int) -> str:
    """Truncate SQL for display, keeping meaningful keywords visible."""
    sql = " ".join(sql.split())  # normalise whitespace
    if len(sql) <= max_length:
        return sql

    # Try to keep up to and including the FROM clause
    upper = sql.upper()
    from_idx = upper.find(" FROM ")
    if 0 < from_idx < max_length - 20:
        # include a bit past FROM
        cut = min(max_length - 3, len(sql))
        return sql[:cut] + "..."

    return sql[: max_length - 1] + "..."


def _oneliner(text: str, max_length: int = 72) -> str:
    """Collapse multi-line text to first meaningful line, truncated."""
    line = text.split("\n")[0].strip()
    if len(line) > max_length:
        return line[: max_length - 3] + "..."
    return line
