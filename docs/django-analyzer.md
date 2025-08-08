# Django ORM Query Analyzer

The Django ORM Query Analyzer is a comprehensive performance analysis tool built into DBCrust that automatically detects Django ORM anti-patterns, provides detailed EXPLAIN analysis, and delivers actionable recommendations with precise code locations.

## Overview

DBCrust's Django analyzer provides comprehensive performance analysis through multiple layers:

- **🔍 Runtime Query Analysis**: N+1 detection, missing optimizations, duplicate queries
- **💻 AST-Based Code Analysis**: Precise line numbers and code context extraction
- **🏗️ Project-Wide Analysis**: Model relationship mapping and optimization scoring
- **📊 Database EXPLAIN Integration**: Deep query plan analysis through DBCrust
- **🎯 Enhanced Recommendations**: Before/after code examples with difficulty ratings

## Quick Start

### 🚀 **Instant Setup with Middleware (Recommended)**

The fastest way to start analyzing your Django application's performance:

```python
# settings.py - Add this one line
MIDDLEWARE = [
    # ... your existing middlewares
    'dbcrust.django.PerformanceAnalysisMiddleware',
]
```

**That's it!** Every request now gets automatic performance analysis with:

> **🛠️ Debug Toolbar Compatibility:** The middleware automatically detects Django Debug Toolbar and disables itself to
> avoid profiling conflicts. Use `'DEBUG_TOOLBAR_COMPATIBILITY': False` to force enable both tools.

- **Real-time N+1 detection** in your browser's developer tools
- **Performance headers** showing query counts and timing
- **Smart categorization** separating user code from framework issues
- **Full file paths** for easy navigation to problematic code
- **Admin-specific recommendations** for Django admin performance
- **Console logging** for requests with performance issues
- **Zero code changes** required in views or models

#### **See Results Immediately**

1. **Browser Developer Tools** → Network tab → Select any request → Response Headers:
   ```
   X-DBCrust-Query-Count: 8
   X-DBCrust-Query-Time: 45.2ms
   X-DBCrust-Status: OK
   ```

2. **Django Console** (when issues detected):
   ```bash
   WARNING:dbcrust.performance: GET /admin/blog/post/ | queries=3 | db_time=4.4ms | issues=6

   📋 USER CODE ISSUES:

   🔸 large_result_set (2x) - medium - Tables: blog_post
      SQL: SELECT COUNT(*) AS "__count" FROM "blog_post"
      Primary: /path/to/django/contrib/admin/filters.py:613 in choices (Django Admin)
      Context: /path/to/django/contrib/admin/templatetags/admin_list.py:514 in admin_list_filter (Django Admin)
      Fix: Django admin filter query detected. Update your blog/admin.py file: Your list_filter fields are generating large result sets

   🔸 inefficient_count - medium - Tables: blog_category
      SQL: SELECT DISTINCT "blog_category"."name" AS "name" FROM "blog_category" ORDER BY 1 ASC
      Primary: /path/to/django/contrib/admin/filters.py:613 in choices (Django Admin)
      Fix: Django admin filter counting detected. Update your blog/admin.py file: Your list_filter is counting large datasets

   ⚙️  FRAMEWORK INSIGHTS: (0 detected)

   📊 Summary: 3 user code issues, 0 framework insights (total: 3)
   ```

   > **⚡ Key Insight:** Even when queries execute in Django admin framework code, they're **YOUR RESPONSIBILITY** to fix
   via admin configuration. The issues above are caused by your basic `PostAdmin` not being optimized. You need to
   configure pagination, filtering, and query optimization in your admin class!

   **The Problem Explained:**
   Your `list_filter = ('published', 'category')` is causing Django admin to generate expensive queries counting how many items exist for each filter option. Note that **normal pagination COUNT queries** are NOT flagged - the analyzer only detects problematic filter counting patterns.

   **Fix Your blog/admin.py:**
   ```python
   @admin.register(Post)
   class PostAdmin(ModelAdmin):
       readonly_fields = ("author", "created_at")

       # BEFORE: Heavy foreign key filter causes counting queries
       # list_filter = ('published', 'category')

       # AFTER: Optimized admin configuration
       list_per_page = 25  # Reduce page size
       show_full_result_count = False  # Disable expensive counting
       list_filter = ('published',)  # Keep simple filters only
       search_fields = ('title', 'author__username')  # Add search instead
       autocomplete_fields = ('category', 'author')  # Use autocomplete for FK fields
       list_select_related = ('category', 'author')  # Optimize related queries
   ```

   **Alternative:** If you need category filtering, register Category for autocomplete:
   ```python
   @admin.register(Category)
   class CategoryAdmin(ModelAdmin):
       search_fields = ('name',)  # Required for autocomplete
   ```

   > **✅ What Won't Trigger Issues:**
   > - Normal pagination COUNT queries (`list_per_page` working correctly)
   > - Simple boolean filters like `list_filter = ('published',)`
   > - Properly configured `show_full_result_count = False`
   > - Regular admin list/change views without heavy foreign key filters

3. **Optional Configuration** for customization:
   ```python
   DBCRUST_PERFORMANCE_ANALYSIS = {
       'QUERY_THRESHOLD': 1,      # Log requests with >1 queries (default: 10)
       'TIME_THRESHOLD': 1,       # Log requests taking >1ms (default: 100ms)
       'MAX_ISSUES_DISPLAYED': 20,# Show up to 20 issues in logs (default: 10)
       'SHOW_SQL_IN_LOGS': True,  # Show SQL queries in logs (default: True)
       'GROUP_DUPLICATE_ISSUES': True,  # Group same issues (default: True)
       'MAX_SQL_LENGTH': 200,     # Max SQL length in logs (default: 200)
       'TRANSACTION_SAFE': False, # Avoid session conflicts (default: False)
       'DEBUG_TOOLBAR_COMPATIBILITY': True,  # Auto-disable with Debug Toolbar (default: True)
   }

   # IMPORTANT: Add logging configuration to see performance issues
   LOGGING = {
       'version': 1,
       'disable_existing_loggers': False,
       'formatters': {
           'verbose': {
               'format': '{levelname} {asctime} {name}: {message}',
               'style': '{',
           },
       },
       'handlers': {
           'console': {
               'class': 'logging.StreamHandler',
               'formatter': 'verbose',
           },
       },
       'loggers': {
           'dbcrust.performance': {
               'handlers': ['console'],
               'level': 'INFO',
               'propagate': False,
           },
       },
   }
   ```

4. **See All Issues (Complete Output):**
   ```python
   # Method 1: Increase the display limit
   DBCRUST_PERFORMANCE_ANALYSIS = {
       'MAX_ISSUES_DISPLAYED': 50,  # Show up to 50 issues per request
   }

   # Method 2: Use the management command for detailed analysis
   python manage.py dbcrust_analyze --model-query "MyModel.objects.all()" --all-issues --verbose
   ```

5. **Django Debug Toolbar Compatibility:**
   ```python
   # Middleware order matters - Debug Toolbar should come first
   MIDDLEWARE = [
       'debug_toolbar.middleware.DebugToolbarMiddleware',  # First
       'dbcrust.django.PerformanceAnalysisMiddleware',     # Second
       # ... other middleware
   ]

   # To use both tools together (not recommended - may cause conflicts):
   DBCRUST_PERFORMANCE_ANALYSIS = {
       'DEBUG_TOOLBAR_COMPATIBILITY': False,
   }
   ```

---

### Basic Runtime Analysis

```python
from dbcrust.django import analyzer

# Analyze Django ORM queries for performance issues
with analyzer.analyze() as analysis:
    # Your Django code here - analyzer captures all queries
    books = Book.objects.all()
    for book in books:
        print(book.author.name)  # Will detect N+1 query
        print(f"Reviews: {book.reviews.count()}")  # Detects missing prefetch_related

# Get comprehensive results with enhanced context
results = analysis.get_results()
print(results.summary)
```

**Enhanced Output Example:**

```
🔍 Detailed Analysis with Specific Recommendations:
============================================================

1. N Plus One - HIGH
   💡 Suggested fields: 'author'
   📍 Code locations: views.py:15
   ⚡ Quick fix: Book.objects.select_related('author')
   📈 Impact: Could reduce 15 queries to 1 query

2. Missing Prefetch Related - MEDIUM
   💡 Suggested fields: 'reviews'
   📍 Code locations: views.py:16
   ⚡ Quick fix: Book.objects.prefetch_related('reviews')
```

### Comprehensive Analysis with Code Scanning

```python
from dbcrust.django.analyzer import create_enhanced_analyzer

# Full-featured analyzer with all capabilities
analyzer = create_enhanced_analyzer(
    dbcrust_url="postgres://localhost/mydb",  # EXPLAIN ANALYZE integration
    project_root="/path/to/django/project",  # Code analysis & model scanning
    enable_all_features=True  # Enable all enhancements
)

# Runtime query analysis
with analyzer.analyze() as analysis:
    # Your Django code here
    expensive_books = (
        Book.objects
        .select_related('author', 'publisher')
        .filter(price__gt=100)
        .order_by('-published_date')[:10]
    )
    list(expensive_books)

# Get comprehensive analysis including:
# - Runtime query patterns
# - Code analysis across entire project
# - Model relationship analysis
# - Database EXPLAIN insights
comprehensive_report = analysis.generate_comprehensive_report()
print(comprehensive_report)
```

### Project-Wide Analysis (No Runtime Required)

```python
from dbcrust.django.analyzer import analyze_django_project

# Analyze entire Django project without running queries
results = analyze_django_project(
    project_root="/path/to/django/project",
    dbcrust_url="postgres://localhost/mydb"  # Optional: for model analysis
)

print(f"📊 Optimization Score: {results['optimization_score']}/100")
print(f"🔍 Code Issues Found: {len(results['code_issues'])}")
print(f"🏗️ Models Analyzed: {len(results['models'])}")

# Get detailed recommendations
for recommendation in results['recommendations']:
    print(f"• {recommendation}")
```

## Configuration

### Analyzer Options

```python
from dbcrust.django import DjangoAnalyzer

analyzer = DjangoAnalyzer(
    dbcrust_url="postgres://localhost/mydb",  # Optional: enables EXPLAIN analysis
    transaction_safe=True,  # Default: rollback after analysis
    enable_explain=True,  # Default: run EXPLAIN ANALYZE
    database_alias='default'  # Django database to analyze
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

## Enhanced Detection Patterns

The analyzer now detects **12+ Django ORM anti-patterns** with precise code locations and actionable fixes:

### 🚨 **Critical Performance Issues**

#### N+1 Query Detection (Enhanced)

**Problem Pattern:**

```python
# views.py:25 - This creates N+1 queries (1 + N author lookups)
authors = Author.objects.all()  # 1 query
for author in authors:  # Line 26 - detected by AST analysis
    print(author.books.count())  # N queries - specific field identified
```

**Enhanced Detection:**

- Pattern Type: `n_plus_one`
- Severity: `critical`
- **Line Number**: `views.py:26` (exact location)
- **Suggested Fields**: `['books']` (specific relationship)
- **Code Context**: Shows problematic loop structure
- **Impact**: "Could reduce 15 queries to 2 queries (93% improvement)"

**Detailed Recommendation with Code Examples:**

```python
# BEFORE (problematic):
authors = Author.objects.all()
for author in authors:
    print(author.books.count())

# AFTER (optimized):
authors = Author.objects.prefetch_related('books')
for author in authors:
    print(author.books.count())  # No additional queries
```

#### Missing select_related Detection (Enhanced)

**Problem Pattern:**

```python
# views.py:42 - Sequential foreign key lookups
orders = Order.objects.all()
for order in orders:  # Line 43 - AST detected loop
    print(order.customer.name)  # Line 44 - specific access pattern
    print(order.customer.email)  # Line 45 - multiple field access
```

**Enhanced Detection:**

- Pattern Type: `missing_select_related`
- Severity: `high`
- **Exact Location**: `views.py:44-45`
- **Suggested Fields**: `['customer']` (relationship identified)
- **Table Context**: `{'order': ['customer_id']}`
- **Migration Suggestion**: Shows index recommendations

### ⚠️ **High Priority Optimizations**

#### Missing prefetch_related Detection

```python
# Detected: Many-to-many or reverse ForeignKey access in loops
authors = Author.objects.all()
for author in authors:
    book_titles = [book.title for book in author.books.all()]  # Detected pattern
```

#### Subqueries in Loops (New)

```python
# Detected: Database queries inside Python loops
for category in categories:
    popular_books = Book.objects.filter(
        category=category, rating__gte=4.0  # Query in loop - line-level detection
    )[:5]
```

#### Missing Database Indexes (New)

```python
# Detected through EXPLAIN analysis: sequential scans on large tables
slow_books = Book.objects.filter(category_name="Fiction")  # Missing index detected
```

**Generated Migration:**

```python
# Automatic migration code generation
operations = [
    migrations.RunSQL(
        'CREATE INDEX CONCURRENTLY idx_book_category_name ON book (category_name);'
    ),
]
```

#### Inefficient Aggregations (New)

```python
# Detected: Manual aggregation that could use database functions
total_price = 0
for book in books:  # Line-level detection
    total_price += book.price  # Should use aggregate()

# Recommended fix:
total_price = books.aggregate(Sum('price'))['price__sum']
```

### 📊 **Performance Optimizations**

#### Bulk Operations Detection (New)

```python
# Detected: Individual saves that could be bulk operations
books_to_update = []
for book in Book.objects.filter(price__lt=20):
    book.price = book.price * 1.1
    book.save()  # Individual saves - bulk opportunity detected

# Recommended:
Book.objects.filter(price__lt=20).update(price=F('price') * 1.1)
```

#### Large Result Sets Without Pagination (Enhanced)

```python
# Detected: Queries fetching large datasets without limits
all_users = User.objects.all()  # Memory risk detected
user_count = len(list(all_users))  # Should use .count()
```

#### Count vs Length Optimization (New)

```python
# Detected: Using len() instead of count() - performance impact calculated
book_count = len(Book.objects.all())  # Inefficient - loads all records
# Fix: Book.objects.count()           # Database-level count
```

#### Missing Only/Defer Fields (New)

```python
# Detected: Fetching unnecessary fields
users = User.objects.all()  # Fetches all fields
for user in users:
    print(user.username)  # Only username needed

# Recommended:
users = User.objects.only('username')  # Fetch only required fields
```

#### Query Result Caching Opportunities (New)

```python
# Detected: Repeated identical queries within request
def view_function(request):
    active_users = User.objects.filter(is_active=True)  # Query 1
    # ... some logic
    active_users = User.objects.filter(is_active=True)  # Duplicate detected
```

### 🏗️ **Database & Architecture Issues**

#### Transaction Race Conditions (New)

```python
# Detected: Operations that could cause race conditions
user = User.objects.get(id=user_id)
user.credits = user.credits + 10  # Race condition risk
user.save()  # Should use F() expressions
```

#### Connection Pooling Issues (New)

```python
# Detected through EXPLAIN analysis: inefficient connection patterns
# Recommendations for connection pooling configuration
```

#### Template N+1 Patterns (New)

```django
<!-- templates/books.html:15 - Template N+1 detected -->
{% for book in books %}
    <h3>{{ book.title }}</h3>
    <p>Author: {{ book.author.name }}</p>  <!-- N+1 in template -->
    <p>Reviews: {{ book.reviews.count }}</p> <!-- Additional N+1 -->
{% endfor %}
```

**Template Fix Recommendation:**

```python
# In view: books = Book.objects.select_related('author').prefetch_related('reviews')
```

### 🎯 **Enhanced Context & Recommendations**

Each detected pattern now includes:

- **📍 Exact Line Numbers**: `views.py:42`, `models.py:156`
- **💡 Specific Field Suggestions**: `['author', 'publisher', 'reviews']`
- **🗃️ Table Context**: `{'book': ['author_id', 'publisher_id']}`
- **⚡ Quick Fix Code**: Ready-to-use code snippets
- **📈 Performance Impact**: Quantified improvements ("80% query reduction")
- **🔧 Migration Code**: Auto-generated Django migrations
- **📚 Reference Links**: Documentation and best practice guides
- **🎚️ Difficulty Rating**: `easy` | `medium` | `hard`
- **🎯 Impact Rating**: `low` | `medium` | `high` | `critical`

## Enhanced Usage Examples

### Real-World Django Optimization Scenarios

#### E-commerce Product Catalog

**Before (Multiple Performance Issues):**

```python
# views.py - Product listing with multiple issues
def product_list(request):
    products = Product.objects.all()  # Line 5: Large result set

    product_data = []
    for product in products:  # Line 8: N+1 patterns ahead
        category_name = product.category.name  # Line 9: Missing select_related
        brand_name = product.brand.name  # Line 10: Another select_related issue

        review_count = 0
        total_rating = 0
        for review in product.reviews.all():  # Line 14: Missing prefetch_related
            review_count += 1  # Line 15: Manual aggregation
            total_rating += review.rating  # Line 16: Should use aggregate()

        avg_rating = total_rating / review_count if review_count > 0 else 0

        product_data.append({
            'name': product.name,
            'category': category_name,
            'brand': brand_name,
            'avg_rating': avg_rating,
            'review_count': review_count
        })

    return render(request, 'products.html', {'products': product_data})
```

**Enhanced Analyzer Detection:**

```
🔍 Detailed Analysis with Specific Recommendations:
============================================================

1. Large Result Set - CRITICAL
   📍 Code locations: views.py:5
   ⚡ Quick fix: Product.objects.all()[:50] or add pagination
   📈 Impact: Prevents memory issues with large datasets

2. Missing Select Related - HIGH
   💡 Suggested fields: 'category', 'brand'
   📍 Code locations: views.py:9, views.py:10
   ⚡ Quick fix: Product.objects.select_related('category', 'brand')
   📈 Impact: Could reduce 200+ queries to 1 query

3. Missing Prefetch Related - HIGH
   💡 Suggested fields: 'reviews'
   📍 Code locations: views.py:14
   ⚡ Quick fix: Product.objects.prefetch_related('reviews')
   📈 Impact: Could reduce 100+ queries to 2 queries

4. Inefficient Aggregation - MEDIUM
   📍 Code locations: views.py:15-16
   ⚡ Quick fix: Use aggregate(Count('reviews'), Avg('reviews__rating'))
   📈 Impact: Database-level aggregation instead of Python loops
```

**After (Fully Optimized):**

```python
# views.py - Optimized version following analyzer recommendations
from django.core.paginator import Paginator
from django.db.models import Count, Avg


def product_list_optimized(request):
    # Fix 1: Add pagination to handle large result sets
    # Fix 2: Use select_related for foreign key relationships
    # Fix 3: Use prefetch_related for reverse relationships
    # Fix 4: Use database aggregation
    products = (
        Product.objects
        .select_related('category', 'brand')  # Fixes lines 9-10
        .prefetch_related('reviews')  # Fixes line 14
        .annotate(
            review_count=Count('reviews'),  # Fixes line 15
            avg_rating=Avg('reviews__rating')  # Fixes line 16
        )
    )

    # Add pagination (fixes line 5)
    paginator = Paginator(products, 25)
    page_number = request.GET.get('page')
    page_products = paginator.get_page(page_number)

    # No loops needed - all data fetched efficiently
    return render(request, 'products.html', {
        'products': page_products,
        'paginator': paginator
    })
```

#### User Dashboard with Complex Relationships

**Before (N+1 Nightmare):**

```python
# views.py - Dashboard with multiple relationship issues
def user_dashboard(request):
    user = User.objects.get(id=request.user.id)

    # Get user's orders
    orders = user.orders.all()
    order_data = []

    for order in orders:  # Line 8: N+1 ahead
        # Get order items (N+1 issue #1)
        items = []
        for item in order.items.all():  # Line 11: Missing prefetch
            product_name = item.product.name  # Line 12: N+1 issue #2
            category = item.product.category.name  # Line 13: N+1 issue #3
            items.append({
                'name': product_name,
                'category': category,
                'price': item.price
            })

        # Get shipping info (N+1 issue #4)
        shipping_cost = order.shipping.cost  # Line 20: Missing select_related

        order_data.append({
            'id': order.id,
            'items': items,
            'shipping_cost': shipping_cost,
            'total': order.total
        })

    return render(request, 'dashboard.html', {'orders': order_data})
```

**Comprehensive Analysis Results:**

```python
# Generated by: analysis.generate_comprehensive_report()

🔍 Comprehensive
Django
ORM
Analysis
Report
== == == == == == == == == == == == == == == == == == == == == == == == ==

📈 Query
Analysis:
- Total
Queries: 247
- Total
Duration: 1, 840.5
ms
- Duplicate
Queries: 0
- Patterns
Detected: 4

🎯 Priority
Recommendations:

🚨 Critical / High
Priority(4
issues):
1.
Fix
N + 1
Query
Problem(query_analysis) - Lines: 11, 12, 13, 20
2.
Use
select_related()
for Foreign Key Relationships (query_analysis)
3.
Use
prefetch_related()
for Many - to - Many Relationships(query_analysis)
    4.
Add
Database
Indexes(explain_analysis) - Missing
index
on
orders.user_id

📊 Overall
Assessment:
🚨 Action
Required: Multiple
critical
performance
issues
detected
Total
Recommendations: 6
Critical / High
Priority: 4
```

**After (Optimized with Comprehensive Prefetching):**

```python
# views.py - Following all analyzer recommendations
def user_dashboard_optimized(request):
    user = User.objects.select_related('profile').get(id=request.user.id)

    # Single optimized query with all relationships prefetched
    orders = (
        user.orders
        .select_related('shipping')  # Fix line 20
        .prefetch_related(
            'items__product__category'  # Fix lines 11-13 combined
        )
        .order_by('-created_at')[:10]  # Limit recent orders
    )

    # No database queries in loops - everything pre-fetched
    order_data = []
    for order in orders:
        items = [
            {
                'name': item.product.name,  # No query - prefetched
                'category': item.product.category.name,  # No query - prefetched
                'price': item.price
            }
            for item in order.items.all()  # No query - prefetched
        ]

        order_data.append({
            'id': order.id,
            'items': items,
            'shipping_cost': order.shipping.cost,  # No query - selected
            'total': order.total
        })

    return render(request, 'dashboard.html', {'orders': order_data})
```

**Performance Improvement:**

```
Before: 247 queries in 1,840ms
After:  2 queries in 45ms
Improvement: 99.2% fewer queries, 97.6% faster
```

### Project-Wide Analysis Example

```python
from dbcrust.django.analyzer import generate_optimization_report

# Generate comprehensive project report
report = generate_optimization_report(
    project_path="/path/to/django/project",
    output_file="optimization_report.md"
)

print(report)
```

**Generated Report:**

```markdown
Django Project Analysis Summary
===============================
Project: ecommerce_site
Optimization Score: 34.2/100

📊 **Project Statistics:**

- Django Apps: 5
- Models: 23
- Code Issues Found: 42
- Model Relationships: 67

🔍 **Issues by Severity:**

- Critical: 8
- High: 15
- Medium: 12
- Low: 7

🏗️ **Model Analysis:**

- Models with relationships: 18/23
- Models with custom indexes: 3/23

🎯 **Priority Recommendations:**

1. 📊 **Database Indexes**: Add 15 recommended indexes
2. 🚨 **Critical Issues**: Fix 8 critical performance issues
3. ⚠️ **High Priority**: Address 15 high-priority optimizations
4. 🏷️ **Model Optimization**: 15 models need index optimization
5. 📝 **Template Optimization**: Fix 6 template-level N+1 patterns

🚨 **Action Required**: This project has significant optimization opportunities
```

## Understanding Results

### Enhanced Analysis Summary

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

## Enhanced Integration Workflows

### Continuous Integration Pipeline

```python
# ci/performance_check.py - Automated performance testing
from dbcrust.django.analyzer import analyze_django_project
import sys


def check_project_performance():
    """CI check for Django ORM performance issues."""

    # Analyze entire project without runtime execution
    results = analyze_django_project("/app")

    # Define performance thresholds
    critical_issues = [issue for issue in results['code_issues']
                       if issue.severity == 'critical']

    optimization_score = results['optimization_score']

    # Fail CI for critical issues or low optimization score
    if critical_issues:
        print(f"❌ {len(critical_issues)} critical performance issues found:")
        for issue in critical_issues[:5]:  # Show top 5
            print(f"   {issue.file_path}:{issue.line_number} - {issue.description}")
        sys.exit(1)

    if optimization_score < 70:
        print(f"⚠️ Optimization score too low: {optimization_score}/100")
        print("Consider addressing high-priority recommendations")
        sys.exit(1)

    print(f"✅ Performance check passed - Score: {optimization_score}/100")
    print(f"📊 Models analyzed: {len(results['models'])}")
    print(f"🔍 Total issues: {len(results['code_issues'])}")


if __name__ == "__main__":
    check_project_performance()
```

### Performance Testing Integration

```python
# tests/test_performance.py - Enhanced performance test suite
from django.test import TestCase, override_settings
from dbcrust.django.analyzer import create_enhanced_analyzer
from django.test.utils import override_settings


class PerformanceTestCase(TestCase):
    def setUp(self):
        """Set up enhanced analyzer for all tests."""
        self.analyzer = create_enhanced_analyzer(
            enable_all_features=True,
            project_root=settings.BASE_DIR
        )

    def test_homepage_performance_budget(self):
        """Test homepage meets performance budget."""
        with self.analyzer.analyze() as analysis:
            response = self.client.get('/')
            self.assertEqual(response.status_code, 200)

        results = analysis.get_results()

        # Strict performance budget
        self.assertLess(results.total_queries, 5,
                        "Homepage exceeded query budget (5 queries)")
        self.assertLess(results.total_duration, 0.050,  # 50ms
                        "Homepage exceeded time budget (50ms)")

        # No critical performance issues allowed
        critical_patterns = [p for p in results.detected_patterns
                             if p.severity == 'critical']
        self.assertEqual(len(critical_patterns), 0,
                         f"Critical performance issues found: {[p.description for p in critical_patterns]}")

    def test_product_listing_optimization(self):
        """Test product listing is properly optimized."""
        with self.analyzer.analyze() as analysis:
            response = self.client.get('/products/')

        results = analysis.get_results()
        comprehensive = analysis.get_comprehensive_analysis()

        # Check for specific patterns
        n_plus_one = [p for p in results.detected_patterns
                      if p.pattern_type == 'n_plus_one']
        self.assertEqual(len(n_plus_one), 0, "N+1 queries detected in product listing")

        # Check combined recommendations from all analysis types
        combined_recs = comprehensive['combined_recommendations']
        critical_recs = [r for r in combined_recs if r.get('impact') == 'critical']
        self.assertEqual(len(critical_recs), 0,
                         "Critical optimization opportunities found")

    def test_user_dashboard_complex_relationships(self):
        """Test complex dashboard query optimization."""
        with self.analyzer.analyze() as analysis:
            # Simulate authenticated user dashboard access
            response = self.client.get('/dashboard/',
                                       HTTP_AUTHORIZATION='Bearer test-token')

        results = analysis.get_results()

        # Ensure efficient relationship handling
        prefetch_issues = [p for p in results.detected_patterns
                           if 'prefetch' in p.pattern_type]
        self.assertLess(len(prefetch_issues), 2,
                        "Too many prefetch_related opportunities found")

        # Dashboard should be fast even with complex data
        self.assertLess(results.total_duration, 0.200,  # 200ms budget
                        "Dashboard query time too slow")

    def test_project_wide_optimization_score(self):
        """Test overall project optimization score."""
        # This test doesn't require runtime analysis
        comprehensive = self.analyzer.get_comprehensive_analysis()

        if comprehensive.get('model_analysis'):
            models = comprehensive['model_analysis']
            models_with_indexes = len([m for m in models if m.indexes])
            models_with_relationships = len([m for m in models
                                             if m.foreign_keys or m.many_to_many])

            # Ensure models with relationships have proper indexes
            index_coverage = models_with_indexes / models_with_relationships if models_with_relationships > 0 else 1
            self.assertGreater(index_coverage, 0.7,
                               "Less than 70% of models with relationships have indexes")
```

### Development Middleware

```python
# middleware/performance_middleware.py - Development performance monitoring
from dbcrust.django.analyzer import create_enhanced_analyzer
from django.conf import settings
from django.utils.deprecation import MiddlewareMixin
import logging

logger = logging.getLogger('performance')


class PerformanceAnalysisMiddleware(MiddlewareMixin):
    """Middleware to analyze performance of all requests in development."""

    def __init__(self, get_response):
        self.get_response = get_response
        self.analyzer = None

        # Only enable in development/staging
        if settings.DEBUG or getattr(settings, 'ENABLE_PERFORMANCE_ANALYSIS', False):
            self.analyzer = create_enhanced_analyzer(
                project_root=settings.BASE_DIR,
                enable_all_features=True
            )

    def process_request(self, request):
        if self.analyzer:
            # Start analysis for this request
            request._performance_analysis = self.analyzer.analyze().__enter__()

    def process_response(self, request, response):
        if hasattr(request, '_performance_analysis'):
            # Finish analysis
            analysis = request._performance_analysis
            analysis.__exit__(None, None, None)

            results = analysis.get_results()

            # Log performance metrics
            logger.info(f"Request: {request.path}")
            logger.info(f"Queries: {results.total_queries}, Time: {results.total_duration * 1000:.1f}ms")

            # Log critical issues
            critical_patterns = [p for p in results.detected_patterns
                                 if p.severity == 'critical']
            if critical_patterns:
                logger.warning(f"Critical performance issues in {request.path}:")
                for pattern in critical_patterns:
                    logger.warning(f"  - {pattern.description}")

            # Add performance headers in development
            if settings.DEBUG:
                response['X-Query-Count'] = str(results.total_queries)
                response['X-Query-Time'] = f"{results.total_duration * 1000:.1f}ms"
                if critical_patterns:
                    response['X-Performance-Issues'] = str(len(critical_patterns))

        return response
```

### Enhanced Management Commands

```python
# management/commands/analyze_performance.py - Comprehensive analysis command
from django.core.management.base import BaseCommand
from dbcrust.django.analyzer import analyze_django_project, generate_optimization_report
import os


class Command(BaseCommand):
    help = 'Analyze Django project for ORM performance issues'

    def add_arguments(self, parser):
        parser.add_argument('--output', type=str,
                            help='Output file for detailed report')
        parser.add_argument('--score-threshold', type=int, default=70,
                            help='Minimum optimization score (default: 70)')
        parser.add_argument('--fix-suggestions', action='store_true',
                            help='Include detailed fix suggestions')
        parser.add_argument('--migrations', action='store_true',
                            help='Generate index migration files')

    def handle(self, *args, **options):
        project_root = os.getcwd()

        self.stdout.write("🔍 Analyzing Django project for performance issues...")

        # Comprehensive project analysis
        if options['output']:
            report = generate_optimization_report(
                project_path=project_root,
                output_file=options['output']
            )
            self.stdout.write(f"📄 Detailed report saved to {options['output']}")

        # Quick analysis for CLI
        results = analyze_django_project(project_root)

        score = results['optimization_score']
        self.stdout.write(f"\n📊 Optimization Score: {score:.1f}/100")

        # Color-coded score output
        if score >= 85:
            self.stdout.write(self.style.SUCCESS("✅ Excellent optimization"))
        elif score >= 70:
            self.stdout.write(self.style.WARNING("⚠️ Good, with room for improvement"))
        else:
            self.stdout.write(self.style.ERROR("🚨 Needs significant optimization"))

        # Summary statistics
        self.stdout.write(f"📋 Models analyzed: {len(results['models'])}")
        self.stdout.write(f"🔍 Issues found: {len(results['code_issues'])}")

        # Issue breakdown
        issue_counts = {}
        for issue in results['code_issues']:
            issue_counts[issue.severity] = issue_counts.get(issue.severity, 0) + 1

        for severity, count in issue_counts.items():
            if severity == 'critical':
                self.stdout.write(self.style.ERROR(f"  🚨 Critical: {count}"))
            elif severity == 'high':
                self.stdout.write(self.style.WARNING(f"  ⚠️ High: {count}"))
            else:
                self.stdout.write(f"  ℹ️ {severity.title()}: {count}")

        # Check against threshold
        if score < options['score_threshold']:
            self.stdout.write(self.style.ERROR(
                f"\n❌ Score {score:.1f} below threshold {options['score_threshold']}"
            ))
            exit(1)
        else:
            self.stdout.write(self.style.SUCCESS(
                f"\n✅ Score {score:.1f} meets threshold {options['score_threshold']}"
            ))
```

## Best Practices

### 1. Automatic Development Integration

The easiest way to integrate performance analysis is using the built-in middleware:

#### **Quick Setup (Recommended)**

```python
# settings.py or settings/development.py
MIDDLEWARE = [
    'dbcrust.django.PerformanceAnalysisMiddleware',
    # ... your other middleware
]

# Optional configuration
DBCRUST_PERFORMANCE_ANALYSIS = {
    'ENABLED': True,  # Override DEBUG mode
    'QUERY_THRESHOLD': 10,  # Warn if > 10 queries
    'TIME_THRESHOLD': 100,  # Warn if > 100ms
    'LOG_ALL_REQUESTS': False,  # Only log problematic requests
    'INCLUDE_HEADERS': True,  # Add performance headers
    'ENABLE_CODE_ANALYSIS': False,  # Enable full project analysis
    'TRANSACTION_SAFE': False,  # Avoid session conflicts (default: False)
    'DEBUG_TOOLBAR_COMPATIBILITY': True,  # Auto-disable with Debug Toolbar (default: True)
}

# Performance logging (recommended)
LOGGING = {
    'version': 1,
    'disable_existing_loggers': False,
    'handlers': {
        'console': {
            'class': 'logging.StreamHandler',
        },
    },
    'loggers': {
        'dbcrust.performance': {
            'handlers': ['console'],
            'level': 'INFO',
            'propagate': False,
        },
    },
}
```

#### **What You Get Automatically**

With just the middleware added, every request gets:

- **🔍 Automatic Analysis**: Detects N+1 queries, missing optimizations
- **📊 Performance Headers**: Visible in browser developer tools
- **📝 Smart Logging**: Only logs requests with performance issues
- **⚡ Zero Code Changes**: No modifications to views or models needed

#### **Browser Developer Tools Integration**

The middleware adds helpful headers you can see in your browser's Network tab:

```
X-DBCrust-Query-Count: 15
X-DBCrust-Query-Time: 234.5ms
X-DBCrust-Issues-Total: 3
X-DBCrust-Issues-Critical: 1
X-DBCrust-Pattern-Types: n_plus_one,missing_select_related
X-DBCrust-Warning: Critical performance issues
```

#### **Development Console Output**

```bash
INFO:dbcrust.performance: GET /products/ | queries=15 | db_time=234.5ms | total_time=445.2ms | issues=3
WARNING:dbcrust.performance:   🔸 n_plus_one: N+1 query detected: accessing related objects in loop (at views.py:42)
WARNING:dbcrust.performance:   🔸 missing_select_related: Use select_related() for foreign key relationships (at views.py:43)
```

#### **Advanced Configuration Example**

```python
# For teams wanting comprehensive analysis
DBCRUST_PERFORMANCE_ANALYSIS = {
    'ENABLED': True,
    'QUERY_THRESHOLD': 5,  # Strict query limit
    'TIME_THRESHOLD': 50,  # Strict timing limit
    'LOG_ALL_REQUESTS': True,  # Log all requests for monitoring
    'INCLUDE_HEADERS': True,
    'ENABLE_CODE_ANALYSIS': True,  # Enable full project scanning
    'TRANSACTION_SAFE': False,  # Avoid session conflicts (recommended)
    'DEBUG_TOOLBAR_COMPATIBILITY': False,  # Force enable even with Debug Toolbar
}
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

    print(f"✅ Performance analysis passed: {results.total_queries} queries in {results.total_duration * 1000:.1f}ms")
```

## Common Optimization Patterns

### 1. Book Library Example

```python
# Before: N+1 queries
def list_books_bad():
    books = Book.objects.all()
    for book in books:
        print(f"{book.title} by {book.author.name}")  # N+1 queries
        print(f"Publisher: {book.publisher.name}")  # More N+1 queries
        print(f"Categories: {', '.join(c.name for c in book.categories.all())}")  # Even more N+1


# After: Optimized
def list_books_good():
    books = (
        Book.objects
        .select_related('author', 'publisher')  # Join author and publisher
        .prefetch_related('categories')  # Prefetch categories
    )
    for book in books:
        print(f"{book.title} by {book.author.name}")  # No additional queries
        print(f"Publisher: {book.publisher.name}")  # No additional queries
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
        .order_by('-created_at')[:10]  # Limit results
    )

    orders_data = []
    for order in recent_orders:
        items_data = [
            {
                'product': item.product.name,  # No additional queries
                'price': item.price
            }
            for item in order.items.all()  # No additional queries
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

## Django Management Command

DBCrust provides a Django management command that works like Django's built-in `dbshell` command but launches DBCrust
instead of the default database client. This gives you access to all of DBCrust's advanced features with automatic
Django database configuration.

### Installation and Setup

1. **Add to INSTALLED_APPS:**

```python
# settings.py
INSTALLED_APPS = [
    # ... your other apps
    'dbcrust',
]
```

2. **Verify Installation:**

```bash
python manage.py dbcrust --help
```

### Basic Usage

The management command automatically reads your Django database configuration and launches DBCrust:

#### Connect to Default Database

```bash
# Launch DBCrust with your default database
python manage.py dbcrust

# Same as above but explicit
python manage.py dbcrust --database default
```

#### Connect to Specific Database

```bash
# Connect to a specific database alias
python manage.py dbcrust --database analytics
python manage.py dbcrust --database cache
```

#### List Available Databases

```bash
# See all configured databases
python manage.py dbcrust --list-databases
```

**Example Output:**

```
📊 Available Database Configurations:

  🔹 default
     Type: PostgreSQL
     Status: ✅ Supported
     Details: Host: localhost:5432, Database: myapp, User: postgres

  🔹 analytics
     Type: MySQL
     Status: ✅ Supported
     Details: Host: mysql.example.com:3306, Database: analytics_db, User: analytics_user

  🔹 cache
     Type: SQLite
     Status: ✅ Supported
     Details: File: /path/to/cache.db
```

### Command Options

#### Information Commands

```bash
# Show connection information
python manage.py dbcrust --show-url --database default

# Check DBCrust version
python manage.py dbcrust --dbcrust-version

# Show what command would be executed
python manage.py dbcrust --dry-run --database default
```

#### Debug Options

```bash
# Enable debug output
python manage.py dbcrust --debug --database default

# Pass additional arguments to DBCrust
python manage.py dbcrust --debug -- --no-banner -c "\\dt"
```

### Database Support

The management command supports all Django database backends that are compatible with DBCrust:

| Django Backend | DBCrust Support | URL Format                          |
|----------------|-----------------|-------------------------------------|
| `postgresql`   | ✅ Full          | `postgres://user:pass@host:port/db` |
| `mysql`        | ✅ Full          | `mysql://user:pass@host:port/db`    |
| `sqlite3`      | ✅ Full          | `sqlite:///path/to/db.sqlite3`      |
| `oracle`       | ❌ Not supported | -                                   |

### Configuration Examples

#### PostgreSQL with SSL

```python
# settings.py
DATABASES = {
    'default': {
        'ENGINE': 'django.db.backends.postgresql',
        'NAME': 'myapp_production',
        'USER': 'myapp_user',
        'PASSWORD': 'secure_password',
        'HOST': 'db.example.com',
        'PORT': '5432',
        'OPTIONS': {
            'sslmode': 'require',
            'connect_timeout': 10,
        }
    }
}
```

```bash
# Launches DBCrust with SSL connection
python manage.py dbcrust
```

#### Multiple Databases

```python
# settings.py
DATABASES = {
    'default': {
        'ENGINE': 'django.db.backends.postgresql',
        'NAME': 'main_db',
        'USER': 'postgres',
        'HOST': 'localhost',
        'PORT': '5432',
    },
    'analytics': {
        'ENGINE': 'django.db.backends.mysql',
        'NAME': 'analytics_db',
        'USER': 'analytics_user',
        'PASSWORD': 'analytics_pass',
        'HOST': 'mysql.example.com',
        'PORT': '3306',
    },
    'cache': {
        'ENGINE': 'django.db.backends.sqlite3',
        'NAME': BASE_DIR / 'cache.db',
    }
}
```

```bash
# Connect to different databases
python manage.py dbcrust --database default    # PostgreSQL
python manage.py dbcrust --database analytics  # MySQL
python manage.py dbcrust --database cache      # SQLite
```

### Integration with Django Workflows

#### Development Database Shell

Replace your regular database shell workflow:

```bash
# Instead of Django's dbshell
python manage.py dbshell

# Use DBCrust for enhanced features
python manage.py dbcrust
```

#### Development Scripts

```bash
# Run SQL scripts during development
python manage.py dbcrust -- -c "\\dt"                    # List tables
python manage.py dbcrust -- -c "SELECT COUNT(*) FROM users;"  # Run query
python manage.py dbcrust -- -f migration.sql             # Execute file
```

#### Production Debugging

```bash
# Safe read-only analysis in production
python manage.py dbcrust --database replica -- --read-only

# Quick table inspection
python manage.py dbcrust --dry-run --show-url --database production
```

### Error Handling

The management command provides helpful error messages:

#### DBCrust Not Found

```bash
❌ DBCrust binary not found. Please ensure DBCrust is installed and in your PATH.
Install with: pip install dbcrust
Or with uv: uv add dbcrust
```

#### Unsupported Database

```bash
❌ Database configuration error: Database engine 'django.db.backends.oracle' is not supported by DBCrust
```

#### Missing Database

```bash
❌ Database configuration error: Database alias 'nonexistent' not found. Available: default, analytics
```

### Advanced Usage

#### Custom Connection Parameters

```bash
# Debug connection issues
python manage.py dbcrust --debug --show-url

# Pass through DBCrust-specific options
python manage.py dbcrust -- --ssh-tunnel user@jumphost.com --vault-role myapp
```

#### Integration with Scripts

```python
# management/commands/analyze_performance.py
from django.core.management.base import BaseCommand
from django.core.management import call_command
import subprocess
import sys


class Command(BaseCommand):
    def handle(self, *args, **options):
        # Launch DBCrust for performance analysis
        try:
            call_command('dbcrust', database='analytics',
                         dbcrust_args=['-c', '\\timing on', '-c', 'SELECT * FROM slow_query_log;'])
        except Exception as e:
            self.stderr.write(f"Performance analysis failed: {e}")
```

#### Docker Integration

```python
# For containerized Django applications
DATABASES = {
    'default': {
        'ENGINE': 'django.db.backends.postgresql',
        'NAME': 'myapp',
        'USER': 'postgres',
        'PASSWORD': 'postgres',
        'HOST': 'db',  # Docker service name
        'PORT': '5432',
    }
}
```

```bash
# Inside Docker container
docker exec -it myapp-web python manage.py dbcrust
```

---

## Summary: Enhanced Django ORM Analyzer

The enhanced Django ORM Query Analyzer in DBCrust now provides **comprehensive, production-ready performance analysis**
for Django applications with unprecedented detail and actionability.

### 🚀 **What's New in the Enhanced Analyzer**

#### **Complete Coverage (12+ Patterns)**

- **N+1 Query Detection** with exact code locations
- **Missing Relationship Optimizations** (select_related/prefetch_related)
- **Subqueries in Loops** detection
- **Missing Database Indexes** identified through EXPLAIN analysis
- **Inefficient Aggregations** and manual calculations
- **Bulk Operation Opportunities** (bulk_create/bulk_update)
- **Large Result Sets** without pagination
- **Count vs Length** inefficiencies
- **Query Result Caching** opportunities
- **Transaction Race Conditions** detection
- **Template N+1 Patterns** in Django templates
- **Connection Pooling** issues

#### **Precision Analysis**

- **📍 Exact Line Numbers**: `views.py:42`, `models.py:156`
- **💡 Specific Field Suggestions**: `['author', 'publisher', 'reviews']`
- **🗃️ Table Context**: `{'book': ['author_id', 'publisher_id']}`
- **⚡ Ready-to-Use Fixes**: Copy-paste optimized code
- **🔧 Migration Generation**: Auto-generated Django migrations
- **📈 Quantified Impact**: "Could reduce 15 queries to 2 (87% improvement)"

#### **Multi-Layer Analysis**

1. **Runtime Query Capture**: Real-time pattern detection
2. **AST Code Analysis**: Entire project scanning with precise locations
3. **EXPLAIN Integration**: Database-level optimization insights
4. **Model Relationship Mapping**: Project-wide structure analysis

#### **Actionable Intelligence**

- **Before/After Code Examples** for every recommendation
- **Difficulty & Impact Ratings** for prioritizing fixes
- **Reference Documentation Links** to Django optimization guides
- **Optimization Scoring** (0-100) for tracking improvement
- **Comprehensive Reports** for teams and stakeholders

### 🎯 **Key Use Cases**

#### **Development Workflow**

```python
# Catch issues during development
with analyzer.analyze() as analysis:
    # Your Django code here
    pass

results = analysis.get_results()
# Get precise recommendations with line numbers
```

#### **CI/CD Integration**

```python
# Automated performance gate in CI pipeline
results = analyze_django_project("/app")
if results['optimization_score'] < 70:
    sys.exit(1)  # Fail build for performance issues
```

#### **Performance Testing**

```python
# Performance budgets in test suite
def test_homepage_performance(self):
    with analyzer.analyze() as analysis:
        response = self.client.get('/')

    results = analysis.get_results()
    self.assertLess(results.total_queries, 5)  # Query budget
```

#### **Production Monitoring**

```python
# Safe analysis in production for critical paths
with analyzer.analyze(transaction_safe=True) as analysis:
    process_checkout(cart_id)  # Monitor critical business logic
```

### 📊 **Impact on Development Teams**

#### **Before Enhanced Analyzer**

- ❌ Manual N+1 detection through logs
- ❌ Guesswork for optimization opportunities
- ❌ No code location tracking
- ❌ Generic recommendations
- ❌ Limited project-wide visibility

#### **After Enhanced Analyzer**

- ✅ **12+ patterns detected automatically** with exact locations
- ✅ **Specific field suggestions** for select_related/prefetch_related
- ✅ **Line-by-line guidance** with code snippets
- ✅ **Quantified performance impact** measurements
- ✅ **Project-wide analysis** and optimization scoring
- ✅ **CI/CD integration** for preventing regressions
- ✅ **Comprehensive reporting** for stakeholders

### 🔗 **Seamless Integration**

The enhanced analyzer integrates perfectly with:

- **Django Management Commands**: `python manage.py dbcrust`
- **Test Suites**: Performance budgets and regression detection
- **CI/CD Pipelines**: Automated performance gates
- **Development Middleware**: Real-time analysis during development
- **Production Monitoring**: Safe analysis of critical code paths
- **DBCrust EXPLAIN**: Database-level optimization insights

### 🎉 **The Result**

Django developers now have a **production-grade performance analysis tool** that provides:

1. **Complete ORM Coverage**: No performance anti-pattern goes undetected
2. **Actionable Recommendations**: Specific, tested fixes with code examples
3. **Development Integration**: Seamless workflow integration from development to production
4. **Team Collaboration**: Comprehensive reports and optimization scoring
5. **Continuous Improvement**: Track performance improvements over time

The enhanced Django ORM Query Analyzer transforms performance optimization from a manual, error-prone process into an *
*automated, precise, and actionable workflow** that ensures Django applications perform optimally at scale.

---

## 🚀 **Get Started Today**

### **Instant Setup (30 Seconds)**

1. **Install DBCrust** (if not already installed):
   ```bash
   pip install dbcrust
   # or
   uv add dbcrust
   ```

2. **Add to Django Apps**:
   ```python
   # settings.py
   INSTALLED_APPS = [
       # ... your other apps
       'dbcrust',
   ]
   ```

3. **Add Performance Middleware** (recommended):
   ```python
   # settings.py - Add this one line for automatic monitoring
   MIDDLEWARE = [
       'dbcrust.django.PerformanceAnalysisMiddleware',
       # ... your other middleware
   ]
   ```

4. **Open Your App** and check browser Developer Tools → Network → Response Headers for:
   ```
   X-DBCrust-Query-Count: 5
   X-DBCrust-Query-Time: 45.2ms
   X-DBCrust-Status: OK
   ```

**🎉 You now have comprehensive Django ORM performance monitoring!**

### **Available Tools**

| Tool                   | Purpose                      | Usage                                            |
|------------------------|------------------------------|--------------------------------------------------|
| **Middleware**         | Automatic request monitoring | `'dbcrust.django.PerformanceAnalysisMiddleware'` |
| **Manual Analysis**    | Custom code analysis         | `from dbcrust.django import analyze`             |
| **Project Scanner**    | Full project analysis        | `analyze_django_project("/path/to/project")`     |
| **Management Command** | Database shell access        | `python manage.py dbcrust`                       |
| **CI Integration**     | Automated performance gates  | `analyze_django_project()` in CI scripts         |

### **Next Steps**

- **🔍 Monitor**: Check headers and console logs for performance issues
- **🎯 Optimize**: Follow specific recommendations with line numbers
- **🧪 Test**: Add performance budgets to test suites
- **📈 Track**: Monitor optimization scores over time
- **🚀 Scale**: Use in CI/CD for preventing performance regressions

---

The enhanced Django ORM Query Analyzer provides **production-ready performance analysis** that transforms Django
optimization from guesswork into a **precise, automated, and actionable workflow**.

## 📚 See Also

- **[Django Middleware Setup](/dbcrust/django/middleware/)** - Real-time ORM analysis middleware
- **[Django Management Commands](/dbcrust/django/management-commands/)** - CLI tools for Django projects
- **[CI/CD Integration](/dbcrust/django/ci-integration/)** - Automated performance testing
- **[Team Workflows](/dbcrust/django/team-workflows/)** - Collaborative optimization workflows
- **[Quick Start Guide](/dbcrust/quick-start/)** - Get started with DBCrust in 2 minutes

---

<div align="center">
    <strong>Ready to optimize your Django application?</strong><br>
    <a href="/dbcrust/django/middleware/" class="md-button md-button--primary">Setup Middleware</a>
    <a href="/dbcrust/django/management-commands/" class="md-button">Management Commands</a>
</div>
