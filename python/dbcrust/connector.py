"""
DBCrust Database Connector

Generic database connection interface supporting PostgreSQL, MySQL, and SQLite
with a mysql.connector-like API.
"""

from typing import Optional, Dict, Any, Union
from urllib.parse import urlparse, urlunparse
from dbcrust._internal import (
    py_connect,
    PyConnection as _PyConnection,
    PyCursor as _PyCursor,
    PyServerInfo,
    PyRow,
    PyResultSet,
    DbcrustConnectionError,
    DbcrustCommandError,
    DbcrustConfigError,
    DbcrustArgumentError
)


def connect(
    # URL-based connection
    url: Optional[str] = None,

    # Individual connection parameters
    host: Optional[str] = None,
    port: Optional[int] = None,
    user: Optional[str] = None,
    username: Optional[str] = None,  # Alias for user
    password: Optional[str] = None,
    database: Optional[str] = None,
    db: Optional[str] = None,  # Alias for database

    # Connection options
    timeout: Optional[float] = None,
    auto_commit: Optional[bool] = None,

    # Database-specific options
    **kwargs
) -> 'Connection':
    """
    Connect to a database using either URL or individual parameters.

    Supports PostgreSQL, MySQL, and SQLite with automatic detection.

    Args:
        url: Full database connection URL (e.g., 'postgres://user:pass@host:5432/db')
        host: Database server hostname
        port: Database server port
        user: Database username (or use 'username')
        password: Database password
        database: Database name (or use 'db')
        timeout: Connection timeout in seconds
        auto_commit: Enable auto-commit mode (default: True)
        **kwargs: Additional database-specific options

    Returns:
        Connection: Database connection object with context manager support

    Examples:
        # URL-based connection
        conn = connect("postgres://user:pass@localhost:5432/mydb")
        conn = connect("mysql://user:pass@localhost:3306/mydb")
        conn = connect("sqlite:///path/to/database.db")

        # Parameter-based connection
        conn = connect(host="localhost", port=5432, user="postgres",
                      password="secret", database="mydb", timeout=30)

        # With context manager
        with connect("postgres://user@host/db") as connection:
            cursor = connection.cursor()
            cursor.execute("SELECT * FROM users")
            results = cursor.fetchall()
    """

    # Handle username/user aliases
    if username and not user:
        user = username

    # Handle database/db aliases
    if db and not database:
        database = db

    # Build connection URL if individual parameters provided
    if url is None:
        if not host:
            raise DbcrustArgumentError("Either 'url' or 'host' parameter is required")

        # Detect database type from port if not specified
        if port is None:
            port = 5432  # Default to PostgreSQL

        # Determine database type from port or other hints
        if port == 3306 or 'mysql' in kwargs.get('driver', '').lower():
            scheme = 'mysql'
            if port is None:
                port = 3306
        elif port == 5432 or 'postgres' in kwargs.get('driver', '').lower():
            scheme = 'postgres'
            if port is None:
                port = 5432
        else:
            scheme = 'postgres'  # Default

        # Handle SQLite special case
        if database and database.endswith('.db'):
            url = f"sqlite:///{database}"
        else:
            # Build standard URL
            auth = f"{user}:{password}" if user and password else user or ""
            netloc = f"{auth}@{host}:{port}" if auth else f"{host}:{port}"
            url = f"{scheme}://{netloc}/{database or ''}"

    # Create the Rust-side connection
    try:
        rust_connection = py_connect(url, timeout, auto_commit)
    except Exception as e:
        # Re-raise with appropriate exception type
        if "connection" in str(e).lower() or "connect" in str(e).lower():
            raise DbcrustConnectionError(str(e))
        else:
            raise DbcrustArgumentError(str(e))

    # Wrap in Python Connection class
    return Connection(rust_connection, url)


class Connection:
    """
    Database connection with context manager support.

    Provides a mysql.connector-like interface for database operations.
    """

    def __init__(self, rust_connection: _PyConnection, connection_url: str):
        self._connection = rust_connection
        self._connection_url = connection_url
        self._closed = False

    def __enter__(self) -> 'Connection':
        """Context manager entry"""
        return self

    def __exit__(self, exc_type, exc_value, traceback) -> None:
        """Context manager exit"""
        self.close()

    def cursor(self) -> 'Cursor':
        """
        Create a new cursor for executing queries.

        Returns:
            Cursor: Database cursor for query execution
        """
        if self._closed:
            raise DbcrustConnectionError("Connection is closed")

        rust_cursor = self._connection.cursor()
        return Cursor(rust_cursor, self)

    def get_server_info(self) -> PyServerInfo:
        """
        Get database server information.

        Returns:
            PyServerInfo: Server metadata including version and capabilities
        """
        if self._closed:
            raise DbcrustConnectionError("Connection is closed")

        return self._connection.get_server_info()

    def execute_immediate(self, query: str) -> PyResultSet:
        """
        Execute a query immediately without a cursor.

        Args:
            query: SQL query to execute

        Returns:
            PyResultSet: Query results
        """
        if self._closed:
            raise DbcrustConnectionError("Connection is closed")

        return self._connection.execute_immediate(query)

    def commit(self):
        """
        Commit the current transaction.

        Note: Currently a no-op as transactions are handled automatically
        """
        # TODO: Implement transaction support
        pass

    def rollback(self):
        """
        Rollback the current transaction.

        Note: Currently a no-op as transactions are handled automatically
        """
        # TODO: Implement transaction support
        pass

    def close(self):
        """Close the database connection"""
        self._closed = True
        # Rust connection will be cleaned up by Python GC

    @property
    def connection_url(self) -> str:
        """Get the connection URL"""
        return self._connection_url

    @property
    def database_type(self) -> str:
        """Get the database type"""
        return self._connection.database_type

    @property
    def auto_commit(self) -> bool:
        """Get auto-commit setting"""
        return self._connection.auto_commit

    @auto_commit.setter
    def auto_commit(self, value: bool):
        """Set auto-commit setting"""
        self._connection.set_auto_commit(value)

    @property
    def is_closed(self) -> bool:
        """Check if connection is closed"""
        return self._closed


class Cursor:
    """
    Database cursor for executing queries and fetching results.

    Supports multi-statement execution and result set navigation.
    """

    def __init__(self, rust_cursor: _PyCursor, connection: Connection):
        self._cursor = rust_cursor
        self._connection = connection
        self._closed = False

    def __enter__(self) -> 'Cursor':
        """Context manager entry"""
        return self

    def __exit__(self, exc_type, exc_value, traceback) -> None:
        """Context manager exit"""
        self.close()

    def execute(self, query: str) -> int:
        """
        Execute a single SQL statement.

        Args:
            query: SQL query to execute

        Returns:
            int: Number of affected/returned rows
        """
        if self._closed:
            raise DbcrustCommandError("Cursor is closed")

        return self._cursor.execute(query)

    def executescript(self, script: str) -> int:
        """
        Execute multiple SQL statements separated by semicolons.

        Args:
            script: SQL script with multiple statements

        Returns:
            int: Total number of affected/returned rows across all statements
        """
        if self._closed:
            raise DbcrustCommandError("Cursor is closed")

        return self._cursor.executescript(script)

    def fetchone(self) -> Optional[PyRow]:
        """
        Fetch the next row from the current result set.

        Returns:
            PyRow: Next row, or None if no more rows
        """
        if self._closed:
            raise DbcrustCommandError("Cursor is closed")

        return self._cursor.fetchone()

    def fetchmany(self, size: Optional[int] = None) -> list[PyRow]:
        """
        Fetch multiple rows from the current result set.

        Args:
            size: Number of rows to fetch (default: cursor.arraysize or 1)

        Returns:
            list[PyRow]: List of rows
        """
        if self._closed:
            raise DbcrustCommandError("Cursor is closed")

        return self._cursor.fetchmany(size)

    def fetchall(self) -> list[PyRow]:
        """
        Fetch all remaining rows from the current result set.

        Returns:
            list[PyRow]: List of all remaining rows
        """
        if self._closed:
            raise DbcrustCommandError("Cursor is closed")

        return self._cursor.fetchall()

    def nextset(self) -> bool:
        """
        Move to the next result set (for multi-statement queries).

        Returns:
            bool: True if there's another result set, False otherwise
        """
        if self._closed:
            raise DbcrustCommandError("Cursor is closed")

        return self._cursor.nextset()

    def close(self):
        """Close the cursor"""
        if not self._closed:
            self._cursor.close()
            self._closed = True

    @property
    def description(self) -> list[str]:
        """Get column metadata for the current result set"""
        if self._closed:
            raise DbcrustCommandError("Cursor is closed")

        return self._cursor.description

    @property
    def rowcount(self) -> int:
        """Get the number of rows in the current result set"""
        if self._closed:
            return -1

        return self._cursor.rowcount

    @property
    def is_closed(self) -> bool:
        """Check if cursor is closed"""
        return self._closed


# Convenience functions for quick operations
def quick_query(connection_url: str, query: str, **kwargs) -> list[PyRow]:
    """
    Execute a query quickly without managing connections/cursors.

    Args:
        connection_url: Database connection URL
        query: SQL query to execute
        **kwargs: Additional connection options

    Returns:
        list[PyRow]: Query results
    """
    with connect(connection_url, **kwargs) as conn:
        with conn.cursor() as cursor:
            cursor.execute(query)
            return cursor.fetchall()


def quick_script(connection_url: str, script: str, **kwargs) -> int:
    """
    Execute a script quickly without managing connections/cursors.

    Args:
        connection_url: Database connection URL
        script: SQL script with multiple statements
        **kwargs: Additional connection options

    Returns:
        int: Total number of affected rows
    """
    with connect(connection_url, **kwargs) as conn:
        with conn.cursor() as cursor:
            return cursor.executescript(script)


# Export the main classes and functions
__all__ = [
    'connect',
    'Connection',
    'Cursor',
    'quick_query',
    'quick_script',
    'PyServerInfo',
    'PyRow',
    'PyResultSet'
]
