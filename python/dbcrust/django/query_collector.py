"""
Query collector for capturing Django database queries.

Uses Django's connection.execute_wrapper to intercept all database queries
and collect them with metadata for analysis.
"""

import time
import traceback
from dataclasses import dataclass
from typing import List, Dict, Any, Optional, Callable, Tuple
from datetime import datetime

#: Depth limit for per-query stack capture. Walking the full stack and
#: reading source lines cost 0.5–2ms per query — a 100-query page paid
#: 50–200ms of pure middleware overhead.
STACK_CAPTURE_LIMIT = 30

#: Hard cap on collected queries per request/analysis block. A long-running
#: management command or streaming response under the analyzer previously
#: grew the list without bound.
MAX_CAPTURED_QUERIES = 2000


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
    # repr() of the raised exception — keeping the exception object pinned
    # its full traceback (frames + locals) in memory until the next request
    exception: Optional[str] = None

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
        self.truncated = False  # True when MAX_CAPTURED_QUERIES was hit
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

        # Stop collecting (but keep executing!) once the cap is reached
        if len(self.queries) >= MAX_CAPTURED_QUERIES:
            self.truncated = True
            return execute(sql, params, many, context)

        # Capture stack trace for analysis (depth-limited: see constant)
        stack = traceback.extract_stack(limit=STACK_CAPTURE_LIMIT)[:-2]  # Exclude this wrapper
        stack_trace = self._extract_meaningful_stack_trace(stack)

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
            captured_query.exception = repr(e)
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
        self.truncated = False

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

    def get_duplicate_queries(self) -> Dict[Tuple[str, str], List[CapturedQuery]]:
        """Find duplicate queries: same SQL **and** same parameters.

        Same-SQL-different-params is an N+1/similar-query signal, not a
        duplicate — keying on SQL alone made every N+1 loop report as
        "duplicates: cache the result", which is the wrong advice.
        """
        duplicates = {}
        seen = {}

        for query in self.queries:
            key = (query.sql.strip(), repr(query.params))
            if key in seen:
                if key not in duplicates:
                    duplicates[key] = [seen[key]]
                duplicates[key].append(query)
            else:
                seen[key] = query

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

    def _extract_meaningful_stack_trace(self, stack) -> List[str]:
        """Extract hierarchical stack trace with primary and secondary locations."""
        # Categorize frames by relevance level
        primary_frames = []      # Most actionable user code
        secondary_frames = []    # Supporting context

        # Patterns to identify Django ORM method calls
        orm_patterns = [
            '.objects.',
            '.filter(',
            '.get(',
            '.all(',
            '.first(',
            '.last(',
            '.count(',
            '.exists(',
            '.create(',
            '.update(',
            '.delete(',
            '.annotate(',
            '.aggregate(',
            '.values(',
            '.values_list(',
            '.select_related(',
            '.prefetch_related(',
            '.distinct(',
            '.order_by(',
            '.exclude(',
        ]

        # High-relevance patterns (Django admin, user views, models)
        high_relevance_patterns = [
            '/admin/',
            '/contrib/admin/',
            'admin.py',
            'views.py',
            'models.py',
            'forms.py',
            'serializers.py'
        ]

        # Medium-relevance patterns (Django framework that provides context)
        medium_relevance_patterns = [
            'django/contrib/admin/',
            'django/views/',
            'django/forms/',
            'rest_framework/'
        ]

        # Skip completely (truly low-level internals)
        skip_patterns = [
            'socketserver.py',
            'threading.py',
            'contextlib.py',
            'django/db/models/sql/',
            'django/db/backends/',
            'django/core/handlers/base.py',
            'django/core/handlers/wsgi.py',
            'site-packages/gunicorn/',
            'site-packages/uwsgi/',
            'dbcrust/django/',
            'query_collector.py'
        ]

        for frame in reversed(stack):
            filename = frame.filename
            code_line = getattr(frame, 'line', '') or ''

            # Skip truly low-level internals
            if any(skip_pattern in filename for skip_pattern in skip_patterns):
                continue

            # Format frame info
            frame_info = f"{filename}:{frame.lineno} in {frame.name}"

            # Add code context if it contains Django ORM patterns
            if any(pattern in code_line for pattern in orm_patterns):
                frame_info += f" ({code_line.strip()})"

            # Categorize by relevance
            is_high_relevance = any(pattern in filename for pattern in high_relevance_patterns)
            is_medium_relevance = any(pattern in filename for pattern in medium_relevance_patterns)

            if is_high_relevance or any(pattern in code_line for pattern in orm_patterns):
                # High relevance: user code or ORM calls
                if frame_info not in [f.split(' (')[0] for f in primary_frames]:  # Avoid duplicates
                    primary_frames.append(frame_info)

            elif is_medium_relevance and len(secondary_frames) < 2:
                # Medium relevance: Django framework context
                if frame_info not in [f.split(' (')[0] for f in secondary_frames]:  # Avoid duplicates
                    secondary_frames.append(frame_info)

        # If no primary frames found, promote the best secondary frames
        if not primary_frames and secondary_frames:
            primary_frames = secondary_frames[:1]
            secondary_frames = secondary_frames[1:]

        # If still no meaningful frames, do a broader search
        if not primary_frames:
            for frame in reversed(stack):
                filename = frame.filename

                # Look for any non-system code
                if not any(skip in filename for skip in skip_patterns + ['site-packages/']):
                    frame_info = f"{filename}:{frame.lineno} in {frame.name}"
                    primary_frames = [frame_info]
                    break

        # Combine primary and secondary, with primary first
        meaningful_frames = primary_frames + secondary_frames

        return meaningful_frames or ["unknown location"]
