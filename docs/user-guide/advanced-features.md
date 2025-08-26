# Advanced Features Guide

DBCrust provides powerful advanced features that go beyond basic database connections. This guide covers named queries, session management, external editor integration, column selection, and other productivity-enhancing capabilities.

## 🔧 Named Queries with Scoping

Save frequently used queries with parameter substitution and intelligent scoping.

### Basic Named Queries

```sql
-- Save a query with parameters
\ns daily_sales SELECT * FROM orders WHERE created_at >= current_date - interval '$1 days';

-- Execute with parameter
daily_sales 7  -- Shows orders from last 7 days
daily_sales 30 -- Shows orders from last 30 days

-- List saved queries
\n
```

### Advanced Parameter Patterns

```sql
-- Multiple parameters with $1, $2, etc.
\ns user_orders SELECT * FROM orders WHERE user_id = $1 AND status = '$2' ORDER BY created_at DESC;

-- Use query
user_orders 123 'completed'

-- Parameter expansion with $* (all parameters)
\ns bulk_update UPDATE products SET category = '$1' WHERE id IN ($*);

-- Use with multiple values
bulk_update electronics 1 2 3 4 5

-- Array-style parameters with $@
\ns product_report SELECT * FROM products WHERE category IN ($@);

-- Use with comma-separated values
product_report 'electronics','books','clothing'
```

### Scoped Named Queries

**Session-local queries (default):**
```sql
-- Only available in current session
\ns session_temp SELECT COUNT(*) FROM temp_analysis_table;
```

**Global queries (available across all databases):**
```sql
-- Available everywhere
\ns connection_info SELECT current_database(), current_user, version() --global
```

**Database-type specific queries:**
```sql
-- Only available for PostgreSQL connections
\ns pg_stats SELECT schemaname, tablename, n_tup_ins, n_tup_del FROM pg_stat_user_tables --postgres

-- Only available for MySQL connections
\ns mysql_processlist SHOW PROCESSLIST --mysql

-- Only available for SQLite connections
\ns sqlite_tables SELECT name FROM sqlite_master WHERE type='table' --sqlite
```

**Check query scoping:**
```sql
\n  -- Shows all queries with scope indicators

-- Output:
-- Named queries:
--   daily_sales      [session]    - SELECT * FROM orders WHERE...
--   connection_info  [global]     - SELECT current_database()...
--   pg_stats         [postgres]   - SELECT schemaname, tablename...
```

## 📄 External Editor Integration

Use your favorite editor for complex queries.

### Basic Editor Usage

```sql
-- Open query in external editor
\ed

-- Edit, save, and close - query executes automatically
-- Empty Enter key executes the last edited query
```

### Editor Configuration

DBCrust uses your `$EDITOR` environment variable:

```bash
# VS Code (waits for editor to close)
export EDITOR="code --wait"

# Vim
export EDITOR="vim"

# Nano
export EDITOR="nano"

# Sublime Text
export EDITOR="subl --wait"

# Emacs
export EDITOR="emacs"
```

### Advanced Editor Features

**Syntax highlighting:** DBCrust creates temporary `.sql` files, so your editor provides SQL syntax highlighting automatically.

**Query templates:**
```sql
-- Use \ed with existing query to edit it
SELECT * FROM users WHERE status = 'active';
\ed  -- Opens editor with above query pre-loaded
```

**Multi-statement scripts:**
```sql
-- Editor supports multiple statements
BEGIN;
UPDATE users SET last_login = NOW() WHERE id = 123;
INSERT INTO user_activity (user_id, activity) VALUES (123, 'login');
COMMIT;
-- All statements execute as a transaction
```

## 📊 Intelligent Column Selection

Handle wide result sets with interactive column selection.

### Automatic Column Selection

```toml
# ~/.config/dbcrust/config.toml
[display]
column_selection_threshold = 10  # Auto-trigger when >10 columns
column_selection_default_all = false  # Opt-in selection behavior
```

**Behavior:**
- Queries returning more columns than threshold show selection interface
- Use arrow keys to navigate, Space to select/deselect, Enter to confirm
- Ctrl+C returns to REPL without executing query

### Manual Column Selection

```sql
-- Force column selection for any query
\cs

-- Now all queries show column selection interface
SELECT * FROM users;  -- Shows column selection even for few columns

-- Toggle off
\cs
```

### Column Selection Commands

```sql
-- Set threshold dynamically
\csthreshold 15  -- Changes threshold to 15 columns

-- Clear saved column selections
\clrcs

-- Reset all view settings
\resetview
```

### Session Persistence

Column selections are remembered per table structure:

```sql
-- First query on 'users' table - shows selection interface
SELECT * FROM users;
-- Select: id, name, email (deselect others)

-- Later query on same table structure - uses saved selection
SELECT * FROM users WHERE active = true;
-- Automatically shows only: id, name, email

-- Different table structure - new selection
SELECT u.id, u.name, p.title FROM users u JOIN posts p ON u.id = p.author_id;
-- Shows new selection interface (different columns)
```

## 💾 Session Management

Save and reuse database connections.

### Basic Session Management

```sql
-- Save current connection as session
\ss production_db

-- List saved sessions
\s

-- Connect to saved session
\s production_db

-- Delete saved session
\sd old_session
```

### Session URL Scheme

```bash
# Connect via command line
dbcrust session://production_db

# Interactive session selection
dbcrust session://
```

### Session Configuration

Sessions store connection parameters (not passwords):

```toml
# ~/.config/dbcrust/config.toml

[saved_sessions.production]
host = "prod.example.com"
port = 5432
user = "app_user"
dbname = "myapp_prod"
database_type = "PostgreSQL"
created_at = "2024-01-15T10:30:00Z"

[saved_sessions.analytics]
host = "analytics.company.com"
port = 5432
user = "analyst"
dbname = "data_warehouse"
database_type = "PostgreSQL"
created_at = "2024-01-15T15:45:00Z"
```

### Password Integration

Sessions integrate with credential stores:

```bash
# PostgreSQL: Uses ~/.pgpass
prod.example.com:5432:myapp_prod:app_user:secret_password

# MySQL: Uses ~/.my.cnf
[client]
host=analytics.company.com
user=analyst
password=analyst_password
```

## 🕐 Connection History

Automatic tracking of recent database connections.

### Recent Connection Features

```sql
-- List recent connections
\r

-- Clear connection history
\rc
```

### Recent URL Scheme

```bash
# Interactive recent connection selection
dbcrust recent://
```

**Example output:**
```
Recent database connections:
1. postgres://user@localhost:5432/myapp (2 minutes ago)
2. docker://postgres-dev/testdb (1 hour ago)
3. mysql://root@mysql-server:3306/analytics (yesterday)

Select connection (1-3): 1
```

### History Configuration

```toml
# ~/.config/dbcrust/config.toml
[history]
max_recent_connections = 15  # Keep last 15 connections
deduplicate = true           # Remove duplicate entries
```

## 🎨 Output Formatting & Display

Customize how query results are displayed.

### Display Modes

```sql
-- Toggle expanded display (vertical layout)
\x

-- Before (horizontal):
-- | id | name        | email                |
-- |----|-------------|----------------------|
-- | 1  | John Smith  | john@example.com     |

-- After (vertical):
-- -[ RECORD 1 ]-------------------
-- id    | 1
-- name  | John Smith
-- email | john@example.com
```

### EXPLAIN Mode

```sql
-- Toggle EXPLAIN mode
\e

-- Now all queries show execution plans
SELECT * FROM users WHERE email = 'user@example.com';

-- Output includes:
-- ○ Execution Time: 1.23 ms • Planning Time: 0.15 ms
-- Index Scan
-- │ Optimized lookup using email_idx
-- │ ○ Duration: 0.96 ms • Cost: 4 • Rows: 1
-- └► id • name • email • created_at
```

### Output Formatting

```toml
# ~/.config/dbcrust/config.toml
[display]
border_style = 1                    # 0=none, 1=light, 2=heavy
date_format = "%Y-%m-%d %H:%M:%S"   # Date display format
number_format = "human"             # "raw" or "human" (with commas)
max_column_width = 50               # Max column width
truncate_long_values = true         # Truncate long text values
null_display = "NULL"               # How to display NULL values
```

### Copy to Clipboard

```sql
-- Copy EXPLAIN output to clipboard
\ecopy

-- Works with any query result - run query first, then copy
SELECT * FROM users LIMIT 5;
\ecopy  -- Copies the table output
```

## 🎯 Complex Data Display

Intelligent formatting for JSON, arrays, vectors, and other complex data types.

### Automatic Data Type Detection

DBCrust automatically detects and formats complex data types:

```sql
-- PostgreSQL JSON/JSONB with syntax highlighting
SELECT user_profile FROM users WHERE id = 1;
-- {"name": "John", "settings": {"theme": "dark"}, "tags": ["admin", "power_user"]}

-- PostgreSQL arrays with proper formatting
SELECT tags FROM posts WHERE published = true;
-- {technology, programming, rust, database}

-- Geographic data (GeoJSON) with coordinate summaries
SELECT location FROM stores;
-- GeoJSON Point: [-122.419, 37.775] (San Francisco, CA)

-- Vector data (pgvector extension)
SELECT embedding FROM documents WHERE title = 'Machine Learning Basics';
-- Vector[384]: [0.123, -0.456, 0.789, ...] (similarity search ready)
```

### Display Modes

Configure how complex data is displayed:

```toml
# ~/.config/dbcrust/config.toml
[complex_display]
display_mode = "truncated"          # Options: "full", "truncated", "summary", "viz"
truncation_length = 8               # Characters shown in truncated mode
show_metadata = true                # Display type information
```

**Display Mode Examples:**

**Full Mode** - Shows complete data structure:
```json
{
  "user_id": 12345,
  "preferences": {
    "theme": "dark",
    "notifications": true,
    "language": "en-US"
  },
  "recent_activity": [
    {"action": "login", "timestamp": "2024-01-15T10:30:00Z"},
    {"action": "update_profile", "timestamp": "2024-01-15T10:35:00Z"}
  ]
}
```

**Truncated Mode** - Shows first N characters with ellipsis:
```json
{"user_id": 12345, "preferences": {"theme": "dark"...}} [124 chars]
```

**Summary Mode** - Shows structure overview:
```
JSON Object (3 keys): user_id, preferences, recent_activity
├─ preferences: Object (3 keys)
└─ recent_activity: Array (2 elements)
```

**Visualization Mode** - ASCII art representation:
```
┌─ JSON Object ─┐
│ user_id: 12345│
│ preferences ──┤
│ activity[2] ──┤
└───────────────┘
```

### Intelligent Mode Switching

DBCrust automatically selects the best display mode based on data size:

```toml
[complex_display]
size_threshold = 30                 # Auto-switch modes for data >30 elements
display_mode = "truncated"          # Default mode for small data
```

**Auto-switching behavior:**
- **Small data (≤30 elements)**: Uses configured `display_mode`
- **Large data (>30 elements)**: Automatically switches to more compact modes
- **Very large data (>100 elements)**: Switches to summary mode

### Database-Specific Formatting

DBCrust handles different database formats intelligently:

**PostgreSQL:**
```sql
-- Array format detection
SELECT ARRAY[1,2,3]::integer[];     -- Displays as: {1, 2, 3}
SELECT '[1,2,3]'::json;             -- Displays as: [1, 2, 3] (JSON syntax)

-- JSONB with syntax highlighting
SELECT '{"status": "active"}'::jsonb;
-- {
--   "status": "active"  ✓ (formatted with colors)
-- }
```

**MongoDB:**
```javascript
// BSON document formatting
db.users.findOne()
// {
//   "_id": ObjectId("507f1f77bcf86cd799439011"),
//   "name": "John Doe",
//   "settings": {
//     "notifications": true
//   }
// }
```

**ClickHouse:**
```sql
-- Tuple formatting
SELECT (1, 'hello', [1,2,3]) AS complex_tuple;
-- Tuple(3): (1, "hello", [1, 2, 3])
```

### Complex Display Commands

Interactive commands for controlling complex data display:

```sql
-- Toggle between display modes
\cdm                    -- Show current complex display mode
\cdm full               -- Set to full mode
\cdm truncated          -- Set to truncated mode
\cdm summary            -- Set to summary mode
\cdm viz                -- Set to visualization mode

-- Adjust display settings
\cdt 15                 -- Set truncation length to 15 characters
\cds 50                 -- Set size threshold to 50 elements

-- Toggle metadata display
\cdmeta                 -- Toggle metadata on/off
\cddim                  -- Toggle dimension display on/off
```

### Performance Considerations

Complex display formatting is optimized for performance:

```toml
[complex_display]
# Performance tuning
max_width = 100                     # Limit display width
size_threshold = 30                 # Smaller threshold = less processing
show_metadata = false               # Disable for faster display
```

**Performance tips:**
- Use `truncated` mode for large datasets
- Set lower `size_threshold` for faster processing
- Disable `show_metadata` for maximum speed
- Use `summary` mode for exploring large JSON structures

### Examples by Data Type

**JSON Analytics:**
```sql
-- E-commerce analytics with complex JSON
SELECT
    order_id,
    customer_data,           -- JSON with nested preferences
    item_details,           -- Array of product objects
    shipping_address        -- GeoJSON location data
FROM orders
WHERE created_at > current_date - interval '1 day';

-- Results automatically formatted:
-- customer_data: {"id": 12345, "tier": "gold", "preferences"...} [234 chars]
-- item_details: Array[3]: [{"sku": "ABC-123", "qty": 2...}]
-- shipping_address: GeoJSON Point: [-74.006, 40.714] (New York, NY)
```

**Machine Learning Vectors:**
```sql
-- Vector similarity search with readable results
SELECT
    title,
    content_summary,
    embedding                -- Vector[1536] from OpenAI embeddings
FROM documents
ORDER BY embedding <-> '[0.1, -0.2, 0.3, ...]'::vector
LIMIT 5;

-- Results show:
-- embedding: Vector[1536]: [0.123, -0.456, 0.789, ...] (cosine similarity ready)
```

**Geographic Analysis:**
```sql
-- Spatial data with intelligent coordinate formatting
SELECT
    store_name,
    location,               -- PostGIS geometry
    service_area           -- GeoJSON polygon
FROM retail_locations
WHERE ST_DWithin(location, ST_Point(-122.4194, 37.7749), 1000);

-- Results show:
-- location: POINT(-122.4194 37.7749) → San Francisco, CA
-- service_area: GeoJSON Polygon: 4 vertices, ~2.3 km²
```

## 🔍 Advanced Query Features

Enhanced query execution and analysis capabilities.

### Query Timing

```toml
# ~/.config/dbcrust/config.toml
[database]
show_execution_time = true      # Show timing for all queries
auto_explain_threshold = 1000   # Auto-EXPLAIN for queries >1s
```

### Query History

```sql
-- DBCrust maintains query history per session
-- Use up/down arrows to navigate history

-- Or access history programmatically
\history  -- Show recent queries
```

### Transaction Management

```sql
-- DBCrust handles transactions transparently
BEGIN;
UPDATE users SET last_login = NOW() WHERE id = 123;
-- Connection remains in transaction state
SELECT * FROM users WHERE id = 123;  -- Shows updated data
COMMIT;
```

### Batch Operations

```sql
-- Load and execute SQL files
\i /path/to/script.sql

-- Save queries to files
\w /path/to/query.sql
```

## 🛠️ Configuration Management

Manage DBCrust settings and preferences.

### View Configuration

```sql
-- Show current configuration
\config

-- Show specific section
\config database
\config display
```

### Runtime Configuration

```sql
-- Some settings can be changed at runtime
\set show_execution_time true
\set max_column_width 80
```

### Environment Variables

```bash
# Override config with environment variables
export DBCRUST_LOG_LEVEL=debug
export DBCRUST_DEFAULT_LIMIT=500
export DBCRUST_SHOW_BANNER=false

dbcrust postgres://user@localhost/db
```

### Configuration Templates

**Development configuration:**
```toml
[database]
default_limit = 100
show_execution_time = true
auto_explain_threshold = 500

[display]
column_selection_threshold = 8
max_column_width = 100

[logging]
level = "debug"
```

**Production configuration:**
```toml
[database]
default_limit = 1000
show_execution_time = false
auto_explain_threshold = 2000

[display]
column_selection_threshold = 15
max_column_width = 50

[security]
verify_ssl = true
require_confirmation_for_deletes = true
```

## 🚨 Troubleshooting Advanced Features

### Named Queries Issues

**Query not found:**
```sql
-- Check available queries
\n

-- Check specific scope
\n --scope global
\n --scope postgres
```

**Parameter substitution not working:**
```sql
-- Check parameter syntax - use $1, $2, not {1}, {2}
\ns test_query SELECT * FROM users WHERE id = $1;  -- Correct
```

### Editor Integration Issues

**Editor not opening:**
```bash
# Check EDITOR environment variable
echo $EDITOR

# Test editor directly
$EDITOR test.sql
```

**Query not executing after editing:**
- Make sure to save the file in your editor
- Press Enter after closing editor to execute

### Column Selection Issues

**Selection interface not showing:**
```toml
# Check threshold setting
[display]
column_selection_threshold = 10  # Lower this value
```

**Ctrl+C not working:**
- This depends on your terminal - try Ctrl+D or Escape

### Session Management Issues

**Session not saving:**
```bash
# Check config directory permissions
ls -la ~/.config/dbcrust/
chmod 755 ~/.config/dbcrust/
```

**Password prompts despite saved session:**
- Sessions don't store passwords - set up `.pgpass` or `.my.cnf`

### Complex Display Issues

**Data not formatting correctly:**
```sql
-- Check complex display settings
\cdm  -- Show current display mode

-- Try different modes
\cdm full      -- Show complete data
\cdm summary   -- Show structure overview
```

**Performance issues with large JSON:**
```toml
# Optimize for performance
[complex_display]
display_mode = "truncated"
truncation_length = 6
size_threshold = 20        # Lower threshold
show_metadata = false      # Disable for speed
```

**Array format confusion:**
- PostgreSQL arrays: `{1,2,3}` format (native)
- JSON arrays: `[1,2,3]` format (JSON/JSONB columns)
- DBCrust automatically detects and formats each type correctly

## 📚 See Also

- **[Configuration Reference](/dbcrust/reference/configuration-reference/)** - Complete configuration options
- **[Backslash Commands](/dbcrust/reference/backslash-commands/)** - All interactive commands
- **[Performance Analysis](/dbcrust/user-guide/performance-analysis/)** - Query optimization guide
- **[Troubleshooting](/dbcrust/user-guide/troubleshooting/)** - Common issues and solutions

---

<div align="center">
    <strong>Ready to master DBCrust's advanced features?</strong><br>
    <a href="/dbcrust/user-guide/performance-analysis/" class="md-button md-button--primary">Performance Analysis</a>
    <a href="/dbcrust/reference/backslash-commands/" class="md-button">Command Reference</a>
</div>
