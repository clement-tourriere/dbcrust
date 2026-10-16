---
title: "Password Management"
---

# Password Management

DBCrust remembers database passwords for all supported database types. Saved passwords live in the **secret store** (OS keychain, or a passphrase-encrypted file on Linux) — see [Secret Storage](secret-storage.md). The older universal `.dbcrust` file is still **read** (and can be migrated with `dbcrust secrets migrate`) but DBCrust never writes to it or encrypts it any more.

## 🔐 Overview

The password management system provides:

- **Universal Support**: Works with PostgreSQL, MySQL, MongoDB, Elasticsearch, ClickHouse
- **Automatic Lookup**: Searches for passwords when connecting without credentials: the secret store first, then the legacy `.dbcrust` file, then `.pgpass` / `.my.cnf`
- **Authentication Retry**: Prompts for password on auth failure and saves it to the secret store
- **Target-bound**: A stored password is only used for the exact engine, host, port, database and user it was saved for
- **Cross-Platform**: macOS Keychain, Windows Credential Manager, Argon2id passphrase file on Linux

## 📁 Legacy `.dbcrust` File Format (read-only)

Older versions saved passwords in `~/.dbcrust`. DBCrust still reads it as a fallback; new passwords go to the secret store instead. Run `dbcrust secrets migrate` to move existing entries. The file uses the format:
```
database_type:host:port:database:username:password
```

### Examples

```bash
# PostgreSQL - plaintext password
postgresql:localhost:5432:myapp:dbuser:mypassword

# MySQL - legacy machine-key value (prefixed with 'enc:'; readable, never written)
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

Use `*` to match any value for that field. Wildcards are a legacy feature: `dbcrust secrets migrate` never converts them automatically (a wildcard would hand one password to every matching target); it lets you bind one to a concrete target instead.

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

- **Unix/macOS**: A legacy file readable by other users is refused; keep it at `0600`
- **Windows**: Relies on NTFS directory security and file system permissions
- `dbcrust secrets audit` reports plaintext and `enc:` lines and loose permissions

## 🔄 Automatic Password Flow

DBCrust automatically manages passwords through this flow:

```mermaid
graph TD
    A[Connect to Database] --> B{Password in URL?}
    B -->|Yes| C[Use Provided Password]
    B -->|No| D[Lookup: secret store, then legacy .dbcrust, then pgpass / my.cnf]
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
    L -->|Yes| M[Save password to the secret store]
    L -->|No| N[Authentication Failed]
    M --> O[Password stored, bound to the target]
    O --> I
```

### Example Flow

```bash
# 1. Connect without password
❯ dbcrust postgres://postgres@localhost:5432/postgres

# 2. Authentication fails, automatic prompt appears
🔐 Password: [hidden input]

# 3. Password accepted, automatically saved to the secret store
✅ Password saved to the secure store
✓ Successfully connected to database

# 4. Next time: automatic connection using saved password
❯ dbcrust postgres://postgres@localhost:5432/postgres
✓ Successfully connected to database  # Uses saved password
```

## Saved Sessions with Dynamic Password Commands

Saved sessions never store password values in `sessions.toml`, but they can store a command that prints the password to stdout. This is useful for Vault, cloud CLIs, password managers, and other rotating-secret workflows.

```sql
\ss chpprd --password-command vault kv get -mount=secret -field=password preprod/gim/admin/clickhouse/default-user
```

Then reconnect normally:

```bash
dbc session://chpprd
```

DBCrust runs the command only during reconnection, trims the trailing newline, injects the result as the password, and does not persist the command output. Only configure commands you trust in `sessions.toml`.

You can also edit `~/.config/dbcrust/sessions.toml` directly:

```toml
[sessions.chpprd]
host = "hh8mmc7lqc.us-west-2.aws.clickhouse.cloud"
port = 8443
user = "default"
dbname = "gim"
database_type = "ClickHouse"

[sessions.chpprd.options]
secure = "true"
password_command = "vault kv get -mount=secret -field=password preprod/gim/admin/clickhouse/default-user"
```

## 🛠️ Interactive Commands

### `\savepass` - Save Password

Interactively save a password to the secret store:

```sql
postgres@myapp=> \savepass
Database type: postgresql
Host: localhost
Port: 5432
Database name: myapp
Username: postgres
Password: [hidden input]
Password saved for postgresql:postgres@localhost:5432/myapp (secure store)
```

**Features:**
- Interactive prompts for all connection details
- Default values provided based on common database ports
- The password is stored in the OS keychain / encrypted file, bound to this exact target
- Updates the existing entry if found

### `\listpass` - List Stored Passwords

Display all stored credentials without showing the actual passwords. Entries still in the legacy `.dbcrust` file are marked `(legacy)`:

```sql
postgres@myapp=> \listpass
Saved passwords:
  postgresql:postgres@localhost:5432/myapp
  mysql:admin@db.example.com:3306/webapp
  mongodb:analyst@cluster.mongodb.net:27017/analytics (legacy)

(legacy) entries are still in ~/.dbcrust — run `dbcrust secrets migrate` to move them into the secure store.
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

### `\encryptpass` - Deprecated

DBCrust no longer encrypts `.dbcrust`: its machine-derived key could be recomputed by any process of your user, so it protected nothing. The command now just points to the replacement:

```sql
postgres@myapp=> \encryptpass
DBCrust no longer encrypts ~/.dbcrust ... Run `dbcrust secrets migrate` to move existing entries
```

## 🔒 Security Model

Saved passwords are protected by the [secret store](secret-storage.md), not by a key derived from your machine:

- **macOS / Windows**: the OS keychain (service `dbcrust.store`).
- **Linux**: `secrets.enc`, AES-256-GCM under an Argon2id key derived from a passphrase only you know; the unlocked key is cached in the kernel session keyring until you log out (`dbcrust secrets unlock` / `lock`).
- **Bound to the target**: the record stores engine, host, port, database, user and TLS mode, and is refused for any other target.
- **Never plaintext, never machine-derived**: legacy `enc:` values are only ever decrypted to be migrated.
- **Memory**: secret values are zeroized after use.

See [Secret Storage](secret-storage.md) for the threat model, `dbcrust secrets audit` and the migration walk-through.

## 🌐 Cross-Platform Usage

### Linux

```bash
# First connect: prompts for the DB password, then (once) for a passphrase
# that protects ~/.config/dbcrust/secrets.enc
dbcrust postgres://postgres@localhost:5432/myapp

# Later runs: unlock once per login session, then connect without prompts
dbcrust secrets unlock
dbcrust -c 'select 1' postgres://postgres@localhost:5432/myapp
```

### macOS and Windows

```bash
# First connect saves the password in the OS keychain / Credential Manager
dbcrust postgres://postgres@localhost:5432/myapp
# Later runs connect without prompting
dbcrust postgres://postgres@localhost:5432/myapp
```

## 🔧 Advanced Configuration

### Custom Legacy Password File Location

Set the `DBCRUST_PASSFILE` environment variable to point the (read-only) legacy lookup and `dbcrust secrets migrate` at another file:


**Linux/macOS**

```bash
export DBCRUST_PASSFILE="/path/to/my/custom/passwords"
dbcrust postgres://postgres@localhost:5432/myapp
```


**Windows**

```powershell
$env:DBCRUST_PASSFILE = "C:\path\to\my\custom\passwords"
dbcrust postgres://postgres@localhost:5432/myapp
```

### Escaping Special Characters (legacy file)

If credentials in a legacy `.dbcrust` file contain colons (`:`) or backslashes (`\`), escape them:

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

1. **Individual stores**: every team member keeps their own secret store; nothing secret is shared through git.
2. **Shared credentials**: use environment variables, Vault (`vault://`) or a `password_command` for shared database credentials.
3. **Role-based access**: save one password per database role (the store keeps one record per user/host/port/database).

## 🐛 Troubleshooting

### Common Issues

#### Password Not Found
```bash
# Where do saved secrets live, and is the store locked?
dbcrust secrets status

# The stored password is bound to the exact host, port, database and user
# it was saved for: a different host (or user) prompts again.
```

#### Secret storage is locked
```bash
# Linux file backend: enter the passphrase once for this login session
dbcrust secrets unlock
# One-shot (-c/-f), --no-input and the desktop app never prompt: they report "locked"
```

#### Forgot the passphrase
```bash
# secrets.enc cannot be recovered. Remove it and save your passwords again:
rm ~/.config/dbcrust/secrets.enc ~/.config/dbcrust/secrets_index.toml
```

#### Legacy `enc:` value cannot be decrypted
```bash
# It was encrypted on another machine (or with another hostname/user).
# `dbcrust secrets migrate` keeps such lines untouched; re-enter the password:
dbcrust postgres://postgres@localhost:5432/myapp
```

#### Permission Denied on the legacy file (Unix/macOS)
```bash
chmod 600 ~/.dbcrust
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
DEBUG [dbcrust] Looking up a saved password
DEBUG [dbcrust] Found a saved password
DEBUG [dbcrust] Connection successful with looked-up password
```

## 🔗 Related Features

- **[Secret Storage](secret-storage.md)**: Where passwords live, `dbcrust secrets status|audit|migrate|unlock|lock`
- **[Vault Integration](../advanced/vault-integration.md)**: For dynamic database credentials
- **[Session Management](../user-guide/advanced-features.md#session-management)**: Save complete connection configurations
- **[Security](../advanced/security.md)**: Overall security best practices
- **[Configuration](../configuration.md)**: General configuration options
