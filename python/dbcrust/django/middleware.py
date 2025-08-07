"""
Django Performance Analysis Middleware

Automatically analyzes Django ORM performance for each request during development,
providing detailed insights about N+1 queries, missing optimizations, and other
performance issues.

Usage:
    Add to your Django MIDDLEWARE setting:
    
    MIDDLEWARE = [
        # Django Debug Toolbar (if used) - should come first
        'debug_toolbar.middleware.DebugToolbarMiddleware',
        
        # DBCrust Performance Analysis - early in the stack for complete analysis
        'dbcrust.django.PerformanceAnalysisMiddleware',
        
        # ... your other middleware
        'django.middleware.security.SecurityMiddleware',
        'django.contrib.sessions.middleware.SessionMiddleware',
        # ... etc
    ]

Configuration:
    # settings.py - Optional configuration
    DBCRUST_PERFORMANCE_ANALYSIS = {
        'ENABLED': True,              # Override DEBUG mode
        'QUERY_THRESHOLD': 10,        # Log requests with >10 queries (set to 1 to see all)
        'TIME_THRESHOLD': 100,        # Log requests taking >100ms (set to 1 to see all)
        'LOG_ALL_REQUESTS': False,    # Log ALL requests (ignores thresholds)
        'INCLUDE_HEADERS': True,      # Add performance headers to responses
        'ENABLE_CODE_ANALYSIS': False,# Enable AST code analysis (slower)
        'TRANSACTION_SAFE': False,    # Wrap in transaction (may interfere with sessions)
        'DEBUG_TOOLBAR_COMPATIBILITY': True,  # Auto-disable to avoid Debug Toolbar conflicts
        'MAX_ISSUES_DISPLAYED': 10,   # Max issues to show in logs (set higher to see all)
        'SHOW_SQL_IN_LOGS': True,     # Show SQL queries in logs for better debugging
        'GROUP_DUPLICATE_ISSUES': True,  # Group same issue types (e.g., 3x large_result_set)
        'MAX_SQL_LENGTH': 200,        # Truncate long SQL queries in logs
    }
    
    Note: Requests with detected performance patterns will ALWAYS be logged,
    regardless of QUERY_THRESHOLD/TIME_THRESHOLD settings.
"""

import logging
import sys
import time
from typing import Optional, Dict, Any

from django.conf import settings
from django.utils.deprecation import MiddlewareMixin
from django.http import HttpRequest, HttpResponse

from .analyzer import create_enhanced_analyzer, DjangoAnalyzer

# Set up dedicated logger for performance analysis
logger = logging.getLogger('dbcrust.performance')


class PerformanceAnalysisMiddleware(MiddlewareMixin):
    """
    Django middleware for automatic ORM performance analysis.
    
    Captures and analyzes Django ORM queries for each request, detecting
    N+1 patterns, missing optimizations, and providing actionable insights.
    """
    
    def __init__(self, get_response):
        """Initialize the middleware with configuration."""
        self.get_response = get_response
        self.analyzer: Optional[DjangoAnalyzer] = None
        self.config = self._load_config()
        
        # Only initialize analyzer if enabled
        if self._is_enabled():
            self._initialize_analyzer()
        
        super().__init__(get_response)
    
    def _load_config(self) -> Dict[str, Any]:
        """Load middleware configuration from Django settings."""
        default_config = {
            'ENABLED': None,  # None means use DEBUG mode
            'QUERY_THRESHOLD': 10,
            'TIME_THRESHOLD': 100,  # milliseconds
            'LOG_ALL_REQUESTS': False,
            'INCLUDE_HEADERS': True,
            'ENABLE_CODE_ANALYSIS': False,
            'PROJECT_ROOT': None,  # Auto-detect from BASE_DIR
            'TRANSACTION_SAFE': False,  # Avoid interfering with session management
            'DEBUG_TOOLBAR_COMPATIBILITY': True,  # Auto-disable when Debug Toolbar conflicts
            'DEBUG_LOGGING': False,  # Also print to stderr for debugging
            'MAX_ISSUES_DISPLAYED': 10,  # Maximum number of issues to show in logs
            'SHOW_SQL_IN_LOGS': True,  # Show SQL queries in logs
            'GROUP_DUPLICATE_ISSUES': True,  # Group same issue types
            'MAX_SQL_LENGTH': 200,  # Max length for SQL queries in logs
            # New framework filtering options
            'SHOW_FULL_PATHS': True,  # Show complete file paths for navigation
            'SUPPRESS_FRAMEWORK_ISSUES': False,  # Hide Django admin/framework issues
            'FRAMEWORK_ISSUE_THRESHOLD': 'medium',  # critical|high|medium|low - minimum severity to show framework issues
            'CATEGORIZE_ISSUES': True,  # Separate user code from framework issues
            'ADMIN_SPECIFIC_ADVICE': True,  # Provide admin-specific recommendations
        }
        
        # Merge with user settings
        user_config = getattr(settings, 'DBCRUST_PERFORMANCE_ANALYSIS', {})
        config = {**default_config, **user_config}
        
        # Auto-detect project root if not specified
        if config['PROJECT_ROOT'] is None and hasattr(settings, 'BASE_DIR'):
            config['PROJECT_ROOT'] = str(settings.BASE_DIR)
        
        return config
    
    def _is_enabled(self) -> bool:
        """Check if performance analysis should be enabled."""
        # Explicit configuration overrides DEBUG mode
        if self.config['ENABLED'] is not None:
            enabled = self.config['ENABLED']
        else:
            # Default: only enable in DEBUG mode
            enabled = getattr(settings, 'DEBUG', False)
        
        # Check for Django Debug Toolbar to avoid conflicts
        if (enabled and 
            self.config['DEBUG_TOOLBAR_COMPATIBILITY'] and 
            self._has_debug_toolbar_conflict()):
            logger.warning(
                "DBCrust Performance Analysis Middleware disabled: "
                "Django Debug Toolbar middleware is active with profiling enabled. "
                "To use both, either: 1) Remove ProfilingPanel from DEBUG_TOOLBAR_PANELS, or "
                "2) Set DBCRUST_PERFORMANCE_ANALYSIS = {'DEBUG_TOOLBAR_COMPATIBILITY': False}"
            )
            # Only disable if not explicitly enabled
            if self.config['ENABLED'] is None:
                return False
        
        return enabled
    
    def _has_debug_toolbar_conflict(self) -> bool:
        """Check if Django Debug Toolbar is configured in a way that would conflict."""
        try:
            # First check if debug_toolbar is in INSTALLED_APPS
            installed_apps = getattr(settings, 'INSTALLED_APPS', [])
            if 'debug_toolbar' not in installed_apps:
                return False
            
            # IMPORTANT: Check if Debug Toolbar middleware is actually active
            middleware = getattr(settings, 'MIDDLEWARE', [])
            debug_toolbar_middleware = 'debug_toolbar.middleware.DebugToolbarMiddleware'
            
            # If Debug Toolbar is installed but not in middleware, no conflict
            if debug_toolbar_middleware not in middleware:
                return False
            
            # Check if profiling panel is enabled
            debug_toolbar_panels = getattr(settings, 'DEBUG_TOOLBAR_PANELS', [])
            profiling_panel = 'debug_toolbar.panels.profiling.ProfilingPanel'
            
            # If no panels configured, it uses defaults which include profiling
            if not debug_toolbar_panels:
                return True
            
            return profiling_panel in debug_toolbar_panels
            
        except Exception:
            # If we can't determine, assume no conflict
            return False
    
    def _initialize_analyzer(self):
        """Initialize the enhanced analyzer with configuration."""
        try:
            # Create enhanced analyzer with appropriate features
            enable_code_analysis = (
                self.config['ENABLE_CODE_ANALYSIS'] and 
                self.config['PROJECT_ROOT'] is not None
            )
            
            self.analyzer = create_enhanced_analyzer(
                project_root=self.config['PROJECT_ROOT'] if enable_code_analysis else None,
                enable_all_features=enable_code_analysis,
                transaction_safe=self.config['TRANSACTION_SAFE']
            )
            
            logger.info(f"DBCrust Performance Analysis Middleware initialized")
            logger.info(f"  - LOG_ALL_REQUESTS: {self.config['LOG_ALL_REQUESTS']}")
            logger.info(f"  - QUERY_THRESHOLD: {self.config['QUERY_THRESHOLD']} (requests with >{self.config['QUERY_THRESHOLD']} queries will be logged)")
            logger.info(f"  - TIME_THRESHOLD: {self.config['TIME_THRESHOLD']}ms (requests taking >{self.config['TIME_THRESHOLD']}ms will be logged)")
            logger.info(f"  - Detected patterns will ALWAYS be logged regardless of thresholds")
            
        except Exception as e:
            logger.warning(f"Could not initialize performance analyzer: {e}")
            self.analyzer = None
    
    def process_request(self, request: HttpRequest):
        """Start performance analysis for this request."""
        if not self.analyzer:
            return None
        
        try:
            # Start analysis context and store it on the request
            analysis_context = self.analyzer.analyze()
            request._dbcrust_analysis = analysis_context.__enter__()
            request._dbcrust_start_time = time.time()
            
        except Exception as e:
            logger.debug(f"Could not start performance analysis: {e}")
            # Don't fail the request if analysis fails
            return None
    
    def process_response(self, request: HttpRequest, response: HttpResponse) -> HttpResponse:
        """Complete performance analysis and add insights."""
        if not hasattr(request, '_dbcrust_analysis'):
            return response
        
        try:
            # Complete the analysis
            analysis = request._dbcrust_analysis
            analysis.__exit__(None, None, None)
            
            # Get analysis results
            results = analysis.get_results()
            if not results:
                return response
            
            # Calculate request timing
            request_time = time.time() - getattr(request, '_dbcrust_start_time', 0)
            
            # Process results
            self._process_analysis_results(request, results, request_time)
            
            # Add performance headers if enabled
            if self.config['INCLUDE_HEADERS']:
                self._add_performance_headers(response, results, request_time)
            
        except Exception as e:
            logger.debug(f"Error processing performance analysis: {e}")
            # Don't fail the request if analysis processing fails
        
        return response
    
    def _process_analysis_results(self, request: HttpRequest, results, request_time: float):
        """Process and log performance analysis results."""
        # Extract key metrics
        query_count = results.total_queries
        query_time_ms = results.total_duration * 1000
        request_time_ms = request_time * 1000
        
        # Count issues by severity
        critical_issues = [p for p in results.detected_patterns if p.severity == 'critical']
        high_issues = [p for p in results.detected_patterns if p.severity == 'high']
        all_issues = results.detected_patterns
        
        # Determine if this request has performance concerns
        has_query_concerns = query_count > self.config['QUERY_THRESHOLD']
        has_time_concerns = query_time_ms > self.config['TIME_THRESHOLD']
        has_critical_issues = len(critical_issues) > 0
        has_any_issues = len(all_issues) > 0
        has_performance_issues = has_query_concerns or has_time_concerns or has_critical_issues or has_any_issues
        
        # Determine log level
        log_level = logging.INFO
        if has_critical_issues:
            log_level = logging.ERROR
        elif has_any_issues or has_query_concerns or has_time_concerns:
            log_level = logging.WARNING
        
        # Log if configured to log all requests OR if there are any issues/concerns
        should_log = self.config['LOG_ALL_REQUESTS'] or has_performance_issues
        
        if should_log:
            # Build log message
            path = getattr(request, 'path', '?')
            method = getattr(request, 'method', '?')
            
            message_parts = [
                f"{method} {path}",
                f"queries={query_count}",
                f"db_time={query_time_ms:.1f}ms",
                f"total_time={request_time_ms:.1f}ms"
            ]
            
            if all_issues:
                message_parts.append(f"issues={len(all_issues)}")
            
            log_message = " | ".join(message_parts)
            logger.log(log_level, log_message)
            
            # Also print to stderr for debugging if logging seems broken
            if self.config.get('DEBUG_LOGGING', False):
                print(f"DBCrust: {log_message}", file=sys.stderr)
            
            # Log specific issues with categorization
            if all_issues:
                if self.config['CATEGORIZE_ISSUES']:
                    self._log_categorized_issues(all_issues, log_level)
                elif self.config['GROUP_DUPLICATE_ISSUES']:
                    self._log_grouped_issues(all_issues, log_level)
                else:
                    self._log_individual_issues(all_issues, log_level)
    
    def _log_categorized_issues(self, issues, log_level):
        """Log issues separated by user code vs framework code."""
        # Separate user code from framework issues
        user_issues = [issue for issue in issues if getattr(issue, 'is_user_code', True)]
        framework_issues = [issue for issue in issues if not getattr(issue, 'is_user_code', True)]
        
        # Apply framework filtering
        if self.config['SUPPRESS_FRAMEWORK_ISSUES']:
            framework_issues = []
        else:
            # Filter by framework issue threshold
            severity_order = {'critical': 0, 'high': 1, 'medium': 2, 'low': 3}
            threshold = severity_order.get(self.config['FRAMEWORK_ISSUE_THRESHOLD'], 2)
            framework_issues = [
                issue for issue in framework_issues 
                if severity_order.get(issue.severity, 3) <= threshold
            ]
        
        # Log user code issues (always shown)
        if user_issues:
            logger.log(log_level, "\n📋 USER CODE ISSUES:")
            self._log_issue_group(user_issues, log_level)
        
        # Log framework issues (if not suppressed)
        if framework_issues:
            framework_label = "\n⚙️  FRAMEWORK INSIGHTS:"
            if self.config['SUPPRESS_FRAMEWORK_ISSUES']:
                framework_label += " (suppressed)"
            else:
                framework_label += f" ({len(framework_issues)} detected)"
            
            logger.log(log_level, framework_label)
            self._log_issue_group(framework_issues, log_level)
        
        # Show summary if both types exist
        if user_issues and framework_issues:
            total = len(user_issues) + len(framework_issues)
            logger.log(log_level, f"\n📊 Summary: {len(user_issues)} user code issues, {len(framework_issues)} framework insights (total: {total})")
        elif not user_issues and not framework_issues:
            logger.log(log_level, "\n✅ No performance issues detected in user code!")
    
    def _log_issue_group(self, issues, log_level):
        """Log a group of issues with consistent formatting."""
        if self.config['GROUP_DUPLICATE_ISSUES']:
            self._log_grouped_issues(issues, log_level)
        else:
            self._log_individual_issues(issues, log_level)
    
    def _log_grouped_issues(self, issues, log_level):
        """Log issues grouped by pattern type with better formatting."""
        from collections import defaultdict
        
        # Group issues by pattern type
        grouped = defaultdict(list)
        for issue in issues:
            grouped[issue.pattern_type].append(issue)
        
        max_display = self.config['MAX_ISSUES_DISPLAYED']
        displayed = 0
        
        # Sort by severity and count
        severity_order = {'critical': 0, 'high': 1, 'medium': 2, 'low': 3}
        sorted_groups = sorted(grouped.items(), key=lambda x: (
            min(severity_order.get(i.severity, 99) for i in x[1]),  # Highest severity first
            -len(x[1])  # Most occurrences first
        ))
        
        for pattern_type, pattern_issues in sorted_groups:
            if displayed >= max_display:
                remaining = len(issues) - displayed
                logger.log(log_level, f"\n  ... and {remaining} more issues (set MAX_ISSUES_DISPLAYED higher to see all)")
                break
            
            # Get the first issue as representative
            issue = pattern_issues[0]
            count = len(pattern_issues)
            
            # Build issue message with count
            if count > 1:
                issue_msg = f"\n🔸 {pattern_type} ({count}x) - {issue.severity}"
            else:
                issue_msg = f"\n🔸 {pattern_type} - {issue.severity}"
            
            # Add table context
            if hasattr(issue, 'table_context') and issue.table_context:
                tables = ', '.join(issue.table_context.keys())
                issue_msg += f" - Tables: {tables}"
            
            logger.log(log_level, issue_msg)
            
            # Add SQL example if enabled
            if self.config['SHOW_SQL_IN_LOGS'] and hasattr(issue, 'query_examples') and issue.query_examples:
                sql = issue.query_examples[0][:self.config['MAX_SQL_LENGTH']]
                if len(issue.query_examples[0]) > self.config['MAX_SQL_LENGTH']:
                    sql += "..."
                logger.log(log_level, f"   SQL: {sql}")
            
            # Add hierarchical locations
            if hasattr(issue, 'code_locations') and issue.code_locations:
                if count > 1:
                    # For grouped issues, show unique primary locations
                    all_primary_locations = set()
                    all_context_locations = set()
                    
                    for pi in pattern_issues:
                        if hasattr(pi, 'code_locations') and pi.code_locations:
                            # First location is primary, rest are context
                            if len(pi.code_locations) > 0:
                                all_primary_locations.add(pi.code_locations[0])
                            if len(pi.code_locations) > 1:
                                all_context_locations.update(pi.code_locations[1:])
                    
                    # Display primary locations
                    if all_primary_locations:
                        primary_list = sorted(all_primary_locations)[:2]  # Show top 2
                        remaining_primary = len(all_primary_locations) - len(primary_list)
                        
                        if remaining_primary > 0:
                            primary_str = ', '.join(primary_list) + f" (+{remaining_primary} more)"
                        else:
                            primary_str = ', '.join(primary_list)
                        
                        logger.log(log_level, f"   Primary: {primary_str}")
                        
                        # Show context if available
                        if all_context_locations:
                            context_list = sorted(all_context_locations)[:1]
                            logger.log(log_level, f"   Context: {context_list[0]}")
                    else:
                        logger.log(log_level, f"   Location: Multiple locations")
                else:
                    # Single issue - show hierarchical location
                    if len(issue.code_locations) > 1:
                        logger.log(log_level, f"   Primary: {issue.code_locations[0]}")
                        logger.log(log_level, f"   Context: {issue.code_locations[1]}")
                    else:
                        logger.log(log_level, f"   Location: {issue.code_locations[0]}")
            
            # Add recommendation
            if hasattr(issue, 'recommendation') and issue.recommendation:
                logger.log(log_level, f"   Fix: {issue.recommendation}")
            
            displayed += count
    
    def _log_individual_issues(self, issues, log_level):
        """Log issues individually (legacy format)."""
        max_display = self.config['MAX_ISSUES_DISPLAYED']
        
        for i, issue in enumerate(issues[:max_display]):
            issue_msg = f"\n🔸 {issue.pattern_type} ({issue.severity}): {issue.description}"
            
            # Add SQL if enabled
            if self.config['SHOW_SQL_IN_LOGS'] and hasattr(issue, 'query_examples') and issue.query_examples:
                sql = issue.query_examples[0][:self.config['MAX_SQL_LENGTH']]
                if len(issue.query_examples[0]) > self.config['MAX_SQL_LENGTH']:
                    sql += "..."
                issue_msg += f"\n   SQL: {sql}"
            
            # Add hierarchical location
            if hasattr(issue, 'code_locations') and issue.code_locations:
                if len(issue.code_locations) > 1:
                    issue_msg += f"\n   Primary: {issue.code_locations[0]}"
                    issue_msg += f"\n   Context: {issue.code_locations[1]}"
                else:
                    issue_msg += f"\n   Location: {issue.code_locations[0]}"
            
            # Add recommendation
            if hasattr(issue, 'recommendation') and issue.recommendation:
                rec_preview = issue.recommendation[:80] + "..." if len(issue.recommendation) > 80 else issue.recommendation
                issue_msg += f"\n   Fix: {rec_preview}"
            
            logger.log(log_level, issue_msg)
        
        if len(issues) > max_display:
            logger.log(log_level, f"\n  ... and {len(issues) - max_display} more issues (set MAX_ISSUES_DISPLAYED higher to see all)")
    
    def _add_performance_headers(self, response: HttpResponse, results, request_time: float):
        """Add performance information to response headers."""
        try:
            # Basic metrics
            response['X-DBCrust-Query-Count'] = str(results.total_queries)
            response['X-DBCrust-Query-Time'] = f"{results.total_duration * 1000:.1f}ms"
            response['X-DBCrust-Request-Time'] = f"{request_time * 1000:.1f}ms"
            
            # Issue counts
            if results.detected_patterns:
                response['X-DBCrust-Issues-Total'] = str(len(results.detected_patterns))
                
                # Count by severity
                critical_count = len([p for p in results.detected_patterns if p.severity == 'critical'])
                high_count = len([p for p in results.detected_patterns if p.severity == 'high'])
                
                if critical_count > 0:
                    response['X-DBCrust-Issues-Critical'] = str(critical_count)
                if high_count > 0:
                    response['X-DBCrust-Issues-High'] = str(high_count)
                
                # Pattern types
                pattern_types = set(p.pattern_type for p in results.detected_patterns)
                if pattern_types:
                    response['X-DBCrust-Pattern-Types'] = ','.join(sorted(pattern_types))
            
            # Duplicate queries
            if results.duplicate_queries > 0:
                response['X-DBCrust-Duplicate-Queries'] = str(results.duplicate_queries)
            
            # Performance assessment
            query_time_ms = results.total_duration * 1000
            if results.total_queries > self.config['QUERY_THRESHOLD']:
                response['X-DBCrust-Warning'] = 'High query count'
            elif query_time_ms > self.config['TIME_THRESHOLD']:
                response['X-DBCrust-Warning'] = 'Slow query time'
            elif len([p for p in results.detected_patterns if p.severity == 'critical']) > 0:
                response['X-DBCrust-Warning'] = 'Critical performance issues'
            else:
                response['X-DBCrust-Status'] = 'OK'
            
        except Exception as e:
            logger.debug(f"Could not add performance headers: {e}")
    
    def process_exception(self, request: HttpRequest, exception: Exception):
        """Clean up analysis context if an exception occurs."""
        if hasattr(request, '_dbcrust_analysis'):
            try:
                analysis = request._dbcrust_analysis
                analysis.__exit__(type(exception), exception, exception.__traceback__)
            except Exception as e:
                logger.debug(f"Error cleaning up performance analysis after exception: {e}")
        
        return None


# Convenience functions for programmatic access
def get_current_request_analysis() -> Optional[Dict[str, Any]]:
    """
    Get performance analysis for the current request (if available).
    
    This function can be used in views or other request-processing code
    to access the current performance analysis results.
    
    Returns:
        Dictionary with analysis results or None if not available
    """
    # This would require request context, which Django doesn't provide by default
    # Users would need to implement request storage if they want this functionality
    # For now, we'll document this as a future enhancement
    return None


def log_performance_summary(results, request_path: str = ""):
    """
    Manually log a performance analysis summary.
    
    Useful for custom analysis scenarios outside of the middleware.
    
    Args:
        results: AnalysisResult from analyzer
        request_path: Optional request path for context
    """
    query_count = results.total_queries
    query_time_ms = results.total_duration * 1000
    issue_count = len(results.detected_patterns)
    
    log_message = f"Performance Analysis{' for ' + request_path if request_path else ''}: "
    log_message += f"queries={query_count}, time={query_time_ms:.1f}ms, issues={issue_count}"
    
    if issue_count > 0:
        logger.warning(log_message)
        # Log top issues
        for issue in results.detected_patterns[:3]:
            logger.warning(f"  - {issue.pattern_type}: {issue.description}")
    else:
        logger.info(log_message)