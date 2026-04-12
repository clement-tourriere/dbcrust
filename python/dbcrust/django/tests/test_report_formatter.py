"""
Tests for report_formatter.py — grades, formatting, edge cases.
"""

import unittest
from unittest.mock import Mock

from ..report_formatter import (
    IssueInfo,
    SlowQueryInfo,
    RequestPerformanceReport,
    build_report_from_analysis,
    calculate_grade,
    format_performance_report,
    _pattern_type_label,
    _truncate_sql,
    _oneliner,
)


class TestCalculateGrade(unittest.TestCase):
    """Tests for the calculate_grade function."""

    def test_perfect_score(self):
        grade, emoji = calculate_grade(
            total_queries=2, db_time_ms=10, critical_count=0,
            high_count=0, duplicate_count=0,
        )
        self.assertEqual(grade, "A")
        self.assertEqual(emoji, "✅")

    def test_moderate_queries_give_b(self):
        grade, _ = calculate_grade(
            total_queries=12, db_time_ms=80, critical_count=0,
            high_count=0, duplicate_count=3,
        )
        self.assertIn(grade, ("A", "B"))

    def test_many_queries_and_slow_give_c(self):
        grade, _ = calculate_grade(
            total_queries=20, db_time_ms=150, critical_count=0,
            high_count=1, duplicate_count=6,
        )
        self.assertIn(grade, ("C", "D"))

    def test_critical_issues_penalize_heavily(self):
        grade, _ = calculate_grade(
            total_queries=5, db_time_ms=30, critical_count=3,
            high_count=0, duplicate_count=0,
        )
        self.assertIn(grade, ("D", "F"))

    def test_worst_case_gives_f(self):
        grade, emoji = calculate_grade(
            total_queries=100, db_time_ms=1000, critical_count=5,
            high_count=5, duplicate_count=20,
        )
        self.assertEqual(grade, "F")
        self.assertEqual(emoji, "🔴")

    def test_score_never_above_a(self):
        grade, _ = calculate_grade(
            total_queries=0, db_time_ms=0, critical_count=0,
            high_count=0, duplicate_count=0,
        )
        self.assertEqual(grade, "A")

    def test_high_duplicates_penalize(self):
        grade1, _ = calculate_grade(
            total_queries=5, db_time_ms=30, critical_count=0,
            high_count=0, duplicate_count=0,
        )
        grade2, _ = calculate_grade(
            total_queries=5, db_time_ms=30, critical_count=0,
            high_count=0, duplicate_count=15,
        )
        # More duplicates should produce equal or worse grade
        grade_order = {"A": 0, "B": 1, "C": 2, "D": 3, "F": 4}
        self.assertGreaterEqual(grade_order[grade2], grade_order[grade1])


class TestPatternTypeLabel(unittest.TestCase):
    """Tests for _pattern_type_label."""

    def test_known_label(self):
        self.assertEqual(_pattern_type_label("n_plus_one"), "N+1 Query")

    def test_unknown_label_fallback(self):
        self.assertEqual(
            _pattern_type_label("some_unknown_pattern"),
            "Some Unknown Pattern",
        )

    def test_all_known_labels_return_strings(self):
        from ..report_formatter import _PATTERN_TYPE_LABELS
        for key in _PATTERN_TYPE_LABELS:
            label = _pattern_type_label(key)
            self.assertIsInstance(label, str)
            self.assertTrue(len(label) > 0)


class TestTruncateSql(unittest.TestCase):
    """Tests for _truncate_sql."""

    def test_short_sql_unchanged(self):
        sql = "SELECT id FROM users"
        self.assertEqual(_truncate_sql(sql, 50), sql)

    def test_long_sql_truncated(self):
        sql = "SELECT " + "a, " * 100 + " FROM users"
        result = _truncate_sql(sql, 40)
        self.assertTrue(result.endswith("..."))
        self.assertLessEqual(len(result), 43)  # 40 + "..."

    def test_whitespace_normalized(self):
        sql = "SELECT  id\n  FROM   users"
        result = _truncate_sql(sql, 100)
        self.assertNotIn("\n", result)
        self.assertNotIn("  ", result)

    def test_from_clause_preserved_when_possible(self):
        sql = "SELECT id, name, email FROM users WHERE id = 1 AND name = 'test' AND extra = 'long'"
        result = _truncate_sql(sql, 60)
        self.assertIn("FROM", result)


class TestOneliner(unittest.TestCase):
    """Tests for _oneliner."""

    def test_single_line(self):
        self.assertEqual(_oneliner("hello world"), "hello world")

    def test_multi_line(self):
        self.assertEqual(_oneliner("first line\nsecond line"), "first line")

    def test_truncation(self):
        long = "a" * 100
        result = _oneliner(long, 50)
        self.assertLessEqual(len(result), 50)
        self.assertTrue(result.endswith("..."))


class TestBuildReportFromAnalysis(unittest.TestCase):
    """Tests for build_report_from_analysis."""

    def _make_result(self, **overrides):
        """Create a mock AnalysisResult."""
        mock = Mock()
        mock.total_queries = overrides.get("total_queries", 5)
        mock.total_duration = overrides.get("total_duration", 0.05)
        mock.detected_patterns = overrides.get("detected_patterns", [])
        mock.duplicate_queries = overrides.get("duplicate_queries", 0)
        mock.queries_by_type = overrides.get(
            "queries_by_type", {"SELECT": 5}
        )
        return mock

    def test_empty_analysis(self):
        result = self._make_result()
        report = build_report_from_analysis(result, path="/test/")
        self.assertEqual(report.path, "/test/")
        self.assertEqual(report.total_queries, 5)
        self.assertEqual(report.grade, "A")
        self.assertEqual(len(report.critical_issues), 0)
        self.assertEqual(len(report.warnings), 0)
        self.assertEqual(len(report.hints), 0)

    def test_critical_pattern_bucketed(self):
        pattern = Mock()
        pattern.severity = "critical"
        pattern.pattern_type = "n_plus_one"
        pattern.description = "N+1 detected"
        pattern.affected_queries = [Mock()]
        pattern.recommendation = "Use prefetch_related"
        pattern.code_suggestion = None
        pattern.code_locations = []
        pattern.query_examples = []

        result = self._make_result(detected_patterns=[pattern])
        report = build_report_from_analysis(result)
        self.assertEqual(len(report.critical_issues), 1)
        self.assertEqual(report.critical_issues[0].label, "N+1 Query")

    def test_high_pattern_goes_to_warnings(self):
        pattern = Mock()
        pattern.severity = "high"
        pattern.pattern_type = "redundant_queries"
        pattern.description = "Redundant"
        pattern.affected_queries = []
        pattern.recommendation = "Cache"
        pattern.code_suggestion = None
        pattern.code_locations = []
        pattern.query_examples = []

        result = self._make_result(detected_patterns=[pattern])
        report = build_report_from_analysis(result)
        self.assertEqual(len(report.warnings), 1)

    def test_low_pattern_goes_to_hints(self):
        pattern = Mock()
        pattern.severity = "low"
        pattern.pattern_type = "unnecessary_ordering"
        pattern.description = "Unnecessary ordering"
        pattern.affected_queries = []
        pattern.recommendation = "Remove ordering"
        pattern.code_suggestion = None
        pattern.code_locations = []
        pattern.query_examples = []

        result = self._make_result(detected_patterns=[pattern])
        report = build_report_from_analysis(result)
        self.assertEqual(len(report.hints), 1)

    def test_slow_queries_passed_through(self):
        sq = SlowQueryInfo(
            sql="SELECT * FROM orders",
            duration_ms=150,
            tables=["orders"],
        )
        result = self._make_result()
        report = build_report_from_analysis(result, slow_queries=[sq])
        self.assertEqual(len(report.slow_queries), 1)

    def test_query_type_counts(self):
        result = self._make_result(
            queries_by_type={"SELECT": 10, "INSERT": 3, "UPDATE": 2, "DELETE": 1}
        )
        report = build_report_from_analysis(result)
        self.assertEqual(report.selects, 10)
        self.assertEqual(report.inserts, 3)
        self.assertEqual(report.updates, 2)
        self.assertEqual(report.deletes, 1)


class TestFormatPerformanceReport(unittest.TestCase):
    """Tests for format_performance_report."""

    def test_minimal_report(self):
        report = RequestPerformanceReport(
            method="GET", path="/", total_queries=1,
            db_time_ms=5, request_time_ms=10,
        )
        text = format_performance_report(report)
        self.assertIn("GET /", text)
        self.assertIn("Grade A", text)
        self.assertIn("Queries: 1", text)
        # Should be a single string (not multiple log calls)
        self.assertIsInstance(text, str)

    def test_report_with_critical_issues(self):
        issue = IssueInfo(
            severity="critical",
            label="N+1 Query",
            description="23x queries on order_items",
            affected_queries_count=23,
            recommendation="Use prefetch_related",
            code_locations=["orders/views.py:45"],
        )
        report = RequestPerformanceReport(
            critical_issues=[issue],
            grade="F", grade_emoji="🔴",
        )
        text = format_performance_report(report)
        self.assertIn("CRITICAL", text)
        self.assertIn("N+1 Query", text)
        self.assertIn("23x", text)
        self.assertIn("orders/views.py:45", text)

    def test_report_with_slow_queries(self):
        sq = SlowQueryInfo(
            sql="SELECT * FROM orders WHERE status=%s ORDER BY created_at",
            duration_ms=89,
            tables=["orders"],
            explain_plan_type="Seq Scan on orders",
            explain_rows_examined=50000,
            explain_django_fix="Meta.indexes = [Index(fields=['status', '-created_at'])]",
        )
        report = RequestPerformanceReport(slow_queries=[sq])
        text = format_performance_report(report)
        self.assertIn("SLOW QUERIES", text)
        self.assertIn("89ms", text)
        self.assertIn("Seq Scan on orders", text)
        self.assertIn("50,000 rows", text)

    def test_report_with_warnings_and_hints(self):
        warning = IssueInfo(
            severity="high",
            label="Redundant Queries",
            description="same query 5x",
        )
        hint = IssueInfo(
            severity="low",
            label="Missing .only()",
            description="fetching 18 unused fields",
            code_suggestion="Use .only('id','name','status')",
        )
        report = RequestPerformanceReport(
            warnings=[warning], hints=[hint],
        )
        text = format_performance_report(report)
        self.assertIn("WARNINGS", text)
        self.assertIn("HINTS", text)

    def test_report_includes_view_name(self):
        report = RequestPerformanceReport(
            view_name="orders:order-list",
        )
        text = format_performance_report(report)
        self.assertIn("orders:order-list", text)

    def test_report_duplicate_count(self):
        report = RequestPerformanceReport(duplicate_queries=7)
        text = format_performance_report(report)
        self.assertIn("Dupes: 7", text)

    def test_report_no_dupes_line_when_zero(self):
        report = RequestPerformanceReport(duplicate_queries=0)
        text = format_performance_report(report)
        self.assertNotIn("Dupes:", text)

    def test_slow_query_severity_icons(self):
        # >= 200ms -> red
        sq_red = SlowQueryInfo(sql="SELECT 1", duration_ms=250, tables=[])
        # >= 100ms -> orange
        sq_orange = SlowQueryInfo(sql="SELECT 1", duration_ms=150, tables=[])
        # < 100ms -> yellow
        sq_yellow = SlowQueryInfo(sql="SELECT 1", duration_ms=50, tables=[])

        report = RequestPerformanceReport(
            slow_queries=[sq_red, sq_orange, sq_yellow]
        )
        text = format_performance_report(report)
        self.assertIn("🔴", text)
        self.assertIn("🟠", text)
        self.assertIn("🟡", text)


if __name__ == "__main__":
    unittest.main()
