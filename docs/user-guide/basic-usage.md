# Basic Usage

Welcome to the complete DBCrust user guide! This page covers everything you need to know to become productive with DBCrust.

!!! info "New: Django ORM Analyzer"
    DBCrust now includes a powerful Django ORM query analyzer that can detect N+1 queries and optimization opportunities. Learn more in the [Django Analyzer Guide](/dbcrust/django-analyzer/).

## 🚀 Starting DBCrust

### Command Line Interface

DBCrust follows a simple pattern: `dbcrust [OPTIONS] <CONNECTION_URL>`

```bash
# Basic connection
dbcrust postgres://user:password@localhost:5432/database

# With smart URL scheme completion
dbc pos[TAB] → postgres://
dbc docker://my[TAB] → docker://my-postgres-container
dbc session://prod[TAB] → session://production_db

# With options
dbcrust --ssh-tunnel jumphost.com postgres://user@db.internal/app

# Short alias
dbc postgres://user:password@localhost/database
```

!!! tip "Shell Autocompletion"
    Set up shell completion for smart URL scheme suggestions:
    ```bash
    dbcrust --completions bash > ~/.local/share/bash-completion/completions/dbcrust
    ```

    See [URL Schemes & Autocompletion](/dbcrust/reference/url-schemes/) for complete setup instructions.

### Interactive vs Non-Interactive Mode

=== "Interactive Mode"

    ```bash
    # Start interactive session
    dbcrust postgres://user:pass@localhost/mydb

    # You'll see the prompt
    mydb=#
    ```

=== "Non-Interactive Mode"

    ```bash
    # Execute single query
    dbcrust postgres://user:pass@localhost/mydb \
      --query "SELECT COUNT(*) FROM users"

    # Execute file
    dbcrust postgres://user:pass@localhost/mydb \
      --file report.sql
    ```

## 🎯 The DBCrust Prompt

When you enter interactive mode, you'll see a rich prompt with context:

```
postgres://localhost:5432/myapp as postgres
myapp=#
```

The prompt shows:
- **Database type** and connection details
- **Current database** name
- **User** you're connected as
- **Transaction state** (if in a transaction)

## 📝 Basic Query Execution

### Simple Queries

```sql
-- Basic SELECT
SELECT * FROM users LIMIT 5;

-- With WHERE clause
SELECT name, email FROM users WHERE created_at > '2024-01-01';

-- Aggregations
SELECT status, COUNT(*) as count
FROM orders
GROUP BY status;
```

### Multi-line Queries

DBCrust automatically detects when you're typing a multi-line query:

```sql
-- Start typing...
SELECT
  u.name,
  u.email,
  COUNT(o.id) as order_count
FROM users u
LEFT JOIN orders o ON u.id = o.user_id
GROUP BY u.id, u.name, u.email
HAVING COUNT(o.id) > 5;
-- Press Enter to execute
```

## 🧠 Smart Autocompletion

DBCrust provides intelligent, context-aware autocompletion that understands both your database schema and SQL syntax context:

### SQL Context-Aware Completion

DBCrust automatically detects what SQL clause you're in and suggests appropriate completions:

#### ✨ NEW: Cursor Context-Aware Completion

DBCrust now supports **forward-looking completion** - it can read the full line and suggest columns even when the table appears after the cursor:

```sql
-- 🎯 NEW: Forward-looking column completion works!
SELECT [TAB] FROM users
-- Suggests: id, name, email, created_at, status (reads 'users' after cursor!)

-- Works with complex queries
SELECT u.[TAB] FROM users u JOIN orders o ON u.id = o.user_id
-- Suggests: id, name, email, created_at, status (from users table)

-- Even with aliases and multiple tables
SELECT [TAB] FROM users u, orders o WHERE u.id = o.user_id
-- Suggests columns from BOTH users and orders tables
```

#### SELECT Clause Suggestions

```sql
SELECT [TAB]
-- Context-aware suggestions:
-- • If FROM clause present after cursor: column names from those tables
-- • Otherwise: *, COUNT(, SUM(, AVG(, MAX(, MIN(, DISTINCT

-- Column completion works when table is visible before cursor:
SELECT * FROM users WHERE [TAB]
-- Suggests: id, name, email, created_at, status (from users table)
```

#### WHERE Clause Intelligence

```sql
-- After WHERE, suggests only column names (no tables or functions)
SELECT * FROM users WHERE [TAB]
-- Suggests: id, name, email, created_at, status, active
-- Does NOT suggest: users, orders, COUNT(, *

-- Works with complex queries and multiple tables
SELECT * FROM users u JOIN orders o ON u.id = o.user_id WHERE [TAB]
-- Suggests columns from BOTH users and orders tables
```

#### FROM Clause Behavior

```sql
-- After FROM, suggests table names (existing behavior preserved)
SELECT * FROM [TAB]
-- Suggests: users, orders, products, categories
-- Does NOT suggest: *, COUNT(, column names
```

### Traditional Schema-Based Completion

#### Table Name Completion

```sql
SELECT * FROM us[TAB]
-- Suggests: users, user_sessions, user_preferences
```

#### Column Completion with Dot Notation

```sql
-- After table.dot, suggests columns from that specific table
SELECT users.[TAB] FROM users
-- Suggests: id, name, email, created_at, status

-- Works with aliases too
SELECT u.[TAB] FROM users u
-- Suggests: id, name, email, created_at, status
```

#### SQL Keywords

```sql
SEL[TAB] name FR[TAB] users WH[TAB] active = true
-- Expands to: SELECT name FROM users WHERE active = true
```

### Advanced Context Examples

#### 🎯 Forward-Looking Completion Examples

The breakthrough cursor context-aware completion enables these previously impossible patterns:

```sql
-- All of these NOW WORK with forward-looking completion!

-- Basic forward completion
SELECT i[TAB] FROM users
-- Suggests: id (from users table that comes after cursor)

-- Multiple tables
SELECT u[TAB] FROM users u, orders o
-- Suggests: user_id, username, updated_at (prefixed completions)

-- Complex JOINs with aliases
SELECT p[TAB] FROM users u
  JOIN orders o ON u.id = o.user_id
  JOIN products p ON o.product_id = p.id
-- Suggests: price, product_name, product_id (from products table)

-- Subqueries and CTEs
SELECT name[TAB] FROM (
  SELECT id, first_name, last_name FROM users
) u
-- Suggests: first_name, last_name (from subquery columns)
```

!!! info "How It Works"

    DBCrust uses a **highlighter bridge** to access the complete line buffer, enabling:

    - **Full SQL parsing** - reads entire statement, not just text before cursor
    - **Future table detection** - finds tables that appear after cursor position
    - **Context-aware filtering** - only suggests relevant completions for current clause
    - **Performance optimized** - no noticeable delay despite advanced parsing

#### ORDER BY and GROUP BY

```sql
-- After ORDER BY, suggests columns from FROM tables
SELECT * FROM users ORDER BY [TAB]
-- Suggests: id, name, email, created_at, status

-- Same for GROUP BY
SELECT COUNT(*) FROM users GROUP BY [TAB]
-- Suggests: status, created_at, department_id
```

#### HAVING Clause

```sql
-- After HAVING, suggests aggregate functions AND column names
SELECT status, COUNT(*) FROM users GROUP BY status HAVING [TAB]
-- Suggests: COUNT(, SUM(, AVG(, MAX(, MIN( and column names
```

#### Multiple Table Support

```sql
-- Autocompletion understands complex FROM clauses
SELECT * FROM users u, orders o, products p WHERE [TAB]
-- Suggests columns from users, orders, AND products tables

-- Works with JOINs too
SELECT * FROM users u
  LEFT JOIN orders o ON u.id = o.user_id
  JOIN products p ON o.product_id = p.id
WHERE [TAB]
-- Suggests: u.id, u.name, o.status, o.total, p.name, p.price, etc.
```

!!! tip "Context-Aware Benefits"

    The context-aware completion provides intelligent suggestions:

    - **🎯 Forward-looking completion** - reads full line to suggest columns even when table comes after cursor
    - **Smart SELECT suggestions** - aggregates, functions, * and column names from future tables
    - **Precise WHERE completions** - only relevant column names from visible tables
    - **Smart FROM clause parsing** - extracts tables from complex queries
    - **Table-qualified completions** - `table.[TAB]` suggests columns from that table

!!! success "NEW: No More Completion Limitations!"

    DBCrust now supports **cursor context-aware completion**:

    - **✅ `SELECT [TAB] FROM table`** - NOW WORKS! Suggests columns from table
    - **✅ `SELECT col[TAB] FROM users, orders`** - Suggests from all tables
    - **✅ Complex queries** - Works with JOINs, aliases, and subqueries

    This breakthrough feature uses advanced line buffer analysis to overcome terminal limitations.

!!! note "Performance"

    Context analysis is lightweight and fast:

    - **Real-time parsing** - no noticeable delay
    - **Cached schema info** - table/column data cached for speed
    - **Smart filtering** - only relevant completions shown

## 📊 Result Display Options

### Default Table Format

```
╭────┬─────────────┬──────────────────────┬────────────╮
│ id │ name        │ email                │ created_at │
├────┼─────────────┼──────────────────────┼────────────┤
│ 1  │ John Doe    │ john@example.com     │ 2024-01-15 │
│ 2  │ Jane Smith  │ jane@example.com     │ 2024-01-16 │
╰────┴─────────────┴──────────────────────┴────────────╯
```

### Expanded Display

For wide tables, toggle expanded display:

```sql
\x  -- Toggle expanded display

SELECT * FROM users WHERE id = 1;
```

Output:
```
-[ RECORD 1 ]----------
id         | 1
name       | John Doe
email      | john@example.com
created_at | 2024-01-15
status     | active
bio        | Software engineer with 10 years of experience...
```

### psql-Compatible Output

```sql
\pset border 2  -- Set border style
\pset format aligned  -- Set format
```

## 🔍 Query Analysis with EXPLAIN

Enable EXPLAIN mode to see query execution plans:

```sql
\e  -- Toggle EXPLAIN mode

-- Now all queries show execution plans
SELECT * FROM users WHERE email = 'john@example.com';
```

Output:
```
○ Execution Time: 0.89 ms
○ Planning Time: 0.12 ms

Index Scan using email_idx on users
│ Index Cond: (email = 'john@example.com'::text)
│ ○ Cost: 0.29..8.31
│ ○ Rows: 1
│ ○ Width: 156
└─ Returns: id, name, email, created_at, status, bio
```

### EXPLAIN Options

```sql
-- Enable different EXPLAIN modes
\e on           -- Basic EXPLAIN
\e analyze      -- EXPLAIN ANALYZE
\e verbose      -- EXPLAIN VERBOSE
\e buffers      -- EXPLAIN (ANALYZE, BUFFERS)

-- Disable EXPLAIN
\e off
```

!!! tip "Advanced Performance Analysis"

    EXPLAIN is just the beginning! Explore comprehensive performance tools:

    - **[Performance Analysis Guide](/dbcrust/user-guide/performance-analysis/)** - Complete query optimization
    - **[Django ORM Analyzer](/dbcrust/django-analyzer/)** - Detect N+1 queries and Django performance issues
    - **[Advanced Features](/dbcrust/user-guide/advanced-features/)** - Column selection and query profiling

## 💾 History and Sessions

### Command History

DBCrust maintains a persistent history of your commands:

```sql
-- Search history with Ctrl+R
-- Navigate with Up/Down arrows
-- History is saved between sessions
```

### Session Management

DBCrust provides two distinct features for managing connections:

#### Saved Sessions

Named sessions for frequently used connections:

```sql
-- Save current connection as a session
\ss production

-- List all saved sessions
\s

-- Connect to a saved session interactively
\s production

-- Delete a saved session
\sd old_staging
```

Command line access:
```bash
# Connect using saved session
dbcrust session://production

# Run query on saved session
dbcrust session://production -c "SELECT version()"
```

!!! tip "Advanced Session Features"

    DBCrust sessions integrate with enterprise security systems:

    - **[SSH Tunneling](/dbcrust/advanced/ssh-tunneling/)** - Automatic tunnel configuration
    - **[Vault Integration](/dbcrust/advanced/vault-integration/)** - Dynamic credential management
    - **[Docker Integration](/dbcrust/advanced/docker-integration/)** - Container-based sessions
    - **[Advanced Features Guide](/dbcrust/user-guide/advanced-features/)** - Complete session management

#### Connection History

Automatic tracking of all connections:

```sql
-- List recent connections with full URLs
\r

-- Clear connection history
\rc
```

Interactive reconnection:
```bash
# Select from recent connections interactively
dbcrust recent://
```

!!! tip "Full URL Storage"
    Connection history stores complete URLs including Docker containers:
    - `docker://user@my-postgres-container/myapp`
    - `postgres://user@host:5432/database`
    - `mysql://user@host:3306/database`

## 📁 File Operations

### Executing SQL Files

```sql
-- Execute a SQL file
\i scripts/create_tables.sql

-- Execute with relative path
\i ../migrations/001_add_users.sql
```

### Saving Queries

```sql
-- Write last query to file
\w my_query.sql

-- Write specific content
\w backup_script.sql
SELECT pg_dump('mydb');
```

### External Editor

For complex queries, use your preferred editor:

```sql
-- Open external editor (uses $EDITOR)
\ed

-- Edit, save, and close - query executes automatically
```

Editor integration works with:
- **vim/nvim** - Full syntax highlighting
- **VS Code** - `code --wait` for integration
- **nano** - Simple editing
- **emacs** - Advanced editing features

## 🏷️ Named Queries

Save frequently used queries with parameters:

```sql
-- Save a parameterized query
\ns active_users SELECT * FROM users WHERE status = '$1' AND created_at > '$2';

-- Use the named query
active_users premium '2024-01-01'
active_users trial '2024-06-01'

-- List all named queries
\n

-- Delete a named query
\nd active_users
```

### Parameter Substitution

Named queries support flexible parameter substitution:

```sql
-- Single parameter
\ns user_by_id SELECT * FROM users WHERE id = $1;

-- Multiple parameters
\ns user_orders SELECT * FROM orders WHERE user_id = $1 AND status = '$2';

-- All remaining parameters
\ns search_users SELECT * FROM users WHERE name ILIKE '%$*%';
```

## 🎨 Customization

### Display Preferences

```sql
-- Toggle various display options
\x              -- Expanded display
\pset border 1  -- Border style (0, 1, 2)
\pset null 'NULL'  -- How to display NULL values
\timing on      -- Show query execution time
```

### Configuration

View and modify settings:

```sql
-- Show current configuration
\config

-- Configuration is stored in ~/.config/dbcrust/config.toml
```

Example configuration:

```toml
[database]
default_limit = 1000
expanded_display_default = false
show_execution_time = true

[display]
null_display = "NULL"
border_style = 1
date_format = "%Y-%m-%d"

[editor]
command = "code --wait"
temp_dir = "/tmp"
```

## ⌨️ Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Ctrl+C` | Cancel current input |
| `Ctrl+D` | Exit DBCrust |
| `Ctrl+L` | Clear screen |
| `Ctrl+R` | Search command history |
| `Ctrl+A` | Move to beginning of line |
| `Ctrl+E` | Move to end of line |
| `Ctrl+U` | Delete to beginning of line |
| `Ctrl+K` | Delete to end of line |
| `Ctrl+W` | Delete previous word |
| `Tab` | Autocomplete |
| `Shift+Tab` | Previous autocomplete suggestion |
| `Up/Down` | Navigate command history |
| `Ctrl+Up/Down` | Navigate multi-line input |

## 🚪 Exiting DBCrust

```sql
-- Any of these will exit
\q
\quit
exit
-- Or press Ctrl+D
```

## 💡 Pro Tips

!!! tip "Startup Scripts"

    Create a startup script for common tasks:

    ```sql
    -- ~/.config/dbcrust/startup.sql
    \timing on
    \x auto
    SET search_path TO public, analytics;
    ```

!!! tip "Aliases"

    Create shell aliases for common connections:

    ```bash
    # In ~/.bashrc or ~/.zshrc
    alias dbp='dbcrust postgres://postgres@localhost/production'
    alias dbd='dbcrust postgres://postgres@localhost/development'
    ```

!!! tip "Quick Data Exploration"

    ```sql
    -- Quick table overview
    SELECT
      column_name,
      data_type,
      is_nullable
    FROM information_schema.columns
    WHERE table_name = 'users';

    -- Row counts for all tables
    SELECT
      schemaname,
      tablename,
      n_tup_ins as inserts,
      n_tup_upd as updates,
      n_tup_del as deletes
    FROM pg_stat_user_tables;
    ```

---

<div align="center">
    <strong>Master the basics? Let's explore advanced features!</strong><br>
    <a href="/dbcrust/reference/url-schemes/" class="md-button md-button--primary">URL Schemes & Autocompletion</a>
    <a href="/dbcrust/reference/backslash-commands/" class="md-button">Command Reference</a>
</div>
