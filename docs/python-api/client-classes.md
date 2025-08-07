# Client Classes

DBCrust provides rich client classes for more advanced database operations. These classes offer object-oriented interfaces, connection management, and database-specific functionality beyond simple command execution.

## 🏗️ Core Client Classes

### Database Client

The primary client class for database operations:

```python
from dbcrust import Database

# Create database client
db = Database("postgres://user@localhost/myapp")

# Execute queries
result = db.execute("SELECT * FROM users WHERE active = true")
users = db.query("SELECT id, name, email FROM users LIMIT 10")

# Get metadata
tables = db.list_tables()
columns = db.describe_table("users")

# Close connection
db.close()
```

### Context Manager Support

Automatic connection management:

```python
from dbcrust import Database

# Automatic cleanup with context manager
with Database("postgres://localhost/myapp") as db:
    result = db.execute("SELECT COUNT(*) FROM users")
    print(f"Total users: {result.scalar()}")
    # Connection automatically closed
```

## 📊 Result Objects

### Query Results

Rich result objects with multiple access patterns:

```python
with Database("postgres://localhost/myapp") as db:
    result = db.execute("SELECT id, name, email, created_at FROM users LIMIT 5")

    # Row count
    print(f"Found {len(result)} users")

    # Iterate over rows
    for row in result:
        print(f"User: {row['name']} ({row['email']})")

    # Access by index
    first_user = result[0]
    print(f"First user: {first_user['name']}")

    # Convert to list of dictionaries
    user_list = result.to_dict()

    # Convert to pandas DataFrame (if pandas installed)
    df = result.to_dataframe()
```

### Performance Information

Results include performance metadata:

```python
with Database("postgres://localhost/myapp") as db:
    result = db.execute("SELECT COUNT(*) FROM large_table WHERE status = 'active'")

    # Performance metrics
    print(f"Query duration: {result.duration}ms")
    print(f"Rows returned: {result.row_count}")
    print(f"Query plan cost: {result.plan_cost}")

    # Check for performance issues
    if result.duration > 1000:
        print("⚠️ Slow query detected")

    if result.full_scan_detected:
        print("⚠️ Full table scan detected - consider adding index")
```

## 🔧 Database-Specific Clients

### PostgreSQL Client

Enhanced PostgreSQL functionality:

```python
from dbcrust import PostgresClient

with PostgresClient("postgres://user@localhost/db") as pg:
    # PostgreSQL-specific methods
    databases = pg.list_databases()
    schemas = pg.list_schemas()
    indexes = pg.list_indexes("users")

    # EXPLAIN support
    plan = pg.explain("SELECT * FROM users WHERE email = ?", ["user@example.com"])
    print(f"Query cost: {plan.cost}")

    # Table statistics
    stats = pg.table_stats("users")
    print(f"Table size: {stats.size_pretty}")
    print(f"Row count: {stats.row_count}")
```

### MySQL Client

MySQL-specific functionality:

```python
from dbcrust import MySQLClient

with MySQLClient("mysql://user@localhost/db") as mysql:
    # MySQL-specific methods
    databases = mysql.show_databases()
    tables = mysql.show_tables()

    # Storage engine information
    engine_info = mysql.table_engine("users")
    print(f"Storage engine: {engine_info}")

    # Process list
    processes = mysql.show_processlist()
    for proc in processes:
        print(f"Process {proc.id}: {proc.info}")
```

### SQLite Client

SQLite-specific functionality:

```python
from dbcrust import SQLiteClient

with SQLiteClient("sqlite:///app.db") as sqlite:
    # SQLite-specific methods
    tables = sqlite.list_tables()

    # Pragma support
    sqlite.execute("PRAGMA optimize")

    # Database info
    info = sqlite.database_info()
    print(f"Page size: {info.page_size}")
    print(f"Database size: {info.size_bytes}")
```

## ⚙️ Configuration and Options

### Client Configuration

Customize client behavior:

```python
from dbcrust import Database, DatabaseConfig

# Create custom configuration
config = DatabaseConfig(
    timeout=30,           # Connection timeout in seconds
    max_retries=3,       # Retry failed connections
    pool_size=10,        # Connection pool size
    performance_tracking=True,  # Enable performance monitoring
    auto_explain=True,   # Automatically explain slow queries
    show_progress=True   # Show progress for long-running queries
)

# Use configuration
with Database("postgres://localhost/db", config=config) as db:
    result = db.execute("SELECT * FROM large_table")
```

### Connection Options

Advanced connection settings:

```python
from dbcrust import Database

# SSL configuration
db = Database(
    "postgres://user@host/db?sslmode=require",
    ssl_cert="/path/to/client.crt",
    ssl_key="/path/to/client.key"
)

# SSH tunnel configuration
db = Database(
    "postgres://user@internal-host/db",
    ssh_tunnel="user@jumphost.com:2222",
    ssh_key="/path/to/ssh/key"
)

# Vault integration
db = Database(
    "vault://role@database/postgres",
    vault_addr="https://vault.company.com",
    vault_token="your-token"
)
```

## 🎯 Advanced Features

### Transaction Management

Explicit transaction control:

```python
with Database("postgres://localhost/myapp") as db:
    # Manual transaction
    with db.transaction() as tx:
        db.execute("UPDATE accounts SET balance = balance - 100 WHERE id = 1")
        db.execute("UPDATE accounts SET balance = balance + 100 WHERE id = 2")
        # Automatically committed on success, rolled back on exception

    # Rollback-only transactions (for testing)
    with db.transaction(rollback=True) as tx:
        result = db.execute("SELECT COUNT(*) FROM users")
        db.execute("INSERT INTO users (name) VALUES ('Test User')")
        new_count = db.execute("SELECT COUNT(*) FROM users")
        print(f"Count changed from {result.scalar()} to {new_count.scalar()}")
        # Transaction automatically rolled back
```

### Prepared Statements

Efficient parameterized queries:

```python
with Database("postgres://localhost/myapp") as db:
    # Prepare statement
    stmt = db.prepare("SELECT * FROM users WHERE created_at > ? AND status = ?")

    # Execute multiple times with different parameters
    recent_active = stmt.execute(["2024-01-01", "active"])
    recent_pending = stmt.execute(["2024-01-01", "pending"])

    print(f"Active users: {len(recent_active)}")
    print(f"Pending users: {len(recent_pending)}")
```

### Streaming Results

Handle large result sets efficiently:

```python
with Database("postgres://localhost/myapp") as db:
    # Stream large result set
    for batch in db.stream("SELECT * FROM huge_table", batch_size=1000):
        # Process batch of 1000 rows
        for row in batch:
            process_row(row)

        # Memory usage remains constant
        print(f"Processed batch of {len(batch)} rows")
```

### Query Builder (Optional)

Programmatic query construction:

```python
from dbcrust import Database, QueryBuilder

with Database("postgres://localhost/myapp") as db:
    # Build query programmatically
    query = (QueryBuilder()
             .select("id", "name", "email")
             .from_table("users")
             .where("active = ?", True)
             .where("created_at > ?", "2024-01-01")
             .order_by("name")
             .limit(10))

    result = db.execute(query)
```

## 🔍 Introspection and Metadata

### Schema Introspection

Explore database structure:

```python
with Database("postgres://localhost/myapp") as db:
    # Get all tables
    tables = db.list_tables()

    for table in tables:
        print(f"\nTable: {table.name}")

        # Get columns
        columns = db.describe_table(table.name)
        for col in columns:
            print(f"  {col.name}: {col.type} {'NULL' if col.nullable else 'NOT NULL'}")

        # Get indexes
        indexes = db.list_indexes(table.name)
        for idx in indexes:
            print(f"  Index: {idx.name} on {idx.columns}")

        # Get foreign keys
        foreign_keys = db.list_foreign_keys(table.name)
        for fk in foreign_keys:
            print(f"  FK: {fk.column} -> {fk.referenced_table}.{fk.referenced_column}")
```

### Performance Analysis

Built-in performance monitoring:

```python
with Database("postgres://localhost/myapp") as db:
    # Enable performance monitoring
    db.enable_performance_monitoring()

    # Execute queries
    db.execute("SELECT * FROM users WHERE email LIKE '%@example.com'")
    db.execute("SELECT COUNT(*) FROM orders WHERE created_at > '2024-01-01'")

    # Get performance report
    report = db.get_performance_report()

    print(f"Total queries: {report.query_count}")
    print(f"Average duration: {report.avg_duration}ms")
    print(f"Slow queries: {report.slow_query_count}")

    # Get optimization suggestions
    suggestions = report.get_suggestions()
    for suggestion in suggestions:
        print(f"💡 {suggestion.message}")
```

## 🧪 Testing Support

### Test Database Management

Built-in testing utilities:

```python
from dbcrust import TestDatabase
import pytest

@pytest.fixture
def test_db():
    """Create test database with sample data"""
    with TestDatabase("sqlite:///:memory:") as db:
        # Create schema
        db.execute("""
            CREATE TABLE users (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                email TEXT UNIQUE,
                active BOOLEAN DEFAULT TRUE
            )
        """)

        # Insert test data
        db.execute_many(
            "INSERT INTO users (name, email) VALUES (?, ?)",
            [
                ("Alice", "alice@test.com"),
                ("Bob", "bob@test.com"),
                ("Charlie", "charlie@test.com")
            ]
        )

        yield db

def test_user_queries(test_db):
    """Test user-related queries"""
    # Count total users
    result = test_db.execute("SELECT COUNT(*) FROM users")
    assert result.scalar() == 3

    # Test active users
    active = test_db.execute("SELECT COUNT(*) FROM users WHERE active = true")
    assert active.scalar() == 3
```

## 🚨 Error Handling and Logging

### Comprehensive Error Handling

```python
from dbcrust import Database, DatabaseError, ConnectionError, QueryError

def robust_database_operation():
    try:
        with Database("postgres://localhost/myapp") as db:
            result = db.execute("SELECT * FROM users")
            return result.to_dict()

    except ConnectionError as e:
        print(f"Connection failed: {e}")
        return None

    except QueryError as e:
        print(f"Query failed: {e.message}")
        print(f"SQL: {e.query}")
        return None

    except DatabaseError as e:
        print(f"Database error: {e}")
        return None
```

### Logging Integration

Built-in logging support:

```python
import logging
from dbcrust import Database

# Configure logging
logging.basicConfig(level=logging.INFO)

with Database("postgres://localhost/myapp") as db:
    # Enable query logging
    db.enable_query_logging(level=logging.DEBUG)

    # Queries are automatically logged
    result = db.execute("SELECT COUNT(*) FROM users")
    # LOG: [2024-01-15 14:30:00] QUERY: SELECT COUNT(*) FROM users [Duration: 25ms]
```

## 📚 See Also

- **[Python API Overview](/dbcrust/python-api/overview/)** - API introduction and patterns
- **[Direct Execution](/dbcrust/python-api/direct-execution/)** - Simple function-based API
- **[Examples & Use Cases](/dbcrust/python-api/examples/)** - Real-world integration patterns

---

<div align="center">
    <strong>Ready for advanced database operations?</strong><br>
    <a href="/dbcrust/python-api/examples/" class="md-button md-button--primary">Examples & Use Cases</a>
    <a href="/dbcrust/python-api/direct-execution/" class="md-button">Direct Execution</a>
</div>
