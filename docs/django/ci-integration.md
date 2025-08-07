# CI/CD Integration

Integrate DBCrust's Django ORM analysis into your CI/CD pipeline to prevent performance regressions and enforce database best practices. This guide covers GitHub Actions, GitLab CI, Jenkins, and other popular CI/CD platforms.

## 🚀 Quick Setup

### GitHub Actions

Add DBCrust performance testing to your Django CI pipeline:

```yaml
# .github/workflows/django.yml
name: Django CI

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest

    services:
      postgres:
        image: postgres:15
        env:
          POSTGRES_PASSWORD: postgres
        options: >-
          --health-cmd pg_isready
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5

    steps:
    - uses: actions/checkout@v4

    - name: Set up Python
      uses: actions/setup-python@v4
      with:
        python-version: '3.11'

    - name: Install dependencies
      run: |
        pip install -r requirements.txt
        pip install dbcrust

    - name: Run Django tests
      env:
        DATABASE_URL: postgres://postgres:postgres@localhost/test_db
      run: |
        python manage.py test

    - name: DBCrust ORM Analysis
      env:
        DATABASE_URL: postgres://postgres:postgres@localhost/test_db
      run: |
        # Analyze code for ORM performance issues
        python manage.py dbcrust_analyze_code --fail-on-critical

        # Profile view performance
        python manage.py dbcrust_profile_views --baseline .dbcrust/performance_baseline.json

        # Run performance tests
        python manage.py test tests.test_performance
```

### GitLab CI

```yaml
# .gitlab-ci.yml
stages:
  - test
  - performance

variables:
  POSTGRES_DB: test_db
  POSTGRES_USER: postgres
  POSTGRES_PASSWORD: postgres
  DATABASE_URL: postgres://postgres:postgres@postgres:5432/test_db

services:
  - postgres:15

test:
  stage: test
  image: python:3.11
  before_script:
    - pip install -r requirements.txt
    - pip install dbcrust
  script:
    - python manage.py test

performance_analysis:
  stage: performance
  image: python:3.11
  before_script:
    - pip install -r requirements.txt
    - pip install dbcrust
  script:
    - python manage.py dbcrust_analyze_code --report json --output analysis.json
    - python manage.py dbcrust_profile_views --output performance.json
  artifacts:
    reports:
      performance: performance.json
    paths:
      - analysis.json
  only:
    - merge_requests
    - main
```

## 🔧 Performance Testing Configuration

### Performance Baselines

Create performance baselines to prevent regressions:

```json
// .dbcrust/performance_baseline.json
{
  "views": {
    "book_list": {
      "max_queries": 5,
      "max_duration_ms": 1000,
      "max_memory_mb": 50
    },
    "author_detail": {
      "max_queries": 3,
      "max_duration_ms": 500,
      "max_memory_mb": 20
    }
  },
  "models": {
    "Book": {
      "max_relations_depth": 3,
      "required_select_related": ["author"],
      "required_prefetch_related": ["reviews"]
    }
  }
}
```

### Django Performance Tests

```python
# tests/test_performance.py
from django.test import TestCase, TransactionTestCase
from django.test.utils import override_settings
from django.urls import reverse
from dbcrust.django.testing import PerformanceTestCase
from myapp.models import Book, Author

class ViewPerformanceTest(PerformanceTestCase):
    """Performance tests for Django views"""

    def setUp(self):
        # Create test data
        self.author = Author.objects.create(name="Test Author")
        self.books = [
            Book.objects.create(title=f"Book {i}", author=self.author)
            for i in range(20)
        ]

    def test_book_list_performance(self):
        """Test book list view meets performance requirements"""
        with self.assert_performance(max_queries=5, max_duration=1000):
            response = self.client.get(reverse('book_list'))
            self.assertEqual(response.status_code, 200)

    def test_author_detail_performance(self):
        """Test author detail view performance"""
        with self.assert_performance(max_queries=3):
            response = self.client.get(
                reverse('author_detail', args=[self.author.id])
            )
            self.assertEqual(response.status_code, 200)

class ORMPerformanceTest(TestCase):
    """Test ORM usage patterns"""

    def test_no_n_plus_one_queries(self):
        """Ensure no N+1 queries in critical paths"""
        from dbcrust.django.analyzers import analyze_code_path

        # Analyze specific code path
        issues = analyze_code_path('myapp.views.book_list')
        n_plus_one_issues = [i for i in issues if i['type'] == 'n_plus_one']

        self.assertEqual(len(n_plus_one_issues), 0,
                        f"N+1 queries detected: {n_plus_one_issues}")

    def test_model_efficiency(self):
        """Test model queries are efficient"""
        with self.assertNumQueries(1):
            # This should use select_related
            books = list(Book.objects.select_related('author').all()[:10])
            for book in books:
                _ = book.author.name  # Should not trigger additional queries
```

### Automated Code Analysis

```python
# tests/test_code_quality.py
import subprocess
import json
from django.test import TestCase

class CodeQualityTest(TestCase):
    """Test code quality with DBCrust static analysis"""

    def test_orm_code_analysis(self):
        """Run DBCrust code analysis and check results"""

        # Run DBCrust code analysis
        result = subprocess.run([
            'python', 'manage.py', 'dbcrust_analyze_code',
            '--report', 'json'
        ], capture_output=True, text=True, cwd='.')

        self.assertEqual(result.returncode, 0,
                        f"Code analysis failed: {result.stderr}")

        # Parse results
        analysis = json.loads(result.stdout)

        # No critical issues allowed
        critical_issues = [
            issue for issue in analysis['issues']
            if issue['severity'] == 'critical'
        ]

        self.assertEqual(len(critical_issues), 0,
                        f"Critical ORM issues found: {critical_issues}")

        # Limit warning count
        warnings = [
            issue for issue in analysis['issues']
            if issue['severity'] == 'warning'
        ]

        self.assertLess(len(warnings), 10,
                       f"Too many ORM warnings ({len(warnings)}): {warnings}")
```

## 🎯 CI/CD Pipeline Patterns

### Pull Request Analysis

Automatically analyze Django ORM changes in pull requests:

```yaml
# .github/workflows/pr-analysis.yml
name: PR ORM Analysis

on:
  pull_request:
    paths:
      - '**/*.py'
      - '**/models.py'
      - '**/views.py'

jobs:
  orm_analysis:
    runs-on: ubuntu-latest

    steps:
    - uses: actions/checkout@v4
      with:
        fetch-depth: 0  # Need full history for diff

    - name: Set up Python
      uses: actions/setup-python@v4
      with:
        python-version: '3.11'

    - name: Install DBCrust
      run: pip install dbcrust

    - name: Analyze changed files
      run: |
        # Get changed Python files
        CHANGED_FILES=$(git diff --name-only origin/main...HEAD | grep '\.py$' | tr '\n' ' ')

        if [ ! -z "$CHANGED_FILES" ]; then
          echo "Analyzing changed files: $CHANGED_FILES"
          python manage.py dbcrust_analyze_code --files "$CHANGED_FILES" --report json > analysis.json
        fi

    - name: Comment on PR
      uses: actions/github-script@v6
      with:
        script: |
          const fs = require('fs');
          if (fs.existsSync('analysis.json')) {
            const analysis = JSON.parse(fs.readFileSync('analysis.json', 'utf8'));

            if (analysis.issues.length > 0) {
              const comment = `## 🔍 DBCrust ORM Analysis

              Found ${analysis.issues.length} potential ORM performance issues:

              ${analysis.issues.map(issue =>
                `- **${issue.severity.toUpperCase()}**: ${issue.message} (${issue.file}:${issue.line})\n  💡 ${issue.suggestion}`
              ).join('\n')}

              [View full analysis report](${context.payload.pull_request.html_url}/checks)`;

              github.rest.issues.createComment({
                issue_number: context.issue.number,
                owner: context.repo.owner,
                repo: context.repo.repo,
                body: comment
              });
            }
          }
```

### Performance Regression Detection

Detect performance regressions across builds:

```yaml
# .github/workflows/performance-tracking.yml
name: Performance Tracking

on:
  push:
    branches: [main]
  schedule:
    - cron: '0 2 * * *'  # Daily at 2 AM

jobs:
  performance_tracking:
    runs-on: ubuntu-latest

    services:
      postgres:
        image: postgres:15
        env:
          POSTGRES_PASSWORD: postgres
        options: >-
          --health-cmd pg_isready
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5

    steps:
    - uses: actions/checkout@v4

    - name: Setup Python
      uses: actions/setup-python@v4
      with:
        python-version: '3.11'

    - name: Install dependencies
      run: |
        pip install -r requirements.txt
        pip install dbcrust

    - name: Run performance benchmarks
      env:
        DATABASE_URL: postgres://postgres:postgres@localhost/test_db
      run: |
        # Create test database and data
        python manage.py migrate
        python manage.py loaddata fixtures/performance_test_data.json

        # Run performance analysis
        python manage.py dbcrust_profile_views --all-views --output current_performance.json

    - name: Compare with baseline
      run: |
        python scripts/compare_performance.py \
          --baseline .dbcrust/performance_baseline.json \
          --current current_performance.json \
          --output performance_report.json

    - name: Upload performance data
      uses: actions/upload-artifact@v3
      with:
        name: performance-data
        path: |
          current_performance.json
          performance_report.json

    - name: Update baseline if improved
      if: success()
      run: |
        # Update baseline if performance improved
        python scripts/update_baseline.py \
          --current current_performance.json \
          --baseline .dbcrust/performance_baseline.json

        # Commit updated baseline
        git config --local user.email "action@github.com"
        git config --local user.name "GitHub Action"
        git add .dbcrust/performance_baseline.json
        git diff --staged --quiet || git commit -m "Update performance baseline [skip ci]"
        git push
```

### Migration Safety Checks

Automatically check Django migrations for performance impact:

```yaml
# .github/workflows/migration-check.yml
name: Migration Safety Check

on:
  pull_request:
    paths:
      - '**/migrations/*.py'

jobs:
  migration_check:
    runs-on: ubuntu-latest

    services:
      postgres:
        image: postgres:15
        env:
          POSTGRES_PASSWORD: postgres

    steps:
    - uses: actions/checkout@v4

    - name: Setup Python
      uses: actions/setup-python@v4
      with:
        python-version: '3.11'

    - name: Install dependencies
      run: |
        pip install -r requirements.txt
        pip install dbcrust

    - name: Check migration safety
      env:
        DATABASE_URL: postgres://postgres:postgres@localhost/test_db
      run: |
        # Analyze migration impact
        python manage.py dbcrust_migrate_check --analyze-impact --report json > migration_analysis.json

    - name: Comment on PR with migration analysis
      uses: actions/github-script@v6
      with:
        script: |
          const fs = require('fs');
          if (fs.existsSync('migration_analysis.json')) {
            const analysis = JSON.parse(fs.readFileSync('migration_analysis.json', 'utf8'));

            let comment = '## 🔄 Migration Safety Analysis\n\n';

            analysis.migrations.forEach(migration => {
              comment += `### ${migration.name}\n`;
              comment += `- **Safety**: ${migration.safety}\n`;
              comment += `- **Estimated Time**: ${migration.estimated_time}\n`;
              comment += `- **Lock Level**: ${migration.lock_level}\n`;

              if (migration.recommendations.length > 0) {
                comment += `- **Recommendations**: ${migration.recommendations.join(', ')}\n`;
              }
              comment += '\n';
            });

            github.rest.issues.createComment({
              issue_number: context.issue.number,
              owner: context.repo.owner,
              repo: context.repo.repo,
              body: comment
            });
          }
```

## 🏗️ Advanced CI/CD Patterns

### Multi-Environment Testing

Test performance across different environments:

```yaml
# .github/workflows/multi-env-performance.yml
name: Multi-Environment Performance

on: [push, pull_request]

strategy:
  matrix:
    environment: [development, staging, production-like]
    python-version: ['3.10', '3.11']
    django-version: ['4.2', '5.0']

jobs:
  performance_test:
    runs-on: ubuntu-latest

    steps:
    - uses: actions/checkout@v4

    - name: Setup Python ${{ matrix.python-version }}
      uses: actions/setup-python@v4
      with:
        python-version: ${{ matrix.python-version }}

    - name: Install Django ${{ matrix.django-version }}
      run: |
        pip install Django==${{ matrix.django-version }}
        pip install -r requirements.txt
        pip install dbcrust

    - name: Load environment config
      run: |
        cp configs/${{ matrix.environment }}.py settings.py

    - name: Run performance tests
      run: |
        python manage.py dbcrust_profile_views \
          --baseline baselines/${{ matrix.environment }}_baseline.json \
          --output results/${{ matrix.environment }}_results.json
```

### Performance Budget Enforcement

Enforce performance budgets in your pipeline:

```python
# scripts/enforce_performance_budget.py
#!/usr/bin/env python3
"""Enforce performance budgets in CI/CD"""

import json
import sys
import argparse

def check_performance_budget(results_file, budget_file):
    """Check if performance results meet budget requirements"""

    with open(results_file) as f:
        results = json.load(f)

    with open(budget_file) as f:
        budgets = json.load(f)

    violations = []

    for view_name, result in results['views'].items():
        if view_name in budgets['views']:
            budget = budgets['views'][view_name]

            # Check query count budget
            if result['query_count'] > budget.get('max_queries', float('inf')):
                violations.append({
                    'view': view_name,
                    'metric': 'query_count',
                    'actual': result['query_count'],
                    'budget': budget['max_queries'],
                    'violation': result['query_count'] - budget['max_queries']
                })

            # Check duration budget
            if result['duration_ms'] > budget.get('max_duration_ms', float('inf')):
                violations.append({
                    'view': view_name,
                    'metric': 'duration_ms',
                    'actual': result['duration_ms'],
                    'budget': budget['max_duration_ms'],
                    'violation': result['duration_ms'] - budget['max_duration_ms']
                })

    return violations

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--results', required=True)
    parser.add_argument('--budget', required=True)

    args = parser.parse_args()

    violations = check_performance_budget(args.results, args.budget)

    if violations:
        print("🚨 Performance budget violations:")
        for v in violations:
            print(f"  - {v['view']}: {v['metric']} = {v['actual']} "
                  f"(budget: {v['budget']}, over by: {v['violation']})")
        sys.exit(1)
    else:
        print("✅ All performance budgets met!")
        sys.exit(0)
```

### Integration with Monitoring Tools

Send performance data to monitoring systems:

```python
# scripts/send_to_monitoring.py
#!/usr/bin/env python3
"""Send performance metrics to monitoring systems"""

import json
import requests
import os
from datetime import datetime

def send_to_datadog(metrics):
    """Send metrics to Datadog"""
    api_key = os.getenv('DATADOG_API_KEY')

    for metric in metrics:
        payload = {
            'series': [{
                'metric': f"django.orm.{metric['name']}",
                'points': [[int(datetime.now().timestamp()), metric['value']]],
                'tags': metric.get('tags', [])
            }]
        }

        requests.post(
            'https://api.datadoghq.com/api/v1/series',
            headers={'DD-API-KEY': api_key},
            json=payload
        )

def send_to_prometheus(metrics):
    """Send metrics to Prometheus pushgateway"""
    gateway_url = os.getenv('PROMETHEUS_PUSHGATEWAY_URL')

    for metric in metrics:
        data = f"{metric['name']} {metric['value']}\n"
        requests.post(f"{gateway_url}/metrics/job/django_orm_analysis", data=data)

# Usage in CI
if __name__ == '__main__':
    with open('performance_results.json') as f:
        results = json.load(f)

    metrics = []
    for view_name, data in results['views'].items():
        metrics.extend([
            {
                'name': 'query_count',
                'value': data['query_count'],
                'tags': [f'view:{view_name}']
            },
            {
                'name': 'duration_ms',
                'value': data['duration_ms'],
                'tags': [f'view:{view_name}']
            }
        ])

    send_to_datadog(metrics)
    send_to_prometheus(metrics)
```

## 🚨 Troubleshooting CI/CD

### Common Issues

**Tests timing out:**
```yaml
# Increase timeout for performance tests
- name: Run performance tests
  run: python manage.py dbcrust_profile_views
  timeout-minutes: 10  # Increase from default 5 minutes
```

**Database connection issues:**
```yaml
# Wait for database to be ready
- name: Wait for PostgreSQL
  run: |
    until pg_isready -h localhost -p 5432; do
      echo "Waiting for PostgreSQL..."
      sleep 2
    done
```

**Memory issues with large datasets:**
```python
# Limit test data size in CI
if os.getenv('CI'):
    TEST_DATA_SIZE = 100  # Smaller dataset for CI
else:
    TEST_DATA_SIZE = 1000  # Full dataset for local testing
```

### Debug CI Performance Issues

```yaml
- name: Debug performance issues
  if: failure()
  run: |
    # Show detailed analysis
    python manage.py dbcrust_analyze --verbose --debug

    # Show database state
    python manage.py dbshell -c "\dt+"  # Show table sizes

    # Show memory usage
    python -c "
    import psutil
    print(f'Memory usage: {psutil.virtual_memory().percent}%')
    print(f'Available memory: {psutil.virtual_memory().available / 1024**3:.2f}GB')
    "
```

## 📚 See Also

- **[Django Management Commands](/dbcrust/django/management-commands/)** - CLI tools for Django
- **[Django Middleware](/dbcrust/django/middleware/)** - Real-time ORM analysis
- **[Team Workflows](/dbcrust/django/team-workflows/)** - Collaborative optimization
- **[Django ORM Analyzer](/dbcrust/django-analyzer/)** - Complete analyzer documentation

---

<div align="center">
    <strong>Ready to automate your Django performance testing?</strong><br>
    <a href="/dbcrust/django/team-workflows/" class="md-button md-button--primary">Team Workflows</a>
    <a href="/dbcrust/django/management-commands/" class="md-button">Management Commands</a>
</div>
