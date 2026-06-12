---
title: "Django Middleware Integration"
---

# Django Middleware Integration

DBCrust ships a Django middleware for automatic ORM performance analysis. It
captures every query of a request, detects N+1 patterns and other
inefficiencies, EXPLAINs the slowest SELECTs **on Django's own database
connection**, and logs one consolidated report per request.

## 🚀 Quick Start

```python
# settings.py
INSTALLED_APPS = [
    # ... your existing apps
    'dbcrust.django',
]

# Development only — the middleware is for finding problems, not production
if DEBUG:
    MIDDLEWARE = [
        'dbcrust.django.PerformanceAnalysisMiddleware',
        # ... your existing middleware
    ]
```

That's it. Each analyzed request logs a consolidated report to the
`dbcrust.performance` logger when it detects issues or crosses the
thresholds below.

## 🛠️ Configuration

All options live under a single setting, `DBCRUST_PERFORMANCE_ANALYSIS`.
The values below are the defaults:

```python
# settings.py
DBCRUST_PERFORMANCE_ANALYSIS = {
    # Core
    'ENABLED': None,                  # None → follow settings.DEBUG
    'QUERY_THRESHOLD': 10,            # report when a request runs more queries
    'TIME_THRESHOLD': 100,            # …or spends more DB time (milliseconds)
    'LOG_ALL_REQUESTS': False,        # True → report every request

    # EXPLAIN (runs on Django's own connection, params bound by the driver)
    'EXPLAIN_ENABLED': True,
    'EXPLAIN_SLOW_THRESHOLD_MS': 100, # SELECTs slower than this get EXPLAINed
    'EXPLAIN_MAX_QUERIES': 5,         # cap per request
    'EXPLAIN_ANALYZE': False,         # True → EXPLAIN ANALYZE (re-executes
                                      # the slow SELECTs to get actual
                                      # rows/timings; plans only by default)

    # Display
    'INCLUDE_HEADERS': True,          # X-DBCrust-* response headers

    # Dashboard (see the Dashboard page)
    'DASHBOARD_ENABLED': True,        # record requests for the web dashboard
    'DASHBOARD_MAX_REQUESTS': 100,    # history size (oldest pruned first)
    'DASHBOARD_PERSIST': True,        # survive restarts (SQLite file)
    'DASHBOARD_DB_PATH': None,        # None → BASE_DIR/.dbcrust/dashboard.sqlite3

    # Advanced
    'TRANSACTION_SAFE': False,        # WARNING: True rolls back ALL writes
                                      # made during every analyzed request —
                                      # never enable outside throwaway
                                      # experiments
    'DEBUG_TOOLBAR_COMPATIBILITY': True,  # auto-disable when the Debug
                                          # Toolbar profiling panel is active
}
```

Unknown or deprecated keys log a one-time warning with the list of valid
keys — typos won't fail silently.

### How EXPLAIN works

For PostgreSQL, MySQL, and SQLite, slow SELECTs are EXPLAINed through the
same connection alias Django used (`EXPLAIN (FORMAT JSON)`,
`EXPLAIN FORMAT=JSON`, `EXPLAIN QUERY PLAN` respectively). Captured query
parameters are bound by the driver, so parameterized queries work exactly
as they executed. With the default `EXPLAIN_ANALYZE: False` the statement
is planned but **never re-executed**.

Unsupported vendors automatically fall back to heuristic-only analysis
(SQL-text inspection: leading-wildcard LIKE, functions on columns in
WHERE, missing indexes…).

## 📋 The consolidated report

One log record per analyzed request on the `dbcrust.performance` logger
(INFO for healthy requests when `LOG_ALL_REQUESTS` is on, WARNING when
issues or bad grades are detected):

```
GET /books/ (orders:book-list) — Grade C
26 queries · 312ms DB · 488ms request
🔴 N+1 Query: 24 similar queries on books_author — use select_related('author')
   books/views.py:42 in book_list
Slow query (118ms): SELECT … FROM books_book WHERE …
   ↳ Seq Scan on books_book — add db_index=True for the filtered fields
```

Response headers (when `INCLUDE_HEADERS` is on) expose
`X-DBCrust-Query-Count`, `X-DBCrust-Query-Time`, and
`X-DBCrust-Warning` for quick inspection from the browser or curl.

Prefer a UI? The same analysis feeds a local web dashboard — see
[Dashboard](/dbcrust/django/dashboard/).

## 🧪 Query budgets in tests and CI

The same analysis engine powers test assertions — fail the build on a
query regression instead of finding it in production:

```python
from dbcrust.django.testing import assert_max_queries, assert_no_n_plus_one

def test_dashboard(client):
    with assert_max_queries(10):
        client.get("/dashboard/")

def test_book_list():
    with assert_no_n_plus_one():
        for book in Book.objects.select_related("author"):
            _ = book.author.name
```

Both raise `AssertionError` subclasses with a summary of the offending
query shapes. With pytest, the auto-registered `dbcrust` fixture provides
the same helpers pre-bound:

```python
def test_dashboard(client, dbcrust):
    with dbcrust.max_queries(10):
        client.get("/dashboard/")
```

## 🔍 Standalone analysis (without the middleware)

```python
from dbcrust.django.analyzer import analyze

with analyze() as analysis:
    books = Book.objects.filter(published=True)
    for book in books:
        print(book.author.name)  # potential N+1

results = analysis.get_results()
print(results.summary)
```

The context manager collects queries only inside the block and never
wraps your code in a transaction unless you opt in
(`analyze(transaction_safe=True)` — which **rolls back every write** in
the block; reserved for throwaway experiments).

## 🤝 Django Debug Toolbar

When the Debug Toolbar's profiling panel is active, the middleware
disables itself to avoid double instrumentation (set
`DEBUG_TOOLBAR_COMPATIBILITY: False` or `ENABLED: True` explicitly to
override).

## 🏭 Production

Don't run the middleware in production: it adds per-query overhead
(stack capture) and is built for development feedback. Keep
`'dbcrust.django'` in `INSTALLED_APPS` if you use the management
commands, and gate the middleware on `DEBUG` as shown above.
