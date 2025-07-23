"""
Tests for Django Management Command and Database Utilities.

These tests verify the functionality of the DBCrust Django management command,
database URL conversion utilities, and integration with Django's database configuration.
"""

import unittest
from unittest.mock import Mock, patch, MagicMock, call
import subprocess
import os
import io
import sys
from django.core.management.base import CommandError
from django.test import override_settings

# Mock Django before importing our modules
django_mock = MagicMock()
django_mock.core.management.base.BaseCommand = Mock
django_mock.core.management.base.CommandError = CommandError
django_mock.conf.settings = MagicMock()

with patch.dict('sys.modules', {
    'django': django_mock,
    'django.core': django_mock.core,
    'django.core.management': django_mock.core.management,
    'django.core.management.base': django_mock.core.management.base,
    'django.conf': django_mock.conf,
}):
    from ..utils import (
        get_database_config,
        django_to_dbcrust_url,
        get_dbcrust_url,
        list_available_databases,
        validate_database_support,
        get_database_info_summary,
        UnsupportedDatabaseError,
        DatabaseConfigurationError,
    )
    from ..management.commands.dbcrust import Command


class TestDatabaseUtils(unittest.TestCase):
    """Test database utility functions."""
    
    def setUp(self):
        """Set up test fixtures."""
        self.postgresql_config = {
            'ENGINE': 'django.db.backends.postgresql',
            'NAME': 'test_db',
            'USER': 'test_user',
            'PASSWORD': 'test_pass',
            'HOST': 'localhost',
            'PORT': '5432',
            'OPTIONS': {
                'sslmode': 'require',
                'connect_timeout': 10,
            }
        }
        
        self.mysql_config = {
            'ENGINE': 'django.db.backends.mysql',
            'NAME': 'test_db',
            'USER': 'test_user',
            'PASSWORD': 'test_pass',
            'HOST': 'localhost',
            'PORT': '3306',
            'OPTIONS': {
                'charset': 'utf8mb4',
                'ssl': {'ssl_ca': '/path/to/ca.pem'},
            }
        }
        
        self.sqlite_config = {
            'ENGINE': 'django.db.backends.sqlite3',
            'NAME': '/path/to/test.db',
        }
    
    def test_get_database_config_success(self):
        """Test successful database config retrieval."""
        mock_databases = {'default': self.postgresql_config}
        
        with patch('django.conf.settings') as mock_settings:
            mock_settings.DATABASES = mock_databases
            
            result = get_database_config('default')
            self.assertEqual(result, self.postgresql_config)
    
    def test_get_database_config_missing_alias(self):
        """Test error when database alias doesn't exist."""
        mock_databases = {'default': self.postgresql_config}
        
        with patch('django.conf.settings') as mock_settings:
            mock_settings.DATABASES = mock_databases
            
            with self.assertRaises(DatabaseConfigurationError) as cm:
                get_database_config('nonexistent')
            
            self.assertIn("Database alias 'nonexistent' not found", str(cm.exception))
    
    def test_get_database_config_no_databases_setting(self):
        """Test error when DATABASES setting doesn't exist."""
        with patch('django.conf.settings') as mock_settings:
            delattr(mock_settings, 'DATABASES') if hasattr(mock_settings, 'DATABASES') else None
            
            with self.assertRaises(DatabaseConfigurationError) as cm:
                get_database_config()
            
            self.assertIn("Django DATABASES setting not found", str(cm.exception))
    
    def test_postgresql_url_conversion(self):
        """Test PostgreSQL database URL conversion."""
        url = django_to_dbcrust_url(self.postgresql_config)
        
        expected = "postgres://test_user:test_pass@localhost:5432/test_db?sslmode=require&connect_timeout=10"
        self.assertEqual(url, expected)
    
    def test_postgresql_url_no_password(self):
        """Test PostgreSQL URL conversion without password."""
        config = self.postgresql_config.copy()
        config['PASSWORD'] = ''
        
        url = django_to_dbcrust_url(config)
        
        self.assertIn("postgres://test_user@localhost:5432/test_db", url)
        self.assertNotIn(":", url.split("@")[0])  # No colon before @
    
    def test_postgresql_url_no_user(self):
        """Test PostgreSQL URL conversion without user."""
        config = self.postgresql_config.copy()
        config['USER'] = ''
        config['PASSWORD'] = ''
        
        url = django_to_dbcrust_url(config)
        
        self.assertIn("postgres://localhost:5432/test_db", url)
        self.assertNotIn("@", url.split("://")[1].split("/")[0])  # No @ in host part
    
    def test_postgresql_url_special_characters(self):
        """Test PostgreSQL URL conversion with special characters."""
        config = self.postgresql_config.copy()
        config['USER'] = 'user@domain'
        config['PASSWORD'] = 'pass:word!'
        config['NAME'] = 'test-db'
        
        url = django_to_dbcrust_url(config)
        
        # Should be URL encoded
        self.assertIn("user%40domain", url)  # @ encoded
        self.assertIn("pass%3Aword%21", url)  # : and ! encoded
        self.assertIn("test-db", url)  # Hyphens are safe
    
    def test_mysql_url_conversion(self):
        """Test MySQL database URL conversion."""
        url = django_to_dbcrust_url(self.mysql_config)
        
        expected = "mysql://test_user:test_pass@localhost:3306/test_db?ssl_ssl_ca=%2Fpath%2Fto%2Fca.pem&charset=utf8mb4"
        self.assertEqual(url, expected)
    
    def test_mysql_url_ssl_boolean(self):
        """Test MySQL URL conversion with boolean SSL option."""
        config = self.mysql_config.copy()
        config['OPTIONS'] = {'ssl': True}
        
        url = django_to_dbcrust_url(config)
        
        self.assertIn("ssl=true", url)
    
    def test_sqlite_url_conversion(self):
        """Test SQLite database URL conversion."""
        url = django_to_dbcrust_url(self.sqlite_config)
        
        expected = "sqlite:////path/to/test.db"
        self.assertEqual(url, expected)
    
    def test_sqlite_memory_database(self):
        """Test SQLite in-memory database URL conversion."""
        config = {'ENGINE': 'django.db.backends.sqlite3', 'NAME': ':memory:'}
        
        url = django_to_dbcrust_url(config)
        
        self.assertEqual(url, "sqlite://:memory:")
    
    def test_sqlite_relative_path(self):
        """Test SQLite relative path conversion."""
        config = {'ENGINE': 'django.db.backends.sqlite3', 'NAME': 'db.sqlite3'}
        
        with patch('django.conf.settings') as mock_settings:
            mock_settings.BASE_DIR = '/project/root'
            
            url = django_to_dbcrust_url(config)
            
            self.assertEqual(url, "sqlite:///project/root/db.sqlite3")
    
    def test_unsupported_database_engine(self):
        """Test error for unsupported database engine."""
        config = {'ENGINE': 'django.db.backends.oracle'}
        
        with self.assertRaises(UnsupportedDatabaseError) as cm:
            django_to_dbcrust_url(config)
        
        self.assertIn("Database engine", str(cm.exception))
        self.assertIn("is not supported", str(cm.exception))
    
    def test_missing_database_name(self):
        """Test error for missing database name."""
        config = self.postgresql_config.copy()
        config['NAME'] = ''
        
        with self.assertRaises(DatabaseConfigurationError) as cm:
            django_to_dbcrust_url(config)
        
        self.assertIn("database NAME is required", str(cm.exception))
    
    def test_list_available_databases(self):
        """Test listing available databases."""
        mock_databases = {
            'default': self.postgresql_config,
            'secondary': self.mysql_config,
            'cache': self.sqlite_config,
        }
        
        with patch('django.conf.settings') as mock_settings:
            mock_settings.DATABASES = mock_databases
            
            result = list_available_databases()
            
            expected = {
                'default': 'django.db.backends.postgresql',
                'secondary': 'django.db.backends.mysql',
                'cache': 'django.db.backends.sqlite3',
            }
            self.assertEqual(result, expected)
    
    def test_list_available_databases_empty(self):
        """Test listing databases when none configured."""
        with patch('django.conf.settings') as mock_settings:
            mock_settings.DATABASES = {}
            
            result = list_available_databases()
            
            self.assertEqual(result, {})
    
    def test_validate_database_support_postgresql(self):
        """Test database support validation for PostgreSQL."""
        with patch('django.conf.settings') as mock_settings:
            mock_settings.DATABASES = {'default': self.postgresql_config}
            
            is_supported, message = validate_database_support('default')
            
            self.assertTrue(is_supported)
            self.assertIn("PostgreSQL database is supported", message)
    
    def test_validate_database_support_unsupported(self):
        """Test database support validation for unsupported engine."""
        config = {'ENGINE': 'django.db.backends.oracle'}
        
        with patch('django.conf.settings') as mock_settings:
            mock_settings.DATABASES = {'default': config}
            
            is_supported, message = validate_database_support('default')
            
            self.assertFalse(is_supported)
            self.assertIn("is not supported by DBCrust", message)
    
    def test_get_database_info_summary_postgresql(self):
        """Test database info summary for PostgreSQL."""
        with patch('django.conf.settings') as mock_settings:
            mock_settings.DATABASES = {'default': self.postgresql_config}
            
            summary = get_database_info_summary('default')
            
            expected = {
                'alias': 'default',
                'engine': 'django.db.backends.postgresql',
                'engine_type': 'PostgreSQL',
                'host': 'localhost',
                'port': '5432',
                'name': 'test_db',
                'user': 'test_user',
                'has_password': True,
            }
            self.assertEqual(summary, expected)
    
    def test_get_database_info_summary_sqlite(self):
        """Test database info summary for SQLite."""
        with patch('django.conf.settings') as mock_settings:
            mock_settings.DATABASES = {'default': self.sqlite_config}
            
            summary = get_database_info_summary('default')
            
            self.assertEqual(summary['engine_type'], 'SQLite')
            self.assertEqual(summary['name'], '/path/to/test.db')
            self.assertEqual(summary['host'], 'N/A')
            self.assertEqual(summary['port'], 'N/A')
    
    def test_get_database_info_summary_error(self):
        """Test database info summary with configuration error."""
        with patch('django.conf.settings') as mock_settings:
            mock_settings.DATABASES = {}
            
            summary = get_database_info_summary('nonexistent')
            
            self.assertEqual(summary['alias'], 'nonexistent')
            self.assertIn('error', summary)
            self.assertEqual(summary['engine_type'], 'Error')


class TestManagementCommand(unittest.TestCase):
    """Test Django management command functionality."""
    
    def setUp(self):
        """Set up test fixtures."""
        self.command = Command()
        self.command.stdout = io.StringIO()
        self.command.stderr = io.StringIO()
        
        # Mock Django settings
        self.mock_databases = {
            'default': {
                'ENGINE': 'django.db.backends.postgresql',
                'NAME': 'test_db',
                'USER': 'test_user',
                'PASSWORD': 'test_pass',
                'HOST': 'localhost',
                'PORT': '5432',
            }
        }
    
    def get_output(self):
        """Get command output."""
        return self.command.stdout.getvalue()
    
    def get_error_output(self):
        """Get command error output."""
        return self.command.stderr.getvalue()
    
    @patch('shutil.which')
    @patch('django.conf.settings')
    def test_handle_version_flag(self, mock_settings, mock_which):
        """Test --version flag functionality."""
        mock_which.return_value = '/usr/local/bin/dbcrust'
        
        with patch('subprocess.run') as mock_run:
            mock_run.return_value.stdout = 'DBCrust 1.0.0\n'
            
            self.command.handle(version=True)
            
            output = self.get_output()
            self.assertIn('DBCrust 1.0.0', output)
            mock_run.assert_called_once_with(
                ['/usr/local/bin/dbcrust', '--version'],
                capture_output=True,
                text=True,
                timeout=10
            )
    
    @patch('shutil.which')
    def test_handle_version_flag_dbcrust_not_found(self, mock_which):
        """Test --version flag when DBCrust not found."""
        mock_which.return_value = None
        
        self.command.handle(version=True)
        
        output = self.get_output()
        self.assertIn('❌ DBCrust not found', output)
    
    @patch('django.conf.settings')
    def test_handle_list_databases_flag(self, mock_settings):
        """Test --list-databases flag functionality."""
        mock_settings.DATABASES = self.mock_databases
        
        self.command.handle(list_databases=True)
        
        output = self.get_output()
        self.assertIn('📊 Available Database Configurations', output)
        self.assertIn('default', output)
        self.assertIn('PostgreSQL', output)
        self.assertIn('✅ Supported', output)
    
    @patch('django.conf.settings')
    def test_handle_list_databases_empty(self, mock_settings):
        """Test --list-databases flag with no databases."""
        mock_settings.DATABASES = {}
        
        self.command.handle(list_databases=True)
        
        output = self.get_output()
        self.assertIn('⚠️  No database configurations found', output)
    
    @patch('django.conf.settings')
    def test_handle_show_url_flag(self, mock_settings):
        """Test --show-url flag functionality."""
        mock_settings.DATABASES = self.mock_databases
        
        self.command.handle(show_url=True, database='default')
        
        output = self.get_output()
        self.assertIn('🔗 Database Connection Info (default)', output)
        self.assertIn('Database Type: PostgreSQL', output)
        self.assertIn('Host: localhost', output)
        self.assertIn('Connection URL:', output)
        self.assertIn('postgres://test_user:***@localhost:5432/test_db', output)
    
    @patch('shutil.which')
    @patch('django.conf.settings')
    def test_handle_dry_run_flag(self, mock_settings, mock_which):
        """Test --dry-run flag functionality."""
        mock_settings.DATABASES = self.mock_databases
        mock_which.return_value = '/usr/local/bin/dbcrust'
        
        self.command.handle(dry_run=True, database='default')
        
        output = self.get_output()
        self.assertIn('🔍 Dry Run - Command that would be executed', output)
        self.assertIn('/usr/local/bin/dbcrust', output)
        self.assertIn('postgres://test_user:***@localhost:5432/test_db', output)
    
    @patch('django.conf.settings')
    def test_handle_unsupported_database(self, mock_settings):
        """Test handling of unsupported database."""
        mock_settings.DATABASES = {
            'default': {
                'ENGINE': 'django.db.backends.oracle',
                'NAME': 'test_db',
            }
        }
        
        with self.assertRaises(CommandError) as cm:
            self.command.handle(database='default')
        
        self.assertIn('❌', str(cm.exception))
        self.assertIn('not supported by DBCrust', str(cm.exception))
    
    @patch('django.conf.settings')
    def test_handle_missing_database_alias(self, mock_settings):
        """Test handling of missing database alias."""
        mock_settings.DATABASES = self.mock_databases
        
        with self.assertRaises(CommandError) as cm:
            self.command.handle(database='nonexistent')
        
        self.assertIn('❌', str(cm.exception))
        self.assertIn("Database alias 'nonexistent' not found", str(cm.exception))
    
    @patch('shutil.which')
    @patch('django.conf.settings')
    def test_handle_dbcrust_not_found(self, mock_settings, mock_which):
        """Test handling when DBCrust binary not found."""
        mock_settings.DATABASES = self.mock_databases
        mock_which.return_value = None
        
        with self.assertRaises(CommandError) as cm:
            self.command.handle(database='default')
        
        self.assertIn('❌ DBCrust binary not found', str(cm.exception))
        self.assertIn('pip install dbcrust', str(cm.exception))
    
    def test_find_dbcrust_binary_found(self):
        """Test finding DBCrust binary when available."""
        with patch('shutil.which') as mock_which:
            mock_which.side_effect = lambda name: '/usr/local/bin/dbcrust' if name == 'dbcrust' else None
            
            result = self.command._find_dbcrust_binary()
            
            self.assertEqual(result, '/usr/local/bin/dbcrust')
    
    def test_find_dbcrust_binary_not_found(self):
        """Test finding DBCrust binary when not available."""
        with patch('shutil.which') as mock_which:
            mock_which.return_value = None
            
            result = self.command._find_dbcrust_binary()
            
            self.assertIsNone(result)
    
    def test_build_command_args_basic(self):
        """Test building basic command arguments."""
        options = {
            'debug': False,
            'dbcrust_args': [],
        }
        
        args = self.command._build_command_args('/usr/local/bin/dbcrust', 'postgres://localhost/test', options)
        
        expected = ['/usr/local/bin/dbcrust', 'postgres://localhost/test']
        self.assertEqual(args, expected)
    
    def test_build_command_args_with_debug(self):
        """Test building command arguments with debug flag."""
        options = {
            'debug': True,
            'dbcrust_args': [],
        }
        
        args = self.command._build_command_args('/usr/local/bin/dbcrust', 'postgres://localhost/test', options)
        
        expected = ['/usr/local/bin/dbcrust', '--debug', 'postgres://localhost/test']
        self.assertEqual(args, expected)
    
    def test_build_command_args_with_extra_args(self):
        """Test building command arguments with extra DBCrust args."""
        options = {
            'debug': False,
            'dbcrust_args': ['--no-banner', '-c', '\\dt'],
        }
        
        args = self.command._build_command_args('/usr/local/bin/dbcrust', 'postgres://localhost/test', options)
        
        expected = ['/usr/local/bin/dbcrust', '--no-banner', '-c', '\\dt', 'postgres://localhost/test']
        self.assertEqual(args, expected)
    
    def test_sanitize_url_for_display(self):
        """Test URL sanitization for display."""
        test_cases = [
            ('postgres://user:pass@host:5432/db', 'postgres://user:***@host:5432/db'),
            ('mysql://user:password@localhost:3306/db', 'mysql://user:***@localhost:3306/db'),
            ('sqlite:///path/to/db.sqlite3', 'sqlite:///path/to/db.sqlite3'),
            ('postgres://user@host:5432/db', 'postgres://user@host:5432/db'),  # No password
        ]
        
        for input_url, expected_output in test_cases:
            with self.subTest(input_url=input_url):
                result = self.command._sanitize_url_for_display(input_url)
                self.assertEqual(result, expected_output)
    
    @patch('os.execvp')
    @patch('shutil.which')
    @patch('django.conf.settings')
    def test_launch_dbcrust_success(self, mock_settings, mock_which, mock_execvp):
        """Test successful DBCrust launch."""
        mock_settings.DATABASES = self.mock_databases
        mock_which.return_value = '/usr/local/bin/dbcrust'
        
        self.command.handle(database='default')
        
        output = self.get_output()
        self.assertIn('🚀 Launching DBCrust for database', output)
        mock_execvp.assert_called_once()
    
    @patch('os.execvp')
    @patch('subprocess.run')
    @patch('shutil.which')
    @patch('django.conf.settings')
    def test_launch_dbcrust_execvp_fallback(self, mock_settings, mock_which, mock_run, mock_execvp):
        """Test DBCrust launch with execvp fallback to subprocess."""
        mock_settings.DATABASES = self.mock_databases
        mock_which.return_value = '/usr/local/bin/dbcrust'
        mock_execvp.side_effect = OSError("execvp failed")
        mock_run.return_value.returncode = 0
        
        with patch('sys.exit') as mock_exit:
            self.command.handle(database='default')
            
            output = self.get_output()
            self.assertIn('🚀 Launching DBCrust for database', output)
            self.assertIn('⚠️  Could not replace process, falling back to subprocess', 
                         self.get_error_output())
            
            mock_execvp.assert_called_once()
            mock_run.assert_called_once()
            mock_exit.assert_called_once_with(0)
    
    @patch('os.execvp')
    @patch('subprocess.run')
    @patch('shutil.which')
    @patch('django.conf.settings')
    def test_launch_dbcrust_keyboard_interrupt(self, mock_settings, mock_which, mock_run, mock_execvp):
        """Test DBCrust launch handling keyboard interrupt."""
        mock_settings.DATABASES = self.mock_databases
        mock_which.return_value = '/usr/local/bin/dbcrust'
        mock_execvp.side_effect = OSError("execvp failed")
        mock_run.side_effect = KeyboardInterrupt()
        
        with patch('sys.exit') as mock_exit:
            self.command.handle(database='default')
            
            output = self.get_output()
            self.assertIn('👋 DBCrust session ended', output)
            mock_exit.assert_called_once_with(0)


class TestIntegration(unittest.TestCase):
    """Integration tests for the complete management command workflow."""
    
    @patch('shutil.which')
    @patch('subprocess.run')
    @patch('os.execvp')
    @patch('django.conf.settings')
    def test_full_workflow_postgresql(self, mock_settings, mock_execvp, mock_run, mock_which):
        """Test complete workflow for PostgreSQL database."""
        # Setup
        mock_settings.DATABASES = {
            'default': {
                'ENGINE': 'django.db.backends.postgresql',
                'NAME': 'myapp_db',
                'USER': 'myapp_user',
                'PASSWORD': 'secret_password',
                'HOST': 'db.example.com',
                'PORT': '5432',
                'OPTIONS': {
                    'sslmode': 'require',
                }
            }
        }
        mock_which.return_value = '/usr/local/bin/dbcrust'
        
        # Execute command
        command = Command()
        command.stdout = io.StringIO()
        command.stderr = io.StringIO()
        
        command.handle(database='default', debug=True)
        
        # Verify
        output = command.stdout.getvalue()
        self.assertIn('🚀 Launching DBCrust for database', output)
        self.assertIn('Database Type: PostgreSQL', output)
        self.assertIn('Host: db.example.com', output)
        
        # Check that execvp was called with correct arguments
        mock_execvp.assert_called_once()
        call_args = mock_execvp.call_args[0]
        self.assertEqual(call_args[0], '/usr/local/bin/dbcrust')
        
        # Check that the URL is correctly formed (but sanitized in debug output)
        debug_output = output
        self.assertIn('postgres://myapp_user:***@db.example.com:5432/myapp_db', debug_output)
    
    @patch('django.conf.settings')
    def test_multiple_database_support(self, mock_settings):
        """Test support for multiple database configurations."""
        mock_settings.DATABASES = {
            'default': {
                'ENGINE': 'django.db.backends.postgresql',
                'NAME': 'main_db',
                'USER': 'postgres',
                'PASSWORD': '',
                'HOST': 'localhost',
                'PORT': '5432',
            },
            'analytics': {
                'ENGINE': 'django.db.backends.mysql',
                'NAME': 'analytics_db',
                'USER': 'mysql_user',
                'PASSWORD': 'mysql_pass',
                'HOST': 'mysql.example.com',
                'PORT': '3306',
            },
            'cache': {
                'ENGINE': 'django.db.backends.sqlite3',
                'NAME': '/tmp/cache.db',
            }
        }
        
        command = Command()
        command.stdout = io.StringIO()
        
        # Test listing all databases
        command.handle(list_databases=True)
        
        output = command.stdout.getvalue()
        self.assertIn('default', output)
        self.assertIn('analytics', output)
        self.assertIn('cache', output)
        self.assertIn('PostgreSQL', output)
        self.assertIn('MySQL', output)
        self.assertIn('SQLite', output)
    
    @patch('django.conf.settings')
    def test_error_handling_chain(self, mock_settings):
        """Test error handling through the complete chain."""
        # Test with completely broken configuration
        mock_settings.DATABASES = {
            'broken': {
                'ENGINE': 'django.db.backends.postgresql',
                # Missing NAME
                'USER': 'test',
                'HOST': 'localhost',
            }
        }
        
        command = Command()
        command.stdout = io.StringIO()
        command.stderr = io.StringIO()
        
        with self.assertRaises(CommandError) as cm:
            command.handle(database='broken')
        
        self.assertIn('❌ Database configuration error', str(cm.exception))
        self.assertIn('database NAME is required', str(cm.exception))


if __name__ == '__main__':
    # Run the tests
    unittest.main(verbosity=2)