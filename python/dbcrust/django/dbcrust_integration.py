"""
DBCrust integration for Django query analysis.

Provides EXPLAIN ANALYZE functionality and performance metrics
using DBCrust's database connections and performance analyzer.
"""

import asyncio
import json
from typing import List, Dict, Any, Optional, Tuple
from concurrent.futures import ThreadPoolExecutor
import threading

from .query_collector import CapturedQuery

# Import DBCrust components with error handling
try:
    from dbcrust import PyDatabase, PyConfig
    DBCRUST_AVAILABLE = True
except ImportError:
    DBCRUST_AVAILABLE = False
    PyDatabase = None
    PyConfig = None


class DBCrustIntegration:
    """Integrates Django analyzer with DBCrust for advanced query analysis."""
    
    def __init__(self, connection_url: Optional[str] = None, database_instance: Optional['PyDatabase'] = None):
        """
        Initialize DBCrust integration.
        
        Args:
            connection_url: DBCrust-compatible database URL
            database_instance: Pre-existing PyDatabase instance to use
        """
        if not DBCRUST_AVAILABLE:
            raise ImportError("DBCrust is not available. This should not happen in a DBCrust installation.")
        
        self.connection_url = connection_url
        self._database = database_instance
        self._connected = False
        self._executor = ThreadPoolExecutor(max_workers=1)
    
    def connect(self):
        """Establish connection to database using DBCrust."""
        if self._database is not None:
            self._connected = True
            return
        
        if not self.connection_url:
            return
        
        try:
            # Parse connection URL to extract connection parameters
            from urllib.parse import urlparse
            
            parsed = urlparse(self.connection_url)
            if parsed.scheme not in ['postgres', 'postgresql']:
                raise ValueError(f"Unsupported database type: {parsed.scheme}")
            
            # Create PyDatabase instance
            self._database = PyDatabase(
                host=parsed.hostname or 'localhost',
                port=parsed.port or 5432,
                user=parsed.username or 'postgres',
                password=parsed.password or '',
                dbname=parsed.path.lstrip('/') or 'postgres'
            )
            
            self._connected = True
            print(f"✅ Connected to database: {parsed.hostname}")
            
        except Exception as e:
            print(f"❌ Failed to connect to database: {e}")
            self._database = None
            self._connected = False
    
    def _analyze_query_sync(self, query: CapturedQuery) -> Dict[str, Any]:
        """Analyze a single query using EXPLAIN ANALYZE (synchronous version)."""
        if not self._database or not self._connected:
            return {"error": "Database not connected"}
        
        try:
            # Run EXPLAIN ANALYZE
            explain_sql = f"EXPLAIN (ANALYZE, FORMAT JSON) {query.sql}"
            
            # Execute the explain query using PyDatabase.execute()
            result = self._database.execute(explain_sql)
            
            # Convert Python result to expected format
            if hasattr(result, '__iter__') and result:
                # Result should be a list of rows, each row a list of columns
                rows = list(result)
                if rows and len(rows) > 0:
                    # First column of first row should contain the JSON
                    explain_json_str = rows[0][0] if isinstance(rows[0], (list, tuple)) else rows[0]
                    
                    if isinstance(explain_json_str, str):
                        explain_json = json.loads(explain_json_str)
                    else:
                        explain_json = explain_json_str
                    
                    # Import our query plan analyzer
                    from .query_plan_analyzer import analyze_explain_output
                    
                    # Analyze the plan
                    suggestions, summary = analyze_explain_output(explain_json)
                    
                    return {
                        "query": query.sql,
                        "execution_time": query.duration * 1000,  # Convert to ms
                        "explain_plan": explain_json,
                        "performance_insights": self._extract_performance_insights(explain_json),
                        "optimization_suggestions": [
                            {
                                "priority": s.priority,
                                "category": s.category,
                                "title": s.title,
                                "description": s.description,
                                "django_suggestion": s.django_suggestion,
                                "sql_suggestion": s.sql_suggestion,
                                "estimated_improvement": s.estimated_improvement
                            } for s in suggestions
                        ],
                        "plan_summary": summary
                    }
                
        except json.JSONDecodeError as e:
            return {
                "query": query.sql,
                "error": f"Failed to parse EXPLAIN output: {e}"
            }
        except Exception as e:
            return {
                "query": query.sql,
                "error": f"EXPLAIN query failed: {e}"
            }
        
        return {"error": "No result returned from EXPLAIN query"}
    
    def analyze_queries(self, queries: List[CapturedQuery], limit: int = 10) -> List[Dict[str, Any]]:
        """
        Analyze multiple queries using DBCrust.
        
        Args:
            queries: List of captured queries to analyze
            limit: Maximum number of queries to analyze with EXPLAIN
        
        Returns:
            List of analysis results
        """
        if not DBCRUST_AVAILABLE or not queries:
            return []
        
        # Connect if not already connected
        if not self._connected:
            self.connect()
        
        if not self._connected:
            return [{"error": "Failed to connect to database"}]
        
        # Filter queries suitable for EXPLAIN
        analyzable_queries = self._filter_analyzable_queries(queries)[:limit]
        
        if not analyzable_queries:
            return [{"info": "No queries suitable for EXPLAIN analysis"}]
        
        print(f"🔍 Analyzing {len(analyzable_queries)} queries with EXPLAIN...")
        
        results = []
        for i, query in enumerate(analyzable_queries):
            print(f"  Analyzing query {i+1}/{len(analyzable_queries)}...")
            result = self._analyze_query_sync(query)
            results.append(result)
            
            # Add a small delay to avoid overwhelming the database
            import time
            time.sleep(0.1)
        
        return results
    
    def _filter_analyzable_queries(self, queries: List[CapturedQuery]) -> List[CapturedQuery]:
        """Filter queries suitable for EXPLAIN ANALYZE."""
        analyzable = []
        
        for query in queries:
            # Only analyze SELECT queries for safety
            if query.query_type != 'SELECT':
                continue
            
            # Skip queries that are already EXPLAIN queries
            if 'EXPLAIN' in query.sql.upper():
                continue
            
            # Skip system/internal queries
            if any(table in ['django_migrations', 'django_content_type', 'auth_permission'] 
                   for table in query.table_names):
                continue
            
            analyzable.append(query)
        
        # Sort by duration to analyze slowest queries first
        analyzable.sort(key=lambda q: q.duration, reverse=True)
        
        return analyzable
    
    def _extract_performance_insights(self, explain_json: Dict[str, Any]) -> Dict[str, Any]:
        """Extract performance insights from EXPLAIN output."""
        insights = {
            "total_cost": 0,
            "execution_time": 0,
            "planning_time": 0,
            "operations": [],
            "warnings": [],
            "recommendations": []
        }
        
        try:
            if isinstance(explain_json, list) and explain_json:
                plan = explain_json[0]
                
                # Extract timing information
                if "Execution Time" in plan:
                    insights["execution_time"] = plan["Execution Time"]
                if "Planning Time" in plan:
                    insights["planning_time"] = plan["Planning Time"]
                
                # Extract plan details
                if "Plan" in plan:
                    self._analyze_plan_node(plan["Plan"], insights)
        
        except Exception as e:
            insights["error"] = str(e)
        
        return insights
    
    def _analyze_plan_node(self, node: Dict[str, Any], insights: Dict[str, Any]):
        """Recursively analyze plan nodes for performance issues."""
        if not isinstance(node, dict):
            return
        
        # Extract node information
        node_type = node.get("Node Type", "Unknown")
        
        operation = {
            "type": node_type,
            "cost": node.get("Total Cost", 0),
            "rows": node.get("Actual Rows", 0),
            "time": node.get("Actual Total Time", 0),
            "loops": node.get("Actual Loops", 1)
        }
        
        insights["operations"].append(operation)
        insights["total_cost"] += operation["cost"]
        
        # Check for performance issues
        if node_type == "Seq Scan":
            table_name = node.get("Relation Name", "unknown")
            if operation["rows"] > 1000:
                insights["warnings"].append(
                    f"Sequential scan on {table_name} examining {operation['rows']} rows"
                )
                insights["recommendations"].append(
                    f"Consider adding an index on {table_name}"
                )
        
        elif node_type == "Nested Loop" and operation["loops"] > 100:
            insights["warnings"].append(
                f"Nested loop with {operation['loops']} iterations"
            )
            insights["recommendations"].append(
                "Consider using a hash join or merge join instead"
            )
        
        # Check for slow operations
        if operation["time"] > 100:  # More than 100ms
            insights["warnings"].append(
                f"{node_type} operation taking {operation['time']:.1f}ms"
            )
        
        # Recursively analyze child plans
        if "Plans" in node:
            for child_plan in node["Plans"]:
                self._analyze_plan_node(child_plan, insights)
    
    def generate_performance_report(self, analysis_results: List[Dict[str, Any]]) -> str:
        """Generate a performance report from analysis results."""
        if not analysis_results:
            return "No queries were analyzed."
        
        report_lines = [
            "DBCrust Performance Analysis Report",
            "==================================",
            ""
        ]
        
        # Summary statistics
        total_time = sum(r.get("execution_time", 0) for r in analysis_results)
        analyzed_count = len([r for r in analysis_results if "error" not in r])
        
        report_lines.extend([
            f"Queries Analyzed: {analyzed_count}/{len(analysis_results)}",
            f"Total Execution Time: {total_time:.2f}ms",
            ""
        ])
        
        # Individual query analysis
        for i, result in enumerate(analysis_results, 1):
            report_lines.append(f"Query {i}:")
            report_lines.append("-" * 40)
            
            if "error" in result:
                report_lines.extend([
                    f"Error: {result['error']}",
                    ""
                ])
                continue
            
            query = result.get("query", "Unknown")
            if len(query) > 100:
                query = query[:97] + "..."
            report_lines.append(f"SQL: {query}")
            
            insights = result.get("performance_insights", {})
            
            # Timing information
            exec_time = insights.get("execution_time", 0)
            plan_time = insights.get("planning_time", 0)
            report_lines.extend([
                f"Execution Time: {exec_time:.2f}ms",
                f"Planning Time: {plan_time:.2f}ms",
                f"Total Cost: {insights.get('total_cost', 0):.2f}",
                ""
            ])
            
            # Operations summary
            operations = insights.get("operations", [])
            if operations:
                report_lines.append("Operations:")
                for op in operations[:5]:  # Show top 5 operations
                    report_lines.append(
                        f"  - {op['type']}: {op['time']:.2f}ms, {op['rows']} rows"
                    )
                if len(operations) > 5:
                    report_lines.append(f"  ... and {len(operations) - 5} more")
                report_lines.append("")
            
            # Warnings and recommendations
            warnings = insights.get("warnings", [])
            if warnings:
                report_lines.append("⚠️  Warnings:")
                for warning in warnings:
                    report_lines.append(f"  - {warning}")
                report_lines.append("")
            
            recommendations = insights.get("recommendations", [])
            if recommendations:
                report_lines.append("💡 Recommendations:")
                for rec in recommendations:
                    report_lines.append(f"  - {rec}")
                report_lines.append("")
            
            report_lines.append("")
        
        return "\n".join(report_lines)
    
    def cleanup(self):
        """Clean up resources."""
        if self._executor:
            self._executor.shutdown(wait=False)
        
        # PyDatabase handles connection cleanup automatically
        self._connected = False
        print("🔌 Database connection cleaned up")


# Integration function for the main analyzer
def enhance_analysis_with_dbcrust(
    queries: List[CapturedQuery],
    connection_url: Optional[str] = None,
    database_instance: Optional['PyDatabase'] = None,
    max_queries: int = 10
) -> Tuple[List[Dict[str, Any]], str]:
    """
    Enhance query analysis with DBCrust EXPLAIN ANALYZE.
    
    Args:
        queries: List of captured queries
        connection_url: DBCrust database connection URL (if database_instance not provided)
        database_instance: Pre-existing PyDatabase instance to use
        max_queries: Maximum number of queries to analyze
    
    Returns:
        Tuple of (analysis results, performance report)
    """
    if not (connection_url or database_instance) or not queries:
        return [], "No DBCrust analysis performed - no connection provided."
    
    integration = DBCrustIntegration(connection_url, database_instance)
    
    try:
        # Analyze queries
        results = integration.analyze_queries(queries, limit=max_queries)
        
        # Generate report
        report = integration.generate_performance_report(results)
        
        return results, report
        
    finally:
        integration.cleanup()


# Convenience function for direct analysis
def analyze_single_query(query: str, connection_url: str, params: Optional[tuple] = None) -> Dict[str, Any]:
    """
    Analyze a single SQL query with EXPLAIN.
    
    Args:
        query: SQL query to analyze
        connection_url: Database connection URL
        params: Query parameters (optional)
    
    Returns:
        Analysis result dictionary
    """
    from datetime import datetime
    
    # Create a mock CapturedQuery
    captured = type('CapturedQuery', (), {
        'sql': query,
        'params': params or (),
        'duration': 0.0,
        'timestamp': datetime.now(),
        'query_type': 'SELECT',
        'table_names': [],
        'status': 'ok'
    })()
    
    integration = DBCrustIntegration(connection_url)
    
    try:
        integration.connect()
        return integration._analyze_query_sync(captured)
    finally:
        integration.cleanup()