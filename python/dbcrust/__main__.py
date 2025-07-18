#!/usr/bin/env python3
"""
Entry point for running dbcrust from Python
"""
import sys


def main(db_url=None):
    """Run the dbcrust CLI using the shared Rust library"""

    # Import the Rust CLI function
    from dbcrust._internal import run_cli_loop

    # Prepare command arguments
    cmd_args = ["dbcrust", "--no-banner"]

    # If db_url is provided, use it as the connection URL
    if db_url:
        cmd_args.append(db_url)

    # Add any additional command line arguments
    cmd_args.extend(sys.argv[1:])

    # Run the CLI using the shared Rust library
    try:
        return run_cli_loop(cmd_args)
    except Exception as e:
        print(f"Error running dbcrust: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
