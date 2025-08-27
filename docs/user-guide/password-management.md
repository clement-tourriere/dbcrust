# Password Management

DBCrust provides comprehensive password management for all supported database types through the universal `.dbcrust` file system. This system works like PostgreSQL's `.pgpass` but extends support to all databases with automatic encryption and cross-platform compatibility.

## 🔐 Overview

The password management system provides:

- **Universal Support**: Works with PostgreSQL, MySQL, MongoDB, Elasticsearch, ClickHouse, SQLite
- **Automatic Lookup**: Searches for passwords when connecting without credentials
- **Authentication Retry**: Prompts for password on auth failure and offers to save it
- **Secure Encryption**: Machine-specific encryption ensures passwords only work on the machine where they were saved
- **Cross-Platform**: Works on Linux, macOS, and Windows with platform-specific security features

## 📁 Password File Format

The `.dbcrust` file uses the format:
```
database_type:host:port:database:username:password
```

### Examples

```bash
# PostgreSQL - plaintext password
postgresql:localhost:5432:myapp:dbuser:mypassword

# MySQL - encrypted password (prefixed with 'enc:')
mysql:db.example.com:3306:webapp:admin:enc:a1b2c3d4e5f6789...

# MongoDB cluster
mongodb:cluster.mongodb.net:27017:analytics:analyst:secret123

# Wildcard matching - any host, port, user for specific database
postgresql:*:*:testdb:*:development_password

# Elasticsearch with custom port
elasticsearch:es.cluster.com:9200:logs:elastic_user:elastic_pass

# ClickHouse
clickhouse:ch.example.com:8123:analytics:analyst:ch_password
```

### Wildcard Support

Use `*` to match any value for that field:

```bash
# Match any host/port for testdb database
postgresql:*:*:testdb:testuser:testpass

# Match any database for specific user on host
mysql:localhost:*:*:admin:admin_pass

# Match any user on localhost PostgreSQL
postgresql:localhost:5432:myapp:*:shared_password
```

## 🗂️ File Location

| Platform | Default Location | Custom Location |
|----------|------------------|-----------------|
| **Linux/macOS** | `~/.dbcrust` | `$DBCRUST_PASSFILE` |
| **Windows** | `%USERPROFILE%\.dbcrust` | `%DBCRUST_PASSFILE%` |

### Security

- **Unix/macOS**: File permissions automatically set to `0600` (readable/writable by owner only)
- **Windows**: Relies on NTFS directory security and file system permissions

## 🔄 Automatic Password Flow

DBCrust automatically manages passwords through this flow:

```mermaid
graph TD
    A[Connect to Database] --> B{Password in URL?}
    B -->|Yes| C[Use Provided Password]
    B -->|No| D[Lookup in .dbcrust]
    D --> E{Password Found?}
    E -->|Yes| F[Try Connection with Found Password]
    E -->|No| G[Prompt for Password]
    F --> H{Connection Success?}
    H -->|Yes| I[Connected Successfully]
    H -->|No| G
    C --> J{Connection Success?}
    J -->|Yes| I
    J -->|No| G
    G --> K[Try Connection with Entered Password]
    K --> L{Connection Success?}
    L -->|Yes| M[Save Password to .dbcrust?]
    L -->|No| N[Authentication Failed]
    M --> O[Password Saved Encrypted]
    O --> I
```

### Example Flow

```bash
# 1. Connect without password
❯ dbcrust postgres://postgres@localhost:5432/postgres

# 2. Authentication fails, automatic prompt appears
🔐 Password: [hidden input]

# 3. Password accepted, automatically saved
✅ Password saved to .dbcrust file (encrypted)
✓ Successfully connected to database

# 4. Next time: automatic connection using saved password
❯ dbcrust postgres://postgres@localhost:5432/postgres
✓ Successfully connected to database  # Uses saved password
```

## 🛠️ Interactive Commands

### `\savepass` - Save Password

Interactively save a password to the `.dbcrust` file:

```sql
postgres@myapp=> \savepass
Database type: postgresql
Host: localhost
Port: 5432
Database name: myapp
Username: postgres
Password: [hidden input]
Password saved for postgresql:postgres@localhost:5432/myapp (encrypted)
```

**Features:**
- Interactive prompts for all connection details
- Default values provided based on common database ports
- Passwords automatically encrypted
- Updates existing entries if found

### `\listpass` - List Stored Passwords

Display all stored credentials without showing the actual passwords:

```sql
postgres@myapp=> \listpass
Saved passwords:
  postgresql:postgres@localhost:5432/myapp
  mysql:admin@db.example.com:3306/webapp
  mongodb:analyst@cluster.mongodb.net:27017/analytics
  elasticsearch:elastic@es.cluster.com:9200/logs
```

### `\deletepass` - Delete Password

Interactively select and remove a stored password:

```sql
postgres@myapp=> \deletepass
Select password to delete:
  1. postgresql:postgres@localhost:5432/myapp
  2. mysql:admin@db.example.com:3306/webapp
  3. mongodb:analyst@cluster.mongodb.net:27017/analytics

Selection: 2
Password deleted: mysql:admin@db.example.com:3306/webapp
```

### `\encryptpass` - Encrypt Passwords

Convert all plaintext passwords in the `.dbcrust` file to encrypted format:

```sql
postgres@myapp=> \encryptpass
Encrypted 3 password(s) in .dbcrust file.
```

**When to use:**
- After manually editing the `.dbcrust` file with plaintext passwords
- Migrating from an old system
- Converting existing plaintext entries to encrypted format

## 🔒 Encryption Details

### Machine-Specific Keys

Passwords are encrypted using AES-256-GCM with machine-specific keys derived from:

=== "Linux"
    - Machine ID from `/etc/machine-id` or `/var/lib/dbus/machine-id`
    - User home directory path
    - Hostname, username, user ID
    - Fixed salt for additional security

=== "macOS"
    - Hardware UUID via `ioreg` or `system_profiler`
    - User home directory path
    - Hostname, username
    - Fixed salt for additional security

=== "Windows"
    - Machine GUID via PowerShell or WMI
    - User home directory path
    - Hostname, USERNAME environment variable
    - Fixed salt for additional security

### Security Features

- **Machine-Specific**: Encrypted passwords only work on the machine where they were created
- **User-Specific**: Incorporates user identity into the encryption key
- **Salt Protection**: Fixed salt prevents rainbow table attacks
- **Memory Safety**: Passwords cleared from memory after use
- **Auto-Detection**: System automatically detects encrypted vs plaintext passwords

### Password Format

```bash
# Plaintext password
postgresql:localhost:5432:myapp:user:mypassword

# Encrypted password (prefixed with 'enc:')
postgresql:localhost:5432:myapp:user:enc:a1b2c3d4e5f6789abcdef...
```

## 🌐 Cross-Platform Usage

### Linux Example

```bash
# Create .dbcrust file
echo "postgresql:localhost:5432:myapp:postgres:mypass" > ~/.dbcrust
chmod 600 ~/.dbcrust

# Connect automatically
dbcrust postgres://postgres@localhost:5432/myapp
```

### macOS Example

```bash
# Same as Linux
echo "postgresql:localhost:5432:myapp:postgres:mypass" > ~/.dbcrust
chmod 600 ~/.dbcrust

# Connect automatically
dbcrust postgres://postgres@localhost:5432/myapp
```

### Windows Example

```powershell
# Create .dbcrust file in user profile
Set-Content -Path "$env:USERPROFILE\.dbcrust" -Value "postgresql:localhost:5432:myapp:postgres:mypass"

# Connect automatically
dbcrust postgres://postgres@localhost:5432/myapp
```

## 🔧 Advanced Configuration

### Custom Password File Location

Set the `DBCRUST_PASSFILE` environment variable to use a custom location:

=== "Linux/macOS"
    ```bash
    export DBCRUST_PASSFILE="/path/to/my/custom/passwords"
    dbcrust postgres://postgres@localhost:5432/myapp
    ```

=== "Windows"
    ```powershell
    $env:DBCRUST_PASSFILE = "C:\path\to\my\custom\passwords"
    dbcrust postgres://postgres@localhost:5432/myapp
    ```

### Escaping Special Characters

If your credentials contain colons (`:`) or backslashes (`\`), escape them:

```bash
# Host with colon in name
postgresql:host\\:with\\:colons:5432:db:user:pass

# Password with colon
postgresql:localhost:5432:db:user:pass\\:with\\:colon

# Database name with backslash
postgresql:localhost:5432:db\\name:user:password
```

### Team Workflows

For team environments, consider these patterns:

1. **Individual Password Files**: Each team member maintains their own `.dbcrust`
2. **Shared Credentials**: Use environment variables for shared database credentials
3. **Role-Based Access**: Different `.dbcrust` entries for different database roles

```bash
# Example team setup
postgresql:prod.example.com:5432:myapp:readonly:readonly_pass
postgresql:prod.example.com:5432:myapp:admin:admin_pass
postgresql:staging.example.com:5432:myapp:developer:dev_pass
```

## 🐛 Troubleshooting

### Common Issues

#### Password Not Found
```bash
# Check file exists and has correct permissions
ls -la ~/.dbcrust

# Verify file format
cat ~/.dbcrust

# Check for typos in connection parameters
```

#### Permission Denied (Unix/macOS)
```bash
# Fix file permissions
chmod 600 ~/.dbcrust
```

#### Encryption/Decryption Errors
```bash
# Usually indicates the password was encrypted on a different machine
# Re-save the password on the current machine:
dbcrust postgres://postgres@localhost:5432/myapp
# Enter password when prompted - it will be re-encrypted for this machine
```

#### Connection Still Fails
```bash
# Verify the stored credentials work manually
dbcrust postgres://username:password@host:port/database

# Check if authentication error detection is working
dbcrust postgres://wronguser@localhost:5432/postgres
# Should prompt for password
```

### Debug Mode

Enable debug logging to troubleshoot password lookup:

```toml
# In ~/.config/dbcrust/config.toml
[logging]
level = "debug"
console_output = true
```

Then check the logs for password lookup activity:
```
DEBUG [dbcrust] Looking up password in .dbcrust file
DEBUG [dbcrust] Found password in .dbcrust file
DEBUG [dbcrust] Connection successful with looked-up password
```

## 🔗 Related Features

- **[Vault Integration](../advanced/vault-integration.md)**: For dynamic database credentials
- **[Session Management](../user-guide/advanced-features.md#session-management)**: Save complete connection configurations
- **[Security](../advanced/security.md)**: Overall security best practices
- **[Configuration](../configuration.md)**: General configuration options
