# Configuration Reference

DBCrust provides extensive configuration options through TOML configuration files, environment variables, and command-line arguments. This reference covers all available settings and their usage.

## 📁 Configuration Files

### Primary Configuration File

**Location**: `~/.config/dbcrust/config.toml`

This is the main configuration file containing all settings and user data.

### Storage Files

DBCrust uses dedicated files for different types of data:

| File | Purpose | Format |
|------|---------|--------|
| `~/.config/dbcrust/config.toml` | Main configuration and settings | TOML |
| `~/.config/dbcrust/named_queries.toml` | Named query storage | TOML |
| `~/.config/dbcrust/vault_credentials.enc` | Encrypted Vault credentials | Binary (AES-256-GCM) |
| `~/.config/dbcrust/dbcrust.log` | Application logs | Plain text |

### Configuration Hierarchy

Settings are applied in the following order (highest precedence first):

1. **Command-line arguments** (highest priority)
2. **Environment variables**
3. **Configuration file** (`config.toml`)
4. **Default values** (lowest priority)

## ⚙️ Core Configuration Sections

### `[database]` - Database Connection Settings

Controls database connection behavior and query execution.

```toml
[database]
# Default query result limit
default_limit = 1000

# Connection timeout in seconds
timeout = 30

# Maximum number of connection retries
max_retries = 3

# Show query execution time
show_execution_time = true

# Automatically run EXPLAIN for slow queries
auto_explain_threshold = 1000  # milliseconds

# SSL verification mode
verify_ssl = true

# Query timeout in seconds
query_timeout = 300
```

**Settings:**

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `default_limit` | Integer | `1000` | Default LIMIT for queries without explicit limit |
| `timeout` | Integer | `30` | Database connection timeout (seconds) |
| `max_retries` | Integer | `3` | Maximum connection retry attempts |
| `show_execution_time` | Boolean | `true` | Display query execution time |
| `auto_explain_threshold` | Integer | `1000` | Auto-EXPLAIN queries slower than N milliseconds |
| `verify_ssl` | Boolean | `true` | Verify SSL certificates |
| `query_timeout` | Integer | `300` | Query execution timeout (seconds) |

### `[display]` - Output Formatting Settings

Controls how query results and data are displayed.

```toml
[display]
# Table formatting
border_style = 1                    # 0=none, 1=light, 2=heavy
max_column_width = 50               # Maximum width for table columns
truncate_long_values = true         # Truncate values longer than max width
null_display = "NULL"               # How to display NULL values

# Date and number formatting
date_format = "%Y-%m-%d %H:%M:%S"   # strftime format for timestamps
number_format = "human"             # "raw" or "human" (with thousands separator)

# Column selection settings
column_selection_threshold = 10     # Auto-trigger when >N columns
column_selection_default_all = false # Default to all columns selected

# Pagination
enable_paging = true                # Enable result paging for large outputs
page_size = 50                      # Rows per page when paging enabled

# Output formats
default_output_format = "table"     # "table", "csv", "json", "expanded"
```

**Settings:**

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `border_style` | Integer | `1` | Table border style (0=none, 1=light, 2=heavy) |
| `max_column_width` | Integer | `50` | Maximum column width in characters |
| `truncate_long_values` | Boolean | `true` | Truncate values exceeding max width |
| `null_display` | String | `"NULL"` | String to display for NULL values |
| `date_format` | String | `"%Y-%m-%d %H:%M:%S"` | Date/time display format |
| `number_format` | String | `"human"` | Number format ("raw" or "human") |
| `column_selection_threshold` | Integer | `10` | Column count for auto-selection |
| `column_selection_default_all` | Boolean | `false` | Default column selection behavior |
| `enable_paging` | Boolean | `true` | Enable pagination for large results |
| `page_size` | Integer | `50` | Number of rows per page |
| `default_output_format` | String | `"table"` | Default output format |

### `[ui]` - User Interface Settings

Controls the interactive experience and CLI behavior.

```toml
[ui]
# Startup behavior
show_banner = true                  # Show DBCrust banner on startup
auto_connect = false                # Auto-connect to recent database

# Prompt customization
show_database_in_prompt = true      # Show database name in prompt
show_host_in_prompt = false         # Show hostname in prompt
prompt_format = "{user}@{db}=> "    # Custom prompt format

# Interactive features
enable_autocomplete = true          # Enable SQL/command autocompletion
autocomplete_min_chars = 2          # Minimum characters before suggestions
history_size = 1000                 # Command history size

# Confirmation prompts
confirm_destructive_operations = true  # Confirm DROP, DELETE, etc.
confirm_large_results = true          # Confirm queries returning many rows
large_result_threshold = 10000        # Threshold for "large" results
```

**Settings:**

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `show_banner` | Boolean | `true` | Display startup banner |
| `auto_connect` | Boolean | `false` | Automatically connect to recent database |
| `show_database_in_prompt` | Boolean | `true` | Include database name in prompt |
| `show_host_in_prompt` | Boolean | `false` | Include hostname in prompt |
| `prompt_format` | String | `"{user}@{db}=> "` | Custom prompt template |
| `enable_autocomplete` | Boolean | `true` | Enable autocompletion |
| `autocomplete_min_chars` | Integer | `2` | Minimum characters for suggestions |
| `history_size` | Integer | `1000` | Maximum command history entries |
| `confirm_destructive_operations` | Boolean | `true` | Confirm dangerous operations |
| `confirm_large_results` | Boolean | `true` | Confirm large result sets |
| `large_result_threshold` | Integer | `10000` | Row count threshold for confirmation |

### `[logging]` - Logging Configuration

Controls application logging behavior.

```toml
[logging]
# Log level: trace, debug, info, warn, error
level = "info"

# Output destinations
console_output = true               # Log to console/terminal
file_output = false                 # Log to file

# File logging settings
file_path = "~/.config/dbcrust/dbcrust.log"  # Log file location
max_file_size_mb = 10              # Max log file size before rotation
max_files = 5                       # Number of rotated files to keep

# Log filtering
enable_query_logging = false        # Log all executed queries
enable_performance_logging = true   # Log performance metrics
log_connection_events = true        # Log connection attempts
```

**Settings:**

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `level` | String | `"info"` | Minimum log level (trace/debug/info/warn/error) |
| `console_output` | Boolean | `true` | Enable console logging |
| `file_output` | Boolean | `false` | Enable file logging |
| `file_path` | String | `"~/.config/dbcrust/dbcrust.log"` | Log file path |
| `max_file_size_mb` | Integer | `10` | Max file size before rotation (MB) |
| `max_files` | Integer | `5` | Number of rotated files to keep |
| `enable_query_logging` | Boolean | `false` | Log all SQL queries |
| `enable_performance_logging` | Boolean | `true` | Log performance metrics |
| `log_connection_events` | Boolean | `true` | Log connection attempts |

### `[ssh_tunnel_patterns]` - SSH Tunnel Configuration

Automatic SSH tunnel patterns based on hostname matching.

```toml
[ssh_tunnel_patterns]
# Pattern = "ssh_connection_string"
"^db\\.internal\\..*\\.com$" = "user@jumphost.example.com:2222"
"^.*\\.prod\\.company\\.com$" = "deploy@bastion.company.com"
"^postgres-.*\\.docker\\.local$" = "docker@localhost:2222"

# Multiple patterns can be defined
"^staging-.*" = "staging-user@staging-jumphost:22"
"^dev-.*" = "dev-user@dev-jumphost:22"
```

**Pattern Format:**
- **Key**: Regular expression matching hostname
- **Value**: SSH connection string (`user@host:port`)

**Examples:**
```bash
# Connecting to db.internal.example.com automatically uses jumphost
dbcrust postgres://user@db.internal.example.com:5432/mydb
# → Tunnels through user@jumphost.example.com:2222

# Multiple patterns can match
dbcrust postgres://app@staging-postgres.company.com:5432/app
# → Tunnels through staging-user@staging-jumphost:22
```

### `[vault]` - HashiCorp Vault Integration

Settings for Vault dynamic credentials.

```toml
[vault]
# Vault server configuration
addr = "https://vault.company.com"
token_file = "~/.vault-token"

# Credential caching
credential_cache_enabled = true
cache_renewal_threshold = 0.25      # Renew when 25% TTL remaining
cache_min_ttl_seconds = 300         # Minimum 5 minutes TTL required

# Default vault paths
default_mount_path = "database"
default_role = "readonly"

# Authentication
auth_method = "token"               # "token", "userpass", "ldap", etc.
```

**Settings:**

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `addr` | String | `""` | Vault server URL |
| `token_file` | String | `"~/.vault-token"` | Vault token file location |
| `credential_cache_enabled` | Boolean | `true` | Enable credential caching |
| `cache_renewal_threshold` | Float | `0.25` | TTL percentage for renewal |
| `cache_min_ttl_seconds` | Integer | `300` | Minimum TTL for cached credentials |
| `default_mount_path` | String | `"database"` | Default mount path |
| `default_role` | String | `"readonly"` | Default role name |
| `auth_method` | String | `"token"` | Vault authentication method |

### `[complex_display]` - Complex Data Type Display

Controls how complex data types (JSON, arrays, vectors, etc.) are displayed and formatted.

```toml
[complex_display]
# Display mode for complex data
display_mode = "truncated"          # "full", "truncated", "summary", "viz"

# Truncation settings
truncation_length = 8               # Characters to show in truncated mode
viz_width = 60                      # Width for visualization mode

# Metadata display
show_metadata = true                # Show type info and dimensions
show_dimensions = true              # Show array/object dimensions
show_numbers = false                # Show element numbers in full mode

# Size thresholds
size_threshold = 30                 # Elements threshold for mode switching
full_elements_per_row = 10          # Elements per row in full mode
max_width = 100                     # Maximum display width
json_pretty_print = false           # Compact JSON by default
```

**Settings:**

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `display_mode` | String | `"truncated"` | Default display mode (full/truncated/summary/viz) |
| `truncation_length` | Integer | `8` | Characters shown in truncated mode |
| `viz_width` | Integer | `60` | Character width for visualization display |
| `show_metadata` | Boolean | `true` | Display data type and structure information |
| `show_dimensions` | Boolean | `true` | Show array dimensions and object key counts |
| `show_numbers` | Boolean | `false` | Show element numbers in full display |
| `size_threshold` | Integer | `30` | Element count threshold for auto-mode switching |
| `full_elements_per_row` | Integer | `10` | Elements displayed per row in full mode |
| `max_width` | Integer | `100` | Maximum character width for displays |
| `json_pretty_print` | Boolean | `false` | Whether to pretty-print JSON (false=compact, true=formatted) |

**Supported Data Types:**
- **JSON/JSONB**: PostgreSQL JSON data with syntax highlighting
- **GeoJSON**: Geographic data with coordinate summaries
- **Arrays**: PostgreSQL arrays (`{1,2,3}` format) and JSON arrays
- **Vectors**: PostgreSQL vector extension data (pgvector)
- **BSON Documents**: MongoDB document structures
- **Tuples**: ClickHouse tuple data types
- **Maps**: Key-value pair structures

### `[docker]` - Docker Integration Settings

Configuration for Docker container discovery and connections.

```toml
[docker]
# Container discovery
enable_discovery = true
discovery_timeout = 5               # Seconds to wait for container list

# Container filtering
supported_images = [               # Image patterns for database containers
    "postgres*",
    "mysql*",
    "mariadb*",
    "sqlite*"
]

# Connection defaults
default_user = "postgres"           # Default username for containers
prefer_named_containers = true     # Prefer containers with custom names

# OrbStack integration (macOS)
enable_orbstack_integration = true
```

**Settings:**

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `enable_discovery` | Boolean | `true` | Enable automatic container discovery |
| `discovery_timeout` | Integer | `5` | Timeout for container listing (seconds) |
| `supported_images` | Array | `["postgres*", "mysql*", "mariadb*", "sqlite*"]` | Database image patterns |
| `default_user` | String | `"postgres"` | Default container username |
| `prefer_named_containers` | Boolean | `true` | Prefer named over auto-generated names |
| `enable_orbstack_integration` | Boolean | `true` | Enable OrbStack support on macOS |

## 🔧 Advanced Configuration

### Session Storage

Saved database sessions are stored in the main config:

```toml
[saved_sessions.production]
host = "prod.db.company.com"
port = 5432
user = "app_user"
dbname = "myapp_prod"
database_type = "PostgreSQL"
created_at = "2024-01-15T10:30:00Z"

[saved_sessions.staging]
host = "staging.db.company.com"
port = 5432
user = "app_user"
dbname = "myapp_staging"
database_type = "PostgreSQL"
created_at = "2024-01-15T11:15:00Z"
```

### Connection History

Recent connections are automatically tracked:

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

### Named Query Storage

Named queries are stored separately by scope:

```toml
# ~/.config/dbcrust/named_queries.toml

[global]
count_all = "SELECT COUNT(*) FROM $1"
user_by_id = "SELECT * FROM users WHERE id = $1"

[postgres]
table_sizes = "SELECT schemaname, tablename, pg_size_pretty(pg_total_relation_size(schemaname||'.'||tablename)) as size FROM pg_tables ORDER BY pg_total_relation_size(schemaname||'.'||tablename) DESC"

[mysql]
show_status = "SHOW GLOBAL STATUS LIKE '$1'"

# Session-local queries stored per connection
["session:postgres://user@localhost:5432/myapp"]
daily_report = "SELECT DATE(created_at), COUNT(*) FROM orders GROUP BY DATE(created_at)"
```

## 🌍 Environment Variables

Override configuration settings with environment variables:

### Database Settings
```bash
export DBCRUST_DEFAULT_LIMIT=500
export DBCRUST_TIMEOUT=60
export DBCRUST_SHOW_EXECUTION_TIME=true
export DBCRUST_AUTO_EXPLAIN_THRESHOLD=2000
```

### Display Settings
```bash
export DBCRUST_BORDER_STYLE=2
export DBCRUST_MAX_COLUMN_WIDTH=80
export DBCRUST_DATE_FORMAT="%m/%d/%Y %I:%M %p"
export DBCRUST_COLUMN_SELECTION_THRESHOLD=15
```

### Complex Display Settings
```bash
export DBCRUST_COMPLEX_DISPLAY_MODE=truncated
export DBCRUST_COMPLEX_TRUNCATION_LENGTH=12
export DBCRUST_COMPLEX_SHOW_METADATA=true
export DBCRUST_COMPLEX_SIZE_THRESHOLD=50
```

### UI Settings
```bash
export DBCRUST_SHOW_BANNER=false
export DBCRUST_ENABLE_AUTOCOMPLETE=true
export DBCRUST_HISTORY_SIZE=2000
```

### Logging Settings
```bash
export DBCRUST_LOG_LEVEL=debug
export DBCRUST_CONSOLE_OUTPUT=true
export DBCRUST_FILE_OUTPUT=true
export DBCRUST_LOG_PATH="/var/log/dbcrust.log"
```

### Vault Settings
```bash
export VAULT_ADDR="https://vault.company.com"
export VAULT_TOKEN="your-vault-token"
export DBCRUST_VAULT_CACHE_ENABLED=true
```

### Docker Settings
```bash
export DBCRUST_DOCKER_DISCOVERY=true
export DBCRUST_DOCKER_TIMEOUT=10
export DOCKER_HOST="unix:///var/run/docker.sock"
```

## 🎯 Command-Line Arguments

Override any setting via command-line arguments:

### Connection Arguments
```bash
dbcrust --timeout 60 --max-retries 5 postgres://localhost/db
dbcrust --no-ssl-verify mysql://host/db
dbcrust --query-timeout 600 postgres://host/db
```

### Display Arguments
```bash
dbcrust --border-style 2 --max-width 100 postgres://localhost/db
dbcrust --output json --no-truncate postgres://localhost/db
dbcrust --column-threshold 20 postgres://localhost/db

# Complex display arguments
dbcrust --complex-display-mode full postgres://localhost/db
dbcrust --complex-truncation 15 --no-complex-metadata postgres://localhost/db
dbcrust --json-pretty-print postgres://localhost/db
```

### UI Arguments
```bash
dbcrust --no-banner --no-autocomplete postgres://localhost/db
dbcrust --prompt-format "{host}:{db}$ " postgres://localhost/db
```

### Logging Arguments
```bash
dbcrust --debug --log-file debug.log postgres://localhost/db
dbcrust --log-level trace --log-queries postgres://localhost/db
```

### Utility Arguments
```bash
dbcrust --show-config                    # Display current configuration
dbcrust --validate-config               # Validate configuration file
dbcrust --show-config-path              # Show config file location
dbcrust --reset-config                  # Reset to default configuration
```

## 🔧 Configuration Management

### View Current Configuration

```bash
# Show all configuration
dbcrust --show-config

# Show specific sections (within DBCrust)
\config
\config database
\config display
```

### Validate Configuration

```bash
# Validate configuration file syntax
dbcrust --validate-config

# Test configuration with connection
dbcrust --test-config postgres://localhost/test
```

### Configuration Templates

#### Development Configuration
```toml
[database]
default_limit = 100
show_execution_time = true
auto_explain_threshold = 500

[display]
column_selection_threshold = 8
max_column_width = 100

[complex_display]
display_mode = "full"
show_metadata = true
show_dimensions = true

[ui]
show_banner = true
enable_autocomplete = true

[logging]
level = "debug"
console_output = true
enable_query_logging = true
```

#### Production Configuration
```toml
[database]
default_limit = 1000
show_execution_time = false
auto_explain_threshold = 2000
verify_ssl = true

[display]
column_selection_threshold = 15
max_column_width = 50

[complex_display]
display_mode = "truncated"
truncation_length = 6
show_metadata = false

[ui]
show_banner = false
confirm_destructive_operations = true

[logging]
level = "warn"
console_output = false
file_output = true
file_path = "/var/log/dbcrust.log"
```

#### Team Shared Configuration
```toml
[database]
default_limit = 500
show_execution_time = true

[complex_display]
display_mode = "truncated"
show_metadata = true
size_threshold = 25

[ui]
confirm_destructive_operations = true
confirm_large_results = true
large_result_threshold = 5000

[ssh_tunnel_patterns]
"^.*\\.internal\\.company\\.com$" = "user@bastion.company.com"
"^.*\\.prod\\..*$" = "prod-user@prod-bastion.company.com:2222"

[vault]
addr = "https://vault.company.com"
default_mount_path = "database"
default_role = "readonly"
```

## 🚨 Troubleshooting Configuration

### Common Issues

**Configuration file not found:**
```bash
# Check config file location
dbcrust --show-config-path

# Create default configuration
mkdir -p ~/.config/dbcrust
dbcrust --reset-config
```

**Invalid TOML syntax:**
```bash
# Validate configuration
dbcrust --validate-config

# Check for syntax errors
toml-check ~/.config/dbcrust/config.toml  # if toml-check installed
```

**Environment variables not working:**
```bash
# Check environment variable names (case-sensitive)
env | grep DBCRUST

# Test with explicit setting
DBCRUST_LOG_LEVEL=debug dbcrust postgres://localhost/db
```

**Settings not taking effect:**
```bash
# Check configuration hierarchy
dbcrust --show-config postgres://localhost/db

# Verify precedence: CLI args > env vars > config file > defaults
```

### Configuration Backup

```bash
# Backup configuration directory
cp -r ~/.config/dbcrust ~/.config/dbcrust.backup

# Restore from backup
rm -rf ~/.config/dbcrust
mv ~/.config/dbcrust.backup ~/.config/dbcrust
```

### Migration Notes

**Upgrading from older versions:**
- Named queries are automatically migrated to the scoped system
- Vault credentials are re-encrypted with updated security
- Configuration format is automatically updated

**Breaking changes:**
- Version 0.15.0+: Named queries moved to separate file
- Version 0.14.0+: Vault credential caching introduced
- Version 0.13.0+: SSH tunnel patterns changed format

## 📚 See Also

- **[URL Schemes & Autocompletion](/dbcrust/reference/url-schemes/)** - Connection URL formats
- **[Backslash Commands](/dbcrust/reference/backslash-commands/)** - Interactive commands
- **[User Guide](/dbcrust/user-guide/basic-usage/)** - Usage patterns and workflows

---

<div align="center">
    <strong>Need help with configuration?</strong><br>
    <a href="/dbcrust/user-guide/troubleshooting/" class="md-button md-button--primary">Troubleshooting Guide</a>
    <a href="/dbcrust/reference/backslash-commands/" class="md-button">Commands Reference</a>
</div>
