#!/usr/bin/env python3
"""
Entry point for running dbcrust from Python
"""
import sys
import os


def run_with_url(db_url, additional_args=None):
    """Run dbcrust programmatically with just a database URL and optional additional arguments"""
    
    # Import the Rust CLI function
    from dbcrust._internal import run_command

    # Prepare command arguments
    cmd_args = ["dbcrust", db_url]
    
    # Add any additional arguments if provided
    if additional_args:
        cmd_args.extend(additional_args)

    # Run the CLI using the shared Rust library
    try:
        return run_command(cmd_args)
    except Exception as e:
        print(f"Error running dbcrust: {e}", file=sys.stderr)
        return 1


def main(db_url=None):
    """Run the dbcrust CLI using the shared Rust library"""

    # Import the Rust CLI function
    from dbcrust._internal import run_command

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
    except Exception as e:
        print(f"Error running dbcrust: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
