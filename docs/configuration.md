# Configuration

DBCrust stores its configuration in a TOML file located at `~/.config/dbcrust/config.toml`. The configuration is automatically created with sensible defaults when you first run DBCrust.

## 📍 Configuration Location

```bash
# Default configuration directory
~/.config/dbcrust/
├── config.toml             # Main configuration file
├── recent.toml             # Recent connections storage
├── vault_credentials.enc   # Encrypted vault credentials cache
└── history.txt             # Command history
```

## 🔧 Configuration Structure

### Complete Example

```toml
# ~/.config/dbcrust/config.toml

[database]
default_limit = 1000
expanded_display_default = false
show_execution_time = true
auto_explain_threshold = 1000  # ms
null_display = "NULL"

[display]
border_style = 1  # 0=none, 1=light, 2=heavy
date_format = "%Y-%m-%d %H:%M:%S"
number_format = "human"  # "raw" or "human" (with commas)
max_column_width = 50
truncate_long_values = true

# Column Selection Settings
column_selection_threshold = 10        # Auto-trigger when result has more than N columns

# Editor settings are controlled via $EDITOR environment variable
# export EDITOR="code --wait"

[history]
max_entries = 10000
save_unnamed_queries = true
deduplicate = true
max_recent_connections = 10

[ssh_tunnel_patterns]
"^db\\.internal\\..*\\.com$" = "jumphost.example.com"
".*\\.private\\.net" = "user@jumphost.example.com:2222"
"prod-.*\\.company\\.com" = "bastion.company.com:22"

[vault]
addr = "https://vault.company.com"
mount_point = "database"
auth_method = "token"  # "token", "userpass", "ldap"
timeout = 30

# Vault Credential Caching
vault_credential_cache_enabled = true          # Enable/disable credential caching
vault_cache_renewal_threshold = 0.25           # Renew when 25% of TTL remaining
vault_cache_min_ttl_seconds = 300              # Minimum TTL required (5 minutes)

[security]
verify_ssl = true
ssl_cert_path = ""
ssl_key_path = ""
password_cache_timeout = 3600  # seconds

[performance]
connection_timeout = 30
query_timeout = 300
pool_max_connections = 10
enable_connection_pooling = true

[completion]
enabled = true
cache_duration = 300  # seconds
max_suggestions = 20
fuzzy_matching = true

[logging]
level = "info"  # "error", "warn", "info", "debug", "trace"
file_path = "~/.config/dbcrust/dbcrust.log"
max_file_size = "10MB"
max_files = 5

# Saved sessions (added by \ss command)
[saved_sessions.production]
host = "prod.example.com"
port = 5432
user = "app_user"
dbname = "myapp"
database_type = "PostgreSQL"
created_at = "2024-01-15T10:30:00Z"

[saved_sessions.staging]
host = "staging.example.com"
port = 3306
user = "root"
dbname = "myapp_staging"
database_type = "MySQL"
created_at = "2024-01-14T15:45:00Z"

# Note: Recent connections are stored separately in ~/.config/dbcrust/recent.toml
```

## 📄 Recent Connections File

DBCrust automatically tracks recent connections in a separate file to avoid mixing transient data with your configuration:

**File:** `~/.config/dbcrust/recent.toml`

```toml
# Recent connections (automatically tracked)
[[connections]]
connection_url = "postgres://postgres@myapp-postgres.orb.local:5432/myapp # Docker: myapp-postgres"
display_name = "postgres@myapp-postgres.orb.local:5432/myapp (Docker: myapp-postgres)"
timestamp = "2024-01-15T14:22:33Z"
database_type = "PostgreSQL"
success = true

[[connections]]
connection_url = "postgres://user@localhost:5432/testdb"
display_name = "user@localhost:5432/testdb"
timestamp = "2024-01-15T14:20:15Z"
database_type = "PostgreSQL"
success = true
```

This file is managed automatically and stores up to the configured number of recent connections (default: 10). You can control this limit with the `max_recent_connections` setting in your main config file.

## ⚙️ Configuration Sections

### [database] - Database Behavior

Controls default database connection and query behavior.

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `default_limit` | integer | `1000` | Default LIMIT for queries without explicit LIMIT |
| `expanded_display_default` | boolean | `false` | Start in expanded display mode |
| `show_execution_time` | boolean | `true` | Show query execution time |
| `auto_explain_threshold` | integer | `1000` | Auto-enable EXPLAIN for slow queries (ms) |
| `null_display` | string | `"NULL"` | How to display NULL values |

**Example:**
```toml
[database]
default_limit = 500
expanded_display_default = true
show_execution_time = true
auto_explain_threshold = 2000
null_display = "∅"
```

### [display] - Output Formatting

Controls how query results and tables are displayed.

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `border_style` | integer | `1` | Table border style (0=none, 1=light, 2=heavy) |
| `date_format` | string | `"%Y-%m-%d %H:%M:%S"` | Date/timestamp display format |
| `number_format` | string | `"human"` | Number formatting (`"raw"` or `"human"`) |
| `max_column_width` | integer | `50` | Maximum column width before truncation |
| `truncate_long_values` | boolean | `true` | Truncate long text values |
| `column_selection_threshold` | integer | `10` | Auto-trigger column selection when results exceed N columns |

**Column Selection Configuration:**

- **`column_selection_threshold`**: Automatically shows column selection interface when queries return more columns than this number

**Example:**
```toml
[display]
border_style = 2
date_format = "%d/%m/%Y %H:%M"
number_format = "human"
max_column_width = 80
truncate_long_values = false

# Column selection settings
column_selection_threshold = 15        # Higher threshold for experienced users
```

**Column Selection Behavior:**

- **Auto-Trigger Mode**: Column selection appears when query results have more columns than the threshold
- **Force Mode**: Use `\cs` to force column selection for all queries (toggle on/off)
- **Runtime Control**: Use `\cs` to toggle force mode and `\csthreshold N` to change threshold temporarily

### [editor] - External Editor

Configuration for external editor integration (`\ed` command).

The editor integration uses your system's `$EDITOR` environment variable to determine which editor to launch. Temporary files are automatically created in the system temp directory.

**How it works:**
- Uses `$EDITOR` environment variable (falls back to vim/nano/notepad)
- Creates temporary files in system temp directory
- Syntax highlighting is handled by your editor, not DBCrust

**Popular editor configurations via environment variable:**
```bash
# VS Code (waits for editor to close)
export EDITOR="code --wait"

# Vim/Neovim
export EDITOR="vim"

# Nano
export EDITOR="nano"

# Sublime Text (waits for editor to close)
export EDITOR="subl --wait"

# Emacs
export EDITOR="emacs"
```

**Using the editor:**
```sql
-- Edit current query in external editor
\ed

-- Load script from file
\i filename.sql

-- Empty Enter executes last edited/loaded script
```

### [history] - History and Session Management

Controls command history and recent connection tracking.

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `max_entries` | integer | `10000` | Maximum number of command history entries |
| `save_unnamed_queries` | boolean | `true` | Save unnamed queries in history |
| `deduplicate` | boolean | `true` | Remove duplicate entries from history |
| `max_recent_connections` | integer | `10` | Maximum number of recent connections to track |

**Example:**
```toml
[history]
max_entries = 5000
save_unnamed_queries = true
deduplicate = true
max_recent_connections = 15
```

### [ssh_tunnel_patterns] - Automatic SSH Tunneling

Define patterns for automatic SSH tunnel creation based on hostname.

**Format:** `"hostname_pattern" = "ssh_target"`

**Examples:**
```toml
[ssh_tunnel_patterns]
# Internal company databases
"^db\\.internal\\..*\\.com$" = "jumphost.example.com"

# Private network
".*\\.private\\.net" = "user@jumphost.example.com:2222"

# Production environment
"prod-.*\\.company\\.com" = "bastion.company.com:22"

# AWS RDS through bastion
".*\\.rds\\.amazonaws\\.com$" = "ec2-bastion.company.com"
```

### [vault] - HashiCorp Vault Integration

Configuration for dynamic database credentials via Vault, including intelligent credential caching.

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `addr` | string | `$VAULT_ADDR` | Vault server address |
| `mount_point` | string | `"database"` | Database secrets engine mount point |
| `auth_method` | string | `"token"` | Authentication method |
| `timeout` | integer | `30` | Request timeout in seconds |
| `vault_credential_cache_enabled` | boolean | `true` | Enable credential caching between sessions |
| `vault_cache_renewal_threshold` | float | `0.25` | Renew when remaining TTL < 25% of original |
| `vault_cache_min_ttl_seconds` | integer | `300` | Minimum TTL required (5 minutes) |

**Example:**
```toml
[vault]
addr = "https://vault.company.com"
mount_point = "database"
auth_method = "userpass"
timeout = 60

# Credential Caching (improves performance)
vault_credential_cache_enabled = true
vault_cache_renewal_threshold = 0.25  # Renew when 25% TTL remaining
vault_cache_min_ttl_seconds = 300     # Require at least 5 minutes TTL
```

**Credential Caching Behavior:**

- **Automatic**: Credentials are cached on first `vault://` connection
- **Persistent**: Cache survives between DBCrust sessions  
- **Secure**: All credentials encrypted with AES-256-GCM using your Vault token
- **Smart Renewal**: Automatically refreshes credentials approaching expiration
- **File Location**: `~/.config/dbcrust/vault_credentials.enc`

**Cache Management Commands:**
- `\vc` - Show cache status and remaining TTL
- `\vcc` - Clear all cached credentials
- `\vcr [role]` - Force refresh credentials
- `\vce` - Show expired credentials

### [security] - Security Settings

SSL/TLS and security-related configuration.

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `verify_ssl` | boolean | `true` | Verify SSL certificates |
| `ssl_cert_path` | string | `""` | Path to client SSL certificate |
| `ssl_key_path` | string | `""` | Path to client SSL key |
| `password_cache_timeout` | integer | `3600` | Password cache timeout (seconds) |

**Example:**
```toml
[security]
verify_ssl = true
ssl_cert_path = "~/.ssl/client.crt"
ssl_key_path = "~/.ssl/client.key"
password_cache_timeout = 1800
```

## 🚀 Environment Variable Overrides

Many configuration options can be overridden with environment variables:

```bash
# Database connection
export DBCRUST_DATABASE_URL="postgres://user@host/db"

# SSH tunnel
export DBCRUST_SSH_TUNNEL="user@jumphost.com:2222"

# Vault configuration
export VAULT_ADDR="https://vault.company.com"
export VAULT_TOKEN="your-token"

# Editor
export EDITOR="code --wait"

# Logging
export DBCRUST_LOG_LEVEL="debug"
```

## 📝 Common Configuration Examples

### Development Environment

```toml
[database]
default_limit = 100
show_execution_time = true
expanded_display_default = false

[display]
border_style = 1
max_column_width = 100

[editor]
command = "code --wait"

[logging]
level = "debug"
```

### Production Environment

```toml
[database]
default_limit = 1000
auto_explain_threshold = 2000

[ssh_tunnel_patterns]
"prod-db\\.company\\.com" = "prod-bastion.company.com"
"staging-db\\.company\\.com" = "staging-bastion.company.com"

[vault]
addr = "https://vault.company.com"
mount_point = "database"
auth_method = "ldap"

[security]
verify_ssl = true
password_cache_timeout = 1800

[logging]
level = "info"
```

### Data Analysis Workflow

```toml
[database]
default_limit = 10000
expanded_display_default = true
show_execution_time = true

[display]
number_format = "human"
max_column_width = 80
truncate_long_values = false

[editor]
command = "jupyter lab"
temp_dir = "~/analysis/queries"

[performance]
query_timeout = 600  # 10 minutes for long analytics queries
```

## 🔧 Configuration Management

### View Current Configuration

```sql
-- Show all current settings
\config

-- Or check specific file
cat ~/.config/dbcrust/config.toml
```

### Reset to Defaults

```bash
# Backup current config
cp ~/.config/dbcrust/config.toml ~/.config/dbcrust/config.toml.backup

# Remove config file (will be recreated with defaults)
rm ~/.config/dbcrust/config.toml

# Start DBCrust to generate new default config
dbcrust --help
```

### Configuration Validation

DBCrust validates configuration on startup and will show warnings for invalid settings:

```
⚠️  Warning: Invalid border_style '3' in config. Using default value '1'.
⚠️  Warning: SSH tunnel pattern '^invalid[regex' is not valid regex.
✅ Configuration loaded successfully.
```

### Hot Reloading

Some configuration changes can be applied without restarting:

```sql
-- Reload configuration
\config reload

-- Or restart connection with new settings
\reconnect
```

## 🎯 Best Practices

### Security

!!! warning "Sensitive Information"
    
    Never store passwords or tokens directly in the config file:
    
    ```toml
    # ❌ Don't do this
    [database]
    password = "secret123"
    
    # ✅ Use environment variables or Vault
    [vault]
    addr = "https://vault.company.com"
    ```

### Performance

!!! tip "Connection Pooling"
    
    Enable connection pooling for frequently accessed databases:
    
    ```toml
    [performance]
    enable_connection_pooling = true
    pool_max_connections = 10
    connection_timeout = 30
    ```

### Team Configurations

!!! info "Shared Settings"
    
    For team environments, consider:
    
    ```toml
    # Share common SSH tunnel patterns
    [ssh_tunnel_patterns]
    "^.*\\.company\\.internal$" = "shared-bastion.company.com"
    
    # Standardize display settings
    [display]
    border_style = 1
    date_format = "%Y-%m-%d %H:%M:%S UTC"
    ```

## 🆘 Troubleshooting

### Common Issues

!!! question "Config file not found"
    
    ```bash
    # Create config directory
    mkdir -p ~/.config/dbcrust
    
    # Generate default config
    dbcrust --init-config
    ```

!!! question "Permission denied"
    
    ```bash
    # Fix permissions
    chmod 755 ~/.config/dbcrust
    chmod 644 ~/.config/dbcrust/config.toml
    ```

!!! question "Invalid TOML syntax"
    
    ```bash
    # Validate TOML syntax
    python -c "import toml; toml.load('~/.config/dbcrust/config.toml')"
    
    # Or use online validator
    echo "Check at: https://www.toml-lint.com/"
    ```

---

<div align="center">
    <strong>Need more configuration help?</strong><br>
    <a href="/dbcrust/reference/url-schemes/" class="md-button md-button--primary">URL Schemes Guide</a>
    <a href="/dbcrust/user-guide/basic-usage/" class="md-button">User Guide</a>
</div>