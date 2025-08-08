# Python API Overview

DBCrust provides a comprehensive Python API for database operations, enabling direct integration into Python applications, scripts, and automation workflows. The API offers both high-level client classes and direct execution methods for maximum flexibility, with robust error handling through specific exception types.

## 🚀 Quick Start

### Basic Python Integration

```python
import dbcrust

# Direct command execution
result = dbcrust.run_command(
    "postgres://user@localhost/myapp",
    "SELECT * FROM users WHERE active = true"
)

# Launch interactive CLI from Python
dbcrust.run_cli("postgres://user@localhost/myapp")
```

### Installation

```bash
# Using uv (recommended for projects)
uv add dbcrust

# Using uv tool (recommended for CLI usage)
uv tool install dbcrust
```

## 🏗️ API Patterns

### 1. Enhanced Connection API (Recommended)

Modern mysql.connector-style API with connection management and cursor support:

```python
import dbcrust

# Context manager with automatic cleanup
with dbcrust.connect("postgres://user@localhost/myapp") as connection:
    # Get server information
    server_info = connection.get_server_info()
    print(f"Connected to: {server_info.database_type} {server_info.version}")

    # Use cursor for query execution
    with connection.cursor() as cursor:
        # Execute single query
        cursor.execute("SELECT * FROM users WHERE active = true")
        users = cursor.fetchall()

        # Execute multiple statements
        script = """
            CREATE TEMP TABLE temp_stats AS
            SELECT status, COUNT(*) as count FROM users GROUP BY status;

            SELECT * FROM temp_stats ORDER BY count DESC;

            DROP TABLE temp_stats;
        """
        cursor.executescript(script)

        # Navigate through result sets
        temp_results = cursor.fetchall()  # First result set (CREATE has no results)
        cursor.nextset()

        stats = cursor.fetchall()  # Second result set (SELECT results)
        cursor.nextset()

        # Process results
        for row in stats:
            print(f"Status: {row[0]}, Count: {row[1]}")

# Connection automatically closed
```

### 2. Direct Command Execution

Execute SQL queries and backslash commands directly:

```python
import dbcrust

# SQL queries
result = dbcrust.run_command(
    "postgres://postgres@localhost/myapp",
    "SELECT name, email FROM users LIMIT 10"
)

# Backslash commands
tables = dbcrust.run_command("postgres://postgres@localhost/myapp", "\\dt")
databases = dbcrust.run_command("postgres://postgres@localhost/myapp", "\\l")
```

### 3. Programmatic Execution with CLI Options

```python
import dbcrust

# Execute with CLI arguments
result = dbcrust.run_with_url(
    "postgres://postgres@localhost/myapp",
    ["--debug", "--no-banner", "-c", "\\dt"]
)

# JSON output for automation
dbcrust.run_with_url(
    "postgres://postgres@localhost/myapp",
    ["-o", "json", "-c", "SELECT * FROM users LIMIT 5"]
)
```

### 4. Interactive CLI Integration

```python
import dbcrust

# Launch full interactive CLI
dbcrust.run_cli("postgres://postgres@localhost/myapp")

# Interactive connection selection
dbcrust.run_cli()
```

## 🎯 Common Use Cases

### Data Analysis & ETL

```python
import dbcrust
import pandas as pd
import json

# Extract data
result = dbcrust.run_command(
    "postgres://analyst@warehouse/analytics",
    """
    SELECT date_trunc('month', created_at) as month,
           COUNT(*) as orders,
           SUM(amount) as revenue
    FROM orders
    WHERE created_at >= '2024-01-01'
    GROUP BY month ORDER BY month
    """
)

# Convert to pandas DataFrame
df = pd.DataFrame(json.loads(result))
```

### Database Administration

```python
import dbcrust

def health_check(connection_url):
    """Database health check"""
    # Check version
    version = dbcrust.run_command(connection_url, "SELECT version()")

    # Check table sizes
    sizes = dbcrust.run_command(connection_url, """
        SELECT tablename, pg_size_pretty(pg_total_relation_size(tablename))
        FROM pg_tables WHERE schemaname = 'public'
        ORDER BY pg_total_relation_size(tablename) DESC LIMIT 5
    """)

    return {"version": version, "top_tables": sizes}
```

### Testing & Development

```python
import dbcrust
import pytest

@pytest.fixture
def test_db():
    """Setup test database"""
    dbcrust.run_command("sqlite:///test.db", """
        CREATE TABLE IF NOT EXISTS users (id INTEGER, name TEXT, email TEXT);
        INSERT INTO users VALUES (1, 'Alice', 'alice@test.com');
    """)
    yield "sqlite:///test.db"
    dbcrust.run_command("sqlite:///test.db", "DROP TABLE users")

def test_user_count(test_db):
    result = dbcrust.run_command(test_db, "SELECT COUNT(*) as count FROM users")
    data = json.loads(result)
    assert data[0]['count'] == 1
```

## 🔧 Advanced Features

### All Connection Types Supported

```python
# Standard databases
dbcrust.run_command("postgres://user@host:5432/db", "SELECT 1")
dbcrust.run_command("mysql://user@host:3306/db", "SELECT 1")
dbcrust.run_command("sqlite:///path/to/file.db", "SELECT 1")

# Advanced connection types
dbcrust.run_command("session://saved_session", "SELECT 1")
dbcrust.run_command("docker://container/db", "SELECT 1")
dbcrust.run_command("vault://role@mount/database", "SELECT 1")
```

### SSH Tunneling & Vault Integration

```python
# SSH tunneling (configured automatically)
result = dbcrust.run_command(
    "postgres://user@db.internal.company.com/prod",
    "SELECT COUNT(*) FROM orders"
)

# Vault dynamic credentials
result = dbcrust.run_command(
    "vault://app-role@database/postgres-prod",
    "SELECT COUNT(*) FROM sensitive_data"
)
```

### Error Handling

DBCrust provides specific exception types for robust error handling:

```python
from dbcrust import (
    DbcrustConnectionError,
    DbcrustCommandError,
    DbcrustConfigError,
    DbcrustArgumentError
)

def safe_query(connection_url, query):
    """Execute query with proper exception handling"""
    try:
        result = dbcrust.run_command(connection_url, query)
        return json.loads(result)
    except DbcrustConnectionError as e:
        return {"error": "Database unreachable", "details": str(e)}
    except DbcrustCommandError as e:
        return {"error": "Query failed", "details": str(e)}
    except DbcrustConfigError as e:
        return {"error": "Configuration issue", "details": str(e)}
    except DbcrustArgumentError as e:
        return {"error": "Invalid arguments", "details": str(e)}
```

See the **[Error Handling Guide](/dbcrust/python-api/error-handling/)** for comprehensive examples.

## 🔍 Django Integration

DBCrust provides comprehensive Django integration with automatic database discovery and powerful ORM analysis:

### Automatic Database Connection

```python
from dbcrust.django import connect

# Use your Django DATABASES configuration automatically
with connect() as connection:
    server_info = connection.get_server_info()
    print(f"Connected to: {server_info.database_type} {server_info.version}")

    with connection.cursor() as cursor:
        cursor.execute("SELECT * FROM auth_user WHERE is_active = %s", (True,))
        active_users = cursor.fetchall()
        print(f"Found {len(active_users)} active users")

# Use specific database alias
with connect("analytics") as connection:
    with connection.cursor() as cursor:
        cursor.execute("SELECT COUNT(*) FROM events")
        event_count = cursor.fetchone()[0]
```

### ORM Performance Analysis

```python
from dbcrust.django import analyzer

# Analyze Django ORM performance issues
with analyzer.analyze() as analysis:
    books = Book.objects.all()
    for book in books:
        print(book.author.name)  # Detects N+1 queries

results = analysis.get_results()
print(f"Found {len(results.n_plus_one_issues)} N+1 query issues")
```

**Key Features:**
- **Automatic Django DATABASES integration** - No manual connection URLs needed
- **Multi-database support** - Work with all your Django databases
- **Enhanced cursor API** - mysql.connector-style operations
- **N+1 query detection** - Find ORM performance issues
- **Performance recommendations** - Get actionable insights
- **CI/CD integration support** - Automate performance testing

[**📖 Complete Django Integration Guide →**](/dbcrust/python-api/django-integration/)

## 📚 See Also

- **[Direct Execution](/dbcrust/python-api/direct-execution/)** - Detailed execution patterns
- **[Client Classes](/dbcrust/python-api/client-classes/)** - Advanced client APIs
- **[Examples & Use Cases](/dbcrust/python-api/examples/)** - Real-world integration patterns

---

<div align="center">
    <strong>Ready to integrate DBCrust into your Python applications?</strong><br>
    <a href="/dbcrust/python-api/direct-execution/" class="md-button md-button--primary">Direct Execution</a>
    <a href="/dbcrust/python-api/examples/" class="md-button">Examples</a>
</div>
