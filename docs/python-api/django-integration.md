# Django Integration

DBCrust provides seamless integration with Django applications through automatic database configuration discovery and enhanced cursor-based operations. This eliminates the need for manual connection URLs and provides a Django-native database interface.

## 🚀 Quick Start

### Automatic Django Database Connection

The Django helper automatically uses your Django `DATABASES` configuration:

```python
# Instead of manually specifying connection URLs
from dbcrust.django import connect

# Use your default Django database
with connect() as connection:
    server_info = connection.get_server_info()
    print(f"Connected to: {server_info.database_type} {server_info.version}")

    with connection.cursor() as cursor:
        cursor.execute("SELECT * FROM auth_user WHERE is_active = %s", (True,))
        active_users = cursor.fetchall()
        print(f"Found {len(active_users)} active users")
```

### Multi-Database Support

Work with multiple Django databases seamlessly:

```python
from dbcrust.django import connect

# Use specific database alias
with connect("analytics") as connection:
    with connection.cursor() as cursor:
        cursor.execute("SELECT COUNT(*) FROM events WHERE date >= %s", (last_month,))
        event_count = cursor.fetchone()[0]

# Connect to all configured databases
from dbcrust.django import connect_all_databases

connections = connect_all_databases()
for alias, connection in connections.items():
    server_info = connection.get_server_info()
    print(f"{alias}: {server_info.database_type} {server_info.version}")
```

## 🏗️ API Reference

### Connection Functions

#### `connect(database=None, **kwargs)`

Connect to a Django database using automatic configuration discovery.

**Parameters:**
- `database` (str, optional): Database alias from `DATABASES` setting (default: 'default')
- `alias` (str, optional): Alternative parameter name for database
- `timeout` (float, optional): Connection timeout in seconds
- `auto_commit` (bool, optional): Enable auto-commit mode
- `cache_connections` (bool): Cache connections per thread (default: True)

**Returns:** `Connection` object with enhanced cursor API

**Examples:**
```python
# Use default database
with connect() as conn:
    pass

# Use specific database
with connect("secondary") as conn:
    pass

# Custom timeout for long operations
with connect("reporting", timeout=60) as conn:
    pass
```

#### `connect_all_databases(**kwargs)`

Connect to all configured Django databases.

**Returns:** Dictionary mapping database alias to `Connection` object

```python
connections = connect_all_databases()
for alias, conn in connections.items():
    print(f"Connected to {alias}")
```

#### `transaction(database=None, **kwargs)`

Context manager for database transactions.

```python
from dbcrust.django import transaction

with transaction() as cursor:
    cursor.execute("INSERT INTO users (name) VALUES (%s)", ("Alice",))
    cursor.execute("UPDATE profiles SET updated_at = NOW() WHERE user_id = %s", (user_id,))
    # Automatically commits on success, rolls back on error
```

### Information Functions

#### `get_database_info(database=None)`

Get detailed information about a Django database.

```python
from dbcrust.django import get_database_info

info = get_database_info()
print(f"Database: {info['server_type']} {info['server_version']}")
print(f"Host: {info['host']}, Port: {info['port']}")
print(f"Database Name: {info['database_name']}")
```

#### `list_django_databases()`

List all Django databases with support status.

```python
from dbcrust.django import list_django_databases

databases = list_django_databases()
for alias, info in databases.items():
    status = "✅" if info['supported'] else "❌"
    print(f"{alias}: {info['engine_type']} {status}")
```

## 📊 Real-World Examples

### Data Analysis with Django Models

```python
from dbcrust.django import connect
from django.contrib.auth.models import User
from myapp.models import Order, Product

def generate_user_analytics_report():
    """Generate comprehensive user analytics using raw SQL."""

    with connect() as connection:
        server_info = connection.get_server_info()
        print(f"Running analysis on {server_info.database_type} {server_info.version}")

        with connection.cursor() as cursor:
            # Multi-statement analysis script
            analysis_script = """
                -- Create temporary tables for analysis
                CREATE TEMP TABLE user_stats AS
                SELECT
                    au.id,
                    au.username,
                    au.date_joined,
                    COUNT(DISTINCT o.id) as order_count,
                    COALESCE(SUM(o.total_amount), 0) as total_spent,
                    MAX(o.created_at) as last_order_date
                FROM auth_user au
                LEFT JOIN myapp_order o ON au.id = o.user_id
                WHERE au.is_active = true
                GROUP BY au.id, au.username, au.date_joined;

                -- Get summary statistics
                SELECT
                    'Total Active Users' as metric,
                    COUNT(*) as value
                FROM auth_user WHERE is_active = true
                UNION ALL
                SELECT
                    'Users with Orders' as metric,
                    COUNT(*) as value
                FROM user_stats WHERE order_count > 0
                UNION ALL
                SELECT
                    'Average Orders per User' as metric,
                    ROUND(AVG(order_count), 2) as value
                FROM user_stats;

                -- Top 10 customers by spend
                SELECT
                    username,
                    order_count,
                    total_spent,
                    last_order_date
                FROM user_stats
                WHERE order_count > 0
                ORDER BY total_spent DESC
                LIMIT 10;
            """

            cursor.executescript(analysis_script)

            # Navigate through result sets
            # First: CREATE TEMP TABLE (no results)
            cursor.fetchall()
            cursor.nextset()

            # Second: Summary statistics
            print("\n📊 User Analytics Summary:")
            summary = cursor.fetchall()
            for row in summary:
                print(f"  {row[0]}: {row[1]}")
            cursor.nextset()

            # Third: Top customers
            print("\n🏆 Top 10 Customers by Spend:")
            top_customers = cursor.fetchall()
            for i, row in enumerate(top_customers, 1):
                username, orders, spent, last_order = row
                print(f"  {i}. {username}: {orders} orders, ${spent:.2f} spent (last: {last_order})")

    return {"summary": summary, "top_customers": top_customers}
```

### Multi-Database ETL Operations

```python
from dbcrust.django import connect
from datetime import datetime, timedelta

def sync_analytics_data():
    """Sync data between main database and analytics warehouse."""

    last_sync = datetime.now() - timedelta(days=1)

    # Extract from main database
    with connect() as main_db:
        with main_db.cursor() as cursor:
            cursor.execute("""
                SELECT
                    id, user_id, product_id, quantity,
                    price, created_at, status
                FROM myapp_order
                WHERE created_at >= %s
                ORDER BY created_at
            """, (last_sync,))

            new_orders = cursor.fetchall()
            print(f"Found {len(new_orders)} new orders to sync")

    # Load into analytics database
    if new_orders:
        with connect("analytics") as analytics_db:
            with analytics_db.cursor() as cursor:
                # Prepare analytics schema
                cursor.execute("""
                    CREATE TABLE IF NOT EXISTS order_events (
                        order_id INTEGER,
                        user_id INTEGER,
                        product_id INTEGER,
                        quantity INTEGER,
                        revenue DECIMAL(10,2),
                        event_date DATE,
                        status VARCHAR(50),
                        created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                    )
                """)

                # Bulk insert with transformation
                for order in new_orders:
                    order_id, user_id, product_id, qty, price, created_at, status = order

                    cursor.execute("""
                        INSERT INTO order_events
                        (order_id, user_id, product_id, quantity, revenue, event_date, status)
                        VALUES (%s, %s, %s, %s, %s, %s, %s)
                    """, (order_id, user_id, product_id, qty, price * qty,
                          created_at.date(), status))

                print(f"Synced {len(new_orders)} orders to analytics database")

                # Generate summary report
                cursor.execute("""
                    SELECT
                        event_date,
                        COUNT(*) as orders,
                        SUM(revenue) as daily_revenue
                    FROM order_events
                    WHERE event_date >= %s
                    GROUP BY event_date
                    ORDER BY event_date
                """, (last_sync.date(),))

                daily_stats = cursor.fetchall()
                print("\n📈 Daily Revenue Summary:")
                for date, orders, revenue in daily_stats:
                    print(f"  {date}: {orders} orders, ${revenue:.2f} revenue")
```

### Django Management Command Integration

```python
# management/commands/database_analysis.py
from django.core.management.base import BaseCommand
from dbcrust.django import connect, list_django_databases

class Command(BaseCommand):
    help = 'Analyze database performance and generate reports'

    def add_arguments(self, parser):
        parser.add_argument(
            '--database',
            default='default',
            help='Database alias to analyze'
        )
        parser.add_argument(
            '--slow-queries',
            action='store_true',
            help='Find slow queries'
        )

    def handle(self, *args, **options):
        database = options['database']

        self.stdout.write(f"Analyzing database: {database}")

        # List all databases
        databases = list_django_databases()
        self.stdout.write("\nAvailable databases:")
        for alias, info in databases.items():
            status = self.style.SUCCESS("✅") if info['supported'] else self.style.ERROR("❌")
            self.stdout.write(f"  {alias}: {info['engine_type']} {status}")

        # Analyze specified database
        with connect(database) as connection:
            server_info = connection.get_server_info()
            self.stdout.write(
                self.style.SUCCESS(f"\nConnected to: {server_info.database_type} {server_info.version}")
            )

            with connection.cursor() as cursor:
                if options['slow_queries']:
                    self.analyze_slow_queries(cursor)
                else:
                    self.analyze_database_stats(cursor)

    def analyze_slow_queries(self, cursor):
        """Find potentially slow queries based on table sizes."""
        self.stdout.write("🔍 Analyzing for potential slow queries...")

        # PostgreSQL-specific analysis
        cursor.execute("""
            SELECT
                schemaname,
                tablename,
                n_tup_ins + n_tup_upd + n_tup_del as total_operations,
                n_tup_ins as inserts,
                n_tup_upd as updates,
                n_tup_del as deletes,
                pg_size_pretty(pg_total_relation_size(schemaname||'.'||tablename)) as size
            FROM pg_stat_user_tables
            WHERE schemaname = 'public'
            ORDER BY total_operations DESC
            LIMIT 10
        """)

        stats = cursor.fetchall()
        self.stdout.write("\n📊 Most Active Tables:")
        for schema, table, ops, ins, upd, del_, size in stats:
            self.stdout.write(f"  {table}: {ops} ops ({size}) - I:{ins} U:{upd} D:{del_}")

    def analyze_database_stats(self, cursor):
        """Generate general database statistics."""
        self.stdout.write("📈 Database Statistics:")

        # Count tables
        cursor.execute("""
            SELECT COUNT(*) FROM information_schema.tables
            WHERE table_schema = 'public' AND table_type = 'BASE TABLE'
        """)
        table_count = cursor.fetchone()[0]

        self.stdout.write(f"  Tables: {table_count}")
```

## 🔧 Configuration

### Django Settings Integration

The Django helper automatically works with your existing `DATABASES` configuration:

```python
# settings.py
DATABASES = {
    'default': {
        'ENGINE': 'django.db.backends.postgresql',
        'NAME': 'myapp_prod',
        'USER': 'myapp_user',
        'PASSWORD': 'secure_password',
        'HOST': 'db.example.com',
        'PORT': '5432',
    },
    'analytics': {
        'ENGINE': 'django.db.backends.postgresql',
        'NAME': 'analytics_warehouse',
        'USER': 'analyst',
        'PASSWORD': 'analyst_password',
        'HOST': 'analytics.example.com',
        'PORT': '5432',
    },
    'cache': {
        'ENGINE': 'django.db.backends.sqlite3',
        'NAME': BASE_DIR / 'cache.sqlite3',
    }
}

# DBCrust automatically discovers and converts these to connection URLs:
# default:   postgres://myapp_user:secure_password@db.example.com:5432/myapp_prod
# analytics: postgres://analyst:analyst_password@analytics.example.com:5432/analytics_warehouse
# cache:     sqlite:///path/to/cache.sqlite3
```

### Supported Database Engines

- **PostgreSQL**: `django.db.backends.postgresql`
- **MySQL/MariaDB**: `django.db.backends.mysql`
- **SQLite**: `django.db.backends.sqlite3`

### Connection Caching

By default, connections are cached per thread to improve performance:

```python
# Enable/disable connection caching
with connect(cache_connections=True) as conn:  # Default
    pass

# Clear all cached connections
from dbcrust.django import clear_connection_cache
clear_connection_cache()
```

## 🚨 Error Handling

The Django helper provides specific error types for better error handling:

```python
from dbcrust.django import connect, DjangoConnectionError
from dbcrust.django.utils import DatabaseConfigurationError, UnsupportedDatabaseError

def safe_database_operation():
    try:
        with connect("analytics") as connection:
            with connection.cursor() as cursor:
                cursor.execute("SELECT COUNT(*) FROM events")
                return cursor.fetchone()[0]

    except DjangoConnectionError as e:
        print(f"Django connection error: {e}")

    except UnsupportedDatabaseError as e:
        print(f"Database not supported: {e}")

    except DatabaseConfigurationError as e:
        print(f"Configuration error: {e}")

    except Exception as e:
        print(f"Unexpected error: {e}")

    return None
```

## 🧪 Testing

### Test Database Support

The Django helper works seamlessly with Django's test databases:

```python
# tests.py
from django.test import TestCase
from dbcrust.django import connect

class DatabaseIntegrationTestCase(TestCase):
    def test_user_analytics(self):
        """Test analytics queries on test database."""

        # Create test data
        from django.contrib.auth.models import User
        User.objects.create_user('testuser', 'test@example.com')

        # Test with DBCrust
        with connect() as connection:
            with connection.cursor() as cursor:
                cursor.execute("""
                    SELECT COUNT(*) FROM auth_user WHERE username = %s
                """, ('testuser',))

                user_count = cursor.fetchone()[0]
                self.assertEqual(user_count, 1)
```

## 🔍 Debugging

### Connection Information

Get detailed information about your Django database connections:

```python
from dbcrust.django import get_database_info, list_django_databases

# Get info for specific database
info = get_database_info('default')
print(f"Host: {info['host']}")
print(f"Database: {info['database_name']}")
print(f"Server: {info['server_type']} {info['server_version']}")

# List all databases
databases = list_django_databases()
for alias, db_info in databases.items():
    print(f"{alias}: {db_info['engine_type']} ({'supported' if db_info['supported'] else 'not supported'})")
```

## 📚 See Also

- **[Python API Overview](/dbcrust/python-api/overview/)** - General Python API patterns
- **[Examples & Use Cases](/dbcrust/python-api/examples/)** - More integration examples
- **[Error Handling](/dbcrust/python-api/error-handling/)** - Comprehensive error handling guide
