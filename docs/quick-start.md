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

DBCrust supports 8 different URL schemes with intelligent shell autocompletion. Type a partial scheme and press **TAB** for suggestions:

```bash
dbc pos[TAB] → postgres://
dbc doc[TAB] → docker://
dbc ses[TAB] → session://
```

=== "PostgreSQL"

    ```bash
    # Standard connection (both schemes work)
    dbcrust postgres://username:password@localhost:5432/database_name
    dbcrust postgresql://username:password@localhost:5432/database_name
    
    # With SSL (recommended)
    dbcrust postgres://username:password@localhost:5432/database_name?sslmode=require
    
    # Short alias with autocompletion
    dbc pos[TAB] → postgres://
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
    
    # Smart container autocompletion (shows running containers)
    dbcrust docker://post[TAB] → docker://postgres-dev
    dbcrust docker://my[TAB]   → docker://mysql-test
    
    # With credentials
    dbcrust docker://user:pass@container-name/database
    ```

=== "Saved Sessions"

    ```bash
    # Interactive session selection
    dbcrust session://
    
    # Smart session autocompletion (shows your saved sessions)
    dbcrust session://prod[TAB] → session://production_db
    dbcrust session://dev[TAB]  → session://development
    ```

=== "Recent Connections"

    ```bash
    # Interactive recent connection selection
    dbcrust recent://
    # → 1. postgres://user@localhost:5432/mydb
    #   2. docker://postgres-dev/testdb
    #   3. mysql://root@server:3306/app
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

## 🚀 Shell Autocompletion Setup

Enable intelligent shell autocompletion for URL schemes and contextual suggestions:

=== "Bash"

    ```bash
    # Install completion script
    dbcrust --completions bash > ~/.local/share/bash-completion/completions/dbcrust
    source ~/.bashrc
    
    # Test autocompletion
    dbc pos[TAB]  # Should complete to postgres://
    ```

=== "Zsh"

    ```bash
    # Create completions directory if it doesn't exist
    mkdir -p ~/.zfunc
    
    # Install completion scripts for both binaries
    dbcrust --completions zsh > ~/.zfunc/_dbcrust
    dbc --completions zsh > ~/.zfunc/_dbc
    
    # Add these lines to your .zshrc (before oh-my-zsh if you use it):
    fpath+=~/.zfunc
    autoload -U compinit && compinit
    
    # If you use oh-my-zsh, make sure these lines come BEFORE:
    # source $ZSH/oh-my-zsh.sh
    
    # Reload your shell
    source ~/.zshrc
    ```

=== "Fish"

    ```bash
    # Install completion script
    dbcrust --completions fish > ~/.config/fish/completions/dbcrust.fish
    
    # Reload fish completions
    fish -c "complete --erase --command dbcrust; source ~/.config/fish/completions/dbcrust.fish"
    ```

!!! tip "Smart Completions"
    Once set up, autocompletion provides:
    - **URL schemes**: `dbc doc[TAB]` → `docker://`
    - **Container names**: `dbc docker://post[TAB]` → `docker://postgres-dev`
    - **Session names**: `dbc session://prod[TAB]` → `session://production_db`

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

1. **[URL Schemes & Autocompletion](/dbcrust/reference/url-schemes/)** - Master all connection methods
2. **[Installation Guide](/dbcrust/installation/)** - Detailed installation options
3. **[User Guide](/dbcrust/user-guide/basic-usage/)** - Complete feature walkthrough  
4. **[Python API](/dbcrust/python-api/overview/)** - Integration with your Python projects

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
    <a href="/dbcrust/user-guide/basic-usage/" class="md-button md-button--primary">Explore User Guide</a>
    <a href="/dbcrust/reference/backslash-commands/" class="md-button">Command Reference</a>
</div>