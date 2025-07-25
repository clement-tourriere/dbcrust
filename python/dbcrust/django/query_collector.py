"""
Query collector for capturing Django database queries.

Uses Django's connection.execute_wrapper to intercept all database queries
and collect them with metadata for analysis.
"""

import time
import traceback
from dataclasses import dataclass, field
from typing import List, Dict, Any, Optional, Callable
from datetime import datetime


@dataclass
class CapturedQuery:
    """Represents a captured database query with metadata."""
    sql: str
    params: tuple
    duration: float
    timestamp: datetime
    stack_trace: List[str]
    query_type: str  # SELECT, INSERT, UPDATE, DELETE
    table_names: List[str]
    status: str = "ok"  # ok or error
    exception: Optional[Exception] = None
    
    def get_normalized_sql(self) -> str:
        """Get SQL with parameters replaced by placeholders for pattern matching."""
        # Simple normalization - replace parameter values with ?
        # This helps identify N+1 queries with different parameter values
        sql = self.sql
        if self.params:
            # For positional parameters (%s), already normalized
            # For named parameters, we'd need more complex logic
            pass
        return sql
    
    def get_base_query(self) -> str:
        """Extract base query pattern for N+1 detection."""
        # Remove specific values from WHERE clauses to find patterns
        import re
        
        # Normalize whitespace
        normalized = " ".join(self.sql.split())
        
        # Replace common parameter patterns
        # IN (...) patterns
        normalized = re.sub(r'IN\s*\([^)]+\)', 'IN (?)', normalized, flags=re.IGNORECASE)
        # = value patterns
        normalized = re.sub(r'=\s*%s', '= ?', normalized)
        normalized = re.sub(r'=\s*\d+', '= ?', normalized)
        normalized = re.sub(r"=\s*'[^']+'", '= ?', normalized)
        
        return normalized


class QueryCollector:
    """Collects database queries executed during analysis."""
    
    def __init__(self):
        self.queries: List[CapturedQuery] = []
        self._active = False
        self._start_time = None
        
    def __call__(self, execute: Callable, sql: str, params: tuple, many: bool, context: Dict[str, Any]) -> Any:
        """
        Wrapper function for Django's execute_wrapper.
        
        Args:
            execute: The actual query execution function
            sql: SQL query string
            params: Query parameters
            many: Whether executemany was called
            context: Execution context from Django
        
        Returns:
            Query execution result
        """
        if not self._active:
            return execute(sql, params, many, context)
        
        # Capture stack trace for analysis
        stack = traceback.extract_stack()[:-2]  # Exclude this wrapper
        stack_trace = [f"{frame.filename}:{frame.lineno} in {frame.name}" for frame in stack]
        
        # Extract query type and table names
        query_type = self._extract_query_type(sql)
        table_names = self._extract_table_names(sql)
        
        # Time the query execution
        start = time.perf_counter()
        timestamp = datetime.now()
        
        captured_query = CapturedQuery(
            sql=sql,
            params=params or (),
            duration=0.0,
            timestamp=timestamp,
            stack_trace=stack_trace,
            query_type=query_type,
            table_names=table_names,
        )
        
        try:
            result = execute(sql, params, many, context)
            captured_query.status = "ok"
            return result
        except Exception as e:
            captured_query.status = "error"
            captured_query.exception = e
            raise
        finally:
            # Calculate duration and store query
            captured_query.duration = time.perf_counter() - start
            self.queries.append(captured_query)
    
    def start_collection(self):
        """Start collecting queries."""
        self._active = True
        self._start_time = time.perf_counter()
        self.queries.clear()
    
    def stop_collection(self):
        """Stop collecting queries."""
        self._active = False
    
    def get_total_duration(self) -> float:
        """Get total duration of all queries."""
        return sum(q.duration for q in self.queries)
    
    def get_query_count(self) -> int:
        """Get total number of queries."""
        return len(self.queries)
    
    def get_queries_by_type(self) -> Dict[str, List[CapturedQuery]]:
        """Group queries by type (SELECT, INSERT, etc.)."""
        grouped = {}
        for query in self.queries:
            if query.query_type not in grouped:
                grouped[query.query_type] = []
            grouped[query.query_type].append(query)
        return grouped
    
    def get_duplicate_queries(self) -> Dict[str, List[CapturedQuery]]:
        """Find duplicate queries (exact SQL matches)."""
        duplicates = {}
        seen = {}
        
        for query in self.queries:
            sql_key = query.sql.strip()
            if sql_key in seen:
                if sql_key not in duplicates:
                    duplicates[sql_key] = [seen[sql_key]]
                duplicates[sql_key].append(query)
            else:
                seen[sql_key] = query
        
        return duplicates
    
    def get_similar_queries(self) -> Dict[str, List[CapturedQuery]]:
        """Find similar queries (same pattern, different parameters)."""
        patterns = {}
        
        for query in self.queries:
            pattern = query.get_base_query()
            if pattern not in patterns:
                patterns[pattern] = []
            patterns[pattern].append(query)
        
        # Only return patterns with multiple queries
        return {k: v for k, v in patterns.items() if len(v) > 1}
    
    def clear(self):
        """Clear all collected queries."""
        self.queries.clear()
    
    @staticmethod
    def _extract_query_type(sql: str) -> str:
        """Extract query type from SQL."""
        sql = sql.strip().upper()
        if sql.startswith('SELECT'):
            return 'SELECT'
        elif sql.startswith('INSERT'):
            return 'INSERT'
        elif sql.startswith('UPDATE'):
            return 'UPDATE'
        elif sql.startswith('DELETE'):
            return 'DELETE'
        elif sql.startswith('CREATE'):
            return 'CREATE'
        elif sql.startswith('DROP'):
            return 'DROP'
        elif sql.startswith('ALTER'):
            return 'ALTER'
        else:
            return 'OTHER'
    
    @staticmethod
    def _extract_table_names(sql: str) -> List[str]:
        """Extract table names from SQL query."""
        import re
        
        tables = []
        sql_upper = sql.upper()
        
        # Simple extraction for common patterns
        # FROM table_name
        from_match = re.search(r'FROM\s+([^\s,]+)', sql_upper)
        if from_match:
            table = from_match.group(1).strip('"').strip("'")
            if table and not table.startswith('('):
                tables.append(table.lower())
        
        # JOIN table_name
        join_matches = re.findall(r'JOIN\s+([^\s]+)', sql_upper)
        for match in join_matches:
            table = match.strip('"').strip("'")
            if table:
                tables.append(table.lower())
        
        # INSERT INTO table_name
        insert_match = re.search(r'INSERT\s+INTO\s+([^\s(]+)', sql_upper)
        if insert_match:
            table = insert_match.group(1).strip('"').strip("'")
            if table:
                tables.append(table.lower())
        
        # UPDATE table_name
        update_match = re.search(r'UPDATE\s+([^\s]+)', sql_upper)
        if update_match:
            table = update_match.group(1).strip('"').strip("'")
            if table:
                tables.append(table.lower())
        
        # DELETE FROM table_name
        delete_match = re.search(r'DELETE\s+FROM\s+([^\s]+)', sql_upper)
        if delete_match:
            table = delete_match.group(1).strip('"').strip("'")
            if table:
                tables.append(table.lower())
        
        # Remove duplicates while preserving order
        seen = set()
        unique_tables = []
        for table in tables:
            if table not in seen:
                seen.add(table)
                unique_tables.append(table)
        
        return unique_tables