# Docker Integration

DBCrust provides seamless integration with Docker containers, making it easy to connect to databases running in containers whether they're part of your development workflow, testing pipeline, or containerized production environment. This guide covers everything from basic container connections to advanced Docker Compose integration.

## 🐳 Why Docker Integration?

Docker integration simplifies database development workflows:
- ✅ **Auto-discovery** - Automatically find running database containers
- ✅ **Intelligent routing** - Works with exposed ports and internal networks
- ✅ **OrbStack support** - Native integration with OrbStack on macOS
- ✅ **Compose integration** - Seamless Docker Compose project support
- ✅ **Development workflow** - Perfect for local development and testing

## 🚀 Quick Start

### Interactive Container Selection

The simplest way to connect is using interactive selection:

```bash
# Show all running database containers
dbcrust docker://

# Example output:
# Available database containers:
# 1. postgres-dev (postgres:15) - Port 5432 → 5433
# 2. mysql-test (mysql:8.0) - Port 3306 → 3307
# 3. clickhouse-analytics (clickhouse:latest) - Port 8123 → 8124
# 4. mongodb-cache (mongo:7) - Port 27017 → 27018
#
# Select container (1-4): 1
```

### Direct Container Connection

```bash
# Connect to specific container by name
dbcrust docker://postgres-dev
dbcrust docker://mysql-test
dbcrust docker://clickhouse-analytics
dbcrust docker://my-app-db

# With credentials
dbcrust docker://user:password@postgres-dev
dbcrust docker://root:secret@mysql-test/specific_database
dbcrust docker://clickhouse:password@clickhouse-analytics/analytics

# Full URL format
dbcrust docker://username:password@container-name/database-name
```

### Shell Autocompletion

DBCrust provides intelligent autocompletion for container names:

```bash
# Type partial name and press TAB
dbc docker://post[TAB] → postgres-dev, postgres-prod
dbc docker://my[TAB]   → mysql-test, my-app-db
dbc docker://click[TAB] → clickhouse-analytics, clickhouse-dev
```

## 🛠️ Connection Methods

### Exposed Ports (Standard Docker)

For containers with exposed ports:

```bash
# Container with exposed port
docker run -d --name postgres-dev -p 5433:5432 postgres:15

# DBCrust automatically detects the port mapping
dbcrust docker://postgres-dev
# → Connects to localhost:5433
```

### OrbStack Integration (macOS)

OrbStack provides seamless container networking on macOS:

```bash
# Containers without exposed ports work automatically
docker run -d --name postgres-dev postgres:15

# DBCrust uses OrbStack's DNS resolution
dbcrust docker://postgres-dev
# → Connects to postgres-dev.orb.local:5432
```

### Docker Network Support

Connect to containers on custom networks:

```bash
# Create custom network
docker network create myapp-network

# Run container on network
docker run -d --name postgres-app --network myapp-network postgres:15

# DBCrust finds the container on the network
dbcrust docker://postgres-app
```

## 🐍 Docker Compose Integration

### Compose Project Detection

DBCrust automatically detects Docker Compose projects:

```yaml
# docker-compose.yml
version: '3.8'
services:
  database:
    image: postgres:15
    environment:
      POSTGRES_DB: myapp
      POSTGRES_USER: myapp
      POSTGRES_PASSWORD: secret
    ports:
      - "5432:5432"

  redis:
    image: redis:7-alpine
    ports:
      - "6379:6379"
```

```bash
# Connect to compose services
dbcrust docker://database          # postgres service
dbcrust docker://myapp_database_1  # alternative name

# With OrbStack DNS
dbcrust docker://database.myapp.orb.local
```

### Multi-Environment Compose

```bash
# Development environment
docker-compose -f docker-compose.dev.yml up -d
dbcrust docker://postgres-dev

# Testing environment
docker-compose -f docker-compose.test.yml up -d
dbcrust docker://postgres-test

# Production-like staging
docker-compose -f docker-compose.staging.yml up -d
dbcrust docker://postgres-staging
```

### Django Development Workflow

Perfect integration with Django development:

```yaml
# docker-compose.yml for Django project
version: '3.8'
services:
  web:
    build: .
    ports:
      - "8000:8000"
    depends_on:
      - db
      - redis
    environment:
      - DATABASE_URL=postgres://django:password@db:5432/django_db

  db:
    image: postgres:15
    environment:
      POSTGRES_DB: django_db
      POSTGRES_USER: django
      POSTGRES_PASSWORD: password
    ports:
      - "5432:5432"
    volumes:
      - postgres_data:/var/lib/postgresql/data

  redis:
    image: redis:7-alpine
    ports:
      - "6379:6379"

volumes:
  postgres_data:
```

```bash
# Start development environment
docker-compose up -d

# Connect to Django database
dbcrust docker://db

# Use with Django management commands
python manage.py dbcrust docker://db

# Connect with specific database/user
dbcrust docker://django:password@db/django_db
```

## 🎯 Database Type Detection

DBCrust automatically detects database types from container images:

### PostgreSQL Containers

```bash
# Detected from image names
dbcrust docker://postgres-container      # postgres:*
dbcrust docker://my-pg-db               # *postgres*
dbcrust docker://timescaledb-dev        # timescale/*
dbcrust docker://postgis-container      # postgis/*
```

### MySQL Containers

```bash
# Detected from image names
dbcrust docker://mysql-container        # mysql:*
dbcrust docker://mariadb-dev           # mariadb:*
dbcrust docker://percona-db            # percona:*
```

### ClickHouse Containers

```bash
# Detected from image names
dbcrust docker://clickhouse-container    # clickhouse/*
dbcrust docker://analytics-ch           # *clickhouse*
dbcrust docker://yandex-clickhouse      # yandex/clickhouse*

# ClickHouse with credentials
dbcrust docker://user:password@clickhouse-analytics
dbcrust docker://clickhouse-analytics/analytics_db

# Special handling for CLICKHOUSE_SKIP_USER_SETUP=1
dbcrust docker://clickhouse-dev  # No password needed when setup is skipped
```

### SQLite in Containers

```bash
# For applications with SQLite
dbcrust docker://app-container --sqlite-path /app/db.sqlite3
```

## 🔧 Advanced Configuration

### Container Detection Settings

```toml
# ~/.config/dbcrust/config.toml

[docker]
# Enable Docker integration
enabled = true

# Docker socket path (auto-detected)
socket_path = "/var/run/docker.sock"

# Container name patterns for database detection
database_patterns = [
    ".*postgres.*",
    ".*mysql.*",
    ".*mariadb.*",
    ".*clickhouse.*",
    ".*mongo.*",
    ".*redis.*"
]

# Port detection timeout
detection_timeout = 5  # seconds

# Prefer exposed ports over network resolution
prefer_exposed_ports = true
```

### OrbStack Configuration

```toml
[docker.orbstack]
# Enable OrbStack DNS resolution
enabled = true

# OrbStack domain (auto-detected)
domain = "orb.local"

# Custom domains (for containers with labels)
custom_domains = true
```

### Network Configuration

```toml
[docker.network]
# Scan specific networks only
networks = ["bridge", "myapp-network"]

# Network detection order
network_priority = ["custom", "bridge", "host"]

# Connection timeout for network resolution
timeout = 10  # seconds
```

## 🏗️ Development Workflows

### Local Development Setup

**1. Database Container Setup:**
```bash
# PostgreSQL for development
docker run -d \
  --name dev-postgres \
  -p 5432:5432 \
  -e POSTGRES_DB=myapp_dev \
  -e POSTGRES_USER=developer \
  -e POSTGRES_PASSWORD=devpass \
  -v postgres_dev_data:/var/lib/postgresql/data \
  postgres:15

# Connect immediately
dbcrust docker://dev-postgres
```

**2. Multi-Database Development:**
```bash
# Start multiple databases
docker run -d --name postgres-main -p 5432:5432 postgres:15
docker run -d --name mysql-analytics -p 3306:3306 mysql:8.0
docker run -d --name redis-cache -p 6379:6379 redis:7

# Connect to different databases
dbcrust docker://postgres-main     # Main application DB
dbcrust docker://mysql-analytics   # Analytics DB
dbcrust docker://redis-cache       # Cache (if supported)
```

### Testing Workflows

**Database Testing with Containers:**
```bash
#!/bin/bash
# test-setup.sh

# Start test database
docker run -d \
  --name test-db-$(date +%s) \
  -p 5433:5432 \
  -e POSTGRES_DB=test_db \
  -e POSTGRES_USER=test \
  -e POSTGRES_PASSWORD=test \
  postgres:15

# Wait for database to be ready
sleep 3

# Run tests
dbcrust docker://test-db-* --query "CREATE DATABASE IF NOT EXISTS test_myapp;"
python -m pytest tests/

# Cleanup
docker rm -f test-db-*
```

### CI/CD Integration

**GitHub Actions with Docker:**
```yaml
name: Database Tests
on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest

    services:
      postgres:
        image: postgres:15
        env:
          POSTGRES_PASSWORD: postgres
          POSTGRES_DB: test_db
        ports:
          - 5432:5432
        options: >-
          --health-cmd pg_isready
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5

    steps:
    - uses: actions/checkout@v4

    - name: Install DBCrust
      run: |
        curl -fsSL https://clement-tourriere.github.io/dbcrust/install.sh | sh

    - name: Test database connection
      run: |
        # Connect to service container
        dbcrust postgres://postgres:postgres@localhost:5432/test_db \
          --query "SELECT version();"

    - name: Run database tests
      run: |
        # Use DBCrust for test queries
        dbcrust postgres://postgres:postgres@localhost:5432/test_db \
          --query "CREATE TABLE test_users (id SERIAL PRIMARY KEY, name VARCHAR(100));"
```

**GitLab CI with Docker Compose:**
```yaml
test:
  stage: test
  services:
    - docker:dind
  before_script:
    - docker-compose up -d database
    - curl -fsSL https://clement-tourriere.github.io/dbcrust/install.sh | sh
  script:
    - dbcrust docker://database --query "SELECT 1;"
    - python -m pytest tests/
  after_script:
    - docker-compose down
```

## 🎨 Real-World Examples

### Django Project with Docker

**Complete Django development setup:**

```yaml
# docker-compose.dev.yml
version: '3.8'
services:
  web:
    build: .
    command: python manage.py runserver 0.0.0.0:8000
    volumes:
      - .:/code
    ports:
      - "8000:8000"
    depends_on:
      - db
      - redis
    environment:
      - DEBUG=1
      - DATABASE_URL=postgres://django:django@db:5432/django_dev

  db:
    image: postgres:15
    environment:
      POSTGRES_DB: django_dev
      POSTGRES_USER: django
      POSTGRES_PASSWORD: django
    ports:
      - "5432:5432"
    volumes:
      - postgres_data:/var/lib/postgresql/data

  redis:
    image: redis:7-alpine
    ports:
      - "6379:6379"

volumes:
  postgres_data:
```

**Development workflow:**
```bash
# Start development environment
docker-compose -f docker-compose.dev.yml up -d

# Database management
dbcrust docker://db  # Interactive database access

# Django migrations
python manage.py migrate

# Connect DBCrust to the same database Django uses
python manage.py dbcrust docker://db

# Run Django-specific queries
dbcrust docker://django:django@db/django_dev \
  --query "SELECT * FROM django_migrations ORDER BY applied DESC LIMIT 10;"
```

### Microservices Architecture

**Multi-service database setup:**
```yaml
# docker-compose.services.yml
version: '3.8'
services:
  user-service-db:
    image: postgres:15
    environment:
      POSTGRES_DB: users
      POSTGRES_USER: user_service
      POSTGRES_PASSWORD: userpass
    ports:
      - "5432:5432"

  order-service-db:
    image: postgres:15
    environment:
      POSTGRES_DB: orders
      POSTGRES_USER: order_service
      POSTGRES_PASSWORD: orderpass
    ports:
      - "5433:5432"

  analytics-db:
    image: mysql:8.0
    environment:
      MYSQL_DATABASE: analytics
      MYSQL_USER: analytics
      MYSQL_PASSWORD: analyticspass
      MYSQL_ROOT_PASSWORD: rootpass
    ports:
      - "3306:3306"

  cache:
    image: redis:7-alpine
    ports:
      - "6379:6379"
```

**Service-specific connections:**
```bash
# Connect to each service database
dbcrust docker://user-service-db       # Users microservice
dbcrust docker://order-service-db      # Orders microservice
dbcrust docker://analytics-db          # Analytics database

# Cross-service queries (if needed)
dbcrust docker://user_service:userpass@user-service-db/users \
  --query "SELECT COUNT(*) FROM users WHERE created_at > '2024-01-01';"
```

### Data Pipeline Development

**ETL pipeline with multiple databases:**
```yaml
# docker-compose.pipeline.yml
version: '3.8'
services:
  source-mysql:
    image: mysql:8.0
    environment:
      MYSQL_DATABASE: source_data
      MYSQL_USER: etl_reader
      MYSQL_PASSWORD: reader123
      MYSQL_ROOT_PASSWORD: rootpass
    ports:
      - "3306:3306"
    volumes:
      - ./sample-data:/docker-entrypoint-initdb.d

  target-postgres:
    image: postgres:15
    environment:
      POSTGRES_DB: data_warehouse
      POSTGRES_USER: etl_writer
      POSTGRES_PASSWORD: writer123
    ports:
      - "5432:5432"

  analytics-postgres:
    image: postgres:15
    environment:
      POSTGRES_DB: analytics
      POSTGRES_USER: analyst
      POSTGRES_PASSWORD: analyst123
    ports:
      - "5433:5432"
```

**ETL workflow:**
```bash
# Extract from source
dbcrust docker://etl_reader:reader123@source-mysql/source_data \
  --query "SELECT * FROM transactions WHERE processed_at > '2024-01-01';" \
  --output json > extracted_data.json

# Transform and load (example Python script)
python etl_transform.py extracted_data.json

# Verify in target
dbcrust docker://etl_writer:writer123@target-postgres/data_warehouse \
  --query "SELECT COUNT(*) FROM transformed_transactions;"

# Analytics queries
dbcrust docker://analyst:analyst123@analytics-postgres/analytics \
  --query "SELECT DATE(created_at), COUNT(*) FROM daily_summaries GROUP BY DATE(created_at);"
```

### ClickHouse Analytics Pipeline

**ClickHouse for real-time analytics:**
```yaml
# docker-compose.analytics.yml
version: '3.8'
services:
  clickhouse:
    image: clickhouse/clickhouse-server:latest
    environment:
      CLICKHOUSE_DB: analytics
      CLICKHOUSE_USER: analyst
      CLICKHOUSE_PASSWORD: analytics123
    ports:
      - "8123:8123"
      - "9000:9000"
    volumes:
      - clickhouse_data:/var/lib/clickhouse

  kafka:
    image: confluentinc/cp-kafka:latest
    environment:
      KAFKA_ZOOKEEPER_CONNECT: zookeeper:2181
      KAFKA_ADVERTISED_LISTENERS: PLAINTEXT://kafka:9092
    depends_on:
      - zookeeper

volumes:
  clickhouse_data:
```

**Analytics workflow:**
```bash
# Start ClickHouse analytics stack
docker-compose -f docker-compose.analytics.yml up -d

# Create analytics tables
dbcrust docker://analyst:analytics123@clickhouse/analytics \
  --query "CREATE TABLE events (
    event_time DateTime,
    user_id UInt32,
    event_type String,
    properties JSON
  ) ENGINE = MergeTree()
  ORDER BY (event_time, user_id);"

# Query real-time analytics
dbcrust docker://clickhouse/analytics \
  --query "SELECT 
    toStartOfHour(event_time) as hour,
    event_type,
    COUNT() as event_count
  FROM events 
  WHERE event_time >= now() - INTERVAL 24 HOUR
  GROUP BY hour, event_type
  ORDER BY hour DESC;"

# Performance analysis with ClickHouse
dbcrust docker://clickhouse \
  --query "SELECT 
    quantile(0.95)(response_time) as p95_response,
    quantile(0.99)(response_time) as p99_response
  FROM api_logs 
  WHERE timestamp >= today();"
```

## 🚨 Troubleshooting

### Container Discovery Issues

**Container not found:**
```bash
# List all running containers
docker ps --format "table {{.Names}}\t{{.Image}}\t{{.Ports}}"

# Check container is running
docker ps | grep postgres-dev

# Try full container name/ID
dbcrust docker://$(docker ps -q --filter name=postgres-dev)
```

**Network connectivity issues:**
```bash
# Test container network
docker exec postgres-dev pg_isready

# Check exposed ports
docker port postgres-dev

# Test connection manually
telnet localhost 5432
```

### OrbStack Issues (macOS)

**OrbStack DNS not resolving:**
```bash
# Check OrbStack is running
orb status

# Test DNS resolution
nslookup postgres-dev.orb.local

# Check container in OrbStack
orb list
```

**Container not accessible:**
```bash
# Verify OrbStack integration
orb ps

# Check container logs
docker logs postgres-dev

# Try explicit OrbStack domain
dbcrust docker://postgres-dev.orb.local
```

### Docker Compose Issues

**Service discovery problems:**
```bash
# Check compose project
docker-compose ps

# Check networks
docker network ls
docker-compose config

# Test inter-service connectivity
docker-compose exec web ping db
```

**Port conflicts:**
```bash
# Check port usage
netstat -tulpn | grep :5432
lsof -i :5432

# Use different ports in compose
ports:
  - "5433:5432"  # Map to different host port
```

### Performance Issues

**Slow container detection:**
```toml
# ~/.config/dbcrust/config.toml
[docker]
detection_timeout = 10      # Increase timeout
prefer_exposed_ports = true # Use faster port detection
```

**Connection timeouts:**
```toml
[docker.network]
timeout = 30  # Increase network timeout

[performance]
connection_timeout = 60  # General connection timeout
```

### Debug Mode

Enable Docker integration debugging:

```bash
# Enable debug logging
export DBCRUST_DOCKER_DEBUG=1

# Full debug mode
dbcrust --debug docker://postgres-dev

# Check Docker daemon connectivity
docker version
```

**Debug output shows:**
```
DEBUG: Docker daemon connected successfully
DEBUG: Found 3 running containers
DEBUG: Container 'postgres-dev' detected as PostgreSQL
DEBUG: Port mapping: 5432/tcp -> 0.0.0.0:5433
DEBUG: Connection URL: postgres://postgres@localhost:5433/postgres
```

## 🛡️ Security Considerations

### Container Security

```bash
# Use non-root containers
docker run -d --name postgres-dev --user postgres postgres:15

# Limit container resources
docker run -d --name postgres-dev --memory 512m --cpus 1.0 postgres:15

# Use security scanning
docker scout cves postgres:15
```

### Network Security

```toml
# ~/.config/dbcrust/config.toml
[docker.network]
# Only connect to specific networks
networks = ["trusted-network"]

# Disable automatic network scanning
auto_discover = false
```

### Credential Security

```bash
# Use environment files for secrets
echo "POSTGRES_PASSWORD=secret123" > .env.secret

# Mount as volume instead of environment variable
docker run -d \
  --name postgres-dev \
  -v "$(pwd)/.env.secret:/etc/postgresql/.env" \
  postgres:15
```

## 📚 See Also

- **[SSH Tunneling](/dbcrust/advanced/ssh-tunneling/)** - Secure database connections
- **[Vault Integration](/dbcrust/advanced/vault-integration/)** - Dynamic credentials
- **[Security Guide](/dbcrust/advanced/security/)** - Complete security practices
- **[URL Schemes](/dbcrust/reference/url-schemes/)** - All connection methods

---

<div align="center">
    <strong>Need help with Docker integration?</strong><br>
    <a href="https://github.com/clement-tourriere/dbcrust/issues" class="md-button md-button--primary">Get Support</a>
    <a href="/dbcrust/user-guide/basic-usage/" class="md-button">User Guide</a>
</div>
