<h1 align="center" style="font-size: 3em; font-weight: 800; margin: 0.5em 0; background: linear-gradient(to right, #7e57c2, #5e35b1); -webkit-background-clip: text; -webkit-text-fill-color: transparent; background-clip: text;">DBCrust</h1>

<p align="center">
<strong>The modern database CLI that speaks your language</strong><br>
<em>PostgreSQL • MySQL • SQLite with zero compromises</em>
</p>

<p align="center">
L|PyPI|https://pypi.org/project/dbcrust/|
L|GitHub|https://github.com/clement-tourriere/dbcrust|
L|MIT License|https://opensource.org/licenses/MIT|
L|Rust 2024|https://blog.rust-lang.org/2024/02/29/1.77.0.html|
</p>

---

## 🚀 Why DBCrust?

DBCrust revolutionizes database interaction by combining the **speed of Rust** with the **simplicity of modern CLIs**.
Whether you're debugging production issues, analyzing data, or automating workflows, DBCrust provides an unmatched
developer experience.

!!! success "One Tool, All Databases"
Stop juggling between `psql`, `mysql`, and `sqlite3`. DBCrust speaks all three languages fluently.

## ✨ Features That Matter

=== "🧠 Smart & Intuitive"

    **Context-Aware Autocompletion**
    ```sql
    SELECT id, na[TAB] → name, email, created_at
    FROM us[TAB] → users, user_sessions, user_preferences
    WHERE st[TAB] → status, state, start_date
    ```

    **Smart URL Scheme Completion**
    ```bash
    dbc pos[TAB] → postgres://
    dbc docker://my[TAB] → docker://my-postgres-container
    dbc session://prod[TAB] → session://production_db
    ```
    
    **Beautiful Query Visualization**
    ```
    ○ Execution Time: 1.23 ms • Planning Time: 0.15 ms
    Index Scan
    │ Optimized lookup using email_idx
    │ ○ Duration: 0.96 ms • Cost: 4 • Rows: 1
    │   on users using email_idx
    │   filter (email = 'user@example.com')
    └► id • name • email • created_at
    ```

=== "🔒 Enterprise Ready"

    **SSH Tunneling Made Simple**
    ```bash
    # Automatic tunnel detection
    dbcrust postgresql://user@db.internal.company.com/prod
    # → Automatically routes through configured jumphost
    ```
    
    **HashiCorp Vault Integration**
    ```bash
    # Dynamic credentials from Vault
    dbcrust vault://app-role@database/postgres-prod
    # → Fetches credentials automatically
    ```

=== "🐳 DevOps Friendly"

    **Docker Integration**
    ```bash
    # Interactive container selection
    dbcrust docker://
    # → 1. postgres-dev (postgres:15)
    #   2. mysql-test (mysql:8.0)
    #   3. redis-cache (redis:7)
    
    # Direct container access with autocompletion
    dbcrust docker://post[TAB] → docker://postgres-dev
    ```
    
    **OrbStack Support**
    ```bash
    # Works with OrbStack DNS
    dbcrust postgresql://user@postgres.myproject.orb.local/db
    ```

=== "🐍 Python Integration"

    **Seamless API**
    ```python
    import dbcrust
    
    # Execute queries directly
    result = dbcrust.run_command(
        "postgresql://user@localhost/db", 
        "SELECT * FROM users LIMIT 10"
    )
    
    # Launch interactive CLI
    dbcrust.run_cli("postgresql://user@localhost/db")
    ```
    
    **Rich Client Class**
    ```python
    from dbcrust import PostgresClient
    
    client = PostgresClient(host="localhost", user="postgres")
    results = client.execute("SELECT COUNT(*) FROM orders")
    tables = client.list_tables()
    ```

## 🏃‍♂️ Quick Start

=== "uvx (Recommended)"

    ```bash
    # Run immediately without installation
    uvx dbcrust postgresql://user:pass@localhost/mydb
    
    # Or install globally
    uv tool install dbcrust
    dbcrust --help
    ```

=== "PyPI"

    ```bash
    pip install dbcrust
    dbcrust postgresql://user:pass@localhost/mydb
    ```

=== "From Source"

    ```bash
    git clone https://github.com/clement-tourriere/pgcrust.git
    cd pgcrust
    cargo install --path .
    dbcrust --help
    ```

## 🎯 Real-World Examples

### Database Administration

```sql
-- List all databases
\l

-- Show table sizes
SELECT schemaname,
       tablename,
       pg_size_pretty(pg_total_relation_size(schemaname || '.' || tablename)) as size
FROM pg_tables
WHERE schemaname = 'public'
ORDER BY pg_total_relation_size(schemaname||'.'||tablename) DESC;
```

### Performance Analysis

```sql
-- Toggle EXPLAIN mode
\e

-- Now all queries show execution plans
SELECT u.email, COUNT(o.id) as order_count
FROM users u
         LEFT JOIN orders o ON u.id = o.user_id
WHERE u.created_at > '2024-01-01'
GROUP BY u.email
HAVING COUNT(o.id) > 5;
```

### DevOps Workflows

```bash
# Production database health check
dbcrust vault://readonly@prod/postgres-main \
  --query "SELECT version(), current_database(), current_user"

# Backup verification
dbcrust postgresql://backup-user@replica.db.internal/app \
  --ssh-tunnel jumphost.company.com \
  --query "SELECT MAX(created_at) FROM critical_table"
```

## 🎨 What Makes DBCrust Special?

### Modern CLI Experience

- **Syntax highlighting** for SQL and output
- **History search** with fuzzy matching
- **External editor** support for complex queries
- **Session management** with saved sessions and connection history
- **Smart URL schemes** with `docker://`, `session://`, `recent://`, `vault://`
- **Shell autocompletion** for connection URLs and contextual suggestions

### Intelligent Features

- **Named queries** with parameter substitution
- **Automatic paging** for large result sets
- **Copy-paste friendly** output formats
- **Error highlighting** with helpful suggestions

### Built for Speed

- **Rust performance** - start in milliseconds
- **Efficient rendering** - handle millions of rows
- **Smart caching** - autocompletion data persists
- **Minimal memory** footprint

## 🛡️ Security First

- ✅ **TLS/SSL encryption** by default
- ✅ **SSH key authentication** support
- ✅ **Password-free workflows** via Vault
- ✅ **No plaintext storage** of credentials
- ✅ **Audit logging** for compliance

## 🌟 Community & Support

<div class="grid cards" markdown>

- :material-book-open-page-variant:{ .lg .middle } **Documentation**

  ---

  Comprehensive guides and API reference

  [:octicons-arrow-right-24: Explore docs](/dbcrust/quick-start/)

- :material-github:{ .lg .middle } **Source Code**

  ---

  Open source on GitHub with MIT license

  [:octicons-arrow-right-24: View source](https://github.com/clement-tourriere/dbcrust)

- :material-package-variant:{ .lg .middle } **PyPI Package**

  ---

  Install via pip or uv package manager

  [:octicons-arrow-right-24: Install now](https://pypi.org/project/dbcrust/)

- :material-chat-question:{ .lg .middle } **Support**

  ---

  Get help via GitHub issues

  [:octicons-arrow-right-24: Get support](https://github.com/clement-tourriere/dbcrust/issues)

</div>

---

<div align="center">
    <strong>Ready to supercharge your database workflow?</strong><br>
    <a href="/dbcrust/quick-start/" class="md-button md-button--primary">Get Started in 2 Minutes</a>
    <a href="/dbcrust/user-guide/basic-usage/" class="md-button">Learn More</a>
</div>

*Built with ❤️ using [Rust](https://www.rust-lang.org/), [SQLx](https://github.com/launchbadge/sqlx),
and [reedline](https://github.com/nushell/reedline)*