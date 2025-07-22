# URL Schemes & Shell Autocompletion

DBCrust provides a comprehensive URL scheme system for connecting to databases with intelligent shell autocompletion support. This guide covers all supported connection methods and how to set up enhanced shell completion.

## 🔗 Supported URL Schemes

DBCrust supports 8 different URL schemes, each optimized for specific use cases:

### Standard Database Schemes

=== "PostgreSQL"

    **Scheme:** `postgres://`

    ```bash
    # Standard connection
    dbcrust postgres://username:password@localhost:5432/database_name
    
    # With SSL (recommended)
    dbcrust postgres://username:password@localhost:5432/database_name?sslmode=require
    
    # Alternative scheme (auto-converted to postgres://)
    dbcrust postgresql://username:password@localhost:5432/database_name
    ```

    !!! note "PostgreSQL Scheme Normalization"
        Both `postgresql://` and `postgres://` are supported and automatically normalized to `postgres://` internally.

=== "MySQL"

    **Scheme:** `mysql://`

    ```bash
    # Standard connection
    dbcrust mysql://username:password@localhost:3306/database_name
    
    # With SSL
    dbcrust mysql://username:password@localhost:3306/database_name?ssl-mode=REQUIRED
    
    # Custom port
    dbcrust mysql://root:secret@mysql-server:3307/production
    ```

=== "SQLite"

    **Scheme:** `sqlite://`

    ```bash
    # Absolute path
    dbcrust sqlite:///path/to/database.db
    
    # Relative path
    dbcrust sqlite://./myapp.db
    
    # Memory database
    dbcrust sqlite://:memory:
    
    # Current directory
    dbcrust sqlite://database.db
    ```

### Advanced Connection Schemes

=== "Docker Containers"

    **Scheme:** `docker://`

    ```bash
    # Interactive container selection
    dbcrust docker://
    # → Shows list of running database containers
    
    # Direct container connection
    dbcrust docker://postgres-container
    dbcrust docker://my-mysql-db
    
    # With credentials and database
    dbcrust docker://user:pass@container-name/database
    
    # Examples
    dbcrust docker://postgres-dev
    dbcrust docker://app_user:secret@mysql-prod/app_db
    ```

    **Features:**
    - Automatic discovery of running database containers
    - Support for PostgreSQL, MySQL, and SQLite containers
    - OrbStack integration on macOS
    - Intelligent port mapping and network resolution

=== "Saved Sessions"

    **Scheme:** `session://`

    ```bash
    # Interactive session selection
    dbcrust session://
    # → Shows list of saved sessions
    
    # Direct session connection
    dbcrust session://production_db
    dbcrust session://staging_postgres
    dbcrust session://local_dev
    ```

    **Session Management:**
    ```bash
    # Save current connection as a session
    \ss production_db
    
    # List all saved sessions
    \s
    
    # Delete a session
    \sd old_session
    
    # Connect to specific session
    \s production_db
    ```

=== "Recent Connections"

    **Scheme:** `recent://`

    ```bash
    # Interactive recent connection selection
    dbcrust recent://
    # → Shows numbered list of recent connections
    #   1. postgres://user@localhost:5432/mydb
    #   2. docker://postgres-dev/testdb  
    #   3. mysql://root@mysql-server:3306/app
    ```

    **Recent Connection Management:**
    ```bash
    # List recent connections
    \r
    
    # Clear recent connection history
    \rc
    ```

=== "HashiCorp Vault"

    **Schemes:** `vault://` or `vaultdb://`

    ```bash
    # Full vault URL
    dbcrust vault://role-name@mount-path/database-path
    
    # Interactive vault connection
    dbcrust vault://
    # → Prompts for role, mount, and database
    
    # Alternative scheme
    dbcrust vaultdb://app-role@database/postgres-prod
    ```

    **Configuration:**
    ```toml
    [vault]
    addr = "https://vault.company.com"
    token_file = "~/.vault-token"
    ```

## 🚀 Shell Autocompletion

DBCrust provides intelligent shell autocompletion that understands URL schemes and provides contextual suggestions.

### Installation

=== "Bash"

    ```bash
    # Generate completion script
    dbcrust --completions bash > ~/.local/share/bash-completion/completions/dbcrust
    
    # Or install system-wide
    sudo dbcrust --completions bash > /etc/bash_completion.d/dbcrust
    
    # Reload your shell
    source ~/.bashrc
    ```

=== "Zsh"

    ```bash
    # Create completions directory if it doesn't exist
    mkdir -p ~/.local/share/zsh/site-functions
    
    # Generate completion script
    dbcrust --completions zsh > ~/.local/share/zsh/site-functions/_dbcrust
    
    # Add to your .zshrc if not already present
    echo 'fpath=(~/.local/share/zsh/site-functions $fpath)' >> ~/.zshrc
    echo 'autoload -Uz compinit && compinit' >> ~/.zshrc
    
    # Reload your shell
    source ~/.zshrc
    ```

=== "Fish"

    ```bash
    # Generate completion script
    dbcrust --completions fish > ~/.config/fish/completions/dbcrust.fish
    
    # Reload fish completions
    fish -c "complete --erase --command dbcrust; source ~/.config/fish/completions/dbcrust.fish"
    ```

=== "PowerShell"

    ```powershell
    # Generate completion script
    dbcrust --completions powershell > $PROFILE.CurrentUserAllHosts.Replace("profile.ps1", "Completions/dbcrust.ps1")
    
    # Add to your PowerShell profile
    Add-Content $PROFILE.CurrentUserAllHosts '. $PSScriptRoot/Completions/dbcrust.ps1'
    ```

### Autocompletion Features

#### URL Scheme Completion

Type a partial scheme and press **TAB** to see available options:

```bash
dbc pos[TAB] → postgres://
dbc doc[TAB] → docker://  
dbc ses[TAB] → session://
dbc rec[TAB] → recent://
dbc va[TAB]  → vault:// vaultdb://
```

#### Contextual Completions

DBCrust provides smart contextual completions based on the URL scheme:

=== "Docker Containers"

    ```bash
    # Shows running database containers
    dbc docker://[TAB]
    # → postgres-dev mysql-test redis-cache
    
    dbc docker://post[TAB] → docker://postgres-dev
    dbc docker://my[TAB]   → docker://mysql-test
    ```

    **How it works:**
    - Queries Docker API for running containers
    - Filters for database containers (PostgreSQL, MySQL, SQLite images)
    - Only shows containers that are currently running
    - Matches container names that start with your input

=== "Saved Sessions"

    ```bash
    # Shows your saved sessions  
    dbc session://[TAB]
    # → production_db staging_postgres local_dev
    
    dbc session://prod[TAB] → session://production_db
    dbc session://loc[TAB]  → session://local_dev
    ```

    **How it works:**
    - Reads from `~/.config/dbcrust/sessions.toml`
    - Shows all saved session names
    - Matches sessions that start with your input

=== "SQLite Files"

    ```bash
    # Delegates to shell file completion
    dbc sqlite://[TAB]
    # → Uses your shell's built-in file completion
    
    dbc sqlite://./[TAB] → sqlite://./myapp.db sqlite://./test.db
    ```

#### Complete Examples

```bash
# Scheme completion
dbc [TAB]
# → postgres:// mysql:// sqlite:// docker:// session:// recent:// vault:// vaultdb://

# Docker container completion  
dbc docker://[TAB]
# → postgres-dev mysql-prod redis-cache

# Session completion
dbc session://[TAB] 
# → production staging development local

# Recent connection (interactive)
dbc recent://[ENTER]
# → 1. postgres://user@localhost:5432/mydb
#   2. docker://postgres-dev/testdb
#   3. mysql://root@server:3306/app
#   Select connection [1-3]: 
```

## 🔧 Advanced Configuration

### SSH Tunnel Patterns

Configure automatic SSH tunnels based on hostname patterns:

```toml
[ssh_tunnel_patterns]
"^db\\.internal\\..*\\.com$" = "user@jumphost.example.com:2222"
"^.*\\.prod\\.company\\.com$" = "deploy@bastion.company.com"
```

When connecting to a matching hostname, DBCrust automatically establishes an SSH tunnel:

```bash
# This automatically uses the SSH tunnel
dbcrust postgres://app@db.internal.example.com:5432/prod
# → Tunnels through user@jumphost.example.com:2222
```

### Default URL Handling

URLs without schemes default to PostgreSQL:

```bash
# These are equivalent
dbcrust localhost:5432/mydb
dbcrust postgres://localhost:5432/mydb
```

### Connection History

All successful connections are automatically saved to recent connection history:

```toml
[[recent_connections]]
connection_url = "postgres://user@localhost:5432/testdb"
display_name = "user@localhost:5432/testdb"
timestamp = "2024-01-15T14:22:33Z"
database_type = "PostgreSQL"
success = true

[[recent_connections]]
connection_url = "docker://postgres-dev/myapp"
display_name = "docker://postgres-dev/myapp"  
timestamp = "2024-01-15T14:20:15Z"
database_type = "PostgreSQL"
success = true
```

## 🎯 Best Practices

### Session Management

1. **Save frequently used connections as sessions:**
   ```bash
   # Connect to production
   dbcrust postgres://readonly@prod.db.company.com:5432/analytics
   
   # Save as session
   \ss prod_analytics
   
   # Later, reconnect easily
   dbcrust session://prod_analytics
   ```

2. **Use meaningful session names:**
   ```bash
   \ss prod_readonly      # Good: describes environment and access
   \ss staging_full       # Good: describes environment and permissions
   \ss db1                # Bad: not descriptive
   ```

### Docker Workflows

1. **Use interactive mode for exploration:**
   ```bash
   # Explore available containers
   dbcrust docker://
   ```

2. **Use direct connection for automation:**
   ```bash
   # Script-friendly (no interaction)
   dbcrust docker://postgres-prod/analytics -c "SELECT COUNT(*) FROM users"
   ```

### URL Scheme Selection

Choose the right scheme for your use case:

| Use Case | Recommended Scheme | Example |
|----------|-------------------|---------|
| Local development | `postgres://`, `mysql://`, `sqlite://` | `postgres://localhost:5432/dev` |
| Production access | `session://` or `vault://` | `session://prod_readonly` |
| Container development | `docker://` | `docker://postgres-dev` |
| Quick reconnection | `recent://` | `recent://` |
| Team sharing | `session://` with shared config | `session://shared_staging` |

## 🔍 Troubleshooting

### Autocompletion Issues

**Completions not working?**
```bash
# Check if completion script is installed
ls ~/.local/share/bash-completion/completions/dbcrust  # Bash
ls ~/.config/fish/completions/dbcrust.fish             # Fish

# Regenerate completion script
dbcrust --completions bash > ~/.local/share/bash-completion/completions/dbcrust

# Test basic completion
type _dbcrust  # Should show function definition
```

**Docker completions not showing containers?**
```bash
# Check Docker connectivity
docker ps --format '{{.Names}}' | grep -E 'postgres|mysql|mariadb|sqlite'

# Check Docker permissions
docker info  # Should not require sudo
```

### Connection Issues

**Session not found?**
```bash
# Check available sessions
\s

# Check session file
cat ~/.config/dbcrust/sessions.toml
```

**Docker connection failed?**
```bash
# Check if container is running
docker ps | grep container-name

# Check container database type
docker inspect container-name | grep -i image
```

### Performance

**Autocompletion feels slow?**

DBCrust caches autocompletion data for performance. If you notice slow completions:

1. **Docker completions** cache running containers for 30 seconds
2. **Session completions** read from disk but are very fast
3. **Scheme completions** are instant (hardcoded)

## 🧠 Interactive SQL Autocompletion

Once connected to a database, DBCrust provides advanced SQL autocompletion within the interactive session. This is separate from shell completion and works by understanding SQL syntax context.

### Context-Aware SQL Completion

DBCrust analyzes your SQL statement in real-time to provide relevant suggestions based on the SQL clause you're in:

#### SELECT Context Intelligence

```sql
-- After SELECT keyword, suggests columns, aggregates, and wildcards
SELECT [TAB]
-- Suggestions: *, COUNT(, SUM(, AVG(, MAX(, MIN(, DISTINCT

-- When FROM clause is present, also suggests actual columns
SELECT [TAB] FROM users
-- Suggestions: *, id, name, email, created_at, status, COUNT(, SUM(, ...
```

#### WHERE Clause Precision  

```sql
-- After WHERE, suggests ONLY column names (no functions or tables)
SELECT * FROM users WHERE [TAB]
-- Suggestions: id, name, email, created_at, status, active
-- NOT suggested: users, orders, *, COUNT(

-- Understands multiple tables in FROM clause  
SELECT * FROM users u JOIN orders o ON u.id = o.user_id WHERE [TAB]
-- Suggestions: columns from BOTH users and orders tables
```

#### FROM Clause Behavior

```sql
-- After FROM, suggests table names (preserves existing behavior)
SELECT * FROM [TAB]
-- Suggestions: users, orders, products, categories
-- NOT suggested: column names, functions, or wildcards
```

### Advanced SQL Context Support

#### ORDER BY and GROUP BY

```sql
-- Suggests column names from tables in FROM clause
SELECT * FROM users ORDER BY [TAB]
-- Suggestions: id, name, email, created_at, status

SELECT COUNT(*) FROM orders GROUP BY [TAB]
-- Suggestions: status, user_id, product_id, created_at
```

#### HAVING Clause

```sql
-- Suggests both aggregate functions AND column names
SELECT status, COUNT(*) FROM users GROUP BY status HAVING [TAB]
-- Suggestions: COUNT(, SUM(, AVG(, MAX(, MIN(, status
```

#### Complex Query Support

```sql
-- Handles complex multi-table scenarios
SELECT u.name, o.total 
FROM users u 
  LEFT JOIN orders o ON u.id = o.user_id 
  JOIN products p ON o.product_id = p.id 
WHERE [TAB]
-- Suggests: u.id, u.name, u.email, o.id, o.total, o.status, p.name, p.price
```

### Schema-Based Completions

#### Table Names

```sql
-- Prefix matching for table names
SELECT * FROM us[TAB]
-- Suggestions: users, user_sessions, user_preferences
```

#### Column Names with Dot Notation

```sql
-- After table.dot, suggests columns from that specific table
SELECT users.[TAB] FROM users
-- Suggestions: id, name, email, created_at, status, active

-- Works with table aliases
SELECT u.[TAB] FROM users u
-- Suggestions: id, name, email, created_at, status, active
```

#### SQL Keywords

```sql
-- Keyword expansion
SEL[TAB] → SELECT
FR[TAB] → FROM  
WH[TAB] → WHERE

-- Full statement completion
SEL[TAB] name FR[TAB] users WH[TAB] active = true
-- Expands to: SELECT name FROM users WHERE active = true
```

### Backslash Command Completion

DBCrust also provides completion for backslash commands:

#### Named Queries

```sql
-- After \n, suggests named query names
\n [TAB]
-- Suggestions: active_users, monthly_report, user_orders

-- After \nd, suggests named queries to delete
\nd [TAB] 
-- Suggestions: old_report, unused_query
```

#### Session Management

```sql
-- After \s, suggests saved session names
\s [TAB]
-- Suggestions: production, staging, development, local

-- After \sd, suggests sessions to delete
\sd [TAB]
-- Suggestions: old_staging, temp_connection
```

### Performance & Caching

- **Real-time Context Analysis**: SQL parsing happens instantly with no noticeable delay
- **Schema Caching**: Table and column metadata is cached for fast repeated access
- **Smart Invalidation**: Cache automatically refreshes when database schema changes
- **Background Loading**: Schema data loads proactively for better responsiveness

### Technical Features

- **Multi-Database Support**: Works with PostgreSQL, MySQL, and SQLite
- **Case-Insensitive Matching**: Completions work regardless of case
- **Backwards Compatible**: All existing completion behavior is preserved
- **Error Recovery**: Completion works even with partial or incomplete SQL statements

!!! tip "Completion Best Practices"
    
    - **Use TAB frequently** - completion works at any point in your SQL statement
    - **Trust the context** - suggestions are filtered to be relevant to your current clause
    - **Mix with schema exploration** - use `\dt` to see tables, then use completion for columns
    - **Leverage named queries** - save complex queries and use `\n [TAB]` to find them quickly

---

<div align="center">
    <strong>Master URL schemes and autocompletion?</strong><br>
    <a href="/dbcrust/user-guide/basic-usage/" class="md-button md-button--primary">Explore Advanced Usage</a>
    <a href="/dbcrust/reference/backslash-commands/" class="md-button">Command Reference</a>
</div>