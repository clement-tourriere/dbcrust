# Python Error Handling Guide

DBCrust provides a comprehensive exception hierarchy for robust error handling in Python applications. This guide covers all exception types, common patterns, and best practices.

## Exception Hierarchy

DBCrust uses a hierarchical exception system that allows you to catch errors at different levels of specificity:

```python
DbcrustError                    # Base exception for all DBCrust errors
├── DbcrustConnectionError      # Database connection failures
├── DbcrustCommandError         # Command execution errors
├── DbcrustConfigError          # Configuration issues
└── DbcrustArgumentError        # Invalid arguments
```

## Importing Exceptions

All exception classes are available from the main dbcrust module:

```python
from dbcrust import (
    DbcrustError,
    DbcrustConnectionError,
    DbcrustCommandError,
    DbcrustConfigError,
    DbcrustArgumentError
)
```

## Exception Types

### DbcrustConnectionError

Raised when database connection fails. Common causes:
- Invalid host or port
- Network connectivity issues
- Authentication failures
- Database server unavailable
- SSL/TLS configuration problems

```python
try:
    result = dbcrust.run_with_url("postgres://user@invalid-host/db")
except dbcrust.DbcrustConnectionError as e:
    print(f"Connection failed: {e}")
    # Error message includes specific details like:
    # "Failed to connect to database: failed to lookup address information"
```

### DbcrustCommandError

Raised when command execution fails. Common causes:
- SQL syntax errors
- Table/column doesn't exist
- Permission denied
- Invalid backslash commands
- Transaction rollback

```python
try:
    result = dbcrust.run_command(["dbcrust", db_url, "-c", "SELECT * FROM nonexistent"])
except dbcrust.DbcrustCommandError as e:
    print(f"Query failed: {e}")
    # Error includes the specific database error message
```

### DbcrustConfigError

Raised for configuration-related issues:
- Missing configuration file
- Invalid configuration values
- Permission issues with config directory
- Failed to save configuration

```python
try:
    config = dbcrust.PyConfig()
    config.save()
except dbcrust.DbcrustConfigError as e:
    print(f"Config error: {e}")
```

### DbcrustArgumentError

Raised for invalid command-line arguments:
- Unknown flags or options
- Missing required arguments
- Invalid argument values
- Conflicting arguments

```python
try:
    result = dbcrust.run_command(["dbcrust", "--invalid-flag"])
except dbcrust.DbcrustArgumentError as e:
    print(f"Invalid arguments: {e}")
```

## Common Patterns

### Basic Error Handling

```python
import dbcrust

def execute_query(connection_url, query):
    """Execute a query with basic error handling"""
    try:
        return dbcrust.run_command([connection_url, "-c", query])
    except dbcrust.DbcrustConnectionError as e:
        print(f"Failed to connect: {e}")
        return None
    except dbcrust.DbcrustCommandError as e:
        print(f"Query failed: {e}")
        return None
    except dbcrust.DbcrustError as e:
        # Catch any other DBCrust error
        print(f"DBCrust error: {e}")
        return None
```

### Retry Pattern

```python
import time
import dbcrust

def execute_with_retry(connection_url, query, max_retries=3, delay=1):
    """Execute query with exponential backoff retry"""
    for attempt in range(max_retries):
        try:
            return dbcrust.run_command([connection_url, "-c", query])
        except dbcrust.DbcrustConnectionError as e:
            if attempt == max_retries - 1:
                raise  # Re-raise on final attempt

            wait_time = delay * (2 ** attempt)
            print(f"Connection failed, retrying in {wait_time}s...")
            time.sleep(wait_time)
        except dbcrust.DbcrustCommandError:
            # Don't retry command errors - they likely won't succeed
            raise
```

### Fallback Pattern

```python
def query_with_fallback(primary_url, backup_url, query):
    """Try primary database, fall back to backup if needed"""
    try:
        return dbcrust.run_command([primary_url, "-c", query])
    except dbcrust.DbcrustConnectionError as e:
        print(f"Primary failed: {e}, trying backup...")
        try:
            return dbcrust.run_command([backup_url, "-c", query])
        except dbcrust.DbcrustConnectionError as e2:
            print(f"Backup also failed: {e2}")
            raise  # Both failed
```

### Context Manager Pattern

```python
class DatabaseConnection:
    """Context manager for database operations"""

    def __init__(self, connection_url):
        self.connection_url = connection_url
        self.connected = False

    def __enter__(self):
        try:
            # Test connection
            dbcrust.run_command([self.connection_url, "-c", "SELECT 1"])
            self.connected = True
            return self
        except dbcrust.DbcrustConnectionError as e:
            raise RuntimeError(f"Failed to establish connection: {e}")

    def execute(self, query):
        if not self.connected:
            raise RuntimeError("Not connected")
        return dbcrust.run_command([self.connection_url, "-c", query])

    def __exit__(self, exc_type, exc_val, exc_tb):
        # Cleanup if needed
        self.connected = False

# Usage
with DatabaseConnection("postgres://user@host/db") as db:
    result = db.execute("SELECT * FROM users")
```

## Advanced Error Handling

### Error Classification

```python
def classify_error(exception):
    """Classify error for appropriate response"""
    error_msg = str(exception).lower()

    if isinstance(exception, dbcrust.DbcrustConnectionError):
        if "authentication" in error_msg or "password" in error_msg:
            return "AUTH_ERROR"
        elif "lookup" in error_msg or "resolve" in error_msg:
            return "DNS_ERROR"
        elif "refused" in error_msg:
            return "CONNECTION_REFUSED"
        elif "timeout" in error_msg:
            return "TIMEOUT"
        else:
            return "CONNECTION_ERROR"

    elif isinstance(exception, dbcrust.DbcrustCommandError):
        if "syntax" in error_msg:
            return "SYNTAX_ERROR"
        elif "permission" in error_msg or "denied" in error_msg:
            return "PERMISSION_ERROR"
        elif "exist" in error_msg:
            return "NOT_FOUND"
        else:
            return "COMMAND_ERROR"

    return "UNKNOWN_ERROR"

# Usage
try:
    result = dbcrust.run_with_url("postgres://user@host/db")
except dbcrust.DbcrustError as e:
    error_type = classify_error(e)

    if error_type == "AUTH_ERROR":
        print("Please check your credentials")
    elif error_type == "DNS_ERROR":
        print("Cannot resolve database hostname")
    elif error_type == "TIMEOUT":
        print("Connection timed out, server may be overloaded")
    # ... handle other error types
```

### Logging and Monitoring

```python
import logging
import dbcrust

logger = logging.getLogger(__name__)

def monitored_query(connection_url, query):
    """Execute query with logging and monitoring"""
    try:
        logger.info(f"Executing query on {connection_url}")
        result = dbcrust.run_command([connection_url, "-c", query])
        logger.info("Query executed successfully")
        return result

    except dbcrust.DbcrustConnectionError as e:
        logger.error(f"Connection error: {e}", exc_info=True)
        # Send alert to monitoring system
        send_alert("database_connection_failed", str(e))
        raise

    except dbcrust.DbcrustCommandError as e:
        logger.warning(f"Query failed: {e}")
        # Log query for debugging
        logger.debug(f"Failed query: {query}")
        raise

    except Exception as e:
        logger.critical(f"Unexpected error: {e}", exc_info=True)
        raise
```

## PyDatabase Class Errors

The `PyDatabase` class methods also raise specific exceptions:

```python
from dbcrust import PyDatabase, DbcrustConnectionError, DbcrustCommandError

try:
    # Connection errors
    db = PyDatabase("invalid_host", 5432, "user", "pass", "database")
except DbcrustConnectionError as e:
    print(f"Failed to connect: {e}")

# Once connected, method calls can raise command errors
db = PyDatabase("localhost", 5432, "user", "pass", "database")

try:
    # Command errors
    result = db.execute("INVALID SQL")
except DbcrustCommandError as e:
    print(f"Query failed: {e}")

try:
    # List operations can also fail
    tables = db.list_tables()
except DbcrustCommandError as e:
    print(f"Failed to list tables: {e}")
```

## Best Practices

### 1. Use Specific Exception Types

```python
# ✅ Good - Specific handling for each error type
try:
    result = dbcrust.run_with_url(url)
except dbcrust.DbcrustConnectionError as e:
    handle_connection_error(e)
except dbcrust.DbcrustCommandError as e:
    handle_command_error(e)

# ❌ Avoid - Too generic
try:
    result = dbcrust.run_with_url(url)
except Exception as e:
    print(f"Error: {e}")
```

### 2. Let Exceptions Propagate When Appropriate

```python
def get_user_count(connection_url):
    """Get user count, let caller handle errors"""
    # Let exceptions propagate to caller
    result = dbcrust.run_command([connection_url, "-c", "SELECT COUNT(*) FROM users"])
    return parse_result(result)

# Caller decides how to handle errors
try:
    count = get_user_count(url)
except dbcrust.DbcrustError as e:
    # Handle at appropriate level
    log_error(e)
    return default_value
```

### 3. Provide Context in Error Messages

```python
def process_batch(connection_url, batch_id):
    try:
        result = dbcrust.run_command([
            connection_url, "-c",
            f"UPDATE batches SET status='processing' WHERE id={batch_id}"
        ])
        return result
    except dbcrust.DbcrustError as e:
        # Add context to help debugging
        raise RuntimeError(f"Failed to process batch {batch_id}: {e}") from e
```

### 4. Use Exit Codes Appropriately

```python
import sys
import dbcrust

def main():
    try:
        exit_code = dbcrust.run_with_url("postgres://user@host/db")
        return exit_code
    except dbcrust.DbcrustConnectionError as e:
        print(f"Connection error: {e}", file=sys.stderr)
        return 1  # General error
    except dbcrust.DbcrustArgumentError as e:
        print(f"Invalid arguments: {e}", file=sys.stderr)
        return 2  # Misuse of shell command
    except KeyboardInterrupt:
        print("\nInterrupted", file=sys.stderr)
        return 130  # Script terminated by Control-C

if __name__ == "__main__":
    sys.exit(main())
```

## Migration from Generic Errors

If you have existing code using generic exception handling, here's how to migrate:

### Before (Generic)
```python
try:
    result = run_command(args)
except Exception as e:
    if "connection" in str(e).lower():
        handle_connection_error()
    elif "syntax" in str(e).lower():
        handle_syntax_error()
    else:
        handle_generic_error()
```

### After (Specific)
```python
try:
    result = run_command(args)
except DbcrustConnectionError as e:
    handle_connection_error(e)
except DbcrustCommandError as e:
    if "syntax" in str(e).lower():
        handle_syntax_error(e)
    else:
        handle_command_error(e)
except DbcrustError as e:
    handle_generic_error(e)
```

## Testing Error Handling

When testing code that uses DBCrust, you can mock exceptions:

```python
import unittest
from unittest.mock import patch, MagicMock
import dbcrust

class TestErrorHandling(unittest.TestCase):

    @patch('dbcrust.run_command')
    def test_connection_error_handling(self, mock_run):
        # Mock a connection error
        mock_run.side_effect = dbcrust.DbcrustConnectionError(
            "Failed to connect to database"
        )

        # Test your error handling
        with self.assertRaises(dbcrust.DbcrustConnectionError):
            result = your_function_using_dbcrust()

    @patch('dbcrust.run_command')
    def test_retry_on_connection_error(self, mock_run):
        # Fail twice, then succeed
        mock_run.side_effect = [
            dbcrust.DbcrustConnectionError("Connection failed"),
            dbcrust.DbcrustConnectionError("Connection failed"),
            "Success!"
        ]

        result = your_retry_function()
        self.assertEqual(result, "Success!")
        self.assertEqual(mock_run.call_count, 3)
```

## Summary

DBCrust's exception hierarchy provides:

- **Clear error classification** - Know exactly what went wrong
- **Targeted error handling** - Handle specific errors appropriately
- **Better debugging** - Detailed error messages with context
- **Robust applications** - Implement retry logic, fallbacks, and recovery
- **Clean code** - No string parsing or guessing error types

Always use the most specific exception type for your use case, and let exceptions propagate to the appropriate level for handling.
