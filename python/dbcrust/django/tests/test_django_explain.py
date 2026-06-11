"""
Tests for EXPLAIN-through-Django.

The SQLite tests are real integration tests: they execute EXPLAIN QUERY
PLAN against the in-memory test database with bound parameters — the exact
scenario the old DBCrust-native pipeline could never handle (raw ``%s``
placeholders were a syntax error).
"""

import unittest
from datetime import datetime

from django.db import connection

from ..django_explain import (
    _summarize_mysql,
    _summarize_postgresql,
    explain_supported,
    run_explain,
    summarize_for_slow_query,
)
from ..query_collector import CapturedQuery
from ..slow_query_analyzer import SlowQueryAnalyzer


def _make_query(sql, params=(), query_type="SELECT", tables=None, duration=0.1):
    return CapturedQuery(
        sql=sql,
        params=params,
        duration=duration,
        timestamp=datetime.now(),
        stack_trace=["test.py:1"],
        query_type=query_type,
        table_names=tables or [],
    )


class TestExplainSupported(unittest.TestCase):
    def test_sqlite_test_database_is_supported(self):
        self.assertEqual(explain_supported("default"), "sqlite")

    def test_unknown_alias_returns_none(self):
        self.assertIsNone(explain_supported("nope-not-an-alias"))


class TestRunExplainSqlite(unittest.TestCase):
    """Real EXPLAIN QUERY PLAN runs against the in-memory database."""

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        with connection.cursor() as cursor:
            cursor.execute(
                "CREATE TABLE IF NOT EXISTS explain_demo "
                "(id INTEGER PRIMARY KEY, name TEXT)"
            )
            cursor.execute("INSERT INTO explain_demo (name) VALUES ('a'), ('b')")

    def test_explain_with_bound_params(self):
        """%s placeholders must be bound by the driver, not EXPLAINed raw."""
        query = _make_query(
            "SELECT * FROM explain_demo WHERE id = %s",
            params=(1,),
            tables=["explain_demo"],
        )
        result = run_explain(query, using="default")
        self.assertIsNotNone(result)
        self.assertEqual(result["vendor"], "sqlite")
        self.assertTrue(result["plan"])  # at least one plan detail line
        self.assertTrue(all(isinstance(d, str) for d in result["plan"]))

    def test_full_scan_summarized(self):
        query = _make_query(
            "SELECT * FROM explain_demo WHERE name = %s",
            params=("a",),
            tables=["explain_demo"],
        )
        summary = summarize_for_slow_query(query, using="default")
        self.assertIsNotNone(summary)
        self.assertIn("scan", summary["plan_type"].lower())

    def test_non_select_refused(self):
        query = _make_query(
            "DELETE FROM explain_demo WHERE id = %s",
            params=(1,),
            query_type="DELETE",
        )
        self.assertIsNone(run_explain(query, using="default"))

    def test_slow_query_analyzer_end_to_end(self):
        """analyze(using=...) populates EXPLAIN-backed plan info."""
        analyzer = SlowQueryAnalyzer()
        query = _make_query(
            "SELECT * FROM explain_demo WHERE name = %s",
            params=("a",),
            tables=["explain_demo"],
            duration=0.2,
        )
        infos = analyzer.analyze([query], using="default")
        self.assertEqual(len(infos), 1)
        self.assertIsNotNone(infos[0].explain_plan_type)

    def test_legacy_db_url_maps_to_default_alias(self):
        analyzer = SlowQueryAnalyzer()
        query = _make_query(
            "SELECT * FROM explain_demo",
            tables=["explain_demo"],
            duration=0.2,
        )
        infos = analyzer.analyze([query], db_url="postgres://ignored/legacy")
        self.assertEqual(len(infos), 1)
        self.assertIsNotNone(infos[0].explain_plan_type)


class TestPostgresqlSummarizer(unittest.TestCase):
    """Pure-function tests with a canned EXPLAIN (FORMAT JSON) plan."""

    CANNED_PLAN = [
        {
            "Plan": {
                "Node Type": "Seq Scan",
                "Relation Name": "users",
                "Total Cost": 1500.0,
                "Plan Rows": 50000,
                "Plan Width": 100,
            },
            "Planning Time": 0.2,
        }
    ]

    def test_seq_scan_detected(self):
        query = _make_query("SELECT * FROM users", tables=["users"])
        summary = _summarize_postgresql(self.CANNED_PLAN, query)
        self.assertIsNotNone(summary)
        self.assertEqual(summary["plan_type"], "Seq Scan on users")
        self.assertEqual(summary["rows_examined"], 50000)

    def test_index_scan_detected(self):
        plan = [
            {
                "Plan": {
                    "Node Type": "Index Scan",
                    "Relation Name": "users",
                    "Plan Rows": 1,
                }
            }
        ]
        query = _make_query("SELECT * FROM users WHERE id = %s", params=(1,))
        summary = _summarize_postgresql(plan, query)
        self.assertIsNotNone(summary)
        self.assertEqual(summary["plan_type"], "Index Scan")

    def test_garbage_plan_returns_none(self):
        query = _make_query("SELECT 1")
        self.assertIsNone(_summarize_postgresql("not a plan", query))
        self.assertIsNone(_summarize_postgresql([], query))


class TestMysqlSummarizer(unittest.TestCase):
    def test_full_table_scan_detected(self):
        plan = {
            "query_block": {
                "table": {
                    "table_name": "users",
                    "access_type": "ALL",
                    "rows_examined_per_scan": 5000,
                }
            }
        }
        query = _make_query("SELECT * FROM users", tables=["users"])
        summary = _summarize_mysql(plan, query)
        self.assertIsNotNone(summary)
        self.assertEqual(summary["plan_type"], "Full table scan on users")
        self.assertEqual(summary["rows_examined"], 5000)

    def test_indexed_access_detected(self):
        plan = {
            "query_block": {
                "nested_loop": [
                    {
                        "table": {
                            "table_name": "users",
                            "access_type": "ref",
                            "key": "users_email_idx",
                            "rows_examined_per_scan": 1,
                        }
                    }
                ]
            }
        }
        query = _make_query("SELECT * FROM users WHERE email = %s", params=("x",))
        summary = _summarize_mysql(plan, query)
        self.assertIsNotNone(summary)
        self.assertIn("ref access on users", summary["plan_type"])
        self.assertIn("users_email_idx", summary["plan_type"])
