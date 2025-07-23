# Django ORM Query Analyzer

The Django ORM Query Analyzer is a powerful performance analysis tool built into DBCrust that automatically detects N+1 queries, missing optimizations, and provides actionable recommendations for Django applications.

## Overview

The analyzer captures Django ORM queries in real-time and identifies common performance anti-patterns:

- **N+1 Query Detection**: Identifies repeated queries that could be optimized
- **Missing select_related()**: Detects foreign key lookups that could be joined
- **Missing prefetch_related()**: Finds many-to-many patterns that need prefetching
- **Inefficient Operations**: Spots count operations and large result sets
- **Performance Insights**: Integrates with DBCrust's EXPLAIN ANALYZE for database-level analysis

## Quick Start

### Basic Analysis

```python
from dbcrust.django import analyzer

# Analyze Django ORM queries for performance issues
with analyzer.analyze() as analysis:
    # Your Django code here
    books = Book.objects.all()
    for book in books:
        print(book.author.name)  # Will detect N+1 query

# Get results
results = analysis.get_results()
print(results.summary)
```

### With Database Analysis

```python
# Include EXPLAIN ANALYZE for detailed query plans
with analyzer.analyze(dbcrust_url="postgres://localhost/mydb") as analysis:
    # Complex queries benefit from database-level analysis
    expensive_books = (
        Book.objects
        .select_related('author', 'publisher')
        .filter(price__gt=100)
        .order_by('-published_date')[:10]
    )
    list(expensive_books)

results = analysis.get_results()
print(results.summary)

# Export detailed report
analysis.export_results("performance_analysis.json")
```

## Configuration

### Analyzer Options

```python
from dbcrust.django import DjangoAnalyzer

analyzer = DjangoAnalyzer(
    dbcrust_url="postgres://localhost/mydb",  # Optional: enables EXPLAIN analysis
    transaction_safe=True,                    # Default: rollback after analysis
    enable_explain=True,                      # Default: run EXPLAIN ANALYZE
    database_alias='default'                  # Django database to analyze
)

with analyzer.analyze() as analysis:
    # Your queries here
    MyModel.objects.all().count()
```

### Safety Options

```python
# Safe analysis that won't affect your database
with analyzer.analyze(transaction_safe=True) as analysis:
    # Even data modifications will be rolled back
    MyModel.objects.create(name="test")
    MyModel.objects.filter(name="test").update(status="active")

# Changes are rolled back, but queries were analyzed
results = analysis.get_results()
```

## Detection Patterns

### N+1 Query Detection

**Problem Pattern:**
```python
# This creates N+1 queries (1 + N author lookups)
authors = Author.objects.all()           # 1 query
for author in authors:
    print(author.books.count())          # N queries
```

**Detected Issue:**
- Pattern Type: `n_plus_one`
- Severity: `critical`
- Description: "N+1 query pattern detected: 15 separate queries for related objects"

**Recommendation:**
```python
# Fixed with prefetch_related
authors = Author.objects.prefetch_related('books')
for author in authors:
    print(author.books.count())  # No additional queries
```

### Missing select_related Detection

**Problem Pattern:**
```python
# Sequential foreign key lookups
orders = Order.objects.all()
for order in orders:
    print(order.customer.name)  # Triggers query for each order
```

**Detected Issue:**
- Pattern Type: `missing_select_related`
- Severity: `high`
- Recommendation: "Use select_related() to fetch related objects in a single query"

**Fix:**
```python
# Optimized with select_related
orders = Order.objects.select_related('customer')
for order in orders:
    print(order.customer.name)  # No additional queries
```

### Missing prefetch_related Detection

**Problem Pattern:**
```python
# Many-to-many relationship queries
authors = Author.objects.all()
for author in authors:
    book_titles = [book.title for book in author.books.all()]  # N queries
```

**Fix:**
```python
# Optimized with prefetch_related
authors = Author.objects.prefetch_related('books')
for author in authors:
    book_titles = [book.title for book in author.books.all()]  # 2 queries total
```

### Other Detected Patterns

- **Inefficient Count**: Using `len(queryset)` instead of `queryset.count()`
- **Missing Field Limits**: Fetching all fields when only few are needed (`only()`, `defer()`)
- **Large Result Sets**: Queries without `LIMIT` that could cause memory issues
- **Unnecessary Ordering**: `ORDER BY` without `LIMIT` that may be unneeded

## Understanding Results

### Analysis Summary

```python
results = analysis.get_results()
print(results.summary)
```

**Example Output:**
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

### Detailed Results

```python
results = analysis.get_results()

# Basic metrics
print(f"Total queries: {results.total_queries}")
print(f"Total time: {results.total_duration * 1000:.2f}ms")
print(f"Duplicates: {results.duplicate_queries}")

# Query breakdown
for query_type, count in results.queries_by_type.items():
    print(f"{query_type}: {count} queries")

# Detected issues
for pattern in results.detected_patterns:
    print(f"\nIssue: {pattern.pattern_type}")
    print(f"Severity: {pattern.severity}")
    print(f"Description: {pattern.description}")
    print(f"Affected queries: {len(pattern.affected_queries)}")
    print(f"Recommendation: {pattern.recommendation}")
    
    if pattern.code_suggestion:
        print(f"Fix: {pattern.code_suggestion}")

# Optimization recommendations
for rec in results.recommendations:
    print(f"\n{rec.title} ({rec.impact} impact, {rec.difficulty} difficulty)")
    print(f"Description: {rec.description}")
    
    if rec.code_before and rec.code_after:
        print("Before:")
        print(rec.code_before)
        print("After:")
        print(rec.code_after)
```

## Integration Scenarios

### Development Workflow

```python
# Add to your development middleware or views
from django.conf import settings

if settings.DEBUG:
    from dbcrust.django import analyzer
    
    def my_view(request):
        with analyzer.analyze() as analysis:
            # Your view logic
            context = get_context_data()
            return render(request, 'template.html', context)
        
        # Log performance issues in development
        results = analysis.get_results()
        if results.detected_patterns:
            logger.warning(f"Performance issues: {len(results.detected_patterns)}")
```

### Performance Testing

```python
# In your test suite
from django.test import TestCase
from dbcrust.django import analyzer

class PerformanceTestCase(TestCase):
    def test_view_has_no_n_plus_one(self):
        with analyzer.analyze() as analysis:
            response = self.client.get('/books/')
            self.assertEqual(response.status_code, 200)
        
        results = analysis.get_results()
        
        # Assert no N+1 queries
        n_plus_one = [p for p in results.detected_patterns 
                     if p.pattern_type == 'n_plus_one']
        self.assertEqual(len(n_plus_one), 0, 
                        "View should not have N+1 queries")
        
        # Assert reasonable query count
        self.assertLess(results.total_queries, 10,
                       "View should use fewer than 10 queries")
```

### Production Monitoring

```python
# Monitor critical code paths in production
import logging
from dbcrust.django import analyzer

def process_user_dashboard(user_id):
    """Critical function that should be optimized."""
    with analyzer.analyze(transaction_safe=True) as analysis:
        # Dashboard logic
        user = User.objects.select_related('profile').get(id=user_id)
        recent_orders = user.orders.prefetch_related('items').recent()
        recommendations = get_product_recommendations(user)
        
        return {
            'user': user,
            'orders': recent_orders,
            'recommendations': recommendations
        }
    
    # Log performance metrics
    results = analysis.get_results()
    if results.total_queries > 5:
        logging.warning(f"Dashboard used {results.total_queries} queries for user {user_id}")
    
    if results.detected_patterns:
        logging.error(f"Performance issues in dashboard: {len(results.detected_patterns)}")
```

## DBCrust Integration

When you provide a `dbcrust_url`, the analyzer gains additional capabilities:

### EXPLAIN ANALYZE Integration

```python
with analyzer.analyze(dbcrust_url="postgres://localhost/mydb") as analysis:
    # Complex query that benefits from EXPLAIN analysis
    complex_books = (
        Book.objects
        .select_related('author', 'publisher', 'author__country')
        .prefetch_related('categories', 'reviews__reviewer')
        .filter(
            published_date__year__gte=2020,
            price__between=(20, 100),
            author__country__name='USA'
        )
        .order_by('-published_date', 'price')[:50]
    )
    
    list(complex_books)  # Force evaluation

results = analysis.get_results()

# Database-level insights
if results.dbcrust_analysis:
    print(f"Analyzed {results.dbcrust_analysis['analyzed_queries']} queries with EXPLAIN")
    print("\nDatabase Performance Report:")
    print(results.dbcrust_analysis['performance_report'])
```

### Performance Insights

The DBCrust integration provides:

- **Query Plans**: Detailed execution plans for slow queries
- **Cost Analysis**: Database cost estimates and actual timings
- **Index Recommendations**: Suggestions for missing indexes
- **Join Analysis**: Optimization opportunities for complex joins
- **Database-Specific Tips**: PostgreSQL, MySQL, and SQLite optimizations

## Best Practices

### 1. Use in Development

Always run the analyzer during development to catch issues early:

```python
# Add to Django settings for development
if DEBUG:
    # Enable query analysis for all requests
    MIDDLEWARE = [
        'myapp.middleware.QueryAnalysisMiddleware',
    ] + MIDDLEWARE
```

### 2. Focus on Critical Paths

Analyze your most important code paths:

```python
# Analyze key business functions
with analyzer.analyze() as analysis:
    process_checkout(cart_id)  # Critical e-commerce path
    
results = analysis.get_results()
if results.detected_patterns:
    # Alert developers to issues in critical paths
    send_performance_alert(results)
```

### 3. Set Performance Budgets

```python
def test_homepage_performance(self):
    with analyzer.analyze() as analysis:
        response = self.client.get('/')
    
    results = analysis.get_results()
    
    # Performance budget assertions
    self.assertLess(results.total_queries, 5, "Homepage should use < 5 queries")
    self.assertLess(results.total_duration, 0.1, "Homepage should take < 100ms")
    
    # No critical issues allowed
    critical_issues = [p for p in results.detected_patterns if p.severity == 'critical']
    self.assertEqual(len(critical_issues), 0, "No critical performance issues allowed")
```

### 4. Continuous Integration

```python
# In your CI pipeline
import sys
from dbcrust.django import analyzer

def analyze_test_performance():
    """Run performance analysis as part of CI."""
    with analyzer.analyze() as analysis:
        # Run your test scenarios
        run_integration_tests()
    
    results = analysis.get_results()
    
    # Fail CI if critical issues found
    critical_issues = [p for p in results.detected_patterns if p.severity == 'critical']
    if critical_issues:
        print(f"❌ Found {len(critical_issues)} critical performance issues")
        for issue in critical_issues:
            print(f"   - {issue.description}")
        sys.exit(1)
    
    print(f"✅ Performance analysis passed: {results.total_queries} queries in {results.total_duration*1000:.1f}ms")
```

## Common Optimization Patterns

### 1. Book Library Example

```python
# Before: N+1 queries
def list_books_bad():
    books = Book.objects.all()
    for book in books:
        print(f"{book.title} by {book.author.name}")  # N+1 queries
        print(f"Publisher: {book.publisher.name}")     # More N+1 queries
        print(f"Categories: {', '.join(c.name for c in book.categories.all())}")  # Even more N+1

# After: Optimized
def list_books_good():
    books = (
        Book.objects
        .select_related('author', 'publisher')        # Join author and publisher
        .prefetch_related('categories')                # Prefetch categories
    )
    for book in books:
        print(f"{book.title} by {book.author.name}")  # No additional queries
        print(f"Publisher: {book.publisher.name}")     # No additional queries
        print(f"Categories: {', '.join(c.name for c in book.categories.all())}")  # No additional queries
```

### 2. E-commerce Dashboard

```python
# Before: Multiple inefficiencies
def user_dashboard_bad(user_id):
    user = User.objects.get(id=user_id)
    orders = user.orders.all()  # Will fetch all orders
    
    recent_orders = []
    for order in orders:
        if order.created_at > recent_date:
            order_items = []
            for item in order.items.all():  # N+1 queries
                order_items.append({
                    'product': item.product.name,  # N+1 queries
                    'price': item.price
                })
            recent_orders.append({
                'id': order.id,
                'items': order_items,
                'total': sum(item.price for item in order.items.all())  # More N+1
            })
    
    return {'user': user, 'orders': recent_orders}

# After: Optimized
def user_dashboard_good(user_id):
    user = User.objects.select_related('profile').get(id=user_id)
    
    recent_orders = (
        user.orders
        .filter(created_at__gt=recent_date)
        .prefetch_related('items__product')  # Prefetch items and their products
        .order_by('-created_at')[:10]        # Limit results
    )
    
    orders_data = []
    for order in recent_orders:
        items_data = [
            {
                'product': item.product.name,    # No additional queries
                'price': item.price
            }
            for item in order.items.all()       # No additional queries
        ]
        orders_data.append({
            'id': order.id,
            'items': items_data,
            'total': sum(item.price for item in order.items.all())  # Still no additional queries
        })
    
    return {'user': user, 'orders': orders_data}
```

## Troubleshooting

### Common Issues

1. **No queries captured**
   - Ensure Django is properly configured
   - Verify you're executing ORM queries within the context manager
   - Check that the database alias exists

2. **Transaction errors**
   - Try setting `transaction_safe=False`
   - Ensure no open transactions before analysis
   - Check database permissions

3. **DBCrust connection issues**
   - Verify the `dbcrust_url` format
   - Ensure database is accessible
   - Check credentials and permissions

### Debug Mode

```python
# Enable verbose debugging
import logging
logging.basicConfig(level=logging.DEBUG)

with analyzer.analyze() as analysis:
    # Your code here
    MyModel.objects.all().count()

# Print detailed query information
analysis.print_queries(verbose=True)
```

### Performance Impact

The analyzer has minimal performance impact:

- **Query Capture**: ~1-2% overhead per query
- **Pattern Analysis**: Runs after query execution
- **Memory Usage**: Stores query metadata only
- **Transaction Mode**: Safe rollback prevents data changes

## Advanced Features

### Custom Pattern Detection

You can extend the analyzer with custom patterns:

```python
# Example: Detect queries in loops
def detect_queries_in_loops(queries):
    """Custom pattern detector for queries inside loops."""
    # Implementation would analyze stack traces for loop patterns
    pass
```

### Integration with Monitoring

```python
# Send metrics to monitoring systems
def send_performance_metrics(results):
    """Send analysis results to monitoring system."""
    metrics = {
        'query_count': results.total_queries,
        'duration_ms': results.total_duration * 1000,
        'n_plus_one_count': len([p for p in results.detected_patterns 
                                if p.pattern_type == 'n_plus_one']),
        'duplicate_count': results.duplicate_queries
    }
    
    # Send to your monitoring system
    monitoring_client.send_metrics('django.orm.performance', metrics)
```

The Django ORM Query Analyzer is a powerful tool for identifying and fixing performance issues in Django applications. By integrating it into your development workflow, you can catch and resolve performance problems before they impact your users.