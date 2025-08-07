# Examples & Use Cases

This guide provides real-world examples of integrating DBCrust into Python applications, from simple scripts to complex data pipelines and monitoring systems.

## 🔬 Data Analysis & Science

### Pandas Integration

```python
import dbcrust
import pandas as pd
import json

class DatabaseAnalyzer:
    def __init__(self, connection_url):
        self.connection_url = connection_url

    def query_to_dataframe(self, query):
        """Convert DBCrust query results to pandas DataFrame"""
        result = dbcrust.run_with_url(
            self.connection_url,
            ["-o", "json", "-c", query]
        )
        data = json.loads(result)
        return pd.DataFrame(data)

    def monthly_revenue_analysis(self):
        """Analyze monthly revenue trends"""
        df = self.query_to_dataframe("""
            SELECT
                DATE_TRUNC('month', created_at) as month,
                COUNT(*) as order_count,
                SUM(amount) as total_revenue,
                AVG(amount) as avg_order_value
            FROM orders
            WHERE created_at >= '2024-01-01'
            GROUP BY month
            ORDER BY month
        """)

        # Calculate growth rates
        df['revenue_growth'] = df['total_revenue'].pct_change()
        df['order_growth'] = df['order_count'].pct_change()

        return df

# Usage
analyzer = DatabaseAnalyzer("postgres://analyst@warehouse/sales")
revenue_df = analyzer.monthly_revenue_analysis()

print("Monthly Revenue Analysis:")
print(revenue_df.to_string())

# Plot with matplotlib
import matplotlib.pyplot as plt
plt.figure(figsize=(12, 6))
plt.plot(revenue_df['month'], revenue_df['total_revenue'])
plt.title('Monthly Revenue Trend')
plt.xticks(rotation=45)
plt.show()
```

### Jupyter Notebook Integration

```python
# Setup notebook helper functions
import dbcrust
import json
import pandas as pd

CONNECTION = "postgres://analyst@warehouse/analytics"

def q(query):
    """Quick query function for notebooks"""
    result = dbcrust.run_with_url(CONNECTION, ["-o", "json", "-c", query])
    return pd.DataFrame(json.loads(result))

def show_tables():
    """Show all tables"""
    return dbcrust.run_command(CONNECTION, "\\dt")

def describe(table_name):
    """Describe table structure"""
    return dbcrust.run_command(CONNECTION, f"\\d {table_name}")

# Now use throughout notebook
show_tables()  # See available tables
describe("orders")  # Understand table structure

# Quick analysis
user_stats = q("SELECT status, COUNT(*) as count FROM users GROUP BY status")
user_stats.plot.bar(x='status', y='count')
```

## 🏭 ETL Pipelines & Data Engineering

### Multi-Database ETL

```python
import dbcrust
import json
import logging
from datetime import datetime, timedelta

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

class ETLPipeline:
    def __init__(self, source_url, target_url):
        self.source_url = source_url
        self.target_url = target_url

    def extract_incremental_users(self, hours_back=24):
        """Extract users modified in last N hours"""
        cutoff = (datetime.now() - timedelta(hours=hours_back)).strftime('%Y-%m-%d %H:%M:%S')

        query = f"""
        SELECT
            id,
            email,
            first_name,
            last_name,
            status,
            created_at,
            updated_at
        FROM users
        WHERE updated_at >= '{cutoff}'
        ORDER BY updated_at
        """

        logger.info(f"Extracting users updated since {cutoff}")
        result = dbcrust.run_with_url(
            self.source_url,
            ["-o", "json", "-c", query]
        )

        users = json.loads(result)
        logger.info(f"Extracted {len(users)} users")
        return users

    def transform_user_data(self, users):
        """Transform user data for warehouse"""
        transformed = []
        for user in users:
            transformed.append({
                'source_user_id': user['id'],
                'email': user['email'],
                'full_name': f"{user['first_name']} {user['last_name']}",
                'is_active': user['status'] == 'active',
                'source_created_at': user['created_at'],
                'source_updated_at': user['updated_at'],
                'etl_processed_at': datetime.now().isoformat()
            })
        return transformed

    def load_users(self, transformed_users):
        """Load users into data warehouse"""
        logger.info(f"Loading {len(transformed_users)} users into warehouse")

        for user in transformed_users:
            query = f"""
            INSERT INTO warehouse_users (
                source_user_id, email, full_name, is_active,
                source_created_at, source_updated_at, etl_processed_at
            ) VALUES (
                {user['source_user_id']},
                '{user['email']}',
                '{user['full_name']}',
                {user['is_active']},
                '{user['source_created_at']}',
                '{user['source_updated_at']}',
                '{user['etl_processed_at']}'
            )
            ON CONFLICT (source_user_id) DO UPDATE SET
                email = EXCLUDED.email,
                full_name = EXCLUDED.full_name,
                is_active = EXCLUDED.is_active,
                source_updated_at = EXCLUDED.source_updated_at,
                etl_processed_at = EXCLUDED.etl_processed_at
            """

            dbcrust.run_command(self.target_url, query)

    def run_pipeline(self):
        """Execute complete ETL pipeline"""
        try:
            # Extract
            users = self.extract_incremental_users()
            if not users:
                logger.info("No new users to process")
                return

            # Transform
            transformed = self.transform_user_data(users)

            # Load
            self.load_users(transformed)

            logger.info("ETL pipeline completed successfully")

        except Exception as e:
            logger.error(f"ETL pipeline failed: {e}")
            raise

# Usage
etl = ETLPipeline(
    source_url="mysql://reader@prod-db/app",
    target_url="postgres://writer@warehouse/analytics"
)

# Run pipeline
etl.run_pipeline()
```

### Data Validation Pipeline

```python
import dbcrust
import json
from dataclasses import dataclass
from typing import List

@dataclass
class ValidationResult:
    table: str
    check: str
    passed: bool
    message: str
    count: int = 0

class DataQualityValidator:
    def __init__(self, connection_url):
        self.connection_url = connection_url
        self.results = []

    def validate_not_null(self, table, column):
        """Validate no null values in important columns"""
        result = dbcrust.run_with_url(
            self.connection_url,
            ["-o", "json", "-c", f"SELECT COUNT(*) as null_count FROM {table} WHERE {column} IS NULL"]
        )

        null_count = json.loads(result)[0]['null_count']
        passed = null_count == 0

        self.results.append(ValidationResult(
            table=table,
            check=f"not_null_{column}",
            passed=passed,
            message=f"Found {null_count} null values in {table}.{column}",
            count=null_count
        ))

    def validate_unique(self, table, column):
        """Validate column uniqueness"""
        result = dbcrust.run_with_url(
            self.connection_url,
            ["-o", "json", "-c", f"""
                SELECT COUNT(*) - COUNT(DISTINCT {column}) as duplicate_count
                FROM {table}
                WHERE {column} IS NOT NULL
            """]
        )

        duplicate_count = json.loads(result)[0]['duplicate_count']
        passed = duplicate_count == 0

        self.results.append(ValidationResult(
            table=table,
            check=f"unique_{column}",
            passed=passed,
            message=f"Found {duplicate_count} duplicate values in {table}.{column}",
            count=duplicate_count
        ))

    def validate_referential_integrity(self, child_table, child_column, parent_table, parent_column):
        """Validate foreign key constraints"""
        result = dbcrust.run_with_url(
            self.connection_url,
            ["-o", "json", "-c", f"""
                SELECT COUNT(*) as orphan_count
                FROM {child_table} c
                LEFT JOIN {parent_table} p ON c.{child_column} = p.{parent_column}
                WHERE c.{child_column} IS NOT NULL AND p.{parent_column} IS NULL
            """]
        )

        orphan_count = json.loads(result)[0]['orphan_count']
        passed = orphan_count == 0

        self.results.append(ValidationResult(
            table=child_table,
            check=f"foreign_key_{child_column}",
            passed=passed,
            message=f"Found {orphan_count} orphaned records in {child_table}.{child_column}",
            count=orphan_count
        ))

    def run_all_validations(self):
        """Run complete data quality suite"""
        # Core validations
        self.validate_not_null("users", "email")
        self.validate_unique("users", "email")
        self.validate_not_null("orders", "user_id")
        self.validate_referential_integrity("orders", "user_id", "users", "id")

        # Generate report
        return self.generate_report()

    def generate_report(self):
        """Generate validation report"""
        passed = sum(1 for r in self.results if r.passed)
        failed = len(self.results) - passed

        report = {
            'summary': {
                'total_checks': len(self.results),
                'passed': passed,
                'failed': failed,
                'success_rate': passed / len(self.results) * 100
            },
            'failures': [r for r in self.results if not r.passed],
            'all_results': self.results
        }

        return report

# Usage
validator = DataQualityValidator("postgres://reader@warehouse/analytics")
report = validator.run_all_validations()

print(f"Data Quality Report:")
print(f"Success Rate: {report['summary']['success_rate']:.1f}%")
print(f"Checks Passed: {report['summary']['passed']}/{report['summary']['total_checks']}")

if report['failures']:
    print("\nFailures:")
    for failure in report['failures']:
        print(f"  ❌ {failure.table}: {failure.message}")
```

## 🖥️ System Monitoring & Administration

### Database Health Monitor

```python
import dbcrust
import json
import time
from datetime import datetime
import smtplib
from email.mime.text import MIMEText

class DatabaseMonitor:
    def __init__(self, databases, alert_email=None):
        self.databases = databases
        self.alert_email = alert_email
        self.alert_history = {}

    def check_database_health(self, name, connection_url):
        """Comprehensive database health check"""
        try:
            start_time = time.time()

            # Basic connectivity
            version = dbcrust.run_command(connection_url, "SELECT version()")

            # Connection count
            conn_result = dbcrust.run_with_url(
                connection_url,
                ["-o", "json", "-c", "SELECT COUNT(*) as active FROM pg_stat_activity"]
            )
            active_connections = json.loads(conn_result)[0]['active']

            # Database size
            size_result = dbcrust.run_with_url(
                connection_url,
                ["-o", "json", "-c", "SELECT pg_size_pretty(pg_database_size(current_database())) as size"]
            )
            database_size = json.loads(size_result)[0]['size']

            # Slow queries
            slow_query_result = dbcrust.run_with_url(
                connection_url,
                ["-o", "json", "-c", """
                    SELECT COUNT(*) as slow_queries
                    FROM pg_stat_activity
                    WHERE state = 'active'
                    AND query_start < NOW() - INTERVAL '30 seconds'
                """]
            )
            slow_queries = json.loads(slow_query_result)[0]['slow_queries']

            response_time = (time.time() - start_time) * 1000  # ms

            return {
                'status': 'healthy',
                'response_time_ms': response_time,
                'active_connections': active_connections,
                'database_size': database_size,
                'slow_queries': slow_queries,
                'version': version.strip().split('\n')[0]
            }

        except Exception as e:
            return {
                'status': 'unhealthy',
                'error': str(e),
                'timestamp': datetime.now().isoformat()
            }

    def check_all_databases(self):
        """Monitor all configured databases"""
        results = {}
        alerts = []

        for name, connection_url in self.databases.items():
            print(f"Checking {name}...")
            result = self.check_database_health(name, connection_url)
            results[name] = result

            # Check for alert conditions
            if result['status'] == 'unhealthy':
                alerts.append(f"🔴 {name}: {result['error']}")
            elif result.get('response_time_ms', 0) > 5000:
                alerts.append(f"🟡 {name}: Slow response ({result['response_time_ms']:.0f}ms)")
            elif result.get('slow_queries', 0) > 5:
                alerts.append(f"🟡 {name}: {result['slow_queries']} slow queries")
            else:
                print(f"  ✅ {name}: Healthy ({result.get('response_time_ms', 0):.0f}ms)")

        if alerts:
            alert_message = "\n".join(alerts)
            print(f"\n⚠️ Alerts:\n{alert_message}")

            if self.alert_email:
                self.send_alert(alert_message)

        return results

    def send_alert(self, message):
        """Send email alert (basic implementation)"""
        # Implementation depends on your email setup
        print(f"📧 Alert would be sent to {self.alert_email}:")
        print(message)

    def continuous_monitoring(self, interval_minutes=5):
        """Run continuous monitoring"""
        print(f"Starting continuous monitoring (checking every {interval_minutes} minutes)")

        while True:
            print(f"\n--- Health Check at {datetime.now().strftime('%Y-%m-%d %H:%M:%S')} ---")
            self.check_all_databases()

            time.sleep(interval_minutes * 60)

# Usage
databases = {
    "production": "postgres://monitor@prod-db/app",
    "analytics": "postgres://monitor@analytics-db/warehouse",
    "cache": "redis://monitor@cache-db/sessions"
}

monitor = DatabaseMonitor(databases, alert_email="ops@company.com")

# One-time check
monitor.check_all_databases()

# Continuous monitoring (run in production)
# monitor.continuous_monitoring(interval_minutes=5)
```

### Backup Automation

```python
import dbcrust
import os
import gzip
from datetime import datetime
import subprocess

class DatabaseBackupManager:
    def __init__(self, databases, backup_dir="/backups"):
        self.databases = databases
        self.backup_dir = backup_dir
        os.makedirs(backup_dir, exist_ok=True)

    def backup_database(self, name, connection_url):
        """Create database backup"""
        timestamp = datetime.now().strftime('%Y%m%d_%H%M%S')
        backup_file = f"{self.backup_dir}/{name}_{timestamp}.sql.gz"

        print(f"Backing up {name} to {backup_file}")

        try:
            # Get all table data
            tables_result = dbcrust.run_with_url(
                connection_url,
                ["-o", "json", "-c", "SELECT tablename FROM pg_tables WHERE schemaname = 'public'"]
            )

            tables = json.loads(tables_result)

            with gzip.open(backup_file, 'wt') as f:
                # Write schema
                f.write("-- Database backup created at " + timestamp + "\n")
                f.write("BEGIN;\n\n")

                for table_info in tables:
                    table_name = table_info['tablename']
                    print(f"  Backing up table: {table_name}")

                    # Get table schema
                    schema = dbcrust.run_command(connection_url, f"\\d {table_name}")
                    f.write(f"-- Table: {table_name}\n")
                    f.write(schema + "\n\n")

                    # Get table data as CSV
                    data = dbcrust.run_with_url(
                        connection_url,
                        ["-o", "csv", "-c", f"SELECT * FROM {table_name}"]
                    )

                    if data.strip():
                        f.write(f"-- Data for {table_name}\n")
                        f.write(f"COPY {table_name} FROM STDIN WITH CSV HEADER;\n")
                        f.write(data)
                        f.write("\\.\n\n")

                f.write("COMMIT;\n")

            backup_size = os.path.getsize(backup_file)
            print(f"  Backup complete: {backup_size:,} bytes")

            return {
                'status': 'success',
                'file': backup_file,
                'size_bytes': backup_size,
                'timestamp': timestamp
            }

        except Exception as e:
            print(f"  Backup failed: {e}")
            if os.path.exists(backup_file):
                os.remove(backup_file)

            return {
                'status': 'failed',
                'error': str(e),
                'timestamp': timestamp
            }

    def backup_all_databases(self):
        """Backup all configured databases"""
        results = {}

        for name, connection_url in self.databases.items():
            results[name] = self.backup_database(name, connection_url)

        # Summary
        successful = sum(1 for r in results.values() if r['status'] == 'success')
        total_size = sum(r.get('size_bytes', 0) for r in results.values())

        print(f"\nBackup Summary:")
        print(f"  Successful: {successful}/{len(results)}")
        print(f"  Total size: {total_size:,} bytes")

        return results

    def cleanup_old_backups(self, days_to_keep=7):
        """Clean up backups older than specified days"""
        cutoff_time = time.time() - (days_to_keep * 24 * 60 * 60)

        for filename in os.listdir(self.backup_dir):
            if filename.endswith('.sql.gz'):
                filepath = os.path.join(self.backup_dir, filename)
                if os.path.getmtime(filepath) < cutoff_time:
                    print(f"Removing old backup: {filename}")
                    os.remove(filepath)

# Usage
databases = {
    "production": "postgres://backup@prod-db/app",
    "analytics": "postgres://backup@analytics-db/warehouse"
}

backup_manager = DatabaseBackupManager(databases)

# Run backups
results = backup_manager.backup_all_databases()

# Cleanup old backups
backup_manager.cleanup_old_backups(days_to_keep=7)
```

## 🧪 Testing & Development

### Test Data Factory

```python
import dbcrust
import random
from faker import Faker
from datetime import datetime, timedelta

fake = Faker()

class TestDataFactory:
    def __init__(self, connection_url):
        self.connection_url = connection_url

    def create_test_users(self, count=100):
        """Create realistic test user data"""
        print(f"Creating {count} test users...")

        for i in range(count):
            user_data = {
                'name': fake.name(),
                'email': fake.email(),
                'username': fake.user_name(),
                'phone': fake.phone_number(),
                'address': fake.address().replace('\n', ', '),
                'company': fake.company(),
                'job_title': fake.job(),
                'created_at': fake.date_time_between(start_date='-2y', end_date='now'),
                'is_active': random.choice([True, True, True, False])  # 75% active
            }

            query = f"""
            INSERT INTO users (
                name, email, username, phone, address,
                company, job_title, created_at, is_active
            ) VALUES (
                '{user_data['name'].replace("'", "''")}',
                '{user_data['email']}',
                '{user_data['username']}',
                '{user_data['phone']}',
                '{user_data['address'].replace("'", "''")}',
                '{user_data['company'].replace("'", "''")}',
                '{user_data['job_title'].replace("'", "''")}',
                '{user_data['created_at']}',
                {user_data['is_active']}
            )
            """

            dbcrust.run_command(self.connection_url, query)

        print(f"✅ Created {count} test users")

    def create_test_orders(self, user_count=None):
        """Create test orders for existing users"""
        # Get user IDs
        user_result = dbcrust.run_with_url(
            self.connection_url,
            ["-o", "json", "-c", "SELECT id FROM users" + (f" LIMIT {user_count}" if user_count else "")]
        )

        user_ids = [u['id'] for u in json.loads(user_result)]
        order_count = len(user_ids) * random.randint(1, 5)  # 1-5 orders per user

        print(f"Creating {order_count} test orders for {len(user_ids)} users...")

        for i in range(order_count):
            order_data = {
                'user_id': random.choice(user_ids),
                'amount': round(random.uniform(10.0, 1000.0), 2),
                'status': random.choice(['pending', 'completed', 'completed', 'completed', 'cancelled']),
                'created_at': fake.date_time_between(start_date='-1y', end_date='now'),
                'product_name': fake.catch_phrase(),
                'quantity': random.randint(1, 5)
            }

            query = f"""
            INSERT INTO orders (
                user_id, amount, status, created_at, product_name, quantity
            ) VALUES (
                {order_data['user_id']},
                {order_data['amount']},
                '{order_data['status']}',
                '{order_data['created_at']}',
                '{order_data['product_name'].replace("'", "''")}',
                {order_data['quantity']}
            )
            """

            dbcrust.run_command(self.connection_url, query)

        print(f"✅ Created {order_count} test orders")

    def reset_test_data(self):
        """Clean and recreate all test data"""
        print("Resetting test data...")

        # Clear existing data
        dbcrust.run_command(self.connection_url, "TRUNCATE orders, users RESTART IDENTITY CASCADE")

        # Create fresh test data
        self.create_test_users(50)
        self.create_test_orders()

        print("✅ Test data reset complete")

# Usage
factory = TestDataFactory("postgres://test@localhost/test_db")

# Create test environment
factory.reset_test_data()

# Or create additional data
# factory.create_test_users(25)
# factory.create_test_orders()
```

### Performance Testing Framework

```python
import dbcrust
import time
import statistics
from concurrent.futures import ThreadPoolExecutor
import json

class PerformanceTester:
    def __init__(self, connection_url):
        self.connection_url = connection_url
        self.results = []

    def time_query(self, query, description="Query"):
        """Time a single query execution"""
        start_time = time.time()

        try:
            result = dbcrust.run_command(self.connection_url, query)
            duration = (time.time() - start_time) * 1000  # Convert to ms

            return {
                'description': description,
                'duration_ms': duration,
                'status': 'success',
                'result_length': len(result)
            }
        except Exception as e:
            duration = (time.time() - start_time) * 1000
            return {
                'description': description,
                'duration_ms': duration,
                'status': 'error',
                'error': str(e)
            }

    def run_load_test(self, query, concurrent_users=10, iterations=5):
        """Run concurrent load test"""
        print(f"Running load test: {concurrent_users} users, {iterations} iterations each")

        def run_iteration():
            return [self.time_query(query, "Load test query") for _ in range(iterations)]

        # Run concurrent tests
        with ThreadPoolExecutor(max_workers=concurrent_users) as executor:
            futures = [executor.submit(run_iteration) for _ in range(concurrent_users)]
            all_results = []

            for future in futures:
                all_results.extend(future.result())

        # Analyze results
        successful_runs = [r for r in all_results if r['status'] == 'success']
        durations = [r['duration_ms'] for r in successful_runs]

        if durations:
            stats = {
                'total_queries': len(all_results),
                'successful_queries': len(successful_runs),
                'failed_queries': len(all_results) - len(successful_runs),
                'avg_duration_ms': statistics.mean(durations),
                'median_duration_ms': statistics.median(durations),
                'min_duration_ms': min(durations),
                'max_duration_ms': max(durations),
                'p95_duration_ms': sorted(durations)[int(len(durations) * 0.95)] if len(durations) > 20 else max(durations)
            }

            print(f"Load Test Results:")
            print(f"  Success Rate: {stats['successful_queries']}/{stats['total_queries']} ({stats['successful_queries']/stats['total_queries']*100:.1f}%)")
            print(f"  Avg Duration: {stats['avg_duration_ms']:.2f}ms")
            print(f"  Median Duration: {stats['median_duration_ms']:.2f}ms")
            print(f"  P95 Duration: {stats['p95_duration_ms']:.2f}ms")
            print(f"  Min/Max: {stats['min_duration_ms']:.2f}ms / {stats['max_duration_ms']:.2f}ms")

            return stats
        else:
            print("❌ All queries failed")
            return None

    def benchmark_queries(self, query_suite):
        """Benchmark a suite of queries"""
        results = {}

        for name, query in query_suite.items():
            print(f"Benchmarking: {name}")
            times = []

            # Run query multiple times for stable results
            for i in range(5):
                result = self.time_query(query, name)
                if result['status'] == 'success':
                    times.append(result['duration_ms'])

            if times:
                results[name] = {
                    'avg_duration_ms': statistics.mean(times),
                    'min_duration_ms': min(times),
                    'max_duration_ms': max(times),
                    'runs': len(times)
                }
                print(f"  Average: {results[name]['avg_duration_ms']:.2f}ms")
            else:
                results[name] = {'status': 'failed'}
                print(f"  ❌ Failed")

        return results

# Usage
tester = PerformanceTester("postgres://test@localhost/test_db")

# Benchmark individual queries
query_suite = {
    "simple_count": "SELECT COUNT(*) FROM users",
    "user_lookup": "SELECT * FROM users WHERE email = 'test@example.com'",
    "join_query": "SELECT u.name, COUNT(o.id) FROM users u LEFT JOIN orders o ON u.id = o.user_id GROUP BY u.id, u.name",
    "complex_analytics": """
        SELECT DATE_TRUNC('month', o.created_at) as month,
               u.company,
               COUNT(o.id) as orders,
               SUM(o.amount) as revenue
        FROM orders o
        JOIN users u ON o.user_id = u.id
        WHERE o.created_at >= '2024-01-01'
        GROUP BY month, u.company
        ORDER BY month, revenue DESC
    """
}

benchmark_results = tester.benchmark_queries(query_suite)

# Run load test on most critical query
load_test_results = tester.run_load_test(
    "SELECT * FROM users WHERE is_active = true LIMIT 10",
    concurrent_users=5,
    iterations=10
)
```

## 📚 See Also

- **[Python API Overview](/dbcrust/python-api/overview/)** - API introduction and patterns
- **[Direct Execution](/dbcrust/python-api/direct-execution/)** - Simple function-based API
- **[Client Classes](/dbcrust/python-api/client-classes/)** - Advanced client APIs

---

<div align="center">
    <strong>Ready to build powerful database applications?</strong><br>
    <a href="/dbcrust/python-api/overview/" class="md-button md-button--primary">API Overview</a>
    <a href="/dbcrust/django-analyzer/" class="md-button">Django Integration</a>
</div>
