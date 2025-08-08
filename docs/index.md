<h1 align="center" style="font-size: 3em; font-weight: 800; margin: 0.5em 0; background: linear-gradient(to right, #7e57c2, #5e35b1); -webkit-background-clip: text; -webkit-text-fill-color: transparent; background-clip: text;">DBCrust</h1>

<p align="center">
<strong>High-performance database CLI for developers</strong><br>
<em>Multi-database support • SSH tunneling • Vault integration • Advanced ORM analysis</em>
</p>

<p align="center">
L|PyPI|https://pypi.org/project/dbcrust/|
L|GitHub|https://github.com/clement-tourriere/dbcrust|
L|MIT License|https://opensource.org/licenses/MIT|
L|Rust 2024|https://blog.rust-lang.org/2024/02/29/1.77.0.html|
</p>

---

## 🚀 Why DBCrust?

**A modern database CLI that speaks your language.** DBCrust combines the speed of Rust with intelligent features like context-aware autocompletion, SSH tunneling, Vault integration, and powerful Django ORM analysis. Whether you're debugging production issues, analyzing data, or optimizing applications, DBCrust provides an unmatched developer experience.

!!! success "One Tool, All Databases + Advanced Features"
    **PostgreSQL • MySQL • SQLite** with smart autocompletion • **SSH tunneling** for secure connections • **Vault integration** for dynamic credentials • **Django ORM analyzer** for performance optimization

## ✨ Core Features

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
    dbcrust postgres://user@db.internal.company.com/prod
    # → Automatically routes through configured jumphost
    ```

    [**📖 Complete SSH Tunneling Guide →**](/dbcrust/advanced/ssh-tunneling/)

    **HashiCorp Vault Integration**
    ```bash
    # Dynamic credentials from Vault
    dbcrust vault://app-role@database/postgres-prod
    # → Fetches credentials automatically
    ```

    [**📖 Complete Vault Integration Guide →**](/dbcrust/advanced/vault-integration/)

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

    [**📖 Complete Docker Integration Guide →**](/dbcrust/advanced/docker-integration/)

    **Session Management**
    ```bash
    # Save connections for easy reuse
    \ss production_db

    # Connect to saved sessions
    dbcrust session://production_db
    ```

=== "🐍 Django ORM Analysis"

    **Real-Time N+1 Query Detection**
    ```python
    from dbcrust.django import analyzer

    with analyzer.analyze() as analysis:
        # Your Django code
        books = Book.objects.all()
        for book in books:
            print(f"{book.title} by {book.author.name}")  # N+1 detected!

    results = analysis.get_results()
    print(results.optimization_suggestions)
    ```

    **Performance Insights**
    ```
    🚨 N+1 Query Detected (CRITICAL):
      Query: SELECT * FROM books_book
      Followed by: 25x SELECT * FROM authors_author WHERE id = ?

    💡 Fix: books = Book.objects.select_related('author').all()
    Performance: 156ms → 12ms (92% improvement)
    ```

    **Django Management Integration**
    ```bash
    # Connect using Django database settings
    python manage.py dbcrust

    # Analyze specific Django database
    python manage.py dbcrust --database analytics
    ```

    [**📖 Complete Django Integration Guide →**](/dbcrust/django-analyzer/)

## 🏃‍♂️ Quick Start

=== "Native Install (Fastest)"

    **Unix (macOS, Linux):**
    ```bash
    curl -fsSL https://clement-tourriere.github.io/dbcrust/install.sh | sh
    ```

    **Windows:**
    ```powershell
    irm https://clement-tourriere.github.io/dbcrust/install.ps1 | iex
    ```

    **Then use immediately:**
    ```bash
    dbcrust postgres://user:pass@localhost/mydb
    dbc postgres://user:pass@localhost/mydb  # Short alias
    ```

=== "Python Package Managers"

    ```bash
    # Install as a tool (recommended for CLI usage)
    uv tool install dbcrust

    # Run immediately without installation
    uvx dbcrust postgres://user:pass@localhost/mydb

    # Add to Python project
    uv add dbcrust
    pip install dbcrust  # traditional pip
    ```

=== "From Source"

    ```bash
    git clone https://github.com/clement-tourriere/dbcrust.git
    cd dbcrust
    cargo install --path .
    dbcrust --help
    ```

## 🎯 First Connection

```bash
# PostgreSQL
dbcrust postgres://user:pass@localhost/myapp

# MySQL
dbcrust mysql://root:pass@localhost:3306/myapp

# SQLite
dbcrust sqlite:///./myapp.db

# Docker containers (with auto-discovery)
dbcrust docker://my-postgres-container
dbcrust docker://   # Interactive selection

# Saved sessions
dbcrust session://production_db
```

## 🐍 Django Quick Start

For Django developers, DBCrust provides specialized ORM analysis tools:

```bash
# Install in your Django project
pip install dbcrust

# Connect using Django settings
cd your_django_project/
python manage.py dbcrust

# Test N+1 detection
python manage.py shell
```

```python
# In Django shell
from dbcrust.django import analyzer

with analyzer.analyze() as analysis:
    for post in Post.objects.all():
        print(post.author.name)  # N+1 detected!

print(analysis.get_results().summary)
```

[**📖 Complete Django Setup Guide →**](/dbcrust/django-analyzer/)

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
dbcrust postgres://backup-user@replica.db.internal/app \
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

*🤖 Proudly crafted with [Claude Code](https://claude.ai/code) — where AI meets thoughtful development.*
