"""
DBCrust Django Integration

Comprehensive Django integration including:
- Database connection helper with automatic Django DATABASES integration
- ORM query analyzer for performance issues and N+1 problems
- Performance analysis middleware with consolidated reporting
- Slow-query analysis with optional EXPLAIN ANALYZE support
"""

from .analyzer import DjangoAnalyzer, analyze
from .middleware import PerformanceAnalysisMiddleware
from .report_formatter import (
    RequestPerformanceReport,
    IssueInfo,
    SlowQueryInfo,
    build_report_from_analysis,
    format_performance_report,
)
from .slow_query_analyzer import SlowQueryAnalyzer
from .database_helper import (
    connect,
    connect_all_databases,
    transaction,
    get_database_info,
    list_django_databases,
    clear_connection_cache,
    DjangoConnectionError,
    # Convenience aliases
    django_connect,
    db_connect
)

__all__ = [
    # ORM Analysis
    "DjangoAnalyzer",
    "analyze",
    "PerformanceAnalysisMiddleware",

    # Report & Slow-query analysis
    "RequestPerformanceReport",
    "IssueInfo",
    "SlowQueryInfo",
    "build_report_from_analysis",
    "format_performance_report",
    "SlowQueryAnalyzer",

    # Database Connection Helper
    "connect",
    "connect_all_databases",
    "transaction",
    "get_database_info",
    "list_django_databases",
    "clear_connection_cache",
    "DjangoConnectionError",

    # Convenience aliases
    "django_connect",
    "db_connect"
]
