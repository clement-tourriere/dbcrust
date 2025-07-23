"""
Django management command to launch DBCrust with Django database configuration.

This command works like Django's built-in 'dbshell' command but launches
DBCrust instead of the default database shells, providing all the advanced
features of DBCrust with automatic Django database configuration.
"""

import sys
from django.conf import settings
from django.core.management.base import BaseCommand, CommandError
from typing import Optional

# Import DBCrust Python bindings
try:
    from dbcrust import run_with_url
except ImportError:
    run_with_url = None

# Import Django utilities
try:
    from dbcrust.django.utils import (
        get_dbcrust_url,
        get_database_info_summary,
        list_available_databases,
        validate_database_support,
        UnsupportedDatabaseError,
        DatabaseConfigurationError
    )
except ImportError:
    # Fallback if django submodule is not available
    get_dbcrust_url = None
    get_database_info_summary = None
    list_available_databases = None
    validate_database_support = None
    UnsupportedDatabaseError = Exception
    DatabaseConfigurationError = Exception


class Command(BaseCommand):
    """Django management command to launch DBCrust."""

    help = (
        'Launch DBCrust with Django database configuration. '
        'Works like dbshell but uses DBCrust instead of default database clients.'
    )

    def add_arguments(self, parser):
        """Add command line arguments."""
        parser.add_argument(
            '--database',
            default='default',
            help='Specify the database alias to connect to (default: "default")'
        )
        parser.add_argument(
            '--list-databases',
            action='store_true',
            help='List available database configurations and exit'
        )
        parser.add_argument(
            '--show-url',
            action='store_true',
            help='Show the connection URL that would be used and exit'
        )
        parser.add_argument(
            '--dbcrust-version',
            action='store_true',
            help='Show DBCrust version and exit'
        )
        parser.add_argument(
            '--dry-run',
            action='store_true',
            help='Show the command that would be executed without running it'
        )
        parser.add_argument(
            '--debug',
            action='store_true',
            help='Enable debug output'
        )
        parser.add_argument(
            'dbcrust_args',
            nargs='*',
            help='Additional arguments to pass to DBCrust'
        )

    def handle(self, *args, **options):
        """Main command handler."""

        # Check if required utilities are available
        if get_dbcrust_url is None:
            raise CommandError(
                "❌ Django utilities not found. Please ensure the dbcrust.django module is properly installed."
            )

        # Handle version flag
        if options['dbcrust_version']:
            self._show_version()
            return

        # Handle list databases flag
        if options['list_databases']:
            self._list_databases()
            return

        # Get database configuration
        database_alias = options['database']

        try:
            # Validate database support
            is_supported, message = validate_database_support(database_alias)
            if not is_supported:
                raise CommandError(f"❌ {message}")

            # Get database connection URL
            connection_url = get_dbcrust_url(database_alias)

            # Handle show URL flag
            if options['show_url']:
                self._show_connection_info(database_alias, connection_url)
                return

            # Check if DBCrust Python bindings are available
            if run_with_url is None:
                raise CommandError(
                    "❌ DBCrust Python bindings not found. Please ensure DBCrust is installed.\n"
                    "Install with: pip install dbcrust\n"
                    "Or with uv: uv add dbcrust"
                )

            # Build command arguments
            cmd_args = self._build_command_args(connection_url, options)

            # Handle dry run
            if options['dry_run']:
                self._show_dry_run(cmd_args, database_alias)
                return

            # Show connection info if debug mode
            if options['debug']:
                self._show_connection_info(database_alias, connection_url, show_url=options['debug'])
                self.stdout.write("")  # Empty line

            # Launch DBCrust using Python bindings
            self._launch_dbcrust(cmd_args, database_alias, connection_url)

        except (UnsupportedDatabaseError, DatabaseConfigurationError) as e:
            raise CommandError(f"❌ Database configuration error: {e}")
        except Exception as e:
            if options['debug']:
                import traceback
                traceback.print_exc()
            raise CommandError(f"❌ Unexpected error: {e}")

    def _show_version(self):
        """Show DBCrust version information."""
        if run_with_url is None:
            self.stdout.write(
                self.style.ERROR("❌ DBCrust not found. Please install with: pip install dbcrust")
            )
            return

        try:
            # Use Python bindings to get version
            exit_code = run_with_url(None, ['--version'])
            # Version is printed directly by run_with_url
        except Exception as e:
            self.stdout.write(
                self.style.ERROR(f"❌ Error getting DBCrust version: {e}")
            )

    def _list_databases(self):
        """List available database configurations."""
        databases = list_available_databases()

        if not databases:
            self.stdout.write(
                self.style.WARNING("⚠️  No database configurations found in Django settings.")
            )
            return

        self.stdout.write(
            self.style.SUCCESS("📊 Available Database Configurations:")
        )
        self.stdout.write("")

        for alias, engine in databases.items():
            summary = get_database_info_summary(alias)

            if 'error' in summary:
                status = self.style.ERROR("❌ Error")
                details = summary['error']
            else:
                # Check if supported
                is_supported, _ = validate_database_support(alias)
                if is_supported:
                    status = self.style.SUCCESS("✅ Supported")
                else:
                    status = self.style.WARNING("⚠️  Unsupported")

                # Build details
                if summary['engine_type'] == 'SQLite':
                    details = f"File: {summary['name']}"
                else:
                    host_info = f"{summary['host']}:{summary['port']}" if summary['port'] != 'N/A' else summary['host']
                    details = f"Host: {host_info}, Database: {summary['name']}, User: {summary['user']}"

            self.stdout.write(f"  🔹 {alias}")
            self.stdout.write(f"     Type: {summary.get('engine_type', 'Unknown')}")
            self.stdout.write(f"     Status: {status}")
            self.stdout.write(f"     Details: {details}")
            self.stdout.write("")

    def _show_connection_info(self, database_alias: str, connection_url: str, show_url: bool = True):
        """Show database connection information."""
        summary = get_database_info_summary(database_alias)

        self.stdout.write(
            self.style.SUCCESS(f"🔗 Database Connection Info ({database_alias}):")
        )

        if 'error' in summary:
            self.stdout.write(
                self.style.ERROR(f"   ❌ Error: {summary['error']}")
            )
            return

        self.stdout.write(f"   Database Type: {summary['engine_type']}")

        if summary['engine_type'] == 'SQLite':
            self.stdout.write(f"   File Path: {summary['name']}")
        else:
            self.stdout.write(f"   Host: {summary['host']}")
            self.stdout.write(f"   Port: {summary['port']}")
            self.stdout.write(f"   Database: {summary['name']}")
            self.stdout.write(f"   User: {summary['user']}")
            self.stdout.write(f"   Password: {'Yes' if summary['has_password'] else 'No'}")

        if show_url:
            # Sanitize URL for display (hide password)
            display_url = self._sanitize_url_for_display(connection_url)
            self.stdout.write(f"   Connection URL: {display_url}")

    def _sanitize_url_for_display(self, url: str) -> str:
        """Sanitize URL for safe display by hiding password."""
        # Simple approach: replace password with ***
        import re
        return re.sub(r'://([^:]+):([^@]+)@', r'://\1:***@', url)

    def _build_command_args(self, connection_url: str, options: dict) -> list[str]:
        """Build the command arguments for launching DBCrust."""
        cmd_args = []

        # Add debug flag if requested
        if options.get('debug'):
            cmd_args.append('--debug')

        # Add additional DBCrust arguments
        if options.get('dbcrust_args'):
            cmd_args.extend(options['dbcrust_args'])

        # Add connection URL last
        cmd_args.append(connection_url)

        return cmd_args

    def _show_dry_run(self, cmd_args: list[str], database_alias: str):
        """Show what command would be executed in dry run mode."""
        # Sanitize connection URL (last argument)
        display_args = cmd_args[:-1] + [self._sanitize_url_for_display(cmd_args[-1])]

        self.stdout.write(
            self.style.SUCCESS(f"🔍 Dry Run - Command that would be executed for '{database_alias}':")
        )
        self.stdout.write(f"   dbcrust {' '.join(display_args)}")

    def _launch_dbcrust(self, cmd_args: list[str], database_alias: str, connection_url: str):
        """Launch DBCrust using Python bindings."""
        self.stdout.write(
            self.style.SUCCESS(f"🚀 Launching DBCrust for database '{database_alias}'...")
        )

        try:
            # Use DBCrust Python bindings directly  
            # run_with_url expects (url, additional_args)
            exit_code = run_with_url(connection_url, cmd_args[:-1])  # URL is separate from other args
            sys.exit(exit_code)

        except KeyboardInterrupt:
            self.stdout.write(
                self.style.SUCCESS("\n👋 DBCrust session ended.")
            )
            sys.exit(0)

        except Exception as e:
            raise CommandError(f"❌ Failed to launch DBCrust: {e}")

    def _get_help_text_additions(self):
        """Get additional help text to show available databases."""
        try:
            if list_available_databases:
                databases = list_available_databases()
                if databases:
                    db_list = ", ".join(f"'{alias}'" for alias in databases.keys())
                    return f"\nAvailable databases: {db_list}"
        except Exception:
            pass
        return ""