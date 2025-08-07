# Direct Execution

DBCrust's direct execution API provides simple functions for running SQL queries and commands directly from Python. This is the most straightforward way to integrate DBCrust into scripts and applications.

## 🚀 Core Functions

### `run_command(connection_url, command)`

Execute SQL queries or backslash commands:

```python
import dbcrust

# SQL queries
result = dbcrust.run_command(
    "postgres://user@localhost/myapp",
    "SELECT id, name, email FROM users WHERE active = true"
)

# Backslash commands
tables = dbcrust.run_command(
    "postgres://user@localhost/myapp",
    "\\dt"
)

# Complex queries
analytics = dbcrust.run_command(
    "postgres://analyst@warehouse/data",
    """
    SELECT DATE_TRUNC('month', created_at) as month,
           COUNT(*) as orders,
           SUM(amount) as revenue
    FROM orders
    WHERE created_at >= '2024-01-01'
    GROUP BY month
    ORDER BY month
    """
)
```

### `run_with_url(connection_url, args_list)`

Execute with CLI arguments for advanced control:

```python
import dbcrust

# Execute with debug mode
result = dbcrust.run_with_url(
    "postgres://user@localhost/myapp",
    ["--debug", "-c", "SELECT COUNT(*) FROM users"]
)

# JSON output format
json_result = dbcrust.run_with_url(
    "postgres://user@localhost/myapp",
    ["-o", "json", "-c", "SELECT * FROM products LIMIT 5"]
)

# Multiple commands
dbcrust.run_with_url(
    "postgres://user@localhost/myapp",
    ["--no-banner", "-c", "\\dt", "-c", "\\l"]
)
```

### `run_cli(connection_url=None)`

Launch interactive CLI:

```python
import dbcrust

# Launch CLI with specific connection
dbcrust.run_cli("postgres://user@localhost/myapp")

# Launch CLI with connection selection
dbcrust.run_cli()

# Launch from script for debugging
if __name__ == "__main__":
    dbcrust.run_cli("session://development_db")
```

## 📊 Return Values and Output

### String Results

All functions return string output that can be parsed:

```python
import json

# Numeric results
count_result = dbcrust.run_command(
    "postgres://localhost/db",
    "SELECT COUNT(*) as total FROM users"
)
print(f"Total users: {count_result.strip()}")

# JSON formatted results (when using -o json)
json_data = dbcrust.run_with_url(
    "postgres://localhost/db",
    ["-o", "json", "-c", "SELECT id, name FROM users LIMIT 3"]
)
users = json.loads(json_data)
for user in users:
    print(f"User: {user['name']} (ID: {user['id']})")

# Table descriptions
table_info = dbcrust.run_command("postgres://localhost/db", "\\d users")
print("Users table structure:")
print(table_info)
```

### Error Handling

Handle connection and query errors:

```python
def safe_execute(connection_url, command):
    """Execute command with error handling"""
    try:
        result = dbcrust.run_command(connection_url, command)
        return {"success": True, "result": result}
    except Exception as e:
        error_message = str(e)
        if "connection refused" in error_message:
            return {"success": False, "error": "Database connection failed"}
        elif "authentication failed" in error_message:
            return {"success": False, "error": "Invalid credentials"}
        elif "syntax error" in error_message:
            return {"success": False, "error": f"SQL syntax error: {error_message}"}
        else:
            return {"success": False, "error": f"Unknown error: {error_message}"}

# Use safe execution
result = safe_execute("postgres://localhost/db", "SELECT * FROM users")
if result["success"]:
    print("Query successful:", result["result"])
else:
    print("Query failed:", result["error"])
```

## 🔗 Connection URL Patterns

### Standard Database URLs

```python
# PostgreSQL
dbcrust.run_command("postgres://user:pass@host:5432/database", "SELECT 1")
dbcrust.run_command("postgresql://user@host/db", "SELECT 1")

# MySQL
dbcrust.run_command("mysql://user:pass@host:3306/database", "SHOW TABLES")

# SQLite
dbcrust.run_command("sqlite:///path/to/database.db", "SELECT 1")
dbcrust.run_command("sqlite://:memory:", "CREATE TABLE test (id INTEGER)")
```

### Advanced Connection Types

```python
# Saved sessions
dbcrust.run_command("session://production_db", "SELECT COUNT(*) FROM orders")

# Recent connections (interactive selection)
dbcrust.run_command("recent://", "\\dt")

# Docker containers
dbcrust.run_command("docker://postgres-container/myapp", "SELECT version()")
dbcrust.run_command("docker://user:pass@mysql-container/testdb", "SHOW DATABASES")

# HashiCorp Vault
dbcrust.run_command("vault://app-role@database/prod-postgres", "SELECT 1")
```

## ⚡ Performance and Best Practices

### Connection Reuse

For multiple queries, consider using sessions:

```python
# Instead of multiple connections
# (inefficient - creates new connection each time)
for table in ['users', 'orders', 'products']:
    result = dbcrust.run_command(f"postgres://localhost/db", f"SELECT COUNT(*) FROM {table}")
    print(f"{table}: {result.strip()}")

# Better: Use session for multiple queries
connection_url = "postgres://localhost/db"

# Get all table counts in one query
result = dbcrust.run_command(connection_url, """
    SELECT 'users' as table_name, COUNT(*) as count FROM users
    UNION ALL
    SELECT 'orders', COUNT(*) FROM orders
    UNION ALL
    SELECT 'products', COUNT(*) FROM products
""")
```

### Query Batching

Execute multiple statements efficiently:

```python
# Batch multiple operations
batch_operations = """
    BEGIN;
    UPDATE users SET last_login = NOW() WHERE id = 123;
    INSERT INTO user_activity (user_id, activity, timestamp)
    VALUES (123, 'login', NOW());
    COMMIT;
"""

result = dbcrust.run_command("postgres://localhost/db", batch_operations)
```

### Output Format Selection

Choose appropriate output formats:

```python
# Default output (human-readable tables)
table_result = dbcrust.run_command("postgres://localhost/db", "SELECT * FROM users LIMIT 3")

# JSON output (for programmatic parsing)
json_result = dbcrust.run_with_url(
    "postgres://localhost/db",
    ["-o", "json", "-c", "SELECT id, name, email FROM users LIMIT 3"]
)

# CSV output (for data export)
csv_result = dbcrust.run_with_url(
    "postgres://localhost/db",
    ["-o", "csv", "-c", "SELECT * FROM sales_data WHERE date >= '2024-01-01'"]
)
```

## 🛠️ Integration Patterns

### Script Automation

```python
#!/usr/bin/env python3
import dbcrust
import sys

def backup_user_data(connection_url, output_file):
    """Backup user data to CSV"""
    try:
        result = dbcrust.run_with_url(
            connection_url,
            ["-o", "csv", "-c", "SELECT * FROM users ORDER BY created_at"]
        )

        with open(output_file, 'w') as f:
            f.write(result)

        print(f"User data backed up to {output_file}")
        return True
    except Exception as e:
        print(f"Backup failed: {e}")
        return False

if __name__ == "__main__":
    if len(sys.argv) != 3:
        print("Usage: python backup.py <connection_url> <output_file>")
        sys.exit(1)

    success = backup_user_data(sys.argv[1], sys.argv[2])
    sys.exit(0 if success else 1)
```

### Data Pipeline Integration

```python
import dbcrust
import logging

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

def extract_transform_load():
    """ETL pipeline example"""

    # Extract
    logger.info("Extracting data from source database...")
    source_data = dbcrust.run_with_url(
        "mysql://reader@source-db/crm",
        ["-o", "json", "-c", """
            SELECT id, email, first_name, last_name, created_at
            FROM customers
            WHERE updated_at >= DATE_SUB(NOW(), INTERVAL 1 HOUR)
        """]
    )

    customers = json.loads(source_data)
    logger.info(f"Extracted {len(customers)} customers")

    # Transform & Load
    for customer in customers:
        # Simple transformation
        full_name = f"{customer['first_name']} {customer['last_name']}"

        # Load to destination
        dbcrust.run_command(
            "postgres://writer@warehouse/analytics",
            f"""
            INSERT INTO customers (source_id, email, full_name, created_at)
            VALUES ({customer['id']}, '{customer['email']}',
                    '{full_name}', '{customer['created_at']}')
            ON CONFLICT (source_id) DO UPDATE SET
                email = EXCLUDED.email,
                full_name = EXCLUDED.full_name
            """
        )

    logger.info("ETL pipeline completed successfully")

if __name__ == "__main__":
    extract_transform_load()
```

### Health Check Monitoring

```python
import dbcrust
from datetime import datetime
import json

def database_health_check(databases):
    """Monitor multiple databases"""
    results = {}

    for name, connection_url in databases.items():
        try:
            start_time = datetime.now()

            # Check basic connectivity
            version = dbcrust.run_command(connection_url, "SELECT version()")

            # Check performance
            connection_count = dbcrust.run_with_url(
                connection_url,
                ["-o", "json", "-c", "SELECT COUNT(*) as active_connections FROM pg_stat_activity"]
            )

            end_time = datetime.now()
            response_time = (end_time - start_time).total_seconds()

            results[name] = {
                "status": "healthy",
                "response_time": response_time,
                "version": version.strip(),
                "connections": json.loads(connection_count)[0]["active_connections"]
            }

        except Exception as e:
            results[name] = {
                "status": "unhealthy",
                "error": str(e)
            }

    return results

# Monitor multiple databases
databases = {
    "production": "postgres://monitor@prod-db/main",
    "analytics": "postgres://monitor@analytics-db/warehouse",
    "cache": "postgres://monitor@cache-db/sessions"
}

health = database_health_check(databases)
for db_name, status in health.items():
    if status["status"] == "healthy":
        print(f"✅ {db_name}: {status['response_time']:.3f}s response time")
    else:
        print(f"❌ {db_name}: {status['error']}")
```

## 📚 See Also

- **[Python API Overview](/dbcrust/python-api/overview/)** - API introduction and patterns
- **[Client Classes](/dbcrust/python-api/client-classes/)** - Advanced client APIs
- **[Examples & Use Cases](/dbcrust/python-api/examples/)** - Real-world integration patterns

---

<div align="center">
    <strong>Ready to execute queries programmatically?</strong><br>
    <a href="/dbcrust/python-api/client-classes/" class="md-button md-button--primary">Client Classes</a>
    <a href="/dbcrust/python-api/examples/" class="md-button">Examples</a>
</div>
