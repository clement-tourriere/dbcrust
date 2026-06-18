"""Tests for the Django AI context builder (pure Python, no native module)."""

import os
import threading
from unittest.mock import patch

from django.test import Client
from django.urls import reverse

from dbcrust.django import dashboard
from dbcrust.django.ai_context import build_django_context, summarize_report
from dbcrust.django.project_analyzer import DjangoModel
from dbcrust.django.report_formatter import (
    IssueInfo,
    RequestPerformanceReport,
    SlowQueryInfo,
)


def _order_model():
    return DjangoModel(
        name="Order",
        file_path="shop/models.py",
        line_number=12,
        fields={"id": "AutoField", "total": "DecimalField", "created": "DateTimeField"},
        foreign_keys={"customer": "Customer"},
        many_to_many={"tags": "Tag"},
        indexes=["created"],
        meta_options={"db_table": "shop_order"},
    )


def test_build_django_context_renders_model():
    ctx = build_django_context([_order_model()])
    assert "shop/models.py:12" in ctx
    assert "class Order (db_table=shop_order)" in ctx
    assert "customer -> Customer" in ctx
    assert "tags -> Tag" in ctx
    # db_table -> Model map ties SQL tables back to models
    assert "shop_order=Order" in ctx


def test_build_django_context_table_filter_scopes_models():
    order = _order_model()
    other = DjangoModel(
        name="AuditLog",
        file_path="audit/models.py",
        line_number=5,
        meta_options={"db_table": "audit_log"},
    )
    ctx = build_django_context([order, other], tables=["shop_order"])
    assert "class Order" in ctx
    assert "AuditLog" not in ctx


def test_build_django_context_includes_captured_queries():
    class FakeQuery:
        sql = "SELECT * FROM shop_order WHERE customer_id = 1"
        duration = 0.0123
        stack_trace = ["shop/views.py:42 in order_list", "framework frame"]
        table_names = ["shop_order"]

    ctx = build_django_context([_order_model()], queries=[FakeQuery()])
    assert "CAPTURED QUERIES" in ctx
    assert "shop/views.py:42" in ctx
    assert "12.3 ms" in ctx


def test_build_django_context_empty_models_is_empty_string():
    assert build_django_context([]) == ""


def test_django_model_default_table_name_uses_app_label():
    # Django's default is "<app_label>_<modelname_lowercased>" — NOT snake_cased.
    m = DjangoModel(name="OrderLine", file_path="shop/models.py", line_number=3, app_label="shop")
    assert m.table_name == "shop_orderline"


def test_django_model_table_name_honors_meta():
    explicit = DjangoModel(
        name="Order", file_path="x/models.py", line_number=1,
        meta_options={"db_table": "custom_orders"},
    )
    assert explicit.table_name == "custom_orders"
    labelled = DjangoModel(
        name="Order", file_path="x/models.py", line_number=1, app_label="shop",
        meta_options={"app_label": "billing"},
    )
    assert labelled.table_name == "billing_order"


def test_build_django_context_filters_default_table_names():
    # A model with a DEFAULT table name (no Meta.db_table) must still match the
    # real table name from a captured query, e.g. shop.Order -> shop_order.
    order = DjangoModel(name="Order", file_path="shop/models.py", line_number=1, app_label="shop")
    other = DjangoModel(name="AuditLog", file_path="audit/models.py", line_number=1, app_label="audit")
    ctx = build_django_context([order, other], tables=["shop_order"])
    assert "class Order" in ctx
    assert "AuditLog" not in ctx
    assert "shop_order=Order" in ctx


def _report_with_issues():
    return RequestPerformanceReport(
        method="GET",
        path="/orders/",
        view_name="shop.views.order_list",
        slow_queries=[
            SlowQueryInfo(sql="SELECT * FROM shop_order", duration_ms=42.5, tables=["shop_order"])
        ],
        critical_issues=[
            IssueInfo(
                severity="critical",
                label="N+1 Query",
                description="customer fetched per order",
                recommendation="use select_related('customer')",
                code_locations=["shop/views.py:42"],
            )
        ],
    )


def test_summarize_report_includes_queries_and_issues():
    text = summarize_report(_report_with_issues())
    assert "GET /orders/" in text
    assert "shop.views.order_list" in text
    assert "42.5 ms" in text
    assert "N+1 Query" in text
    assert "select_related('customer')" in text
    assert "shop/views.py:42" in text


def test_dbcrust_config_dir_setting_sets_env(monkeypatch, tmp_path):
    from django.conf import settings

    from dbcrust.django.ai_context import _apply_dbcrust_config_dir_setting

    monkeypatch.delenv("DBCRUST_CONFIG_DIR", raising=False)
    monkeypatch.setattr(settings, "DBCRUST_CONFIG_DIR", tmp_path, raising=False)

    _apply_dbcrust_config_dir_setting()

    assert os.environ["DBCRUST_CONFIG_DIR"] == str(tmp_path)


def test_dashboard_ai_investigate_starts_job_and_reports_failure():
    # POST kicks off a background job; when the investigation raises, ai-status
    # renders a friendly error and stops polling — never a 500. `investigate_report`
    # is patched so the test is deterministic regardless of the native build state.
    from dbcrust.django.ai_jobs import get_job_store

    store = dashboard.get_store()
    store.clear()
    record = store.add(_report_with_issues())

    client = Client()
    with patch(
        "dbcrust.django.ai_context.investigate_report",
        side_effect=RuntimeError("ai-unavailable-in-test"),
    ):
        start = client.post(reverse("dbcrust:ai-investigate", args=[record.id]))
        assert start.status_code == 200

        job = get_job_store().get(record.id)
        assert job is not None
        if job.thread:
            job.thread.join(timeout=5)

        status = client.get(reverse("dbcrust:ai-status", args=[record.id]))

    assert status.status_code == 200
    assert b"AI investigation failed" in status.content
    assert b"ai-unavailable-in-test" in status.content
    # Terminal state must NOT keep polling.
    assert b"hx-trigger" not in status.content


def test_dashboard_ai_investigate_streams_progress_and_answer():
    # End-to-end of the async flow with a fake investigation: the runner writes
    # progress to the file and returns an answer. The status panel shows the live
    # trace while running, then the final answer with polling stopped.
    from dbcrust.django.ai_jobs import get_job_store

    store = dashboard.get_store()
    store.clear()
    record = store.add(_report_with_issues())

    def fake_investigate(report, *, progress_path=None, **kwargs):
        with open(progress_path, "w", encoding="utf-8") as fh:
            fh.write("🔧 describe_table: shop_order\n📊 1 rows × 2 cols\n")
        return "## Finding\nMissing index on customer_id.\n## Recommendation\nCREATE INDEX ..."

    client = Client()
    with patch(
        "dbcrust.django.ai_context.investigate_report", side_effect=fake_investigate
    ):
        client.post(reverse("dbcrust:ai-investigate", args=[record.id]))
        job = get_job_store().get(record.id)
        assert job is not None
        if job.thread:
            job.thread.join(timeout=5)
        status = client.get(reverse("dbcrust:ai-status", args=[record.id]))

    assert b"Missing index on customer_id" in status.content
    assert b"describe_table: shop_order" in status.content  # trace preserved
    assert b"hx-trigger" not in status.content  # stopped polling


def test_dashboard_ai_status_running_panel_polls():
    # A running job renders the live panel with the htmx poll trigger. Injected
    # directly (no real thread) so it's deterministic and native-module-free.
    import tempfile

    from dbcrust.django import ai_jobs

    store = dashboard.get_store()
    store.clear()
    record = store.add(_report_with_issues())

    fd, path = tempfile.mkstemp()
    os.close(fd)
    job_store = ai_jobs.get_job_store()
    with job_store._lock:
        job_store._jobs[record.id] = ai_jobs.AiJob(
            key=record.id, progress_path=path, status="running"
        )

    response = Client().get(reverse("dbcrust:ai-status", args=[record.id]))
    assert response.status_code == 200
    assert b"investigating" in response.content
    assert b"hx-trigger" in response.content
    assert b"ai/status/" in response.content


def test_dashboard_ai_status_unknown_record_404s():
    response = Client().get(reverse("dbcrust:ai-status", args=[999999]))
    assert response.status_code == 404


def test_ask_ai_is_silent_by_default_and_opts_into_stdout():
    # ask_ai must default to NO stdout progress (it returns a string); the
    # trailing run_ai_investigation arg is the stdout_progress flag.
    import dbcrust._internal as internal

    from dbcrust.django.ai_context import ask_ai

    with patch.object(internal, "run_ai_investigation", return_value="answer") as m:
        ask_ai("q")
        assert m.call_args.args[-1] is False  # silent by default

        ask_ai("q", stdout_progress=True)
        assert m.call_args.args[-1] is True   # opt-in


def test_dashboard_ai_investigate_requires_post():
    store = dashboard.get_store()
    store.clear()
    record = store.add(_report_with_issues())
    response = Client().get(reverse("dbcrust:ai-investigate", args=[record.id]))
    assert response.status_code == 405


def test_ai_job_store_caps_concurrent_running():
    from dbcrust.django import ai_jobs

    store = ai_jobs.AiJobStore()
    release = threading.Event()

    def blocking(_path):
        release.wait(timeout=5)
        return "ok"

    started = [store.start(k, blocking) for k in range(ai_jobs.MAX_RUNNING)]
    assert all(j.status == "running" for j in started)

    overflow = store.start(9999, blocking)
    assert overflow.status == "error"
    assert "Too many" in overflow.error
    assert overflow.thread is None

    release.set()
    for j in started:
        if j.thread:
            j.thread.join(timeout=5)


def test_ai_job_store_clear_keeps_running_and_unlinks_finished():
    from dbcrust.django import ai_jobs

    store = ai_jobs.AiJobStore()
    done = store.start(1, lambda _p: "ok")
    if done.thread:
        done.thread.join(timeout=5)
    assert store.get(1).status == "done"
    finished_path = done.progress_path
    assert os.path.exists(finished_path)

    release = threading.Event()
    running = store.start(2, lambda _p: release.wait(timeout=5))
    store.clear()
    assert store.get(1) is None  # finished dropped
    assert not os.path.exists(finished_path)  # temp file removed
    assert store.get(2) is not None  # running preserved

    release.set()
    if running.thread:
        running.thread.join(timeout=5)


def test_ai_job_store_prunes_finished_over_budget():
    from dbcrust.django import ai_jobs

    store = ai_jobs.AiJobStore()
    for k in range(ai_jobs.MAX_JOBS + 5):
        job = store.start(k, lambda _p: "ok")
        if job.thread:
            job.thread.join(timeout=5)
    assert len(store._jobs) <= ai_jobs.MAX_JOBS


def test_ai_job_store_overflow_unlinks_replaced_finished_job():
    # Retrying a finished job while at the concurrency cap must still unlink the
    # finished job's temp file instead of orphaning it.
    from dbcrust.django import ai_jobs

    store = ai_jobs.AiJobStore()
    done = store.start(0, lambda _p: "ok")
    if done.thread:
        done.thread.join(timeout=5)
    old_path = done.progress_path
    assert os.path.exists(old_path)

    release = threading.Event()
    blockers = [
        store.start(100 + k, lambda _p: release.wait(timeout=5))
        for k in range(ai_jobs.MAX_RUNNING)
    ]

    overflow = store.start(0, lambda _p: "ok")  # at capacity -> overflow branch
    assert overflow.status == "error"
    assert not os.path.exists(old_path)  # replaced finished job's file removed

    release.set()
    for j in blockers:
        if j.thread:
            j.thread.join(timeout=5)
