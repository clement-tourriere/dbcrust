---
title: Performance Dashboard
description: Local web dashboard for browsing per-request ORM analysis — N+1s, slow queries, and recommendations
---

The middleware's per-request analysis also feeds a **local web dashboard**: a live, self-refreshing view of every analyzed request with its grade, query metrics, detected issues (N+1, missing `select_related`, …), recommendations, and slow queries with EXPLAIN insights.

It is a development tool in the spirit of django-debug-toolbar, but request-history oriented: browse your app in one tab, watch the timeline fill up in another, click any request to see exactly what to fix and where (`file:line` locations included).

## Setup

Three lines on top of the [middleware](/dbcrust/django/middleware/) you already have:

```python
# settings.py
INSTALLED_APPS = [
    # ...
    'dbcrust.django',          # template discovery for the dashboard
]

MIDDLEWARE = [
    'dbcrust.django.PerformanceAnalysisMiddleware',
    # ...
]
```

```python
# urls.py
from django.conf import settings
from django.urls import include, path

if settings.DEBUG:
    urlpatterns += [path('__dbcrust__/', include('dbcrust.django.urls'))]
```

Open <http://localhost:8000/__dbcrust__/> and browse your app — requests appear as they happen.

Any prefix works; `__dbcrust__` is just a convention. No `staticfiles` configuration, build step, or CDN is needed: the UI is plain Django templates driven by a vendored [htmx](https://htmx.org) (the request list polls every 2 seconds).

## What you see

**Request list** (newest first) — grade badge (A–F), time, method + path, query count, DB time, request time, and issue counts split by severity. Header stats aggregate the buffer: total requests, requests with issues, average queries, and average DB time.

**Detail pane** — click a request:

- Metrics: queries, DB time, request time, duplicates, and the SELECT/INSERT/UPDATE/DELETE breakdown
- **Critical / Warnings / Hints** — each detected pattern with its description, a concrete recommendation (e.g. `select_related('author')`), a code suggestion, and clickable `file:line` locations
- **Slow queries** — SQL, duration, tables, and EXPLAIN insights (plan type, rows examined, suggested fix) when the [EXPLAIN integration](/dbcrust/django/middleware/#how-explain-works) is active

The dashboard's own polling requests are recognized (by URL namespace) and excluded from analysis, so it never pollutes its own data.

## Configuration

All dashboard keys in `DBCRUST_PERFORMANCE_ANALYSIS` (defaults shown):

```python
DBCRUST_PERFORMANCE_ANALYSIS = {
    'DASHBOARD_ENABLED': True,      # record analyzed requests for the dashboard
    'DASHBOARD_MAX_REQUESTS': 100,  # history size (oldest pruned first)
    'DASHBOARD_PERSIST': True,      # keep history across restarts (SQLite file)
    'DASHBOARD_DB_PATH': None,      # None → BASE_DIR/.dbcrust/dashboard.sqlite3
}
```

Unlike console reports — which only fire on issues or threshold breaches — the dashboard records **every** analyzed request, healthy ones included, so the timeline is complete.

## Storage

History is persisted by default to a dedicated SQLite file at `BASE_DIR/.dbcrust/dashboard.sqlite3`, so it **survives `runserver` autoreloads and restarts** — fix the N+1 the dashboard showed you, save, and the "before" requests are still there to compare against. Add the directory to your project's `.gitignore`:

```gitignore
.dbcrust/
```

Worth knowing:

- This is **not your project database** — no models, no migrations, no `DATABASES` entry. It's a self-managed file (WAL mode, capped at `DASHBOARD_MAX_REQUESTS`, schema migrated by drop-and-recreate since history is disposable).
- **Multi-process servers work**: gunicorn workers all write to the same file, so the dashboard shows one combined timeline.
- Nothing is ever sent anywhere; delete the file (or click **Clear**) to wipe history.
- Prefer zero filesystem footprint? Set `'DASHBOARD_PERSIST': False` to use a per-process in-memory ring buffer instead (history dies with the process).

## Security

- **DEBUG-only**: every dashboard view returns 404 when `settings.DEBUG` is off. The dashboard exposes raw SQL and code paths; keep the `if settings.DEBUG:` guard around the URL include as a second layer.

## Troubleshooting

:::danger[TemplateDoesNotExist: dbcrust/dashboard.html]
Add `'dbcrust.django'` to `INSTALLED_APPS` — the templates are discovered through Django's app loader.
:::

:::danger[404 on /__dbcrust__/]
The dashboard 404s by design when `DEBUG = False`. Check your settings.
:::

:::danger[Requests don't appear]
The middleware must be active: it is enabled when `DEBUG=True` (or `'ENABLED': True` explicitly). Note that another tool's profiling panel can disable it — see [Debug Toolbar compatibility](/dbcrust/django/middleware/).
:::
