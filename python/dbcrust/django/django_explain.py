"""
EXPLAIN through Django's own database connection.

The previous pipeline routed EXPLAIN through a second native DBCrust
connection using the raw, un-parameterized SQL. Three independent defects
meant it could never return a result: Django's captured SQL still contains
``%s`` placeholders (a syntax error once EXPLAINed verbatim), the first
result row — the column header — was fed to ``json.loads``, and every
failure was logged at DEBUG, hiding that the feature was dead.

Running EXPLAIN on the connection Django already holds fixes all of it:

- the driver binds the captured ``params`` exactly as the original query did,
- the database alias, search path, and session settings match the app's,
- no second connection, URL detection, or native client is needed,
- MySQL and SQLite work too (the old path was PostgreSQL-only).

By default plans are collected **without** ``ANALYZE`` so the statement is
*planned but not re-executed*; pass ``analyze_execution=True`` (middleware
option ``EXPLAIN_ANALYZE``) to re-run slow SELECTs for actual row counts
and timings.
"""

import json
import logging
from typing import Any, Dict, List, Optional, Tuple

from .query_collector import CapturedQuery

logger = logging.getLogger("dbcrust.performance")

#: Vendors `run_explain` knows how to drive and interpret.
SUPPORTED_VENDORS = ("postgresql", "mysql", "sqlite")


def explain_supported(using: str = "default") -> Optional[str]:
    """Return the connection's vendor when EXPLAIN is supported, else None."""
    try:
        from django.db import connections

        vendor = connections[using].vendor
    except Exception as exc:  # pragma: no cover - defensive
        logger.debug("Could not determine database vendor for %r: %s", using, exc)
        return None
    return vendor if vendor in SUPPORTED_VENDORS else None


def run_explain(
    query: CapturedQuery,
    using: str = "default",
    analyze_execution: bool = False,
) -> Optional[Dict[str, Any]]:
    """
    EXPLAIN a captured SELECT on Django's own connection.

    Returns ``{"vendor": ..., "plan": ...}`` where ``plan`` is the parsed
    EXPLAIN output (PostgreSQL/MySQL: JSON structure; SQLite: list of
    ``EXPLAIN QUERY PLAN`` detail strings), or ``None`` when EXPLAIN could
    not run. Failures are logged at WARNING — silently swallowing them is
    how the previous implementation stayed broken unnoticed.
    """
    if not query.sql.lstrip()[:6].upper().startswith("SELECT"):
        # Defense in depth: with ANALYZE a non-SELECT would actually execute
        logger.debug("Refusing to EXPLAIN non-SELECT: %.60s…", query.sql)
        return None

    try:
        from django.db import connections

        conn = connections[using]
        vendor = conn.vendor
    except Exception as exc:
        logger.warning("EXPLAIN unavailable (no usable connection %r): %s", using, exc)
        return None

    if vendor not in SUPPORTED_VENDORS:
        logger.debug("EXPLAIN not supported for vendor %r", vendor)
        return None

    if vendor == "postgresql":
        options = "ANALYZE, FORMAT JSON" if analyze_execution else "FORMAT JSON"
        prefix = f"EXPLAIN ({options}) "
    elif vendor == "mysql":
        prefix = "EXPLAIN FORMAT=JSON "
    else:  # sqlite
        prefix = "EXPLAIN QUERY PLAN "

    # Django's execute_wrapper hands us params exactly as the driver expects
    # them (tuple/list/dict); pass them straight back so placeholders bind
    params = query.params or None

    try:
        with conn.cursor() as cursor:
            cursor.execute(prefix + query.sql, params)
            rows = cursor.fetchall()
    except Exception as exc:
        logger.warning("EXPLAIN failed for %.80s… : %s", query.sql, exc)
        return None

    if not rows:
        return None

    try:
        if vendor == "postgresql":
            # One row, one column; psycopg parses the json type to Python
            # objects, other drivers may hand back a string
            plan = rows[0][0]
            if isinstance(plan, str):
                plan = json.loads(plan)
            return {"vendor": vendor, "plan": plan}
        if vendor == "mysql":
            plan = rows[0][0]
            if isinstance(plan, str):
                plan = json.loads(plan)
            return {"vendor": vendor, "plan": plan}
        # sqlite: rows of (id, parent, notused, detail)
        details = [str(row[-1]) for row in rows]
        return {"vendor": vendor, "plan": details}
    except (json.JSONDecodeError, IndexError, TypeError) as exc:
        logger.warning("Could not parse EXPLAIN output: %s", exc)
        return None


def summarize_for_slow_query(
    query: CapturedQuery,
    using: str = "default",
    analyze_execution: bool = False,
) -> Optional[Dict[str, Any]]:
    """
    EXPLAIN ``query`` and condense the plan into the fields the slow-query
    report displays: ``plan_type``, ``rows_examined``, ``suggestion``,
    ``django_fix``.
    """
    result = run_explain(query, using=using, analyze_execution=analyze_execution)
    if not result:
        return None

    vendor = result["vendor"]
    plan = result["plan"]

    if vendor == "postgresql":
        return _summarize_postgresql(plan, query)
    if vendor == "mysql":
        return _summarize_mysql(plan, query)
    return _summarize_sqlite(plan, query)


# ---------------------------------------------------------------------------
# Vendor-specific plan condensation
# ---------------------------------------------------------------------------


def _walk_pg_nodes(node: Dict[str, Any]):
    """Yield every node of a PostgreSQL plan tree, depth-first."""
    if not isinstance(node, dict):
        return
    yield node
    for child in node.get("Plans", []) or []:
        yield from _walk_pg_nodes(child)


def _summarize_postgresql(
    plan: Any, query: CapturedQuery
) -> Optional[Dict[str, Any]]:
    if isinstance(plan, list) and plan:
        plan_info = plan[0]
    elif isinstance(plan, dict):
        plan_info = plan
    else:
        return None

    root = plan_info.get("Plan")
    if not isinstance(root, dict):
        return None

    plan_type = None
    rows_examined = 0
    for node in _walk_pg_nodes(root):
        node_type = node.get("Node Type", "")
        # Plain EXPLAIN has no Actual Rows; fall back to the estimate
        rows = node.get("Actual Rows") or node.get("Plan Rows", 0)
        if node_type == "Seq Scan":
            table = node.get("Relation Name") or (
                query.table_names[0] if query.table_names else "?"
            )
            plan_type = f"Seq Scan on {table}"
            rows_examined = rows
            break
        if "Index" in node_type and plan_type is None:
            plan_type = node_type
            rows_examined = rows

    # The full plan analyzer produces ranked, Django-aware suggestions
    suggestion = None
    django_fix = None
    try:
        from .query_plan_analyzer import analyze_explain_output

        suggestions, _summary = analyze_explain_output(plan)
        if suggestions:
            top = suggestions[0]
            suggestion = top.description
            django_fix = top.django_suggestion
            if django_fix and "\n" in django_fix:
                django_fix = django_fix.split("\n")[0]
    except Exception as exc:  # pragma: no cover - defensive
        logger.debug("Plan analyzer failed: %s", exc)

    if plan_type is None and suggestion is None:
        return None

    return {
        "plan_type": plan_type,
        "rows_examined": rows_examined or None,
        "suggestion": suggestion,
        "django_fix": django_fix,
    }


def _walk_mysql_tables(node: Any):
    """Yield every ``table`` dict in a MySQL JSON plan, depth-first."""
    if isinstance(node, dict):
        if "table" in node and isinstance(node["table"], dict):
            yield node["table"]
        for value in node.values():
            yield from _walk_mysql_tables(value)
    elif isinstance(node, list):
        for item in node:
            yield from _walk_mysql_tables(item)


def _summarize_mysql(plan: Any, query: CapturedQuery) -> Optional[Dict[str, Any]]:
    if not isinstance(plan, dict):
        return None

    for table in _walk_mysql_tables(plan.get("query_block", plan)):
        access_type = table.get("access_type")
        name = table.get("table_name") or (
            query.table_names[0] if query.table_names else "?"
        )
        if access_type == "ALL":
            rows = table.get("rows_examined_per_scan")
            return {
                "plan_type": f"Full table scan on {name}",
                "rows_examined": rows,
                "suggestion": f"No usable index for the filter on {name}",
                "django_fix": "Add db_index=True or Meta.indexes for the filtered fields",
            }
        if access_type in ("index", "range", "ref", "eq_ref", "const"):
            return {
                "plan_type": f"{access_type} access on {name}"
                + (f" via {table['key']}" if table.get("key") else ""),
                "rows_examined": table.get("rows_examined_per_scan"),
                "suggestion": None,
                "django_fix": None,
            }
    return None


def _summarize_sqlite(plan: Any, query: CapturedQuery) -> Optional[Dict[str, Any]]:
    if not isinstance(plan, list) or not plan:
        return None

    details: List[str] = [str(d) for d in plan]
    for detail in details:
        upper = detail.upper()
        if upper.startswith("SCAN") and "USING" not in upper:
            table = detail.split()[1] if len(detail.split()) > 1 else (
                query.table_names[0] if query.table_names else "?"
            )
            return {
                "plan_type": f"Full scan: {detail}",
                "rows_examined": None,
                "suggestion": f"No index used for {table}",
                "django_fix": "Add db_index=True or Meta.indexes for the filtered fields",
            }

    first = details[0]
    return {
        "plan_type": first,
        "rows_examined": None,
        "suggestion": None,
        "django_fix": None,
    }


__all__: Tuple[str, ...] = (
    "SUPPORTED_VENDORS",
    "explain_supported",
    "run_explain",
    "summarize_for_slow_query",
)
