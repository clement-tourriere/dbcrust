"""
Django Performance Analysis Middleware

Automatically analyzes Django ORM performance for each request during
development, producing a **single consolidated report** per request with
grade, metrics, critical issues, slow queries (with optional EXPLAIN
ANALYZE), warnings, and hints.

Usage:
    Add to your Django MIDDLEWARE setting:

    MIDDLEWARE = [
        # DBCrust Performance Analysis - early in the stack
        'dbcrust.django.PerformanceAnalysisMiddleware',

        # ... your other middleware
        'django.middleware.security.SecurityMiddleware',
        'django.contrib.sessions.middleware.SessionMiddleware',
        # ... etc
    ]

Configuration:
    # settings.py - Optional configuration
    DBCRUST_PERFORMANCE_ANALYSIS = {
        # ── Core ──────────────────────────────────────────────────
        'ENABLED': True,              # Override DEBUG mode
        'QUERY_THRESHOLD': 10,        # Log requests with >N queries
        'TIME_THRESHOLD': 100,        # Log requests taking >N ms
        'LOG_ALL_REQUESTS': False,    # Log ALL requests (ignores thresholds)

        # ── EXPLAIN ───────────────────────────────────────────────
        'EXPLAIN_ENABLED': True,      # Auto-EXPLAIN slow queries via DBCrust
        'EXPLAIN_SLOW_THRESHOLD_MS': 100,  # Only EXPLAIN queries above this
        'EXPLAIN_MAX_QUERIES': 5,     # Max EXPLAIN calls per request

        # ── Display ───────────────────────────────────────────────
        'INCLUDE_HEADERS': True,      # Add X-DBCrust-* response headers

        # ── Advanced ─────────────────────────────────────────────
        'TRANSACTION_SAFE': False,    # Wrap analysis in a transaction
        'DEBUG_TOOLBAR_COMPATIBILITY': True,
    }

    Note: Requests with detected performance patterns will ALWAYS be
    logged, regardless of QUERY_THRESHOLD / TIME_THRESHOLD settings.
"""

import logging
import time
from typing import Optional, Dict, Any
from urllib.parse import urlparse

from django.conf import settings
from django.utils.deprecation import MiddlewareMixin
from django.http import HttpRequest, HttpResponse

from .analyzer import create_enhanced_analyzer, DjangoAnalyzer
from .report_formatter import (
    build_report_from_analysis,
    format_performance_report,
)
from .slow_query_analyzer import SlowQueryAnalyzer, SlowQueryThresholds, get_django_db_url

# Dedicated logger for performance analysis
logger = logging.getLogger('dbcrust.performance')

# Config keys that were removed/renamed -- warn once if they appear.
_DEPRECATED_KEYS: Dict[str, str] = {
    'CATEGORIZE_ISSUES': 'Removed. The consolidated report replaces categorised logging.',
    'GROUP_DUPLICATE_ISSUES': 'Removed. Issues are now grouped automatically in the report.',
    'SHOW_SQL_IN_LOGS': 'Removed. SQL is always shown inline in the consolidated report.',
    'MAX_ISSUES_DISPLAYED': 'Removed. All issues are rendered in the consolidated report.',
    'SHOW_FULL_PATHS': 'Removed. Full paths are always used for IDE click-through.',
    'SUPPRESS_FRAMEWORK_ISSUES': 'Removed. Framework issues are shown as HINTS in the report.',
    'FRAMEWORK_ISSUE_THRESHOLD': 'Removed. Framework issues are shown as HINTS in the report.',
    'ADMIN_SPECIFIC_ADVICE': 'Removed. Admin-specific advice is always included.',
    'DEBUG_LOGGING': 'Removed. Use standard Python logging config for dbcrust.performance.',
    'ENABLE_CODE_ANALYSIS': 'Removed. Use the DjangoAnalyzer API directly for code analysis.',
    'PROJECT_ROOT': 'Removed. Use the DjangoAnalyzer API directly for code analysis.',
    'MAX_SQL_LENGTH': 'Removed. SQL truncation is handled automatically by the report formatter.',
}

# Track which deprecated/unknown keys have already been warned about.
_warned_config_keys: set = set()


class PerformanceAnalysisMiddleware(MiddlewareMixin):
    """
    Django middleware for automatic ORM performance analysis.

    Captures and analyzes Django ORM queries for each request, detecting
    N+1 patterns, missing optimizations, and slow queries.  Produces a
    **single consolidated report** in the ``dbcrust.performance`` logger.
    """

    def __init__(self, get_response):
        self.get_response = get_response
        self.analyzer: Optional[DjangoAnalyzer] = None
        self.config = self._load_config()
        self._slow_query_analyzer: Optional[SlowQueryAnalyzer] = None
        self._db_url: Optional[str] = None

        if self._is_enabled():
            self._initialize_analyzer()

        super().__init__(get_response)

    # ------------------------------------------------------------------
    # Configuration
    # ------------------------------------------------------------------

    def _load_config(self) -> Dict[str, Any]:
        """
        Load middleware configuration from Django settings.

        The config surface is intentionally small.  Deprecated keys from
        previous versions still work but emit a one-time warning.
        """
        default_config = {
            # Core
            'ENABLED': None,                    # None -> fall back to DEBUG
            'QUERY_THRESHOLD': 10,
            'TIME_THRESHOLD': 100,              # milliseconds
            'LOG_ALL_REQUESTS': False,

            # EXPLAIN
            'EXPLAIN_ENABLED': True,
            'EXPLAIN_SLOW_THRESHOLD_MS': 100,
            'EXPLAIN_MAX_QUERIES': 5,

            # Display
            'INCLUDE_HEADERS': True,

            # Advanced
            'TRANSACTION_SAFE': False,
            'DEBUG_TOOLBAR_COMPATIBILITY': True,
        }

        user_config = getattr(settings, 'DBCRUST_PERFORMANCE_ANALYSIS', {})

        # Warn about deprecated / unknown keys (once per key, ever)
        for key in user_config:
            if key in _warned_config_keys:
                continue
            if key in _DEPRECATED_KEYS:
                _warned_config_keys.add(key)
                logger.warning(
                    "DBCrust config key '%s' is deprecated: %s",
                    key,
                    _DEPRECATED_KEYS[key],
                )
            elif key not in default_config:
                _warned_config_keys.add(key)
                logger.warning(
                    "DBCrust: unknown config key '%s' — possible typo? "
                    "Valid keys: %s",
                    key,
                    ", ".join(sorted(default_config.keys())),
                )

        config = {**default_config, **user_config}
        return config

    def _is_enabled(self) -> bool:
        """Check if performance analysis should be enabled."""
        explicitly_set = self.config['ENABLED'] is not None
        if explicitly_set:
            enabled = self.config['ENABLED']
        else:
            enabled = getattr(settings, 'DEBUG', False)

        if (enabled
                and self.config['DEBUG_TOOLBAR_COMPATIBILITY']
                and self._has_debug_toolbar_conflict()):
            if explicitly_set:
                # User explicitly asked for ENABLED=True; warn but respect it.
                logger.info(
                    "DBCrust: Debug Toolbar profiling panel detected, but "
                    "ENABLED=True is set explicitly — keeping middleware active. "
                    "Set DEBUG_TOOLBAR_COMPATIBILITY=False to suppress this."
                )
            else:
                # Auto-mode: disable to avoid conflicts.
                logger.warning(
                    "DBCrust middleware disabled: Django Debug Toolbar profiling "
                    "panel is active.  Set DEBUG_TOOLBAR_COMPATIBILITY=False to "
                    "override."
                )
                return False

        return enabled

    def _has_debug_toolbar_conflict(self) -> bool:
        """Check if Django Debug Toolbar profiling would conflict."""
        try:
            installed_apps = getattr(settings, 'INSTALLED_APPS', [])
            if 'debug_toolbar' not in installed_apps:
                return False

            middleware = getattr(settings, 'MIDDLEWARE', [])
            if 'debug_toolbar.middleware.DebugToolbarMiddleware' not in middleware:
                return False

            panels = getattr(settings, 'DEBUG_TOOLBAR_PANELS', [])
            if not panels:
                return True  # default panels include profiling
            return 'debug_toolbar.panels.profiling.ProfilingPanel' in panels

        except Exception:
            return False

    # ------------------------------------------------------------------
    # Initialization
    # ------------------------------------------------------------------

    def _initialize_analyzer(self):
        """Initialize the enhanced analyzer and slow-query subsystem."""
        try:
            self.analyzer = create_enhanced_analyzer(
                transaction_safe=self.config['TRANSACTION_SAFE'],
                enable_all_features=False,
            )

            # Slow-query analyzer with configurable thresholds
            self._slow_query_analyzer = SlowQueryAnalyzer(
                SlowQueryThresholds(
                    absolute_ms=self.config['EXPLAIN_SLOW_THRESHOLD_MS'],
                )
            )

            # Auto-detect the DB URL from Django settings for EXPLAIN
            if self.config['EXPLAIN_ENABLED']:
                detected_db_url = get_django_db_url()
                if detected_db_url and self._supports_explain_db_url(detected_db_url):
                    self._db_url = detected_db_url
                    logger.info("DBCrust EXPLAIN enabled (auto-detected DB URL)")
                elif detected_db_url:
                    logger.debug(
                        "DBCrust EXPLAIN disabled: backend %r is not supported; "
                        "heuristic-only analysis will be used",
                        urlparse(detected_db_url).scheme,
                    )
                else:
                    logger.debug(
                        "DBCrust EXPLAIN: could not auto-detect DB URL "
                        "(heuristic-only analysis will be used)"
                    )

            logger.info(
                "DBCrust Performance Analysis Middleware initialised "
                "(threshold: >%d queries or >%dms, EXPLAIN: %s)",
                self.config['QUERY_THRESHOLD'],
                self.config['TIME_THRESHOLD'],
                "on" if self._db_url else "heuristic",
            )

        except Exception as e:
            logger.warning("Could not initialise performance analyzer: %s", e)
            self.analyzer = None

    # ------------------------------------------------------------------
    # Request lifecycle
    # ------------------------------------------------------------------

    def process_request(self, request: HttpRequest):
        """Start performance analysis for this request."""
        if not self.analyzer:
            return None

        try:
            analysis_context = self.analyzer.analyze()
            request._dbcrust_analysis = analysis_context.__enter__()
            request._dbcrust_start_time = time.time()
        except Exception as e:
            logger.debug("Could not start performance analysis: %s", e)
            return None

    def process_response(self, request: HttpRequest, response: HttpResponse) -> HttpResponse:
        """Complete analysis, build report, log it."""
        if not hasattr(request, '_dbcrust_analysis'):
            return response

        try:
            # 1. Complete the analysis context
            analysis = request._dbcrust_analysis
            analysis.__exit__(None, None, None)

            results = analysis.get_results()
            if not results:
                return response

            request_time = time.time() - getattr(request, '_dbcrust_start_time', time.time())
            request_time_ms = request_time * 1000

            # 2. Build the consolidated report
            self._emit_consolidated_report(request, response, results, request_time_ms)

            # 3. Performance headers
            if self.config['INCLUDE_HEADERS']:
                self._add_performance_headers(response, results, request_time_ms)

        except Exception as e:
            logger.debug("Error processing performance analysis: %s", e)

        return response

    def process_exception(self, request: HttpRequest, exception: Exception):
        """Clean up analysis context on exception."""
        if hasattr(request, '_dbcrust_analysis'):
            try:
                request._dbcrust_analysis.__exit__(
                    type(exception), exception, exception.__traceback__
                )
            except Exception as e:
                logger.debug("Error cleaning up analysis after exception: %s", e)
        return None

    # ------------------------------------------------------------------
    # Consolidated report
    # ------------------------------------------------------------------

    def _emit_consolidated_report(
        self,
        request: HttpRequest,
        response: HttpResponse,
        results,
        request_time_ms: float,
    ):
        """
        Build and log a single consolidated performance report.

        This replaces the old scattered ``_log_categorized_issues``,
        ``_log_grouped_issues``, and ``_log_individual_issues`` methods.
        """
        query_count = results.total_queries
        query_time_ms = results.total_duration * 1000
        has_issues = bool(results.detected_patterns)
        has_query_concerns = query_count > self.config['QUERY_THRESHOLD']
        has_time_concerns = query_time_ms > self.config['TIME_THRESHOLD']

        should_log = (
            self.config['LOG_ALL_REQUESTS']
            or has_issues
            or has_query_concerns
            or has_time_concerns
        )
        if not should_log:
            return

        # -- slow query analysis -------------------------------------------
        slow_query_infos = []
        if self._slow_query_analyzer:
            captured_queries = getattr(
                self.analyzer.query_collector, 'queries', []
            )
            slow_raw = self._slow_query_analyzer.identify_slow_queries(
                captured_queries,
                total_db_time=results.total_duration,
            )
            if slow_raw:
                db_url = self._db_url if self.config['EXPLAIN_ENABLED'] else None
                slow_query_infos = self._slow_query_analyzer.analyze(
                    slow_raw,
                    db_url=db_url,
                    max_explain=self.config['EXPLAIN_MAX_QUERIES'],
                )

        # -- build report --------------------------------------------------
        path = getattr(request, 'path', '?')
        method = getattr(request, 'method', '?')
        view_name = self._resolve_view_name(request)

        report = build_report_from_analysis(
            results,
            method=method,
            path=path,
            view_name=view_name,
            status_code=getattr(response, 'status_code', None),
            request_time_ms=request_time_ms,
            slow_queries=slow_query_infos,
        )

        # -- render & log --------------------------------------------------
        report_text = format_performance_report(report)

        # Choose log level based on grade
        if report.grade in ("F", "D"):
            log_level = logging.ERROR
        elif report.grade == "C":
            log_level = logging.WARNING
        else:
            log_level = logging.INFO

        logger.log(log_level, report_text)

    # ------------------------------------------------------------------
    # Response headers
    # ------------------------------------------------------------------

    def _add_performance_headers(self, response: HttpResponse, results, request_time_ms: float):
        """Add performance information to response headers.

        Args:
            response: The Django HTTP response to add headers to.
            results: The analysis results.
            request_time_ms: Total request time in **milliseconds**.
        """
        try:
            query_count = results.total_queries
            query_time_ms = results.total_duration * 1000

            response['X-DBCrust-Query-Count'] = str(results.total_queries)
            response['X-DBCrust-Query-Time'] = f"{query_time_ms:.1f}ms"
            response['X-DBCrust-Request-Time'] = f"{request_time_ms:.1f}ms"

            if results.detected_patterns:
                response['X-DBCrust-Issues-Total'] = str(len(results.detected_patterns))

                critical_count = len([p for p in results.detected_patterns if p.severity == 'critical'])
                high_count = len([p for p in results.detected_patterns if p.severity == 'high'])
                if critical_count > 0:
                    response['X-DBCrust-Issues-Critical'] = str(critical_count)
                if high_count > 0:
                    response['X-DBCrust-Issues-High'] = str(high_count)

                pattern_types = set(p.pattern_type for p in results.detected_patterns)
                if pattern_types:
                    response['X-DBCrust-Pattern-Types'] = ','.join(sorted(pattern_types))

                # Summary warning header for quick devtools inspection
                if critical_count > 0:
                    response['X-DBCrust-Warning'] = 'Critical performance issues'
                elif high_count > 0:
                    response['X-DBCrust-Warning'] = 'Performance issues detected'

            if 'X-DBCrust-Warning' not in response:
                has_query_concerns = query_count > self.config['QUERY_THRESHOLD']
                has_time_concerns = query_time_ms > self.config['TIME_THRESHOLD']

                if has_query_concerns and has_time_concerns:
                    response['X-DBCrust-Warning'] = 'High query count and slow query time'
                elif has_query_concerns:
                    response['X-DBCrust-Warning'] = 'High query count'
                elif has_time_concerns:
                    response['X-DBCrust-Warning'] = 'Slow query time'

            if results.duplicate_queries > 0:
                response['X-DBCrust-Duplicate-Queries'] = str(results.duplicate_queries)

        except Exception as e:
            logger.debug("Could not add performance headers: %s", e)

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    @staticmethod
    def _resolve_view_name(request: HttpRequest) -> Optional[str]:
        """Attempt to resolve the view name from the request."""
        try:
            resolver_match = getattr(request, 'resolver_match', None)
            if resolver_match:
                return resolver_match.view_name
        except Exception:
            pass
        return None

    @staticmethod
    def _supports_explain_db_url(db_url: str) -> bool:
        """Return whether this auto-detected URL can be EXPLAINed by DBCrust."""
        try:
            return urlparse(db_url).scheme in {'postgres', 'postgresql'}
        except Exception:
            return False


# ---------------------------------------------------------------------------
# Public convenience helpers
# ---------------------------------------------------------------------------

def log_performance_summary(results, request_path: str = ""):
    """
    Manually log a performance analysis summary using the consolidated
    report format.

    Useful for custom analysis scenarios outside of the middleware.
    """
    report = build_report_from_analysis(
        results,
        path=request_path or "?",
    )
    report_text = format_performance_report(report)

    if report.grade in ("F", "D"):
        logger.error(report_text)
    elif report.grade == "C":
        logger.warning(report_text)
    else:
        logger.info(report_text)
