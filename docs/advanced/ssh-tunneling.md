# SSH Tunneling Guide

DBCrust provides powerful SSH tunneling capabilities that make connecting to databases behind firewalls and in secure environments seamless. This guide covers everything from basic setup to advanced configurations.

## 🔒 Why SSH Tunneling?

SSH tunneling allows you to securely connect to databases that are:
- Behind corporate firewalls
- In private networks or VPCs
- Requiring jump host access
- In production environments with restricted access

**Benefits:**
- ✅ **Secure**: All traffic encrypted through SSH
- ✅ **Automatic**: Set up once, works transparently
- ✅ **Pattern-based**: Configure rules that apply automatically
- ✅ **Multiple protocols**: Works with all database types

## 🚀 Quick Start

### Manual SSH Tunnel

For one-time connections, use the `--ssh-tunnel` flag:

```bash
# Basic SSH tunnel
dbcrust postgres://user:pass@internal-db.company.com/myapp \
  --ssh-tunnel jumphost.company.com

# With SSH user and port
dbcrust postgres://user:pass@internal-db.company.com/myapp \
  --ssh-tunnel admin@jumphost.company.com:2222

# With SSH key
dbcrust postgres://user:pass@internal-db.company.com/myapp \
  --ssh-tunnel admin@jumphost.company.com \
  --ssh-key ~/.ssh/production_key
```

### Automatic Pattern Matching

Configure automatic tunnels in your config file for seamless connections:

```toml
# ~/.config/dbcrust/config.toml

[ssh_tunnel_patterns]
# Pattern → SSH target
"^db\\.internal\\..*\\.com$" = "jumphost.example.com"
".*\\.private\\.net" = "admin@jumphost.example.com:2222"
"prod-.*\\.company\\.com" = "bastion.company.com:22"
".*\\.rds\\.amazonaws\\.com$" = "ec2-bastion.company.com"
```

**Now connections automatically use tunnels:**
```bash
# This automatically routes through jumphost.example.com
dbcrust postgres://user:pass@db.internal.mycompany.com/prod

# This automatically routes through admin@jumphost.example.com:2222
dbcrust mysql://user:pass@mysql.private.net/analytics
```

## 🛠️ Configuration Options

### SSH Tunnel Patterns

Patterns use **regular expressions** to match database hostnames:

```toml
[ssh_tunnel_patterns]
# Exact match
"production-db.company.com" = "bastion.company.com"

# Wildcard patterns
"^.*\\.internal\\.company\\.com$" = "jumphost.company.com"

# Multiple environments
"dev-.*\\.company\\.com" = "dev-bastion.company.com"
"staging-.*\\.company\\.com" = "staging-bastion.company.com"
"prod-.*\\.company\\.com" = "prod-bastion.company.com"

# Cloud providers
".*\\.rds\\.amazonaws\\.com$" = "ec2-bastion.us-west-2.amazonaws.com"
".*\\.postgres\\.database\\.azure\\.com$" = "vm-bastion.westus2.cloudapp.azure.com"
```

### SSH Configuration

DBCrust respects your SSH configuration (`~/.ssh/config`):

```bash
# ~/.ssh/config
Host jumphost
    HostName jumphost.company.com
    User admin
    Port 2222
    IdentityFile ~/.ssh/company_key
    ServerAliveInterval 60
    ServerAliveCountMax 3

Host prod-bastion
    HostName bastion.prod.company.com
    User dbadmin
    IdentityFile ~/.ssh/prod_key
    ProxyJump jumphost
```

**Then use SSH config names in patterns:**
```toml
[ssh_tunnel_patterns]
"^prod-.*\\.company\\.com$" = "prod-bastion"
"^staging-.*\\.company\\.com$" = "jumphost"
```

## 🎯 Real-World Examples

### Enterprise AWS Setup

```toml
# ~/.config/dbcrust/config.toml
[ssh_tunnel_patterns]
# Production RDS instances
"^prod-.*\\.rds\\.amazonaws\\.com$" = "prod-bastion"
"^staging-.*\\.rds\\.amazonaws\\.com$" = "staging-bastion"

# Private subnet databases
"^.*\\.vpc-internal\\.company\\.com$" = "vpc-bastion"
```

```bash
# ~/.ssh/config
Host prod-bastion
    HostName bastion.prod.company.com
    User ec2-user
    IdentityFile ~/.ssh/prod-access.pem

Host staging-bastion
    HostName bastion.staging.company.com
    User ec2-user
    IdentityFile ~/.ssh/staging-access.pem
```

**Usage:**
```bash
# Automatically tunnels through prod-bastion
dbcrust postgres://app_user@prod-main.rds.amazonaws.com/myapp

# Automatically tunnels through staging-bastion
dbcrust postgres://app_user@staging-replica.rds.amazonaws.com/myapp
```

### Multi-Hop SSH Connections

For environments requiring multiple jumps:

```bash
# ~/.ssh/config
Host jump1
    HostName first-jump.company.com
    User admin
    IdentityFile ~/.ssh/company_key

Host jump2
    HostName second-jump.internal
    User admin
    IdentityFile ~/.ssh/internal_key
    ProxyJump jump1

Host database-host
    HostName db.deep-internal.company.com
    User dbuser
    ProxyJump jump2
```

```toml
# ~/.config/dbcrust/config.toml
[ssh_tunnel_patterns]
"^db\\.deep-internal\\.company\\.com$" = "database-host"
```

### Django Production Setup

Perfect for Django teams accessing production databases:

```toml
# ~/.config/dbcrust/config.toml
[ssh_tunnel_patterns]
# Django production databases
"^django-prod\\..*" = "prod-bastion"
"^django-staging\\..*" = "staging-bastion"

# Analytics databases
"^analytics\\..*" = "data-bastion"
```

**Django management commands work seamlessly:**
```bash
# These automatically tunnel through appropriate bastions
python manage.py dbcrust --database default
python manage.py dbcrust --database analytics
```

## 🔧 Advanced Features

### Port Forwarding Configuration

DBCrust automatically manages local ports, but you can configure the range:

```toml
# ~/.config/dbcrust/config.toml
[ssh_tunnel]
local_port_range_start = 5000
local_port_range_end = 5999
bind_address = "127.0.0.1"  # Default
```

### Connection Pooling with Tunnels

When using connection pooling, tunnels are shared efficiently:

```toml
[performance]
enable_connection_pooling = true
pool_max_connections = 5

[ssh_tunnel]
reuse_connections = true  # Default: true
connection_timeout = 30   # seconds
```

### Tunnel Health Monitoring

DBCrust monitors tunnel health and automatically reconnects:

```toml
[ssh_tunnel]
health_check_interval = 30  # seconds
max_reconnect_attempts = 3
reconnect_delay = 5         # seconds
```

## 🚨 Troubleshooting

### Common Issues

#### Connection Refused

```bash
# Test SSH connection manually
ssh -v jumphost.company.com

# Test with specific user/port
ssh -v admin@jumphost.company.com -p 2222

# Test key authentication
ssh -v -i ~/.ssh/company_key admin@jumphost.company.com
```

#### Permission Denied

```bash
# Check SSH key permissions
chmod 600 ~/.ssh/your_key

# Check SSH config permissions
chmod 644 ~/.ssh/config

# Verify key is loaded in SSH agent
ssh-add ~/.ssh/your_key
```

#### Timeout Issues

```toml
# Increase timeouts in config
[ssh_tunnel]
connection_timeout = 60
health_check_interval = 60

[performance]
connection_timeout = 60
```

#### Pattern Matching Problems

Test your regex patterns:
```bash
# Enable debug logging to see pattern matching
dbcrust --debug postgres://db.internal.company.com/test
```

**Debug output shows:**
```
DEBUG: Testing SSH pattern '^db\.internal\..*\.com$' against 'db.internal.company.com'
DEBUG: Pattern matched! Using SSH tunnel: jumphost.company.com
DEBUG: Creating SSH tunnel: db.internal.company.com:5432 -> jumphost.company.com -> localhost:5001
```

### Debug Mode

Enable detailed SSH debugging:

```bash
# Full debug output
dbcrust --debug postgres://internal-db/myapp --ssh-tunnel jumphost.com

# SSH-specific debugging
export DBCRUST_SSH_DEBUG=1
dbcrust postgres://internal-db/myapp --ssh-tunnel jumphost.com
```

### Logging

SSH tunnel operations are logged:

```toml
# ~/.config/dbcrust/config.toml
[logging]
level = "debug"
file_output = true
file_path = "~/.config/dbcrust/dbcrust.log"
```

**Log output includes:**
```
2024-01-15 14:30:00 DEBUG [ssh_tunnel] Creating tunnel: db.company.com:5432 -> bastion.company.com -> localhost:5001
2024-01-15 14:30:01 DEBUG [ssh_tunnel] Tunnel established successfully
2024-01-15 14:30:01 DEBUG [ssh_tunnel] Health check passed
```

## 🛡️ Security Best Practices

### SSH Key Management

```bash
# Generate dedicated keys for database access
ssh-keygen -t ed25519 -C "dbcrust-access" -f ~/.ssh/dbcrust_key

# Use different keys for different environments
ssh-keygen -t ed25519 -C "prod-db-access" -f ~/.ssh/prod_db_key
ssh-keygen -t ed25519 -C "staging-db-access" -f ~/.ssh/staging_db_key
```

### Principle of Least Privilege

Configure jump hosts with minimal permissions:
- SSH access only (no shell access)
- Specific port forwarding rules
- Time-limited access tokens

### Audit Logging

Enable comprehensive logging for compliance:

```toml
[logging]
level = "info"  # Log all tunnel creation/destruction
file_output = true
file_path = "/var/log/dbcrust/dbcrust.log"

[ssh_tunnel]
log_connections = true    # Log each tunnel establishment
log_disconnections = true # Log tunnel closures
```

## 🔗 Integration Examples

### CI/CD Pipelines

```yaml
# GitHub Actions example
- name: Setup SSH tunnel for database tests
  run: |
    # Configure SSH key
    mkdir -p ~/.ssh
    echo "${{ secrets.SSH_PRIVATE_KEY }}" > ~/.ssh/ci_key
    chmod 600 ~/.ssh/ci_key

    # Add SSH config
    cat >> ~/.ssh/config << EOF
    Host ci-bastion
        HostName bastion.ci.company.com
        User ci-user
        IdentityFile ~/.ssh/ci_key
        ServerAliveInterval 60
    EOF

    # Run tests through tunnel
    dbcrust postgres://test@prod-replica.company.com/testdb \
      --ssh-tunnel ci-bastion \
      --query "SELECT version()"
```

### Docker Compose

```yaml
# docker-compose.yml
version: '3.8'
services:
  app:
    build: .
    volumes:
      - ~/.ssh:/root/.ssh:ro
      - ~/.config/dbcrust:/root/.config/dbcrust:ro
    environment:
      - DATABASE_URL=postgres://user@prod-db.company.com/app
    # DBCrust automatically uses SSH tunnel based on patterns
```

### Team Configurations

Share SSH tunnel patterns across your team:

```bash
# Store in version control
git add .dbcrust/ssh-patterns.toml

# Team members can import
cp .dbcrust/ssh-patterns.toml ~/.config/dbcrust/
```

## 📚 See Also

- **[Configuration Reference](/dbcrust/reference/configuration-reference/)** - Complete configuration options
- **[Security Guide](/dbcrust/advanced/security/)** - Security best practices
- **[Vault Integration](/dbcrust/advanced/vault-integration/)** - Dynamic credentials with Vault
- **[URL Schemes](/dbcrust/reference/url-schemes/)** - All supported connection methods

---

<div align="center">
    <strong>Need help with SSH tunnel setup?</strong><br>
    <a href="https://github.com/clement-tourriere/dbcrust/issues" class="md-button md-button--primary">Get Support</a>
    <a href="/dbcrust/advanced/security/" class="md-button">Security Guide</a>
</div>
