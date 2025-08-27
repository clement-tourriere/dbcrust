# Security Guide

Security is paramount when working with databases. DBCrust provides comprehensive security features and follows best practices to protect your data and credentials. This guide covers all security aspects from basic connection security to enterprise-grade compliance features.

## 🛡️ Security Architecture

DBCrust implements defense-in-depth security:
- ✅ **Encrypted connections** by default (TLS/SSL)
- ✅ **No credential storage** in plaintext
- ✅ **Dynamic credentials** via HashiCorp Vault
- ✅ **Secure SSH tunneling** for network isolation
- ✅ **Audit logging** for compliance
- ✅ **Key-based authentication** support

## 🔐 Connection Security

### TLS/SSL Encryption

**DBCrust enforces encrypted connections by default:**

```bash
# PostgreSQL with SSL (default behavior)
dbcrust postgres://user:pass@db.company.com/prod
# → Automatically uses sslmode=require

# Explicit SSL configuration
dbcrust postgres://user:pass@db.company.com/prod?sslmode=require
dbcrust postgres://user:pass@db.company.com/prod?sslmode=verify-full

# MySQL with SSL
dbcrust mysql://user:pass@db.company.com/prod?ssl-mode=REQUIRED

# Custom SSL certificates
dbcrust postgres://user:pass@db.company.com/prod \
  --ssl-cert client.crt \
  --ssl-key client.key \
  --ssl-ca ca.crt
```

### SSL Configuration Options

```toml
# ~/.config/dbcrust/config.toml

[security]
# SSL/TLS settings
verify_ssl = true                    # Verify SSL certificates (default: true)
ssl_cert_path = "~/.ssl/client.crt" # Client certificate path
ssl_key_path = "~/.ssl/client.key"  # Client private key path
ssl_ca_path = "~/.ssl/ca.crt"       # Certificate Authority path

# Connection security
require_ssl = true                   # Require SSL for all connections
min_tls_version = "1.2"             # Minimum TLS version
cipher_suites = []                   # Allowed cipher suites (empty = default)

# Timeout settings
connection_timeout = 30              # Connection timeout (seconds)
ssl_handshake_timeout = 10           # SSL handshake timeout (seconds)
```

### Certificate Management

**Enterprise certificate setup:**

```bash
# Generate client certificate (if required)
openssl genrsa -out client.key 2048
openssl req -new -key client.key -out client.csr
# Send CSR to your CA for signing

# Configure DBCrust to use certificates
mkdir -p ~/.config/dbcrust/ssl
cp client.crt client.key ca.crt ~/.config/dbcrust/ssl/
chmod 600 ~/.config/dbcrust/ssl/client.key
```

**Configuration for certificate-based auth:**
```toml
[security]
ssl_cert_path = "~/.config/dbcrust/ssl/client.crt"
ssl_key_path = "~/.config/dbcrust/ssl/client.key"
ssl_ca_path = "~/.config/dbcrust/ssl/ca.crt"
```

## 🔑 Credential Management

### Universal Password Management

DBCrust provides a comprehensive password management system that works across all supported database types through the `.dbcrust` file system:

**Key Security Features:**
- **Machine-Specific Encryption**: Passwords are encrypted with AES-256-GCM using machine-specific keys
- **Cross-Platform Security**: Uses OS-specific identifiers (Linux: machine-id, macOS: Hardware UUID, Windows: Machine GUID)
- **Automatic Password Detection**: System automatically detects encrypted vs plaintext passwords
- **Secure File Permissions**: Unix systems automatically set file permissions to `0600` (owner read/write only)

**File Format:**
```bash
# ~/.dbcrust - Universal password file for all database types
database_type:host:port:database:username:password

# Examples with encryption
postgresql:localhost:5432:myapp:postgres:enc:a1b2c3d4e5f6789...
mysql:db.example.com:3306:webapp:admin:enc:b2c3d4e5f6789...
mongodb:cluster.mongodb.net:27017:analytics:analyst:enc:c3d4e5f6789...
```

**Machine-Specific Key Generation:**
The encryption key is derived from multiple machine-specific identifiers:

=== "Linux"
    - `/etc/machine-id` or `/var/lib/dbus/machine-id`
    - User home directory path
    - Hostname and username
    - User ID (UID)

=== "macOS"
    - IOKit Hardware UUID via `ioreg`
    - User home directory path
    - Hostname and username

=== "Windows"
    - Machine GUID via PowerShell/WMI
    - User home directory path
    - Hostname and USERNAME

**Security Benefits:**
- Encrypted passwords only work on the machine where they were created
- Cannot be copied to other machines or users
- Incorporates user identity into encryption key
- Uses fixed salt to prevent rainbow table attacks
- Memory-safe password handling

### Legacy Credential Storage

DBCrust also supports traditional credential storage methods:

```toml
# ❌ NEVER do this - passwords not stored in config
[database]
# password = "secret123"  # This is not supported

# ✅ Use these secure methods instead:
# 1. Universal .dbcrust file (recommended)
# 2. Environment variables
# 3. Database-specific files (.pgpass/.my.cnf)
# 3. HashiCorp Vault
# 4. SSH key authentication
```

### Environment Variable Security

```bash
# Secure environment variable handling
export DATABASE_PASSWORD="$(security find-generic-password -s dbcrust -a postgres -w)"

# Or use a secrets manager
export DATABASE_PASSWORD="$(aws secretsmanager get-secret-value --secret-id prod/db/password --query SecretString --output text)"

# Connect without exposing password
dbcrust postgres://user:$DATABASE_PASSWORD@db.company.com/prod
```

### Database-Specific Credential Stores

**PostgreSQL (.pgpass):**
```bash
# ~/.pgpass (permissions: 0600)
hostname:port:database:username:password
db.company.com:5432:prod:app_user:secret123
*.company.com:5432:*:readonly:readonly_pass

# DBCrust automatically uses .pgpass
dbcrust postgres://app_user@db.company.com:5432/prod
```

**MySQL (.my.cnf):**
```bash
# ~/.my.cnf (permissions: 0600)
[client]
host=db.company.com
port=3306
user=app_user
password=secret123

# DBCrust automatically uses .my.cnf
dbcrust mysql://app_user@db.company.com:3306/prod
```

**SQLite (file permissions):**
```bash
# Secure SQLite file permissions
chmod 600 /path/to/database.db
chmod 700 /path/to/database/

# Connect with proper permissions
dbcrust sqlite:///path/to/database.db
```

## 🏢 Enterprise Security Features

### HashiCorp Vault Integration

**Dynamic credentials with automatic rotation:**

```bash
# Vault-based authentication
export VAULT_ADDR="https://vault.company.com"
export VAULT_TOKEN="your-vault-token"

# Connect with dynamic credentials
dbcrust vault://app-readonly@database/postgres-prod

# Credentials are automatically:
# - Generated on demand
# - Rotated periodically
# - Revoked when expired
# - Audit logged in Vault
```

**Vault security configuration:**
```toml
# ~/.config/dbcrust/config.toml

[vault]
addr = "https://vault.company.com"
tls_skip_verify = false              # Always verify TLS
tls_ca_cert = "/etc/ssl/vault-ca.crt"
max_retries = 3
timeout = 30

# Credential caching security
vault_credential_cache_enabled = true
vault_cache_encryption = "aes-256-gcm"  # Strong encryption
vault_cache_file_permissions = 0600     # Secure file permissions
```

### SSH Tunnel Security

**Secure database access through SSH:**

```bash
# SSH key-based authentication
dbcrust postgres://user@internal-db.company.com/prod \
  --ssh-tunnel admin@jumphost.company.com \
  --ssh-key ~/.ssh/production_key

# Multi-hop SSH (even more secure)
dbcrust postgres://user@deep-internal-db/prod \
  --ssh-tunnel multi-hop-bastion
```

**SSH security configuration:**
```bash
# ~/.ssh/config
Host production-bastion
    HostName bastion.company.com
    User dbaccess
    Port 2222
    IdentityFile ~/.ssh/production_ed25519
    IdentitiesOnly yes
    ServerAliveInterval 60
    ServerAliveCountMax 3

    # Security hardening
    Protocol 2
    Ciphers chacha20-poly1305@openssh.com,aes256-gcm@openssh.com
    MACs hmac-sha2-256-etm@openssh.com,hmac-sha2-512-etm@openssh.com
    KexAlgorithms curve25519-sha256@libssh.org,diffie-hellman-group16-sha512
```

### Audit Logging

**Comprehensive audit trail:**

```toml
# ~/.config/dbcrust/config.toml

[logging]
level = "info"                       # Log all significant events
file_output = true
file_path = "/var/log/dbcrust/audit.log"
max_file_size = "100MB"
max_files = 10
format = "json"                      # Structured logging

# Security event logging
[security.audit]
log_connections = true               # Log all database connections
log_queries = false                  # Don't log query content (privacy)
log_failed_auth = true               # Log authentication failures
log_privilege_escalation = true     # Log role/permission changes
log_ssl_errors = true                # Log SSL/TLS errors
log_ssh_tunnel_events = true        # Log SSH tunnel creation/destruction

# Vault audit logging
[vault.audit]
log_credential_requests = true       # Log Vault credential requests
log_token_renewals = true            # Log token renewal events
log_cache_operations = true          # Log credential cache operations
```

**Audit log format:**
```json
{
  "timestamp": "2024-01-15T14:30:00.123Z",
  "level": "INFO",
  "event": "database_connection",
  "user": "admin",
  "source_ip": "192.168.1.100",
  "database": {
    "type": "postgresql",
    "host": "prod-db.company.com",
    "database": "myapp",
    "ssl_used": true
  },
  "auth_method": "vault",
  "vault_role": "app-readonly",
  "duration_ms": 1234,
  "success": true
}
```

## 🎯 Access Control

### Role-Based Database Access

**PostgreSQL role-based security:**

```sql
-- Create read-only role
CREATE ROLE app_readonly;
GRANT CONNECT ON DATABASE myapp TO app_readonly;
GRANT USAGE ON SCHEMA public TO app_readonly;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO app_readonly;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON TABLES TO app_readonly;

-- Create application writer role
CREATE ROLE app_writer;
GRANT app_readonly TO app_writer;  -- Inherit read permissions
GRANT INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO app_writer;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT INSERT, UPDATE, DELETE ON TABLES TO app_writer;

-- Create users with roles
CREATE USER app_prod_readonly WITH PASSWORD 'secure_password' IN ROLE app_readonly;
CREATE USER app_prod_writer WITH PASSWORD 'secure_password' IN ROLE app_writer;
```

**Connect with appropriate role:**
```bash
# Development queries (read-only)
dbcrust postgres://app_prod_readonly@prod-db.company.com/myapp

# Application operations (read-write)
dbcrust postgres://app_prod_writer@prod-db.company.com/myapp

# Administrative tasks (separate admin credentials)
dbcrust vault://dba-admin@database/postgres-prod
```

### Principle of Least Privilege

**Grant minimum required permissions:**

```sql
-- Application user - only specific tables
GRANT SELECT, INSERT, UPDATE ON users, orders, products TO app_user;
GRANT SELECT ON analytics_views TO app_user;

-- Analytics user - read-only on specific schemas
GRANT USAGE ON SCHEMA analytics TO analytics_user;
GRANT SELECT ON ALL TABLES IN SCHEMA analytics TO analytics_user;

-- Backup user - specific permissions
GRANT SELECT ON ALL TABLES IN SCHEMA public TO backup_user;
GRANT USAGE ON SCHEMA public TO backup_user;
```

### Session-Based Security

**Configure secure session parameters:**

```toml
# ~/.config/dbcrust/config.toml

[security]
# Session timeouts
session_timeout = 3600               # 1 hour session timeout
idle_timeout = 1800                  # 30 minutes idle timeout
max_session_duration = 28800         # 8 hours maximum session

# Query restrictions
max_query_duration = 300             # 5 minutes max query time
query_result_limit = 10000           # Maximum result rows
memory_limit = "1GB"                 # Query memory limit

# Dangerous command restrictions
disable_drop_statements = true       # Block DROP commands
disable_truncate_statements = true   # Block TRUNCATE commands
require_confirmation_for_deletes = true  # Confirm DELETE operations
```

## 🚨 Security Monitoring

### Threat Detection

**Monitor for suspicious activity:**

```toml
# ~/.config/dbcrust/config.toml

[security.monitoring]
# Failed authentication detection
max_failed_auth_attempts = 5
failed_auth_window = 300             # 5 minutes
failed_auth_action = "block"         # block, warn, log

# Anomaly detection
detect_unusual_query_patterns = true
detect_off_hours_access = true
detect_new_ip_addresses = true
detect_privilege_escalation = true

# Rate limiting
max_queries_per_minute = 100
max_connections_per_hour = 10
burst_connection_limit = 3
```

### Security Alerts

**Configure security alerts:**

```toml
[security.alerts]
# Alert mechanisms
email_alerts = true
slack_webhook = "https://hooks.slack.com/services/YOUR/WEBHOOK/URL"
syslog_alerts = true

# Alert conditions
alert_on_failed_auth = true
alert_on_privilege_escalation = true
alert_on_suspicious_queries = true
alert_on_ssl_errors = true
alert_on_vault_errors = true
```

### Compliance Features

**Meet regulatory requirements:**

```toml
[compliance]
# Data governance
log_all_data_access = true          # Log all data access for audit
encrypt_logs = true                  # Encrypt audit logs
log_retention_days = 2555            # 7 years retention

# Privacy protection
mask_sensitive_data_in_logs = true  # Mask PII in logs
anonymize_user_identifiers = true   # Anonymize user IDs in logs

# Regulatory frameworks
gdpr_compliance = true               # GDPR-specific features
hipaa_compliance = true              # HIPAA-specific features
sox_compliance = true                # SOX-specific features
```

## 🔧 Security Hardening

### Network Security

**Secure network configuration:**

```toml
# ~/.config/dbcrust/config.toml

[network]
# Allowed source networks (CIDR notation)
allowed_networks = [
    "10.0.0.0/8",        # Internal networks only
    "192.168.0.0/16",    # Local networks
    "172.16.0.0/12"      # Private networks
]

# Denied networks
denied_networks = [
    "0.0.0.0/0"          # Block all external by default
]

# DNS security
use_secure_dns = true
dns_servers = ["1.1.1.1", "8.8.8.8"]  # Use trusted DNS
```

### File System Security

**Secure configuration and data files:**

```bash
# Set secure permissions on DBCrust directory
chmod 700 ~/.config/dbcrust/
chmod 600 ~/.config/dbcrust/config.toml
chmod 600 ~/.config/dbcrust/vault_credentials.enc

# Secure SSH keys
chmod 700 ~/.ssh/
chmod 600 ~/.ssh/production_*
chmod 644 ~/.ssh/config

# Secure database credential files
chmod 600 ~/.pgpass
chmod 600 ~/.my.cnf
```

### Binary Security

**Verify DBCrust binary integrity:**

```bash
# Verify checksums (when available)
curl -fsSL https://releases.dbcrust.com/checksums.txt | grep dbcrust-linux-x64
sha256sum dbcrust-linux-x64

# Use package managers for verified installs
uv tool install dbcrust  # Verified PyPI package
```

## 🏭 Production Security Patterns

### Multi-Environment Security

**Separate credentials per environment:**

```bash
# Development
export VAULT_ADDR="https://vault-dev.company.com"
dbcrust vault://dev-app@database/postgres-dev

# Staging
export VAULT_ADDR="https://vault-staging.company.com"
dbcrust vault://staging-app@database/postgres-staging

# Production
export VAULT_ADDR="https://vault-prod.company.com"
dbcrust vault://prod-app-readonly@database/postgres-prod
```

### Zero-Trust Database Access

**Implement zero-trust principles:**

```toml
# ~/.config/dbcrust/config.toml

[security.zero_trust]
# Always verify identity
require_mfa = true                   # Multi-factor authentication
verify_device_identity = true       # Device verification
require_certificate_auth = true     # Certificate-based auth

# Never trust network location
ignore_source_ip_whitelist = false  # Still check IP restrictions
require_vpn = true                   # VPN required for access
encrypt_all_traffic = true          # End-to-end encryption

# Continuously validate
revalidate_credentials = 300         # Revalidate every 5 minutes
monitor_behavior = true              # Behavioral monitoring
log_everything = true                # Comprehensive logging
```

### Incident Response

**Security incident procedures:**

```bash
# Emergency credential rotation
dbcrust vault://admin@database/postgres-prod \
  --query "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename = 'compromised_user';"

# Audit trail analysis
grep "failed_auth" /var/log/dbcrust/audit.log | tail -100

# Lock down access immediately
echo "security.emergency_lockdown = true" >> ~/.config/dbcrust/config.toml
```

## 🧪 Security Testing

### Penetration Testing

**Test DBCrust security:**

```bash
# Test SQL injection resistance
dbcrust postgres://test@localhost/test \
  --query "SELECT * FROM users WHERE id = '1; DROP TABLE users; --'"

# Test authentication bypass attempts
dbcrust postgres://admin:wrongpass@localhost/test

# Test privilege escalation
dbcrust postgres://readonly@localhost/test \
  --query "CREATE USER hacker WITH SUPERUSER PASSWORD 'hack';"
```

### Security Scanning

**Integrate with security tools:**

```bash
# Scan for secrets in configuration
truffleHog ~/.config/dbcrust/

# Check for insecure permissions
find ~/.config/dbcrust/ -type f -perm +044

# Audit SSL configuration
testssl.sh db.company.com:5432
```

### Compliance Testing

**Automated compliance checks:**

```python
#!/usr/bin/env python3
"""DBCrust security compliance checker"""

import subprocess
import json
import os

def check_ssl_enforcement():
    """Verify SSL is required for connections"""
    config_path = os.path.expanduser("~/.config/dbcrust/config.toml")
    with open(config_path) as f:
        content = f.read()
        return "verify_ssl = true" in content

def check_audit_logging():
    """Verify audit logging is enabled"""
    # Check if audit logs are being written
    log_path = "/var/log/dbcrust/audit.log"
    return os.path.exists(log_path) and os.path.getsize(log_path) > 0

def check_credential_security():
    """Verify no plaintext passwords in config"""
    config_path = os.path.expanduser("~/.config/dbcrust/config.toml")
    with open(config_path) as f:
        content = f.read()
        # Should not contain password fields
        return "password =" not in content

def run_compliance_check():
    """Run full compliance check"""
    checks = {
        "ssl_enforcement": check_ssl_enforcement(),
        "audit_logging": check_audit_logging(),
        "credential_security": check_credential_security()
    }

    print(json.dumps(checks, indent=2))
    return all(checks.values())

if __name__ == "__main__":
    success = run_compliance_check()
    exit(0 if success else 1)
```

## 📚 See Also

- **[SSH Tunneling](/dbcrust/advanced/ssh-tunneling/)** - Secure network access
- **[Vault Integration](/dbcrust/advanced/vault-integration/)** - Dynamic credentials
- **[Docker Integration](/dbcrust/advanced/docker-integration/)** - Container security
- **[Configuration Reference](/dbcrust/reference/configuration-reference/)** - Complete settings

---

<div align="center">
    <strong>Questions about DBCrust security?</strong><br>
    <a href="https://github.com/clement-tourriere/dbcrust/issues" class="md-button md-button--primary">Security Issues</a>
    <a href="/dbcrust/advanced/vault-integration/" class="md-button">Vault Guide</a>
</div>
