# DBCrust

**The high-performance database CLI engineered for Django developers — Advanced ORM analysis, performance optimization, SSH tunneling, and seamless multi-database support (PostgreSQL, MySQL, SQLite).**

*🤖 Proudly crafted with [Claude Code](https://claude.ai/code) — where AI meets thoughtful development.*

[![Rust](https://img.shields.io/badge/rust-2024-orange.svg)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Documentation](https://img.shields.io/badge/docs-mkdocs-blue.svg)](https://clement-tourriere.github.io/dbcrust/)
[![PyPI](https://img.shields.io/pypi/v/dbcrust.svg)](https://pypi.org/project/dbcrust/)

## Why DBCrust?

DBCrust is the first database CLI built specifically for Django developers. Beyond standard database management, it provides real-time ORM analysis, N+1 query detection, and performance optimization recommendations. Built in Rust for speed, with advanced features like SSH tunneling, Vault integration, and Docker support.

## 🚀 Key Features

- **🐍 Django ORM Analyzer** - Real-time N+1 query detection, performance monitoring, and optimization recommendations
- **🔍 Smart Performance Analysis** - Built-in EXPLAIN visualization and query optimization tools
- **🐳 Multi-Database & Docker Support** - PostgreSQL, MySQL, SQLite with container auto-discovery
- **🔐 Enterprise Ready** - SSH tunneling, HashiCorp Vault integration, and encrypted connections
- **⚡ Intelligent CLI** - Context-aware autocompletion, syntax highlighting, and external editor support
- **📊 Python Library** - Complete programmatic access with unified CLI and Python APIs

## Quick Start

### Installation

```bash
# Native install (fastest, recommended)
curl -fsSL https://clement-tourriere.github.io/dbcrust/install.sh | sh  # Unix
# irm https://clement-tourriere.github.io/dbcrust/install.ps1 | iex  # Windows

# Or via uv (Python package manager)
uvx dbcrust postgres://user:pass@localhost/mydb  # Run immediately
uv tool install dbcrust  # Install as isolated tool (recommended)
```

### Basic Usage

```bash
# Database connections with intelligent autocompletion
dbcrust postgres://user:pass@localhost/mydb
dbcrust docker://postgres-container  # Container auto-discovery
dbcrust session://production_db      # Saved sessions
dbc pos[TAB] → postgres://           # Smart completions
```

## 🐍 Django & Python Integration

### Django ORM Performance Analysis

```python
# Real-time ORM analysis with middleware (fastest setup)
# settings.py
MIDDLEWARE = ['dbcrust.django.PerformanceAnalysisMiddleware', ...]

# Or manual analysis
from dbcrust.django import analyzer
with analyzer.analyze() as analysis:
    books = Book.objects.all()
    for book in books:
        print(book.author.name)  # Detects N+1 automatically

results = analysis.get_results()  # Get optimization recommendations
```

**Perfect for Django teams:** N+1 detection, performance monitoring, CI/CD integration, and real-time optimization suggestions.

[**📖 Complete Django Integration Guide →**](https://clement-tourriere.github.io/dbcrust/django-analyzer/)

## Essential Commands

```bash
# Multi-database connections
dbcrust postgres://postgres:pass@localhost/myapp
dbcrust docker://my-postgres-container
dbcrust session://production_db  # Saved sessions

# Interactive commands (once connected)
\dt                               # List tables
\d users                         # Describe table
\e                               # Toggle EXPLAIN mode
\cs                              # Column selection for wide results
\ss production_db                # Save current connection
```

## Advanced Features

```bash
# EXPLAIN visualization - toggle with \e
SELECT * FROM users WHERE email = 'user@example.com';
# ○ Execution Time: 1.23 ms • Planning Time: 0.15 ms
# Index Scan using email_idx (Cost: 4, Rows: 1)

# SSH tunneling for secure connections
dbcrust postgres://user:pass@db.internal.com/myapp --ssh-tunnel jumphost.com

# HashiCorp Vault integration
dbcrust vault://app-role@database/postgres-prod
```

## Python API

```python
import dbcrust

# Direct command execution
result = dbcrust.run_command("postgres://user:pass@localhost/mydb", "SELECT * FROM users LIMIT 10")

# Launch interactive CLI from Python
dbcrust.run_cli("postgres://user:pass@localhost/mydb")

# PostgresClient class for object-oriented usage
from dbcrust import PostgresClient
client = PostgresClient(host="localhost", user="postgres", dbname="myapp")
tables = client.list_tables()
```

[**📖 Complete Python API Documentation →**](https://clement-tourriere.github.io/dbcrust/python-api/)

## Documentation & Support

- **[📚 Complete Documentation](https://clement-tourriere.github.io/dbcrust/)** - Installation, usage guides, and API reference
- **[🔧 Command Reference](https://clement-tourriere.github.io/dbcrust/reference/backslash-commands/)** - All 40+ interactive commands
- **[🐍 Django Integration](https://clement-tourriere.github.io/dbcrust/django-analyzer/)** - ORM performance analysis
- **[🐛 Issues & Support](https://github.com/clement-tourriere/dbcrust/issues)** - Bug reports and questions
- **[📦 PyPI Package](https://pypi.org/project/dbcrust/)** - Python package information

---

**Built with ❤️ using [Rust](https://www.rust-lang.org/) • Designed for Django developers • Security-first architecture**
