"""
Tests for Django Performance Analysis Middleware.
"""

import logging
import unittest
from unittest.mock import Mock, patch, MagicMock
from django.test import TestCase, RequestFactory, override_settings
from django.http import HttpResponse

from ..middleware import PerformanceAnalysisMiddleware

try:
    import debug_toolbar  # noqa: F401
    HAS_DEBUG_TOOLBAR = True
except ImportError:
    HAS_DEBUG_TOOLBAR = False


class TestPerformanceAnalysisMiddleware(TestCase):
    """Test the performance analysis middleware."""

    def setUp(self):
        """Set up test fixtures."""
        self.factory = RequestFactory()
        self.get_response = Mock(return_value=HttpResponse())
        # Reset the warned-keys set between tests so warnings fire every time.
        from ..middleware import _warned_config_keys
        _warned_config_keys.clear()

    @override_settings(DEBUG=True)
    def test_middleware_enabled_in_debug_mode(self):
        """Test that middleware is enabled in DEBUG mode."""
        middleware = PerformanceAnalysisMiddleware(self.get_response)
        self.assertTrue(middleware._is_enabled())

    @override_settings(DEBUG=False)
    def test_middleware_disabled_in_production_mode(self):
        """Test that middleware is disabled when DEBUG=False."""
        middleware = PerformanceAnalysisMiddleware(self.get_response)
        self.assertFalse(middleware._is_enabled())

    @override_settings(
        DEBUG=False,
        DBCRUST_PERFORMANCE_ANALYSIS={'ENABLED': True}
    )
    def test_middleware_explicit_enable_overrides_debug(self):
        """Test that explicit configuration overrides DEBUG mode."""
        middleware = PerformanceAnalysisMiddleware(self.get_response)
        self.assertTrue(middleware._is_enabled())

    @override_settings(DEBUG=True)
    def test_config_loading_with_defaults(self):
        """Test configuration loading with default values."""
        middleware = PerformanceAnalysisMiddleware(self.get_response)

        self.assertEqual(middleware.config['QUERY_THRESHOLD'], 10)
        self.assertEqual(middleware.config['TIME_THRESHOLD'], 100)
        self.assertFalse(middleware.config['LOG_ALL_REQUESTS'])
        self.assertTrue(middleware.config['INCLUDE_HEADERS'])
        self.assertTrue(middleware.config['EXPLAIN_ENABLED'])
        self.assertEqual(middleware.config['EXPLAIN_SLOW_THRESHOLD_MS'], 100)
        self.assertEqual(middleware.config['EXPLAIN_MAX_QUERIES'], 5)

    @override_settings(
        DEBUG=True,
        DBCRUST_PERFORMANCE_ANALYSIS={
            'QUERY_THRESHOLD': 5,
            'TIME_THRESHOLD': 50,
            'LOG_ALL_REQUESTS': True,
        }
    )
    def test_config_loading_with_custom_settings(self):
        """Test configuration loading with custom settings."""
        middleware = PerformanceAnalysisMiddleware(self.get_response)

        self.assertEqual(middleware.config['QUERY_THRESHOLD'], 5)
        self.assertEqual(middleware.config['TIME_THRESHOLD'], 50)
        self.assertTrue(middleware.config['LOG_ALL_REQUESTS'])
        self.assertFalse(middleware.config['TRANSACTION_SAFE'])  # Default

    @override_settings(
        DEBUG=True,
        DBCRUST_PERFORMANCE_ANALYSIS={'TRANSACTION_SAFE': True}
    )
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_transaction_safe_configuration(self, mock_create_analyzer):
        """Test that TRANSACTION_SAFE configuration is passed through correctly."""
        mock_analyzer = Mock()
        mock_create_analyzer.return_value = mock_analyzer

        middleware = PerformanceAnalysisMiddleware(self.get_response)

        # Check config loaded correctly
        self.assertTrue(middleware.config['TRANSACTION_SAFE'])

        # Check passed to analyzer
        mock_create_analyzer.assert_called_once_with(
            transaction_safe=True,
            enable_all_features=False,
        )

    @override_settings(DEBUG=False)
    def test_middleware_disabled_no_analyzer_created(self):
        """Test that no analyzer is created when middleware is disabled."""
        middleware = PerformanceAnalysisMiddleware(self.get_response)
        self.assertIsNone(middleware.analyzer)

    @override_settings(DEBUG=True)
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_analyzer_initialization_with_defaults(self, mock_create_analyzer):
        """Test analyzer initialization with default settings."""
        mock_analyzer = Mock()
        mock_create_analyzer.return_value = mock_analyzer

        middleware = PerformanceAnalysisMiddleware(self.get_response)

        mock_create_analyzer.assert_called_once_with(
            transaction_safe=False,
            enable_all_features=False,
        )
        self.assertEqual(middleware.analyzer, mock_analyzer)

    @override_settings(DEBUG=True)
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_slow_query_analyzer_initialized(self, mock_create_analyzer):
        """Test that SlowQueryAnalyzer is initialised alongside the main analyzer."""
        mock_create_analyzer.return_value = Mock()

        middleware = PerformanceAnalysisMiddleware(self.get_response)

        self.assertIsNotNone(middleware._slow_query_analyzer)

    @override_settings(
        DEBUG=True,
        DBCRUST_PERFORMANCE_ANALYSIS={
            'EXPLAIN_SLOW_THRESHOLD_MS': 200,
        }
    )
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_custom_explain_threshold(self, mock_create_analyzer):
        """Test that custom EXPLAIN threshold is propagated."""
        mock_create_analyzer.return_value = Mock()

        middleware = PerformanceAnalysisMiddleware(self.get_response)

        self.assertEqual(
            middleware._slow_query_analyzer.thresholds.absolute_ms, 200
        )

    @override_settings(DEBUG=True)
    @patch('dbcrust.django.middleware.get_django_db_url', return_value='sqlite://:memory:')
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_non_postgres_auto_detected_db_url_uses_heuristics_only(
        self,
        mock_create_analyzer,
        mock_get_django_db_url,
    ):
        """Auto-detected non-Postgres URLs should not enable EXPLAIN mode."""
        mock_create_analyzer.return_value = Mock()

        middleware = PerformanceAnalysisMiddleware(self.get_response)

        mock_get_django_db_url.assert_called_once_with()
        self.assertIsNone(middleware._db_url)

    @override_settings(DEBUG=True)
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_deprecated_config_keys_warn(self, mock_create_analyzer):
        """Test that deprecated config keys emit a warning."""
        mock_create_analyzer.return_value = Mock()

        with override_settings(
            DBCRUST_PERFORMANCE_ANALYSIS={'CATEGORIZE_ISSUES': True}
        ):
            with self.assertLogs('dbcrust.performance', level='WARNING') as log:
                PerformanceAnalysisMiddleware(self.get_response)
            self.assertTrue(
                any('CATEGORIZE_ISSUES' in m and 'deprecated' in m for m in log.output)
            )

    @override_settings(DEBUG=False)
    def test_process_request_disabled_middleware(self):
        """Test process_request when middleware is disabled."""
        middleware = PerformanceAnalysisMiddleware(self.get_response)
        request = self.factory.get('/')

        result = middleware.process_request(request)

        self.assertIsNone(result)
        self.assertFalse(hasattr(request, '_dbcrust_analysis'))

    @override_settings(DEBUG=True)
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_process_request_starts_analysis(self, mock_create_analyzer):
        """Test that process_request starts analysis context."""
        # Set up mock analyzer
        mock_analysis_context = Mock()
        mock_analysis = Mock()
        mock_analysis_context.__enter__ = Mock(return_value=mock_analysis)

        mock_analyzer = Mock()
        mock_analyzer.analyze = Mock(return_value=mock_analysis_context)
        mock_create_analyzer.return_value = mock_analyzer

        middleware = PerformanceAnalysisMiddleware(self.get_response)
        request = self.factory.get('/')

        result = middleware.process_request(request)

        self.assertIsNone(result)
        mock_analyzer.analyze.assert_called_once()
        mock_analysis_context.__enter__.assert_called_once()
        self.assertEqual(request._dbcrust_analysis, mock_analysis)
        self.assertTrue(hasattr(request, '_dbcrust_start_time'))

    @override_settings(DEBUG=True)
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_process_response_completes_analysis(self, mock_create_analyzer):
        """Test that process_response completes analysis and adds headers."""
        # Set up mock analyzer and results
        mock_results = Mock()
        mock_results.total_queries = 5
        mock_results.total_duration = 0.045  # 45ms
        mock_results.detected_patterns = []
        mock_results.duplicate_queries = 0
        mock_results.queries_by_type = {'SELECT': 5}

        mock_analysis = Mock()
        mock_analysis.__exit__ = Mock()
        mock_analysis.get_results = Mock(return_value=mock_results)

        mock_analyzer = Mock()
        mock_analyzer.query_collector = Mock(queries=[])
        mock_create_analyzer.return_value = mock_analyzer

        middleware = PerformanceAnalysisMiddleware(self.get_response)

        # Set up request with analysis context
        request = self.factory.get('/')
        request._dbcrust_analysis = mock_analysis
        request._dbcrust_start_time = 123456789.0  # Mock timestamp

        response = HttpResponse()

        with patch('time.time', return_value=123456789.1):  # 100ms later
            result = middleware.process_response(request, response)

        # Verify analysis was completed
        mock_analysis.__exit__.assert_called_once_with(None, None, None)
        mock_analysis.get_results.assert_called_once()

        # Verify headers were added
        self.assertEqual(result['X-DBCrust-Query-Count'], '5')
        self.assertEqual(result['X-DBCrust-Query-Time'], '45.0ms')
        self.assertEqual(result['X-DBCrust-Request-Time'], '100.0ms')

    @override_settings(DEBUG=True)
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_threshold_only_request_adds_warning_header(self, mock_create_analyzer):
        """Threshold-only slow requests should still surface a warning header."""
        mock_results = Mock()
        mock_results.total_queries = 25
        mock_results.total_duration = 0.05
        mock_results.detected_patterns = []
        mock_results.duplicate_queries = 0
        mock_results.queries_by_type = {'SELECT': 25}

        mock_analysis = Mock()
        mock_analysis.__exit__ = Mock()
        mock_analysis.get_results = Mock(return_value=mock_results)

        mock_analyzer = Mock()
        mock_analyzer.query_collector = Mock(queries=[])
        mock_create_analyzer.return_value = mock_analyzer

        middleware = PerformanceAnalysisMiddleware(self.get_response)

        request = self.factory.get('/threshold-only/')
        request._dbcrust_analysis = mock_analysis
        request._dbcrust_start_time = 123456789.0

        response = HttpResponse()

        with patch('time.time', return_value=123456789.08):
            with self.assertLogs('dbcrust.performance', level='INFO'):
                result = middleware.process_response(request, response)

        self.assertEqual(result['X-DBCrust-Warning'], 'High query count')

    @override_settings(DEBUG=True, DBCRUST_PERFORMANCE_ANALYSIS={'INCLUDE_HEADERS': False})
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_process_response_no_headers_when_disabled(self, mock_create_analyzer):
        """Test that headers are not added when disabled in config."""
        mock_results = Mock()
        mock_results.total_queries = 3
        mock_results.total_duration = 0.025
        mock_results.detected_patterns = []
        mock_results.duplicate_queries = 0
        mock_results.queries_by_type = {'SELECT': 3}

        mock_analysis = Mock()
        mock_analysis.__exit__ = Mock()
        mock_analysis.get_results = Mock(return_value=mock_results)

        mock_analyzer = Mock()
        mock_analyzer.query_collector = Mock(queries=[])
        mock_create_analyzer.return_value = mock_analyzer

        middleware = PerformanceAnalysisMiddleware(self.get_response)

        request = self.factory.get('/')
        request._dbcrust_analysis = mock_analysis
        request._dbcrust_start_time = 123456789.0

        response = HttpResponse()

        with patch('time.time', return_value=123456789.05):
            result = middleware.process_response(request, response)

        # Verify no headers were added
        self.assertNotIn('X-DBCrust-Query-Count', result)

    @override_settings(DEBUG=True)
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_process_response_with_performance_issues(self, mock_create_analyzer):
        """Test response processing with detected performance issues."""
        # Create mock patterns -- enough critical issues to produce a D/F grade
        # which is logged at ERROR level.
        mock_pattern = Mock()
        mock_pattern.severity = 'critical'
        mock_pattern.pattern_type = 'n_plus_one'
        mock_pattern.description = 'N+1 query detected'
        mock_pattern.affected_queries = [Mock(), Mock()]
        mock_pattern.recommendation = 'Use prefetch_related'
        mock_pattern.code_suggestion = '.prefetch_related("items")'
        mock_pattern.code_locations = ['orders/views.py:45']
        mock_pattern.query_examples = ['SELECT * FROM items WHERE order_id=%s']

        mock_pattern2 = Mock()
        mock_pattern2.severity = 'critical'
        mock_pattern2.pattern_type = 'missing_select_related'
        mock_pattern2.description = 'Missing select_related on author FK'
        mock_pattern2.affected_queries = [Mock()]
        mock_pattern2.recommendation = 'Use select_related("author")'
        mock_pattern2.code_suggestion = '.select_related("author")'
        mock_pattern2.code_locations = ['orders/views.py:50']
        mock_pattern2.query_examples = ['SELECT * FROM authors WHERE id=%s']

        mock_results = Mock()
        mock_results.total_queries = 35
        mock_results.total_duration = 0.550  # 550ms -- triggers large db_time penalty
        mock_results.detected_patterns = [mock_pattern, mock_pattern2]
        mock_results.duplicate_queries = 8
        mock_results.queries_by_type = {'SELECT': 30, 'INSERT': 5}

        mock_analysis = Mock()
        mock_analysis.__exit__ = Mock()
        mock_analysis.get_results = Mock(return_value=mock_results)

        mock_analyzer = Mock()
        mock_analyzer.query_collector = Mock(queries=[])
        mock_create_analyzer.return_value = mock_analyzer

        middleware = PerformanceAnalysisMiddleware(self.get_response)

        request = self.factory.get('/test-path/')
        request._dbcrust_analysis = mock_analysis
        request._dbcrust_start_time = 123456789.0

        response = HttpResponse()

        with patch('time.time', return_value=123456789.6):  # 600ms later
            with self.assertLogs('dbcrust.performance', level='ERROR') as log:
                result = middleware.process_response(request, response)

        # Verify consolidated report was logged (single log call)
        self.assertEqual(len(log.records), 1, "Expected exactly 1 log record (consolidated report)")
        report_text = log.records[0].message
        self.assertIn('GET /test-path/', report_text)
        self.assertIn('Grade', report_text)
        self.assertIn('N+1 Query', report_text)
        self.assertIn('CRITICAL', report_text)

        # Verify warning headers were added
        self.assertEqual(result['X-DBCrust-Query-Count'], '35')
        self.assertEqual(result['X-DBCrust-Issues-Total'], '2')
        self.assertEqual(result['X-DBCrust-Issues-Critical'], '2')
        self.assertIn('n_plus_one', result['X-DBCrust-Pattern-Types'])
        self.assertEqual(result['X-DBCrust-Warning'], 'Critical performance issues')
        self.assertEqual(result['X-DBCrust-Duplicate-Queries'], '8')

    def test_process_response_without_analysis_context(self):
        """Test process_response when no analysis context exists."""
        middleware = PerformanceAnalysisMiddleware(self.get_response)
        request = self.factory.get('/')
        response = HttpResponse()

        result = middleware.process_response(request, response)

        self.assertEqual(result, response)
        # Should not add any headers when no analysis was performed
        self.assertNotIn('X-DBCrust-Query-Count', result)

    @override_settings(DEBUG=True)
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_process_exception_cleanup(self, mock_create_analyzer):
        """Test that process_exception cleans up analysis context."""
        mock_analysis = Mock()
        mock_analysis.__exit__ = Mock()

        mock_create_analyzer.return_value = Mock()

        middleware = PerformanceAnalysisMiddleware(self.get_response)

        request = self.factory.get('/')
        request._dbcrust_analysis = mock_analysis

        exception = ValueError("Test exception")
        result = middleware.process_exception(request, exception)

        # Should return None to not handle the exception
        self.assertIsNone(result)

        # Should have cleaned up the analysis context
        mock_analysis.__exit__.assert_called_once_with(
            ValueError, exception, exception.__traceback__
        )

    @override_settings(DEBUG=True)
    def test_debug_toolbar_compatibility_auto_disable(self):
        """Test that middleware auto-disables when Debug Toolbar profiling is active."""
        from django.conf import settings as _settings
        with (
            patch.object(_settings, 'INSTALLED_APPS', ['debug_toolbar']),
            patch.object(_settings, 'MIDDLEWARE', ['debug_toolbar.middleware.DebugToolbarMiddleware']),
            patch.object(_settings, 'DEBUG_TOOLBAR_PANELS', ['debug_toolbar.panels.profiling.ProfilingPanel'], create=True),
        ):
            middleware = PerformanceAnalysisMiddleware(self.get_response)

            # Should be disabled due to Debug Toolbar conflict
            self.assertFalse(middleware._is_enabled())
            self.assertIsNone(middleware.analyzer)

    @override_settings(
        DEBUG=True,
        DBCRUST_PERFORMANCE_ANALYSIS={'DEBUG_TOOLBAR_COMPATIBILITY': False}
    )
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_debug_toolbar_compatibility_override(self, mock_create_analyzer):
        """Test that DEBUG_TOOLBAR_COMPATIBILITY=False overrides auto-disable."""
        mock_analyzer = Mock()
        mock_create_analyzer.return_value = mock_analyzer

        from django.conf import settings as _settings
        with (
            patch.object(_settings, 'INSTALLED_APPS', ['debug_toolbar']),
            patch.object(_settings, 'MIDDLEWARE', ['debug_toolbar.middleware.DebugToolbarMiddleware']),
            patch.object(_settings, 'DEBUG_TOOLBAR_PANELS', ['debug_toolbar.panels.profiling.ProfilingPanel'], create=True),
        ):
            middleware = PerformanceAnalysisMiddleware(self.get_response)

            # Should be enabled despite Debug Toolbar due to override
            self.assertTrue(middleware._is_enabled())
            self.assertIsNotNone(middleware.analyzer)

    @override_settings(DEBUG=True)
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_debug_toolbar_no_profiling_no_conflict(self, mock_create_analyzer):
        """Test that Debug Toolbar without profiling doesn't cause conflict."""
        mock_analyzer = Mock()
        mock_create_analyzer.return_value = mock_analyzer

        from django.conf import settings as _settings
        with (
            patch.object(_settings, 'INSTALLED_APPS', ['debug_toolbar']),
            patch.object(_settings, 'DEBUG_TOOLBAR_PANELS', ['debug_toolbar.panels.sql.SQLPanel'], create=True),
        ):
            middleware = PerformanceAnalysisMiddleware(self.get_response)

            # Should be enabled - no profiling conflict
            self.assertTrue(middleware._is_enabled())
            self.assertIsNotNone(middleware.analyzer)

    @override_settings(DEBUG=True)
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_debug_toolbar_installed_but_not_active_no_conflict(self, mock_create_analyzer):
        """Test that Debug Toolbar installed but not in middleware doesn't cause conflict."""
        mock_analyzer = Mock()
        mock_create_analyzer.return_value = mock_analyzer

        from django.conf import settings as _settings
        with (
            patch.object(_settings, 'INSTALLED_APPS', ['debug_toolbar']),
            patch.object(_settings, 'MIDDLEWARE', []),
            patch.object(_settings, 'DEBUG_TOOLBAR_PANELS', ['debug_toolbar.panels.profiling.ProfilingPanel'], create=True),
        ):
            middleware = PerformanceAnalysisMiddleware(self.get_response)

            # Should be enabled - Debug Toolbar is installed but not active
            self.assertTrue(middleware._is_enabled())
            self.assertIsNotNone(middleware.analyzer)

    @override_settings(DEBUG=True)
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_consolidated_report_grade_in_output(self, mock_create_analyzer):
        """Test that the consolidated report includes a grade."""
        mock_pattern = Mock()
        mock_pattern.severity = 'high'
        mock_pattern.pattern_type = 'redundant_queries'
        mock_pattern.description = 'Same query executed 5 times'
        mock_pattern.affected_queries = [Mock()]
        mock_pattern.recommendation = 'Cache the result'
        mock_pattern.code_suggestion = None
        mock_pattern.code_locations = []
        mock_pattern.query_examples = []

        mock_results = Mock()
        mock_results.total_queries = 25
        mock_results.total_duration = 0.150
        mock_results.detected_patterns = [mock_pattern]
        mock_results.duplicate_queries = 5
        mock_results.queries_by_type = {'SELECT': 20, 'UPDATE': 5}

        mock_analysis = Mock()
        mock_analysis.__exit__ = Mock()
        mock_analysis.get_results = Mock(return_value=mock_results)

        mock_analyzer = Mock()
        mock_analyzer.query_collector = Mock(queries=[])
        mock_create_analyzer.return_value = mock_analyzer

        middleware = PerformanceAnalysisMiddleware(self.get_response)

        request = self.factory.get('/api/items/')
        request._dbcrust_analysis = mock_analysis
        request._dbcrust_start_time = 123456789.0

        response = HttpResponse()

        with patch('time.time', return_value=123456789.2):
            with self.assertLogs('dbcrust.performance', level='WARNING') as log:
                middleware.process_response(request, response)

        report_text = log.records[0].message
        self.assertIn('Grade', report_text)
        self.assertIn('Queries: 25', report_text)
        self.assertIn('Dupes: 5', report_text)

    @override_settings(DEBUG=True)
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_view_name_resolved(self, mock_create_analyzer):
        """Test that view name is resolved and included in the report."""
        mock_results = Mock()
        mock_results.total_queries = 20
        mock_results.total_duration = 0.200
        mock_results.detected_patterns = []
        mock_results.duplicate_queries = 0
        mock_results.queries_by_type = {'SELECT': 20}

        mock_analysis = Mock()
        mock_analysis.__exit__ = Mock()
        mock_analysis.get_results = Mock(return_value=mock_results)

        mock_analyzer = Mock()
        mock_analyzer.query_collector = Mock(queries=[])
        mock_create_analyzer.return_value = mock_analyzer

        middleware = PerformanceAnalysisMiddleware(self.get_response)

        request = self.factory.get('/api/orders/')
        request._dbcrust_analysis = mock_analysis
        request._dbcrust_start_time = 123456789.0

        # Simulate resolved view
        request.resolver_match = Mock()
        request.resolver_match.view_name = 'orders:order-list'

        response = HttpResponse()

        # 20 queries > QUERY_THRESHOLD(10) triggers logging;
        # grade B → logged at INFO level.
        with patch('time.time', return_value=123456789.25):
            with self.assertLogs('dbcrust.performance', level='INFO') as log:
                middleware.process_response(request, response)

        report_text = log.records[-1].message
        self.assertIn('orders:order-list', report_text)

    @override_settings(DEBUG=True)
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_below_threshold_no_issues_no_log(self, mock_create_analyzer):
        """Test that a fast, low-query request with no issues produces no log."""
        mock_results = Mock()
        mock_results.total_queries = 3     # below QUERY_THRESHOLD (10)
        mock_results.total_duration = 0.02  # 20ms, below TIME_THRESHOLD (100)
        mock_results.detected_patterns = []
        mock_results.duplicate_queries = 0
        mock_results.queries_by_type = {'SELECT': 3}

        mock_analysis = Mock()
        mock_analysis.__exit__ = Mock()
        mock_analysis.get_results = Mock(return_value=mock_results)

        mock_analyzer = Mock()
        mock_analyzer.query_collector = Mock(queries=[])
        mock_create_analyzer.return_value = mock_analyzer

        middleware = PerformanceAnalysisMiddleware(self.get_response)

        request = self.factory.get('/fast/')
        request._dbcrust_analysis = mock_analysis
        request._dbcrust_start_time = 123456789.0

        response = HttpResponse()

        with patch('time.time', return_value=123456789.03):
            # Should NOT produce any log output
            with self.assertRaises(AssertionError):
                # assertLogs raises AssertionError when no logs are emitted
                with self.assertLogs('dbcrust.performance', level='DEBUG'):
                    middleware.process_response(request, response)

    @override_settings(
        DEBUG=True,
        DBCRUST_PERFORMANCE_ANALYSIS={'QUERY_THRESHHOLD': 5}  # Typo!
    )
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_unknown_config_key_warns(self, mock_create_analyzer):
        """Test that unknown config keys produce a warning (issue #9)."""
        mock_create_analyzer.return_value = Mock()

        with self.assertLogs('dbcrust.performance', level='WARNING') as log:
            PerformanceAnalysisMiddleware(self.get_response)
        self.assertTrue(
            any('QUERY_THRESHHOLD' in m and 'unknown' in m for m in log.output)
        )

    @override_settings(
        DEBUG=False,
        DBCRUST_PERFORMANCE_ANALYSIS={'ENABLED': True},
    )
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_explicit_enabled_with_debug_toolbar_stays_enabled(self, mock_create_analyzer):
        """Issue #2: ENABLED=True with Debug Toolbar conflict should stay enabled."""
        mock_create_analyzer.return_value = Mock()

        from django.conf import settings as _settings
        with (
            patch.object(_settings, 'INSTALLED_APPS', ['debug_toolbar']),
            patch.object(_settings, 'MIDDLEWARE', ['debug_toolbar.middleware.DebugToolbarMiddleware']),
        ):
            middleware = PerformanceAnalysisMiddleware(self.get_response)
            self.assertTrue(middleware._is_enabled())

    @override_settings(DEBUG=True)
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_request_time_header_consistent_units(self, mock_create_analyzer):
        """Issue #8: X-DBCrust-Request-Time should be in ms, matching report."""
        mock_results = Mock()
        mock_results.total_queries = 20
        mock_results.total_duration = 0.1
        mock_results.detected_patterns = []
        mock_results.duplicate_queries = 0
        mock_results.queries_by_type = {'SELECT': 20}

        mock_analysis = Mock()
        mock_analysis.__exit__ = Mock()
        mock_analysis.get_results = Mock(return_value=mock_results)

        mock_analyzer = Mock()
        mock_analyzer.query_collector = Mock(queries=[])
        mock_create_analyzer.return_value = mock_analyzer

        middleware = PerformanceAnalysisMiddleware(self.get_response)

        request = self.factory.get('/')
        request._dbcrust_analysis = mock_analysis
        request._dbcrust_start_time = 1000.0

        response = HttpResponse()

        with patch('time.time', return_value=1000.25):
            with self.assertLogs('dbcrust.performance', level='INFO'):
                result = middleware.process_response(request, response)

        # Should be 250ms, not some epoch-scale number
        self.assertEqual(result['X-DBCrust-Request-Time'], '250.0ms')
