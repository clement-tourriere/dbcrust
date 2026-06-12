"""Tests for the performance dashboard: store, views, and middleware hook."""

import logging

import pytest
from django.http import HttpResponse
from django.test import Client, RequestFactory, override_settings

from dbcrust.django import dashboard
from dbcrust.django.dashboard import DashboardStore
from dbcrust.django.middleware import PerformanceAnalysisMiddleware
from dbcrust.django.report_formatter import (
    IssueInfo,
    RequestPerformanceReport,
    SlowQueryInfo,
)


def _make_report(**overrides) -> RequestPerformanceReport:
    defaults = dict(
        method="GET",
        path="/shop/orders/",
        view_name="shop.views.order_list",
        status_code=200,
        total_queries=12,
        db_time_ms=45.6,
        request_time_ms=120.3,
        duplicate_queries=2,
        selects=10,
        inserts=1,
        updates=1,
        deletes=0,
        grade="C",
    )
    defaults.update(overrides)
    return RequestPerformanceReport(**defaults)


@pytest.fixture(autouse=True)
def _clean_store():
    """The store is a process-wide singleton — isolate every test."""
    dashboard.store.clear()
    yield
    dashboard.store.clear()


# ---------------------------------------------------------------------------
# Store
# ---------------------------------------------------------------------------


class TestDashboardStore:
    def test_records_newest_first(self):
        store = DashboardStore()
        first = store.add(_make_report(path="/first/"))
        second = store.add(_make_report(path="/second/"))

        records = store.records()
        assert [r.id for r in records] == [second.id, first.id]

    def test_ring_buffer_caps_entries(self):
        store = DashboardStore(max_entries=3)
        for i in range(5):
            store.add(_make_report(path=f"/page-{i}/"))

        records = store.records()
        assert len(records) == 3
        assert [r.report.path for r in records] == ["/page-4/", "/page-3/", "/page-2/"]

    def test_set_max_entries_keeps_most_recent(self):
        store = DashboardStore(max_entries=10)
        for i in range(6):
            store.add(_make_report(path=f"/page-{i}/"))

        store.set_max_entries(2)
        assert [r.report.path for r in store.records()] == ["/page-5/", "/page-4/"]

    def test_get_and_clear(self):
        store = DashboardStore()
        record = store.add(_make_report())

        assert store.get(record.id) is record
        assert store.get(record.id + 999) is None

        store.clear()
        assert store.records() == []
        assert store.get(record.id) is None

    def test_stats_empty(self):
        stats = DashboardStore().stats()
        assert stats["request_count"] == 0
        assert stats["avg_queries"] == 0.0

    def test_stats_aggregates(self):
        store = DashboardStore()
        store.add(_make_report(total_queries=10, db_time_ms=100.0))
        store.add(
            _make_report(
                total_queries=20,
                db_time_ms=200.0,
                critical_issues=[IssueInfo(severity="critical", label="N+1 Query", description="x")],
            )
        )

        stats = store.stats()
        assert stats["request_count"] == 2
        assert stats["with_issues"] == 1
        assert stats["total_queries"] == 30
        assert stats["avg_queries"] == 15.0
        assert stats["avg_db_time_ms"] == 150.0

    def test_record_counts_and_grade_class(self):
        store = DashboardStore()
        record = store.add(
            _make_report(
                grade="F",
                critical_issues=[IssueInfo(severity="critical", label="N+1 Query", description="x")],
                warnings=[
                    IssueInfo(severity="medium", label="Large Result Set", description="y"),
                    IssueInfo(severity="medium", label="Inefficient Count", description="z"),
                ],
            )
        )

        assert record.critical_count == 1
        assert record.warning_count == 2
        assert record.hint_count == 0
        assert record.total_issues == 3
        assert record.grade_class == "bad"

        assert store.add(_make_report(grade="A")).grade_class == "good"
        assert store.add(_make_report(grade="C")).grade_class == "warn"


# ---------------------------------------------------------------------------
# Views
# ---------------------------------------------------------------------------


class TestDashboardViews:
    def setup_method(self):
        self.client = Client()

    @override_settings(DEBUG=False)
    def test_all_views_404_when_debug_off(self):
        dashboard.store.add(_make_report())
        record_id = dashboard.store.records()[0].id

        for url in (
            "/__dbcrust__/",
            "/__dbcrust__/requests/",
            f"/__dbcrust__/requests/{record_id}/",
            "/__dbcrust__/htmx.min.js",
        ):
            assert self.client.get(url).status_code == 404, url
        assert self.client.post("/__dbcrust__/clear/").status_code == 404

    def test_index_renders_shell(self):
        response = self.client.get("/__dbcrust__/")
        assert response.status_code == 200
        content = response.content.decode()
        assert "Performance Dashboard" in content
        assert "/__dbcrust__/htmx.min.js" in content
        assert "/__dbcrust__/requests/" in content

    def test_request_list_partial_shows_records(self):
        dashboard.store.add(_make_report(path="/shop/orders/", grade="C"))

        response = self.client.get("/__dbcrust__/requests/")
        assert response.status_code == 200
        content = response.content.decode()
        assert "/shop/orders/" in content
        assert ">C<" in content  # grade badge

    def test_request_list_empty_state(self):
        response = self.client.get("/__dbcrust__/requests/")
        assert "No requests recorded yet" in response.content.decode()

    def test_request_detail_renders_issues_and_slow_queries(self):
        record = dashboard.store.add(
            _make_report(
                critical_issues=[
                    IssueInfo(
                        severity="critical",
                        label="N+1 Query",
                        description="Detected 11 similar queries",
                        affected_queries_count=11,
                        recommendation="Use select_related('customer')",
                        code_suggestion="Order.objects.select_related('customer')",
                        code_locations=["shop/views.py:42"],
                    )
                ],
                slow_queries=[
                    SlowQueryInfo(
                        sql="SELECT * FROM orders WHERE customer_id = %s",
                        duration_ms=240.5,
                        tables=["orders"],
                        explain_plan_type="Seq Scan on orders",
                        explain_suggestion="Add an index on customer_id",
                    )
                ],
            )
        )

        response = self.client.get(f"/__dbcrust__/requests/{record.id}/")
        assert response.status_code == 200
        content = response.content.decode()
        assert "N+1 Query" in content
        assert "Detected 11 similar queries" in content
        assert "select_related" in content
        assert "shop/views.py:42" in content
        assert "Seq Scan on orders" in content
        assert "240.5" in content

    def test_request_detail_unknown_id_404(self):
        assert self.client.get("/__dbcrust__/requests/12345/").status_code == 404

    def test_clear_requires_post(self):
        assert self.client.get("/__dbcrust__/clear/").status_code == 405

    def test_clear_empties_store(self):
        dashboard.store.add(_make_report())

        response = self.client.post("/__dbcrust__/clear/")
        assert response.status_code == 200
        assert dashboard.store.records() == []
        assert "No requests recorded yet" in response.content.decode()

    def test_htmx_is_served(self):
        response = self.client.get("/__dbcrust__/htmx.min.js")
        assert response.status_code == 200
        assert response["Content-Type"] == "text/javascript"
        assert b"htmx" in response.content


# ---------------------------------------------------------------------------
# Middleware integration
# ---------------------------------------------------------------------------


def _run_request_through_middleware(path="/some-view/"):
    middleware = PerformanceAnalysisMiddleware(lambda request: HttpResponse())
    request = RequestFactory().get(path)
    middleware.process_request(request)
    response = middleware.process_response(request, HttpResponse())
    return request, response


class TestMiddlewareDashboardHook:
    @override_settings(DBCRUST_PERFORMANCE_ANALYSIS={"ENABLED": True})
    def test_analyzed_request_is_recorded(self):
        _run_request_through_middleware(path="/some-view/")

        records = dashboard.store.records()
        assert len(records) == 1
        assert records[0].report.path == "/some-view/"
        assert records[0].report.method == "GET"

    @override_settings(
        DBCRUST_PERFORMANCE_ANALYSIS={"ENABLED": True, "DASHBOARD_ENABLED": False}
    )
    def test_dashboard_can_be_disabled(self):
        _run_request_through_middleware()
        assert dashboard.store.records() == []

    @override_settings(DBCRUST_PERFORMANCE_ANALYSIS={"ENABLED": True})
    def test_dashboard_requests_are_not_analyzed(self):
        middleware = PerformanceAnalysisMiddleware(lambda request: HttpResponse())
        request = RequestFactory().get("/__dbcrust__/requests/")

        middleware.process_request(request)
        assert not hasattr(request, "_dbcrust_analysis")

        middleware.process_response(request, HttpResponse())
        assert dashboard.store.records() == []

    @override_settings(DBCRUST_PERFORMANCE_ANALYSIS={"ENABLED": True})
    def test_healthy_request_recorded_but_not_logged(self, caplog):
        # A 0-query request is below every logging threshold: it must land in
        # the dashboard (full timeline) without emitting a console report.
        middleware = PerformanceAnalysisMiddleware(lambda request: HttpResponse())
        caplog.set_level(logging.DEBUG, logger="dbcrust.performance")
        caplog.clear()  # drop middleware-initialisation log lines

        request = RequestFactory().get("/healthy/")
        middleware.process_request(request)
        middleware.process_response(request, HttpResponse())

        assert len(dashboard.store.records()) == 1
        report_logs = [
            r for r in caplog.records
            if r.name == "dbcrust.performance" and r.levelno >= logging.INFO
        ]
        assert report_logs == []

    @override_settings(
        DBCRUST_PERFORMANCE_ANALYSIS={"ENABLED": True, "DASHBOARD_MAX_REQUESTS": 7}
    )
    def test_buffer_size_is_configurable(self):
        _run_request_through_middleware()
        assert dashboard.store._entries.maxlen == 7
