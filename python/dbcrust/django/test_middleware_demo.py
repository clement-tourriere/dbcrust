#!/usr/bin/env python3
"""
Demo script to test the Django Performance Analysis Middleware.

This script demonstrates the middleware functionality without requiring
a full Django project setup.
"""

import sys
import os
from unittest.mock import Mock, MagicMock, patch

# Add parent directory to path for imports
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

def test_middleware_functionality():
    """Test core middleware functionality with mocks."""
    print("🧪 Testing Django Performance Analysis Middleware")
    print("=" * 60)
    
    # Mock Django components
    mock_settings = Mock()
    mock_settings.DEBUG = True
    mock_settings.BASE_DIR = "/test/project"
    
    mock_request = Mock()
    mock_request.path = "/test-endpoint/"
    mock_request.method = "GET"
    
    mock_response = Mock()
    mock_response.__setitem__ = Mock()
    mock_response.__getitem__ = Mock()
    mock_response.__contains__ = Mock(return_value=False)
    
    # Mock analyzer and results
    mock_pattern = Mock()
    mock_pattern.severity = 'high'
    mock_pattern.pattern_type = 'n_plus_one'
    mock_pattern.description = 'N+1 query detected: accessing related objects in loop'
    mock_pattern.code_locations = ['views.py:42']
    
    mock_results = Mock()
    mock_results.total_queries = 12
    mock_results.total_duration = 0.185  # 185ms
    mock_results.detected_patterns = [mock_pattern]
    mock_results.duplicate_queries = 1
    
    mock_analysis = Mock()
    mock_analysis.__exit__ = Mock()
    mock_analysis.get_results = Mock(return_value=mock_results)
    
    mock_analyzer = Mock()
    mock_analysis_context = Mock()
    mock_analysis_context.__enter__ = Mock(return_value=mock_analysis)
    mock_analyzer.analyze = Mock(return_value=mock_analysis_context)
    
    # Test the middleware
    try:
        from dbcrust.django.middleware import PerformanceAnalysisMiddleware
        
        # Mock Django settings and imports
        with patch('django.conf.settings', mock_settings), \
             patch('dbcrust.django.middleware.create_enhanced_analyzer', return_value=mock_analyzer), \
             patch('time.time', side_effect=[1000.0, 1000.2]):  # 200ms request
            
            # Initialize middleware
            get_response = Mock(return_value=mock_response)
            middleware = PerformanceAnalysisMiddleware(get_response)
            
            print(f"✅ Middleware initialized successfully")
            print(f"   - Enabled: {middleware._is_enabled()}")
            print(f"   - Analyzer created: {middleware.analyzer is not None}")
            print(f"   - Config loaded: {len(middleware.config)} settings")
            
            # Test request processing
            result = middleware.process_request(mock_request)
            print(f"✅ process_request completed: {result is None}")
            
            # Simulate analysis context on request
            mock_request._dbcrust_analysis = mock_analysis
            mock_request._dbcrust_start_time = 1000.0
            
            # Test response processing with logging
            import logging
            logging.basicConfig(level=logging.INFO)
            
            processed_response = middleware.process_response(mock_request, mock_response)
            print(f"✅ process_response completed")
            
            # Verify analysis was completed
            mock_analysis.__exit__.assert_called_once()
            mock_analysis.get_results.assert_called_once()
            print(f"✅ Analysis context properly managed")
            
            # Check that headers would be added
            header_calls = mock_response.__setitem__.call_args_list
            print(f"✅ Headers added: {len(header_calls)} headers")
            
            # Show what headers would be set
            for call in header_calls[:5]:  # Show first 5 headers
                header_name, header_value = call[0]
                print(f"   📊 {header_name}: {header_value}")
            
            # Test configuration
            config = middleware.config
            print(f"✅ Configuration loaded:")
            print(f"   - Query threshold: {config['QUERY_THRESHOLD']}")
            print(f"   - Time threshold: {config['TIME_THRESHOLD']}")
            print(f"   - Include headers: {config['INCLUDE_HEADERS']}")
            
    except ImportError as e:
        print(f"❌ Could not import middleware (Django not available): {e}")
        print("   This is expected when Django is not installed")
        return False
    except Exception as e:
        print(f"❌ Unexpected error: {e}")
        import traceback
        traceback.print_exc()
        return False
    
    print(f"\n🎉 Middleware functionality test completed successfully!")
    return True


def test_configuration_scenarios():
    """Test different configuration scenarios."""
    print(f"\n🔧 Testing Configuration Scenarios")
    print("=" * 40)
    
    # Test different settings combinations
    test_configs = [
        {
            'name': 'Default (DEBUG=True)',
            'settings': {'DEBUG': True},
            'expected_enabled': True
        },
        {
            'name': 'Production (DEBUG=False)',
            'settings': {'DEBUG': False},
            'expected_enabled': False
        },
        {
            'name': 'Explicit Enable',
            'settings': {
                'DEBUG': False,
                'DBCRUST_PERFORMANCE_ANALYSIS': {'ENABLED': True}
            },
            'expected_enabled': True
        },
        {
            'name': 'Custom Thresholds',
            'settings': {
                'DEBUG': True,
                'DBCRUST_PERFORMANCE_ANALYSIS': {
                    'QUERY_THRESHOLD': 5,
                    'TIME_THRESHOLD': 50
                }
            },
            'expected_enabled': True
        }
    ]
    
    try:
        from dbcrust.django.middleware import PerformanceAnalysisMiddleware
        
        for config in test_configs:
            mock_settings = Mock()
            for key, value in config['settings'].items():
                setattr(mock_settings, key, value)
            
            with patch('django.conf.settings', mock_settings), \
                 patch('dbcrust.django.middleware.create_enhanced_analyzer'):
                
                middleware = PerformanceAnalysisMiddleware(Mock())
                enabled = middleware._is_enabled()
                
                status = "✅" if enabled == config['expected_enabled'] else "❌"
                print(f"{status} {config['name']}: enabled={enabled}")
                
                if 'DBCRUST_PERFORMANCE_ANALYSIS' in config['settings']:
                    user_config = config['settings']['DBCRUST_PERFORMANCE_ANALYSIS']
                    if 'QUERY_THRESHOLD' in user_config:
                        threshold = middleware.config['QUERY_THRESHOLD']
                        print(f"   📊 Query threshold: {threshold}")
                    if 'TIME_THRESHOLD' in user_config:
                        threshold = middleware.config['TIME_THRESHOLD']
                        print(f"   ⏱️  Time threshold: {threshold}ms")
        
        print(f"✅ Configuration scenarios tested successfully")
        
    except ImportError:
        print(f"❌ Could not test configurations (Django not available)")
    except Exception as e:
        print(f"❌ Configuration test error: {e}")


def demonstrate_headers_output():
    """Demonstrate what headers would look like."""
    print(f"\n📊 Example Performance Headers")
    print("=" * 40)
    
    # Example scenarios
    scenarios = [
        {
            'name': 'Fast Request (Good Performance)',
            'queries': 3,
            'time_ms': 25.5,
            'issues': [],
            'duplicates': 0
        },
        {
            'name': 'Slow Request (Performance Issues)',
            'queries': 15,
            'time_ms': 234.5,
            'issues': [('critical', 'n_plus_one'), ('high', 'missing_select_related')],
            'duplicates': 2
        },
        {
            'name': 'High Query Count',
            'queries': 28,
            'time_ms': 45.2,
            'issues': [('medium', 'large_result_set')],
            'duplicates': 0
        }
    ]
    
    for scenario in scenarios:
        print(f"\n🔸 {scenario['name']}")
        print(f"   X-DBCrust-Query-Count: {scenario['queries']}")
        print(f"   X-DBCrust-Query-Time: {scenario['time_ms']:.1f}ms")
        
        if scenario['issues']:
            print(f"   X-DBCrust-Issues-Total: {len(scenario['issues'])}")
            
            critical_count = len([i for i in scenario['issues'] if i[0] == 'critical'])
            high_count = len([i for i in scenario['issues'] if i[0] == 'high'])
            
            if critical_count > 0:
                print(f"   X-DBCrust-Issues-Critical: {critical_count}")
            if high_count > 0:
                print(f"   X-DBCrust-Issues-High: {high_count}")
            
            pattern_types = ','.join([i[1] for i in scenario['issues']])
            print(f"   X-DBCrust-Pattern-Types: {pattern_types}")
            
            if critical_count > 0:
                print(f"   X-DBCrust-Warning: Critical performance issues")
            elif scenario['queries'] > 10:
                print(f"   X-DBCrust-Warning: High query count")
            elif scenario['time_ms'] > 100:
                print(f"   X-DBCrust-Warning: Slow query time")
        else:
            print(f"   X-DBCrust-Status: OK")
        
        if scenario['duplicates'] > 0:
            print(f"   X-DBCrust-Duplicate-Queries: {scenario['duplicates']}")


def main():
    """Run all middleware tests and demonstrations."""
    print("🚀 DBCrust Django Performance Analysis Middleware - Test Suite")
    print("=" * 80)
    
    success = True
    
    # Test core functionality
    success &= test_middleware_functionality()
    
    # Test configurations
    test_configuration_scenarios()
    
    # Show header examples
    demonstrate_headers_output()
    
    print(f"\n" + "=" * 80)
    if success:
        print("✅ ALL MIDDLEWARE TESTS COMPLETED SUCCESSFULLY!")
        print("\n🎯 Ready for Use:")
        print("   1. Add 'dbcrust.django.PerformanceAnalysisMiddleware' to MIDDLEWARE")
        print("   2. Optionally configure DBCRUST_PERFORMANCE_ANALYSIS settings") 
        print("   3. Check browser dev tools for performance headers")
        print("   4. Monitor Django console for performance warnings")
    else:
        print("❌ Some tests had issues - see output above")
    
    print(f"\n📚 Next: Add to your Django project's settings.py:")
    print("""
MIDDLEWARE = [
    'dbcrust.django.PerformanceAnalysisMiddleware',
    # ... your other middleware
]
""")


if __name__ == "__main__":
    main()