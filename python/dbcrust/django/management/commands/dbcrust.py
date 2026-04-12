"""
Django management command to launch DBCrust with Django database configuration.

This command works like Django's built-in ``dbshell`` command but launches
DBCrust instead of the default database shell, using the configured Django
database connection settings.
"""

import os
import shutil
import subprocess
import sys

from django.core.management.base import BaseCommand, CommandError

from ...utils import (
    DatabaseConfigurationError,
    UnsupportedDatabaseError,
    get_database_info_summary,
    get_dbcrust_url,
    list_available_databases,
    validate_database_support,
)


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
        database_alias = options.get('database', 'default')
        debug = options.get('debug', False)

        try:
            if options.get('dbcrust_version') or options.get('version'):
                self._show_version()
                return

            if options.get('list_databases'):
                self._list_databases()
                return

            is_supported, message = validate_database_support(database_alias)
            if not is_supported:
                raise CommandError(f"❌ {message}")

            connection_url = get_dbcrust_url(database_alias)

            if options.get('show_url'):
                self._show_connection_info(database_alias, connection_url)
                return

            dbcrust_binary = self._find_dbcrust_binary()
            if not dbcrust_binary:
                raise CommandError(
                    "❌ DBCrust binary not found. Please install with: pip install dbcrust"
                )

            cmd_args = self._build_command_args(dbcrust_binary, connection_url, options)

            if options.get('dry_run'):
                self._show_dry_run(cmd_args, database_alias)
                return

            if debug:
                self._show_connection_info(database_alias, connection_url, show_url=True)
                self.stdout.write("")

            self._launch_dbcrust(cmd_args, database_alias)

        except (UnsupportedDatabaseError, DatabaseConfigurationError) as e:
            raise CommandError(f"❌ Database configuration error: {e}")
        except CommandError:
            raise
        except Exception as e:
            if debug:
                import traceback

                traceback.print_exc()
            raise CommandError(f"❌ Unexpected error: {e}")

    def _find_dbcrust_binary(self) -> str | None:
        """Find the DBCrust executable on PATH."""
        return shutil.which('dbcrust')

    def _show_version(self):
        """Show DBCrust version information."""
        dbcrust_binary = self._find_dbcrust_binary()
        if not dbcrust_binary:
            self.stdout.write("❌ DBCrust not found. Please install with: pip install dbcrust")
            return

        try:
            result = subprocess.run(
                [dbcrust_binary, '--version'],
                capture_output=True,
                text=True,
                timeout=10,
            )
            self.stdout.write(result.stdout.strip())
        except Exception as e:
            self.stdout.write(f"❌ Error getting DBCrust version: {e}")

    def _list_databases(self):
        """List available database configurations."""
        databases = list_available_databases()

        if not databases:
            self.stdout.write("⚠️  No database configurations found in Django settings.")
            return

        self.stdout.write("📊 Available Database Configurations:")
        self.stdout.write("")

        for alias, _engine in databases.items():
            summary = get_database_info_summary(alias)

            if 'error' in summary:
                status = "❌ Error"
                details = summary['error']
            else:
                is_supported, _ = validate_database_support(alias)
                if is_supported:
                    status = "✅ Supported"
                else:
                    status = "⚠️  Unsupported"

                if summary['engine_type'] == 'SQLite':
                    details = f"File: {summary['name']}"
                else:
                    host_info = (
                        f"{summary['host']}:{summary['port']}"
                        if summary['port'] != 'N/A'
                        else summary['host']
                    )
                    details = (
                        f"Host: {host_info}, Database: {summary['name']}, "
                        f"User: {summary['user']}"
                    )

            self.stdout.write(f"  🔹 {alias}")
            self.stdout.write(f"     Type: {summary.get('engine_type', 'Unknown')}")
            self.stdout.write(f"     Status: {status}")
            self.stdout.write(f"     Details: {details}")
            self.stdout.write("")

    def _show_connection_info(self, database_alias: str, connection_url: str, show_url: bool = True):
        """Show database connection information."""
        summary = get_database_info_summary(database_alias)

        self.stdout.write(f"🔗 Database Connection Info ({database_alias}):")

        if 'error' in summary:
            self.stdout.write(f"   ❌ Error: {summary['error']}")
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
            display_url = self._sanitize_url_for_display(connection_url)
            self.stdout.write(f"   Connection URL: {display_url}")

    def _sanitize_url_for_display(self, url: str) -> str:
        """Sanitize URL for safe display by hiding password."""
        import re

        return re.sub(r'://([^:]+):([^@]+)@', r'://\1:***@', url)

    def _build_command_args(self, dbcrust_binary: str, connection_url: str, options: dict) -> list[str]:
        """Build the command arguments for launching DBCrust."""
        cmd_args = [dbcrust_binary]

        if options.get('debug'):
            cmd_args.append('--debug')

        if options.get('dbcrust_args'):
            cmd_args.extend(options['dbcrust_args'])

        cmd_args.append(connection_url)
        return cmd_args

    def _show_dry_run(self, cmd_args: list[str], database_alias: str):
        """Show what command would be executed in dry run mode."""
        display_args = cmd_args[:-1] + [self._sanitize_url_for_display(cmd_args[-1])]

        self.stdout.write(f"🔍 Dry Run - Command that would be executed for '{database_alias}':")
        self.stdout.write(f"   {' '.join(display_args)}")

    def _launch_dbcrust(self, cmd_args: list[str], database_alias: str):
        """Launch DBCrust using execvp with subprocess fallback."""
        self.stdout.write(f"🚀 Launching DBCrust for database '{database_alias}'...")

        try:
            os.execvp(cmd_args[0], cmd_args)
        except OSError:
            self.stderr.write(
                "⚠️  Could not replace process, falling back to subprocess"
            )
            try:
                result = subprocess.run(cmd_args)
                sys.exit(result.returncode)
            except KeyboardInterrupt:
                self.stdout.write("\n👋 DBCrust session ended.")
                sys.exit(0)
        except KeyboardInterrupt:
            self.stdout.write("\n👋 DBCrust session ended.")
            sys.exit(0)

    def _get_help_text_additions(self):
        """Get additional help text to show available databases."""
        try:
            databases = list_available_databases()
            if databases:
                db_list = ", ".join(f"'{alias}'" for alias in databases.keys())
                return f"\nAvailable databases: {db_list}"
        except Exception:
            pass
        return ""
