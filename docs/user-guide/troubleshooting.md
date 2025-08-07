# Troubleshooting Guide

This comprehensive guide helps you diagnose and resolve common issues with DBCrust. Whether you're experiencing connection problems, performance issues, or unexpected behavior, this guide provides systematic solutions and debugging techniques.

## 🚀 Quick Diagnostics

### First Steps for Any Issue

```bash
# Check DBCrust version and environment
dbcrust --version
dbcrust --debug postgres://localhost/test_connection

# Test basic connectivity
dbcrust postgres://localhost/postgres -c "SELECT 1;"

# Check configuration
dbcrust --show-config

# Enable debug logging for detailed information
export DBCRUST_DEBUG=1
dbcrust your_connection_url
```

### Common Quick Fixes

```bash
# Reset DBCrust configuration
rm -rf ~/.config/dbcrust/
dbcrust postgres://localhost/db  # Will recreate default config

# Clear session data and cache
dbcrust postgres://localhost/db
\resetview
\clrcs
\config reset

# Update DBCrust to latest version
pip install --upgrade dbcrust
# or
cargo install --force dbcrust
```

## 🔌 Connection Issues

### Database Connection Failures

**Problem: "Connection refused" or "Connection timeout"**

```bash
# Debug connection step by step
dbcrust --debug postgres://localhost:5432/mydb

# Test specific connection components
ping localhost                    # Test network connectivity
telnet localhost 5432            # Test port accessibility
psql -h localhost -p 5432 -U user -d mydb  # Test with native client
```

**Common solutions:**
1. **Database not running:** Start your database service
2. **Wrong port:** Check database port (5432 for PostgreSQL, 3306 for MySQL)
3. **Firewall issues:** Open database port in firewall
4. **Host binding:** Ensure database accepts connections from your IP

**Problem: "Authentication failed" or "Access denied"**

```bash
# Test credentials manually
psql -h localhost -U username -d database  # PostgreSQL
mysql -h localhost -u username -p database  # MySQL

# Check credential files
cat ~/.pgpass                    # PostgreSQL passwords
cat ~/.my.cnf                   # MySQL configuration
```

**Solutions:**
1. **Verify credentials:** Ensure username/password are correct
2. **Check .pgpass/.my.cnf:** Verify credential file format
3. **Database permissions:** Grant user access to specific database
4. **SSL requirements:** Add `?sslmode=require` if needed

### SSH Tunnel Connection Issues

**Problem: SSH tunnel fails to establish**

```bash
# Test SSH connection manually
ssh -p 2222 user@jumphost.example.com
ssh -L 5433:db.internal.com:5432 user@jumphost.example.com

# Debug DBCrust SSH tunnel
dbcrust --debug --ssh-tunnel user@jumphost.example.com postgres://db.internal.com/mydb
```

**Common SSH tunnel issues:**
```bash
# SSH key authentication
ssh-add ~/.ssh/id_rsa           # Add SSH key to agent
ssh-keygen -t rsa -b 4096       # Generate new SSH key if needed

# SSH configuration
cat ~/.ssh/config
# Host jumphost
#     HostName jumphost.example.com
#     User your_user
#     Port 2222
#     IdentityFile ~/.ssh/id_rsa

# Test tunnel manually
ssh -L 5433:db.internal.com:5432 user@jumphost.example.com -N -v
```

### Docker Container Connection Issues

**Problem: Cannot connect to database in Docker container**

```bash
# List running containers
docker ps

# Check container logs
docker logs postgres-container

# Test container connectivity
docker exec -it postgres-container psql -U postgres

# Debug DBCrust Docker connection
dbcrust --debug docker://postgres-container/mydb
```

**Common Docker issues:**
1. **Container not running:** `docker start container-name`
2. **Wrong container name:** Use `docker ps` to verify names
3. **Network issues:** Ensure container is on correct network
4. **Port mapping:** Check if ports are properly exposed

### Vault Integration Issues

**Problem: Vault authentication failures**

```bash
# Test Vault connectivity
vault status
vault auth -method=userpass username=myuser

# Check Vault configuration
echo $VAULT_ADDR
echo $VAULT_TOKEN

# Debug DBCrust Vault connection
dbcrust --debug vault://role@mount/database
```

**Vault troubleshooting steps:**
```bash
# Check Vault policies
vault policy read my-policy

# Verify database secrets engine
vault secrets list
vault read database/config/my-database

# Test credential generation
vault read database/creds/my-role
```

## ⚡ Performance Issues

### Slow Query Performance

**Problem: Queries taking too long**

```sql
-- Enable timing and analysis
\timing
\e

-- Run your slow query to see execution plan
SELECT * FROM large_table WHERE complex_condition = true;

-- Check for missing indexes
\di table_name

-- Analyze table statistics
\analyze table_name
```

**Performance debugging steps:**
1. **Check execution plan:** Look for sequential scans, nested loops
2. **Verify indexes:** Ensure appropriate indexes exist
3. **Update table statistics:** Run `ANALYZE` on affected tables
4. **Check query patterns:** Look for N+1 queries, inefficient joins

**Common performance solutions:**
```sql
-- Add missing indexes
CREATE INDEX idx_table_column ON table_name(column_name);

-- Optimize queries
-- Before: SELECT * FROM users WHERE status = 'active';
-- After: SELECT id, name, email FROM users WHERE status = 'active' LIMIT 100;

-- Use query optimization suggestions
\suggest query_optimization
```

### Memory and Resource Issues

**Problem: High memory usage or out-of-memory errors**

```bash
# Check system resources
free -h                          # Available memory
ps aux | grep dbcrust           # DBCrust memory usage

# Enable memory monitoring
export DBCRUST_MEMORY_DEBUG=1
dbcrust postgres://localhost/db
```

**Memory optimization:**
```sql
-- Limit result sets
SELECT * FROM large_table LIMIT 1000;

-- Use streaming for large datasets
\copy (SELECT * FROM huge_table) TO 'output.csv' WITH CSV;

-- Avoid loading large result sets into memory
\paging on                       # Enable result paging
```

### Connection Pool Exhaustion

**Problem: "Too many connections" or connection pool errors**

```sql
-- Check active connections
\connections

-- Monitor connection usage
\monitor connections

-- Check database connection limits
SHOW max_connections;            -- PostgreSQL
SHOW VARIABLES LIKE 'max_connections';  -- MySQL
```

**Connection management:**
```bash
# Optimize connection settings
dbcrust --max-connections 10 postgres://localhost/db

# Use connection pooling
dbcrust --connection-pool-size 5 postgres://localhost/db

# Close idle connections
dbcrust postgres://localhost/db
\connections close_idle
```

## 🛠️ Configuration Issues

### Configuration File Problems

**Problem: Settings not applied or configuration errors**

```bash
# Check configuration location
dbcrust --show-config-path

# Validate configuration syntax
dbcrust --validate-config

# Debug configuration loading
dbcrust --debug-config postgres://localhost/db
```

**Configuration troubleshooting:**
```bash
# Reset to default configuration
mv ~/.config/dbcrust/config.toml ~/.config/dbcrust/config.toml.backup
dbcrust postgres://localhost/db  # Creates new default config

# Check configuration syntax
python -c "import toml; toml.load('~/.config/dbcrust/config.toml')"

# Verify file permissions
ls -la ~/.config/dbcrust/
chmod 644 ~/.config/dbcrust/config.toml
```

### Environment Variables Issues

**Problem: Environment variables not recognized**

```bash
# Check current environment
env | grep DBCRUST
env | grep DATABASE_URL

# Test environment variable override
export DBCRUST_DEBUG=1
export DATABASE_URL=postgres://localhost/test
dbcrust

# Debug environment loading
dbcrust --show-env postgres://localhost/db
```

**Common environment issues:**
```bash
# Shell profile loading
source ~/.bashrc                # Reload bash profile
source ~/.zshrc                 # Reload zsh profile

# Variable scoping
export DBCRUST_LOG_LEVEL=debug  # Make sure to export
dbcrust postgres://localhost/db

# Configuration precedence
# 1. Command line arguments (highest)
# 2. Environment variables
# 3. Configuration file
# 4. Default values (lowest)
```

## 🔍 Feature-Specific Issues

### Autocomplete Not Working

**Problem: SQL autocomplete not functioning**

```sql
-- Test autocomplete functionality
SELECT u. [TAB]                  -- Should show user table columns
\dt [TAB]                        -- Should show available tables

-- Enable debug for autocomplete
\set debug autocomplete on
```

**Autocomplete debugging:**
```bash
# Check database introspection permissions
dbcrust postgres://localhost/db
\dt                              # List tables - should work
\d table_name                   # Describe table - should work

# Clear autocomplete cache
\cache clear

# Check logs for autocomplete errors
tail -f ~/.config/dbcrust/dbcrust.log | grep autocomplete
```

### Column Selection Issues

**Problem: Column selection interface not appearing**

```sql
-- Check column selection settings
\config display

-- Manually test column selection
\cs                              -- Toggle column selection mode
SELECT * FROM wide_table;        -- Should show column selection

-- Check threshold settings
\csthreshold 5                   -- Lower threshold to trigger more often
```

### Named Queries Issues

**Problem: Named queries not saving or executing**

```sql
-- Test named query functionality
\ns test_query SELECT 1 as test;
\n                               -- Should show 'test_query'
test_query                       -- Should execute

-- Check named query storage
\n --verbose                     -- Show detailed query information

-- Debug named query issues
\debug named_queries on
\ns another_test SELECT * FROM small_table;
```

### External Editor Integration

**Problem: External editor not opening**

```bash
# Check EDITOR environment variable
echo $EDITOR

# Test editor directly
$EDITOR test.sql

# Set editor explicitly
export EDITOR="code --wait"      # VS Code
export EDITOR="vim"              # Vim
export EDITOR="nano"             # Nano

# Test DBCrust editor integration
dbcrust postgres://localhost/db
\ed                              # Should open editor
```

## 📁 File and Permission Issues

### Configuration Directory Problems

**Problem: Cannot write to configuration directory**

```bash
# Check configuration directory permissions
ls -la ~/.config/
ls -la ~/.config/dbcrust/

# Fix directory permissions
mkdir -p ~/.config/dbcrust/
chmod 755 ~/.config/dbcrust/
chmod 644 ~/.config/dbcrust/*.toml

# Check disk space
df -h ~/.config/dbcrust/
```

### Log File Issues

**Problem: Cannot write to log files or logs not appearing**

```bash
# Check log file location and permissions
ls -la ~/.config/dbcrust/dbcrust.log

# Fix log file permissions
touch ~/.config/dbcrust/dbcrust.log
chmod 644 ~/.config/dbcrust/dbcrust.log

# Test logging
export DBCRUST_LOG_LEVEL=debug
dbcrust --debug postgres://localhost/db
tail -f ~/.config/dbcrust/dbcrust.log
```

### Session File Corruption

**Problem: Saved sessions not loading or corrupted**

```bash
# Check session file integrity
cat ~/.config/dbcrust/config.toml | grep saved_sessions

# Backup and reset sessions
cp ~/.config/dbcrust/config.toml ~/.config/dbcrust/config.toml.backup
dbcrust postgres://localhost/db
\ss new_session                  # Save new session to test
```

## 🌐 Network and Security Issues

### SSL/TLS Connection Issues

**Problem: SSL certificate errors or connection encryption issues**

```bash
# Test SSL connection manually
psql "sslmode=require host=localhost dbname=test user=postgres"

# Debug SSL in DBCrust
dbcrust "postgres://user@localhost/db?sslmode=require" --debug

# Common SSL parameters
# sslmode=disable     # No SSL
# sslmode=allow       # SSL if available
# sslmode=prefer      # Prefer SSL (default)
# sslmode=require     # Require SSL
# sslmode=verify-ca   # Verify certificate authority
# sslmode=verify-full # Full certificate verification
```

**SSL troubleshooting:**
```bash
# Check certificate files
ls -la ~/.postgresql/
ls -la /etc/ssl/certs/

# Test certificate validation
openssl s_client -connect database-host:5432 -starttls postgres

# Common SSL issues and fixes:
dbcrust "postgres://user@host/db?sslmode=disable"  # Disable SSL temporarily
dbcrust "postgres://user@host/db?sslmode=require&sslcert=client.crt&sslkey=client.key"
```

### Proxy and Network Issues

**Problem: Connection issues through corporate firewalls or proxies**

```bash
# Check proxy settings
echo $http_proxy
echo $https_proxy
echo $no_proxy

# Test direct connection vs proxy
telnet database-host 5432        # Direct connection
curl -v database-host:5432       # Through proxy

# Bypass proxy for database connections
export no_proxy="localhost,127.0.0.1,database-host"
dbcrust postgres://database-host/db
```

## 🐛 Advanced Debugging

### Enable Comprehensive Debug Logging

```bash
# Enable all debug modes
export DBCRUST_DEBUG=1
export DBCRUST_TRACE=1
export DBCRUST_LOG_LEVEL=trace

# Run with maximum debugging
dbcrust --debug --verbose postgres://localhost/db 2>&1 | tee debug.log
```

### Debug Specific Components

```bash
# Debug specific subsystems
export DBCRUST_DEBUG_AUTOCOMPLETE=1
export DBCRUST_DEBUG_CONNECTIONS=1
export DBCRUST_DEBUG_QUERIES=1
export DBCRUST_DEBUG_PARSER=1

# Component-specific debugging
dbcrust --debug-connections postgres://localhost/db
dbcrust --debug-autocomplete postgres://localhost/db
```

### Performance Profiling

```bash
# Profile DBCrust performance
time dbcrust postgres://localhost/db -c "SELECT COUNT(*) FROM large_table;"

# Memory profiling (Linux)
valgrind --tool=memcheck dbcrust postgres://localhost/db

# System call tracing (Linux)
strace -e trace=network dbcrust postgres://localhost/db
```

### Generate Debug Reports

```sql
-- Generate comprehensive debug report
\debug report

-- System information
\debug system

-- Connection information
\debug connection

-- Configuration dump
\debug config

-- Recent query history
\debug queries
```

## 🆘 Getting Help

### Information Collection for Bug Reports

When reporting issues, collect this information:

```bash
# System information
dbcrust --version
uname -a                         # System info
psql --version                   # Database client version

# Configuration
dbcrust --show-config
cat ~/.config/dbcrust/config.toml

# Debug output
export DBCRUST_DEBUG=1
dbcrust your_connection_url 2>&1 | tee debug_output.txt

# Database information
dbcrust postgres://localhost/db -c "SELECT version();"
```

### Minimal Reproduction Case

Create a minimal test case:

```bash
# Test with minimal setup
dbcrust postgres://localhost/postgres -c "SELECT 1;"

# Test with fresh configuration
mv ~/.config/dbcrust ~/.config/dbcrust.backup
dbcrust postgres://localhost/test_db

# Test with specific version
pip install dbcrust==0.15.1      # Specific version
dbcrust --version
```

### Common Error Patterns and Solutions

**Error: "command not found: dbcrust"**
```bash
# Check installation
pip list | grep dbcrust
which dbcrust

# Reinstall
pip uninstall dbcrust
pip install dbcrust

# Check PATH
echo $PATH
```

**Error: "SSL connection has been closed unexpectedly"**
```bash
# Common SSL fixes
dbcrust "postgres://user@host/db?sslmode=disable"
dbcrust "postgres://user@host/db?sslmode=require"
```

**Error: "FATAL: remaining connection slots are reserved"**
```bash
# Reduce connection usage
dbcrust --max-connections 1 postgres://host/db
# Or increase database max_connections
```

**Error: "permission denied for database/table"**
```bash
# Check user permissions
psql -h host -U user -d postgres -c "\\du"    # List users and roles
psql -h host -U user -d database -c "\\z"     # Check permissions
```

## 📚 See Also

- **[Performance Analysis](/dbcrust/user-guide/performance-analysis/)** - Optimize database performance
- **[Advanced Features](/dbcrust/user-guide/advanced-features/)** - Session management and tools
- **[Configuration Reference](/dbcrust/reference/configuration-reference/)** - Complete configuration options
- **[Backslash Commands](/dbcrust/reference/backslash-commands/)** - All interactive commands

---

<div align="center">
    <strong>Still having issues?</strong><br>
    <a href="https://github.com/anthropics/dbcrust/issues" class="md-button md-button--primary">Report an Issue</a>
    <a href="/dbcrust/user-guide/performance-analysis/" class="md-button">Performance Guide</a>
</div>
