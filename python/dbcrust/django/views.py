"""
DEBUG-only views for the DBCrust performance dashboard.

Mount them with::

    # urls.py
    if settings.DEBUG:
        urlpatterns += [path('__dbcrust__/', include('dbcrust.django.urls'))]

Every view 404s when ``DEBUG`` is off (same policy as django-debug-toolbar):
the dashboard exposes raw SQL and code paths and must never reach production.
htmx is vendored and served by :func:`htmx_js`, so the dashboard needs no
``staticfiles`` setup, no CDN, and works offline.
"""

import functools
from pathlib import Path

from django.conf import settings
from django.http import Http404, HttpResponse, HttpResponseNotAllowed
from django.shortcuts import render

from . import dashboard

_HTMX_PATH = Path(__file__).parent / "static" / "dbcrust" / "htmx.min.js"


def _debug_only(view):
    """404 unless settings.DEBUG — the dashboard is a development tool."""

    @functools.wraps(view)
    def wrapped(request, *args, **kwargs):
        if not settings.DEBUG:
            raise Http404
        return view(request, *args, **kwargs)

    return wrapped


def _list_context():
    store = dashboard.get_store()
    return {
        "records": store.records(),
        "stats": store.stats(),
    }


@_debug_only
def index(request):
    """Dashboard shell: header, stats, polling request list, detail pane."""
    return render(request, "dbcrust/dashboard.html", _list_context())


@_debug_only
def request_list(request):
    """htmx partial polled by the dashboard: stats + request table."""
    return render(request, "dbcrust/_request_list.html", _list_context())


@_debug_only
def request_detail(request, record_id):
    """htmx partial: issues, recommendations, and slow queries for one request."""
    record = dashboard.get_store().get(record_id)
    if record is None:
        raise Http404
    return render(request, "dbcrust/_request_detail.html", {"record": record})


@_debug_only
def clear(request):
    """Empty the ring buffer and return the refreshed list partial."""
    if request.method != "POST":
        return HttpResponseNotAllowed(["POST"])
    dashboard.get_store().clear()
    return render(request, "dbcrust/_request_list.html", _list_context())


@functools.lru_cache(maxsize=1)
def _htmx_source() -> bytes:
    return _HTMX_PATH.read_bytes()


@_debug_only
def htmx_js(request):
    """Serve the vendored htmx build (no staticfiles dependency)."""
    response = HttpResponse(_htmx_source(), content_type="text/javascript")
    response["Cache-Control"] = "public, max-age=86400"
    return response
