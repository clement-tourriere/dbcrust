"""
PostgreSQL EXPLAIN plan analyzer for Django ORM optimization.

Analyzes EXPLAIN (ANALYZE, FORMAT JSON) output to identify specific
performance issues and provide targeted optimization recommendations.
"""

import json
import re
from dataclasses import dataclass, field
from typing import List, Dict, Optional, Any, Union, Tuple
from collections import defaultdict


@dataclass
class PlanNode:
    """Represents a node in the PostgreSQL execution plan."""
    node_type: str
    relation_name: Optional[str] = None
    alias: Optional[str] = None
    startup_cost: float = 0.0
    total_cost: float = 0.0
    plan_rows: int = 0
    plan_width: int = 0
    actual_startup_time: float = 0.0
    actual_total_time: float = 0.0
    actual_rows: int = 0
    actual_loops: int = 1
    
    # Filter conditions
    filter: Optional[str] = None
    index_cond: Optional[str] = None
    hash_cond: Optional[str] = None
    join_filter: Optional[str] = None
    
    # Join information
    join_type: Optional[str] = None
    parent_relationship: Optional[str] = None
    
    # Buffer usage
    shared_hit_blocks: int = 0
    shared_read_blocks: int = 0
    shared_dirtied_blocks: int = 0
    shared_written_blocks: int = 0
    
    # Child plans
    plans: List['PlanNode'] = field(default_factory=list)
    
    @property
    def effective_rows(self) -> int:
        """Calculate effective rows considering loops."""
        return self.actual_rows * self.actual_loops
    
    @property
    def cost_per_row(self) -> float:
        """Calculate cost per row if rows > 0."""
        return self.total_cost / max(self.plan_rows, 1)
    
    @property
    def time_per_loop(self) -> float:
        """Calculate time per loop."""
        return self.actual_total_time / max(self.actual_loops, 1)


@dataclass
class OptimizationSuggestion:
    """Represents an optimization suggestion based on plan analysis."""
    priority: str  # critical, high, medium, low
    category: str  # index, join, query_structure, etc.
    title: str
    description: str
    django_suggestion: str
    sql_suggestion: Optional[str] = None
    estimated_improvement: Optional[str] = None
    affected_tables: List[str] = field(default_factory=list)
    affected_columns: List[str] = field(default_factory=list)


class PostgreSQLPlanAnalyzer:
    """Analyzes PostgreSQL execution plans for optimization opportunities."""
    
    def __init__(self):
        self.suggestions: List[OptimizationSuggestion] = []
        self.plan_stats = {
            'total_cost': 0.0,
            'total_time': 0.0,
            'total_rows': 0,
            'seq_scans': 0,
            'index_scans': 0,
            'nested_loops': 0,
            'hash_joins': 0,
            'buffer_hits': 0,
            'buffer_reads': 0
        }
    
    def analyze_plan(self, plan_json: Union[str, Dict[str, Any]]) -> List[OptimizationSuggestion]:
        """
        Analyze a PostgreSQL execution plan.
        
        Args:
            plan_json: JSON string or dict containing EXPLAIN output
            
        Returns:
            List of optimization suggestions
        """
        self.suggestions.clear()
        
        try:
            if isinstance(plan_json, str):
                plan_data = json.loads(plan_json)
            else:
                plan_data = plan_json
            
            # Extract the plan from EXPLAIN output
            if isinstance(plan_data, list) and len(plan_data) > 0:
                plan_info = plan_data[0]
            else:
                plan_info = plan_data
            
            # Extract timing information
            execution_time = plan_info.get('Execution Time', 0)
            planning_time = plan_info.get('Planning Time', 0)
            
            # Parse the plan tree
            if 'Plan' in plan_info:
                root_plan = self._parse_plan_node(plan_info['Plan'])
                
                # Analyze the plan tree
                self._analyze_plan_tree(root_plan)
                
                # Add overall performance suggestions
                self._add_general_suggestions(execution_time, planning_time, root_plan)
            
            return self.suggestions
            
        except (json.JSONDecodeError, KeyError, TypeError) as e:
            print(f"Error parsing execution plan: {e}")
            return []
    
    def _parse_plan_node(self, node_data: Dict[str, Any]) -> PlanNode:
        """Parse a plan node from JSON data."""
        node = PlanNode(
            node_type=node_data.get('Node Type', 'Unknown'),
            relation_name=node_data.get('Relation Name'),
            alias=node_data.get('Alias'),
            startup_cost=node_data.get('Startup Cost', 0.0),
            total_cost=node_data.get('Total Cost', 0.0),
            plan_rows=node_data.get('Plan Rows', 0),
            plan_width=node_data.get('Plan Width', 0),
            actual_startup_time=node_data.get('Actual Startup Time', 0.0),
            actual_total_time=node_data.get('Actual Total Time', 0.0),
            actual_rows=node_data.get('Actual Rows', 0),
            actual_loops=node_data.get('Actual Loops', 1),
            filter=node_data.get('Filter'),
            index_cond=node_data.get('Index Cond'),
            hash_cond=node_data.get('Hash Cond'),
            join_filter=node_data.get('Join Filter'),
            join_type=node_data.get('Join Type'),
            parent_relationship=node_data.get('Parent Relationship'),
        )
        
        # Parse buffer usage if available
        if 'Buffers' in node_data:
            buffers = node_data['Buffers']
            node.shared_hit_blocks = buffers.get('Shared Hit Blocks', 0)
            node.shared_read_blocks = buffers.get('Shared Read Blocks', 0)
            node.shared_dirtied_blocks = buffers.get('Shared Dirtied Blocks', 0)
            node.shared_written_blocks = buffers.get('Shared Written Blocks', 0)
        
        # Parse child plans
        if 'Plans' in node_data:
            for child_data in node_data['Plans']:
                child_node = self._parse_plan_node(child_data)
                node.plans.append(child_node)
        
        return node
    
    def _analyze_plan_tree(self, node: PlanNode):
        """Recursively analyze the plan tree."""
        # Update statistics
        self.plan_stats['total_cost'] += node.total_cost
        self.plan_stats['total_time'] += node.actual_total_time
        self.plan_stats['total_rows'] += node.effective_rows
        
        # Analyze specific node types
        if node.node_type == 'Seq Scan':
            self._analyze_sequential_scan(node)
            self.plan_stats['seq_scans'] += 1
        
        elif 'Index Scan' in node.node_type:
            self._analyze_index_scan(node)
            self.plan_stats['index_scans'] += 1
        
        elif node.node_type == 'Nested Loop':
            self._analyze_nested_loop(node)
            self.plan_stats['nested_loops'] += 1
        
        elif node.node_type == 'Hash Join':
            self._analyze_hash_join(node)
            self.plan_stats['hash_joins'] += 1
        
        elif node.node_type == 'Sort':
            self._analyze_sort_operation(node)
        
        elif node.node_type == 'Aggregate':
            self._analyze_aggregate_operation(node)
        
        elif 'Bitmap' in node.node_type:
            self._analyze_bitmap_scan(node)
        
        # Update buffer statistics
        self.plan_stats['buffer_hits'] += node.shared_hit_blocks
        self.plan_stats['buffer_reads'] += node.shared_read_blocks
        
        # Recursively analyze child nodes
        for child in node.plans:
            self._analyze_plan_tree(child)
    
    def _analyze_sequential_scan(self, node: PlanNode):
        """Analyze sequential scan operations."""
        if node.actual_rows > 1000 or node.actual_total_time > 50:  # 50ms threshold
            # Check if there's a filter that could benefit from an index
            if node.filter:
                columns = self._extract_columns_from_condition(node.filter)
                
                suggestion = OptimizationSuggestion(
                    priority="high" if node.actual_rows > 10000 else "medium",
                    category="index",
                    title=f"Sequential Scan on {node.relation_name}",
                    description=f"Table {node.relation_name} is being scanned sequentially, "
                               f"examining {node.actual_rows:,} rows in {node.actual_total_time:.1f}ms",
                    django_suggestion=self._generate_django_index_suggestion(node.relation_name, columns),
                    sql_suggestion=self._generate_sql_index_suggestion(node.relation_name, columns),
                    estimated_improvement=f"Could reduce scan time from {node.actual_total_time:.1f}ms to <10ms",
                    affected_tables=[node.relation_name] if node.relation_name else [],
                    affected_columns=columns
                )
                
                self.suggestions.append(suggestion)
            else:
                # Sequential scan without filter - might need query optimization
                suggestion = OptimizationSuggestion(
                    priority="medium",
                    category="query_structure",
                    title=f"Full Table Scan on {node.relation_name}",
                    description=f"Full table scan reading {node.actual_rows:,} rows",
                    django_suggestion="Consider adding WHERE conditions or using pagination:\n\n"
                                   "# Add filtering:\nqueryset.filter(field=value)\n\n"
                                   "# Or use pagination:\nfrom django.core.paginator import Paginator",
                    estimated_improvement="Reduce data processing and improve response time",
                    affected_tables=[node.relation_name] if node.relation_name else []
                )
                
                self.suggestions.append(suggestion)
    
    def _analyze_index_scan(self, node: PlanNode):
        """Analyze index scan operations."""
        # Check for inefficient index usage
        if node.actual_loops > 100:
            suggestion = OptimizationSuggestion(
                priority="high",
                category="query_structure",
                title=f"High Loop Count in Index Scan",
                description=f"Index scan on {node.relation_name} executed {node.actual_loops} times",
                django_suggestion="This indicates a potential N+1 query pattern:\n\n"
                               "# Use select_related or prefetch_related:\n"
                               "queryset.select_related('foreign_key_field')\n"
                               "queryset.prefetch_related('many_to_many_field')",
                estimated_improvement=f"Reduce {node.actual_loops} index scans to 1-2 queries",
                affected_tables=[node.relation_name] if node.relation_name else []
            )
            
            self.suggestions.append(suggestion)
    
    def _analyze_nested_loop(self, node: PlanNode):
        """Analyze nested loop joins."""
        if node.actual_loops > 1000 or (node.actual_total_time > 100 and node.actual_rows > 0):
            estimated_operations = node.actual_rows * node.actual_loops
            
            suggestion = OptimizationSuggestion(
                priority="high" if estimated_operations > 10000 else "medium",
                category="join",
                title="Expensive Nested Loop Join",
                description=f"Nested loop performing {estimated_operations:,} operations "
                           f"in {node.actual_total_time:.1f}ms",
                django_suggestion="Consider optimizing the join:\n\n"
                               "# Add select_related for foreign keys:\n"
                               "queryset.select_related('foreign_key')\n\n"
                               "# Or add database indexes on join columns:\n"
                               "class Meta:\n    indexes = [models.Index(fields=['join_column'])]",
                sql_suggestion="Consider adding indexes on join columns or using a hash join",
                estimated_improvement="Could reduce join time significantly with proper indexes",
                affected_tables=self._extract_tables_from_join(node)
            )
            
            self.suggestions.append(suggestion)
    
    def _analyze_hash_join(self, node: PlanNode):
        """Analyze hash join operations."""
        # Hash joins are generally good, but check for memory usage
        if node.actual_total_time > 500:  # 500ms threshold
            suggestion = OptimizationSuggestion(
                priority="low",
                category="memory",
                title="Slow Hash Join",
                description=f"Hash join taking {node.actual_total_time:.1f}ms",
                django_suggestion="Hash join is generally efficient, but if slow:\n\n"
                               "# Consider reducing the dataset:\n"
                               "queryset.filter(...).select_related(...)",
                estimated_improvement="Reduce join complexity or increase work_mem",
                affected_tables=self._extract_tables_from_join(node)
            )
            
            self.suggestions.append(suggestion)
    
    def _analyze_sort_operation(self, node: PlanNode):
        """Analyze sort operations."""
        if node.actual_total_time > 50:  # 50ms threshold
            sort_keys = self._extract_sort_keys(node)
            
            suggestion = OptimizationSuggestion(
                priority="medium",
                category="index",
                title="Expensive Sort Operation",
                description=f"Sort operation taking {node.actual_total_time:.1f}ms on {node.actual_rows:,} rows",
                django_suggestion=self._generate_django_ordering_suggestion(sort_keys),
                sql_suggestion=f"Consider adding an index on sort columns: {', '.join(sort_keys)}",
                estimated_improvement="Could eliminate sort step with proper index",
                affected_columns=sort_keys
            )
            
            self.suggestions.append(suggestion)
    
    def _analyze_aggregate_operation(self, node: PlanNode):
        """Analyze aggregate operations."""
        if node.actual_total_time > 100:  # 100ms threshold
            suggestion = OptimizationSuggestion(
                priority="medium",
                category="aggregation",
                title="Slow Aggregation",
                description=f"Aggregation taking {node.actual_total_time:.1f}ms",
                django_suggestion="Optimize aggregation queries:\n\n"
                               "# Use database aggregation instead of Python:\n"
                               "from django.db.models import Count, Sum, Avg\n"
                               "Model.objects.aggregate(total=Count('id'))\n\n"
                               "# Add indexes on aggregated fields",
                estimated_improvement="Significantly reduce aggregation time",
                affected_tables=[node.relation_name] if node.relation_name else []
            )
            
            self.suggestions.append(suggestion)
    
    def _analyze_bitmap_scan(self, node: PlanNode):
        """Analyze bitmap scan operations."""
        # Bitmap scans indicate partial index usage - often good
        if node.actual_total_time > 100:
            suggestion = OptimizationSuggestion(
                priority="low",
                category="index",
                title="Bitmap Index Scan",
                description=f"Bitmap scan on {node.relation_name} taking {node.actual_total_time:.1f}ms",
                django_suggestion="Bitmap scans are generally efficient for range queries.\n"
                               "Consider if the query can be more selective",
                estimated_improvement="Already using index efficiently",
                affected_tables=[node.relation_name] if node.relation_name else []
            )
            
            self.suggestions.append(suggestion)
    
    def _extract_columns_from_condition(self, condition: str) -> List[str]:
        """Extract column names from a filter condition."""
        # Simple regex to extract column names
        columns = []
        # Pattern for "table.column operator value"
        matches = re.findall(r'(\w+)\s*[<>=!]+', condition)
        for match in matches:
            if match not in ['AND', 'OR', 'NOT']:
                columns.append(match)
        
        return list(set(columns))  # Remove duplicates
    
    def _extract_tables_from_join(self, node: PlanNode) -> List[str]:
        """Extract table names involved in a join."""
        tables = []
        
        if node.relation_name:
            tables.append(node.relation_name)
        
        for child in node.plans:
            if child.relation_name:
                tables.append(child.relation_name)
        
        return list(set(tables))
    
    def _extract_sort_keys(self, node: PlanNode) -> List[str]:
        """Extract sort keys from a sort node."""
        # This would need more sophisticated parsing of the sort keys
        # For now, return empty list as placeholder
        return []
    
    def _generate_django_index_suggestion(self, table_name: Optional[str], columns: List[str]) -> str:
        """Generate Django model index suggestion."""
        if not columns:
            return "Consider adding appropriate database indexes"
        
        model_name = self._table_to_model_name(table_name)
        fields_str = ', '.join(f"'{col}'" for col in columns)
        
        return f"""Add index to {model_name} model:

class {model_name}(models.Model):
    # ... existing fields ...
    
    class Meta:
        indexes = [
            models.Index(fields=[{fields_str}]),
        ]"""
    
    def _generate_sql_index_suggestion(self, table_name: Optional[str], columns: List[str]) -> str:
        """Generate SQL index creation suggestion."""
        if not columns or not table_name:
            return ""
        
        columns_str = ', '.join(columns)
        index_name = f"idx_{table_name}_{'_'.join(columns[:3])}"  # Limit length
        
        return f"CREATE INDEX {index_name} ON {table_name} ({columns_str});"
    
    def _generate_django_ordering_suggestion(self, sort_keys: List[str]) -> str:
        """Generate Django ordering optimization suggestion."""
        if not sort_keys:
            return "Consider adding database indexes for ORDER BY fields"
        
        fields_str = ', '.join(f"'{key}'" for key in sort_keys)
        
        return f"""Add index for ordering:

class Meta:
    indexes = [
        models.Index(fields=[{fields_str}]),
    ]
    # Or use db_index on individual fields:
    # field_name = models.CharField(db_index=True)"""
    
    def _table_to_model_name(self, table_name: Optional[str]) -> str:
        """Convert table name to likely Django model name."""
        if not table_name:
            return "Model"
        
        # Convert snake_case to PascalCase
        return ''.join(word.capitalize() for word in table_name.split('_'))
    
    def _add_general_suggestions(self, execution_time: float, planning_time: float, root_plan: PlanNode):
        """Add general performance suggestions based on overall metrics."""
        # High planning time
        if planning_time > 10:  # 10ms planning time
            self.suggestions.append(OptimizationSuggestion(
                priority="low",
                category="planning",
                title="High Query Planning Time",
                description=f"Query planning took {planning_time:.1f}ms",
                django_suggestion="Consider using prepared statements or connection pooling:\n\n"
                               "# In settings.py:\n"
                               "DATABASES['default']['CONN_MAX_AGE'] = 600",
                estimated_improvement="Reduce planning overhead"
            ))
        
        # High execution time
        if execution_time > 1000:  # 1 second
            self.suggestions.append(OptimizationSuggestion(
                priority="critical",
                category="performance",
                title="Very Slow Query",
                description=f"Total execution time: {execution_time:.1f}ms",
                django_suggestion="This query is very slow. Consider:\n\n"
                               "1. Adding appropriate indexes\n"
                               "2. Using select_related/prefetch_related\n"
                               "3. Adding query filters\n"
                               "4. Using pagination\n"
                               "5. Caching results",
                estimated_improvement=f"Target: reduce from {execution_time:.0f}ms to <100ms"
            ))
        
        # Buffer read/hit ratio
        total_buffers = self.plan_stats['buffer_hits'] + self.plan_stats['buffer_reads']
        if total_buffers > 0:
            hit_ratio = self.plan_stats['buffer_hits'] / total_buffers
            if hit_ratio < 0.9:  # Less than 90% cache hit ratio
                self.suggestions.append(OptimizationSuggestion(
                    priority="medium",
                    category="caching",
                    title="Low Buffer Cache Hit Ratio",
                    description=f"Cache hit ratio: {hit_ratio:.1%}",
                    django_suggestion="Consider increasing PostgreSQL shared_buffers or optimizing query patterns",
                    estimated_improvement="Better cache usage will improve performance"
                ))
    
    def generate_summary(self) -> str:
        """Generate a summary of the analysis."""
        if not self.suggestions:
            return "✅ No major optimization opportunities found in the execution plan."
        
        summary_lines = [
            "PostgreSQL Execution Plan Analysis",
            "==================================",
            f"Found {len(self.suggestions)} optimization opportunities:",
            ""
        ]
        
        # Group by priority
        by_priority = defaultdict(list)
        for suggestion in self.suggestions:
            by_priority[suggestion.priority].append(suggestion)
        
        for priority in ['critical', 'high', 'medium', 'low']:
            if priority in by_priority:
                count = len(by_priority[priority])
                summary_lines.append(f"🔴 {priority.upper()}: {count} issues" if priority == 'critical'
                                   else f"🟡 {priority.upper()}: {count} issues" if priority == 'high'
                                   else f"🟠 {priority.upper()}: {count} issues" if priority == 'medium'
                                   else f"🟢 {priority.upper()}: {count} issues")
        
        summary_lines.extend([
            "",
            "Plan Statistics:",
            f"- Sequential Scans: {self.plan_stats['seq_scans']}",
            f"- Index Scans: {self.plan_stats['index_scans']}",
            f"- Nested Loops: {self.plan_stats['nested_loops']}",
            f"- Hash Joins: {self.plan_stats['hash_joins']}",
            f"- Buffer Hit Ratio: {self.plan_stats['buffer_hits']/(self.plan_stats['buffer_hits']+self.plan_stats['buffer_reads']+1):.1%}",
        ])
        
        return "\n".join(summary_lines)
    
    def get_top_suggestions(self, limit: int = 5) -> List[OptimizationSuggestion]:
        """Get top optimization suggestions by priority."""
        priority_order = {'critical': 0, 'high': 1, 'medium': 2, 'low': 3}
        
        sorted_suggestions = sorted(
            self.suggestions,
            key=lambda s: (priority_order.get(s.priority, 4), s.title)
        )
        
        return sorted_suggestions[:limit]


def analyze_explain_output(explain_json: Union[str, Dict[str, Any]]) -> Tuple[List[OptimizationSuggestion], str]:
    """
    Analyze PostgreSQL EXPLAIN output and return optimization suggestions.
    
    Args:
        explain_json: JSON output from EXPLAIN (ANALYZE, FORMAT JSON)
    
    Returns:
        Tuple of (suggestions list, summary string)
    """
    analyzer = PostgreSQLPlanAnalyzer()
    suggestions = analyzer.analyze_plan(explain_json)
    summary = analyzer.generate_summary()
    
    return suggestions, summary