"""
DBCrust Django ORM Query Analyzer

Analyze Django ORM queries for performance issues, N+1 problems,
and missing optimizations like select_related and prefetch_related.
"""

from .analyzer import DjangoAnalyzer, analyze

__all__ = ["DjangoAnalyzer", "analyze"]