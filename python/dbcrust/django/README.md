# DBCrust Django ORM Query Analyzer

A powerful Django ORM query analyzer that detects performance issues, N+1 queries, and missing optimizations. Built on top of DBCrust's performance analysis infrastructure.

## Overview

The Django analyzer is included with DBCrust and provides enterprise-grade performance analysis for Django applications. It automatically detects common ORM anti-patterns and provides actionable recommendations.

For complete documentation, examples, and advanced usage, see the [Django Analyzer Guide](../../../docs/django-analyzer.md) in the main documentation.

## Quick Start

### Installation

The Django analyzer is included with DBCrust. Make sure you have Django installed:

```bash
pip install django
```

### Basic Usage

```python
from dbcrust.django import analyzer

# Analyze your Django ORM code
with analyzer.analyze() as analysis:
    # Your Django ORM code here
    books = Book.objects.all()
    for book in books:
        print(book.author.name)  # Potential N+1 query

# Get results
results = analysis.get_results()
print(results.summary)
```

### Key Features

- **N+1 Query Detection**: Automatically detects N+1 query patterns
- **Missing Optimization Detection**: Identifies missing `select_related()` and `prefetch_related()`
- **Performance Analysis**: Integrates with DBCrust for EXPLAIN ANALYZE insights
- **Transaction Safety**: Optional transaction rollback for safe analysis
- **Detailed Recommendations**: Django-specific optimization suggestions with code examples
- **Multiple Database Support**: Works with PostgreSQL, MySQL, and SQLite

## Documentation

For comprehensive documentation including:

- **Configuration Options**: Advanced analyzer settings
- **Detection Patterns**: All supported performance issue types
- **Integration Examples**: Development, testing, and production workflows
- **Best Practices**: How to integrate into your development process
- **Troubleshooting**: Common issues and solutions
- **Advanced Features**: DBCrust integration and custom patterns

Please see the complete [Django Analyzer Documentation](../../../docs/django-analyzer.md).

## Example Output

```
Django Query Analysis Summary
============================
Time Range: 14:30:25 - 14:30:27
Total Queries: 15
Total Duration: 245.67ms
Average Query Time: 16.38ms

Query Types:
  - SELECT: 14
  - INSERT: 1

⚠️  Duplicate Queries: 3

Performance Issues Detected:
  🔴 N Plus One: 1
  🟡 Missing Select Related: 2
  🟡 Large Result Set: 1

🚨 CRITICAL (1 issues):
   - Fix N+1 Query Problem

⚠️  HIGH (2 issues):
   - Use select_related() for Foreign Key Relationships
   - Use prefetch_related() for Many-to-Many Relationships
```