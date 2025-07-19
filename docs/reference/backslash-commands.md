# Backslash Commands Reference

DBCrust provides a comprehensive set of backslash commands (meta-commands) that help you navigate and manage your database sessions efficiently. These commands are inspired by PostgreSQL's `psql` but enhanced with modern features.

## 📚 Command Categories

=== "Navigation & Info"

    | Command | Description | Example |
    |---------|-------------|---------|
    | `\l` | List databases | `\l` |
    | `\dt` | List tables | `\dt` |
    | `\d [table]` | Describe table or list all tables | `\d users` |
    | `\c <database>` | Connect to database | `\c production` |
    | `\config` | Show current configuration | `\config` |

=== "Display & Output"

    | Command | Description | Example |
    |---------|-------------|---------|
    | `\x` | Toggle expanded display | `\x` |
    | `\e` | Toggle EXPLAIN mode | `\e` |
    | `\ecopy` | Copy last EXPLAIN to clipboard | `\ecopy` |

=== "File Operations"

    | Command | Description | Example |
    |---------|-------------|---------|
    | `\w <file>` | Write last script to file | `\w query.sql` |
    | `\i <file>` | Execute SQL file | `\i setup.sql` |
    | `\ed` | Edit query in external editor | `\ed` |

=== "Named Queries"

    | Command | Description | Example |
    |---------|-------------|---------|
    | `\n` | List named queries | `\n` |
    | `\ns <name> <query>` | Save named query | `\ns users SELECT * FROM users` |
    | `\nd <name>` | Delete named query | `\nd users` |

=== "Help & Control"

    | Command | Description | Example |
    |---------|-------------|---------|
    | `\h` | Show help | `\h` |
    | `\q` | Quit DBCrust | `\q` |

## 🔍 Detailed Command Reference

### Navigation Commands

#### `\l` - List Databases

Lists all databases on the current server.

```sql
\l
```

**Output:**
```
╭──────────────┬───────────┬──────────┬───────────────╮
│ Name         │ Owner     │ Encoding │ Description   │
├──────────────┼───────────┼──────────┼───────────────┤
│ myapp_dev    │ postgres  │ UTF8     │ Development   │
│ myapp_prod   │ postgres  │ UTF8     │ Production    │
│ analytics    │ analyst   │ UTF8     │ Data warehouse│
╰──────────────┴───────────┴──────────┴───────────────╯
```

#### `\dt` - List Tables

Lists all tables in the current database.

```sql
\dt
```

**Output:**
```
╭─────────────┬──────────┬──────────┬─────────────╮
│ Schema      │ Name     │ Type     │ Owner       │
├─────────────┼──────────┼──────────┼─────────────┤
│ public      │ users    │ table    │ postgres    │
│ public      │ orders   │ table    │ postgres    │
│ public      │ products │ table    │ postgres    │
╰─────────────┴──────────┴──────────┴─────────────╯
```

#### `\d [table]` - Describe Table

Without arguments, lists all tables. With a table name, shows detailed table structure.

```sql
-- List all tables
\d

-- Describe specific table
\d users
```

**Output for `\d users`:**
```
Table "public.users"
╭─────────────┬─────────────────────┬───────────┬─────────┬─────────────╮
│ Column      │ Type                │ Nullable  │ Default │ Description │
├─────────────┼─────────────────────┼───────────┼─────────┼─────────────┤
│ id          │ integer             │ not null  │ nextval │ Primary key │
│ name        │ character varying   │ not null  │         │ Full name   │
│ email       │ character varying   │ not null  │         │ Email addr  │
│ created_at  │ timestamp           │ not null  │ now()   │ Created     │
│ status      │ character varying   │           │ active  │ User status │
╰─────────────┴─────────────────────┴───────────┴─────────┴─────────────╯

Indexes:
    "users_pkey" PRIMARY KEY, btree (id)
    "users_email_key" UNIQUE CONSTRAINT, btree (email)
    "idx_users_status" btree (status)
```

#### `\c <database>` - Connect to Database

Switches to a different database on the same server.

```sql
\c production_db
```

**Output:**
```
You are now connected to database "production_db" as user "postgres".
```

### Display Commands

#### `\x` - Toggle Expanded Display

Switches between table and expanded (vertical) display formats.

```sql
\x
```

**Before (table format):**
```
╭────┬──────────┬──────────────────────╮
│ id │ name     │ email                │
├────┼──────────┼──────────────────────┤
│ 1  │ John Doe │ john@example.com     │
╰────┴──────────┴──────────────────────╯
```

**After (expanded format):**
```
-[ RECORD 1 ]----------
id    | 1
name  | John Doe  
email | john@example.com
```

#### `\e` - Toggle EXPLAIN Mode

Enables or disables automatic EXPLAIN for all queries.

```sql
\e  -- Enable EXPLAIN mode

SELECT * FROM users WHERE email = 'john@example.com';
```

**Output with EXPLAIN enabled:**
```
○ Execution Time: 0.89 ms
○ Planning Time: 0.12 ms

Index Scan using email_idx on users
│ Index Cond: (email = 'john@example.com'::text)
│ ○ Cost: 0.29..8.31 ○ Rows: 1 ○ Width: 156
└─ Returns: id, name, email, created_at, status

╭────┬──────────┬──────────────────────╮
│ id │ name     │ email                │
├────┼──────────┼──────────────────────┤
│ 1  │ John Doe │ john@example.com     │
╰────┴──────────┴──────────────────────╯
```

**EXPLAIN modes:**
```sql
\e on          -- Basic EXPLAIN
\e analyze     -- EXPLAIN ANALYZE
\e verbose     -- EXPLAIN VERBOSE
\e buffers     -- EXPLAIN (ANALYZE, BUFFERS)
\e off         -- Disable EXPLAIN
```

#### `\ecopy` - Copy EXPLAIN to Clipboard

Copies the last EXPLAIN plan in JSON format to your clipboard.

```sql
\ecopy
```

**Output:**
```
EXPLAIN plan copied to clipboard (JSON format)
```

### File Operations

#### `\w <filename>` - Write Script to File

Saves the last executed query or script to a file.

```sql
-- Execute a query
SELECT * FROM users WHERE created_at > '2024-01-01';

-- Save it to file
\w recent_users.sql
```

**Output:**
```
Script written to 'recent_users.sql' (156 bytes)
```

#### `\i <filename>` - Execute SQL File

Loads and executes SQL commands from a file.

```sql
\i setup_tables.sql
```

**File contents (`setup_tables.sql`):**
```sql
CREATE TABLE IF NOT EXISTS users (
    id SERIAL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    email VARCHAR(255) UNIQUE NOT NULL,
    created_at TIMESTAMP DEFAULT NOW()
);

INSERT INTO users (name, email) VALUES
('Alice Johnson', 'alice@example.com'),
('Bob Smith', 'bob@example.com');
```

**Output:**
```
Executing setup_tables.sql...
CREATE TABLE
INSERT 0 2
Script execution completed successfully.
```

#### `\ed` - External Editor

Opens your default editor to write or edit a query.

```sql
\ed
```

**Process:**
1. Opens `$EDITOR` (vim, nano, code, etc.)
2. Edit your query
3. Save and close
4. Query executes automatically

**Editor integration:**
```bash
# Set preferred editor
export EDITOR="code --wait"  # VS Code
export EDITOR="vim"          # Vim
export EDITOR="nano"         # Nano
```

### Named Queries

#### `\n` - List Named Queries

Shows all saved named queries.

```sql
\n
```

**Output:**
```
Named Queries:
╭─────────────────┬────────────────────────────────────────────╮
│ Name            │ Query                                      │
├─────────────────┼────────────────────────────────────────────┤
│ active_users    │ SELECT * FROM users WHERE status = 'act.. │
│ daily_orders    │ SELECT DATE(created_at), COUNT(*) FROM .. │
│ user_summary    │ SELECT COUNT(*), MAX(created_at) FROM ..  │
╰─────────────────┴────────────────────────────────────────────╯
```

#### `\ns <name> <query>` - Save Named Query

Saves a query with a name for later use. Supports parameter substitution.

```sql
-- Simple named query
\ns active_users SELECT * FROM users WHERE status = 'active'

-- With parameters
\ns user_by_id SELECT * FROM users WHERE id = $1

-- Multiple parameters
\ns user_orders SELECT * FROM orders WHERE user_id = $1 AND status = '$2'

-- All remaining parameters
\ns search_users SELECT * FROM users WHERE name ILIKE '%$*%'
```

**Usage:**
```sql
-- Execute named queries
active_users
user_by_id 123
user_orders 123 completed
search_users John Doe
```

#### `\nd <name>` - Delete Named Query

Removes a saved named query.

```sql
\nd active_users
```

**Output:**
```
Named query 'active_users' deleted.
```

## 💡 Advanced Usage Patterns

### Combining Commands

```sql
-- Switch database and list tables
\c analytics
\dt

-- Enable EXPLAIN and run query
\e on
SELECT COUNT(*) FROM large_table;

-- Save result and write to file
\w large_table_count.sql
```

### Scripting Workflows

```sql
-- Create a setup script
\ed

-- In editor, write:
-- \c development
-- \i create_tables.sql  
-- \i seed_data.sql
-- \dt

-- Save and execute automatically
```

### Query Development

```sql
-- Start with simple query
SELECT * FROM users LIMIT 5;

-- Refine in editor
\ed

-- Save final version
\w final_user_report.sql

-- Create named query for reuse
\ns user_report SELECT u.*, COUNT(o.id) as order_count FROM users u LEFT JOIN orders o ON u.id = o.user_id GROUP BY u.id
```

## 🚀 Pro Tips

!!! tip "Command History"
    
    All backslash commands are saved in your command history and can be recalled with ↑/↓ arrows or Ctrl+R search.

!!! tip "Tab Completion"
    
    Most commands support tab completion:
    
    ```sql
    \d us[TAB]  -- Completes to table names starting with 'us'
    \ns my[TAB] -- Completes to existing named query names
    ```

!!! tip "Command Aliases"
    
    Some commands have shorter aliases:
    
    ```sql
    \q = \quit
    \? = \h = \help
    ```

!!! tip "File Paths"
    
    File commands support both absolute and relative paths:
    
    ```sql
    \i /home/user/scripts/setup.sql     -- Absolute
    \i ../migrations/001_create.sql     -- Relative
    \w ~/backups/current_query.sql      -- Home directory
    ```

!!! tip "Error Recovery"
    
    If a file operation fails, the error message will suggest corrections:
    
    ```sql
    \i nonexistent.sql
    -- Error: File 'nonexistent.sql' not found
    -- Did you mean: setup.sql, test.sql?
    ```

---

<div align="center">
    <strong>Master backslash commands?</strong><br>
    <a href="cli-commands.md" class="md-button md-button--primary">CLI Commands</a>
    <a href="../user-guide/commands.md" class="md-button">Interactive Guide</a>
</div>