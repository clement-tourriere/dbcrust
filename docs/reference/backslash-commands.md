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
    | `\cs` | Toggle column selection mode | `\cs` |
    | `\csthreshold <n>` | Set column selection threshold | `\csthreshold 15` |
    | `\clrcs` | Clear saved column selections | `\clrcs` |
    | `\resetview` | Reset all view settings | `\resetview` |
    | `\serverinfo` | Toggle server info display | `\serverinfo` |

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
    | `\ns <name> <query> [--scope]` | Save named query with scope | `\ns users SELECT * FROM users --global` |
    | `\nd <name>` | Delete named query | `\nd users` |

=== "Sessions & History"

    | Command | Description | Example |
    |---------|-------------|---------|
    | `\s [name]` | List saved sessions or connect | `\s` or `\s prod` |
    | `\ss <name>` | Save current connection as session | `\ss production` |
    | `\sd <name>` | Delete saved session | `\sd oldprod` |
    | `\r` | List recent connections | `\r` |
    | `\rc` | Clear recent connections | `\rc` |

=== "Vault Management"

    | Command | Description | Example |
    |---------|-------------|---------|
    | `\vc` | Show vault credential cache status | `\vc` |
    | `\vcc` | Clear all cached vault credentials | `\vcc` |
    | `\vcr [role]` | Force refresh vault credentials | `\vcr` or `\vcr my-role` |
    | `\vce` | Show expired vault credentials | `\vce` |

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

#### `\cs` - Toggle Column Selection Mode

Enables or disables interactive column selection for all queries. When enabled, all queries will prompt for column selection regardless of the number of columns.

```sql
\cs  -- Toggle column selection mode on/off
```

**Output:**
```
Column selection is now enabled.
```

!!! info "Auto-Trigger vs Manual Mode"
    - **Auto-Trigger**: Column selection appears automatically when queries return more than the configured threshold (default: 10 columns)
    - **Manual Mode** (`\cs` enabled): Column selection appears for ALL queries, regardless of column count

#### `\csthreshold <number>` - Set Column Selection Threshold

Configures the number of columns that triggers automatic column selection. This setting is saved to your configuration file.

```sql
-- Set threshold to 15 columns
\csthreshold 15

-- Set threshold to 5 columns for detailed work
\csthreshold 5
```

**Output:**
```
Column selection threshold set to: 15
```

**Default threshold:** 10 columns

#### Interactive Column Selection Interface

When column selection is triggered (either automatically or via `\cs` mode), an interactive interface appears:

**Features:**
- **Visual Selection**: Checkbox-style interface with arrow key navigation
- **Multi-Select**: Use spacebar to select/deselect multiple columns
- **Keyboard Controls**:
  - ↑/↓ Arrow keys: Navigate between columns
  - Space: Toggle column selection
  - Enter: Confirm selection and show results
  - **Ctrl+C: Abort query and return to prompt** (doesn't exit DBCrust)

**Example Usage:**
```sql
-- This query has 11 columns, exceeds default threshold of 10
SELECT * FROM users_detailed;
```

**Interactive Interface:**
```
? Select columns to display:
❯ ◯ id
  ◯ username
  ◯ email
  ◯ first_name
  ◯ last_name
  ◯ created_at
  ◯ updated_at
  ◯ last_login
  ◯ is_active
  ◯ phone
  ◯ address
[↑↓ to move, space to select, enter to confirm, ctrl+c to abort]
```

After selection (e.g., selecting id, username, email):
```
Showing 3 of 11 columns
╭────┬──────────┬──────────────────────╮
│ id │ username │ email                │
├────┼──────────┼──────────────────────┤
│ 1  │ john_doe │ john@example.com     │
│ 2  │ jane_doe │ jane@example.com     │
╰────┴──────────┴──────────────────────╯
```

#### Session Persistence

Column selections are automatically remembered during your session:

**Behavior:**
- Selections saved per table structure (based on column names)
- Subsequent queries on same table use saved selection automatically
- Persists until you clear selections or reset views

**Example:**
```sql
-- First time: interactive selection appears
SELECT * FROM users_detailed;
-- [Select id, username, email]

-- Second time: uses saved selection automatically
SELECT * FROM users_detailed WHERE created_at > '2024-01-01';
-- Shows only id, username, email columns
```

#### `\clrcs` - Clear Column Selections

Removes all saved column selections, returning to fresh selection state for all tables.

```sql
\clrcs
```

**Output:**
```
Column views cleared.
```

After clearing, the next query on any table will prompt for column selection again.

#### `\resetview` - Reset All View Settings

Resets all display settings to defaults, including:
- Column selections (clears all saved selections)
- Expanded display mode (`\x`)
- EXPLAIN mode (`\e`)

```sql
\resetview
```

**Output:**
```
View settings reset to defaults.
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
4. Script is loaded and ready - press Enter to execute

**Editor integration:**
```bash
# Set preferred editor
export EDITOR="code --wait"  # VS Code
export EDITOR="vim"          # Vim
export EDITOR="nano"         # Nano
```

**Workflow tip:** After using `\ed` or `\i`, press Enter on an empty line to re-execute the last loaded script.

### Named Queries

DBCrust provides a powerful scoped named query system that allows you to organize queries by visibility scope: **global**, **database-type specific**, or **session-local**.

#### Query Scopes

**Global Scope** - Available across all database connections and sessions
**Database-Type Scope** - Available only for specific database types (PostgreSQL, MySQL, SQLite)
**Session-Local Scope** - Available only for the current database session (host+port+database+user)

#### `\n` - List Named Queries

Shows all named queries available in the current context, with scope indicators.

```sql
\n
```

**Output:**
```
Named queries:
  active_users     [global]     - SELECT * FROM users WHERE status = 'active'
  pg_stats         [postgres]   - SELECT * FROM pg_stat_activity
  daily_summary    [session]    - SELECT DATE(created_at), COUNT(*) FROM orders
  user_report      [global]     - SELECT u.*, COUNT(o.id) FROM users u LEFT JOIN orders o ON u.id = o.user_id
```

**Scope Priority:** Session-local queries take precedence over database-type queries, which take precedence over global queries when names conflict.

#### `\ns <name> <query> [--scope]` - Save Named Query with Scope

Saves a query with a name and optional scope specification. Supports parameter substitution.

**Scope Options:**
- `--global` - Available for all database connections
- `--postgres` - Available only for PostgreSQL connections
- `--mysql` - Available only for MySQL connections
- `--sqlite` - Available only for SQLite connections
- No flag (default) - Session-local scope (current database session only)

**Basic Examples:**
```sql
-- Session-local query (default)
\ns active_users SELECT * FROM users WHERE status = 'active'

-- Global query (all databases)
\ns count_all SELECT COUNT(*) FROM $1 --global

-- PostgreSQL-specific query
\ns pg_activity SELECT * FROM pg_stat_activity --postgres

-- MySQL-specific query
\ns mysql_status SHOW GLOBAL STATUS LIKE 'Connections' --mysql

-- SQLite-specific query
\ns sqlite_tables SELECT name FROM sqlite_master WHERE type='table' --sqlite
```

**Parameter Substitution:**
```sql
-- Single parameter
\ns user_by_id SELECT * FROM users WHERE id = $1 --global

-- Multiple parameters
\ns user_orders SELECT * FROM orders WHERE user_id = $1 AND status = '$2'

-- All remaining parameters (space-separated)
\ns search_users SELECT * FROM users WHERE name ILIKE '%$*%' --global

-- All remaining parameters (single string)
\ns full_search SELECT * FROM users WHERE CONCAT(first_name, ' ', last_name) ILIKE '%$@%'
```

**Advanced Scope Examples:**
```sql
-- Database-type specific reporting queries
\ns pg_table_sizes SELECT schemaname, tablename, pg_size_pretty(pg_total_relation_size(schemaname||'.'||tablename)) as size FROM pg_tables --postgres

\ns mysql_table_info SELECT table_name, table_rows, data_length FROM information_schema.tables WHERE table_schema = DATABASE() --mysql

-- Global utility queries
\ns today_records SELECT * FROM $1 WHERE DATE(created_at) = CURRENT_DATE --global

-- Session-specific queries (no flag needed)
\ns my_analysis SELECT customer_id, SUM(amount) FROM local_sales_data GROUP BY customer_id
```

**Query Execution:**
```sql
-- Execute named queries with parameters
active_users
user_by_id 123
user_orders 123 completed
search_users John Doe
pg_table_sizes
```

**Save Confirmation:**
```
Named query 'active_users' saved successfully (scope: session-local).
Named query 'pg_activity' saved successfully (scope: postgres).
Named query 'count_all' saved successfully (scope: global).
```

#### `\nd <name>` - Delete Named Query

Removes a named query from the current context. Automatically detects the scope of the query to delete.

```sql
\nd active_users
```

**Output:**
```
Named query 'active_users' deleted successfully (scope: session-local).
```

**Scope Resolution:** When deleting, DBCrust follows the same priority order as execution - it will delete the session-local query first, then database-type, then global if multiple queries exist with the same name.

#### Practical Usage Patterns

**Development Workflow:**
```sql
-- Create session-specific analysis queries during development
\ns debug_orders SELECT * FROM orders WHERE created_at > '2024-01-01' AND status = 'pending'

-- Create global utilities for reuse across projects
\ns table_info SELECT table_name, table_rows FROM information_schema.tables WHERE table_schema = '$1' --global

-- Create database-specific maintenance queries
\ns pg_vacuum_analyze VACUUM ANALYZE $1 --postgres
```

**Team Collaboration:**
```sql
-- Global queries shared across team
\ns daily_metrics SELECT DATE(created_at), COUNT(*), AVG(amount) FROM orders WHERE created_at >= CURRENT_DATE - INTERVAL '7 days' GROUP BY DATE(created_at) --global

-- Database-specific performance queries
\ns pg_slow_queries SELECT query, calls, total_time, mean_time FROM pg_stat_statements ORDER BY mean_time DESC LIMIT 10 --postgres
```

**Multi-Database Projects:**
```sql
-- PostgreSQL analytics
\ns user_engagement SELECT user_id, COUNT(*) as actions FROM user_events WHERE created_at > $1 GROUP BY user_id --postgres

-- MySQL equivalent
\ns user_engagement SELECT user_id, COUNT(*) as actions FROM user_events WHERE created_at > '$1' GROUP BY user_id --mysql

-- Global fallback
\ns simple_count SELECT COUNT(*) FROM $1 --global
```

#### Autocomplete Support

The named query system provides intelligent autocomplete:

**Query Name Completion:**
```sql
\n act[TAB]          -- Shows: active_users
\ns my_qu[TAB]       -- Shows existing query names for overwriting
\nd debug[TAB]       -- Shows: debug_orders
```

**Scope Flag Completion:**
```sql
\ns myquery SELECT 1 --glo[TAB]    -- Shows: --global
\ns test SELECT 1 --post[TAB]      -- Shows: --postgres
```

**SQL Completion:**
```sql
\ns myquery SELE[TAB]              -- Shows: SELECT, SELECT *, etc.
\ns myquery SELECT * FROM use[TAB]  -- Shows: users table
```

#### Storage and Migration

Named queries are stored separately by scope:
- **Global**: Available across all sessions and database types
- **Database-type**: Available for all sessions of that database type
- **Session-local**: Available only for the specific database session

**Storage Location:** `~/.config/dbcrust/named_queries.toml`

**Migration:** Existing named queries from older versions are automatically migrated to the new scoped system as global queries during the first run.

### Session Management

#### `\s [name]` - List or Connect to Sessions

Without arguments, lists all saved sessions. With a session name, connects to that session.

```sql
-- List all saved sessions
\s
```

**Output:**
```
Saved Sessions:
  production - PostgreSQL postgres@prod.db.com:5432/myapp
  staging - PostgreSQL postgres@staging.db.com:5432/myapp_staging
  local_mysql - MySQL root@localhost:3306/testdb
  analytics - SQLite /data/analytics.db

Use 'session://<name>' to connect via command line
```

```sql
-- Connect to a saved session
\s production
```

**Output:**
```
Connecting to saved session 'production'...
✓ Successfully connected to database
```

#### `\ss <name>` - Save Session

Saves the current connection as a named session for quick reconnection.

```sql
\ss production
```

**Output:**
```
Session 'production' saved successfully
```

!!! info "Password Security"
    Sessions never store passwords. DBCrust integrates with:
    - PostgreSQL: `.pgpass` file
    - MySQL: `.my.cnf` file
    - SQLite: No authentication needed

#### `\sd <name>` - Delete Session

Removes a saved session.

```sql
\sd old_staging
```

**Output:**
```
Deleted session 'old_staging'
```

### Connection History

#### `\r` - List Recent Connections

Shows your recent connection history with full URLs (excluding passwords).

```sql
\r
```

**Output:**
```
Recent Connections:
  [1] ✓ docker://postgres@myapp-postgres/myapp_dev - 2024-01-15 14:22 (PostgreSQL)
  [2] ✓ postgres://user@localhost:5432/testdb - 2024-01-15 14:15 (PostgreSQL)
  [3] ✗ mysql://root@badhost:3306/db - 2024-01-15 14:10 (MySQL)
  [4] ✓ sqlite:///home/user/data.db - 2024-01-15 13:55 (SQLite)

Use 'recent://' to interactively select and connect to a recent connection
```

!!! tip "Connection Status"
    - ✓ = Successful connection
    - ✗ = Failed connection attempt

#### `\rc` - Clear Recent Connections

Clears all connection history.

```sql
\rc
```

**Output:**
```
Cleared all recent connections
Configuration saved
```

### Vault Management

DBCrust provides intelligent caching for HashiCorp Vault dynamic credentials to improve performance and reduce Vault API calls.

#### `\vc` - Show Vault Credential Cache Status

Displays all cached Vault credentials with their expiration status and remaining TTL.

```sql
\vc
```

**Output:**
```
Vault credential cache status (showing 2 entries):
  database/myapp-prod/readonly (v-user-prod--ABC123-1234567890) - 0h58m remaining - VALID
  database/myapp-dev/admin (v-user-dev--XYZ789-9876543210) - 0h02m remaining - EXPIRES SOON
```

**Status indicators:**
- **VALID**: Credentials have sufficient TTL remaining
- **EXPIRES SOON**: Credentials below renewal threshold (default: 25% of original TTL)
- **EXPIRED**: Credentials past expiration time (automatically cleaned up)

#### `\vcc` - Clear Vault Credential Cache

Removes all cached vault credentials, forcing fresh authentication on next vault:// connection.

```sql
\vcc
```

**Output:**
```
Cleared all cached vault credentials (2 entries removed)
```

!!! warning "Cache Clearing"
    This forces all subsequent Vault connections to fetch fresh credentials from Vault, which may impact performance and increase Vault API usage.

#### `\vcr [role]` - Force Refresh Vault Credentials

Forces refresh of Vault credentials, either for all cached entries or a specific role.

```sql
-- Refresh all cached credentials
\vcr

-- Refresh specific role
\vcr readonly
```

**Output:**
```
Refreshed vault credentials for role 'readonly'
New credentials valid for 1h0m
```

**Use cases:**
- Force credential renewal before long-running operations
- Refresh credentials that are near expiration
- Update credentials after Vault policy changes

#### `\vce` - Show Expired Vault Credentials

Lists vault credentials that have expired but haven't been cleaned up yet.

```sql
\vce
```

**Output:**
```
Expired vault credentials (1 entry):
  database/myapp-staging/readonly - expired 0h15m ago
```

!!! info "Automatic Cleanup"
    Expired credentials are automatically removed during normal cache operations. This command is mainly useful for troubleshooting.

#### Vault Credential Caching Behavior

**Automatic Caching:**
- Credentials are automatically cached when using `vault://` URLs
- Cache persists between DBCrust sessions
- Stored in encrypted file: `~/.config/dbcrust/vault_credentials.enc`

**Cache Validation:**
- Credentials are checked for expiration before use
- TTL threshold prevents using credentials that expire soon (default: 5 minutes minimum)
- Automatic cleanup removes expired credentials

**Security Features:**
- All cached credentials are encrypted using AES-256-GCM
- Encryption key derived from your Vault token
- Cache automatically invalidated if Vault token changes

**Configuration:**
```toml
# ~/.config/dbcrust/config.toml
vault_credential_cache_enabled = true          # Enable/disable caching
vault_cache_renewal_threshold = 0.25           # Renew when 25% TTL remaining
vault_cache_min_ttl_seconds = 300              # Minimum 5 minutes required
```

#### Example Workflow

```sql
-- First connection: fetches and caches credentials
dbcrust vault://readonly@database/myapp-prod

-- Check cache status
\vc
-- Output: database/myapp-prod/readonly (v-user--ABC123-1234567890) - 0h59m remaining - VALID

-- Reconnect quickly using cached credentials
dbcrust vault://readonly@database/myapp-prod
-- Uses cached credentials, no Vault API call

-- Force refresh if needed
\vcr readonly

-- Clear cache when done
\vcc
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

!!! tip "Column Selection Shortcuts"

    Efficient column selection workflows:

    ```sql
    -- Temporarily adjust threshold for current session
    \csthreshold 5          -- Lower threshold for detailed analysis

    -- Enable manual mode for exploration
    \cs                     -- Now all queries show column selection

    -- Clear and reset when done
    \clrcs                  -- Clear saved selections
    \cs                     -- Disable manual mode
    \csthreshold 10         -- Reset to default threshold
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
    <a href="/dbcrust/reference/url-schemes/" class="md-button md-button--primary">URL Schemes Guide</a>
    <a href="/dbcrust/user-guide/basic-usage/" class="md-button">User Guide</a>
</div>
