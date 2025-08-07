# Django Middleware Integration

DBCrust provides powerful Django middleware for automatic ORM performance analysis. The middleware captures Django queries in real-time, detects N+1 query problems, and provides actionable optimization recommendations without requiring code changes.

## 🚀 Quick Start

### Basic Middleware Setup

Add DBCrust middleware to your Django project in 3 simple steps:

```python
# settings.py
INSTALLED_APPS = [
    # ... your existing apps
    'dbcrust.django',
]

# Add middleware (for development only)
if DEBUG:
    MIDDLEWARE = [
        'dbcrust.django.PerformanceAnalysisMiddleware',
        # ... your existing middleware
    ]
```

**That's it!** DBCrust now automatically analyzes all ORM queries and reports performance issues.

### Environment-Specific Configuration

```python
# settings/base.py
INSTALLED_APPS = [
    # ... your apps
    'dbcrust.django',
]

# settings/development.py
from .base import *

MIDDLEWARE = [
    'dbcrust.django.PerformanceAnalysisMiddleware',
    # ... other middleware
]

DBCRUST_ANALYSIS = {
    'ENABLED': True,
    'AUTO_REPORT': True,
    'REPORT_THRESHOLD': 5,  # Report if more than 5 queries
}

# settings/production.py
from .base import *

# Don't include middleware in production
# But keep the app for management commands
```

## 🛠️ Middleware Configuration

### Performance Analysis Settings

```python
# settings.py

DBCRUST_ANALYSIS = {
    # Enable/disable analysis
    'ENABLED': True,

    # Automatic reporting
    'AUTO_REPORT': True,           # Print reports automatically
    'REPORT_THRESHOLD': 3,         # Report if query count > threshold
    'SLOW_QUERY_THRESHOLD': 100,   # Report queries slower than 100ms

    # N+1 Detection
    'DETECT_N_PLUS_ONE': True,
    'N_PLUS_ONE_THRESHOLD': 3,     # Flag if >3 similar queries

    # Missing optimization detection
    'DETECT_MISSING_SELECT_RELATED': True,
    'DETECT_MISSING_PREFETCH_RELATED': True,
    'DETECT_LARGE_RESULT_SETS': True,
    'LARGE_RESULT_SET_THRESHOLD': 100,

    # Query pattern analysis
    'ANALYZE_QUERY_PATTERNS': True,
    'SUGGEST_INDEXES': True,
    'DETECT_INEFFICIENT_QUERIES': True,

    # Reporting options
    'REPORT_FORMAT': 'console',    # 'console', 'json', 'html'
    'INCLUDE_STACK_TRACE': True,   # Show where queries originated
    'INCLUDE_QUERY_DETAILS': True, # Show actual SQL
    'MAX_QUERIES_IN_REPORT': 10,  # Limit report size

    # Storage options
    'STORE_RESULTS': False,        # Store results in database
    'STORE_DURATION_DAYS': 7,     # How long to keep stored results
}
```

### Advanced Middleware Options

```python
# settings.py

DBCRUST_MIDDLEWARE = {
    # Middleware behavior
    'ANALYZE_ONLY_VIEWS': True,    # Only analyze view requests (not API calls)
    'SKIP_ADMIN': True,            # Skip Django admin requests
    'SKIP_STATIC': True,           # Skip static file requests

    # Request filtering
    'ANALYZE_PATHS': [             # Only analyze these URL patterns
        r'^/api/',
        r'^/dashboard/',
    ],
    'SKIP_PATHS': [                # Skip these URL patterns
        r'^/health/',
        r'^/metrics/',
    ],

    # User filtering
    'ANALYZE_USERS': ['admin', 'developer'],  # Only analyze these users
    'SKIP_ANONYMOUS': False,       # Analyze anonymous user requests

    # Performance limits
    'MAX_ANALYSIS_TIME': 1000,     # Max 1 second for analysis
    'SKIP_LONG_REQUESTS': True,    # Skip requests longer than 5 seconds
    'LONG_REQUEST_THRESHOLD': 5000,
}
```

## 📊 Real-Time Analysis Output

### Console Output Format

When `AUTO_REPORT = True`, you'll see real-time analysis:

```
🚨 DBCrust Django ORM Analysis - /books/
============================================
Request: GET /books/ (user: admin)
Duration: 2.34 seconds | Queries: 26

🔴 CRITICAL ISSUES (1):
   N+1 Query Detected:
   - Query: SELECT * FROM books_book ORDER BY created_at DESC
   - Followed by: 25x SELECT * FROM authors_author WHERE id = ?

   💡 Fix: Use select_related()
   books = Book.objects.select_related('author').all()
   Estimated improvement: 2.1s → 0.12s (94% faster)

🟡 OPTIMIZATIONS (2):
   Missing prefetch_related:
   - Model: Book → reviews (accessed 25 times)
   💡 Fix: Book.objects.prefetch_related('reviews')

   Large result set without pagination:
   - Query returned 500 rows, consider pagination
   💡 Fix: Use Django's Paginator class

📈 PERFORMANCE SUMMARY:
   Total queries: 26 (24 duplicates)
   Total time: 2.34s (2.1s in duplicates)
   Potential improvement: 94% faster with optimizations

View file: books/views.py:42
Query origins:
  → books/views.py:45 (Book.objects.all())
  → books/templates/books/book_item.html:12 ({{ book.author.name }})
```

### JSON Output Format

```python
# settings.py
DBCRUST_ANALYSIS = {
    'REPORT_FORMAT': 'json',
    'JSON_OUTPUT_FILE': '/tmp/dbcrust_analysis.json',
}
```

**JSON output structure:**
```json
{
  "request": {
    "path": "/books/",
    "method": "GET",
    "user": "admin",
    "timestamp": "2024-01-15T14:30:00Z",
    "duration_ms": 2340
  },
  "query_analysis": {
    "total_queries": 26,
    "duplicate_queries": 24,
    "total_duration_ms": 2340,
    "potential_improvement_percent": 94
  },
  "issues": [
    {
      "severity": "critical",
      "type": "n_plus_one",
      "description": "N+1 query detected accessing author.name",
      "query": "SELECT * FROM authors_author WHERE id = ?",
      "occurrence_count": 25,
      "fix_suggestion": "Use select_related('author')",
      "estimated_improvement": {
        "current_ms": 2100,
        "optimized_ms": 120,
        "improvement_percent": 94
      },
      "location": {
        "file": "books/views.py",
        "line": 45,
        "function": "book_list"
      }
    }
  ],
  "optimizations": [
    {
      "type": "missing_prefetch",
      "model": "Book",
      "relation": "reviews",
      "access_count": 25,
      "suggestion": "Use prefetch_related('reviews')"
    }
  ]
}
```

## 🎯 Advanced Usage Patterns

### Custom Analysis Rules

Create custom analysis rules for your specific needs:

```python
# myapp/dbcrust_rules.py

from dbcrust.django.analyzers import BaseAnalyzer

class CustomModelAnalyzer(BaseAnalyzer):
    """Custom analyzer for specific model patterns"""

    def analyze_queryset(self, queryset, context):
        """Analyze custom business logic patterns"""
        issues = []

        # Custom rule: Check for missing status filters
        if hasattr(queryset.model, 'status'):
            query_sql = str(queryset.query)
            if 'WHERE' not in query_sql or 'status' not in query_sql:
                issues.append({
                    'type': 'missing_status_filter',
                    'severity': 'warning',
                    'message': 'Query missing status filter - may return inactive records',
                    'suggestion': 'Add .filter(status="active") to queryset'
                })

        # Custom rule: Check for expensive aggregations
        if any(op in str(queryset.query) for op in ['COUNT(*)', 'SUM(', 'AVG(']):
            if 'GROUP BY' not in str(queryset.query):
                issues.append({
                    'type': 'expensive_aggregation',
                    'severity': 'warning',
                    'message': 'Aggregation without GROUP BY may be slow',
                    'suggestion': 'Consider adding appropriate grouping or using database views'
                })

        return issues

# Register custom analyzer
from dbcrust.django import register_analyzer
register_analyzer(CustomModelAnalyzer)
```

```python
# settings.py
DBCRUST_ANALYSIS = {
    'CUSTOM_ANALYZERS': [
        'myapp.dbcrust_rules.CustomModelAnalyzer',
    ]
}
```

### View-Specific Analysis

Analyze specific views in detail:

```python
# views.py
from dbcrust.django.decorators import analyze_performance

@analyze_performance(
    max_queries=5,
    max_duration=1000,  # 1 second
    detect_n_plus_one=True
)
def book_list(request):
    """Book list view with performance monitoring"""
    # This view will be analyzed even if middleware is disabled
    books = Book.objects.all()
    return render(request, 'books/list.html', {'books': books})

@analyze_performance(
    custom_rules=['check_pagination', 'check_caching']
)
def expensive_report(request):
    """Complex report with custom analysis rules"""
    # Complex query logic here
    return render(request, 'reports/expensive.html', context)
```

### Class-Based View Integration

```python
# views.py
from django.views.generic import ListView
from dbcrust.django.mixins import PerformanceAnalysisMixin

class BookListView(PerformanceAnalysisMixin, ListView):
    model = Book
    template_name = 'books/list.html'

    # Performance analysis settings
    performance_max_queries = 10
    performance_detect_n_plus_one = True
    performance_suggest_optimizations = True

    def get_queryset(self):
        # This queryset will be analyzed automatically
        return Book.objects.select_related('author').prefetch_related('reviews')

class AuthorDetailView(PerformanceAnalysisMixin, DetailView):
    model = Author

    # Custom performance rules for this view
    performance_custom_rules = [
        'check_related_objects',
        'check_expensive_annotations'
    ]
```

## 🔧 Integration with Development Workflow

### Pre-Commit Hooks

Catch performance issues before they reach production:

```yaml
# .pre-commit-config.yaml
repos:
  - repo: local
    hooks:
      - id: django-orm-analysis
        name: Django ORM Performance Analysis
        entry: python manage.py dbcrust_analyze_code
        language: system
        files: \.py$
        pass_filenames: true
```

```python
# management/commands/dbcrust_analyze_code.py
from django.core.management.base import BaseCommand
from dbcrust.django.static_analysis import analyze_python_files

class Command(BaseCommand):
    help = 'Analyze Python files for potential ORM performance issues'

    def add_arguments(self, parser):
        parser.add_argument('files', nargs='*', help='Python files to analyze')

    def handle(self, *args, **options):
        issues_found = False

        for file_path in options['files']:
            issues = analyze_python_files([file_path])
            if issues:
                issues_found = True
                self.stdout.write(f"\n🚨 Issues found in {file_path}:")
                for issue in issues:
                    self.stdout.write(f"  - {issue['message']} (line {issue['line']})")
                    self.stdout.write(f"    Fix: {issue['suggestion']}")

        if issues_found:
            self.stdout.write("\n💡 Run 'python manage.py dbcrust_fix_auto' to auto-fix simple issues")
            exit(1)
        else:
            self.stdout.write("✅ No ORM performance issues detected")
```

### IDE Integration

**VS Code Extension Integration:**

```json
// .vscode/settings.json
{
    "python.linting.enabled": true,
    "python.linting.pylintEnabled": false,
    "dbcrust.analysis.enabled": true,
    "dbcrust.analysis.realtime": true,
    "dbcrust.analysis.showInlineWarnings": true
}
```

**PyCharm Plugin Integration:**

```python
# PyCharm external tool configuration
# Program: python
# Arguments: manage.py dbcrust_analyze_file $FilePath$
# Working directory: $ProjectFileDir$
```

### Testing Integration

```python
# test_performance.py
from django.test import TestCase
from dbcrust.django.testing import PerformanceTestCase

class BookViewPerformanceTest(PerformanceTestCase):
    """Test view performance with DBCrust"""

    # Performance constraints
    max_queries = 3
    max_duration = 500  # 500ms
    detect_n_plus_one = True

    def setUp(self):
        # Create test data
        self.author = Author.objects.create(name="Test Author")
        self.books = [
            Book.objects.create(title=f"Book {i}", author=self.author)
            for i in range(10)
        ]

    def test_book_list_performance(self):
        """Test that book list view meets performance requirements"""
        with self.assert_performance():
            response = self.client.get('/books/')
            self.assertEqual(response.status_code, 200)

        # Performance analysis runs automatically
        # Test fails if constraints are violated

    def test_book_detail_performance(self):
        """Test book detail view performance"""
        book = self.books[0]

        with self.assert_performance(max_queries=2):
            response = self.client.get(f'/books/{book.id}/')
            self.assertEqual(response.status_code, 200)

# Run performance tests
# python manage.py test test_performance --keepdb
```

## 📈 Performance Monitoring

### Continuous Performance Monitoring

```python
# settings/production.py

# Enable lightweight monitoring in production
DBCRUST_MONITORING = {
    'ENABLED': True,
    'SAMPLE_RATE': 0.1,  # Monitor 10% of requests
    'STORE_RESULTS': True,
    'ALERT_THRESHOLDS': {
        'query_count': 20,
        'duration_ms': 5000,
        'n_plus_one_count': 3,
    },
    'ALERTING': {
        'SLACK_WEBHOOK': os.getenv('SLACK_WEBHOOK_URL'),
        'EMAIL_RECIPIENTS': ['dev-team@company.com'],
    }
}
```

### Performance Dashboard

```python
# urls.py
urlpatterns = [
    # ... your URLs
    path('dbcrust/', include('dbcrust.django.dashboard.urls')),
]
```

Access performance dashboard at `/dbcrust/dashboard/`:
- Real-time performance metrics
- N+1 query detection trends
- Slow query analysis
- Optimization recommendations
- Historical performance data

### Metrics Integration

```python
# settings.py
DBCRUST_METRICS = {
    'PROMETHEUS_ENABLED': True,
    'PROMETHEUS_PREFIX': 'dbcrust_django',
    'STATSD_ENABLED': True,
    'STATSD_HOST': 'localhost',
    'STATSD_PORT': 8125,
    'CUSTOM_METRICS': {
        'query_count': 'counter',
        'query_duration': 'histogram',
        'n_plus_one_detected': 'counter',
    }
}
```

**Prometheus metrics exposed:**
```
# HELP dbcrust_django_queries_total Total number of queries
# TYPE dbcrust_django_queries_total counter
dbcrust_django_queries_total{view="book_list",method="GET"} 156

# HELP dbcrust_django_query_duration_seconds Query duration
# TYPE dbcrust_django_query_duration_seconds histogram
dbcrust_django_query_duration_seconds_bucket{view="book_list",le="0.1"} 45
dbcrust_django_query_duration_seconds_bucket{view="book_list",le="0.5"} 120

# HELP dbcrust_django_n_plus_one_total N+1 queries detected
# TYPE dbcrust_django_n_plus_one_total counter
dbcrust_django_n_plus_one_total{view="book_list"} 3
```

## 🚨 Troubleshooting

### Common Issues

**Middleware not running:**
```python
# Check middleware is properly installed
python manage.py shell
>>> from django.conf import settings
>>> 'dbcrust.django.PerformanceAnalysisMiddleware' in settings.MIDDLEWARE
True

# Check DEBUG mode is enabled
>>> settings.DEBUG
True
```

**No analysis output:**
```python
# Check configuration
DBCRUST_ANALYSIS = {
    'ENABLED': True,
    'AUTO_REPORT': True,  # Must be True for console output
}

# Check logging configuration
LOGGING = {
    'loggers': {
        'dbcrust.django': {
            'level': 'DEBUG',
            'handlers': ['console'],
        }
    }
}
```

**Performance impact:**
```python
# Reduce middleware overhead
DBCRUST_MIDDLEWARE = {
    'SKIP_ADMIN': True,          # Skip admin pages
    'SKIP_STATIC': True,         # Skip static files
    'ANALYZE_ONLY_VIEWS': True,  # Only analyze views
    'MAX_ANALYSIS_TIME': 100,    # Limit analysis time
}
```

### Debug Mode

Enable detailed debugging:

```python
# settings.py
LOGGING = {
    'version': 1,
    'disable_existing_loggers': False,
    'handlers': {
        'console': {
            'class': 'logging.StreamHandler',
        },
    },
    'loggers': {
        'dbcrust.django': {
            'handlers': ['console'],
            'level': 'DEBUG',
            'propagate': False,
        },
    },
}

# Enable debug mode
DBCRUST_DEBUG = True
```

## 📚 See Also

- **[Django Management Commands](/dbcrust/django/management-commands/)** - CLI tools for Django
- **[CI/CD Integration](/dbcrust/django/ci-integration/)** - Automated performance testing
- **[Team Workflows](/dbcrust/django/team-workflows/)** - Collaborative performance optimization
- **[Django ORM Analyzer](/dbcrust/django-analyzer/)** - Complete analyzer documentation

---

<div align="center">
    <strong>Ready to optimize your Django ORM performance?</strong><br>
    <a href="/dbcrust/django/management-commands/" class="md-button md-button--primary">Management Commands</a>
    <a href="/dbcrust/django-analyzer/" class="md-button">Complete Django Guide</a>
</div>
