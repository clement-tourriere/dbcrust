# Performance Analysis Guide

DBCrust provides comprehensive performance analysis tools to optimize database queries, identify bottlenecks, and improve application performance. This guide covers query profiling, performance monitoring, optimization techniques, and best practices.

## 🚀 Quick Performance Check

### Instant Query Analysis

```sql
-- Enable query timing for immediate performance feedback
\timing

-- Run your query to see execution time
SELECT * FROM users WHERE email = 'user@example.com';
-- Output: Time: 45.123 ms

-- Enable EXPLAIN mode for query plan analysis
\e
SELECT * FROM orders WHERE created_at >= '2024-01-01';
-- Shows execution plan with timing and optimization suggestions
```

### Built-in Performance Commands

```sql
-- Show slowest recent queries
\slow

-- Analyze current connection performance
\perf

-- Show database performance stats
\stats

-- Profile a specific query
\profile SELECT COUNT(*) FROM large_table WHERE status = 'active';
```

## 🔍 Query Performance Analysis

### Understanding Query Execution Plans

DBCrust's EXPLAIN mode provides visual, easy-to-understand execution plans:

```sql
\e  -- Enable EXPLAIN mode

SELECT u.name, COUNT(o.id) as order_count
FROM users u
LEFT JOIN orders o ON u.id = o.user_id
WHERE u.created_at >= '2024-01-01'
GROUP BY u.id, u.name
ORDER BY order_count DESC;

-- DBCrust Output:
-- ○ Execution Time: 234.56 ms • Planning Time: 12.34 ms
-- Hash Aggregate
-- │ Optimized GROUP BY with hash-based aggregation
-- │ ○ Duration: 187.23 ms • Cost: 15643 • Rows: 1247
-- └─ Hash Join
--    │ LEFT JOIN using hash table (efficient)
--    │ ○ Duration: 156.78 ms • Cost: 12456 • Rows: 2489
--    ├─ Index Scan
--    │  │ Range scan on users.created_at_idx
--    │  │ ○ Duration: 23.45 ms • Cost: 145 • Rows: 1247
--    │  └► id • name • email • created_at
--    └─ Sequential Scan
--       │ ⚠️ Full table scan on orders (consider index)
--       │ ○ Duration: 123.45 ms • Cost: 11890 • Rows: 45123
--       └► id • user_id • amount • created_at
```

### Performance Indicators

DBCrust highlights performance issues with clear visual indicators:

- **🟢 Fast Operations** (<10ms): Efficient index usage, optimal joins
- **🟡 Moderate Operations** (10-100ms): Acceptable but watch for patterns
- **🔴 Slow Operations** (>100ms): Requires attention and optimization
- **⚠️ Warnings**: Missing indexes, full table scans, inefficient joins

### Query Optimization Suggestions

```sql
-- DBCrust automatically suggests optimizations
SELECT * FROM products p
JOIN categories c ON p.category_id = c.id
WHERE c.name = 'Electronics';

-- DBCrust suggests:
-- 💡 Performance Suggestions:
-- 1. Add index: CREATE INDEX idx_products_category_id ON products(category_id);
-- 2. Add index: CREATE INDEX idx_categories_name ON categories(name);
-- 3. Consider: SELECT specific columns instead of SELECT *
-- Estimated improvement: 89% faster (234ms → 25ms)
```

## 📊 Database Performance Monitoring

### Connection Performance Overview

```sql
-- View current database performance metrics
\perf

-- Sample Output:
-- 📊 Database Performance Overview
-- ================================
-- Connection: postgres://user@localhost:5432/myapp
--
-- Query Performance:
--   Total queries this session: 1,247
--   Average query time: 67.8ms
--   Slowest query: 2.34s (SELECT FROM logs WHERE...)
--   Cache hit ratio: 94.2% (excellent)
--
-- Index Usage:
--   Index scans: 89.3%
--   Sequential scans: 10.7% (good)
--   Most used index: users_email_idx (1,234 uses)
--
-- Connection Health:
--   Active connections: 12/100
--   Idle connections: 3
--   Long-running queries: 1 (> 30s)
```

### Historical Performance Analysis

```sql
-- Show performance trends
\history performance

-- Analyze slow queries from history
\history slow 10

-- Performance regression analysis
\history compare yesterday today
```

### Real-Time Performance Monitoring

```sql
-- Monitor live database activity
\monitor

-- Sample Output:
-- 🔄 Real-Time Database Monitor (Ctrl+C to stop)
-- ================================================
-- 14:30:15 | Query: SELECT * FROM users WHERE... | 45ms | Index Scan
-- 14:30:16 | Query: INSERT INTO logs VALUES... | 12ms | Direct Insert
-- 14:30:17 | Query: UPDATE orders SET status... | 156ms | ⚠️ Seq Scan
-- 14:30:18 | Query: SELECT COUNT(*) FROM... | 2.1s | 🔴 Slow Query
```

## ⚡ Query Optimization Techniques

### Index Analysis and Recommendations

```sql
-- Analyze existing indexes
\di

-- Get index usage statistics
\di+

-- Analyze missing indexes for a query
\explain_analyze SELECT * FROM orders WHERE customer_id = 123 AND status = 'pending';

-- DBCrust Output:
-- 🔍 Index Analysis:
-- Missing recommended indexes:
--   CREATE INDEX idx_orders_customer_status ON orders(customer_id, status);
--   Benefit: 94% faster queries, affects 1,247 similar queries
--   Impact: Speeds up customer order lookups significantly
```

### Query Rewriting Suggestions

DBCrust analyzes your queries and suggests more efficient alternatives:

```sql
-- Original inefficient query
SELECT * FROM users WHERE id IN (
    SELECT user_id FROM orders WHERE amount > 1000
);

-- DBCrust suggests:
-- 💡 Query Optimization Suggestion:
-- Consider rewriting as JOIN for better performance:
SELECT DISTINCT u.*
FROM users u
INNER JOIN orders o ON u.id = o.user_id
WHERE o.amount > 1000;

-- Estimated improvement: 67% faster
```

### Table-Specific Performance Analysis

```sql
-- Analyze table performance
\analyze table users

-- Sample Output:
-- 📈 Table Analysis: users
-- ========================
-- Rows: 156,789 | Size: 45.2 MB | Last analyzed: 2024-01-15
--
-- Query Patterns:
--   Most frequent: SELECT by email (34.2%)
--   Second most: SELECT by id (28.9%)
--   Third most: SELECT by status (15.6%)
--
-- Performance Issues:
--   🔴 Missing index on email (high usage, slow queries)
--   🟡 Large result sets without LIMIT (15 queries)
--   🟡 Full table scans on status column
--
-- Recommendations:
--   1. CREATE INDEX idx_users_email ON users(email);
--   2. CREATE INDEX idx_users_status ON users(status);
--   3. Add LIMIT clauses to large SELECT queries
```

## 🎯 Application Performance Patterns

### N+1 Query Detection

DBCrust automatically detects common performance anti-patterns:

```sql
-- This query pattern triggers N+1 detection
SELECT * FROM posts;  -- 1 query
-- Then accessing post.author.name in a loop triggers N+1 warning

-- DBCrust Alert:
-- ⚠️ Potential N+1 Query Pattern Detected
-- Base query returned 50 posts
-- Followed by 50 similar queries accessing author data
--
-- 💡 Optimization suggestions:
-- 1. Use JOIN: SELECT p.*, a.name FROM posts p JOIN authors a ON p.author_id = a.id;
-- 2. Use subqueries for specific data: SELECT p.*, (SELECT name FROM authors WHERE id = p.author_id) as author_name FROM posts p;
-- 3. Consider application-level solutions like eager loading
```

### Large Result Set Analysis

```sql
-- Query returning large datasets
SELECT * FROM transactions WHERE date >= '2024-01-01';

-- DBCrust automatically analyzes:
-- 📊 Large Result Set Analysis
-- ===========================
-- Query returned: 145,678 rows (23.4 MB)
-- Network transfer time: 2.34s
-- Memory usage: 156 MB
--
-- 💡 Optimization recommendations:
-- 1. Add pagination: LIMIT 100 OFFSET 0
-- 2. Add date range limit: date BETWEEN '2024-01-01' AND '2024-01-31'
-- 3. Select specific columns: SELECT id, amount, date FROM transactions...
-- 4. Consider using streaming for large datasets
```

### Connection Pool Analysis

```sql
-- Analyze connection performance
\connections

-- Sample Output:
-- 🔗 Connection Pool Analysis
-- ===========================
-- Pool size: 20 connections
-- Active: 12 | Idle: 5 | Waiting: 0
--
-- Connection Usage:
--   Average connection time: 45.6s
--   Longest connection: 12m 34s
--   Connection churn: Low (good)
--
-- Performance Impact:
--   ✅ No connection bottlenecks
--   ✅ Healthy connection reuse
--   ⚠️ 2 long-running connections (>10 minutes)
```

## 🔧 Performance Configuration

### Query Performance Settings

```toml
# ~/.config/dbcrust/config.toml

[performance]
# Query timing
show_execution_time = true
slow_query_threshold = 1000      # Mark queries >1s as slow
auto_explain_threshold = 5000    # Auto-EXPLAIN queries >5s

# Analysis settings
detect_n_plus_one = true
suggest_indexes = true
analyze_large_result_sets = true
large_result_threshold = 1000    # Flag result sets >1000 rows

# Monitoring
enable_performance_monitoring = true
performance_sample_rate = 0.1    # Sample 10% of queries
store_performance_history = true
history_retention_days = 30
```

### Database-Specific Optimizations

```toml
[performance.postgresql]
enable_pg_stat_statements = true
track_io_timing = true
log_statement_stats = true

[performance.mysql]
enable_performance_schema = true
log_slow_queries = true
long_query_time = 1.0

[performance.sqlite]
enable_query_planner = true
analyze_on_startup = true
```

### Performance Alerting

```toml
[performance.alerting]
enable_alerts = true
slow_query_alert = 5000         # Alert on queries >5s
high_query_count_alert = 100    # Alert if >100 queries in session
memory_usage_alert = 500        # Alert if >500MB memory usage

# Integration
slack_webhook = "https://hooks.slack.com/..."
email_alerts = ["dev-team@company.com"]
```

## 📈 Performance Benchmarking

### Query Benchmarking

```sql
-- Benchmark a specific query
\benchmark 10 SELECT COUNT(*) FROM large_table WHERE status = 'active';

-- Sample Output:
-- 🏁 Query Benchmark Results (10 runs)
-- =====================================
-- Query: SELECT COUNT(*) FROM large_table WHERE status = 'active'
--
-- Performance Statistics:
--   Average time: 234.5 ms
--   Minimum time: 198.2 ms
--   Maximum time: 345.6 ms
--   Standard deviation: 32.1 ms
--   Median time: 226.8 ms
--
-- Consistency: Good (low deviation)
-- Cache effects: Detected (first run slower)
--
-- 💡 Recommendations:
--   - Add index on status column for consistent <50ms performance
--   - Consider partitioning large_table by status
```

### Database Baseline Creation

```sql
-- Create performance baseline for your database
\baseline create production_baseline

-- Compare current performance to baseline
\baseline compare production_baseline

-- Sample comparison output:
-- 📊 Performance Baseline Comparison
-- ==================================
-- Baseline: production_baseline (created 2024-01-01)
--
-- Query Performance Changes:
--   User queries: +15% faster (avg: 67ms → 58ms)
--   Order queries: -23% slower (avg: 123ms → 151ms) ⚠️
--   Product queries: +5% faster (avg: 89ms → 85ms)
--
-- Index Usage Changes:
--   Overall index usage: +12% (87% → 98%)
--   New indexes detected: 3
--   Unused indexes: 1 (consider dropping)
--
-- 🚨 Performance Regressions:
--   Order-related queries significantly slower
--   Recommendation: Analyze recent schema changes
```

## 🛠️ Advanced Performance Features

### Custom Performance Rules

```sql
-- Create custom performance monitoring rules
\performance rule add slow_queries
SET rule_name = 'detect_slow_user_queries'
SET condition = 'table_name LIKE "user%" AND duration > 500'
SET action = 'log_and_alert'
SET message = 'Slow user table query detected';

-- View active performance rules
\performance rules

-- Enable/disable rules
\performance rule enable detect_slow_user_queries
\performance rule disable detect_slow_user_queries
```

### Performance Regression Detection

```sql
-- Enable automatic regression detection
\performance regression enable

-- Set regression thresholds
\performance regression thresholds
SET query_time_increase = 50%     # Alert if query time increases >50%
SET result_set_increase = 200%    # Alert if result set grows >200%
SET index_usage_decrease = 20%    # Alert if index usage drops >20%

-- View regression alerts
\performance regressions
```

### Query Performance Profiling

```sql
-- Profile complex queries step by step
\profile verbose
SELECT u.name,
       COUNT(o.id) as order_count,
       SUM(o.amount) as total_spent,
       AVG(o.amount) as avg_order
FROM users u
LEFT JOIN orders o ON u.id = o.user_id
WHERE u.created_at >= '2024-01-01'
GROUP BY u.id, u.name
HAVING COUNT(o.id) > 5
ORDER BY total_spent DESC
LIMIT 100;

-- Detailed profiling output:
-- 🔍 Detailed Query Profile
-- =========================
-- Phase 1: Table Access (23.4ms)
--   users table scan: 12.3ms (using created_at index)
--   orders table scan: 11.1ms (full table scan ⚠️)
--
-- Phase 2: Join Processing (45.6ms)
--   Hash join preparation: 15.2ms
--   Join execution: 30.4ms (efficient hash join)
--
-- Phase 3: Aggregation (67.8ms)
--   GROUP BY processing: 45.2ms (using hash aggregation)
--   HAVING clause filter: 22.6ms
--
-- Phase 4: Sorting & Limiting (12.3ms)
--   ORDER BY processing: 8.9ms (quicksort)
--   LIMIT application: 3.4ms
--
-- 💡 Optimization opportunities:
--   1. Add index on orders.user_id (eliminates full table scan)
--   2. Consider partial index: CREATE INDEX ON orders(user_id) WHERE amount > 0;
```

## 🚨 Troubleshooting Performance Issues

### Common Performance Problems

**Slow Queries:**
```sql
-- Identify slow queries
\slow queries 10

-- Analyze specific slow query
\explain_verbose SELECT * FROM large_table WHERE complex_condition = true;

-- Common fixes:
-- 1. Missing indexes
-- 2. Inefficient WHERE clauses
-- 3. Large result sets without pagination
-- 4. Complex JOINs without proper indexes
```

**Memory Issues:**
```sql
-- Check memory usage
\memory

-- Identify memory-intensive queries
\memory queries

-- Common solutions:
-- 1. Use LIMIT for large result sets
-- 2. Stream large datasets instead of loading all at once
-- 3. Optimize query to return fewer columns
-- 4. Use pagination for user interfaces
```

**Connection Issues:**
```sql
-- Check connection health
\connections health

-- Identify connection bottlenecks
\connections analyze

-- Common fixes:
-- 1. Optimize connection pool size
-- 2. Close idle connections
-- 3. Reduce connection hold time
-- 4. Use connection pooling middleware
```

### Performance Debugging Steps

1. **Identify the Problem:**
   ```sql
   \perf                    # Overall performance overview
   \slow                    # Find slow queries
   \connections             # Check connection issues
   ```

2. **Analyze Root Cause:**
   ```sql
   \explain slow_query      # Understand execution plan
   \profile slow_query      # Detailed profiling
   \analyze table_name      # Table-specific analysis
   ```

3. **Implement Solutions:**
   ```sql
   \suggest indexes         # Get index recommendations
   \benchmark optimized_query  # Test improvements
   \baseline compare        # Measure improvement
   ```

4. **Monitor Results:**
   ```sql
   \performance monitor     # Track ongoing performance
   \regression check        # Watch for regressions
   \baseline update         # Update performance baselines
   ```

## 📚 See Also

- **[Troubleshooting Guide](/dbcrust/user-guide/troubleshooting/)** - Common issues and solutions
- **[Advanced Features](/dbcrust/user-guide/advanced-features/)** - Session management and tools
- **[Configuration Reference](/dbcrust/reference/configuration-reference/)** - Complete configuration options
- **[Django ORM Analyzer](/dbcrust/django-analyzer/)** - Django-specific performance analysis

---

<div align="center">
    <strong>Ready to optimize your database performance?</strong><br>
    <a href="/dbcrust/user-guide/troubleshooting/" class="md-button md-button--primary">Troubleshooting Guide</a>
    <a href="/dbcrust/reference/backslash-commands/" class="md-button">Command Reference</a>
</div>
