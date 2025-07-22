"""
dbcrust - Multi-database interactive client with tab completion
"""
# Import from the compiled Rust extension
from dbcrust._internal import PyDatabase, PyConfig, run_command, run_cli_loop

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
        >>> dbcrust.run_cli("postgresql://user:pass@localhost/mydb")
        >>> dbcrust.run_cli("mysql://user:pass@localhost/mydb")
        >>> dbcrust.run_cli("sqlite:///path/to/database.db")
    """
    return _run_cli_main(db_url)

__all__ = ["PyDatabase", "PyConfig", "PostgresClient", "run_cli", "run_with_url", "run_command", "run_cli_loop"] 