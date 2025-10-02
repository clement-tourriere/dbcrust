# File Formats Guide

DBCrust supports querying Parquet, CSV, and JSON files directly using Apache DataFusion 50.0, a powerful SQL query engine. This allows you to analyze file-based data using familiar SQL syntax without loading it into a database.

## 🚀 Getting Started

### Supported File Formats

DBCrust supports the following file formats via Apache DataFusion:

- **Parquet** - Columnar storage format optimized for analytics
- **CSV** - Comma-separated values with configurable delimiters
- **JSON** - JSON records and NDJSON (newline-delimited JSON)

### Quick Start

```bash
# Query a Parquet file
dbcrust parquet:///data/sales_2024.parquet
> SELECT COUNT(*), AVG(amount) FROM sales_2024;

# Query multiple CSV files
dbcrust 'csv:///logs/*.csv?header=true'
> SELECT date, COUNT(*) FROM logs WHERE level = 'ERROR' GROUP BY date;

# Query JSON with nested structures
dbcrust json:///api_responses.json
> SELECT data.customer.name, data.order.total FROM api_responses LIMIT 10;
```

## 📦 Connection URLs

### Parquet Files

**Scheme:** `parquet://`

```bash
# Single Parquet file
dbcrust parquet:///data/sales_2024.parquet

# Multiple files with glob pattern
dbcrust 'parquet:///data/sales_*.parquet'

# All Parquet files in directory
dbcrust 'parquet:///warehouse/*.parquet'

# Recursive glob pattern
dbcrust 'parquet:///data/**/*.parquet'
```

**Table Naming:**
- Single file: Uses filename without extension (`sales_2024.parquet` → `sales_2024`)
- Directory: Uses directory name (`/warehouse/` → `warehouse`)
- Glob pattern: Uses directory name or sanitized pattern

### CSV Files

**Scheme:** `csv://`

```bash
# CSV with header row (default)
dbcrust csv:///data/users.csv

# Custom delimiter (tab-separated)
dbcrust 'csv:///data/data.tsv?delimiter=\t'

# No header row
dbcrust 'csv:///data/logs.csv?header=false'

# Multiple CSV files
dbcrust 'csv:///logs/*.csv?header=true'

# Custom delimiter with glob
dbcrust 'csv:///exports/*.tsv?delimiter=\t&header=true'
```

**Query Parameters:**
- `?header=true|false` - CSV has header row (default: true)
- `?delimiter=,` - Field delimiter character (default: ',')

**Common Delimiters:**
- `,` - Comma (CSV)
- `\t` - Tab (TSV)
- `|` - Pipe
- `;` - Semicolon

### JSON Files

**Scheme:** `json://`

```bash
# Standard JSON file
dbcrust json:///api_responses.json

# NDJSON (newline-delimited JSON)
dbcrust json:///events.ndjson

# JSON with deeply nested structures
dbcrust json:///vault_policies.json
```

**NDJSON Detection:**
DBCrust automatically detects NDJSON format and converts it to Arrow format for efficient querying:

```json
{"id": 1, "name": "Alice", "age": 30}
{"id": 2, "name": "Bob", "age": 25}
{"id": 3, "name": "Charlie", "age": 35}
```

## 📊 Querying File Formats

### Basic SELECT Queries

```sql
-- All columns from Parquet file
SELECT * FROM sales_2024 LIMIT 10;

-- Specific columns with filtering
SELECT customer_name, total, date
FROM sales_2024
WHERE total > 1000
ORDER BY total DESC;

-- Aggregations
SELECT
    product_category,
    COUNT(*) as count,
    SUM(amount) as total_sales,
    AVG(amount) as avg_sale
FROM sales_2024
GROUP BY product_category;
```

### Advanced SQL Features

DataFusion provides a rich SQL dialect with extensive function support:

#### Aggregate Functions

```sql
SELECT
    department,
    COUNT(*) as employees,
    AVG(salary) as avg_salary,
    MIN(salary) as min_salary,
    MAX(salary) as max_salary,
    STDDEV(salary) as salary_stddev,
    MEDIAN(salary) as median_salary
FROM employees
GROUP BY department;
```

#### Window Functions

```sql
SELECT
    employee_name,
    department,
    salary,
    ROW_NUMBER() OVER (PARTITION BY department ORDER BY salary DESC) as rank_in_dept,
    LAG(salary, 1) OVER (PARTITION BY department ORDER BY salary) as previous_salary,
    AVG(salary) OVER (PARTITION BY department) as dept_avg_salary
FROM employees;
```

#### String Functions

```sql
SELECT
    UPPER(name) as name_upper,
    LOWER(email) as email_lower,
    CONCAT(first_name, ' ', last_name) as full_name,
    SUBSTRING(product_code, 1, 3) as category,
    LENGTH(description) as desc_length,
    TRIM(title) as trimmed_title
FROM products;
```

#### Date/Time Functions

```sql
SELECT
    date,
    EXTRACT(YEAR FROM date) as year,
    EXTRACT(MONTH FROM date) as month,
    DATE_TRUNC('month', date) as month_start,
    NOW() as current_timestamp,
    date + INTERVAL '7 days' as next_week
FROM events;
```

#### Array Functions

```sql
SELECT
    ARRAY_AGG(product_name) as products,
    ARRAY_LENGTH(tags) as tag_count,
    ARRAY_CONTAINS(categories, 'electronics') as has_electronics
FROM orders;
```

### Multi-File Queries with Glob Patterns

Query multiple files as if they were a single table:

```sql
-- Query all CSV files in directory
SELECT date, level, COUNT(*) as log_count
FROM logs
WHERE level IN ('ERROR', 'CRITICAL')
GROUP BY date, level
ORDER BY date DESC;

-- Combine multiple Parquet files by pattern
SELECT
    EXTRACT(MONTH FROM date) as month,
    SUM(revenue) as monthly_revenue
FROM 'sales_*.parquet'
GROUP BY month
ORDER BY month;
```

## 🔍 Nested Field Navigation

### Understanding Nested Structures

Parquet and JSON files often contain nested structures (Struct, Array, Map types). DBCrust provides intelligent multi-level autocomplete for navigating these structures.

### Schema Display for Complex Types

When you describe a table with nested fields, DBCrust shows two sections:

1. **Main table** with column summaries
2. **Nested field details** section with expandable information

```sql
\d policies

Table: policies
Column           | Type
-----------------+--------------------
id               | Int64
data             | Struct<4 fields>
timestamp        | Utf8

Nested field details:
  data (Struct):
    - chroot_namespace: Utf8
    - exact_paths: Struct<25 fields>
    - glob_paths: Struct<10 fields>
    - root: Utf8

  data.exact_paths (Struct):
    - auth/token/create: Struct<1 fields>
    - auth/token/lookup-self: Struct<1 fields>
    - aws_okta/creds/management-ecr: Struct<1 fields>
    ... and 22 more fields
```

### Multi-Level Autocomplete

DBCrust supports deep nested field navigation with intelligent autocomplete:

```sql
-- First level - shows all top-level paths
SELECT data.[TAB] FROM policies
-- Suggests: data, data.chroot_namespace, data.exact_paths, data.glob_paths, data.root

-- Second level - shows direct children only
SELECT data.exact_paths.[TAB] FROM policies
-- Suggests: auth/token/create, auth/token/lookup-self, aws_okta/creds/management-ecr, ...

-- Third level - navigate even deeper
SELECT data.exact_paths.auth/token/create.[TAB] FROM policies
-- Suggests: capabilities

-- Fourth level - works at any depth
SELECT data.exact_paths.auth/token/create.capabilities[TAB] FROM policies
-- Shows capabilities array fields
```

**Autocomplete Features:**
- **Direct Children Only**: Shows immediate children, not all descendants
- **Any Depth**: Navigate nested structures to unlimited depth
- **Special Characters**: Handles field names with `/`, `-`, `@`, and other characters
- **Context-Aware**: Works in SELECT, WHERE, ORDER BY, GROUP BY, and all SQL clauses
- **Performance**: Fast even with deeply nested structures (100+ fields)

### Querying Nested Fields

```sql
-- Select specific nested fields
SELECT
    id,
    data.chroot_namespace,
    data.exact_paths.auth/token/create.capabilities
FROM policies;

-- Filter by nested fields
SELECT *
FROM policies
WHERE data.root = 'true'
  AND data.exact_paths.auth/token/create.capabilities IS NOT NULL;

-- Aggregate by nested fields
SELECT
    data.chroot_namespace,
    COUNT(*) as policy_count
FROM policies
GROUP BY data.chroot_namespace;
```

### Working with Arrays

```sql
-- Access array elements
SELECT
    tags[1] as first_tag,
    ARRAY_LENGTH(tags) as tag_count
FROM products;

-- Unnest arrays
SELECT
    product_id,
    tag
FROM (
    SELECT product_id, UNNEST(tags) as tag
    FROM products
);
```

## 🎯 Use Cases

### Data Analysis

Analyze large datasets without database setup:

```sql
-- Analyze sales trends from Parquet files
SELECT
    DATE_TRUNC('month', order_date) as month,
    product_category,
    COUNT(DISTINCT customer_id) as unique_customers,
    SUM(amount) as total_revenue,
    AVG(amount) as avg_order_value
FROM 'warehouse/sales_*.parquet'
WHERE order_date >= '2024-01-01'
GROUP BY month, product_category
ORDER BY month, total_revenue DESC;
```

### Log Analysis

Query log files with SQL:

```sql
-- Analyze error patterns in CSV logs
SELECT
    date,
    service,
    error_type,
    COUNT(*) as error_count
FROM 'logs/*.csv'
WHERE level = 'ERROR'
  AND date >= CURRENT_DATE - INTERVAL '7 days'
GROUP BY date, service, error_type
ORDER BY error_count DESC
LIMIT 20;
```

### Data Lake Queries

Query data lake structures:

```sql
-- Query partitioned Parquet files
SELECT
    year,
    month,
    country,
    SUM(revenue) as total_revenue
FROM 'datalake/events/**/*.parquet'
WHERE year = 2024
GROUP BY year, month, country;
```

### API Response Analysis

Analyze JSON API responses:

```sql
-- Analyze nested JSON from API responses
SELECT
    data.user.id,
    data.user.email,
    data.order.total,
    data.order.status,
    ARRAY_LENGTH(data.order.items) as item_count
FROM api_responses
WHERE data.order.total > 100
  AND data.order.status = 'completed';
```

## ⚡ Performance Tips

### Parquet Files

**Best for:** Large datasets (100MB+), analytical queries

**Advantages:**
- **Columnar Format**: Only reads columns needed for query
- **Predicate Pushdown**: Filters applied at file level before loading data
- **Compression**: Efficient storage with compression
- **Type Preservation**: Full type information maintained

**Example:**
```sql
-- Only reads 'amount' and 'date' columns from file
SELECT SUM(amount)
FROM 'sales.parquet'
WHERE date >= '2024-01-01';
-- ✓ Fast: Only scans needed columns
-- ✓ Predicate pushdown: Filters at file level
```

### CSV Files

**Best for:** Small to medium datasets (<100MB), simple structure

**Advantages:**
- **Universal Format**: Works everywhere
- **Human Readable**: Easy to inspect
- **Simple Schema**: Good for flat data

**Limitations:**
- Sequential reading (no column pruning)
- Type inference required
- Less efficient compression

**Example:**
```sql
-- Reads entire file to parse CSV
SELECT *
FROM 'logs.csv'
WHERE date >= '2024-01-01';
-- ✗ Slower: Must read full file
-- ✓ Simple: Easy to work with
```

### JSON Files

**Best for:** Semi-structured data, varying schemas, nested structures

**Advantages:**
- **Flexible Schema**: Handles varying field sets
- **Nested Structures**: Natural representation of hierarchical data
- **Self-Describing**: Field names included in data

**NDJSON Benefits:**
- Line-by-line processing
- Streaming-friendly
- Partial read support

**Example:**
```sql
-- Handles varying schemas gracefully
SELECT
    data.customer.name,
    data.order.total
FROM 'api_responses.json'
WHERE data.order.total > 100;
-- ✓ Flexible: Works with varying schemas
-- ✓ Nested: Direct access to nested fields
```

## 🔧 Advanced Features

### Cross-Format JOINs

Join data across different file formats:

```sql
-- Join Parquet and CSV data
SELECT
    u.name,
    u.email,
    o.order_id,
    o.total
FROM 'users.parquet' u
JOIN 'orders.csv' o ON u.id = o.user_id
WHERE o.total > 1000;

-- Combine JSON and Parquet
SELECT
    p.data.customer.name as customer,
    s.product_name,
    s.amount
FROM 'policies.json' p
JOIN 'sales.parquet' s ON p.id = s.policy_id
WHERE s.amount > 500;

-- Three-way join across formats
SELECT
    u.name,
    o.order_id,
    p.product_name
FROM 'users.csv' u
JOIN 'orders.parquet' o ON u.user_id = o.user_id
JOIN 'products.json' p ON o.product_id = p.id;
```

### File Path References

Reference files directly in queries:

```sql
-- Explicit file paths in FROM clause
SELECT a.*, b.*
FROM '/data/warehouse/sales.parquet' a
JOIN '/exports/customers.csv' b ON a.customer_id = b.id;

-- Mix table names and file paths
SELECT *
FROM sales s
JOIN '/path/to/products.parquet' p ON s.product_id = p.id;
```

### Subqueries and CTEs

Use Common Table Expressions for complex queries:

```sql
-- CTE with file formats
WITH monthly_sales AS (
    SELECT
        DATE_TRUNC('month', date) as month,
        SUM(amount) as total
    FROM 'sales_*.parquet'
    GROUP BY month
)
SELECT
    month,
    total,
    LAG(total, 1) OVER (ORDER BY month) as prev_month,
    (total - LAG(total, 1) OVER (ORDER BY month)) / LAG(total, 1) OVER (ORDER BY month) * 100 as growth_pct
FROM monthly_sales
ORDER BY month;
```

## 📖 Complete SQL Reference

DataFusion provides PostgreSQL-compatible SQL with extensive function support. Here's a quick reference:

### Supported SQL Features

✅ **SELECT Statements**
- Column selection, aliases, wildcards
- DISTINCT
- WHERE conditions with all comparison operators
- GROUP BY, HAVING
- ORDER BY with ASC/DESC
- LIMIT, OFFSET

✅ **JOINs**
- INNER JOIN
- LEFT/RIGHT/FULL OUTER JOIN
- CROSS JOIN
- Self joins

✅ **Subqueries**
- Scalar subqueries
- IN/NOT IN subqueries
- EXISTS/NOT EXISTS
- Correlated subqueries

✅ **Set Operations**
- UNION, UNION ALL
- INTERSECT
- EXCEPT

✅ **Window Functions**
- ROW_NUMBER, RANK, DENSE_RANK
- LAG, LEAD
- FIRST_VALUE, LAST_VALUE
- Aggregate window functions

✅ **Common Table Expressions (CTEs)**
- WITH clause
- Recursive CTEs (limited support)

### Function Categories

**Aggregate Functions:**
`COUNT`, `SUM`, `AVG`, `MIN`, `MAX`, `STDDEV`, `VARIANCE`, `MEDIAN`, `APPROX_DISTINCT`, `APPROX_PERCENTILE`

**String Functions:**
`CONCAT`, `UPPER`, `LOWER`, `TRIM`, `LTRIM`, `RTRIM`, `SUBSTRING`, `REPLACE`, `SPLIT_PART`, `LENGTH`, `CHAR_LENGTH`, `POSITION`, `REGEXP_MATCH`, `REGEXP_REPLACE`

**Date/Time Functions:**
`NOW`, `CURRENT_DATE`, `CURRENT_TIME`, `CURRENT_TIMESTAMP`, `DATE_TRUNC`, `EXTRACT`, `TO_TIMESTAMP`, `TO_DATE`, `DATE_ADD`, `DATE_SUB`

**Math Functions:**
`ABS`, `CEIL`, `FLOOR`, `ROUND`, `TRUNC`, `SQRT`, `POWER`, `EXP`, `LN`, `LOG`, `SIN`, `COS`, `TAN`

**Type Conversion:**
`CAST`, `TRY_CAST`, `COALESCE`, `NULLIF`

**Array Functions:**
`ARRAY_AGG`, `ARRAY_LENGTH`, `ARRAY_CONTAINS`, `ARRAY_POSITION`, `ARRAY_CONCAT`, `UNNEST`

**Conditional:**
`CASE WHEN`, `IF`, `COALESCE`, `NULLIF`

For complete DataFusion SQL documentation, see the [Apache Arrow DataFusion SQL Reference](https://arrow.apache.org/datafusion/user-guide/sql/index.html).

## 🔍 Troubleshooting

### Common Issues

**File Not Found:**
```
Error: File not found: /data/sales.parquet
```
- Verify file path is absolute (`///` prefix for `file://`)
- Check file permissions
- Ensure file exists: `ls -la /data/sales.parquet`

**Schema Inference Failed:**
```
Error: Failed to infer schema
```
- For CSV: Check if `?header=true` is set correctly
- For JSON: Verify file is valid JSON or NDJSON
- Try reading first few rows to diagnose

**Nested Field Not Found:**
```
Error: No field named 'data.customer.address'
```
- Check schema with `\d table_name`
- Verify field names match exactly (case-sensitive)
- Use autocomplete to discover available fields

**Glob Pattern No Results:**
```
Table 'logs' not found
```
- Verify glob pattern matches files: `ls /path/*.csv`
- Check directory exists and contains matching files
- Ensure proper quoting: `dbcrust 'csv:///logs/*.csv'`

### Performance Issues

**Slow Query on Large CSV:**
- Consider converting to Parquet for better performance
- Use column selection to reduce data read
- Apply filters early in the query

**Memory Usage:**
- Parquet files are memory-efficient (columnar)
- CSV/JSON may load more data into memory
- Use LIMIT for exploratory queries

**Nested Field Queries Slow:**
- DataFusion efficiently handles nested structures
- Consider flattening extremely deep nesting
- Use specific field paths instead of SELECT *

---

**Ready to query your data files?** Start with a simple Parquet or CSV file and explore the powerful SQL capabilities DBCrust provides!

<div align="center">
    <a href="/dbcrust/user-guide/basic-usage/" class="md-button md-button--primary">Back to Basic Usage</a>
    <a href="/dbcrust/reference/url-schemes/" class="md-button">URL Schemes Reference</a>
</div>
