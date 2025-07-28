"""
Django ORM Query Analyzer

Main analyzer class that provides a context manager interface for
analyzing Django ORM queries and providing optimization recommendations.
"""

import asyncio
import json
from contextlib import contextmanager
from dataclasses import dataclass, field
from datetime import datetime
from typing import List, Dict, Optional, Any, Union

try:
    from django.db import connection, connections, transaction
    from django.conf import settings
    DJANGO_AVAILABLE = True
except ImportError:
    DJANGO_AVAILABLE = False
    connection = None
    connections = None
    transaction = None

from .query_collector import QueryCollector, CapturedQuery
from .pattern_detector import PatternDetector, DetectedPattern
from .recommendations import DjangoRecommendations, Recommendation


@dataclass
class AnalysisResult:
    """Results from Django query analysis."""
    start_time: datetime
    end_time: datetime
    total_queries: int
    total_duration: float
    queries_by_type: Dict[str, int]
    duplicate_queries: int
    detected_patterns: List[DetectedPattern]
    recommendations: List[Recommendation]
    dbcrust_analysis: Optional[Dict[str, Any]] = None
    
    @property
    def summary(self) -> str:
        """Generate a human-readable summary of the analysis."""
        duration_ms = self.total_duration * 1000
        summary_lines = [
            f"Django Query Analysis Summary",
            f"============================",
            f"Time Range: {self.start_time.strftime('%H:%M:%S')} - {self.end_time.strftime('%H:%M:%S')}",
            f"Total Queries: {self.total_queries}",
            f"Total Duration: {duration_ms:.2f}ms",
            f"Average Query Time: {(duration_ms / self.total_queries if self.total_queries > 0 else 0):.2f}ms",
            f"",
            f"Query Types:",
        ]
        
        for query_type, count in sorted(self.queries_by_type.items()):
            summary_lines.append(f"  - {query_type}: {count}")
        
        if self.duplicate_queries > 0:
            summary_lines.extend([
                f"",
                f"⚠️  Duplicate Queries: {self.duplicate_queries}",
            ])
        
        if self.detected_patterns:
            summary_lines.extend([
                f"",
                f"Performance Issues Detected:",
            ])
            
            # Group patterns by type
            pattern_counts = {}
            for pattern in self.detected_patterns:
                if pattern.pattern_type not in pattern_counts:
                    pattern_counts[pattern.pattern_type] = 0
                pattern_counts[pattern.pattern_type] += 1
            
            for pattern_type, count in pattern_counts.items():
                severity = max(p.severity for p in self.detected_patterns if p.pattern_type == pattern_type)
                icon = "🔴" if severity == "critical" else "🟡" if severity == "high" else "🟢"
                summary_lines.append(f"  {icon} {pattern_type.replace('_', ' ').title()}: {count}")
        
        if self.recommendations:
            summary_lines.extend([
                f"",
                DjangoRecommendations.format_recommendations_summary(self.recommendations)
            ])
        
        # Add detailed pattern analysis with specific context
        if self.detected_patterns:
            summary_lines.extend([
                f"",
                f"🔍 Detailed Analysis with Specific Recommendations:",
                f"=" * 60
            ])
            
            for i, pattern in enumerate(self.detected_patterns, 1):
                summary_lines.extend([
                    f"",
                    f"{i}. {pattern.pattern_type.replace('_', ' ').title()} - {pattern.severity.upper()}"
                ])
                
                # Show specific fields if available
                if pattern.specific_fields:
                    fields_str = ', '.join(f"'{f}'" for f in pattern.specific_fields)
                    summary_lines.append(f"   💡 Suggested fields: {fields_str}")
                
                # Show code locations
                if pattern.code_locations:
                    summary_lines.append(f"   📍 Code locations:")
                    for location in pattern.code_locations[:3]:  # Show up to 3 locations
                        summary_lines.append(f"      - {location}")
                
                # Show table context
                if pattern.table_context:
                    tables = ', '.join(pattern.table_context.keys())
                    summary_lines.append(f"   🗃️  Tables involved: {tables}")
                
                # Show specific recommendation
                if pattern.code_suggestion:
                    summary_lines.append(f"   ⚡ Quick fix: {pattern.code_suggestion}")
                
                # Show impact
                if pattern.estimated_impact:
                    summary_lines.append(f"   📈 Impact: {pattern.estimated_impact}")
                
                # Show example queries (first one only, truncated)
                if pattern.query_examples:
                    example = pattern.query_examples[0]
                    if len(example) > 100:
                        example = example[:100] + "..."
                    summary_lines.append(f"   🔍 Example query: {example}")
        
        return "\n".join(summary_lines)
    
    def to_dict(self) -> Dict[str, Any]:
        """Convert results to dictionary for JSON serialization."""
        return {
            "start_time": self.start_time.isoformat(),
            "end_time": self.end_time.isoformat(),
            "total_queries": self.total_queries,
            "total_duration": self.total_duration,
            "queries_by_type": self.queries_by_type,
            "duplicate_queries": self.duplicate_queries,
            "detected_patterns": [
                {
                    "pattern_type": p.pattern_type,
                    "severity": p.severity,
                    "description": p.description,
                    "query_count": len(p.affected_queries),
                    "recommendation": p.recommendation,
                    "code_suggestion": p.code_suggestion,
                    "estimated_impact": p.estimated_impact,
                }
                for p in self.detected_patterns
            ],
            "recommendations": [
                {
                    "title": r.title,
                    "description": r.description,
                    "impact": r.impact,
                    "difficulty": r.difficulty,
                }
                for r in self.recommendations
            ],
            "dbcrust_analysis": self.dbcrust_analysis,
        }


class DjangoAnalyzer:
    """
    Django ORM Query Analyzer with DBCrust integration.
    
    Captures and analyzes Django database queries to detect performance
    issues like N+1 queries, missing select_related/prefetch_related,
    and provides optimization recommendations.
    """
    
    def __init__(self, 
                 dbcrust_url: Optional[str] = None,
                 transaction_safe: bool = True,
                 enable_explain: bool = True,
                 database_alias: str = 'default'):
        """
        Initialize the Django analyzer.
        
        Args:
            dbcrust_url: Optional DBCrust database URL for EXPLAIN analysis
            transaction_safe: Whether to wrap analysis in a transaction (for safety)
            enable_explain: Whether to run EXPLAIN ANALYZE on queries
            database_alias: Django database alias to analyze
        """
        if not DJANGO_AVAILABLE:
            raise ImportError("Django is not installed. Please install Django to use this analyzer.")
        
        self.dbcrust_url = dbcrust_url
        self.transaction_safe = transaction_safe
        self.enable_explain = enable_explain
        self.database_alias = database_alias
        self.query_collector = QueryCollector()
        self.result: Optional[AnalysisResult] = None
        self._connection = None
        self._transaction_ctx = None
    
    def analyze(self):
        """
        Context manager for analyzing Django queries.
        
        Usage:
            with analyzer.analyze() as analysis:
                # Your Django ORM code here
                Book.objects.filter(author__name='Smith').count()
            
            results = analysis.get_results()
            print(results.summary)
        """
        return self
    
    def __enter__(self):
        """Enter the analysis context."""
        # Get the Django database connection
        self._connection = connections[self.database_alias]
        
        # Start query collection
        self.query_collector.start_collection()
        
        # Install the query wrapper
        self._connection_ctx = self._connection.execute_wrapper(self.query_collector)
        self._connection_ctx.__enter__()
        
        # Start transaction if requested
        if self.transaction_safe:
            self._transaction_ctx = transaction.atomic(using=self.database_alias)
            self._transaction_ctx.__enter__()
        
        return self
    
    def __exit__(self, exc_type, exc_val, exc_tb):
        """Exit the analysis context and perform analysis."""
        try:
            # Stop query collection
            self.query_collector.stop_collection()
            
            # Remove the query wrapper
            if self._connection_ctx:
                self._connection_ctx.__exit__(exc_type, exc_val, exc_tb)
            
            # Rollback transaction if in transaction mode
            if self._transaction_ctx:
                # Force rollback by raising an exception inside the atomic block
                try:
                    self._transaction_ctx.__exit__(Exception, Exception("Analysis rollback"), None)
                except:
                    pass  # Expected - we're forcing a rollback
            
            # Perform analysis if no exception occurred
            if exc_type is None:
                self._perform_analysis()
                
        except Exception as e:
            # Log the error but don't re-raise to avoid masking the original exception
            print(f"Error during analysis cleanup: {e}")
        
        return False  # Don't suppress exceptions
    
    def _perform_analysis(self):
        """Perform the actual query analysis."""
        # Get captured queries
        queries = self.query_collector.queries
        
        if not queries:
            self.result = AnalysisResult(
                start_time=datetime.now(),
                end_time=datetime.now(),
                total_queries=0,
                total_duration=0.0,
                queries_by_type={},
                duplicate_queries=0,
                detected_patterns=[],
                recommendations=[],
            )
            return
        
        # Basic metrics
        start_time = queries[0].timestamp
        end_time = queries[-1].timestamp
        total_duration = self.query_collector.get_total_duration()
        queries_by_type = {
            qt: len(qs) for qt, qs in self.query_collector.get_queries_by_type().items()
        }
        duplicate_queries = sum(
            len(dups) - 1 for dups in self.query_collector.get_duplicate_queries().values()
        )
        
        # Pattern detection
        pattern_detector = PatternDetector(queries)
        detected_patterns = pattern_detector.analyze()
        
        # Generate recommendations
        recommendations = DjangoRecommendations.generate_recommendations(detected_patterns)
        
        # DBCrust integration for EXPLAIN analysis
        dbcrust_analysis = None
        if self.enable_explain and self.dbcrust_url:
            dbcrust_analysis = self._run_dbcrust_analysis(queries)
        
        # Create result
        self.result = AnalysisResult(
            start_time=start_time,
            end_time=end_time,
            total_queries=len(queries),
            total_duration=total_duration,
            queries_by_type=queries_by_type,
            duplicate_queries=duplicate_queries,
            detected_patterns=detected_patterns,
            recommendations=recommendations,
            dbcrust_analysis=dbcrust_analysis,
        )
    
    def _run_dbcrust_analysis(self, queries: List[CapturedQuery]) -> Optional[Dict[str, Any]]:
        """Run DBCrust EXPLAIN analysis on captured queries."""
        try:
            from .dbcrust_integration import enhance_analysis_with_dbcrust
            
            # Run DBCrust analysis
            results, report = enhance_analysis_with_dbcrust(
                queries=queries,
                connection_url=self.dbcrust_url,
                max_queries=10  # Analyze top 10 slowest queries
            )
            
            return {
                "analyzed_queries": len(results),
                "performance_report": report,
                "detailed_results": results
            }
            
        except Exception as e:
            print(f"DBCrust analysis failed: {e}")
            return None
    
    def get_results(self) -> Optional[AnalysisResult]:
        """Get the analysis results."""
        return self.result
    
    def print_queries(self, verbose: bool = False):
        """Print all captured queries for debugging."""
        if not self.query_collector.queries:
            print("No queries captured.")
            return
        
        print(f"\nCaptured {len(self.query_collector.queries)} queries:")
        print("-" * 80)
        
        for i, query in enumerate(self.query_collector.queries, 1):
            print(f"\nQuery {i}:")
            print(f"Type: {query.query_type}")
            print(f"Duration: {query.duration * 1000:.2f}ms")
            print(f"Tables: {', '.join(query.table_names)}")
            print(f"SQL: {query.sql[:200]}{'...' if len(query.sql) > 200 else ''}")
            
            if verbose and query.params:
                print(f"Params: {query.params}")
            
            if verbose and query.stack_trace:
                print("Stack trace:")
                for frame in query.stack_trace[-3:]:  # Show last 3 frames
                    print(f"  {frame}")
    
    def export_results(self, filename: str):
        """Export analysis results to JSON file."""
        if not self.result:
            raise ValueError("No analysis results available. Run analyze() first.")
        
        with open(filename, 'w') as f:
            json.dump(self.result.to_dict(), f, indent=2)
        
        print(f"Results exported to {filename}")


# Convenience function for quick analysis
@contextmanager
def analyze(dbcrust_url: Optional[str] = None, **kwargs):
    """
    Convenience function for analyzing Django queries.
    
    Usage:
        from dbcrust.django import analyze
        
        with analyze() as analysis:
            # Your Django code here
            MyModel.objects.all()
        
        print(analysis.get_results().summary)
    """
    analyzer = DjangoAnalyzer(dbcrust_url=dbcrust_url, **kwargs)
    with analyzer.analyze() as analysis:
        yield analysis