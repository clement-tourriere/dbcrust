# Elasticsearch Integration

DBCrust provides comprehensive support for Elasticsearch through its SQL API, making it easy to query, analyze, and explore your Elasticsearch indices with familiar SQL syntax.

## Quick Start

### Basic Connection

```bash
# Connect to local Elasticsearch instance
dbcrust elasticsearch://localhost:9200

# With authentication
dbcrust elasticsearch://elastic:password@localhost:9200

# With SSL (skip certificate verification for development)
dbcrust "elasticsearch://elastic:password@localhost:9200?ssl=true&verify_certs=false"

# Different URL schemes (all equivalent)
dbcrust elasticsearch://localhost:9200
dbcrust elastic://localhost:9200
dbcrust es://localhost:9200
```

### Docker Integration

DBCrust automatically detects Elasticsearch containers:

```bash
# Connect to Elasticsearch container by name
dbcrust docker://my-elasticsearch-container

# List all database containers (includes Elasticsearch)
dbcrust --list-containers
```

## Key Features

### Intelligent Index Name Handling

DBCrust automatically handles index names with special characters:

```sql
-- These work automatically (no manual quoting needed):
SELECT * FROM logs-2024.01.01
SELECT * FROM my-index-name
\d commit-data-2020.01.01

-- DBCrust auto-converts to:
SELECT * FROM "logs-2024.01.01"
SELECT * FROM "my-index-name"
\d "commit-data-2020.01.01"
```

### Smart SELECT * Query Rewriting

Elasticsearch SQL fails when indices contain array fields. DBCrust automatically detects and excludes array fields:

```sql
-- Original query that would fail:
SELECT * FROM commits-2020.01.01

-- DBCrust automatically rewrites to:
SELECT sha, author.login, committer.login, commit.message, ... FROM "commits-2020.01.01"
-- Note: 2 array fields excluded: parents.html_url, parents.sha
```

### Comprehensive Field Mapping

Use `\d` to see complete field structure with capabilities:

```sql
\d commits-2020.01.01
```

Shows all fields including:
- **Nested fields**: `author.login`, `commit.message`, `stats.additions`
- **Multi-fields**: `field.keyword`, `field.text` variants
- **Field capabilities**: What operations each field supports

#### Field Capabilities Explained

| Capability | Description | Example Fields |
|------------|-------------|----------------|
| `select` | Can be used in SELECT queries | Most fields |
| `filter` | Efficient filtering/WHERE clauses | keyword, numeric, date |
| `search` | Full-text search capabilities | text fields |
| `group` | GROUP BY operations | keyword, numeric, date |
| `agg` | Aggregation functions (COUNT, SUM, etc.) | keyword, numeric, date |
| `sort` | ORDER BY operations | keyword, numeric, date |
| `math` | Mathematical operations | numeric fields |
| `range` | Range queries (BETWEEN, >, <) | date, numeric |
| `geo` | Geographic queries | geo_point, geo_shape |

## Essential Commands

### Exploration Commands

```sql
-- List all indices
\dt
\d

-- Describe specific index (shows all fields + capabilities)
\d "logs-2024.01.01"

-- Show server information
\conninfo
```

### Query Examples

```sql
-- Basic queries (auto-quoted index names)
SELECT * FROM logs-2024.01.01 LIMIT 10;
SELECT COUNT(*) FROM user-events;

-- Field selection with nested fields
SELECT author.login, commit.message, stats.additions
FROM commits-2020.01.01
WHERE author.login = 'username';

-- Aggregations (use .keyword fields for grouping)
SELECT author.login, COUNT(*) as commits
FROM commits-2020.01.01
GROUP BY author.login
ORDER BY commits DESC
LIMIT 10;

-- Date range queries
SELECT * FROM logs-2024.01.01
WHERE timestamp BETWEEN '2024-01-01' AND '2024-01-02';

-- Text search (use text fields)
SELECT commit.message FROM commits-2020.01.01
WHERE commit.message LIKE '%fix%';
```

## Advanced Features

### Column Selection Mode

For indices with many fields, use column selection:

```sql
-- Enable column selection mode
\cs

-- Now queries will prompt you to select which columns to display
SELECT * FROM wide-index-with-100-fields;
-- (Interactive column selection appears)
```

### Query Optimization Tips

1. **Use appropriate field types**:
   - `.keyword` fields for exact matches, grouping, sorting
   - `.text` fields for full-text search
   - Numeric fields for mathematical operations

2. **Leverage field capabilities**:
   ```sql
   -- Good: Use keyword field for grouping
   SELECT author.login, COUNT(*) FROM commits GROUP BY author.login;

   -- Avoid: Using text field for grouping (will fail)
   SELECT author.name, COUNT(*) FROM commits GROUP BY author.name; -- Error
   ```

3. **Index name patterns**:
   - Indices with hyphens/dots are auto-quoted
   - Use `\dt` to see correct quoting hints

### Container Environment Variables

When using Docker containers, DBCrust looks for:
- `ELASTIC_USERNAME` / `ES_USERNAME`
- `ELASTIC_PASSWORD` / `ES_PASSWORD`
- `ELASTIC_INDEX` / `ES_INDEX` (default index)

## Limitations

DBCrust uses Elasticsearch's SQL API, which has some limitations:

1. **No JOINs**: Elasticsearch doesn't support table joins
2. **Array fields**: Cannot select array fields directly (auto-excluded)
3. **Nested queries**: Complex nested queries may need special syntax
4. **Aggregation limits**: Some advanced aggregations may not be available

## Configuration

### Connection Options

```bash
# SSL Configuration
dbcrust "elasticsearch://localhost:9200?ssl=true&verify_certs=false"

# Timeout settings
dbcrust "elasticsearch://localhost:9200?timeout=30"
```

### Saved Sessions

```sql
-- Save current connection
\ss production_elasticsearch

-- Connect using saved session
dbcrust session://production_elasticsearch
```

### SSH Tunneling

```bash
# Connect through SSH tunnel
dbcrust elasticsearch://localhost:9200 --ssh-tunnel user@jumphost.com

# Configure automatic tunneling for internal hosts
# In ~/.config/dbcrust/config.toml:
[ssh_tunnel_patterns]
"^es\.internal\..*\.com$" = "user@jumphost.example.com:2222"
```

## Troubleshooting

### Common Issues

1. **Array field errors**:
   - **Issue**: `Arrays are not supported` error
   - **Solution**: Use specific field selection instead of `SELECT *`, or let DBCrust auto-exclude arrays

2. **Index name errors**:
   - **Issue**: `parsing_exception` with special characters
   - **Solution**: DBCrust auto-quotes names, ensure you're using the latest version

3. **Connection failures**:
   - **Issue**: Cannot connect to Elasticsearch
   - **Solution**: Check network, authentication, SSL settings

### Debug Mode

Enable debug logging to troubleshoot issues:

```toml
# In ~/.config/dbcrust/config.toml
[logging]
level = "debug"
```

This shows:
- Query rewriting process
- Auto-quoting decisions
- Field mapping analysis
- Array field exclusions

## Best Practices

1. **Use `\d` to explore**: Always check field structure before writing complex queries
2. **Leverage capabilities**: Match your query operations to field capabilities
3. **Save sessions**: Use saved sessions for frequently accessed clusters
4. **Monitor performance**: Use `\e` to enable EXPLAIN mode for query analysis
5. **Container integration**: Use Docker URLs for containerized environments

## Examples

### Complete Workflow

```bash
# Connect to Elasticsearch cluster
dbcrust elasticsearch://elastic:password@localhost:9200

# Explore indices
\dt

# Examine structure with all capabilities
\d "logs-2024.01.01"

# Query with auto-quoting and field selection
SELECT timestamp, level, message.keyword
FROM logs-2024.01.01
WHERE level = 'ERROR'
ORDER BY timestamp DESC
LIMIT 20;

# Enable column selection for wide results
\cs
SELECT * FROM complex-index-with-many-fields;

# Save session for future use
\ss production_logs
```

This integration makes Elasticsearch feel like a traditional SQL database while respecting its unique characteristics and limitations.
