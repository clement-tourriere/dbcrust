# HashiCorp Vault Integration

DBCrust provides seamless integration with HashiCorp Vault for dynamic database credentials, eliminating the need to store passwords in configuration files or environment variables. This guide covers setup, configuration, and advanced usage patterns.

## 🔐 Why Vault Integration?

Dynamic database credentials provide significant security benefits:
- ✅ **No stored passwords** - Credentials are generated on-demand
- ✅ **Automatic rotation** - Credentials expire and rotate automatically
- ✅ **Audit trail** - All access is logged in Vault
- ✅ **Fine-grained permissions** - Role-based access control
- ✅ **Compliance ready** - Meets enterprise security requirements

## 🚀 Quick Start

### Prerequisites

1. **HashiCorp Vault server** running and accessible
2. **Database secrets engine** configured
3. **Vault authentication** (token, userpass, LDAP, etc.)

### Basic Connection

```bash
# Set Vault environment variables
export VAULT_ADDR="https://vault.company.com"
export VAULT_TOKEN="your-vault-token"

# Connect using Vault URL scheme
dbcrust vault://database-role@database/postgres-prod

# Interactive connection (prompts for role and database)
dbcrust vault://
```

### Vault URL Format

```
vault://[role]@[mount-point]/[database-name]
```

**Components:**
- **`role`** (optional): Vault role name
- **`mount-point`** (optional): Database secrets engine mount point (default: "database")
- **`database-name`** (optional): Database configuration name in Vault

**Examples:**
```bash
# Full specification
dbcrust vault://app-readonly@database/postgres-prod

# Use default mount point
dbcrust vault://app-readonly/postgres-prod

# Interactive selection
dbcrust vault://app-readonly
dbcrust vault://
```

## 🛠️ Configuration

### Environment Variables

```bash
# Required
export VAULT_ADDR="https://vault.company.com"

# Authentication (choose one)
export VAULT_TOKEN="your-token"                    # Token auth
export VAULT_USERNAME="your-username"              # Userpass auth
export VAULT_PASSWORD="your-password"              # Userpass auth

# Optional
export VAULT_NAMESPACE="your-namespace"            # Vault Enterprise
export VAULT_MOUNT_POINT="database"                # Default mount point
export VAULT_SKIP_VERIFY="false"                   # Skip TLS verification
```

### DBCrust Configuration

```toml
# ~/.config/dbcrust/config.toml

[vault]
addr = "https://vault.company.com"
mount_point = "database"              # Default secrets engine mount
auth_method = "token"                 # "token", "userpass", "ldap"
timeout = 30                          # Request timeout in seconds
namespace = ""                        # Vault Enterprise namespace

# Credential Caching (recommended for performance)
vault_credential_cache_enabled = true
vault_cache_renewal_threshold = 0.25  # Renew when 25% TTL remaining
vault_cache_min_ttl_seconds = 300     # Minimum 5 minutes TTL required

# TLS Configuration
tls_skip_verify = false               # Don't skip TLS verification
tls_ca_cert = ""                      # Path to CA certificate
tls_client_cert = ""                  # Path to client certificate
tls_client_key = ""                   # Path to client private key
```

### Authentication Methods

#### Token Authentication

```bash
export VAULT_ADDR="https://vault.company.com"
export VAULT_TOKEN="s.abc123def456..."

dbcrust vault://app-role@database/postgres-main
```

#### Username/Password Authentication

```bash
export VAULT_ADDR="https://vault.company.com"
export VAULT_USERNAME="your-username"
export VAULT_PASSWORD="your-password"

# DBCrust automatically authenticates with Vault
dbcrust vault://app-role@database/postgres-main
```

#### LDAP Authentication

```toml
# ~/.config/dbcrust/config.toml
[vault]
addr = "https://vault.company.com"
auth_method = "ldap"
```

```bash
export VAULT_ADDR="https://vault.company.com"
export VAULT_USERNAME="your-ldap-username"
export VAULT_PASSWORD="your-ldap-password"

dbcrust vault://app-role@database/postgres-main
```

## 🎯 Credential Caching

DBCrust intelligently caches Vault credentials to improve performance and reduce Vault API calls.

### How Caching Works

1. **First connection**: DBCrust fetches credentials from Vault
2. **Encryption**: Credentials encrypted with AES-256-GCM using your Vault token
3. **Storage**: Cached in `~/.config/dbcrust/vault_credentials.enc`
4. **Reuse**: Subsequent connections use cached credentials if still valid
5. **Renewal**: Automatically refreshes when approaching expiration
6. **Security**: Cache is tied to your Vault token - invalid if token changes

### Cache Configuration

```toml
# ~/.config/dbcrust/config.toml
[vault]
vault_credential_cache_enabled = true

# Renew credentials when 25% of TTL remains
vault_cache_renewal_threshold = 0.25

# Only cache credentials with at least 5 minutes TTL
vault_cache_min_ttl_seconds = 300
```

### Cache Management Commands

```bash
# Show cache status
\vc

# Clear all cached credentials
\vcc

# Force refresh specific role credentials
\vcr app-readonly

# Show expired credentials
\vce
```

**Example cache status output:**
```
Vault Credential Cache Status:
=============================

app-readonly@database/postgres-prod:
  ✅ Valid until: 2024-01-15 16:30:00 UTC (45 minutes remaining)
  📍 Database: postgres-prod
  🔄 Will renew at: 2024-01-15 16:18:45 UTC

app-writer@database/postgres-main:
  ⚠️  Expires soon: 2024-01-15 14:45:00 UTC (5 minutes remaining)
  📍 Database: postgres-main
  🔄 Auto-renewal in progress...

Cache file: ~/.config/dbcrust/vault_credentials.enc (2.1 KB)
Encryption: AES-256-GCM
```

## 🏗️ Vault Setup

### Database Secrets Engine

Configure the database secrets engine in Vault:

```bash
# Enable database secrets engine
vault secrets enable database

# Configure PostgreSQL connection
vault write database/config/postgres-prod \
    plugin_name=postgresql-database-plugin \
    connection_url="postgresql://vault@postgres.company.com:5432/postgres?sslmode=require" \
    allowed_roles="app-readonly,app-writer,admin"

# Configure MySQL connection
vault write database/config/mysql-analytics \
    plugin_name=mysql-database-plugin \
    connection_url="{{username}}:{{password}}@tcp(mysql.company.com:3306)/" \
    allowed_roles="analytics-readonly,analytics-writer"
```

### Role Configuration

Create roles with appropriate permissions:

```bash
# Read-only role for application
vault write database/roles/app-readonly \
    db_name=postgres-prod \
    creation_statements="CREATE ROLE \"{{name}}\" WITH LOGIN PASSWORD '{{password}}' VALID UNTIL '{{expiration}}' IN ROLE readonly;" \
    default_ttl="1h" \
    max_ttl="24h"

# Writer role for application
vault write database/roles/app-writer \
    db_name=postgres-prod \
    creation_statements="CREATE ROLE \"{{name}}\" WITH LOGIN PASSWORD '{{password}}' VALID UNTIL '{{expiration}}' IN ROLE app_writer;" \
    default_ttl="2h" \
    max_ttl="8h"

# Admin role for DBAs
vault write database/roles/admin \
    db_name=postgres-prod \
    creation_statements="CREATE ROLE \"{{name}}\" WITH LOGIN PASSWORD '{{password}}' VALID UNTIL '{{expiration}}' SUPERUSER;" \
    default_ttl="30m" \
    max_ttl="2h"
```

### Policy Configuration

Create Vault policies for role access:

```bash
# Create policy for developers
vault policy write developers - <<EOF
# Allow reading credentials for app roles
path "database/creds/app-readonly" {
  capabilities = ["read"]
}

path "database/creds/app-writer" {
  capabilities = ["read"]
}
EOF

# Create policy for DBAs
vault policy write dbas - <<EOF
# Allow reading all database credentials
path "database/creds/*" {
  capabilities = ["read"]
}
EOF
```

## 🎨 Real-World Examples

### Django Application Setup

**Production Django setup with different roles:**

```python
# settings/production.py
import os
from dbcrust import get_vault_credentials

# Get database credentials from Vault
if os.getenv('USE_VAULT', 'false').lower() == 'true':
    db_creds = get_vault_credentials('app-writer', 'database', 'django-prod')

    DATABASES = {
        'default': {
            'ENGINE': 'django.db.backends.postgresql',
            'NAME': 'django_prod',
            'USER': db_creds['username'],
            'PASSWORD': db_creds['password'],
            'HOST': 'postgres.company.com',
            'PORT': '5432',
        }
    }
else:
    # Fallback to environment variables for development
    DATABASES = {
        'default': {
            'ENGINE': 'django.db.backends.postgresql',
            'NAME': os.getenv('DB_NAME', 'django_dev'),
            'USER': os.getenv('DB_USER', 'django'),
            'PASSWORD': os.getenv('DB_PASSWORD'),
            'HOST': os.getenv('DB_HOST', 'localhost'),
            'PORT': os.getenv('DB_PORT', '5432'),
        }
    }
```

**Django management commands:**
```bash
# Connect with appropriate role
export USE_VAULT=true

# Read-only access for queries
python manage.py dbcrust vault://app-readonly@database/django-prod

# Writer access for migrations
python manage.py dbcrust vault://app-writer@database/django-prod

# Admin access for schema changes
python manage.py dbcrust vault://admin@database/django-prod
```

### Multi-Environment Setup

```bash
# Development environment
export VAULT_ADDR="https://vault-dev.company.com"
dbcrust vault://dev-app@database/postgres-dev

# Staging environment
export VAULT_ADDR="https://vault-staging.company.com"
dbcrust vault://staging-app@database/postgres-staging

# Production environment
export VAULT_ADDR="https://vault.company.com"
dbcrust vault://app-readonly@database/postgres-prod
```

### Analytics Workflow

```bash
# Data analysts get read-only access to analytics databases
dbcrust vault://analyst@analytics/data-warehouse

# Data engineers get writer access for ETL
dbcrust vault://etl-writer@analytics/data-warehouse

# Data platform team gets admin access
dbcrust vault://admin@analytics/data-warehouse
```

### Automated Scripts

```python
#!/usr/bin/env python3
import dbcrust
import os

# Set Vault configuration
os.environ['VAULT_ADDR'] = 'https://vault.company.com'
os.environ['VAULT_TOKEN'] = os.environ.get('VAULT_TOKEN')

def run_daily_report():
    """Generate daily analytics report using Vault credentials"""

    # Connect using Vault credentials
    result = dbcrust.run_command(
        "vault://analytics-readonly@database/data-warehouse",
        """
        SELECT
            date_trunc('day', created_at) as day,
            COUNT(*) as orders,
            SUM(amount) as revenue
        FROM orders
        WHERE created_at >= current_date - interval '7 days'
        GROUP BY day
        ORDER BY day;
        """
    )

    # Process results
    print("Daily Revenue Report:")
    print(result)

if __name__ == "__main__":
    run_daily_report()
```

## 🔧 Advanced Configuration

### Custom Mount Points

If your Vault setup uses custom mount points:

```bash
# Multiple database engines
dbcrust vault://app@postgres-engine/main-db
dbcrust vault://app@mysql-engine/analytics-db
dbcrust vault://app@mongodb-engine/logs-db
```

### Vault Namespaces (Enterprise)

For Vault Enterprise with namespaces:

```bash
export VAULT_NAMESPACE="development"
dbcrust vault://app@database/postgres-dev

export VAULT_NAMESPACE="production"
dbcrust vault://app@database/postgres-prod
```

### Custom TTL Configuration

Request specific credential TTLs:

```bash
# Request 4-hour credentials (if role allows)
dbcrust vault://app-writer@database/postgres-prod?ttl=4h

# Request maximum TTL
dbcrust vault://admin@database/postgres-prod?ttl=max
```

### Connection Retry Logic

```toml
# ~/.config/dbcrust/config.toml
[vault]
max_retries = 3
retry_delay = 1000  # milliseconds
backoff_multiplier = 2

# Retry on these HTTP status codes
retry_on_status = [500, 502, 503, 504]
```

## 🚨 Troubleshooting

### Common Issues

#### Vault Connection Problems

```bash
# Test Vault connectivity
curl -s "$VAULT_ADDR/v1/sys/health" | jq

# Test authentication
vault auth -method=userpass username=yourname

# Test role access
vault read database/creds/app-readonly
```

#### Permission Denied

```bash
# Check your Vault token capabilities
vault token lookup

# Check policy attachments
vault token lookup -format=json | jq '.data.policies'

# Test specific path permissions
vault policy read your-policy-name
```

#### Credential Cache Issues

```bash
# Clear cache and retry
\vcc
dbcrust vault://app-readonly@database/postgres-prod

# Check cache file permissions
ls -la ~/.config/dbcrust/vault_credentials.enc

# Enable debug logging
dbcrust --debug vault://app-readonly@database/postgres-prod
```

#### SSL/TLS Problems

```bash
# Skip TLS verification (not recommended for production)
export VAULT_SKIP_VERIFY=true

# Or specify CA certificate
export VAULT_CACERT=/path/to/ca.crt
```

### Debug Mode

Enable detailed Vault debugging:

```bash
# Enable Vault client debug logging
export VAULT_LOG_LEVEL=debug

# Enable DBCrust debug logging
dbcrust --debug vault://app@database/postgres-prod
```

### Health Checks

Monitor Vault integration health:

```bash
# Check Vault server health
\vault health

# Check credential cache status
\vc

# Test credential renewal
\vcr app-readonly
```

## 🛡️ Security Best Practices

### Token Security

```bash
# Use short-lived tokens
vault auth -method=userpass username=yourname

# Renew tokens before expiration
vault token renew

# Use token helpers for secure storage
vault auth -method=aws
```

### Network Security

- Always use HTTPS for Vault connections
- Implement proper firewall rules
- Use VPN or private networks when possible
- Enable Vault audit logging

### Role Design

- Follow principle of least privilege
- Use separate roles for different access levels
- Implement short TTLs for sensitive operations
- Regular role permission audits

### Compliance

```toml
# Enable comprehensive logging for audits
[logging]
level = "info"
file_output = true
file_path = "/var/log/dbcrust/audit.log"

[vault]
log_credential_requests = true
log_cache_operations = true
```

## 🔗 Integration Examples

### CI/CD Pipelines

```yaml
# GitHub Actions
name: Database Tests
on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    env:
      VAULT_ADDR: ${{ secrets.VAULT_ADDR }}
      VAULT_ROLE_ID: ${{ secrets.VAULT_ROLE_ID }}
      VAULT_SECRET_ID: ${{ secrets.VAULT_SECRET_ID }}

    steps:
    - uses: actions/checkout@v2

    - name: Authenticate to Vault
      run: |
        vault write -field=token auth/approle/login \
          role_id="$VAULT_ROLE_ID" \
          secret_id="$VAULT_SECRET_ID" > /tmp/vault-token
        export VAULT_TOKEN=$(cat /tmp/vault-token)

    - name: Run database tests
      run: |
        # Tests automatically use Vault credentials
        python -m pytest tests/database_tests.py

    - name: Database migration check
      run: |
        dbcrust vault://ci-readonly@database/test-db \
          --query "SELECT version()"
```

### Kubernetes Integration

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: django-app
spec:
  template:
    spec:
      serviceAccountName: django-vault-sa
      containers:
      - name: django
        image: django-app:latest
        env:
        - name: VAULT_ADDR
          value: "https://vault.company.com"
        - name: VAULT_ROLE
          value: "django-app"
        - name: DATABASE_URL
          value: "vault://app@database/django-prod"
```

## 📚 See Also

- **[SSH Tunneling](/dbcrust/advanced/ssh-tunneling/)** - Secure database connections
- **[Security Guide](/dbcrust/advanced/security/)** - Complete security practices
- **[Configuration Reference](/dbcrust/reference/configuration-reference/)** - All configuration options
- **[HashiCorp Vault Documentation](https://www.vaultproject.io/docs)** - Official Vault docs

---

<div align="center">
    <strong>Need help with Vault integration?</strong><br>
    <a href="https://github.com/clement-tourriere/dbcrust/issues" class="md-button md-button--primary">Get Support</a>
    <a href="/dbcrust/advanced/security/" class="md-button">Security Guide</a>
</div>
