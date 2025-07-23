"""
Database utilities for converting Django database settings to DBCrust URLs.

This module provides functions to convert Django's DATABASES configuration
to DBCrust-compatible connection URLs for seamless integration.
"""

import urllib.parse
from typing import Dict, Any, Optional
from django.conf import settings


class UnsupportedDatabaseError(Exception):
    """Raised when trying to convert an unsupported database engine."""
    pass


class DatabaseConfigurationError(Exception):
    """Raised when database configuration is invalid or incomplete."""
    pass


def get_database_config(database_alias: str = 'default') -> Dict[str, Any]:
    """
    Get Django database configuration for a specific database alias.
    
    Args:
        database_alias: The database alias from DATABASES setting
        
    Returns:
        Database configuration dictionary
        
    Raises:
        DatabaseConfigurationError: If database alias not found or invalid
    """
    if not hasattr(settings, 'DATABASES'):
        raise DatabaseConfigurationError("Django DATABASES setting not found")
    
    if not settings.DATABASES:
        raise DatabaseConfigurationError("Django DATABASES setting is empty")
    
    if database_alias not in settings.DATABASES:
        available = ", ".join(settings.DATABASES.keys())
        raise DatabaseConfigurationError(
            f"Database alias '{database_alias}' not found. Available: {available}"
        )
    
    return settings.DATABASES[database_alias]


def django_to_dbcrust_url(database_config: Dict[str, Any]) -> str:
    """
    Convert Django database configuration to DBCrust connection URL.
    
    Args:
        database_config: Django database configuration dictionary
        
    Returns:
        DBCrust-compatible connection URL string
        
    Raises:
        UnsupportedDatabaseError: For unsupported database engines
        DatabaseConfigurationError: For invalid configuration
    """
    engine = database_config.get('ENGINE', '')
    
    if not engine:
        raise DatabaseConfigurationError("Database ENGINE not specified")
    
    # PostgreSQL
    if 'postgresql' in engine.lower() or 'postgres' in engine.lower():
        return _build_postgresql_url(database_config)
    
    # MySQL/MariaDB
    elif 'mysql' in engine.lower():
        return _build_mysql_url(database_config)
    
    # SQLite
    elif 'sqlite' in engine.lower():
        return _build_sqlite_url(database_config)
    
    else:
        raise UnsupportedDatabaseError(
            f"Database engine '{engine}' is not supported. "
            f"Supported engines: PostgreSQL, MySQL, SQLite"
        )


def _build_postgresql_url(config: Dict[str, Any]) -> str:
    """Build PostgreSQL connection URL."""
    host = config.get('HOST') or 'localhost'
    port = config.get('PORT') or 5432
    user = config.get('USER') or ''
    password = config.get('PASSWORD') or ''
    name = config.get('NAME') or ''
    
    if not name:
        raise DatabaseConfigurationError("PostgreSQL database NAME is required")
    
    # URL encode components that might contain special characters
    user_encoded = urllib.parse.quote(user, safe='') if user else ''
    password_encoded = urllib.parse.quote(password, safe='') if password else ''
    host_encoded = urllib.parse.quote(host, safe='') if host else 'localhost'
    name_encoded = urllib.parse.quote(name, safe='')
    
    # Build URL components
    if user_encoded and password_encoded:
        auth = f"{user_encoded}:{password_encoded}@"
    elif user_encoded:
        auth = f"{user_encoded}@"
    else:
        auth = ""
    
    # Base URL
    url = f"postgres://{auth}{host_encoded}:{port}/{name_encoded}"
    
    # Add query parameters from OPTIONS
    query_params = []
    options = config.get('OPTIONS', {})
    
    # Handle SSL mode
    sslmode = options.get('sslmode')
    if sslmode:
        query_params.append(f"sslmode={sslmode}")
    
    # Handle other common PostgreSQL options
    for key in ['connect_timeout', 'application_name', 'sslcert', 'sslkey', 'sslrootcert']:
        if key in options:
            query_params.append(f"{key}={urllib.parse.quote(str(options[key]))}")
    
    if query_params:
        url += "?" + "&".join(query_params)
    
    return url


def _build_mysql_url(config: Dict[str, Any]) -> str:
    """Build MySQL connection URL."""
    host = config.get('HOST') or 'localhost'
    port = config.get('PORT') or 3306
    user = config.get('USER') or ''
    password = config.get('PASSWORD') or ''
    name = config.get('NAME') or ''
    
    if not name:
        raise DatabaseConfigurationError("MySQL database NAME is required")
    
    # URL encode components
    user_encoded = urllib.parse.quote(user, safe='') if user else ''
    password_encoded = urllib.parse.quote(password, safe='') if password else ''
    host_encoded = urllib.parse.quote(host, safe='') if host else 'localhost'
    name_encoded = urllib.parse.quote(name, safe='')
    
    # Build URL components
    if user_encoded and password_encoded:
        auth = f"{user_encoded}:{password_encoded}@"
    elif user_encoded:
        auth = f"{user_encoded}@"
    else:
        auth = ""
    
    # Base URL
    url = f"mysql://{auth}{host_encoded}:{port}/{name_encoded}"
    
    # Add query parameters from OPTIONS
    query_params = []
    options = config.get('OPTIONS', {})
    
    # Handle SSL options
    if 'ssl' in options:
        ssl_options = options['ssl']
        if isinstance(ssl_options, dict):
            for key, value in ssl_options.items():
                query_params.append(f"ssl_{key}={urllib.parse.quote(str(value))}")
        elif ssl_options:
            query_params.append("ssl=true")
    
    # Handle charset
    charset = options.get('charset')
    if charset:
        query_params.append(f"charset={charset}")
    
    # Handle other common MySQL options
    for key in ['connect_timeout', 'read_timeout', 'write_timeout', 'sql_mode']:
        if key in options:
            query_params.append(f"{key}={urllib.parse.quote(str(options[key]))}")
    
    if query_params:
        url += "?" + "&".join(query_params)
    
    return url


def _build_sqlite_url(config: Dict[str, Any]) -> str:
    """Build SQLite connection URL."""
    name = config.get('NAME')
    
    if not name:
        raise DatabaseConfigurationError("SQLite database NAME (file path) is required")
    
    # Handle special SQLite database names
    if name == ':memory:':
        return "sqlite://:memory:"
    
    # Convert to absolute path if relative
    import os
    if not os.path.isabs(name):
        # Make it relative to Django's BASE_DIR if available
        if hasattr(settings, 'BASE_DIR'):
            name = os.path.join(settings.BASE_DIR, name)
        else:
            name = os.path.abspath(name)
    
    # SQLite URLs use file:// format with absolute paths
    # Convert to proper file URL
    return f"sqlite:///{name}"


def get_dbcrust_url(database_alias: str = 'default') -> str:
    """
    Get DBCrust connection URL for a Django database.
    
    This is the main convenience function that combines getting the
    database config and converting it to a DBCrust URL.
    
    Args:
        database_alias: The database alias from DATABASES setting
        
    Returns:
        DBCrust-compatible connection URL string
        
    Raises:
        UnsupportedDatabaseError: For unsupported database engines
        DatabaseConfigurationError: For invalid configuration
    """
    config = get_database_config(database_alias)
    return django_to_dbcrust_url(config)


def list_available_databases() -> Dict[str, str]:
    """
    List all available database aliases and their engines.
    
    Returns:
        Dictionary mapping database alias to engine name
    """
    if not hasattr(settings, 'DATABASES') or not settings.DATABASES:
        return {}
    
    return {
        alias: config.get('ENGINE', 'Unknown')
        for alias, config in settings.DATABASES.items()
    }


def validate_database_support(database_alias: str = 'default') -> tuple[bool, str]:
    """
    Validate if a database is supported by DBCrust.
    
    Args:
        database_alias: The database alias to validate
        
    Returns:
        Tuple of (is_supported, message)
    """
    try:
        config = get_database_config(database_alias)
        engine = config.get('ENGINE', '')
        
        if 'postgresql' in engine.lower() or 'postgres' in engine.lower():
            return True, "PostgreSQL database is supported"
        elif 'mysql' in engine.lower():
            return True, "MySQL database is supported"
        elif 'sqlite' in engine.lower():
            return True, "SQLite database is supported"
        else:
            return False, f"Database engine '{engine}' is not supported by DBCrust"
            
    except DatabaseConfigurationError as e:
        return False, str(e)


def get_database_info_summary(database_alias: str = 'default') -> Dict[str, Any]:
    """
    Get a summary of database information for display.
    
    Args:
        database_alias: The database alias to summarize
        
    Returns:
        Dictionary with database information
    """
    try:
        config = get_database_config(database_alias)
        engine = config.get('ENGINE', 'Unknown')
        
        # Extract engine type
        if 'postgresql' in engine.lower() or 'postgres' in engine.lower():
            engine_type = 'PostgreSQL'
        elif 'mysql' in engine.lower():
            engine_type = 'MySQL'
        elif 'sqlite' in engine.lower():
            engine_type = 'SQLite'
        else:
            engine_type = 'Unknown'
        
        # Build summary
        summary = {
            'alias': database_alias,
            'engine': engine,
            'engine_type': engine_type,
            'host': config.get('HOST', 'N/A'),
            'port': config.get('PORT', 'N/A'),
            'name': config.get('NAME', 'N/A'),
            'user': config.get('USER', 'N/A'),
        }
        
        # Add password indicator (never show actual password)
        summary['has_password'] = bool(config.get('PASSWORD'))
        
        return summary
        
    except Exception as e:
        return {
            'alias': database_alias,
            'error': str(e),
            'engine_type': 'Error'
        }