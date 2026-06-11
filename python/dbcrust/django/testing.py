"""
Query-budget assertions for tests and CI.

Fail the build when a view or function regresses into an N+1 or blows its
query budget, instead of discovering it in production. Works with any test
runner; with pytest the ``dbcrust`` fixture (auto-registered entry point)
exposes the same helpers pre-bound.

Usage::

    from dbcrust.django.testing import assert_max_queries, assert_no_n_plus_one

    def test_dashboard(client):
        with assert_max_queries(10):
            client.get("/dashboard/")

    def test_book_list():
        with assert_no_n_plus_one():
            for book in Book.objects.select_related("author"):
                _ = book.author.name
"""

from collections import Counter
from contextlib import contextmanager
from typing import Iterator, List

from .query_collector import CapturedQuery, QueryCollector

__all__ = [
    "QueryBudgetExceeded",
    "NPlusOneDetected",
    "capture_queries",
    "assert_max_queries",
    "assert_no_n_plus_one",
]


class QueryBudgetExceeded(AssertionError):
    """More queries executed than the declared budget."""


class NPlusOneDetected(AssertionError):
    """An N+1 query pattern was detected inside the guarded block."""


def _summarize_queries(queries: List[CapturedQuery], limit: int = 5) -> str:
    """Group queries by their normalized shape, most frequent first."""
    patterns = Counter(q.get_base_query() for q in queries)
    lines = [
        f"  {count}× {pattern[:140]}"
        for pattern, count in patterns.most_common(limit)
    ]
    if len(patterns) > limit:
        lines.append(f"  … and {len(patterns) - limit} more distinct statements")
    return "\n".join(lines)


@contextmanager
def capture_queries(using: str = "default") -> Iterator[QueryCollector]:
    """Collect every query executed on ``using`` inside the block."""
    from django.db import connections

    collector = QueryCollector()
    collector.start_collection()
    try:
        with connections[using].execute_wrapper(collector):
            yield collector
    finally:
        collector.stop_collection()


@contextmanager
def assert_max_queries(count: int, using: str = "default") -> Iterator[QueryCollector]:
    """
    Fail (``QueryBudgetExceeded``, an ``AssertionError``) when the block
    executes more than ``count`` queries on the ``using`` alias.
    """
    with capture_queries(using) as collector:
        yield collector

    executed = len(collector.queries)
    if executed > count:
        raise QueryBudgetExceeded(
            f"{executed} queries executed, budget is {count}.\n"
            f"Query shapes (most frequent first):\n"
            f"{_summarize_queries(collector.queries)}"
        )


@contextmanager
def assert_no_n_plus_one(using: str = "default") -> Iterator[QueryCollector]:
    """
    Fail (``NPlusOneDetected``, an ``AssertionError``) when the block runs an
    N+1 pattern: three or more SELECTs of the same shape with different
    parameters (the detection threshold of :class:`PatternDetector`).
    """
    with capture_queries(using) as collector:
        yield collector

    from .pattern_detector import PatternDetector

    patterns = PatternDetector(collector.queries).analyze()
    n_plus_one = [p for p in patterns if p.pattern_type == "n_plus_one"]
    if n_plus_one:
        worst = max(n_plus_one, key=lambda p: len(p.affected_queries))
        example = worst.affected_queries[0].sql if worst.affected_queries else "?"
        raise NPlusOneDetected(
            f"N+1 query pattern detected: {len(worst.affected_queries)} similar "
            f"queries.\n"
            f"  Example: {example[:140]}\n"
            f"  Fix: {worst.recommendation}\n"
            f"All captured query shapes:\n"
            f"{_summarize_queries(collector.queries)}"
        )
