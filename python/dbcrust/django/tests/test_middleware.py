"""
Tests for Django Performance Analysis Middleware.
"""

import logging
from unittest.mock import Mock, patch, MagicMock
from django.test import TestCase, RequestFactory, override_settings
from django.http import HttpResponse

from ..middleware import PerformanceAnalysisMiddleware


class TestPerformanceAnalysisMiddleware(TestCase):
    """Test the performance analysis middleware."""
    
    def setUp(self):
        """Set up test fixtures."""
        self.factory = RequestFactory()
        self.get_response = Mock(return_value=HttpResponse())
        
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
            project_root=None,
            enable_all_features=False,
            transaction_safe=True
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
            project_root=None,
            enable_all_features=False,
            transaction_safe=False
        )
        self.assertEqual(middleware.analyzer, mock_analyzer)
    
    @override_settings(
        DEBUG=True,
        BASE_DIR='/test/project',
        DBCRUST_PERFORMANCE_ANALYSIS={'ENABLE_CODE_ANALYSIS': True}
    )
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_analyzer_initialization_with_code_analysis(self, mock_create_analyzer):
        """Test analyzer initialization with code analysis enabled."""
        mock_analyzer = Mock()
        mock_create_analyzer.return_value = mock_analyzer
        
        middleware = PerformanceAnalysisMiddleware(self.get_response)
        
        mock_create_analyzer.assert_called_once_with(
            project_root='/test/project',
            enable_all_features=True,
            transaction_safe=False
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
        
        mock_analysis = Mock()
        mock_analysis.__exit__ = Mock()
        mock_analysis.get_results = Mock(return_value=mock_results)
        
        mock_analyzer = Mock()
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
        self.assertEqual(result['X-DBCrust-Status'], 'OK')
    
    @override_settings(DEBUG=True, DBCRUST_PERFORMANCE_ANALYSIS={'INCLUDE_HEADERS': False})
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_process_response_no_headers_when_disabled(self, mock_create_analyzer):
        """Test that headers are not added when disabled in config."""
        mock_results = Mock()
        mock_results.total_queries = 3
        mock_results.total_duration = 0.025
        mock_results.detected_patterns = []
        mock_results.duplicate_queries = 0
        
        mock_analysis = Mock()
        mock_analysis.__exit__ = Mock()
        mock_analysis.get_results = Mock(return_value=mock_results)
        
        mock_create_analyzer.return_value = Mock()
        
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
        # Create mock pattern
        mock_pattern = Mock()
        mock_pattern.severity = 'critical'
        mock_pattern.pattern_type = 'n_plus_one'
        mock_pattern.description = 'N+1 query detected'
        
        mock_results = Mock()
        mock_results.total_queries = 15
        mock_results.total_duration = 0.250  # 250ms
        mock_results.detected_patterns = [mock_pattern]
        mock_results.duplicate_queries = 2
        
        mock_analysis = Mock()
        mock_analysis.__exit__ = Mock()
        mock_analysis.get_results = Mock(return_value=mock_results)
        
        mock_create_analyzer.return_value = Mock()
        
        middleware = PerformanceAnalysisMiddleware(self.get_response)
        
        request = self.factory.get('/test-path/')
        request._dbcrust_analysis = mock_analysis
        request._dbcrust_start_time = 123456789.0
        
        response = HttpResponse()
        
        with patch('time.time', return_value=123456789.3):  # 300ms later
            with self.assertLogs('dbcrust.performance', level='ERROR') as log:
                result = middleware.process_response(request, response)
        
        # Verify error was logged
        self.assertTrue(any('GET /test-path/' in record.message for record in log.records))
        self.assertTrue(any('n_plus_one: N+1 query detected' in record.message for record in log.records))
        
        # Verify warning headers were added
        self.assertEqual(result['X-DBCrust-Query-Count'], '15')
        self.assertEqual(result['X-DBCrust-Issues-Total'], '1')
        self.assertEqual(result['X-DBCrust-Issues-Critical'], '1')
        self.assertEqual(result['X-DBCrust-Pattern-Types'], 'n_plus_one')
        self.assertEqual(result['X-DBCrust-Warning'], 'Critical performance issues')
        self.assertEqual(result['X-DBCrust-Duplicate-Queries'], '2')
    
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
    
    @override_settings(
        DEBUG=True,
        INSTALLED_APPS=['debug_toolbar'],
        MIDDLEWARE=['debug_toolbar.middleware.DebugToolbarMiddleware'],  # Active in middleware
        DEBUG_TOOLBAR_PANELS=['debug_toolbar.panels.profiling.ProfilingPanel']
    )
    def test_debug_toolbar_compatibility_auto_disable(self):
        """Test that middleware auto-disables when Debug Toolbar profiling is active."""
        middleware = PerformanceAnalysisMiddleware(self.get_response)
        
        # Should be disabled due to Debug Toolbar conflict
        self.assertFalse(middleware._is_enabled())
        self.assertIsNone(middleware.analyzer)
    
    @override_settings(
        DEBUG=True,
        INSTALLED_APPS=['debug_toolbar'],
        MIDDLEWARE=['debug_toolbar.middleware.DebugToolbarMiddleware'],
        DEBUG_TOOLBAR_PANELS=['debug_toolbar.panels.profiling.ProfilingPanel'],
        DBCRUST_PERFORMANCE_ANALYSIS={'DEBUG_TOOLBAR_COMPATIBILITY': False}
    )
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_debug_toolbar_compatibility_override(self, mock_create_analyzer):
        """Test that DEBUG_TOOLBAR_COMPATIBILITY=False overrides auto-disable."""
        mock_analyzer = Mock()
        mock_create_analyzer.return_value = mock_analyzer
        
        middleware = PerformanceAnalysisMiddleware(self.get_response)
        
        # Should be enabled despite Debug Toolbar due to override
        self.assertTrue(middleware._is_enabled())
        self.assertIsNotNone(middleware.analyzer)
    
    @override_settings(
        DEBUG=True,
        INSTALLED_APPS=['debug_toolbar'],
        DEBUG_TOOLBAR_PANELS=['debug_toolbar.panels.sql.SQLPanel']  # No profiling panel
    )
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_debug_toolbar_no_profiling_no_conflict(self, mock_create_analyzer):
        """Test that Debug Toolbar without profiling doesn't cause conflict."""
        mock_analyzer = Mock()
        mock_create_analyzer.return_value = mock_analyzer
        
        middleware = PerformanceAnalysisMiddleware(self.get_response)
        
        # Should be enabled - no profiling conflict
        self.assertTrue(middleware._is_enabled())
        self.assertIsNotNone(middleware.analyzer)
    
    @override_settings(
        DEBUG=True,
        INSTALLED_APPS=['debug_toolbar'],
        MIDDLEWARE=[],  # Debug Toolbar NOT in middleware
        DEBUG_TOOLBAR_PANELS=['debug_toolbar.panels.profiling.ProfilingPanel']
    )
    @patch('dbcrust.django.middleware.create_enhanced_analyzer')
    def test_debug_toolbar_installed_but_not_active_no_conflict(self, mock_create_analyzer):
        """Test that Debug Toolbar installed but not in middleware doesn't cause conflict."""
        mock_analyzer = Mock()
        mock_create_analyzer.return_value = mock_analyzer
        
        middleware = PerformanceAnalysisMiddleware(self.get_response)
        
        # Should be enabled - Debug Toolbar is installed but not active
        self.assertTrue(middleware._is_enabled())
        self.assertIsNotNone(middleware.analyzer)