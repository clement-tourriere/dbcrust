# Django Management Commands

DBCrust provides seamless integration with Django's management command system, allowing you to access your Django databases using DBCrust's advanced features directly through familiar Django workflows. This guide covers all Django-specific commands and integration patterns.

## 🚀 Quick Start

### Basic Django Integration

Connect to your Django databases using the same settings Django uses:

```bash
# Connect to default database
cd your_django_project/
python manage.py dbcrust

# Connect to specific database from DATABASES setting
python manage.py dbcrust --database analytics
python manage.py dbcrust --database users
python manage.py dbcrust --database cache
```

**That's it!** DBCrust automatically reads your Django database configuration and connects with the same credentials Django uses.

## 📋 Available Commands

### Core Database Commands

#### `python manage.py dbcrust`
Launch DBCrust interactive session using Django database settings.

```bash
# Basic usage
python manage.py dbcrust                    # Default database
python manage.py dbcrust --database users  # Specific database

# With additional options
python manage.py dbcrust --debug           # Enable debug output
python manage.py dbcrust --no-banner       # Skip startup banner
python manage.py dbcrust --read-only       # Read-only connection
```

**Options:**
- `--database DB`: Use specific database from Django settings
- `--debug`: Enable debug logging
- `--no-banner`: Skip DBCrust startup banner
- `--read-only`: Open read-only connection
- `--sql-timeout N`: Set query timeout in seconds

#### `python manage.py dbcrust_analyze`
Analyze Django ORM performance across your application.

```bash
# Analyze all models and views
python manage.py dbcrust_analyze

# Analyze specific app
python manage.py dbcrust_analyze --app myapp

# Analyze specific models
python manage.py dbcrust_analyze --models User,Order,Product

# Generate detailed report
python manage.py dbcrust_analyze --report detailed --output report.html
```

**Options:**
- `--app APP`: Analyze specific Django app
- `--models MODELS`: Comma-separated list of models to analyze
- `--views`: Include view analysis
- `--report FORMAT`: Report format (summary, detailed, json)
- `--output FILE`: Save report to file
- `--slow-queries`: Focus on slow query detection
- `--n-plus-one`: Focus on N+1 query detection

#### `python manage.py dbcrust_migrate_check`
Verify migration performance and detect potential issues.

```bash
# Check pending migrations
python manage.py dbcrust_migrate_check

# Analyze migration performance impact
python manage.py dbcrust_migrate_check --analyze-impact

# Check for dangerous operations
python manage.py dbcrust_migrate_check --check-safety
```

### Performance Analysis Commands

#### `python manage.py dbcrust_profile_views`
Profile Django views for database performance.

```bash
# Profile all views
python manage.py dbcrust_profile_views

# Profile specific views
python manage.py dbcrust_profile_views --views book_list,author_detail

# Run with test data
python manage.py dbcrust_profile_views --use-fixtures
```

**Example output:**
```
📊 Django View Performance Profile
==================================

book_list (/books/):
  ✅ Queries: 3 (good)
  ⚠️  Duration: 1.2s (slow)
  🔴 N+1 detected: author.name accessed 50 times
  💡 Fix: Book.objects.select_related('author')

author_detail (/authors/{id}/):
  ✅ Queries: 2 (good)
  ✅ Duration: 45ms (fast)
  💡 Consider: prefetch_related('books') for related books
```

#### `python manage.py dbcrust_model_stats`
Generate comprehensive model usage statistics.

```bash
# Statistics for all models
python manage.py dbcrust_model_stats

# Specific models
python manage.py dbcrust_model_stats --models User,Order

# Include relationship analysis
python manage.py dbcrust_model_stats --include-relations
```

### Code Analysis Commands

#### `python manage.py dbcrust_analyze_code`
Static analysis of Python code for ORM performance issues.

```bash
# Analyze entire codebase
python manage.py dbcrust_analyze_code

# Analyze specific files
python manage.py dbcrust_analyze_code --files views.py,models.py

# Auto-fix simple issues
python manage.py dbcrust_analyze_code --auto-fix
```

**Detects:**
- Missing `select_related()` calls
- Missing `prefetch_related()` calls
- Queries in loops
- Inefficient filter patterns
- Missing database indexes

#### `python manage.py dbcrust_generate_optimizations`
Generate optimization suggestions for Django models and queries.

```bash
# Generate optimizations for all models
python manage.py dbcrust_generate_optimizations

# Focus on specific issues
python manage.py dbcrust_generate_optimizations --focus n-plus-one
python manage.py dbcrust_generate_optimizations --focus indexes
python manage.py dbcrust_generate_optimizations --focus queries

# Generate code patches
python manage.py dbcrust_generate_optimizations --generate-patches
```

## 🔧 Configuration

### Django Settings Integration

DBCrust automatically integrates with your Django database configuration:

```python
# settings.py

# Standard Django database config
DATABASES = {
    'default': {
        'ENGINE': 'django.db.backends.postgresql',
        'NAME': 'myapp_prod',
        'USER': 'myapp',
        'PASSWORD': os.getenv('DB_PASSWORD'),
        'HOST': 'db.company.com',
        'PORT': '5432',
    },
    'analytics': {
        'ENGINE': 'django.db.backends.postgresql',
        'NAME': 'analytics',
        'USER': 'analyst',
        'PASSWORD': os.getenv('ANALYTICS_DB_PASSWORD'),
        'HOST': 'analytics.company.com',
        'PORT': '5432',
    },
    'cache': {
        'ENGINE': 'django.db.backends.sqlite3',
        'NAME': BASE_DIR / 'cache.sqlite3',
    }
}

# DBCrust-specific settings (optional)
DBCRUST = {
    # Default database for management commands
    'DEFAULT_DATABASE': 'default',

    # Analysis settings
    'ANALYSIS': {
        'ENABLED': True,
        'AUTO_ANALYZE_MIGRATIONS': True,
        'DETECT_N_PLUS_ONE': True,
        'SUGGEST_INDEXES': True,
    },

    # Command-specific settings
    'COMMANDS': {
        'dbcrust': {
            'AUTO_CONNECT': True,
            'SHOW_BANNER': True,
            'READ_ONLY_DEFAULT': False,
        },
        'dbcrust_analyze': {
            'DEFAULT_FORMAT': 'detailed',
            'INCLUDE_STACK_TRACES': True,
        }
    }
}
```

### Environment-Specific Commands

```python
# settings/production.py
DBCRUST = {
    'COMMANDS': {
        'dbcrust': {
            'READ_ONLY_DEFAULT': True,  # Default to read-only in production
            'REQUIRE_CONFIRMATION': True,  # Confirm destructive operations
        }
    }
}

# settings/development.py
DBCRUST = {
    'ANALYSIS': {
        'AUTO_ANALYZE_MIGRATIONS': True,
        'REAL_TIME_ANALYSIS': True,
    }
}
```

## 🎯 Django-Specific Features

### Model Introspection

DBCrust understands Django models and provides enhanced introspection:

```sql
-- Inside DBCrust session via `python manage.py dbcrust`

-- List Django models (not just tables)
\dt
-- Shows: auth_user, auth_group, myapp_book, myapp_author, etc.

-- Describe Django model with relationship info
\d auth_user
-- Shows fields with Django field types and relationships

-- Django-specific table analysis
\d myapp_book
-- Shows:
-- - Django field types (CharField, ForeignKey, etc.)
-- - Relationship information
-- - Custom model methods
-- - Admin integration status
```

### Migration Analysis

Analyze Django migrations before applying:

```bash
# Check migration safety
python manage.py dbcrust_migrate_check

# Analyze specific migration
python manage.py dbcrust_migrate_check --migration 0023_add_index

# Performance impact analysis
python manage.py dbcrust_migrate_check --analyze-impact --database default
```

**Example output:**
```
🔍 Django Migration Analysis
============================

Pending migrations for 'myapp':
  0023_add_index: Adding index to User.email
  0024_alter_book_title: Changing Book.title max_length

Analysis Results:

0023_add_index:
  ✅ Safe operation
  ⏱️  Estimated time: 2.3s (User table has 50K rows)
  🔒 Locking: Minimal (CONCURRENT index creation)
  💡 Recommendation: Apply during low traffic

0024_alter_book_title:
  ⚠️  Requires table rewrite
  ⏱️  Estimated time: 45s (Book table has 500K rows)
  🔒 Locking: Full table lock
  ⚠️  Recommendation: Apply during maintenance window
```

### ORM Query Pattern Detection

Automatically detect common Django ORM anti-patterns:

```python
# This command analyzes your codebase
python manage.py dbcrust_analyze_code --patterns django-orm
```

**Detected patterns:**
```python
# ❌ N+1 Query Pattern Detected
# File: views.py, Line: 45
books = Book.objects.all()
for book in books:
    print(book.author.name)  # Triggers N+1

# 💡 Suggested fix:
books = Book.objects.select_related('author').all()

# ❌ Query in Template Loop Detected
# File: templates/books/list.html, Line: 12
{% for book in books %}
    {{ book.reviews.count }}  {# Query per book #}
{% endfor %}

# 💡 Suggested fix:
books = Book.objects.prefetch_related('reviews').all()

# ❌ Missing Database Index Detected
# File: models.py, Line: 23
class Book(models.Model):
    isbn = models.CharField(max_length=13)  # Frequently filtered, no index

# 💡 Suggested fix:
class Book(models.Model):
    isbn = models.CharField(max_length=13, db_index=True)
```

## 🔍 Advanced Analysis Features

### Custom Analysis Rules

Create Django-specific analysis rules:

```python
# myapp/dbcrust_django_rules.py

from dbcrust.django.analyzers import DjangoAnalyzer

class CustomDjangoAnalyzer(DjangoAnalyzer):
    """Custom Django ORM analysis rules"""

    def analyze_queryset_patterns(self, view_name, querysets):
        """Analyze Django-specific patterns"""
        issues = []

        for qs in querysets:
            # Check for missing pagination on list views
            if 'list' in view_name.lower():
                if not hasattr(qs, 'paginator') and qs.count() > 50:
                    issues.append({
                        'type': 'missing_pagination',
                        'severity': 'warning',
                        'message': f'List view returns {qs.count()} objects without pagination',
                        'suggestion': 'Use Django Paginator or limit queryset'
                    })

            # Check for inefficient datetime filters
            query_sql = str(qs.query)
            if 'created_at >=' in query_sql and 'created_at <' not in query_sql:
                issues.append({
                    'type': 'inefficient_datetime_filter',
                    'severity': 'info',
                    'message': 'Open-ended datetime filter may be slow',
                    'suggestion': 'Add upper bound to datetime filters'
                })

        return issues

    def analyze_model_usage(self, model_class):
        """Analyze Django model usage patterns"""
        issues = []

        # Check for missing Meta.indexes
        if hasattr(model_class._meta, 'get_fields'):
            for field in model_class._meta.get_fields():
                if hasattr(field, 'db_index') and not field.db_index:
                    if field.name in ['email', 'username', 'slug']:
                        issues.append({
                            'type': 'missing_common_index',
                            'model': model_class.__name__,
                            'field': field.name,
                            'suggestion': f'Consider adding db_index=True to {field.name}'
                        })

        return issues
```

```python
# settings.py
DBCRUST = {
    'CUSTOM_ANALYZERS': [
        'myapp.dbcrust_django_rules.CustomDjangoAnalyzer',
    ]
}
```

### Performance Regression Testing

Integrate with Django tests to prevent performance regressions:

```python
# test_performance.py
from django.test import TestCase
from django.core.management import call_command
from django.test.utils import override_settings

class PerformanceRegressionTest(TestCase):
    """Prevent Django ORM performance regressions"""

    def test_view_performance_baseline(self):
        """Ensure views don't exceed performance baselines"""

        # Run performance analysis
        result = call_command(
            'dbcrust_profile_views',
            views='book_list,author_detail',
            verbosity=0,
            return_result=True
        )

        # Check performance baselines
        baselines = {
            'book_list': {'max_queries': 5, 'max_duration': 1000},
            'author_detail': {'max_queries': 3, 'max_duration': 500}
        }

        for view, baseline in baselines.items():
            view_result = result[view]

            self.assertLessEqual(
                view_result['query_count'],
                baseline['max_queries'],
                f"{view} exceeded query limit: {view_result['query_count']} > {baseline['max_queries']}"
            )

            self.assertLessEqual(
                view_result['duration_ms'],
                baseline['max_duration'],
                f"{view} exceeded duration limit: {view_result['duration_ms']}ms > {baseline['max_duration']}ms"
            )

    def test_model_efficiency(self):
        """Test model usage efficiency"""

        result = call_command(
            'dbcrust_analyze_code',
            files='views.py,models.py',
            verbosity=0,
            return_result=True
        )

        # No critical issues allowed
        critical_issues = [issue for issue in result if issue['severity'] == 'critical']
        self.assertEqual(
            len(critical_issues), 0,
            f"Critical ORM issues found: {critical_issues}"
        )
```

### Production Monitoring Integration

Monitor Django ORM performance in production:

```python
# management/commands/dbcrust_production_monitor.py

from django.core.management.base import BaseCommand
from dbcrust.django.monitoring import ProductionMonitor

class Command(BaseCommand):
    help = 'Monitor Django ORM performance in production'

    def add_arguments(self, parser):
        parser.add_argument('--duration', type=int, default=300, help='Monitor duration in seconds')
        parser.add_argument('--sample-rate', type=float, default=0.1, help='Sampling rate (0.1 = 10%)')

    def handle(self, *args, **options):
        monitor = ProductionMonitor(
            sample_rate=options['sample_rate'],
            alert_thresholds={
                'query_count': 20,
                'duration_ms': 5000,
                'n_plus_one_count': 3
            }
        )

        self.stdout.write(f"Starting production monitoring for {options['duration']} seconds...")

        results = monitor.run(duration=options['duration'])

        # Report findings
        if results['alerts']:
            self.stdout.write("🚨 Performance alerts:")
            for alert in results['alerts']:
                self.stdout.write(f"  - {alert['message']}")

        self.stdout.write(f"✅ Monitoring complete. Sampled {results['requests_monitored']} requests.")
```

## 🚨 Troubleshooting

### Common Issues

**Command not found:**
```bash
# Ensure DBCrust is in INSTALLED_APPS
python manage.py shell
>>> from django.conf import settings
>>> 'dbcrust.django' in settings.INSTALLED_APPS
True
```

**Database connection issues:**
```bash
# Test Django database connection first
python manage.py dbshell

# Check specific database
python manage.py dbcrust --database analytics --debug
```

**Permission issues:**
```bash
# Check database user permissions
python manage.py dbcrust --read-only
```

**Analysis not working:**
```python
# Check Django settings
DBCRUST = {
    'ANALYSIS': {
        'ENABLED': True,  # Must be True
    }
}
```

### Debug Mode

Enable detailed debugging for Django integration:

```bash
# Enable Django debug logging
export DJANGO_LOG_LEVEL=DEBUG

# Enable DBCrust debug logging
export DBCRUST_DEBUG=1

# Run command with debugging
python manage.py dbcrust --debug
```

## 💡 Best Practices

### Development Workflow

```bash
# Daily development routine
python manage.py dbcrust_analyze --app myapp          # Check for issues
python manage.py dbcrust_migrate_check               # Verify migrations
python manage.py dbcrust --database default          # Interactive analysis

# Before committing code
python manage.py dbcrust_analyze_code --auto-fix     # Fix simple issues
python manage.py test test_performance               # Run performance tests
```

### Team Workflows

```bash
# Code review preparation
python manage.py dbcrust_generate_optimizations --generate-patches

# Performance monitoring
python manage.py dbcrust_production_monitor --duration 3600  # 1 hour

# Regular health checks
python manage.py dbcrust_model_stats --include-relations
```

### CI/CD Integration

```bash
# In your CI pipeline
python manage.py dbcrust_analyze_code --fail-on-critical
python manage.py dbcrust_profile_views --baseline performance_baseline.json
python manage.py test test_performance
```

## 📚 See Also

- **[Django Middleware](/dbcrust/django/middleware/)** - Real-time ORM analysis
- **[CI/CD Integration](/dbcrust/django/ci-integration/)** - Automated performance testing
- **[Team Workflows](/dbcrust/django/team-workflows/)** - Collaborative optimization
- **[Django ORM Analyzer](/dbcrust/django-analyzer/)** - Complete analyzer documentation

---

<div align="center">
    <strong>Ready to optimize your Django project?</strong><br>
    <a href="/dbcrust/django/ci-integration/" class="md-button md-button--primary">CI/CD Integration</a>
    <a href="/dbcrust/django/middleware/" class="md-button">Middleware Setup</a>
</div>
