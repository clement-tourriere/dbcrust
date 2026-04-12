"""
Slow query analyzer with heuristic and EXPLAIN-based detection.

Identifies slow queries from captured Django ORM queries using multiple
heuristics (absolute time, relative dominance, top-N) and optionally
enriches them with real ``EXPLAIN ANALYZE`` insights via DBCrust.

Two operating modes:
  1. **Heuristic-only** -- works without any DB connection.  Analyses SQL
     text to guess likely performance issues (leading-wildcard LIKE,
     functions on columns, missing WHERE, etc.).
  2. **Heuristic + EXPLAIN** -- auto-detects the Django DB URL, connects
     via DBCrust, and runs ``EXPLAIN (ANALYZE, FORMAT JSON)`` on the
     slowest SELECT queries.

Relationship with ``pattern_detector.py``:
  ``PatternDetector`` focuses on **ORM-level patterns** (N+1, missing
  select_related, bulk operations, etc.) by analysing the *set* of
  captured queries.  ``SlowQueryAnalyzer`` focuses on **per-query SQL
  performance** -- it identifies which individual queries are slow and
  why (missing indexes, full scans, etc.).  The two are complementary
  and their results are merged into a single report by the middleware.
"""

import logging
import re
from dataclasses import dataclass, field
from typing import List, Dict, Optional, Any, Tuple

from .query_collector import CapturedQuery
from .report_formatter import SlowQueryInfo

logger = logging.getLogger("dbcrust.performance")


# ---------------------------------------------------------------------------
# Pre-compiled regexes (issue #17: avoid re-compiling on every call)
# ---------------------------------------------------------------------------

# Matches LIKE with leading wildcard
_RE_LEADING_WILDCARD_LIKE = re.compile(r"LIKE\s+'%", re.IGNORECASE)

# Function calls on columns in WHERE clauses (issue #6: only check after WHERE)
_RE_FUNC_IN_WHERE = re.compile(
    r"""WHERE\s+.*?\b(LOWER|UPPER|TRIM|COALESCE|DATE|CAST)\s*\(""",
    re.IGNORECASE | re.DOTALL,
)

# Django-style table-qualified column references in WHERE clause:
# handles "table"."col" =, "col" =, col =, table.col =
# Issue #5: supports Django's "app_model"."field" pattern
_RE_WHERE_COLUMNS = re.compile(
    r"""WHERE\s+(?:.*?\bAND\s+|.*?\bOR\s+)*"""
    r"""(?:"?\w+"?\.)?"?(\w+)"?\s*[=<>!]""",
    re.IGNORECASE | re.DOTALL,
)

# ORDER BY column extraction -- also handles "table"."col" pattern
_RE_ORDER_BY_COLUMNS = re.compile(
    r"""ORDER\s+BY\s+(?:"?\w+"?\.)?"?(\w+)"?""",
    re.IGNORECASE,
)


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

@dataclass
class SlowQueryThresholds:
    """Thresholds for classifying a query as *slow*."""
    absolute_ms: float = 50.0       # Queries ≥ this are unconditionally slow
    relative_pct: float = 30.0      # Queries consuming ≥ 30% of total DB time
    top_n: int = 3                  # Always surface the N slowest queries …
    min_interesting_ms: float = 10.0  # … but only if they are ≥ this threshold

    def __post_init__(self):
        if self.absolute_ms < 0:
            raise ValueError(f"absolute_ms must be >= 0, got {self.absolute_ms}")
        if not (0 <= self.relative_pct <= 100):
            raise ValueError(f"relative_pct must be 0-100, got {self.relative_pct}")
        if self.top_n < 0:
            raise ValueError(f"top_n must be >= 0, got {self.top_n}")
        if self.min_interesting_ms < 0:
            raise ValueError(f"min_interesting_ms must be >= 0, got {self.min_interesting_ms}")


# ---------------------------------------------------------------------------
# Analyzer
# ---------------------------------------------------------------------------

class SlowQueryAnalyzer:
    """Identifies and analyses slow queries."""

    def __init__(self, thresholds: Optional[SlowQueryThresholds] = None):
        self.thresholds = thresholds or SlowQueryThresholds()

    # ------------------------------------------------------------------
    # public API
    # ------------------------------------------------------------------

    def identify_slow_queries(
        self,
        queries: List[CapturedQuery],
        total_db_time: float = 0.0,
    ) -> List[CapturedQuery]:
        """
        Identify slow queries using multiple heuristics.

        Args:
            queries: All captured queries for the request.
            total_db_time: Sum of all query durations (seconds).

        Returns:
            De-duplicated list of slow queries, **slowest first**.
        """
        if not queries:
            return []

        selects = [q for q in queries if q.query_type == "SELECT"]
        if not selects:
            return []

        slow_ids: set[int] = set()

        # Heuristic 1 — absolute wall-clock threshold
        for q in selects:
            if q.duration * 1000 >= self.thresholds.absolute_ms:
                slow_ids.add(id(q))

        # Heuristic 2 — relative dominance (single query is a large chunk)
        if total_db_time > 0:
            for q in selects:
                pct = (q.duration / total_db_time) * 100
                if pct >= self.thresholds.relative_pct:
                    slow_ids.add(id(q))

        # Heuristic 3 — top-N slowest (if non-trivial)
        by_dur = sorted(selects, key=lambda q: q.duration, reverse=True)
        for q in by_dur[: self.thresholds.top_n]:
            if q.duration * 1000 >= self.thresholds.min_interesting_ms:
                slow_ids.add(id(q))

        result = [q for q in selects if id(q) in slow_ids]
        result.sort(key=lambda q: q.duration, reverse=True)
        return result

    def analyze(
        self,
        slow_queries: List[CapturedQuery],
        db_url: Optional[str] = None,
        max_explain: int = 3,
    ) -> List[SlowQueryInfo]:
        """
        Produce ``SlowQueryInfo`` objects for each slow query.

        If *db_url* is provided, the first *max_explain* queries are
        enriched with real ``EXPLAIN ANALYZE`` data.  Otherwise only
        heuristic analysis is applied.

        Args:
            slow_queries: Queries identified by :meth:`identify_slow_queries`.
            db_url: DBCrust-compatible connection URL (auto-detected when
                    used from the middleware).
            max_explain: Cap on the number of EXPLAIN calls per request.

        Returns:
            List of :class:`SlowQueryInfo` (same order as input).
        """
        explain_map: Dict[int, Dict[str, Any]] = {}
        if db_url and slow_queries:
            explain_map = self._run_explain_batch(
                slow_queries[:max_explain], db_url
            )

        results: list[SlowQueryInfo] = []
        for q in slow_queries:
            info = SlowQueryInfo(
                sql=q.sql,
                duration_ms=q.duration * 1000,
                tables=list(q.table_names),
            )

            explain = explain_map.get(id(q))
            if explain:
                info.explain_plan_type = explain.get("plan_type")
                info.explain_rows_examined = explain.get("rows_examined")
                info.explain_suggestion = explain.get("suggestion")
                info.explain_django_fix = explain.get("django_fix")
            else:
                heuristic = self._heuristic_analysis(q)
                if heuristic:
                    info.explain_plan_type = heuristic.get("plan_type")
                    info.explain_suggestion = heuristic.get("suggestion")
                    info.explain_django_fix = heuristic.get("django_fix")

            results.append(info)

        return results

    # ------------------------------------------------------------------
    # EXPLAIN
    # ------------------------------------------------------------------

    _SAFE_EXPLAIN_PREFIX = re.compile(r"^\s*SELECT\b", re.IGNORECASE)

    def _run_explain_batch(
        self,
        queries: List[CapturedQuery],
        db_url: str,
    ) -> Dict[int, Dict[str, Any]]:
        """Run ``EXPLAIN (ANALYZE, FORMAT JSON)`` for a batch of queries.

        Only SELECT queries are sent to EXPLAIN ANALYZE.  Mutating
        statements (INSERT/UPDATE/DELETE) are silently skipped to
        prevent accidental data modification.
        """
        results: dict[int, dict[str, Any]] = {}

        try:
            from .dbcrust_integration import DBCrustIntegration

            integration = DBCrustIntegration(db_url)
            integration.connect()

            try:
                for query in queries:
                    # CRITICAL: Only EXPLAIN SELECT statements.  Running
                    # EXPLAIN ANALYZE on INSERT/UPDATE/DELETE would actually
                    # execute the mutation.
                    if not self._SAFE_EXPLAIN_PREFIX.match(query.sql):
                        logger.debug(
                            "Skipping EXPLAIN for non-SELECT query: %.60s...",
                            query.sql,
                        )
                        continue

                    try:
                        raw = integration._analyze_query_sync(query)
                        if "error" in raw:
                            continue

                        insights = raw.get("performance_insights", {})
                        operations = insights.get("operations", [])

                        plan_type = None
                        rows_examined = 0

                        for op in operations:
                            op_type = op.get("type", "")
                            if op_type == "Seq Scan":
                                tbl = query.table_names[0] if query.table_names else "?"
                                plan_type = f"Seq Scan on {tbl}"
                                rows_examined = op.get("rows", 0)
                            elif "Index" in op_type and plan_type is None:
                                plan_type = op_type
                                rows_examined = op.get("rows", 0)

                        # best suggestion from query_plan_analyzer
                        suggestion = None
                        django_fix = None
                        opt_sugg = raw.get("optimization_suggestions", [])
                        if opt_sugg:
                            top = opt_sugg[0]
                            suggestion = top.get("description", "")
                            django_fix = top.get("django_suggestion", "")
                            if django_fix and "\n" in django_fix:
                                django_fix = django_fix.split("\n")[0]

                        # fallback to warnings
                        if not suggestion:
                            warnings = insights.get("warnings", [])
                            if warnings:
                                suggestion = warnings[0]

                        results[id(query)] = {
                            "plan_type": plan_type,
                            "rows_examined": rows_examined or None,
                            "suggestion": suggestion,
                            "django_fix": django_fix,
                        }
                    except Exception as exc:  # noqa: BLE001
                        logger.debug("EXPLAIN failed for query: %s", exc)
            finally:
                integration.cleanup()

        except Exception as exc:  # noqa: BLE001
            logger.debug("Could not initialise EXPLAIN analysis: %s", exc)

        return results

    # ------------------------------------------------------------------
    # Heuristic fallback (no DB connection needed)
    # ------------------------------------------------------------------

    def _heuristic_analysis(
        self, query: CapturedQuery
    ) -> Optional[Dict[str, Any]]:
        """Best-effort plan guess based purely on the SQL text."""
        sql_upper = query.sql.upper()
        sql_upper_norm = " ".join(sql_upper.split())

        has_where = "WHERE" in sql_upper_norm
        has_limit = "LIMIT" in sql_upper_norm
        has_order = "ORDER BY" in sql_upper_norm
        table = query.table_names[0] if query.table_names else "?"

        # 1. No WHERE and no LIMIT → almost certainly a full scan
        if not has_where and not has_limit:
            return {
                "plan_type": f"Likely full scan on {table}",
                "suggestion": "No WHERE clause — scanning entire table",
                "django_fix": "Add .filter() to narrow results or slice [:limit]",
            }

        # 2. LIKE with leading wildcard
        if _RE_LEADING_WILDCARD_LIKE.search(query.sql):
            return {
                "plan_type": "Likely full scan (leading-wildcard LIKE)",
                "suggestion": "Leading wildcard prevents index usage",
                "django_fix": "Use SearchVector / SearchQuery for full-text search",
            }

        # 3. Function wrapping a column **in WHERE clause only** (issue #6)
        if has_where and _RE_FUNC_IN_WHERE.search(query.sql):
            return {
                "plan_type": "Possible full scan (function on column in WHERE)",
                "suggestion": "Function on column in WHERE prevents index usage",
                "django_fix": "Add a functional index or use db_collation",
            }

        # 4. NOT IN / NOT EXISTS — often slow
        if "NOT IN" in sql_upper_norm or "NOT EXISTS" in sql_upper_norm:
            return {
                "plan_type": f"Possibly slow anti-join on {table}",
                "suggestion": "NOT IN / NOT EXISTS can be slow on large tables",
                "django_fix": "Use .exclude() with indexed fields or LEFT JOIN approach",
            }

        # 5. Generic slow WHERE — suggest checking indexes (issue #5)
        if has_where and query.duration >= 0.05:
            where_match = _RE_WHERE_COLUMNS.findall(query.sql)
            if where_match:
                cols = ", ".join(c.lower() for c in where_match[:3])
                return {
                    "plan_type": f"Slow query on {table}",
                    "suggestion": f"Check indexes on: {cols}",
                    "django_fix": "Add db_index=True or Meta.indexes for filtered fields",
                }

        # 6. ORDER BY on slow query
        if has_order and query.duration >= 0.05:
            order_match = _RE_ORDER_BY_COLUMNS.findall(query.sql)
            if order_match:
                col = order_match[0].lower()
                return {
                    "plan_type": "Slow sort",
                    "suggestion": f"Sort on {col} may need an index",
                    "django_fix": f"Meta.indexes = [Index(fields=['{col}'])]",
                }

        return None


# ---------------------------------------------------------------------------
# Auto-detect Django DB URL
# ---------------------------------------------------------------------------

def get_django_db_url(alias: str = "default") -> Optional[str]:
    """
    Build a DBCrust connection URL from Django's ``DATABASES`` setting.

    Returns ``None`` (instead of raising) if Django is not configured or
    the engine is unsupported — this keeps the middleware safe.
    """
    try:
        from .utils import get_dbcrust_url

        return get_dbcrust_url(alias)
    except Exception:
        return None
