"""
dbcrust - Multi-database interactive client with tab completion
"""
# Import from the compiled Rust extension
from dbcrust._internal import (  # ty: ignore[unresolved-import]
    PyDatabase,
    PyConfig,
    run_command,
    run_cli_loop,
    # New enhanced API classes
    PyConnection,
    PyCursor,
    PyServerInfo,
    PyRow,
    PyResultSet,
    py_connect,
    # Exception classes
    DbcrustError,
    DbcrustConnectionError,
    DbcrustCommandError,
    DbcrustConfigError,
    DbcrustArgumentError
)

try:
    from dbcrust._internal import ai_config_status  # ty: ignore[unresolved-import]
except ImportError:  # pragma: no cover - only for mismatched editable installs
    def ai_config_status():
        raise DbcrustConfigError(
            "ai_config_status requires the dbcrust native extension from this release"
        )

# Import enhanced connector API
from .connector import (
    connect,
    Connection,
    Cursor,
    quick_query,
    quick_script
)

# Legacy imports for backward compatibility
from .client import PostgresClient
from .__main__ import main as _run_cli_main, run_with_url

def run_cli(db_url=None):
    """
    Run the interactive DBCrust CLI.

    Args:
        db_url (str, optional): Database connection URL. If not provided,
                               will use command line arguments or prompt for connection.

    Examples:
        >>> import dbcrust
        >>> dbcrust.run_cli("postgres://user:pass@localhost/mydb")
        >>> dbcrust.run_cli("mysql://user:pass@localhost/mydb")
        >>> dbcrust.run_cli("sqlite:///path/to/database.db")
    """
    return _run_cli_main(db_url)

__all__ = [
    # Enhanced API (recommended for new projects)
    "connect",
    "Connection",
    "Cursor",
    "quick_query",
    "quick_script",

    # Rust-level classes (advanced usage)
    "PyConnection",
    "PyCursor",
    "PyServerInfo",
    "PyRow",
    "PyResultSet",
    "py_connect",

    # Legacy API (backward compatibility)
    "PyDatabase",
    "PyConfig",
    "PostgresClient",
    "run_cli",
    "run_with_url",
    "run_command",
    "run_cli_loop",
    "ai_config_status",

    # Exception classes
    "DbcrustError",
    "DbcrustConnectionError",
    "DbcrustCommandError",
    "DbcrustConfigError",
    "DbcrustArgumentError"
]
