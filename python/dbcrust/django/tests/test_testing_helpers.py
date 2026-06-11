"""
Tests for the query-budget assertion helpers (dbcrust.django.testing).

These run real SQL against the in-memory SQLite test database — the
helpers wrap Django's execute_wrapper, so plain cursor statements count
exactly like ORM queries.
"""

import unittest

from django.db import connection

from ..testing import (
    NPlusOneDetected,
    QueryBudgetExceeded,
    assert_max_queries,
    assert_no_n_plus_one,
    capture_queries,
)


def _run(sql, params=None):
    with connection.cursor() as cursor:
        cursor.execute(sql, params)
        return cursor.fetchall()


class TestingHelpersBase(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        with connection.cursor() as cursor:
            cursor.execute(
                "CREATE TABLE IF NOT EXISTS budget_books "
                "(id INTEGER PRIMARY KEY, author_id INTEGER, title TEXT)"
            )
            cursor.execute(
                "INSERT INTO budget_books (author_id, title) "
                "VALUES (1, 'a'), (2, 'b'), (3, 'c'), (4, 'd')"
            )


class TestCaptureQueries(TestingHelpersBase):
    def test_captures_inside_block_only(self):
        _run("SELECT 1")  # outside — must not count
        with capture_queries() as collector:
            _run("SELECT * FROM budget_books")
            _run("SELECT COUNT(*) FROM budget_books")
        self.assertEqual(len(collector.queries), 2)
        _run("SELECT 1")  # outside — must not count
        self.assertEqual(len(collector.queries), 2)


class TestAssertMaxQueries(TestingHelpersBase):
    def test_within_budget_passes(self):
        with assert_max_queries(2):
            _run("SELECT * FROM budget_books")

    def test_over_budget_fails_with_summary(self):
        with self.assertRaises(QueryBudgetExceeded) as cm:
            with assert_max_queries(1):
                _run("SELECT * FROM budget_books")
                _run("SELECT COUNT(*) FROM budget_books")
        message = str(cm.exception)
        self.assertIn("2 queries executed, budget is 1", message)
        self.assertIn("budget_books", message)

    def test_is_an_assertion_error(self):
        self.assertTrue(issubclass(QueryBudgetExceeded, AssertionError))


class TestAssertNoNPlusOne(TestingHelpersBase):
    def test_n_plus_one_loop_detected(self):
        with self.assertRaises(NPlusOneDetected) as cm:
            with assert_no_n_plus_one():
                _run("SELECT * FROM budget_books")
                for author_id in (1, 2, 3, 4):
                    _run(
                        "SELECT * FROM budget_books WHERE author_id = %s",
                        [author_id],
                    )
        message = str(cm.exception)
        self.assertIn("N+1 query pattern detected", message)
        self.assertIn("author_id", message)

    def test_distinct_queries_pass(self):
        with assert_no_n_plus_one():
            _run("SELECT * FROM budget_books")
            _run("SELECT COUNT(*) FROM budget_books")
            _run("SELECT title FROM budget_books WHERE id = %s", [1])


class TestPytestFixture(TestingHelpersBase):
    def test_fixture_helpers_bound(self):
        from dbcrust.pytest_plugin import DbcrustQueryHelpers

        helpers = DbcrustQueryHelpers()
        with helpers.max_queries(1):
            _run("SELECT 1")
        with self.assertRaises(QueryBudgetExceeded):
            with helpers.max_queries(0):
                _run("SELECT 1")
