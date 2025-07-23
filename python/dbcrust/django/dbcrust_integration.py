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
    
    def __init__(self, connection_url: Optional[str] = None):
        """
        Initialize DBCrust integration.
        
        Args:
            connection_url: DBCrust-compatible database URL
        """
        if not DBCRUST_AVAILABLE:
            raise ImportError("DBCrust is not available. This should not happen in a DBCrust installation.")
        
        self.connection_url = connection_url
        self._database = None
        self._loop = None
        self._thread = None
        self._executor = ThreadPoolExecutor(max_workers=1)
    
    def connect(self):
        """Establish connection to database using DBCrust."""
        if not self.connection_url:
            return
        
        # Parse connection URL and create PyDatabase instance
        # This is simplified - real implementation would parse the URL properly
        try:
            # For now, we'll use a placeholder since full implementation
            # requires proper URL parsing and async setup
            pass
        except Exception as e:
            print(f"Failed to connect to database: {e}")
    
    async def _analyze_query_async(self, query: CapturedQuery) -> Dict[str, Any]:
        """Analyze a single query using EXPLAIN ANALYZE (async)."""
        if not self._database:
            return {}
        
        try:
            # Run EXPLAIN ANALYZE
            explain_sql = f"EXPLAIN (ANALYZE, FORMAT JSON) {query.sql}"
            
            # Execute the explain query
            result = await self._database.execute_query(explain_sql, query.params)
            
            if result and len(result) > 0 and len(result[0]) > 0:
                # Parse the JSON result
                explain_json = json.loads(result[0][0])
                
                # Import Rust performance analyzer through PyO3
                # This would need to be exposed in the Rust lib.rs
                # For now, we'll process the raw explain output
                
                return {
                    "query": query.sql,
                    "execution_time": query.duration * 1000,  # Convert to ms
                    "explain_plan": explain_json,
                    "performance_insights": self._extract_performance_insights(explain_json)
                }
                
        except Exception as e:
            return {
                "query": query.sql,
                "error": str(e)
            }
        
        return {}
    
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
        
        # Filter queries suitable for EXPLAIN
        analyzable_queries = self._filter_analyzable_queries(queries)[:limit]
        
        if not analyzable_queries:
            return []
        
        # Run async analysis in a separate thread
        def run_async_analysis():
            loop = asyncio.new_event_loop()
            asyncio.set_event_loop(loop)
            
            try:
                # Connect to database
                self.connect()
                
                # Analyze queries
                tasks = [self._analyze_query_async(q) for q in analyzable_queries]
                results = loop.run_until_complete(asyncio.gather(*tasks))
                
                return results
            finally:
                loop.close()
        
        # Execute in thread pool
        future = self._executor.submit(run_async_analysis)
        results = future.result(timeout=30)  # 30 second timeout
        
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
        
        if self._database:
            # Close database connection
            pass


# Integration function for the main analyzer
def enhance_analysis_with_dbcrust(
    queries: List[CapturedQuery],
    connection_url: Optional[str] = None,
    max_queries: int = 10
) -> Tuple[List[Dict[str, Any]], str]:
    """
    Enhance query analysis with DBCrust EXPLAIN ANALYZE.
    
    Args:
        queries: List of captured queries
        connection_url: DBCrust database connection URL
        max_queries: Maximum number of queries to analyze
    
    Returns:
        Tuple of (analysis results, performance report)
    """
    if not connection_url or not queries:
        return [], "No DBCrust analysis performed."
    
    integration = DBCrustIntegration(connection_url)
    
    try:
        # Analyze queries
        results = integration.analyze_queries(queries, limit=max_queries)
        
        # Generate report
        report = integration.generate_performance_report(results)
        
        return results, report
        
    finally:
        integration.cleanup()