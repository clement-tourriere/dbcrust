# Django Team Workflows

Establish effective team workflows for Django ORM performance optimization using DBCrust. This guide covers collaboration patterns, code review processes, performance monitoring, and team adoption strategies.

## 🚀 Quick Team Setup

### 1. Project Configuration

Create shared team configuration for consistent analysis:

```toml
# .dbcrust/team-config.toml (commit to version control)
[analysis]
enabled = true
detect_n_plus_one = true
suggest_indexes = true
report_threshold = 3

[standards]
max_queries_per_view = 10
max_query_duration_ms = 1000
require_select_related = ["author", "category", "user"]
require_prefetch_related = ["tags", "comments", "reviews"]

[team]
enforce_performance_budgets = true
auto_fix_simple_issues = true
require_approval_for_slow_queries = true
```

### 2. Pre-commit Hooks

Prevent performance issues from entering the codebase:

```yaml
# .pre-commit-config.yaml
repos:
  - repo: local
    hooks:
      - id: django-orm-check
        name: Django ORM Performance Check
        entry: python manage.py dbcrust_analyze_code --fail-on-critical
        language: system
        files: \.(py)$
        pass_filenames: false

      - id: django-migration-check
        name: Django Migration Safety Check
        entry: python manage.py dbcrust_migrate_check
        language: system
        files: migrations/.*\.py$
        pass_filenames: false
```

### 3. Shared Performance Baselines

```json
// .dbcrust/team_baselines.json
{
  "views": {
    "critical_views": {
      "user_dashboard": {"max_queries": 5, "max_duration": 500},
      "product_list": {"max_queries": 8, "max_duration": 800},
      "checkout_process": {"max_queries": 12, "max_duration": 1200}
    },
    "standard_views": {
      "max_queries": 10,
      "max_duration": 1000
    }
  },
  "models": {
    "required_optimizations": {
      "User": ["profile", "permissions"],
      "Product": ["category", "brand"],
      "Order": ["user", "items__product"]
    }
  }
}
```

## 👥 Code Review Workflows

### Performance-Focused Code Reviews

**Automated Analysis in PRs:**

```yaml
# .github/workflows/pr-performance-review.yml
name: Performance Review

on:
  pull_request:
    paths: ['**/*.py']

jobs:
  performance_review:
    runs-on: ubuntu-latest
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

    - name: Analyze changed files
      run: |
        CHANGED_FILES=$(git diff --name-only origin/main...HEAD -- '*.py')
        python manage.py dbcrust_analyze_code --files $CHANGED_FILES --output pr_analysis.json

    - name: Generate performance review comment
      uses: actions/github-script@v6
      with:
        script: |
          const fs = require('fs');
          const analysis = JSON.parse(fs.readFileSync('pr_analysis.json'));

          let comment = '## 🔍 DBCrust Performance Review\n\n';

          if (analysis.critical_issues.length > 0) {
            comment += '### 🔴 Critical Issues (Must Fix)\n';
            analysis.critical_issues.forEach(issue => {
              comment += `- **${issue.type}**: ${issue.message} (${issue.file}:${issue.line})\n`;
              comment += `  💡 **Fix**: ${issue.suggestion}\n\n`;
            });
          }

          if (analysis.optimization_opportunities.length > 0) {
            comment += '### 💡 Optimization Opportunities\n';
            analysis.optimization_opportunities.forEach(opt => {
              comment += `- ${opt.message} (${opt.file}:${opt.line})\n`;
            });
          }

          if (analysis.critical_issues.length === 0 && analysis.optimization_opportunities.length === 0) {
            comment += '✅ No performance issues detected. Great work!\n';
          }

          github.rest.issues.createComment({
            issue_number: context.issue.number,
            owner: context.repo.owner,
            repo: context.repo.repo,
            body: comment
          });
```

**Manual Review Checklist:**

```markdown
## Django ORM Performance Review Checklist

### Before Reviewing
- [ ] Run `python manage.py dbcrust_analyze_code --files <changed_files>`
- [ ] Check for new migrations with `python manage.py dbcrust_migrate_check`

### Code Review Points
- [ ] **N+1 Queries**: Look for loops accessing related objects
- [ ] **Missing select_related**: Check ForeignKey access patterns
- [ ] **Missing prefetch_related**: Check Many-to-Many/Reverse FK access
- [ ] **Inefficient filters**: Look for complex WHERE clauses
- [ ] **Missing indexes**: Check frequently filtered/ordered fields

### View-Specific Checks
- [ ] List views use pagination
- [ ] Detail views use select_related for displayed relationships
- [ ] Search/filter views have appropriate indexes
- [ ] API endpoints meet performance budgets

### Migration Reviews
- [ ] No dangerous operations in production migrations
- [ ] Index additions use CONCURRENT where possible
- [ ] Large table changes planned for maintenance windows
```

### Performance-Aware Git Hooks

**Pre-push Hook:**

```bash
#!/bin/bash
# .git/hooks/pre-push

echo "🔍 Running Django ORM performance checks..."

# Check for critical performance issues
python manage.py dbcrust_analyze_code --fail-on-critical
if [ $? -ne 0 ]; then
    echo "❌ Critical performance issues detected. Push blocked."
    echo "Run 'python manage.py dbcrust_analyze_code' for details."
    exit 1
fi

# Check migration safety
python manage.py dbcrust_migrate_check --fail-on-unsafe
if [ $? -ne 0 ]; then
    echo "❌ Unsafe migrations detected. Push blocked."
    echo "Run 'python manage.py dbcrust_migrate_check' for details."
    exit 1
fi

echo "✅ Performance checks passed."
```

## 📊 Team Performance Monitoring

### Shared Performance Dashboard

```python
# monitoring/team_dashboard.py
from django.shortcuts import render
from dbcrust.django.monitoring import get_team_metrics

def team_performance_dashboard(request):
    """Team performance monitoring dashboard"""

    metrics = get_team_metrics(days=30)

    context = {
        'team_metrics': {
            'total_views_analyzed': metrics['views_count'],
            'critical_issues_found': metrics['critical_issues'],
            'performance_improvements': metrics['improvements'],
            'team_performance_score': metrics['team_score']
        },
        'developer_metrics': [
            {
                'developer': dev['name'],
                'commits_analyzed': dev['commits'],
                'issues_introduced': dev['issues'],
                'fixes_applied': dev['fixes'],
                'performance_score': dev['score']
            }
            for dev in metrics['by_developer']
        ],
        'recent_improvements': metrics['recent_improvements'][:10]
    }

    return render(request, 'monitoring/team_dashboard.html', context)
```

### Performance Metrics Tracking

```python
# settings.py
DBCRUST_TEAM_MONITORING = {
    'TRACK_BY_DEVELOPER': True,
    'TRACK_BY_FEATURE': True,
    'METRICS_RETENTION_DAYS': 90,

    'ALERTS': {
        'SLACK_WEBHOOK': os.getenv('SLACK_PERFORMANCE_WEBHOOK'),
        'ALERT_ON_REGRESSION': True,
        'ALERT_ON_CRITICAL_ISSUES': True,
        'WEEKLY_TEAM_REPORT': True
    },

    'GAMIFICATION': {
        'ENABLE_SCORING': True,
        'POINTS_FOR_FIXES': 10,
        'POINTS_FOR_OPTIMIZATIONS': 5,
        'PENALTY_FOR_REGRESSIONS': -15
    }
}
```

### Slack Integration

```python
# utils/slack_notifications.py
import requests
import os
from django.conf import settings

def send_performance_alert(issue_type, details):
    """Send performance alerts to Slack"""

    webhook_url = getattr(settings, 'SLACK_PERFORMANCE_WEBHOOK', None)
    if not webhook_url:
        return

    color_map = {
        'critical': '#ff0000',
        'warning': '#ffaa00',
        'improvement': '#00ff00'
    }

    message = {
        "attachments": [{
            "color": color_map.get(issue_type, '#cccccc'),
            "title": f"🔍 Django ORM Performance Alert",
            "text": details['message'],
            "fields": [
                {
                    "title": "Issue Type",
                    "value": details['type'],
                    "short": True
                },
                {
                    "title": "Location",
                    "value": f"{details['file']}:{details['line']}",
                    "short": True
                },
                {
                    "title": "Fix Suggestion",
                    "value": details['suggestion'],
                    "short": False
                }
            ]
        }]
    }

    requests.post(webhook_url, json=message)
```

## 🎯 Team Training & Adoption

### Django ORM Performance Training Plan

**Week 1: Foundations**
- Understanding N+1 queries and their impact
- Introduction to select_related() and prefetch_related()
- Hands-on: Setting up DBCrust in development

**Week 2: Analysis Tools**
- Using DBCrust middleware for real-time analysis
- Management commands for code analysis
- Interpreting performance reports

**Week 3: Optimization Techniques**
- Database indexing strategies
- Query optimization patterns
- Custom QuerySet methods

**Week 4: Team Workflows**
- Code review processes
- CI/CD integration
- Performance monitoring

### Knowledge Sharing Sessions

**Monthly ORM Performance Review:**

```python
# utils/team_knowledge_sharing.py

def generate_monthly_performance_report():
    """Generate team knowledge sharing content"""

    # Analyze common issues from last month
    common_issues = analyze_team_issues(days=30)

    # Generate training content
    training_topics = []

    if common_issues['n_plus_one_count'] > 5:
        training_topics.append({
            'topic': 'N+1 Query Prevention',
            'examples': common_issues['n_plus_one_examples'],
            'priority': 'high'
        })

    if common_issues['missing_indexes'] > 3:
        training_topics.append({
            'topic': 'Database Indexing Strategies',
            'examples': common_issues['index_examples'],
            'priority': 'medium'
        })

    return {
        'training_topics': training_topics,
        'team_improvements': common_issues['improvements'],
        'success_stories': common_issues['success_stories']
    }
```

### Performance Champions Program

**Designate Performance Champions:**

```python
# Team performance champions configuration
PERFORMANCE_CHAMPIONS = {
    'backend_team': {
        'champion': 'senior_dev@company.com',
        'responsibilities': [
            'Review performance-critical PRs',
            'Mentor junior developers on ORM optimization',
            'Maintain team performance standards'
        ]
    },
    'frontend_team': {
        'champion': 'frontend_lead@company.com',
        'responsibilities': [
            'Review database queries from frontend',
            'Optimize API endpoint performance',
            'Coordinate with backend on data fetching'
        ]
    }
}
```

## 🛠️ Team Tools & Scripts

### Shared Development Scripts

```bash
#!/bin/bash
# scripts/team_performance_check.sh

echo "🔍 Running team performance checks..."

# 1. Check current branch for issues
echo "Checking current branch..."
python manage.py dbcrust_analyze_code --report summary

# 2. Compare with main branch performance
echo "Comparing with main branch..."
git checkout main
python manage.py dbcrust_profile_views --output main_performance.json
git checkout -

python manage.py dbcrust_profile_views --output current_performance.json
python scripts/compare_performance.py main_performance.json current_performance.json

# 3. Check migration safety
if ls */migrations/*.py 1> /dev/null 2>&1; then
    echo "Checking migrations..."
    python manage.py dbcrust_migrate_check
fi

# 4. Run performance tests
echo "Running performance tests..."
python manage.py test tests.performance --keepdb

echo "✅ Team performance check complete!"
```

### Performance Issue Templates

**GitHub Issue Template:**

```markdown
---
name: Performance Issue
about: Report Django ORM performance issue
labels: performance, django-orm
---

## Performance Issue Report

### Issue Description
<!-- Brief description of the performance issue -->

### DBCrust Analysis Output
```
<!-- Paste output from: python manage.py dbcrust_analyze_code -->
```

### Environment
- Django version:
- Database:
- DBCrust version:

### Current Performance
- Query count:
- Duration:
- Memory usage:

### Expected Performance
- Target query count:
- Target duration:

### Reproduction Steps
1.
2.
3.

### Additional Context
<!-- Any additional context about the issue -->
```

### Team Code Review Templates

**Pull Request Template:**

```markdown
## Performance Review Checklist

### Automated Checks
- [ ] DBCrust analysis passed (check CI results)
- [ ] Performance tests passed
- [ ] Migration safety verified

### Manual Review
- [ ] No new N+1 queries introduced
- [ ] Appropriate use of select_related/prefetch_related
- [ ] Database queries are efficient
- [ ] No missing indexes for new filters/searches

### Performance Impact
- [ ] No performance regression detected
- [ ] Performance improvements documented
- [ ] Performance budget maintained

### Documentation
- [ ] Performance-critical code documented
- [ ] Database schema changes documented
- [ ] Migration rollback plan documented (if applicable)
```

## 📈 Performance Culture

### Team Performance Goals

**Quarterly OKRs Example:**

```yaml
Q1_2024_Performance_OKRs:
  objective: "Improve Django application performance"
  key_results:
    - reduce_average_page_load_time:
        target: "< 500ms"
        current: "800ms"
        metric: "average API response time"

    - eliminate_n_plus_one_queries:
        target: "0 critical issues"
        current: "5 issues"
        metric: "DBCrust critical issue count"

    - improve_database_efficiency:
        target: "< 5 queries per view average"
        current: "8 queries per view"
        metric: "average queries per view"
```

### Success Metrics

**Team Performance Dashboard KPIs:**

```python
TEAM_PERFORMANCE_KPIS = {
    'code_quality': {
        'critical_issues_per_week': {'target': 0, 'weight': 40},
        'performance_regression_rate': {'target': 0, 'weight': 30},
        'optimization_adoption_rate': {'target': 95, 'weight': 20}
    },
    'development_velocity': {
        'time_to_fix_issues': {'target': 24, 'weight': 30},  # hours
        'performance_review_time': {'target': 2, 'weight': 20}  # days
    },
    'knowledge_sharing': {
        'team_training_completion': {'target': 100, 'weight': 25},
        'performance_champions_active': {'target': 2, 'weight': 15}
    }
}
```

## 🚨 Troubleshooting Team Issues

### Common Team Adoption Challenges

**Issue: Developers bypassing performance checks**
```python
# Solution: Make checks part of CI/CD pipeline
# .github/workflows/required-checks.yml
required_status_checks:
  strict: true
  contexts:
    - "django-orm-performance-check"
    - "migration-safety-check"
```

**Issue: Too many false positives in analysis**
```python
# Solution: Customize analysis rules for your team
DBCRUST_TEAM_RULES = {
    'IGNORE_PATTERNS': [
        'admin.*',  # Skip Django admin
        'test_.*',  # Skip test files
    ],
    'CUSTOM_THRESHOLDS': {
        'n_plus_one_threshold': 5,  # Adjust for your needs
        'query_count_threshold': 15,
    }
}
```

**Issue: Slow adoption of best practices**
```python
# Solution: Gamification and positive reinforcement
PERFORMANCE_REWARDS = {
    'performance_improvement_points': 50,
    'n_plus_one_fix_points': 25,
    'team_leaderboard': True,
    'monthly_performance_champion': True
}
```

## 📚 See Also

- **[Django Middleware](/dbcrust/django/middleware/)** - Real-time ORM analysis setup
- **[CI/CD Integration](/dbcrust/django/ci-integration/)** - Automated performance testing
- **[Django Management Commands](/dbcrust/django/management-commands/)** - CLI tools for teams
- **[Django ORM Analyzer](/dbcrust/django-analyzer/)** - Complete analyzer documentation

---

<div align="center">
    <strong>Ready to build a performance-focused Django team?</strong><br>
    <a href="/dbcrust/django-analyzer/" class="md-button md-button--primary">Complete Django Guide</a>
    <a href="/dbcrust/django/ci-integration/" class="md-button">CI/CD Integration</a>
</div>
