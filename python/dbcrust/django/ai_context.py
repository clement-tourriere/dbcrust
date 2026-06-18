"""
Build a compact Django context block (models + ORM code locations) for the
DBCrust AI assistant.

The AI lives in the Rust core and only sees the raw SQL schema. Django model
definitions, ORM relationships, and the code that issues queries are gathered
here, on the Python side, and passed into the Rust agent (via
``dbcrust._internal.run_ai_investigation``) so the model can recommend
Django-level fixes (``select_related`` / ``prefetch_related`` / ``db_index`` / …)
with exact ``file:line`` references — not just raw SQL.
"""

from __future__ import annotations

import os
from typing import Any, List, Optional, Sequence

# Cap on the rendered context handed to the model — keeps token cost bounded.
MAX_CONTEXT_CHARS = 12000


def load_project_models(project_root: Optional[str]) -> List[Any]:
    """Introspect Django models via AST. Returns [] if unavailable."""
    if not project_root:
        return []
    try:
        from .project_analyzer import DjangoProjectAnalyzer

        return DjangoProjectAnalyzer(project_root).analyze_models_only()
    except Exception:
        # Not a Django project root, unparseable models, etc. — degrade to the
        # plain (schema-only) agent rather than failing the whole investigation.
        return []


def _default_project_root() -> Optional[str]:
    try:
        from django.conf import settings

        base = getattr(settings, "BASE_DIR", None)
        return str(base) if base else None
    except Exception:
        return None


def _apply_dbcrust_config_dir_setting() -> None:
    """Let Django settings point the Rust core at the same config as the CLI.

    The Rust core reads ``DBCRUST_CONFIG_DIR`` at call time. This setting is
    useful when ``runserver``/gunicorn/Docker runs with a different ``HOME`` than
    the shell where ``dbcrust`` was configured.
    """
    try:
        from django.conf import settings

        config_dir = getattr(settings, "DBCRUST_CONFIG_DIR", None)
    except Exception:
        return

    if config_dir:
        os.environ["DBCRUST_CONFIG_DIR"] = str(config_dir)


def _render_model(model: Any) -> str:
    lines = [f"# {model.file_path}:{model.line_number}",
             f"class {model.name} (db_table={model.table_name})"]
    if model.fields:
        lines.append("  fields: " + ", ".join(f"{n}:{t}" for n, t in model.fields.items()))
    if model.foreign_keys:
        lines.append("  foreign_keys: " + ", ".join(f"{n} -> {t}" for n, t in model.foreign_keys.items()))
    if model.many_to_many:
        lines.append("  many_to_many: " + ", ".join(f"{n} -> {t}" for n, t in model.many_to_many.items()))
    if model.indexes:
        lines.append("  indexes: " + ", ".join(str(i) for i in model.indexes))
    return "\n".join(lines)


def _stack_origin(stack_trace: Any) -> str:
    # The collector orders frames most-relevant-first; show the top user frame.
    if not stack_trace:
        return ""
    if isinstance(stack_trace, (list, tuple)):
        return str(stack_trace[0]).strip()
    return str(stack_trace).strip()


def build_django_context(
    models: Sequence[Any],
    queries: Optional[Sequence[Any]] = None,
    patterns: Optional[Sequence[Any]] = None,
    tables: Optional[Sequence[str]] = None,
) -> str:
    """Render a compact Django context block.

    Args:
        models: ``DjangoModel`` instances (from
            ``DjangoProjectAnalyzer.analyze_models_only()``).
        queries: optional captured queries (``CapturedQuery``) with stack traces.
        patterns: optional detected patterns (``DetectedPattern``) with
            descriptions / recommendations.
        tables: optional table-name allowlist to scope which models are included
            (e.g. the tables touched by the captured queries).
    """
    table_filter = {t.lower() for t in tables} if tables else None

    selected = [
        m
        for m in models
        if table_filter is None
        or m.table_name.lower() in table_filter
        or m.name.lower() in table_filter
    ]
    if not selected:
        # Nothing matched the filter — fall back to all known models.
        selected = list(models)

    parts: List[str] = []

    if selected:
        # db_table -> Model map ties SQL tables back to Django models.
        parts.append(
            "Model ↔ table map: "
            + ", ".join(f"{m.table_name}={m.name}" for m in selected)
        )
        parts.append("MODELS:\n" + "\n\n".join(_render_model(m) for m in selected))

    if queries:
        qlines = []
        for q in queries:
            sql = " ".join(getattr(q, "sql", "").split())
            if len(sql) > 200:
                sql = sql[:200] + "…"
            origin = _stack_origin(getattr(q, "stack_trace", None))
            dur_ms = (getattr(q, "duration", 0.0) or 0.0) * 1000
            loc = f"  @ {origin}" if origin else ""
            qlines.append(f"- ({dur_ms:.1f} ms) {sql}{loc}")
        if qlines:
            parts.append("CAPTURED QUERIES (and the code that issued them):\n" + "\n".join(qlines))

    if patterns:
        plines = []
        for p in patterns:
            desc = getattr(p, "description", None) or getattr(p, "pattern_type", "")
            rec = getattr(p, "recommendation", "")
            plines.append(f"- {desc}" + (f" → {rec}" if rec else ""))
        if plines:
            parts.append("DETECTED PATTERNS:\n" + "\n".join(plines))

    context = "\n\n".join(parts)
    if len(context) > MAX_CONTEXT_CHARS:
        context = context[:MAX_CONTEXT_CHARS] + "\n… (context truncated)"
    return context


def ask_ai(
    question: str,
    *,
    database: str = "default",
    project_root: Optional[str] = None,
    agentic: bool = True,
    max_iterations: Optional[int] = None,
    stdout_progress: bool = False,
) -> str:
    """Ask the DBCrust AI a question with Django model context.

    Static entry point (no request capture): introspects the project's models
    and hands them to the Rust agent, which investigates the live database
    read-only and returns a Django-aware analysis. For richer context that
    includes the actual slow queries and the code that issued them, use
    ``DjangoAnalyzer.investigate_ai`` inside an ``analyze()`` block instead.

    Progress is silent by default (this returns a string). Pass
    ``stdout_progress=True`` to stream the agent's tool trace to stdout — the
    ``dbcrust_ai`` management command does this.
    """
    _apply_dbcrust_config_dir_setting()
    from dbcrust._internal import run_ai_investigation  # ty: ignore[unresolved-import]

    from .utils import get_dbcrust_url

    root = project_root or _default_project_root()
    models = load_project_models(root)
    context = build_django_context(models) if models else ""
    url = get_dbcrust_url(database)
    return run_ai_investigation(
        url, question, context, agentic, max_iterations, None, stdout_progress
    )


def _report_tables(report: Any) -> List[str]:
    tables = set()
    for q in getattr(report, "slow_queries", None) or []:
        for t in getattr(q, "tables", None) or []:
            tables.add(t)
    return sorted(tables)


def summarize_report(report: Any) -> str:
    """Render one request's slow queries + flagged issues as AI context text.

    Duck-typed over ``RequestPerformanceReport`` so this module stays free of a
    hard dependency on ``report_formatter``.
    """
    lines: List[str] = []
    head = f"Request: {getattr(report, 'method', '?')} {getattr(report, 'path', '?')}"
    view_name = getattr(report, "view_name", None)
    if view_name:
        head += f"  (view: {view_name})"
    lines.append(head)

    slow = getattr(report, "slow_queries", None) or []
    if slow:
        lines.append("\nSLOW QUERIES:")
        for q in slow:
            sql = " ".join(getattr(q, "sql", "").split())
            if len(sql) > 300:
                sql = sql[:300] + "…"
            dur = getattr(q, "duration_ms", 0.0) or 0.0
            tables = ", ".join(getattr(q, "tables", None) or [])
            lines.append(f"- ({dur:.1f} ms) {sql}" + (f"   [tables: {tables}]" if tables else ""))

    def _issues(bucket: str, name: str) -> None:
        items = getattr(report, bucket, None) or []
        if not items:
            return
        lines.append(f"\n{name}:")
        for it in items:
            label = getattr(it, "label", "")
            desc = getattr(it, "description", "")
            rec = getattr(it, "recommendation", "")
            locs = ", ".join(getattr(it, "code_locations", None) or [])
            entry = f"- [{label}] {desc}"
            if rec:
                entry += f" → {rec}"
            if locs:
                entry += f"  @ {locs}"
            lines.append(entry)

    _issues("critical_issues", "CRITICAL ISSUES")
    _issues("warnings", "WARNINGS")
    _issues("hints", "HINTS")
    return "\n".join(lines)


def investigate_report(
    report: Any,
    *,
    database: str = "default",
    project_root: Optional[str] = None,
    agentic: bool = True,
    max_iterations: Optional[int] = None,
    progress_path: Optional[str] = None,
) -> str:
    """Run an AI investigation focused on one request's slow queries + issues.

    Backs the dashboard's one-click "Investigate with AI" button: it scopes the
    Django model context to the tables the request touched, attaches the slow
    queries and flagged issues, and asks the agent for Django-level fixes. When
    ``progress_path`` is given, the agent's narration is streamed to that file
    for the dashboard to tail.
    """
    _apply_dbcrust_config_dir_setting()
    from dbcrust._internal import run_ai_investigation  # ty: ignore[unresolved-import]

    from .utils import get_dbcrust_url

    root = project_root or _default_project_root()
    models = load_project_models(root)
    tables = _report_tables(report)
    models_ctx = build_django_context(models, tables=tables or None) if models else ""
    report_ctx = summarize_report(report)
    context = f"{models_ctx}\n\n{report_ctx}".strip() if models_ctx else report_ctx

    question = (
        "This Django request was flagged by the performance analyzer. Investigate its slow "
        "queries and ORM issues against the live database, explain the root cause, and recommend "
        "Django-level fixes (select_related / prefetch_related / only / db_index / Meta.indexes) "
        "with the underlying SQL/DDL."
    )
    url = get_dbcrust_url(database)
    return run_ai_investigation(url, question, context, agentic, max_iterations, progress_path)
