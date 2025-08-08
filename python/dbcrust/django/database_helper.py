"""
Django Database Helper for DBCrust

Provides seamless integration with Django's database configuration,
allowing developers to use DBCrust's enhanced cursor API with automatic
connection management based on Django's DATABASES setting.
"""

import threading
from contextlib import contextmanager
from typing import Optional, Dict, Any, Union
from ..connector import Connection
from .utils import (
    get_dbcrust_url,
    get_database_config,
    validate_database_support,
    DatabaseConfigurationError,
    UnsupportedDatabaseError
)

# Thread-local storage for connection caching
_connection_cache = threading.local()


class DjangoConnectionError(Exception):
    """Raised when there's an issue with Django database connection."""
    pass


def connect(
    database: Optional[str] = None,
    alias: Optional[str] = None,
    timeout: Optional[float] = None,
    auto_commit: Optional[bool] = None,
    cache_connections: bool = True,
    **kwargs
) -> Connection:
    """
    Connect to a Django database using DBCrust's enhanced cursor API.

    This function automatically uses Django's DATABASES configuration to
    establish connections, eliminating the need for manual connection URLs.

    Args:
        database: Database alias from Django DATABASES (default: 'default')
        alias: Alternative parameter name for database (for compatibility)
        timeout: Connection timeout in seconds
        auto_commit: Enable auto-commit mode
        cache_connections: Cache connections per thread (default: True)
        **kwargs: Additional connection parameters

    Returns:
        Connection object with enhanced cursor API

    Raises:
        DjangoConnectionError: If Django is not configured or database not found
        UnsupportedDatabaseError: If database engine is not supported
        DatabaseConfigurationError: If database configuration is invalid

    Examples:
        # Use default database
        with connect() as connection:
            server_info = connection.get_server_info()
            with connection.cursor() as cursor:
                cursor.execute("SELECT * FROM myapp_users")
                users = cursor.fetchall()

        # Use specific database alias
        with connect("analytics") as connection:
            with connection.cursor() as cursor:
                cursor.execute("SELECT COUNT(*) FROM events")
                count = cursor.fetchone()[0]

        # Override connection settings
        with connect(database="reporting", timeout=30) as connection:
            # Custom timeout for long-running reports
            pass
    """
    # Determine database alias to use
    db_alias = database or alias or 'default'

    try:
        # Validate database support
        is_supported, message = validate_database_support(db_alias)
        if not is_supported:
            raise UnsupportedDatabaseError(message)

        # Check connection cache if enabled
        if cache_connections:
            cached_connection = _get_cached_connection(db_alias)
            if cached_connection and not cached_connection.is_closed:
                return cached_connection

        # Get DBCrust URL for Django database
        connection_url = get_dbcrust_url(db_alias)

        # Import the connector to avoid circular imports
        from ..connector import connect as dbcrust_connect

        # Create connection with additional parameters
        connection = dbcrust_connect(
            url=connection_url,
            timeout=timeout,
            auto_commit=auto_commit,
            **kwargs
        )

        # Cache the connection if enabled
        if cache_connections:
            _cache_connection(db_alias, connection)

        return connection

    except (DatabaseConfigurationError, UnsupportedDatabaseError):
        # Re-raise these as they already have good error messages
        raise

    except Exception as e:
        # Wrap other exceptions with Django context
        raise DjangoConnectionError(
            f"Failed to connect to Django database '{db_alias}': {e}"
        ) from e


def connect_all_databases(
    timeout: Optional[float] = None,
    auto_commit: Optional[bool] = None,
    **kwargs
) -> Dict[str, Connection]:
    """
    Connect to all configured Django databases.

    Args:
        timeout: Connection timeout for all databases
        auto_commit: Auto-commit mode for all databases
        **kwargs: Additional connection parameters

    Returns:
        Dictionary mapping database alias to Connection object

    Raises:
        DjangoConnectionError: If any database connection fails

    Example:
        connections = connect_all_databases()

        # Use default database
        with connections['default'].cursor() as cursor:
            cursor.execute("SELECT COUNT(*) FROM users")
            user_count = cursor.fetchone()[0]

        # Use analytics database
        if 'analytics' in connections:
            with connections['analytics'].cursor() as cursor:
                cursor.execute("SELECT COUNT(*) FROM events")
                event_count = cursor.fetchone()[0]
    """
    from django.conf import settings

    if not hasattr(settings, 'DATABASES') or not settings.DATABASES:
        raise DjangoConnectionError("Django DATABASES setting not found or empty")

    connections = {}
    errors = []

    for alias in settings.DATABASES.keys():
        try:
            # Skip unsupported databases but don't fail the whole operation
            is_supported, message = validate_database_support(alias)
            if not is_supported:
                errors.append(f"{alias}: {message}")
                continue

            connections[alias] = connect(
                database=alias,
                timeout=timeout,
                auto_commit=auto_commit,
                **kwargs
            )

        except Exception as e:
            errors.append(f"{alias}: {e}")

    if not connections:
        error_msg = "No supported databases found. Errors:\n" + "\n".join(errors)
        raise DjangoConnectionError(error_msg)

    # Log any errors but don't fail if we have at least one connection
    if errors:
        import logging
        logger = logging.getLogger(__name__)
        logger.warning(f"Some databases could not be connected: {'; '.join(errors)}")

    return connections


@contextmanager
def transaction(database: Optional[str] = None, **kwargs):
    """
    Context manager for database transactions using Django database.

    Args:
        database: Database alias (default: 'default')
        **kwargs: Additional connection parameters

    Example:
        with transaction() as cursor:
            cursor.execute("INSERT INTO users (name) VALUES (%s)", ("Alice",))
            cursor.execute("INSERT INTO profiles (user_id) VALUES (%s)", (cursor.lastrowid,))
            # Transaction automatically committed on success, rolled back on error
    """
    with connect(database=database, auto_commit=False, **kwargs) as connection:
        with connection.cursor() as cursor:
            try:
                yield cursor
                connection.commit()
            except Exception:
                connection.rollback()
                raise


def get_database_info(database: Optional[str] = None) -> Dict[str, Any]:
    """
    Get information about a Django database including server details.

    Args:
        database: Database alias (default: 'default')

    Returns:
        Dictionary with database information

    Example:
        info = get_database_info()
        print(f"Database: {info['server_type']} {info['server_version']}")
        print(f"Host: {info['host']}, Database: {info['database_name']}")
    """
    db_alias = database or 'default'

    try:
        # Get Django database config
        from .utils import get_database_info_summary
        config_info = get_database_info_summary(db_alias)

        # Get server info if we can connect
        try:
            with connect(database=db_alias) as connection:
                server_info = connection.get_server_info()

                return {
                    'alias': db_alias,
                    'server_type': server_info.database_type,
                    'server_version': server_info.version,
                    'version_major': server_info.version_major,
                    'version_minor': server_info.version_minor,
                    'supports_transactions': server_info.supports_transactions,
                    'supports_roles': server_info.supports_roles,
                    'host': config_info.get('host', 'N/A'),
                    'port': config_info.get('port', 'N/A'),
                    'database_name': config_info.get('name', 'N/A'),
                    'user': config_info.get('user', 'N/A'),
                    'connection_url': get_dbcrust_url(db_alias).replace(
                        f":{config_info.get('password', '')}", ":***"
                    ) if config_info.get('has_password') else get_dbcrust_url(db_alias)
                }

        except Exception as e:
            # Return config info even if we can't connect
            config_info['connection_error'] = str(e)
            return config_info

    except Exception as e:
        return {
            'alias': db_alias,
            'error': str(e)
        }


def list_django_databases() -> Dict[str, Dict[str, Any]]:
    """
    List all Django databases with their support status and configuration.

    Returns:
        Dictionary mapping alias to database information

    Example:
        databases = list_django_databases()
        for alias, info in databases.items():
            status = "✅ Supported" if info.get('supported') else "❌ Not supported"
            print(f"{alias}: {info['engine_type']} - {status}")
    """
    from django.conf import settings

    if not hasattr(settings, 'DATABASES') or not settings.DATABASES:
        return {}

    result = {}

    for alias in settings.DATABASES.keys():
        try:
            # Check if database is supported
            is_supported, message = validate_database_support(alias)

            # Get basic config info
            from .utils import get_database_info_summary
            info = get_database_info_summary(alias)
            info['supported'] = is_supported
            info['support_message'] = message

            if is_supported:
                try:
                    # Try to get the DBCrust URL
                    url = get_dbcrust_url(alias)
                    # Sanitize password in URL
                    from urllib.parse import urlparse
                    parsed = urlparse(url)
                    if parsed.password:
                        sanitized_url = url.replace(f":{parsed.password}", ":***")
                    else:
                        sanitized_url = url
                    info['dbcrust_url'] = sanitized_url
                except Exception as e:
                    info['url_error'] = str(e)

            result[alias] = info

        except Exception as e:
            result[alias] = {
                'alias': alias,
                'error': str(e),
                'supported': False
            }

    return result


def _get_cached_connection(alias: str) -> Optional[Connection]:
    """Get cached connection for database alias."""
    if not hasattr(_connection_cache, 'connections'):
        _connection_cache.connections = {}
    return _connection_cache.connections.get(alias)


def _cache_connection(alias: str, connection: Connection):
    """Cache connection for database alias."""
    if not hasattr(_connection_cache, 'connections'):
        _connection_cache.connections = {}
    _connection_cache.connections[alias] = connection


def clear_connection_cache():
    """Clear all cached database connections."""
    if hasattr(_connection_cache, 'connections'):
        # Close all cached connections
        for connection in _connection_cache.connections.values():
            try:
                connection.close()
            except Exception:
                pass  # Ignore errors when closing

        _connection_cache.connections.clear()


# Convenience aliases for common use cases
django_connect = connect  # Alternative name
db_connect = connect      # Short name
