# Quick Start

Get up and running with DBCrust in under 2 minutes! This guide will have you querying databases like a pro in no time.

## 🚀 Installation

=== "uvx (Recommended)"

    The fastest way to try DBCrust without any setup:

    ```bash
    # Run immediately without installation
    uvx dbcrust postgresql://postgres:password@localhost/mydb
    ```

    Or install globally for repeated use:

    ```bash
    uv tool install dbcrust
    dbcrust --version
    ```

=== "pip"

    ```bash
    pip install dbcrust
    dbcrust --version
    ```

=== "From Source"

    ```bash
    git clone https://github.com/clement-tourriere/dbcrust.git
    cd dbcrust
    cargo install --path .
    dbcrust --version
    ```

## 🔌 First Connection

DBCrust supports standard database URLs for all major databases:

=== "PostgreSQL"

    ```bash
    # Standard connection
    dbcrust postgresql://username:password@localhost:5432/database_name
    
    # With SSL (recommended)
    dbcrust postgresql://username:password@localhost:5432/database_name?sslmode=require
    
    # Short alias
    dbc postgresql://username:password@localhost:5432/database_name
    ```

=== "MySQL"

    ```bash
    # Standard connection
    dbcrust mysql://username:password@localhost:3306/database_name
    
    # With SSL
    dbcrust mysql://username:password@localhost:3306/database_name?ssl-mode=REQUIRED
    ```

=== "SQLite"

    ```bash
    # Local file
    dbcrust sqlite:///path/to/database.db
    
    # Relative path
    dbcrust sqlite://./myapp.db
    
    # Memory database
    dbcrust sqlite://:memory:
    ```

=== "Docker"

    ```bash
    # Interactive container selection
    dbcrust docker://
    
    # Specific container
    dbcrust docker://postgres-container
    
    # With credentials
    dbcrust docker://user:pass@container-name/database
    ```

## 🎯 Essential Commands

Once connected, you'll see the DBCrust prompt. Here are the commands you'll use most:

### Database Navigation

```sql
-- List all databases
\l

-- Switch to a different database
\c production_db

-- List tables in current database
\dt

-- Describe a specific table
\d users
```

### Query Basics

```sql
-- Simple query
SELECT * FROM users LIMIT 5;

-- With autocompletion (press TAB)
SELECT id, na[TAB] FROM us[TAB] WHERE st[TAB] = 'active';
                ↓            ↓         ↓
              name         users     status
```

### Display Options

```sql
-- Toggle expanded display for wide tables
\x

-- Toggle EXPLAIN mode to see query plans
\e

-- Now all queries show execution plans automatically
SELECT * FROM users WHERE email = 'john@example.com';
```

## 💡 Power Features in 30 Seconds

### 1. Query Visualization

Enable EXPLAIN mode to see how your queries perform:

```sql
\e  -- Enable EXPLAIN mode
SELECT u.name, COUNT(o.id) as orders
FROM users u
LEFT JOIN orders o ON u.id = o.user_id
GROUP BY u.name;
```

Output:
```
○ Execution Time: 2.34 ms • Planning Time: 0.45 ms

Hash Join
│ Combines users and orders using hash table
│ ○ Duration: 1.89 ms • Cost: 156 • Rows: 42
│   Hash Cond: (u.id = o.user_id)
├─ Seq Scan on users u
│  │ ○ Duration: 0.23 ms • Cost: 12 • Rows: 42
└─ Hash on orders o
   │ ○ Duration: 0.34 ms • Cost: 89 • Rows: 234
   └─ Seq Scan on orders o
      │ ○ Duration: 0.19 ms • Cost: 67 • Rows: 234
```

### 2. Named Queries

Save frequently used queries:

```sql
-- Save a query with parameters
\ns daily_active SELECT * FROM users WHERE last_login >= current_date - interval '$1 days';

-- Use it later
daily_active 7  -- Shows users active in last 7 days
daily_active 30 -- Shows users active in last 30 days

-- List all saved queries
\n
```

### 3. External Editor

For complex queries, use your favorite editor:

```sql
-- Opens query in $EDITOR (vim, nano, vscode, etc.)
\ed

-- Edit, save, and close - query runs automatically
```

### 4. File Operations

```sql
-- Save last query to file
\w my_query.sql

-- Load and execute a SQL file
\i scripts/monthly_report.sql
```

## 🔧 Quick Configuration

DBCrust works great out of the box, but you can customize it:

```bash
# Show current configuration
\config

# Configuration is stored in ~/.config/dbcrust/config.toml
```

Common settings:

```toml
[database]
default_limit = 1000
expanded_display_default = false

[ssh_tunnel_patterns]
"^db\\.internal\\..*\\.com$" = "jumphost.example.com"

[vault]
addr = "https://vault.company.com"
```

## 🐍 Python Integration

DBCrust isn't just a CLI - it's also a powerful Python library:

```python
import dbcrust

# Execute a query and get results
result = dbcrust.run_command(
    "postgresql://user:pass@localhost/mydb",
    "SELECT COUNT(*) FROM orders WHERE created_at >= current_date"
)
print(result)

# Launch interactive CLI from Python
dbcrust.run_cli("postgresql://user:pass@localhost/mydb")
```

## 📚 What's Next?

Now that you're up and running:

1. **[Installation Guide](installation.md)** - Detailed installation options
2. **[User Guide](user-guide/basic-usage.md)** - Complete feature walkthrough  
3. **[Python API](python-api/overview.md)** - Integration with your Python projects
4. **Advanced Features** - SSH tunnels, Vault, Docker (coming soon)

## 🆘 Common Issues

!!! question "Connection refused?"
    
    Make sure your database is running and accessible:
    ```bash
    # Test connectivity first
    telnet localhost 5432  # PostgreSQL
    telnet localhost 3306  # MySQL
    ```

!!! question "Permission denied?"
    
    Check your credentials and database permissions:
    ```sql
    -- In PostgreSQL
    \du  -- List users and roles
    
    -- In MySQL  
    SHOW GRANTS FOR 'your_username'@'localhost';
    ```

!!! question "SSL/TLS issues?"
    
    Try disabling SSL first to test basic connectivity:
    ```bash
    dbcrust postgresql://user:pass@localhost/db?sslmode=disable
    ```

---

<div align="center">
    <strong>Ready for advanced features?</strong><br>
    <a href="user-guide/basic-usage.md" class="md-button md-button--primary">Explore User Guide</a>
    <a href="reference/cli-commands.md" class="md-button">Command Reference</a>
</div>