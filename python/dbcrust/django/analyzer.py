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
                 database_alias: str = 'default',
                 enable_code_analysis: bool = False,
                 project_root: Optional[str] = None,
                 database_instance: Optional[Any] = None):
        """
        Initialize the Django analyzer.
        
        Args:
            dbcrust_url: Optional DBCrust database URL for EXPLAIN analysis
            transaction_safe: Whether to wrap analysis in a transaction (for safety)
            enable_explain: Whether to run EXPLAIN ANALYZE on queries
            database_alias: Django database alias to analyze
            enable_code_analysis: Whether to perform AST-based code analysis
            project_root: Django project root for code analysis
            database_instance: Pre-existing PyDatabase instance to use
        """
        if not DJANGO_AVAILABLE:
            raise ImportError("Django is not installed. Please install Django to use this analyzer.")
        
        self.dbcrust_url = dbcrust_url
        self.transaction_safe = transaction_safe
        self.enable_explain = enable_explain
        self.database_alias = database_alias
        self.enable_code_analysis = enable_code_analysis
        self.project_root = project_root
        self.database_instance = database_instance
        
        self.query_collector = QueryCollector()
        self.result: Optional[AnalysisResult] = None
        self._connection = None
        self._connection_ctx = None
        self._transaction_ctx = None
        
        # Initialize code analyzer if requested
        self.code_analyzer = None
        if enable_code_analysis and project_root:
            try:
                from .code_analyzer import DjangoCodeAnalyzer
                self.code_analyzer = DjangoCodeAnalyzer(project_root)
            except ImportError as e:
                print(f"Warning: Could not initialize code analyzer: {e}")
    
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
            
            # Run DBCrust analysis with enhanced integration
            results, report = enhance_analysis_with_dbcrust(
                queries=queries,
                connection_url=self.dbcrust_url,
                database_instance=self.database_instance,
                max_queries=10  # Analyze top 10 slowest queries
            )
            
            # Extract optimization suggestions from DBCrust results
            all_suggestions = []
            for result in results:
                if "optimization_suggestions" in result:
                    all_suggestions.extend(result["optimization_suggestions"])
            
            return {
                "analyzed_queries": len(results),
                "performance_report": report,
                "detailed_results": results,
                "optimization_suggestions": all_suggestions,
                "explain_enabled": True
            }
            
        except Exception as e:
            print(f"DBCrust analysis failed: {e}")
            return None
    
    def get_results(self) -> Optional[AnalysisResult]:
        """Get the analysis results."""
        return self.result
    
    def analyze_project_code(self) -> Optional[List[Any]]:
        """
        Analyze project code for Django ORM patterns.
        
        Returns:
            List of code issues found through AST analysis
        """
        if not self.code_analyzer:
            print("Code analysis not enabled. Initialize with enable_code_analysis=True and project_root.")
            return None
        
        print("🔍 Analyzing project code for Django ORM patterns...")
        
        try:
            # Analyze the Django project
            code_issues = self.code_analyzer.analyze_directory(self.project_root)
            
            print(f"📋 Found {len(code_issues)} potential code issues")
            return code_issues
            
        except Exception as e:
            print(f"❌ Code analysis failed: {e}")
            return None
    
    def analyze_project_models(self) -> Optional[List[Any]]:
        """
        Analyze Django models for optimization opportunities.
        
        Returns:
            List of model analysis results
        """
        if not self.project_root:
            print("Project root not specified. Cannot analyze models.")
            return None
        
        print("🏗️ Analyzing Django models...")
        
        try:
            from .project_analyzer import DjangoProjectAnalyzer
            
            project_analyzer = DjangoProjectAnalyzer(self.project_root)
            models = project_analyzer.analyze_models_only()
            
            print(f"📊 Analyzed {len(models)} Django models")
            return models
            
        except Exception as e:
            print(f"❌ Model analysis failed: {e}")
            return None
    
    def get_comprehensive_analysis(self) -> Dict[str, Any]:
        """
        Get comprehensive analysis including queries, code, and models.
        
        Returns:
            Dictionary containing all analysis results
        """
        comprehensive = {
            "query_analysis": self.result.to_dict() if self.result else None,
            "code_issues": None,
            "model_analysis": None,
            "combined_recommendations": []
        }
        
        # Add code analysis if available
        if self.enable_code_analysis:
            comprehensive["code_issues"] = self.analyze_project_code()
        
        # Add model analysis if available
        if self.project_root:
            comprehensive["model_analysis"] = self.analyze_project_models()
        
        # Combine recommendations from all sources
        all_recommendations = []
        
        # Query-based recommendations
        if self.result and self.result.recommendations:
            all_recommendations.extend([{
                "source": "query_analysis",
                "type": rec.title,
                "description": rec.description,
                "difficulty": rec.difficulty,
                "impact": rec.impact
            } for rec in self.result.recommendations])
        
        # EXPLAIN-based recommendations
        if (self.result and self.result.dbcrust_analysis and 
            "optimization_suggestions" in self.result.dbcrust_analysis):
            for suggestion in self.result.dbcrust_analysis["optimization_suggestions"]:
                all_recommendations.append({
                    "source": "explain_analysis",
                    "type": suggestion["title"],
                    "description": suggestion["description"],
                    "difficulty": "medium",  # Default for EXPLAIN suggestions
                    "impact": suggestion["priority"]
                })
        
        comprehensive["combined_recommendations"] = all_recommendations
        return comprehensive
    
    def generate_comprehensive_report(self) -> str:
        """Generate a comprehensive analysis report."""
        comprehensive = self.get_comprehensive_analysis()
        
        report_lines = [
            "🔍 Comprehensive Django ORM Analysis Report",
            "=" * 50,
            ""
        ]
        
        # Query analysis summary
        if comprehensive["query_analysis"]:
            query_data = comprehensive["query_analysis"]
            report_lines.extend([
                "📈 Query Analysis:",
                f"  - Total Queries: {query_data['total_queries']}",
                f"  - Total Duration: {query_data['total_duration']*1000:.1f}ms",
                f"  - Duplicate Queries: {query_data['duplicate_queries']}",
                f"  - Patterns Detected: {len(query_data['detected_patterns'])}",
                ""
            ])
        
        # Code analysis summary
        if comprehensive["code_issues"]:
            code_issues = comprehensive["code_issues"]
            severity_counts = {}
            for issue in code_issues:
                severity = issue.severity
                severity_counts[severity] = severity_counts.get(severity, 0) + 1
            
            report_lines.extend([
                "💻 Code Analysis:",
                f"  - Total Issues Found: {len(code_issues)}",
            ])
            
            for severity, count in severity_counts.items():
                report_lines.append(f"  - {severity.title()}: {count}")
            
            report_lines.append("")
        
        # Model analysis summary
        if comprehensive["model_analysis"]:
            models = comprehensive["model_analysis"]
            models_with_indexes = len([m for m in models if m.indexes])
            models_with_relationships = len([m for m in models if m.foreign_keys or m.many_to_many])
            
            report_lines.extend([
                "🏗️ Model Analysis:",
                f"  - Total Models: {len(models)}",
                f"  - Models with Relationships: {models_with_relationships}",
                f"  - Models with Custom Indexes: {models_with_indexes}",
                ""
            ])
        
        # Combined recommendations
        recommendations = comprehensive["combined_recommendations"]
        if recommendations:
            # Group by impact/priority
            critical = [r for r in recommendations if r.get("impact") == "critical" or r.get("impact") == "high"]
            high = [r for r in recommendations if r.get("impact") == "high" and r not in critical]
            medium = [r for r in recommendations if r.get("impact") == "medium"]
            
            report_lines.extend([
                "🎯 Priority Recommendations:",
                ""
            ])
            
            if critical:
                report_lines.append(f"🚨 Critical/High Priority ({len(critical)} issues):")
                for i, rec in enumerate(critical[:5], 1):  # Show top 5
                    report_lines.append(f"  {i}. {rec['type']} ({rec['source']})")
                report_lines.append("")
            
            if medium:
                report_lines.append(f"⚠️ Medium Priority ({len(medium)} issues):")
                for i, rec in enumerate(medium[:3], 1):  # Show top 3
                    report_lines.append(f"  {i}. {rec['type']} ({rec['source']})")
                report_lines.append("")
        
        # Overall assessment
        total_issues = len(recommendations)
        critical_count = len([r for r in recommendations if r.get("impact") in ["critical", "high"]])
        
        if total_issues == 0:
            assessment = "✅ Excellent: No major optimization opportunities found"
        elif critical_count == 0:
            assessment = "👍 Good: Only minor optimizations available"
        elif critical_count < 3:
            assessment = "⚠️ Needs Attention: Some critical issues to address"
        else:
            assessment = "🚨 Action Required: Multiple critical performance issues detected"
        
        report_lines.extend([
            "📊 Overall Assessment:",
            f"  {assessment}",
            f"  Total Recommendations: {total_issues}",
            f"  Critical/High Priority: {critical_count}",
            ""
        ])
        
        return "\n".join(report_lines)
    
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
def analyze(dbcrust_url: Optional[str] = None, 
           enable_code_analysis: bool = False,
           project_root: Optional[str] = None,
           database_instance: Optional[Any] = None,
           **kwargs):
    """
    Enhanced convenience function for analyzing Django queries with optional code analysis.
    
    Args:
        dbcrust_url: Database URL for EXPLAIN analysis
        enable_code_analysis: Whether to analyze code patterns
        project_root: Django project root for code/model analysis
        database_instance: Pre-existing PyDatabase instance
        **kwargs: Additional arguments for DjangoAnalyzer
    
    Usage:
        from dbcrust.django import analyze
        
        # Basic query analysis
        with analyze() as analysis:
            MyModel.objects.all()
        print(analysis.get_results().summary)
        
        # Comprehensive analysis with code patterns
        with analyze(
            dbcrust_url="postgres://user@localhost/db",
            enable_code_analysis=True,
            project_root="/path/to/project"
        ) as analysis:
            MyModel.objects.all()
        
        # Get comprehensive report
        comprehensive_report = analysis.generate_comprehensive_report()
        print(comprehensive_report)
    """
    analyzer = DjangoAnalyzer(
        dbcrust_url=dbcrust_url,
        enable_code_analysis=enable_code_analysis,
        project_root=project_root,
        database_instance=database_instance,
        **kwargs
    )
    
    with analyzer.analyze() as analysis:
        yield analysis


# Convenience function for project-wide analysis
def analyze_django_project(project_root: str, 
                          dbcrust_url: Optional[str] = None,
                          database_instance: Optional[Any] = None) -> Dict[str, Any]:
    """
    Analyze an entire Django project without runtime query capture.
    
    Args:
        project_root: Path to Django project root
        dbcrust_url: Optional database URL for model analysis
        database_instance: Pre-existing PyDatabase instance
    
    Returns:
        Comprehensive analysis results
    
    Usage:
        from dbcrust.django.analyzer import analyze_django_project
        
        results = analyze_django_project(
            project_root="/path/to/django/project",
            dbcrust_url="postgres://user@localhost/db"
        )
        
        print(f"Found {len(results['code_issues'])} code issues")
        print(f"Analyzed {len(results['model_analysis'])} models")
    """
    from .project_analyzer import analyze_django_project as project_analyze
    
    # Use the project analyzer for comprehensive analysis
    project_results = project_analyze(project_root)
    
    # Convert to dictionary format compatible with query analyzer
    return {
        "models": [model.__dict__ for model in project_results.models],
        "code_issues": [issue.__dict__ for issue in project_results.code_issues],
        "model_relationships": project_results.model_relationships,
        "optimization_score": project_results.optimization_score,
        "summary": project_results.summary,
        "recommendations": project_results.recommendations
    }


# Function to create an enhanced analyzer for advanced usage
def create_enhanced_analyzer(dbcrust_url: Optional[str] = None,
                           project_root: Optional[str] = None,
                           database_instance: Optional[Any] = None,
                           enable_all_features: bool = True,
                           transaction_safe: bool = True) -> DjangoAnalyzer:
    """
    Create a fully-featured Django analyzer with all capabilities enabled.
    
    Args:
        dbcrust_url: Database URL for EXPLAIN analysis  
        project_root: Django project root for code/model analysis
        database_instance: Pre-existing PyDatabase instance
        enable_all_features: Enable all analysis features
        transaction_safe: Whether to wrap analysis in transaction (can interfere with session management)
    
    Returns:
        Configured DjangoAnalyzer instance
        
    Usage:
        analyzer = create_enhanced_analyzer(
            dbcrust_url="postgres://user@localhost/db",
            project_root="/path/to/project"
        )
        
        # Runtime query analysis
        with analyzer.analyze():
            MyModel.objects.filter(active=True)
        
        # Get comprehensive results
        report = analyzer.generate_comprehensive_report()
    """
    return DjangoAnalyzer(
        dbcrust_url=dbcrust_url,
        database_instance=database_instance,
        enable_explain=enable_all_features and (dbcrust_url or database_instance),
        enable_code_analysis=enable_all_features and bool(project_root),
        project_root=project_root,
        transaction_safe=transaction_safe
    )