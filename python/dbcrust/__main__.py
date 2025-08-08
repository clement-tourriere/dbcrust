#!/usr/bin/env python3
"""
Entry point for running dbcrust from Python
"""
import sys
import os


def run_with_url(db_url, additional_args=None):
    """Run dbcrust programmatically with just a database URL and optional additional arguments

    Args:
        db_url: Database connection URL
        additional_args: Optional list of additional command arguments

    Returns:
        int: Exit code (0 for success, non-zero for failure)

    Raises:
        DbcrustConnectionError: When database connection fails
        DbcrustCommandError: When command execution fails
        DbcrustConfigError: When configuration issues occur
        DbcrustArgumentError: When invalid arguments are provided
    """

    # Import the Rust CLI function
    from dbcrust._internal import run_command

    # Prepare command arguments
    cmd_args = ["dbcrust", db_url]

    # Add any additional arguments if provided
    if additional_args:
        cmd_args.extend(additional_args)

    # Run the CLI using the shared Rust library
    # Let exceptions propagate naturally for proper error handling
    return run_command(cmd_args)


def main(db_url=None):
    """Run the dbcrust CLI using the shared Rust library

    Args:
        db_url: Optional database connection URL

    Returns:
        int: Exit code (0 for success, non-zero for failure)
    """

    # Import the Rust CLI function and exceptions
    from dbcrust._internal import (
        run_command,
        DbcrustConnectionError,
        DbcrustCommandError,
        DbcrustConfigError,
        DbcrustArgumentError,
        DbcrustError
    )

    # Detect the binary name that was used to invoke this script
    # This handles both 'dbcrust' and 'dbc' entry points
    script_name = os.path.basename(sys.argv[0])
    if script_name in ['dbc', 'dbcrust']:
        binary_name = script_name
    elif script_name.endswith('.py') or script_name == 'python3' or script_name == 'python':
        # Running as python -m dbcrust - default to dbcrust
        binary_name = "dbcrust"
    else:
        # Fallback
        binary_name = "dbcrust"

    # Prepare command arguments
    cmd_args = [binary_name]

    # If db_url is provided programmatically, only add it and skip sys.argv
    if db_url:
        cmd_args.append(db_url)
    else:
        # Only add sys.argv arguments when no db_url is provided programmatically
        cmd_args.extend(sys.argv[1:])

    # Run the CLI using the shared Rust library
    try:
        return run_command(cmd_args)
    except (DbcrustConnectionError, DbcrustCommandError, DbcrustConfigError,
            DbcrustArgumentError, DbcrustError) as e:
        # For CLI usage, print the error message to stderr
        print(f"Error: {e}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        # Handle Ctrl-C gracefully
        print("\nInterrupted", file=sys.stderr)
        return 130  # Standard exit code for SIGINT
    except Exception as e:
        # Catch any unexpected errors
        print(f"Unexpected error: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
