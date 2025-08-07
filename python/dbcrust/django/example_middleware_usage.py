#!/usr/bin/env python3
"""
Example: Using DBCrust Performance Analysis Middleware in Django

This example shows how to integrate and use the Performance Analysis Middleware
in a Django project for automatic ORM performance monitoring.
"""

# ====================================================================
# STEP 1: Django Settings Configuration
# ====================================================================

"""
# settings.py or settings/development.py

MIDDLEWARE = [
    # Add at the beginning for comprehensive analysis
    'dbcrust.django.PerformanceAnalysisMiddleware',
    
    'django.middleware.security.SecurityMiddleware',
    'django.contrib.sessions.middleware.SessionMiddleware',
    'django.middleware.common.CommonMiddleware',
    'django.middleware.csrf.CsrfViewMiddleware',
    'django.contrib.auth.middleware.AuthenticationMiddleware',
    'django.contrib.messages.middleware.MessageMiddleware',
    'django.middleware.clickjacking.XFrameOptionsMiddleware',
]

# Optional: Configure performance analysis behavior
DBCRUST_PERFORMANCE_ANALYSIS = {
    'ENABLED': True,              # Force enable (overrides DEBUG)
    'QUERY_THRESHOLD': 5,         # Warn if > 5 queries (default: 10)
    'TIME_THRESHOLD': 50,         # Warn if > 50ms (default: 100ms)
    'LOG_ALL_REQUESTS': False,    # Only log problematic requests (default)
    'INCLUDE_HEADERS': True,      # Add performance headers (default)
    'ENABLE_CODE_ANALYSIS': False,# Enable full AST analysis (slower)
    'TRANSACTION_SAFE': False,    # Avoid Django session conflicts (default: False)
}

# Recommended: Set up logging to see performance issues
LOGGING = {
    'version': 1,
    'disable_existing_loggers': False,
    'formatters': {
        'verbose': {
            'format': '{levelname} {asctime} {name}: {message}',
            'style': '{',
        },
    },
    'handlers': {
        'console': {
            'class': 'logging.StreamHandler',
            'formatter': 'verbose',
        },
        'file': {
            'class': 'logging.handlers.RotatingFileHandler',
            'filename': 'logs/performance.log',
            'maxBytes': 10*1024*1024,  # 10MB
            'backupCount': 5,
            'formatter': 'verbose',
        },
    },
    'loggers': {
        'dbcrust.performance': {
            'handlers': ['console', 'file'],
            'level': 'INFO',
            'propagate': False,
        },
    },
}
"""

# ====================================================================
# STEP 2: What You'll See in Development
# ====================================================================

"""
BROWSER DEVELOPER TOOLS (Network Tab → Response Headers):

✅ Good Performance:
   X-DBCrust-Query-Count: 3
   X-DBCrust-Query-Time: 25.5ms
   X-DBCrust-Request-Time: 45.2ms
   X-DBCrust-Status: OK

⚠️ Performance Issues:
   X-DBCrust-Query-Count: 15
   X-DBCrust-Query-Time: 234.5ms
   X-DBCrust-Request-Time: 445.8ms
   X-DBCrust-Issues-Total: 3
   X-DBCrust-Issues-Critical: 1
   X-DBCrust-Issues-High: 2
   X-DBCrust-Pattern-Types: n_plus_one,missing_select_related
   X-DBCrust-Duplicate-Queries: 2
   X-DBCrust-Warning: Critical performance issues

DJANGO CONSOLE OUTPUT:

INFO:dbcrust.performance: GET /products/ | queries=3 | db_time=25.5ms | total_time=45.2ms
WARNING:dbcrust.performance: GET /books/ | queries=15 | db_time=234ms | total_time=445ms | issues=3
  🔸 n_plus_one: N+1 query detected: accessing related objects in loop (at views.py:42)
  🔸 missing_select_related: Use select_related() for foreign key relationships (at views.py:43)
  🔸 missing_prefetch_related: Use prefetch_related() for many-to-many relationships (at views.py:45)
ERROR:dbcrust.performance: POST /checkout/ | queries=28 | db_time=456ms | total_time=1234ms | issues=5
  🔸 n_plus_one: N+1 query detected: accessing related objects in loop (at checkout/views.py:156)
  🔸 bulk_operations: Individual saves that could be bulk operations (at checkout/views.py:178)
  🔸 large_result_set: Query without LIMIT that could cause memory issues (at checkout/views.py:134)
"""

# ====================================================================
# STEP 3: Using Headers in JavaScript/Frontend
# ====================================================================

"""
// JavaScript: Monitor performance in browser
document.addEventListener('DOMContentLoaded', function() {
    // Monitor AJAX requests for performance
    const originalFetch = window.fetch;
    window.fetch = function(...args) {
        return originalFetch(...args).then(response => {
            // Check for performance warnings
            const queryCount = response.headers.get('X-DBCrust-Query-Count');
            const queryTime = response.headers.get('X-DBCrust-Query-Time');
            const warning = response.headers.get('X-DBCrust-Warning');
            const issues = response.headers.get('X-DBCrust-Issues-Total');
            
            if (warning && parseInt(issues) > 0) {
                console.warn(
                    `⚠️ Performance Warning: ${warning}\\n` +
                    `Queries: ${queryCount}, Time: ${queryTime}, Issues: ${issues}`
                );
                
                // Optional: Show warning to developers
                if (localStorage.getItem('showPerformanceWarnings') === 'true') {
                    const patternTypes = response.headers.get('X-DBCrust-Pattern-Types');
                    showPerformanceWarning({
                        queries: queryCount,
                        time: queryTime,
                        issues: issues,
                        patterns: patternTypes?.split(',') || []
                    });
                }
            }
            
            return response;
        });
    };
});
"""


# ====================================================================
# STEP 4: Team Workflows and Monitoring
# ====================================================================

def show_team_integration_examples():
    """Examples of how teams can use the middleware."""

    print("🏢 Team Integration Examples")
    print("=" * 40)

    print("""
📋 CODE REVIEW CHECKLIST:
□ Check browser dev tools for X-DBCrust-* headers
□ Verify no Critical or High priority issues
□ Query count < 10 for typical pages
□ Database time < 100ms for standard requests

📊 MONITORING & ALERTS:
# Custom Django management command
class Command(BaseCommand):
    def handle(self, *args, **options):
        # Monitor performance across requests
        with analyzer.analyze() as analysis:
            # Run critical business logic
            process_daily_reports()
        
        results = analysis.get_results()
        if len([p for p in results.detected_patterns if p.severity == 'critical']) > 0:
            send_slack_alert("🚨 Critical performance issues detected in daily reports")

🧪 AUTOMATED TESTING:
# In test settings
DBCRUST_PERFORMANCE_ANALYSIS = {
    'ENABLED': True,
    'QUERY_THRESHOLD': 3,   # Strict limits in tests
    'TIME_THRESHOLD': 25,   # Very fast required
}

class ViewPerformanceTests(TestCase):
    def test_homepage_performance(self):
        response = self.client.get('/')
        
        # Check headers in test response
        query_count = int(response.get('X-DBCrust-Query-Count', '0'))
        self.assertLess(query_count, 5, "Homepage should use < 5 queries")
        
        issues = response.get('X-DBCrust-Issues-Total')
        self.assertIsNone(issues, "Homepage should have no performance issues")

📈 PERFORMANCE BUDGETS:
# settings/performance_budgets.py
PERFORMANCE_BUDGETS = {
    '/': {'queries': 5, 'time': 50},           # Homepage: 5 queries, 50ms
    '/products/': {'queries': 8, 'time': 100}, # Product list: 8 queries, 100ms
    '/checkout/': {'queries': 15, 'time': 200}, # Checkout: 15 queries, 200ms
}

🔄 CI/CD INTEGRATION:
# In CI pipeline
- name: Performance Gate
  run: |
    python manage.py test --pattern="test_performance_*"
    if [ $? -ne 0 ]; then
      echo "❌ Performance tests failed - check query patterns"
      exit 1
    fi
""")


# ====================================================================
# STEP 5: Advanced Configuration Examples
# ====================================================================

def show_advanced_configurations():
    """Show advanced configuration patterns."""

    print("\n🔧 Advanced Configuration Patterns")
    print("=" * 45)

    print("""
🎯 ENVIRONMENT-SPECIFIC SETTINGS:

# settings/base.py
DBCRUST_PERFORMANCE_ANALYSIS = {
    'ENABLED': False,  # Disabled by default
    'INCLUDE_HEADERS': True,
    'TRANSACTION_SAFE': False,  # Avoid session conflicts
}

# settings/development.py
from .base import *
DBCRUST_PERFORMANCE_ANALYSIS.update({
    'ENABLED': True,
    'QUERY_THRESHOLD': 8,
    'TIME_THRESHOLD': 75,
    'LOG_ALL_REQUESTS': True,        # Log everything in dev
    'ENABLE_CODE_ANALYSIS': True,    # Full analysis in dev
})

# settings/testing.py  
from .base import *
DBCRUST_PERFORMANCE_ANALYSIS.update({
    'ENABLED': True,
    'QUERY_THRESHOLD': 3,     # Strict limits for tests
    'TIME_THRESHOLD': 25,
    'LOG_ALL_REQUESTS': False,
})

# settings/staging.py
from .base import *
DBCRUST_PERFORMANCE_ANALYSIS.update({
    'ENABLED': True,
    'QUERY_THRESHOLD': 12,
    'TIME_THRESHOLD': 150,
    'LOG_ALL_REQUESTS': False, # Only log issues in staging
})

# settings/production.py - Usually disabled
from .base import *
# Keep ENABLED: False for production

🚀 SELECTIVE ACTIVATION:

# Activate only for specific paths
class ConditionalPerformanceMiddleware(PerformanceAnalysisMiddleware):
    MONITORED_PATHS = ['/api/', '/admin/', '/dashboard/']
    
    def _is_enabled(self):
        if not super()._is_enabled():
            return False
        
        # Only enable for specific paths
        request_path = getattr(self.current_request, 'path', '')
        return any(request_path.startswith(path) for path in self.MONITORED_PATHS)

🎛️ CUSTOM LOGGING:

# Custom logger with different handlers per severity
LOGGING['loggers']['dbcrust.performance'] = {
    'handlers': ['console', 'performance_file', 'slack_critical'],
    'level': 'INFO',
    'propagate': False,
}

LOGGING['handlers']['slack_critical'] = {
    'class': 'myapp.logging.SlackHandler',
    'level': 'ERROR',
    'webhook_url': 'https://hooks.slack.com/...',
}

📊 METRICS INTEGRATION:

# Send metrics to monitoring systems
import statsd
from dbcrust.django.middleware import PerformanceAnalysisMiddleware

class MetricsPerformanceMiddleware(PerformanceAnalysisMiddleware):
    def __init__(self, get_response):
        super().__init__(get_response)
        self.statsd = statsd.StatsClient('localhost', 8125)
    
    def _process_analysis_results(self, request, results, request_time):
        super()._process_analysis_results(request, results, request_time)
        
        # Send metrics
        self.statsd.gauge('django.queries.count', results.total_queries)
        self.statsd.gauge('django.queries.time', results.total_duration * 1000)
        self.statsd.gauge('django.issues.count', len(results.detected_patterns))
        
        # Track critical issues
        critical_count = len([p for p in results.detected_patterns if p.severity == 'critical'])
        self.statsd.gauge('django.issues.critical', critical_count)
""")


def main():
    """Show comprehensive middleware usage examples."""
    print("🚀 DBCrust Django Performance Analysis Middleware")
    print("📖 Comprehensive Usage Guide")
    print("=" * 60)

    show_team_integration_examples()
    show_advanced_configurations()

    print("\n" + "=" * 60)
    print("✅ READY TO USE!")
    print("=" * 60)
    print("""
🎯 Quick Start:
   1. Add 'dbcrust.django.PerformanceAnalysisMiddleware' to MIDDLEWARE
   2. Open browser dev tools → Network tab  
   3. Navigate your Django app
   4. Check response headers for X-DBCrust-* performance data

📊 What to Look For:
   • X-DBCrust-Query-Count: Number of database queries
   • X-DBCrust-Query-Time: Total database time
   • X-DBCrust-Issues-Total: Performance issues detected
   • X-DBCrust-Warning: Performance problem summary

🔍 Console Monitoring:
   • INFO: All request metrics (if LOG_ALL_REQUESTS=True)
   • WARNING: Requests exceeding thresholds
   • ERROR: Requests with critical performance issues

📈 Optimization Workflow:
   1. Identify high query counts or slow times in headers
   2. Check console logs for specific pattern types
   3. Use line numbers to find exact code locations
   4. Apply recommended fixes (select_related, prefetch_related, etc.)
   5. Verify improvements in subsequent requests

🎉 You now have comprehensive Django ORM performance monitoring!
""")


if __name__ == "__main__":
    main()
