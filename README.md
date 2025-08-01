# DBCrust

**The modern database CLI and Python library that speaks your language — PostgreSQL, MySQL, SQLite with zero compromises. Built for developers, data analysts, and Django teams.**

[![Rust](https://img.shields.io/badge/rust-2024-orange.svg)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Documentation](https://img.shields.io/badge/docs-mkdocs-blue.svg)](https://clement-tourriere.github.io/dbcrust/)
[![PyPI](https://img.shields.io/pypi/v/dbcrust.svg)](https://pypi.org/project/dbcrust/)

## Why DBCrust?

DBCrust combines a high-performance database CLI with a powerful Python library for database management and Django ORM optimization. Built in Rust for maximum performance, it provides an intuitive interface for PostgreSQL, MySQL, and SQLite with advanced features that boost developer productivity and catch performance issues early.

## 🚀 Key Features

- **Multi-Database Support** - PostgreSQL, MySQL, SQLite in one tool
- **Smart Autocompletion** - Context-aware suggestions for tables, columns, and SQL keywords
- **🐍 Python Library & Django ORM Analyzer** - Rich Python API with N+1 query detection and Django optimization tools
- **📊 Query Visualization** - Beautiful EXPLAIN output with execution plans
- **🔐 Enterprise Security** - SSH tunneling, HashiCorp Vault integration, and encrypted connections
- **🐳 Docker Integration** - Connect to databases in Docker containers with automatic port detection and OrbStack DNS support
- **⚡ Performance Analysis** - Built-in tools to catch database performance issues in development
- **🎯 Developer Experience** - History, syntax highlighting, external editor support, and automated testing integration

## Quick Start

### Unix Installation (macOS, Linux)

```bash
# Install DBCrust with a single command
curl -fsSL https://clement-tourriere.github.io/dbcrust/install.sh | sh

# Then use it immediately
dbcrust postgres://user:pass@localhost/mydb
dbc postgres://user:pass@localhost/mydb  # Short alias
```

### Windows Installation

```powershell
# Install DBCrust with PowerShell
irm https://clement-tourriere.github.io/dbcrust/install.ps1 | iex

# Then use it immediately
dbcrust postgres://user:pass@localhost/mydb
dbc postgres://user:pass@localhost/mydb  # Short alias
```

### Python/PyPI Installation (All Platforms)

```bash
# Quick run with uv (no installation needed)
uvx dbcrust postgres://user:pass@localhost/mydb

# Or install globally via PyPI
uv tool install dbcrust
dbcrust postgres://user:pass@localhost/mydb
```

### Basic Usage

```bash
# Multi-database support
dbcrust postgres://user:pass@localhost/mydb
dbcrust mysql://user:pass@localhost/mydb
dbcrust sqlite:///path/to/database.db

# Docker container databases
dbcrust docker://postgres-container
dbcrust docker://   # Interactive container selection
```

## 🐍 Python & Django Integration

DBCrust isn't just a CLI – it's a powerful Python library designed for developers working with databases and Django applications.

### Quick Python Examples

```python
import dbcrust

# Execute SQL queries programmatically
result = dbcrust.run_command("postgres://user:pass@localhost/db", "SELECT * FROM users LIMIT 10")

# Django ORM performance analysis
from dbcrust.django import analyzer
with analyzer.analyze() as analysis:
    books = Book.objects.all()
    for book in books:
        print(book.author.name)  # Detects N+1 queries automatically

# Get results
results = analysis.get_results()
print(results.summary)
```

### Perfect for Django Teams

- **🔍 N+1 Query Detection** - Automatically catch expensive query patterns
- **📈 Performance Monitoring** - Integrate into CI/CD for performance regression testing  
- **🛠️ Development Debugging** - Real-time analysis during development
- **📋 Code Review Automation** - Generate performance reports for pull requests
- **🧪 Testing Integration** - Validate query performance in test suites

[**📖 Complete Python & Django Documentation →**](https://clement-tourriere.github.io/dbcrust/django-analyzer/)

## Installation

### Prerequisites

- Rust 2024 edition or later (for building from source)
- [uv](https://github.com/astral-sh/uv) (recommended for Python installation)

### Quick Install with uv (Recommended)

```bash
# Install globally as a tool
uv tool install dbcrust

# Or run directly without installation
uvx dbcrust postgres://user:pass@localhost/mydb
```

### Install from PyPI

```bash
# Using uv
uv pip install dbcrust

# Using pip (if you prefer)
pip install dbcrust
```

### Install from Source

```bash
git clone https://github.com/clement-tourriere/dbcrust.git
cd dbcrust
cargo install --path .
```

## Usage Examples

### Basic Connection

```bash
# PostgreSQL
dbcrust postgres://postgres:pass@localhost/myapp

# MySQL  
dbcrust mysql://root:pass@localhost:3306/myapp

# SQLite
dbcrust sqlite:///./myapp.db

# Docker containers
dbcrust docker://my-postgres-container
dbcrust docker://user:pass@container-name/database
```

### Interactive Commands

```sql
-- List databases
\l

-- List tables
\
dt

-- Describe table structure
\d users

-- Switch database
\c analytics

-- List Docker containers
\docker

-- Query with autocompletion
SELECT id, name, email
FROM users
WHERE active = true;
```

### Query Visualization

Enable EXPLAIN mode to see execution plans:

```
\e
SELECT * FROM users WHERE email = 'user@example.com';
```

Output:

```
○ Execution Time: 1.23 ms
○ Planning Time: 0.15 ms
Index Scan
│ Finds relevant records based on an Index. Index Scans perform 2 read operations: one to read the index and another to read the actual value from the table.
│ ○ Duration: 0.96 ms
│ ○ Cost: 4
│ ○ Rows: 1
│   on users
│   using email_idx
│   filter (email = 'user@example.com')
├► id + name + email + created_at
```

### SSH Tunneling

```bash
# Connect through SSH tunnel
dbcrust postgres://user:pass@db.internal.com/myapp \
  --ssh-tunnel jumphost.example.com

# With SSH credentials
dbcrust postgres://user:pass@db.internal.com/myapp \
  --ssh-tunnel user:pass@jumphost.example.com:2222
```

### Vault Integration

```bash
# Connect using HashiCorp Vault
dbcrust vault://app-role@database/postgres-prod

# Interactive vault connection
dbcrust vault:///
```

## 🐍 Complete Python API Reference

DBCrust provides comprehensive Python integration with multiple approaches for different use cases. Whether you're building automation scripts, analyzing Django applications, or integrating database operations into your Python workflow, DBCrust has you covered.

### 1. Direct Command Execution

```python
import dbcrust

# Execute SQL queries
result = dbcrust.run_command("postgres://user:pass@localhost/mydb", "SELECT * FROM users LIMIT 10")
print(result)

# Execute backslash commands
tables = dbcrust.run_command("postgres://user:pass@localhost/mydb", "\\dt")
databases = dbcrust.run_command("postgres://user:pass@localhost/mydb", "\\l")

# Multi-database support
mysql_result = dbcrust.run_command("mysql://user:pass@localhost/mydb", "SHOW TABLES")
sqlite_result = dbcrust.run_command("sqlite:///path/to/database.db", "SELECT * FROM users")
```

### 2. Programmatic Execution with CLI Arguments

```python
import dbcrust

# Execute with additional CLI options - perfect for automation
result = dbcrust.run_with_url(
    "postgres://user:pass@localhost/mydb", 
    ["--debug", "-c", "\\dt"]
)

# Use saved sessions without sys.argv conflicts
dbcrust.run_with_url("session://production", ["-o", "json", "-c", "SELECT version()"])

# Clean programmatic calls for integration
dbcrust.run_with_url("docker://postgres-container/mydb")
```

### 3. Interactive CLI from Python

```python
import dbcrust

# Launch interactive CLI
dbcrust.run_cli("postgres://user:pass@localhost/mydb")

# Or without specifying URL (will prompt for connection)
dbcrust.run_cli()
```

### 4. PostgresClient Class

```python
from dbcrust import PostgresClient

# Connect to database
client = PostgresClient(
    host="localhost",
    port=5432,
    user="postgres",
    password="secret",
    dbname="myapp"
)

# Execute queries
results = client.execute("SELECT * FROM users LIMIT 10")
print(results)

# List operations
databases = client.list_databases()
tables = client.list_tables()

# Use the new run_command method
result = client.run_command("SELECT COUNT(*) FROM users")
```

### 5. Django ORM Performance Analysis

DBCrust includes a powerful Django ORM analyzer that detects performance issues:

```python
from dbcrust.django import analyzer

# Analyze Django ORM queries for performance issues
with analyzer.analyze() as analysis:
    # Your Django code here
    books = Book.objects.all()
    for book in books:
        print(book.author.name)  # Will detect N+1 query

# Get results
results = analysis.get_results()
print(results.summary)
```

```bash
# Django management command integration
python manage.py dbcrust                    # Connect to default database
python manage.py dbcrust --database analytics  # Connect to specific database
```

**Features:**
- **N+1 Query Detection** - Automatically identifies repeated query patterns
- **Missing Optimizations** - Detects missing `select_related()` and `prefetch_related()`
- **Transaction Safety** - Optional rollback mode for safe analysis
- **EXPLAIN Integration** - Combines with DBCrust for database-level insights
- **Django Management Command** - `python manage.py dbcrust` works like `dbshell` but with DBCrust features

**Perfect for:**
- Development debugging and optimization
- Performance testing in CI/CD pipelines
- Production monitoring and analysis
- Code review automation

[**📖 Complete Django Analyzer Documentation →**](https://clement-tourriere.github.io/dbcrust/django-analyzer/)

## Commands

DBCrust provides 40+ interactive commands for database management, query optimization, and developer workflows.

**Quick examples:**
- `\l` - List databases
- `\dt` - List tables  
- `\d users` - Describe table structure
- `\e` - Toggle EXPLAIN mode for query analysis
- `\docker` - List Docker database containers

[**📖 Complete Command Reference →**](https://clement-tourriere.github.io/dbcrust/reference/backslash-commands/)

## Advanced Features

<details>
<summary>SSH Tunneling</summary>

Configure automatic SSH tunnels in your config file:

```toml
[ssh_tunnel_patterns]
"^db\\.internal\\..*\\.com$" = "jumphost.example.com"
".*\\.private\\.net" = "user@jumphost.example.com:2222"
```

</details>

<details>
<summary>HashiCorp Vault</summary>

Set up Vault integration:

```bash
export VAULT_ADDR="https://vault.example.com"
export VAULT_TOKEN="your-token"

dbcrust vault://my-role@database/postgres-prod
```

</details>

<details>
<summary>Configuration</summary>

DBCrust stores configuration in `~/.config/dbcrust/config.toml`:

```toml
[database]
default_limit = 1000
expanded_display_default = false

[ssh_tunnel_patterns]
"^db\\.internal\\..*\\.com$" = "jumphost.example.com"
```

</details>

<details>
<summary>Docker Integration</summary>

DBCrust can connect to databases running in Docker containers:

```bash
# Connect to a specific container
dbcrust docker://postgres-container

# Interactive container selection
dbcrust docker://

# With credentials and database
dbcrust docker://user:pass@container-name/dbname
```

Features:

- Automatic port detection for exposed containers
- OrbStack DNS support for containers without exposed ports
- Support for custom OrbStack domains via `dev.orbstack.domains` label
- Automatic DNS for Docker Compose projects: `service.project.orb.local`

</details>

## Contributing

We welcome contributions! Please see our [Contributing Guide](CONTRIBUTING.md) for details.

### Development Setup

```bash
git clone https://github.com/clement-tourriere/dbcrust.git
cd dbcrust
cargo build
cargo test
```

### Running Tests

```bash
cargo test -- --nocapture
```

## Security

- All connections support SSL/TLS encryption
- Passwords are never stored in plain text
- SSH key authentication supported
- HashiCorp Vault integration for dynamic credentials
- Audit logging for enterprise environments

## Performance

- Written in Rust for maximum performance
- Efficient connection pooling
- Minimal memory footprint
- Fast query execution and result rendering

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Support

- **[📚 Documentation](https://clement-tourriere.github.io/dbcrust/)** - Comprehensive guides and API reference
- **[🐛 Issues](https://github.com/clement-tourriere/dbcrust/issues)** - Bug reports and feature requests
- **[📦 PyPI Package](https://pypi.org/project/dbcrust/)** - Install via pip or uv
- **[⭐ GitHub Repository](https://github.com/clement-tourriere/dbcrust)** - Source code and contributions

---

Built with ❤️ using [Rust](https://www.rust-lang.org/), [SQLx](https://github.com/launchbadge/sqlx),
and [reedline](https://github.com/nushell/reedline).