# Python API Overview

DBCrust isn't just a CLI tool - it's also a powerful Python library that brings all the features of the command-line interface to your Python applications. Whether you're building data pipelines, automation scripts, or interactive notebooks, DBCrust's Python API provides a seamless bridge to your databases.

## 🐍 Why Use DBCrust in Python?

### Unified Database Interface

```python
import dbcrust

# Same API works with any database
postgres_result = dbcrust.run_command(
    "postgresql://user@localhost/db", 
    "SELECT COUNT(*) FROM users"
)

mysql_result = dbcrust.run_command(
    "mysql://user@localhost/db", 
    "SELECT COUNT(*) FROM customers"
)

sqlite_result = dbcrust.run_command(
    "sqlite:///data.db", 
    "SELECT COUNT(*) FROM products"
)
```

### Rich Feature Set

- **Smart Autocompletion** - Tab completion in Jupyter notebooks
- **Query Analysis** - EXPLAIN plans accessible in Python
- **Secure Connections** - SSH tunnels and Vault integration
- **Performance** - Rust-powered speed with Python convenience
- **Interactive CLI** - Launch full CLI from Python scripts

## 🚀 Installation

Install DBCrust with Python support:

```bash
# Using uv (recommended)
uv add dbcrust

# Using pip
pip install dbcrust
```

## 📚 API Patterns

DBCrust provides three main patterns for Python integration:

### 1. Direct Command Execution

Execute SQL queries and backslash commands directly:

```python
import dbcrust

# Execute SQL queries
result = dbcrust.run_command(
    "postgresql://postgres@localhost/myapp",
    "SELECT name, email FROM users WHERE created_at > current_date - interval '7 days'"
)

# Execute backslash commands
tables = dbcrust.run_command(
    "postgresql://postgres@localhost/myapp",
    "\\dt"
)

databases = dbcrust.run_command(
    "postgresql://postgres@localhost/myapp", 
    "\\l"
)
```

### 2. Interactive CLI Integration

Launch the full interactive CLI from Python:

```python
import dbcrust

# Launch interactive CLI
dbcrust.run_cli("postgresql://postgres@localhost/myapp")

# Or let user choose connection interactively
dbcrust.run_cli()
```

### 3. Database Client Classes

Use rich client objects for specific database types:

```python
from dbcrust import PostgresClient

# Create client
client = PostgresClient(
    host="localhost",
    port=5432,
    user="postgres",
    password="secret",
    dbname="myapp"
)

# Execute queries
results = client.execute("SELECT * FROM users LIMIT 10")
tables = client.list_tables()
databases = client.list_databases()
```

## 🎯 Common Use Cases

### Data Analysis Workflows

```python
import dbcrust
import pandas as pd

# Extract data with complex query
query = """
SELECT 
    date_trunc('month', created_at) as month,
    status,
    COUNT(*) as count,
    AVG(amount) as avg_amount
FROM orders 
WHERE created_at >= '2024-01-01'
GROUP BY month, status
ORDER BY month, status
"""

result = dbcrust.run_command(
    "postgresql://analyst@warehouse/analytics",
    query
)

# Convert to pandas DataFrame for analysis
df = pd.read_json(result)
```

### Database Administration

```python
import dbcrust
from datetime import datetime

def database_health_check(connection_url):
    """Comprehensive database health check"""
    
    # Check connection
    version = dbcrust.run_command(connection_url, "SELECT version()")
    
    # Check table sizes
    sizes = dbcrust.run_command(connection_url, """
        SELECT 
            schemaname,
            tablename,
            pg_size_pretty(pg_total_relation_size(schemaname||'.'||tablename)) as size
        FROM pg_tables 
        WHERE schemaname = 'public'
        ORDER BY pg_total_relation_size(schemaname||'.'||tablename) DESC
        LIMIT 10
    """)
    
    # Check active connections
    connections = dbcrust.run_command(connection_url, """
        SELECT state, COUNT(*) 
        FROM pg_stat_activity 
        WHERE state IS NOT NULL 
        GROUP BY state
    """)
    
    return {
        'timestamp': datetime.now(),
        'version': version,
        'top_tables': sizes,
        'connections': connections
    }

# Run health check
health = database_health_check("postgresql://admin@prod-db/main")
print(f"Health check completed at {health['timestamp']}")
```

### ETL Pipelines

```python
import dbcrust
import json

def sync_user_data():
    """Sync users from MySQL to PostgreSQL"""
    
    # Extract from MySQL
    mysql_users = dbcrust.run_command(
        "mysql://reader@legacy-db/crm",
        """
        SELECT 
            id, 
            email, 
            first_name, 
            last_name, 
            created_at,
            updated_at
        FROM users 
        WHERE updated_at >= DATE_SUB(NOW(), INTERVAL 1 HOUR)
        """
    )
    
    # Parse results
    users = json.loads(mysql_users)
    
    # Load into PostgreSQL
    for user in users:
        query = f"""
        INSERT INTO users (
            legacy_id, email, first_name, last_name, created_at, updated_at
        ) VALUES (
            {user['id']}, 
            '{user['email']}', 
            '{user['first_name']}', 
            '{user['last_name']}', 
            '{user['created_at']}',
            '{user['updated_at']}'
        )
        ON CONFLICT (legacy_id) DO UPDATE SET
            email = EXCLUDED.email,
            first_name = EXCLUDED.first_name,
            last_name = EXCLUDED.last_name,
            updated_at = EXCLUDED.updated_at
        """
        
        dbcrust.run_command(
            "postgresql://writer@data-warehouse/analytics",
            query
        )
    
    print(f"Synced {len(users)} users")

# Run ETL job
sync_user_data()
```

### Testing and Development

```python
import dbcrust
import pytest

class TestUserQueries:
    
    @pytest.fixture
    def test_db(self):
        """Setup test database"""
        # Create test data
        dbcrust.run_command(
            "postgresql://test@localhost/test_db",
            """
            INSERT INTO users (name, email, status) VALUES 
            ('Alice', 'alice@test.com', 'active'),
            ('Bob', 'bob@test.com', 'inactive'),
            ('Charlie', 'charlie@test.com', 'active')
            """
        )
        yield "postgresql://test@localhost/test_db"
        
        # Cleanup
        dbcrust.run_command(
            "postgresql://test@localhost/test_db",
            "TRUNCATE users"
        )
    
    def test_active_user_count(self, test_db):
        """Test active user counting"""
        result = dbcrust.run_command(
            test_db,
            "SELECT COUNT(*) as count FROM users WHERE status = 'active'"
        )
        
        data = json.loads(result)
        assert data[0]['count'] == 2
```

## 🔧 Advanced Features

### SSH Tunneling

```python
import dbcrust

# Automatic SSH tunneling (configured in ~/.config/dbcrust/config.toml)
result = dbcrust.run_command(
    "postgresql://user@db.internal.company.com/prod",
    "SELECT COUNT(*) FROM orders"
    # SSH tunnel automatically established
)

# Manual SSH tunnel
result = dbcrust.run_command(
    "postgresql://user@internal-db/prod",
    "SELECT COUNT(*) FROM orders",
    ssh_tunnel="user@jumphost.company.com:2222"
)
```

### Vault Integration

```python
import dbcrust
import os

# Set Vault environment
os.environ['VAULT_ADDR'] = 'https://vault.company.com'
os.environ['VAULT_TOKEN'] = 'your-token'

# Use Vault for dynamic credentials
result = dbcrust.run_command(
    "vault://app-role@database/postgres-prod",
    "SELECT COUNT(*) FROM sensitive_table"
)
```

### Docker Database Access

```python
import dbcrust

# Connect to containerized databases
postgres_result = dbcrust.run_command(
    "docker://postgres-container",
    "SELECT version()"
)

# With explicit credentials
mysql_result = dbcrust.run_command(
    "docker://user:pass@mysql-container/testdb",
    "SHOW TABLES"
)
```

## 🔍 Error Handling

```python
import dbcrust
import json

def safe_query(connection_url, query):
    """Execute query with proper error handling"""
    try:
        result = dbcrust.run_command(connection_url, query)
        return json.loads(result)
    except Exception as e:
        if "connection refused" in str(e):
            print("Database is not reachable")
        elif "authentication failed" in str(e):
            print("Invalid credentials")
        elif "syntax error" in str(e):
            print(f"SQL syntax error: {e}")
        else:
            print(f"Unexpected error: {e}")
        return None

# Use safe query execution
data = safe_query(
    "postgresql://user@localhost/db",
    "SELECT * FROM users LIMIT 10"
)

if data:
    print(f"Found {len(data)} users")
```

## 📊 Integration with Data Science Tools

### Pandas Integration

```python
import dbcrust
import pandas as pd
import json

def dbcrust_to_dataframe(connection_url, query):
    """Convert DBCrust results to pandas DataFrame"""
    result = dbcrust.run_command(connection_url, query)
    data = json.loads(result)
    return pd.DataFrame(data)

# Use in data analysis
df = dbcrust_to_dataframe(
    "postgresql://analyst@warehouse/sales",
    """
    SELECT 
        product_category,
        EXTRACT(month FROM order_date) as month,
        SUM(amount) as revenue
    FROM orders 
    WHERE order_date >= '2024-01-01'
    GROUP BY product_category, month
    ORDER BY month, product_category
    """
)

# Analyze with pandas
monthly_revenue = df.groupby('month')['revenue'].sum()
print(monthly_revenue)
```

### Jupyter Notebook Integration

```python
# In Jupyter notebook
import dbcrust

# Set up connection for the session
CONNECTION = "postgresql://analyst@warehouse/analytics"

def query(sql):
    """Convenience function for notebook queries"""
    return dbcrust.run_command(CONNECTION, sql)

# Use throughout notebook
query("\\dt")  # List tables
query("SELECT COUNT(*) FROM users")  # Quick counts
query("\\d users")  # Describe table structure
```

## 📚 API Reference

For more information, see:

- **[Quick Start](/dbcrust/quick-start/)** - Get started with DBCrust
- **[User Guide](/dbcrust/user-guide/basic-usage/)** - Complete feature walkthrough
- **[Installation](/dbcrust/installation/)** - Setup instructions
- **[Configuration](/dbcrust/configuration/)** - Configuration options

---

<div align="center">
    <strong>Ready to integrate DBCrust into your Python workflow?</strong><br>
    <a href="/dbcrust/quick-start/" class="md-button md-button--primary">Get Started</a>
    <a href="/dbcrust/user-guide/basic-usage/" class="md-button">User Guide</a>
</div>