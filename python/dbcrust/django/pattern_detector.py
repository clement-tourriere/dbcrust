"""
Pattern detector for identifying Django ORM performance issues.

Detects common patterns like N+1 queries, missing select_related,
missing prefetch_related, and other optimization opportunities.
"""

import re
from dataclasses import dataclass
from typing import List, Dict, Set, Optional, Tuple
from collections import defaultdict

from .query_collector import CapturedQuery


@dataclass
class DetectedPattern:
    """Represents a detected performance issue pattern."""
    pattern_type: str  # n_plus_one, missing_select_related, etc.
    severity: str  # critical, high, medium, low
    description: str
    affected_queries: List[CapturedQuery]
    recommendation: str
    code_suggestion: Optional[str] = None
    estimated_impact: Optional[str] = None
    # Enhanced context
    specific_fields: List[str] = None  # Specific field names for select_related/prefetch_related
    code_locations: List[str] = None  # Where the issue occurs in code
    table_context: Dict[str, str] = None  # Table -> Model mapping context
    query_examples: List[str] = None  # Example problematic SQL queries
    # Issue categorization
    is_user_code: bool = True  # True for user code, False for framework code
    framework_type: Optional[str] = None  # 'django_admin', 'django_forms', etc.


class PatternDetector:
    """Detects performance patterns in captured queries."""
    
    def __init__(self, queries: List[CapturedQuery]):
        self.queries = queries
        self.detected_patterns: List[DetectedPattern] = []
    
    def analyze(self) -> List[DetectedPattern]:
        """Run all pattern detection algorithms."""
        self.detected_patterns.clear()
        
        # Run different detection algorithms
        self._detect_n_plus_one()
        self._detect_missing_select_related()
        self._detect_missing_prefetch_related()
        self._detect_inefficient_count()
        self._detect_missing_only()
        self._detect_large_result_sets()
        self._detect_unnecessary_ordering()
        
        # New advanced pattern detections
        self._detect_subqueries_in_loops()
        self._detect_missing_database_indexes()
        self._detect_inefficient_aggregations()
        self._detect_missing_bulk_operations()
        self._detect_inefficient_exists_checks()
        self._detect_missing_select_for_update()
        self._detect_transaction_issues()
        self._detect_connection_pool_exhaustion()
        self._detect_inefficient_distinct()
        self._detect_missing_values_values_list()
        self._detect_redundant_queries()
        self._detect_missing_query_caching()
        
        return self.detected_patterns
    
    def _detect_n_plus_one(self):
        """Detect N+1 query patterns."""
        # Group queries by their base pattern
        similar_queries = self._group_similar_queries()
        
        for pattern, queries in similar_queries.items():
            if len(queries) < 3:  # Need at least 3 similar queries to suspect N+1
                continue
            
            # Check if these are SELECT queries on related tables
            if not all(q.query_type == 'SELECT' for q in queries):
                continue
            
            # Look for patterns like SELECT ... WHERE id = ?
            if self._is_n_plus_one_pattern(pattern, queries):
                # Try to identify the parent table and related table
                parent_table, related_table = self._identify_related_tables(queries)
                
                # Extract enhanced context
                specific_fields = self._extract_foreign_key_fields(queries)
                code_locations = self._extract_code_locations(queries)
                table_context = self._build_table_context(queries)
                query_examples = [q.sql for q in queries[:2]]  # Show first 2 examples
                
                # Categorize the issue by RESPONSIBILITY, not location
                user_locations, framework_locations = self._categorize_issue_location(code_locations)
                framework_type = self._detect_framework_type(framework_locations)
                
                # For Django admin, responsibility is ALWAYS with the user (configuration)
                is_user_responsibility = len(user_locations) > 0 or framework_type == 'django_admin'
                
                # Generate appropriate recommendations
                recommendation, code_suggestion = self._generate_categorized_recommendation(
                    "n_plus_one", is_user_responsibility, framework_type, specific_fields, len(queries), table_context, code_locations
                )
                
                # Determine if this needs select_related or prefetch_related
                optimization_type = "select_related" if len(queries) < 10 else "prefetch_related"
                
                # Format field suggestions
                field_suggestions = ', '.join(f"'{f}'" for f in specific_fields)
                
                self.detected_patterns.append(DetectedPattern(
                    pattern_type="n_plus_one",
                    severity="critical",
                    description=f"N+1 query pattern: {len(queries)} queries on {related_table or 'related table'} - use {optimization_type}({field_suggestions})",
                    affected_queries=queries,
                    recommendation=recommendation,
                    code_suggestion=code_suggestion,
                    estimated_impact=f"Reduce {len(queries)} queries to 1-2 queries",
                    specific_fields=specific_fields,
                    code_locations=code_locations,
                    table_context=table_context,
                    query_examples=query_examples,
                    is_user_code=is_user_responsibility,
                    framework_type=framework_type
                ))
    
    def _detect_missing_select_related(self):
        """Detect missing select_related for ForeignKey/OneToOne fields."""
        # Look for sequential queries that could be joined
        for i in range(len(self.queries) - 1):
            query1 = self.queries[i]
            query2 = self.queries[i + 1]
            
            # Check if query2 is selecting by ID returned from query1
            if (query1.query_type == 'SELECT' and 
                query2.query_type == 'SELECT' and
                self._is_foreign_key_lookup(query1, query2)):
                
                # Extract specific field names and context
                specific_fields = self._extract_foreign_key_fields([query1, query2])
                code_locations = self._extract_code_locations([query1, query2])
                table_context = self._build_table_context([query1, query2])
                query_examples = [query1.sql, query2.sql]
                
                # Format field suggestions without backslashes in f-string
                field_suggestions = ', '.join(f"'{f}'" for f in specific_fields)
                tables_list = ', '.join(table_context.keys())
                
                self.detected_patterns.append(DetectedPattern(
                    pattern_type="missing_select_related",
                    severity="high",
                    description=f"Sequential queries on {tables_list} could use select_related({field_suggestions})",
                    affected_queries=[query1, query2],
                    recommendation=f"Use select_related({field_suggestions}) to fetch related objects in a single query",
                    code_suggestion=self._generate_contextual_select_related_suggestion(query1, query2, specific_fields),
                    estimated_impact="Reduce 2 queries to 1 query",
                    specific_fields=specific_fields,
                    code_locations=code_locations,
                    table_context=table_context,
                    query_examples=query_examples
                ))
    
    def _detect_missing_prefetch_related(self):
        """Detect missing prefetch_related for ManyToMany/reverse FK fields."""
        # Look for multiple queries on related tables after a main query
        main_queries = [q for q in self.queries if self._is_main_table_query(q)]
        
        for main_query in main_queries:
            # Find subsequent queries that might be fetching related objects
            related_queries = self._find_related_queries_after(main_query)
            
            if len(related_queries) >= 2 and self._is_many_to_many_pattern(main_query, related_queries):
                # Extract enhanced context
                all_queries = [main_query] + related_queries
                specific_fields = self._extract_prefetch_fields(main_query, related_queries)
                code_locations = self._extract_code_locations(all_queries)
                table_context = self._build_table_context(all_queries)
                query_examples = [q.sql for q in all_queries[:2]]
                
                # Format field suggestions without backslashes in f-string
                field_suggestions = ', '.join(f"'{f}'" for f in specific_fields)
                
                self.detected_patterns.append(DetectedPattern(
                    pattern_type="missing_prefetch_related",
                    severity="high",
                    description=f"Multiple queries for related objects: {len(related_queries)} queries - use prefetch_related({field_suggestions})",
                    affected_queries=all_queries,
                    recommendation=f"Use prefetch_related({field_suggestions}) for many-to-many or reverse foreign key relationships",
                    code_suggestion=self._generate_contextual_prefetch_related_suggestion(main_query, related_queries, specific_fields),
                    estimated_impact=f"Reduce {len(related_queries) + 1} queries to 2 queries",
                    specific_fields=specific_fields,
                    code_locations=code_locations,
                    table_context=table_context,
                    query_examples=query_examples
                ))
    
    def _detect_inefficient_count(self):
        """Detect inefficient count operations."""
        for query in self.queries:
            if query.query_type == 'SELECT' and 'COUNT(*)' not in query.sql.upper():
                # Check if the query fetches all rows but only uses count
                if self._is_count_only_pattern(query):
                    # Extract enhanced context
                    code_locations = self._extract_code_locations([query])
                    table_context = self._build_table_context([query])
                    query_examples = [self._truncate_sql(query.sql)]
                    
                    # Categorize by RESPONSIBILITY, not location
                    user_locations, framework_locations = self._categorize_issue_location(code_locations)
                    framework_type = self._detect_framework_type(framework_locations)
                    is_user_responsibility = len(user_locations) > 0 or framework_type == 'django_admin'
                    
                    # Generate appropriate recommendations
                    recommendation, code_suggestion = self._generate_categorized_recommendation(
                        "inefficient_count", is_user_responsibility, framework_type, [], 1, table_context, code_locations
                    )
                    
                    table_names = ', '.join(table_context.keys()) if table_context else 'table'
                    
                    self.detected_patterns.append(DetectedPattern(
                        pattern_type="inefficient_count",
                        severity="medium",
                        description=f"Fetching all rows from {table_names} when only count is needed",
                        affected_queries=[query],
                        recommendation=recommendation,
                        code_suggestion=code_suggestion,
                        estimated_impact="Reduce memory usage and query time",
                        code_locations=code_locations,
                        table_context=table_context,
                        query_examples=query_examples,
                        is_user_code=is_user_responsibility,
                        framework_type=framework_type
                    ))
    
    def _detect_missing_only(self):
        """Detect queries fetching unnecessary fields."""
        for query in self.queries:
            if query.query_type == 'SELECT' and self._fetches_all_fields(query):
                field_count = self._estimate_field_count(query)
                if field_count > 10:  # Arbitrary threshold
                    # Extract enhanced context
                    code_locations = self._extract_code_locations([query])
                    table_context = self._build_table_context([query])
                    query_examples = [query.sql]
                    
                    # Try to suggest actual field names from the SQL
                    suggested_fields = self._extract_commonly_used_fields(query)
                    fields_str = ', '.join(f"'{f}'" for f in suggested_fields)
                    field_suggestion = f"queryset.only({fields_str})"
                    
                    self.detected_patterns.append(DetectedPattern(
                        pattern_type="missing_only",
                        severity="low",
                        description=f"Query fetching all fields ({field_count}+) from {', '.join(table_context.keys())} - consider using only()",
                        affected_queries=[query],
                        recommendation="Use .only() or .defer() to limit fields fetched",
                        code_suggestion=field_suggestion,
                        estimated_impact="Reduce data transfer and memory usage",
                        specific_fields=suggested_fields,
                        code_locations=code_locations,
                        table_context=table_context,
                        query_examples=query_examples
                    ))
    
    def _detect_large_result_sets(self):
        """Detect queries that might return large result sets."""
        for query in self.queries:
            if (query.query_type == 'SELECT' and 
                'LIMIT' not in query.sql.upper() and
                not self._has_specific_where_clause(query)):
                
                # Skip legitimate pagination COUNT queries - they're expected!
                if self._is_legitimate_pagination_query(query):
                    continue
                
                # Extract enhanced context
                code_locations = self._extract_code_locations([query])
                table_context = self._build_table_context([query])
                query_examples = [self._truncate_sql(query.sql)]
                
                # Categorize the issue by RESPONSIBILITY, not location
                user_locations, framework_locations = self._categorize_issue_location(code_locations)
                framework_type = self._detect_framework_type(framework_locations)
                
                # For Django admin, responsibility is ALWAYS with the user (configuration)
                is_user_responsibility = len(user_locations) > 0 or framework_type == 'django_admin'
                
                # Generate appropriate recommendations
                recommendation, code_suggestion = self._generate_categorized_recommendation(
                    "large_result_set", is_user_responsibility, framework_type, [], 1, table_context, code_locations
                )
                
                table_names = ', '.join(table_context.keys()) if table_context else 'unknown table'
                
                self.detected_patterns.append(DetectedPattern(
                    pattern_type="large_result_set",
                    severity="medium",
                    description=f"Query without LIMIT on {table_names} might return large result set",
                    affected_queries=[query],
                    recommendation=recommendation,
                    code_suggestion=code_suggestion,
                    estimated_impact="Prevent memory issues with large datasets",
                    code_locations=code_locations,
                    table_context=table_context,
                    query_examples=query_examples,
                    is_user_code=is_user_responsibility,
                    framework_type=framework_type
                ))
    
    def _detect_unnecessary_ordering(self):
        """Detect unnecessary ORDER BY clauses."""
        order_queries = [q for q in self.queries if 'ORDER BY' in q.sql.upper()]
        
        for query in order_queries:
            # Check if ordering is used without LIMIT (might be unnecessary)
            if 'LIMIT' not in query.sql.upper() and query.duration > 0.1:  # 100ms threshold
                # Extract enhanced context
                code_locations = self._extract_code_locations([query])
                table_context = self._build_table_context([query])
                query_examples = [self._truncate_sql(query.sql)]
                
                table_names = ', '.join(table_context.keys()) if table_context else 'table'
                
                self.detected_patterns.append(DetectedPattern(
                    pattern_type="unnecessary_ordering",
                    severity="low",
                    description=f"ORDER BY on {table_names} without LIMIT might be unnecessary",
                    affected_queries=[query],
                    recommendation="Remove ordering if not needed, or add index for ordered field",
                    estimated_impact="Reduce query execution time",
                    code_locations=code_locations,
                    table_context=table_context,
                    query_examples=query_examples
                ))
    
    # Helper methods
    
    def _group_similar_queries(self) -> Dict[str, List[CapturedQuery]]:
        """Group queries by their base pattern."""
        patterns = defaultdict(list)
        for query in self.queries:
            pattern = query.get_base_query()
            patterns[pattern].append(query)
        return dict(patterns)
    
    def _is_n_plus_one_pattern(self, pattern: str, queries: List[CapturedQuery]) -> bool:
        """Check if queries match N+1 pattern."""
        # Look for patterns like SELECT ... WHERE foreign_key_id = ?
        # or SELECT ... WHERE id IN (?)
        pattern_upper = pattern.upper()
        
        # Common N+1 patterns
        n_plus_one_patterns = [
            r'WHERE\s+\w+_ID\s*=\s*\?',  # WHERE user_id = ?
            r'WHERE\s+ID\s*=\s*\?',       # WHERE id = ?
            r'WHERE\s+\w+\s+IN\s*\(\?\)', # WHERE id IN (?)
        ]
        
        return any(re.search(p, pattern_upper) for p in n_plus_one_patterns)
    
    def _identify_related_tables(self, queries: List[CapturedQuery]) -> Tuple[Optional[str], Optional[str]]:
        """Try to identify parent and related tables from queries."""
        if not queries:
            return None, None
        
        # Get table from first query
        tables = queries[0].table_names
        related_table = tables[0] if tables else None
        
        # Try to find parent table from stack traces or previous queries
        parent_table = None
        # This is simplified - in real implementation would analyze stack traces
        
        return parent_table, related_table
    
    def _is_foreign_key_lookup(self, query1: CapturedQuery, query2: CapturedQuery) -> bool:
        """Check if query2 is looking up a foreign key from query1."""
        # Simplified check - look for ID in WHERE clause
        if 'WHERE' in query2.sql.upper() and 'ID' in query2.sql.upper():
            # Check if queries are close in time (within 10ms)
            time_diff = abs((query2.timestamp - query1.timestamp).total_seconds())
            return time_diff < 0.01
        return False
    
    def _is_main_table_query(self, query: CapturedQuery) -> bool:
        """Check if this looks like a main table query (not a lookup)."""
        sql_upper = query.sql.upper()
        # Main queries typically don't have simple ID lookups
        return ('WHERE ID = ?' not in sql_upper and 
                'LIMIT 1' not in sql_upper and
                query.query_type == 'SELECT')
    
    def _find_related_queries_after(self, main_query: CapturedQuery) -> List[CapturedQuery]:
        """Find queries that might be fetching related objects after main query."""
        related = []
        main_index = self.queries.index(main_query)
        
        # Look at next 10 queries or 100ms window
        for i in range(main_index + 1, min(main_index + 10, len(self.queries))):
            query = self.queries[i]
            time_diff = (query.timestamp - main_query.timestamp).total_seconds()
            
            if time_diff > 0.1:  # 100ms window
                break
            
            # Check if it's a related lookup
            if query.query_type == 'SELECT' and self._looks_like_related_query(query):
                related.append(query)
        
        return related
    
    def _is_many_to_many_pattern(self, main_query: CapturedQuery, related_queries: List[CapturedQuery]) -> bool:
        """Check if queries match many-to-many pattern."""
        # Look for patterns like through tables or IN clauses
        for query in related_queries:
            sql_upper = query.sql.upper()
            if 'JOIN' in sql_upper or 'IN (' in sql_upper:
                return True
        return False
    
    def _looks_like_related_query(self, query: CapturedQuery) -> bool:
        """Check if query looks like it's fetching related objects."""
        sql_upper = query.sql.upper()
        return ('WHERE' in sql_upper and 
                ('_ID' in sql_upper or 'IN (' in sql_upper))
    
    def _is_count_only_pattern(self, query: CapturedQuery) -> bool:
        """Check if query result is only used for counting."""
        # This would need integration with code analysis
        # For now, check if it's selecting all fields without limit
        sql_upper = query.sql.upper()
        return ('SELECT *' in sql_upper or 
                'SELECT ' in sql_upper and 'FROM' in sql_upper and
                'LIMIT' not in sql_upper)
    
    def _fetches_all_fields(self, query: CapturedQuery) -> bool:
        """Check if query fetches all fields (SELECT *)."""
        return 'SELECT *' in query.sql.upper() or 'SELECT "' in query.sql
    
    def _estimate_field_count(self, query: CapturedQuery) -> int:
        """Estimate number of fields being fetched."""
        # Count commas in SELECT clause as rough estimate
        sql_upper = query.sql.upper()
        if 'SELECT *' in sql_upper:
            return 20  # Assume many fields
        
        select_end = sql_upper.find('FROM')
        if select_end > 0:
            select_clause = query.sql[:select_end]
            return select_clause.count(',') + 1
        
        return 5  # Default estimate
    
    def _has_specific_where_clause(self, query: CapturedQuery) -> bool:
        """Check if query has specific WHERE conditions."""
        sql_upper = query.sql.upper()
        if 'WHERE' not in sql_upper:
            return False
        
        # Check for specific conditions (not just IS NOT NULL, etc.)
        where_idx = sql_upper.find('WHERE')
        where_clause = sql_upper[where_idx:]
        
        # Look for equality or IN conditions
        return ('=' in where_clause or 'IN (' in where_clause)
    
    # Suggestion generators
    
    def _generate_n_plus_one_suggestion(self, parent_table: Optional[str], 
                                       related_table: Optional[str], 
                                       queries: List[CapturedQuery]) -> str:
        """Generate code suggestion for N+1 fix."""
        # Analyze the queries to determine the relationship
        sample_query = queries[0].sql.upper()
        
        if 'JOIN' in sample_query or len(queries[0].table_names) > 1:
            # Likely needs prefetch_related
            return "Model.objects.prefetch_related('related_field')"
        else:
            # Likely needs select_related
            field_hint = self._guess_field_name(queries)
            return f"Model.objects.select_related('{field_hint}')"
    
    def _guess_field_name(self, queries: List[CapturedQuery]) -> str:
        """Try to guess the field name from queries."""
        # Look for patterns like table_name_id
        for query in queries:
            match = re.search(r'WHERE\s+(\w+)_id\s*=', query.sql, re.IGNORECASE)
            if match:
                return match.group(1)
        return "related_field"
    
    def _generate_select_related_suggestion(self, query1: CapturedQuery, query2: CapturedQuery) -> str:
        """Generate select_related suggestion."""
        # Try to identify the field name
        field_name = self._guess_field_name([query2])
        return f"queryset.select_related('{field_name}')"
    
    def _generate_prefetch_related_suggestion(self, main_query: CapturedQuery, 
                                            related_queries: List[CapturedQuery]) -> str:
        """Generate prefetch_related suggestion."""
        # Try to identify the field name from table names
        if related_queries and related_queries[0].table_names:
            table = related_queries[0].table_names[0]
            # Convert table name to field name (simplified)
            field_name = table.rstrip('s')  # Remove plural 's'
            return f"queryset.prefetch_related('{field_name}_set')"
        
        return "queryset.prefetch_related('related_set')"
    
    # Enhanced context extraction methods
    
    def _extract_foreign_key_fields(self, queries: List[CapturedQuery]) -> List[str]:
        """Extract specific foreign key field names from queries."""
        fields = []
        for query in queries:
            # Look for WHERE clauses with foreign key patterns
            sql_upper = query.sql.upper()
            
            # Pattern: WHERE table_field_id = ?
            import re
            patterns = [
                r'WHERE\s+"?(\w+)"?\."?(\w+)_ID"?\s*=',  # table.field_id = 
                r'WHERE\s+"?(\w+_ID)"?\s*=',              # field_id =
                r'INNER\s+JOIN\s+"?(\w+)"?\s+ON.*"?(\w+)"?\."?(\w+)_ID"?'  # JOIN patterns
            ]
            
            for pattern in patterns:
                matches = re.findall(pattern, sql_upper, re.IGNORECASE)
                for match in matches:
                    if isinstance(match, tuple):
                        # Extract field name (remove _id suffix)
                        for part in match:
                            if part.endswith('_ID'):
                                field = part[:-3].lower()
                                if field not in fields:
                                    fields.append(field)
                    else:
                        if match.endswith('_ID'):
                            field = match[:-3].lower()
                            if field not in fields:
                                fields.append(field)
        
        # If no specific fields found, try to infer from table names
        if not fields:
            for query in queries[1:]:  # Skip first query
                for table in query.table_names:
                    if table not in [t for q in queries[:1] for t in q.table_names]:
                        # This is a related table, convert to field name
                        field = table.rstrip('s')  # Remove plural
                        if field not in fields:
                            fields.append(field)
        
        return fields or ['related_field']
    
    def _extract_code_locations(self, queries: List[CapturedQuery]) -> List[str]:
        """Extract hierarchical code locations with primary and secondary context."""
        primary_locations = []
        secondary_locations = []
        seen_locations = set()
        
        for query in queries:
            if query.stack_trace:
                # Process each frame from the hierarchical stack trace
                for i, frame_info in enumerate(query.stack_trace):
                    if ' in ' in frame_info:
                        location_part = frame_info.split(' in ')[0]
                        method_part = frame_info.split(' in ')[1].split(' (')[0]
                        
                        # Create readable location string
                        readable_location = self._format_location_string(location_part, method_part, frame_info)
                        
                        # Skip if already seen
                        location_key = location_part
                        if location_key in seen_locations:
                            continue
                        seen_locations.add(location_key)
                        
                        # Categorize as primary (first frame) or secondary (additional context)
                        if i == 0:  # First frame is most relevant
                            primary_locations.append(readable_location)
                        else:  # Additional frames provide context
                            secondary_locations.append(readable_location)
                    else:
                        # Handle simpler frame formats
                        if frame_info not in seen_locations:
                            seen_locations.add(frame_info)
                            if not primary_locations:  # If no primary yet, make this primary
                                primary_locations.append(frame_info)
                            else:
                                secondary_locations.append(frame_info)
        
        # Combine locations with clear hierarchy
        if primary_locations and secondary_locations:
            # Show primary + limited secondary for context
            result = primary_locations[:1]  # Primary location
            if len(secondary_locations) > 1:
                context_info = f"{secondary_locations[0]} (+{len(secondary_locations)-1} more)"
                result.append(context_info)
            else:
                result.extend(secondary_locations[:1])
            return result
        elif primary_locations:
            return primary_locations[:2]  # Show up to 2 primary locations
        elif secondary_locations:
            return secondary_locations[:2]  # Fallback to secondary
        else:
            return ["Unable to determine specific code location"]
    
    def _format_location_string(self, location_part: str, method_part: str, original_frame: str) -> str:
        """Format a location string with full path for better navigation."""
        if '/' in location_part and ':' in location_part:
            # Use full path instead of just filename for better navigation
            full_path_and_line = location_part  # Keep full path like "/path/to/file.py:25"
            
            # Enhance method context for Django admin
            if 'django/contrib/admin' in location_part:
                # For Django admin, show the admin component
                if 'changelist' in method_part.lower():
                    return f"{full_path_and_line} in {method_part} (Django Admin List)"
                elif 'change_view' in method_part.lower() or 'add_view' in method_part.lower():
                    return f"{full_path_and_line} in {method_part} (Django Admin Form)"
                else:
                    return f"{full_path_and_line} in {method_part} (Django Admin)"
            else:
                return f"{full_path_and_line} in {method_part}"
        else:
            return original_frame
    
    def _build_table_context(self, queries: List[CapturedQuery]) -> Dict[str, str]:
        """Build table to model name mapping context."""
        context = {}
        for query in queries:
            for table in query.table_names:
                # Convert table name to likely model name
                model_name = ''.join(word.capitalize() for word in table.split('_'))
                context[table] = model_name
        return context
    
    def _is_user_admin_code(self, file_path: str) -> bool:
        """Detect if this is user's custom admin code vs Django framework."""
        # User admin code patterns
        user_admin_patterns = [
            '/admin.py:',           # Direct admin file: /path/to/myapp/admin.py:25
            '/admin.py ',           # Admin file in method name
        ]
        
        # Django framework patterns to exclude
        framework_patterns = [
            'django/contrib/admin/',
            'site-packages/',
            'unfold/',              # Django Unfold admin theme
            'django/forms/',
        ]
        
        # Check if it's user admin code
        is_admin_file = any(pattern in file_path for pattern in user_admin_patterns)
        is_framework = any(pattern in file_path for pattern in framework_patterns)
        
        # Admin file but not framework = user's custom admin code
        return is_admin_file and not is_framework
    
    def _is_framework_code(self, file_path: str) -> bool:
        """Detect if this is Django framework internal code."""
        framework_patterns = [
            'django/contrib/admin/',
            'django/forms/',
            'django/views/generic/',
            'django/core/',
            'unfold/',
            'site-packages/',
        ]
        
        return any(pattern in file_path for pattern in framework_patterns)
    
    def _categorize_issue_location(self, code_locations: List[str]) -> tuple:
        """Categorize issue locations as user code vs framework code."""
        user_locations = []
        framework_locations = []
        
        for location in code_locations:
            # Extract file path from location string
            file_path = location.split(' in ')[0] if ' in ' in location else location
            
            if self._is_user_admin_code(file_path):
                # User's custom admin code - always show
                user_locations.append(location)
            elif self._is_framework_code(file_path):
                # Django framework internal code - optionally filter
                framework_locations.append(location)
            else:
                # Default to user code if unsure (better safe than sorry)
                user_locations.append(location)
        
        return user_locations, framework_locations
    
    def _detect_framework_type(self, framework_locations: List[str]) -> Optional[str]:
        """Detect the type of framework based on locations."""
        if not framework_locations:
            return None
            
        location_text = ' '.join(framework_locations).lower()
        
        if 'django/contrib/admin' in location_text or 'unfold/' in location_text:
            return 'django_admin'
        elif 'django/forms' in location_text:
            return 'django_forms'
        elif 'django/views' in location_text:
            return 'django_views'
        else:
            return 'django_framework'
    
    def _generate_categorized_recommendation(self, pattern_type: str, is_user_code: bool, 
                                          framework_type: Optional[str], specific_fields: List[str],
                                          query_count: int = 1, table_context: Dict[str, str] = None,
                                          code_locations: List[str] = None) -> tuple:
        """Generate appropriate recommendations based on responsibility, not location."""
        # For Django admin issues, the RESPONSIBILITY is always with the user
        # even if the query executes in framework code
        if framework_type == 'django_admin':
            # Django admin issues are USER RESPONSIBILITY - provide admin-specific configuration advice
            return self._generate_admin_recommendation(pattern_type, specific_fields, query_count, table_context, code_locations)
        elif is_user_code:
            # Direct user code - provide standard ORM optimization advice
            return self._generate_user_code_recommendation(pattern_type, specific_fields, query_count)
        else:
            # Other framework - provide general framework advice
            return self._generate_framework_recommendation(pattern_type, framework_type)
    
    def _generate_user_code_recommendation(self, pattern_type: str, specific_fields: List[str], 
                                         query_count: int) -> tuple:
        """Generate recommendations for user code issues."""
        if pattern_type == "n_plus_one":
            optimization_type = "select_related" if query_count < 10 else "prefetch_related"
            fields_str = ', '.join(f"'{f}'" for f in specific_fields)
            
            recommendation = f"Use {optimization_type}({fields_str}) to fetch related objects in a single query"
            code_suggestion = f"queryset.{optimization_type}({fields_str})"
            
            return recommendation, code_suggestion
        
        # Add more pattern types as needed
        return "Optimize this query in your code", "# Add optimization here"
    
    def _generate_admin_recommendation(self, pattern_type: str, specific_fields: List[str], 
                                     query_count: int, table_context: Dict[str, str] = None, 
                                     code_locations: List[str] = None) -> tuple:
        """Generate Django admin-specific recommendations with file path guidance."""
        # Try to infer the admin file path from table name
        admin_file_hint = self._infer_admin_file_path(table_context, code_locations)
        
        if pattern_type == "n_plus_one":
            recommendation = f"Django admin N+1 detected. {admin_file_hint}Add list_select_related or raw_id_fields to your ModelAdmin"
            
            if specific_fields:
                fields_str = ', '.join(f"'{f}'" for f in specific_fields)
                code_suggestion = f"class MyModelAdmin(admin.ModelAdmin):\n    list_select_related = ({fields_str},)\n    # or use raw_id_fields for large datasets"
            else:
                code_suggestion = "class MyModelAdmin(admin.ModelAdmin):\n    list_select_related = ('related_field',)\n    # or use raw_id_fields = ('related_field',)"
            
            return recommendation, code_suggestion
            
        elif pattern_type == "large_result_set":
            # Check if this is from admin filters (common pattern)
            if self._is_admin_filter_query(code_locations):
                recommendation = f"Django admin filter query detected. {admin_file_hint}Your list_filter fields are generating large result sets"
                code_suggestion = f"# In your admin class, optimize list_filter fields:\nclass MyModelAdmin(admin.ModelAdmin):\n    list_per_page = 25  # Reduce page size\n    list_max_show_all = 50  # Limit 'Show all' option\n    # Consider using autocomplete_fields for foreign keys:\n    # autocomplete_fields = ('related_field',)"
            else:
                recommendation = f"Django admin: Large result set detected. {admin_file_hint}Add pagination and filtering to your ModelAdmin"
                code_suggestion = "class MyModelAdmin(admin.ModelAdmin):\n    list_per_page = 25  # Default is 100\n    list_filter = ('status', 'created_at')\n    search_fields = ('name',)"
            
            return recommendation, code_suggestion
            
        elif pattern_type == "inefficient_count":
            # Detect if this is admin filter counting
            if self._is_admin_filter_query(code_locations):
                recommendation = f"Django admin filter counting detected. {admin_file_hint}Your list_filter is counting large datasets"
                code_suggestion = f"# In your admin class, optimize filter performance:\nclass MyModelAdmin(admin.ModelAdmin):\n    list_per_page = 25  # Reduce page size\n    show_full_result_count = False  # Disable expensive counting\n    # For foreign key filters, use autocomplete:\n    # autocomplete_fields = ('foreign_key_field',)\n    # Or limit filter choices:\n    # list_filter = ('status',)  # Remove heavy foreign key filters"
            else:
                recommendation = f"Django admin: Inefficient counting detected. {admin_file_hint}Configure admin pagination properly"
                code_suggestion = "class MyModelAdmin(admin.ModelAdmin):\n    list_per_page = 25  # Reduce page size for large tables\n    show_full_result_count = False  # Disable expensive counting\n    list_filter = ('status',)  # Add filtering"
                
            return recommendation, code_suggestion
            
        elif pattern_type == "inefficient_aggregations":
            recommendation = f"Django admin: Multiple aggregations detected. {admin_file_hint}This may be expected admin behavior"
            code_suggestion = "# Consider if you need custom admin views:\nclass MyModelAdmin(admin.ModelAdmin):\n    list_per_page = 25\n    # Admin aggregations are often internal - focus on pagination"
            
            return recommendation, code_suggestion
            
        elif pattern_type == "redundant_queries":
            recommendation = f"Django admin: Redundant queries detected. {admin_file_hint}Check if your admin configuration is optimized"
            code_suggestion = "class MyModelAdmin(admin.ModelAdmin):\n    list_select_related = ('related_field',)  # Optimize related fields\n    list_per_page = 25  # Reduce page size\n    show_full_result_count = False  # Reduce counting overhead"
            
            return recommendation, code_suggestion
        
        # Default admin recommendation
        return f"Django admin issue detected. {admin_file_hint}Optimize your ModelAdmin configuration", "# Add admin optimization"
    
    def _infer_admin_file_path(self, table_context: Dict[str, str] = None, code_locations: List[str] = None) -> str:
        """Infer the likely admin.py file path from table names."""
        if not table_context:
            return ""
        
        # Get the first table name and try to infer app name
        table_names = list(table_context.keys())
        if not table_names:
            return ""
        
        table_name = table_names[0]
        
        # Django table naming convention: app_model -> app/admin.py
        if '_' in table_name:
            app_name = table_name.split('_')[0]
            return f"Update your {app_name}/admin.py file: "
        
        return "Update your admin.py file: "
    
    def _is_admin_filter_query(self, code_locations: List[str] = None) -> bool:
        """Detect if this query is from Django admin filters."""
        if not code_locations:
            return False
        
        # Check for admin filter-specific stack traces
        admin_filter_patterns = [
            'admin/filters.py',
            'admin_list_filter',
            'choices',  # Filter choices method
            'admin/templatetags/admin_list.py',
        ]
        
        location_text = ' '.join(code_locations).lower()
        return any(pattern in location_text for pattern in admin_filter_patterns)
    
    def _is_legitimate_pagination_query(self, query: CapturedQuery) -> bool:
        """Detect if this is a legitimate pagination COUNT query that should be ignored."""
        sql_upper = query.sql.upper()
        
        # Check if it's a pagination COUNT query
        if 'COUNT(*)' not in sql_upper:
            return False
        
        # Check if it's from Django admin pagination (not filter counting)
        code_locations = self._extract_code_locations([query])
        location_text = ' '.join(code_locations).lower()
        
        # These are legitimate pagination patterns we should NOT flag as issues
        legitimate_patterns = [
            'get_results',  # Django admin pagination in views/main.py
            'changelist_view',  # Django admin list view
        ]
        
        # These are problematic filter patterns we SHOULD flag
        problematic_patterns = [
            'admin/filters.py',
            'choices',  # Filter choices counting
            'admin_list_filter',
        ]
        
        # If it's from problematic filter patterns, don't consider it legitimate
        if any(pattern in location_text for pattern in problematic_patterns):
            return False
        
        # If it's from legitimate pagination patterns, consider it legitimate
        return any(pattern in location_text for pattern in legitimate_patterns)
    
    def _generate_framework_recommendation(self, pattern_type: str, framework_type: Optional[str]) -> tuple:
        """Generate recommendations for other framework issues."""
        if framework_type == 'django_forms':
            recommendation = "Django forms framework issue detected. Consider form optimization"
            code_suggestion = "# Form optimization needed"
        else:
            recommendation = f"Django {framework_type or 'framework'} issue detected. This may be expected behavior"
            code_suggestion = "# Framework-level optimization may not be needed"
        
        return recommendation, code_suggestion
    
    def _generate_contextual_select_related_suggestion(self, query1: CapturedQuery, 
                                                     query2: CapturedQuery, 
                                                     specific_fields: List[str]) -> str:
        """Generate contextual select_related suggestion with specific fields."""
        if specific_fields and specific_fields != ['related_field']:
            fields_str = ', '.join(f"'{field}'" for field in specific_fields)
            return f"queryset.select_related({fields_str})"
        else:
            return "queryset.select_related('related_field')  # Update with actual field name"
    
    def _generate_enhanced_n_plus_one_suggestion(self, parent_table: Optional[str], 
                                               related_table: Optional[str], 
                                               queries: List[CapturedQuery],
                                               specific_fields: List[str]) -> str:
        """Generate enhanced N+1 suggestion with specific context."""
        if len(queries) < 10:
            # Use select_related for smaller sets
            if specific_fields and specific_fields != ['related_field']:
                fields_str = ', '.join(f"'{field}'" for field in specific_fields)
                return f"Model.objects.select_related({fields_str})"
            else:
                return "Model.objects.select_related('related_field')"
        else:
            # Use prefetch_related for larger sets
            if specific_fields and specific_fields != ['related_field']:
                fields_str = ', '.join(f"'{field}'" for field in specific_fields)
                return f"Model.objects.prefetch_related({fields_str})"
            else:
                return "Model.objects.prefetch_related('related_set')"
    
    def _extract_prefetch_fields(self, main_query: CapturedQuery, 
                               related_queries: List[CapturedQuery]) -> List[str]:
        """Extract specific field names for prefetch_related."""
        fields = []
        
        # Try to identify the relationship from table names
        main_tables = set(main_query.table_names)
        
        for query in related_queries:
            for table in query.table_names:
                if table not in main_tables:
                    # This is likely a related table
                    # Convert table name to field name
                    if table.endswith('_set') or '_' in table:
                        # Handle many-to-many through tables
                        field = table.replace('_', '')
                        if field.endswith('s'):
                            field = field[:-1] + '_set'
                    else:
                        # Simple case: table name to field name
                        field = table.rstrip('s') + '_set'
                    
                    if field not in fields:
                        fields.append(field)
        
        # If no fields found, extract from SQL patterns
        if not fields:
            for query in related_queries:
                # Look for JOIN or IN patterns that indicate relationships
                sql_upper = query.sql.upper()
                if 'JOIN' in sql_upper or 'IN (' in sql_upper:
                    # Try to extract related field name from WHERE clauses
                    import re
                    matches = re.findall(r'WHERE\s+"?(\w+)"?\."?(\w+_ID)"?', sql_upper)
                    for table, field_id in matches:
                        field = field_id[:-3].lower() + '_set'
                        if field not in fields:
                            fields.append(field)
        
        return fields or ['related_set']
    
    def _generate_contextual_prefetch_related_suggestion(self, main_query: CapturedQuery, 
                                                       related_queries: List[CapturedQuery],
                                                       specific_fields: List[str]) -> str:
        """Generate contextual prefetch_related suggestion."""
        if specific_fields and specific_fields != ['related_set']:
            fields_str = ', '.join(f"'{field}'" for field in specific_fields)
            return f"queryset.prefetch_related({fields_str})"
        else:
            return "queryset.prefetch_related('related_set')  # Update with actual field name"
    
    def _detect_subqueries_in_loops(self):
        """Detect subqueries being executed in loops."""
        # Look for patterns where similar queries with IN clauses have different parameter counts
        in_clause_queries = defaultdict(list)
        
        for query in self.queries:
            if 'IN (' in query.sql.upper():
                # Extract the base pattern without the IN clause contents
                base = re.sub(r'IN\s*\([^)]+\)', 'IN (?)', query.sql, flags=re.IGNORECASE)
                in_clause_queries[base].append(query)
        
        for pattern, queries in in_clause_queries.items():
            if len(queries) >= 3:
                # Check if the IN clauses have increasing sizes (sign of loop accumulation)
                param_counts = []
                for q in queries:
                    match = re.search(r'IN\s*\(([^)]+)\)', q.sql, re.IGNORECASE)
                    if match:
                        params = match.group(1).split(',')
                        param_counts.append(len(params))
                
                if param_counts and max(param_counts) > min(param_counts) * 2:
                    self.detected_patterns.append(DetectedPattern(
                        pattern_type="subqueries_in_loops",
                        severity="high",
                        description=f"Detected {len(queries)} subqueries likely executed in a loop",
                        affected_queries=queries,
                        recommendation="Consider fetching all data at once and filtering in Python, or use a single query with all IDs",
                        code_suggestion="# Instead of loop with queries, use:\nids = [item.id for item in items]\nresults = Model.objects.filter(id__in=ids)",
                        estimated_impact=f"Reduce {len(queries)} queries to 1",
                        code_locations=self._extract_code_locations(queries),
                        query_examples=[q.sql for q in queries[:2]]
                    ))
    
    def _detect_missing_database_indexes(self):
        """Detect queries on fields that likely need indexes."""
        # Look for WHERE clauses and ORDER BY on non-indexed fields
        for query in self.queries:
            if query.query_type != 'SELECT':
                continue
            
            sql_upper = query.sql.upper()
            
            # Check for WHERE clauses on non-id fields (likely need indexes)
            where_fields = re.findall(r'WHERE\s+["\']?(\w+)["\']?\s*=', sql_upper)
            order_fields = re.findall(r'ORDER\s+BY\s+["\']?(\w+)["\']?', sql_upper)
            
            fields_needing_index = []
            for field in where_fields + order_fields:
                # Skip if it's an ID field (usually indexed)
                if field.lower() not in ['id', 'pk'] and query.duration > 0.05:  # 50ms threshold
                    fields_needing_index.append(field.lower())
            
            if fields_needing_index:
                unique_fields = list(set(fields_needing_index))
                fields_str = ', '.join(f"'{f}'" for f in unique_fields)
                
                self.detected_patterns.append(DetectedPattern(
                    pattern_type="missing_database_index",
                    severity="high" if query.duration > 0.1 else "medium",
                    description=f"Slow query ({query.duration*1000:.1f}ms) on non-indexed fields: {fields_str}",
                    affected_queries=[query],
                    recommendation=f"Add database indexes on fields: {fields_str}",
                    code_suggestion=f"class Meta:\n    indexes = [\n        models.Index(fields=[{fields_str}]),\n    ]",
                    estimated_impact=f"Could improve query time by 10-100x",
                    specific_fields=unique_fields,
                    code_locations=self._extract_code_locations([query]),
                    query_examples=[query.sql]
                ))
    
    def _detect_inefficient_aggregations(self):
        """Detect inefficient aggregation patterns."""
        # Look for multiple COUNT, SUM, AVG queries that could be combined
        aggregation_queries = []
        
        for query in self.queries:
            sql_upper = query.sql.upper()
            if any(agg in sql_upper for agg in ['COUNT(', 'SUM(', 'AVG(', 'MAX(', 'MIN(']):
                aggregation_queries.append(query)
        
        # Group by table to find related aggregations
        table_aggregations = defaultdict(list)
        for query in aggregation_queries:
            if query.table_names:
                table_aggregations[query.table_names[0]].append(query)
        
        for table, queries in table_aggregations.items():
            if len(queries) >= 2:
                # Check if queries are close in time (likely related)
                time_diffs = []
                for i in range(len(queries) - 1):
                    diff = (queries[i+1].timestamp - queries[i].timestamp).total_seconds()
                    time_diffs.append(diff)
                
                if time_diffs and max(time_diffs) < 0.1:  # Within 100ms
                    code_locations = self._extract_code_locations(queries)
                    
                    # Categorize by RESPONSIBILITY, not location
                    user_locations, framework_locations = self._categorize_issue_location(code_locations)
                    framework_type = self._detect_framework_type(framework_locations)
                    is_user_responsibility = len(user_locations) > 0 or framework_type == 'django_admin'
                    
                    # Generate appropriate recommendations
                    recommendation, code_suggestion = self._generate_categorized_recommendation(
                        "inefficient_aggregations", is_user_responsibility, framework_type, [], len(queries), {table: table.title()}, code_locations
                    )
                    
                    self.detected_patterns.append(DetectedPattern(
                        pattern_type="inefficient_aggregations",
                        severity="medium",
                        description=f"Multiple aggregation queries on {table} could be combined",
                        affected_queries=queries,
                        recommendation=recommendation,
                        code_suggestion=code_suggestion,
                        estimated_impact=f"Reduce {len(queries)} queries to 1",
                        code_locations=code_locations,
                        table_context={table: table.title()},
                        query_examples=[q.sql for q in queries[:2]],
                        is_user_code=is_user_responsibility,
                        framework_type=framework_type
                    ))
    
    def _detect_missing_bulk_operations(self):
        """Detect opportunities for bulk_create, bulk_update, or bulk operations."""
        # Look for multiple INSERT or UPDATE queries in sequence
        insert_queries = []
        update_queries = []
        
        for query in self.queries:
            if query.query_type == 'INSERT':
                insert_queries.append(query)
            elif query.query_type == 'UPDATE':
                update_queries.append(query)
        
        # Check for bulk insert opportunities
        self._check_bulk_opportunity(insert_queries, 'INSERT', 'bulk_create')
        self._check_bulk_opportunity(update_queries, 'UPDATE', 'bulk_update')
    
    def _check_bulk_opportunity(self, queries: List[CapturedQuery], operation: str, bulk_method: str):
        """Check if queries could benefit from bulk operations."""
        if len(queries) < 3:
            return
        
        # Group by table
        table_queries = defaultdict(list)
        for query in queries:
            if query.table_names:
                table_queries[query.table_names[0]].append(query)
        
        for table, table_ops in table_queries.items():
            if len(table_ops) >= 3:
                # Check if operations are close in time
                time_span = (table_ops[-1].timestamp - table_ops[0].timestamp).total_seconds()
                if time_span < 1.0:  # Within 1 second
                    self.detected_patterns.append(DetectedPattern(
                        pattern_type="missing_bulk_operations",
                        severity="high" if len(table_ops) > 10 else "medium",
                        description=f"{len(table_ops)} individual {operation} operations on {table}",
                        affected_queries=table_ops,
                        recommendation=f"Use {bulk_method}() for batch operations",
                        code_suggestion=f"# Instead of individual {operation.lower()}s:\nobjects_to_{operation.lower()} = [...]\nModel.objects.{bulk_method}(objects_to_{operation.lower()})",
                        estimated_impact=f"Reduce {len(table_ops)} queries to 1, 10-100x faster",
                        code_locations=self._extract_code_locations(table_ops[:3]),
                        table_context={table: table.title()},
                        query_examples=[q.sql for q in table_ops[:2]]
                    ))
    
    def _detect_inefficient_exists_checks(self):
        """Detect inefficient existence checks using count() or len()."""
        for query in self.queries:
            sql_upper = query.sql.upper()
            
            # Look for COUNT(*) queries without other aggregations
            if 'COUNT(*)' in sql_upper and 'GROUP BY' not in sql_upper:
                # Check if this is likely used for existence check (count = 0 or > 0)
                if query.duration > 0.01:  # If it takes more than 10ms
                    self.detected_patterns.append(DetectedPattern(
                        pattern_type="inefficient_exists_check",
                        severity="low",
                        description="Using COUNT(*) for existence check is inefficient",
                        affected_queries=[query],
                        recommendation="Use .exists() instead of .count() for existence checks",
                        code_suggestion="# Instead of:\nif queryset.count() > 0:\n    ...\n\n# Use:\nif queryset.exists():\n    ...",
                        estimated_impact="Faster existence checks, especially for large tables",
                        code_locations=self._extract_code_locations([query]),
                        query_examples=[query.sql]
                    ))
    
    def _detect_missing_select_for_update(self):
        """Detect potential race conditions where select_for_update might be needed."""
        # Look for SELECT followed by UPDATE on same table
        for i in range(len(self.queries) - 1):
            if self.queries[i].query_type == 'SELECT' and self.queries[i+1].query_type == 'UPDATE':
                # Check if same table
                if (self.queries[i].table_names and self.queries[i+1].table_names and
                    self.queries[i].table_names[0] == self.queries[i+1].table_names[0]):
                    
                    time_diff = (self.queries[i+1].timestamp - self.queries[i].timestamp).total_seconds()
                    if time_diff < 0.1:  # Within 100ms
                        table = self.queries[i].table_names[0]
                        self.detected_patterns.append(DetectedPattern(
                            pattern_type="missing_select_for_update",
                            severity="medium",
                            description=f"SELECT followed by UPDATE on {table} - potential race condition",
                            affected_queries=[self.queries[i], self.queries[i+1]],
                            recommendation="Use select_for_update() to prevent race conditions",
                            code_suggestion="with transaction.atomic():\n    obj = Model.objects.select_for_update().get(id=pk)\n    obj.field = new_value\n    obj.save()",
                            estimated_impact="Prevent race conditions and data inconsistencies",
                            code_locations=self._extract_code_locations([self.queries[i], self.queries[i+1]]),
                            table_context={table: table.title()},
                            query_examples=[self.queries[i].sql, self.queries[i+1].sql]
                        ))
    
    def _detect_transaction_issues(self):
        """Detect potential transaction-related issues."""
        # Look for large numbers of queries without transaction boundaries
        query_burst_threshold = 10
        time_window = 0.5  # 500ms
        
        for i in range(len(self.queries) - query_burst_threshold):
            window_queries = self.queries[i:i+query_burst_threshold]
            time_span = (window_queries[-1].timestamp - window_queries[0].timestamp).total_seconds()
            
            if time_span < time_window:
                # Check if these are write operations
                write_ops = [q for q in window_queries if q.query_type in ['INSERT', 'UPDATE', 'DELETE']]
                
                if len(write_ops) >= 5:
                    self.detected_patterns.append(DetectedPattern(
                        pattern_type="transaction_issues",
                        severity="high",
                        description=f"{len(write_ops)} write operations without explicit transaction",
                        affected_queries=write_ops,
                        recommendation="Wrap related operations in a transaction for consistency",
                        code_suggestion="from django.db import transaction\n\nwith transaction.atomic():\n    # Your database operations here\n    ...",
                        estimated_impact="Ensure data consistency and potentially improve performance",
                        code_locations=self._extract_code_locations(write_ops[:3]),
                        query_examples=[q.sql for q in write_ops[:2]]
                    ))
                    break
    
    def _detect_connection_pool_exhaustion(self):
        """Detect patterns that might exhaust connection pool."""
        # Look for rapid-fire queries that might indicate connection pool issues
        rapid_queries = []
        
        for i in range(len(self.queries) - 1):
            time_diff = (self.queries[i+1].timestamp - self.queries[i].timestamp).total_seconds()
            if time_diff < 0.001:  # Less than 1ms between queries
                rapid_queries.append(self.queries[i])
        
        if len(rapid_queries) > 20:
            self.detected_patterns.append(DetectedPattern(
                pattern_type="connection_pool_risk",
                severity="high",
                description=f"{len(rapid_queries)} queries executing rapidly - risk of connection pool exhaustion",
                affected_queries=rapid_queries[:10],  # Sample
                recommendation="Consider connection pooling configuration and query batching",
                code_suggestion="# In settings.py:\nDATABASES = {\n    'default': {\n        'CONN_MAX_AGE': 600,  # Persistent connections\n        'OPTIONS': {\n            'MAX_CONNS': 50,  # Max pool size\n        }\n    }\n}",
                estimated_impact="Prevent connection pool exhaustion and improve stability",
                code_locations=self._extract_code_locations(rapid_queries[:3])
            ))
    
    def _detect_inefficient_distinct(self):
        """Detect inefficient use of DISTINCT."""
        for query in self.queries:
            if 'DISTINCT' in query.sql.upper():
                # Check if DISTINCT is on all columns (SELECT DISTINCT *)
                if 'SELECT DISTINCT' in query.sql.upper() and query.duration > 0.05:
                    self.detected_patterns.append(DetectedPattern(
                        pattern_type="inefficient_distinct",
                        severity="medium",
                        description="DISTINCT on all columns is inefficient",
                        affected_queries=[query],
                        recommendation="Use distinct() on specific fields or reconsider query design",
                        code_suggestion="# Use distinct on specific fields:\nqueryset.values('field1', 'field2').distinct()\n\n# Or use annotation:\nqueryset.annotate(...).distinct('specific_field')",
                        estimated_impact="Reduce processing time and memory usage",
                        code_locations=self._extract_code_locations([query]),
                        query_examples=[query.sql]
                    ))
    
    def _detect_missing_values_values_list(self):
        """Detect when values() or values_list() would be more efficient."""
        # Look for SELECT queries fetching many columns when only a few might be needed
        for query in self.queries:
            if query.query_type == 'SELECT':
                # Count the number of fields being selected
                field_count = self._estimate_field_count(query)
                
                # If selecting many fields but query pattern suggests simple usage
                if field_count > 5 and 'JOIN' not in query.sql.upper():
                    # Check if this looks like it's being used for simple data extraction
                    if any(pattern in query.sql.upper() for pattern in ['LIMIT 1000', 'LIMIT 100', 'ORDER BY']):
                        self.detected_patterns.append(DetectedPattern(
                            pattern_type="missing_values_optimization",
                            severity="low",
                            description=f"Fetching {field_count}+ fields when you might only need a few",
                            affected_queries=[query],
                            recommendation="Use values() or values_list() for better performance",
                            code_suggestion="# If you only need specific fields:\nqueryset.values('id', 'name', 'email')\n\n# For single field:\nqueryset.values_list('name', flat=True)",
                            estimated_impact="Reduce memory usage and data transfer",
                            code_locations=self._extract_code_locations([query]),
                            query_examples=[query.sql]
                        ))
    
    def _detect_redundant_queries(self):
        """Detect completely redundant/duplicate queries."""
        # Find exact duplicate queries
        duplicates = self.get_duplicate_queries()
        
        for sql, queries in duplicates.items():
            if len(queries) >= 2:
                # Check if these are close in time (likely redundant)
                time_span = (queries[-1].timestamp - queries[0].timestamp).total_seconds()
                
                if time_span < 1.0:  # Within 1 second
                    code_locations = self._extract_code_locations(queries)
                    
                    # Categorize by RESPONSIBILITY, not location
                    user_locations, framework_locations = self._categorize_issue_location(code_locations)
                    framework_type = self._detect_framework_type(framework_locations)
                    is_user_responsibility = len(user_locations) > 0 or framework_type == 'django_admin'
                    
                    # Generate appropriate recommendations
                    recommendation, code_suggestion = self._generate_categorized_recommendation(
                        "redundant_queries", is_user_responsibility, framework_type, [], len(queries), {}, code_locations
                    )
                    
                    self.detected_patterns.append(DetectedPattern(
                        pattern_type="redundant_queries",
                        severity="medium",
                        description=f"Same query executed {len(queries)} times",
                        affected_queries=queries,
                        recommendation=recommendation,
                        code_suggestion=code_suggestion,
                        estimated_impact=f"Eliminate {len(queries)-1} redundant queries",
                        code_locations=code_locations,
                        query_examples=[queries[0].sql],
                        is_user_code=is_user_responsibility,
                        framework_type=framework_type
                    ))
    
    def _detect_missing_query_caching(self):
        """Detect queries that would benefit from caching."""
        # Look for expensive queries that are repeated
        expensive_queries = defaultdict(list)
        
        for query in self.queries:
            if query.duration > 0.1:  # 100ms threshold
                base_pattern = query.get_base_query()
                expensive_queries[base_pattern].append(query)
        
        for pattern, queries in expensive_queries.items():
            if len(queries) >= 2:
                total_time = sum(q.duration for q in queries)
                self.detected_patterns.append(DetectedPattern(
                    pattern_type="missing_query_caching",
                    severity="high",
                    description=f"Expensive query repeated {len(queries)} times (total {total_time*1000:.1f}ms)",
                    affected_queries=queries,
                    recommendation="Implement query result caching",
                    code_suggestion="from django.core.cache import cache\n\ncache_key = f'expensive_query_{params}'\nresult = cache.get(cache_key)\nif result is None:\n    result = expensive_queryset.all()\n    cache.set(cache_key, result, timeout=300)  # Cache for 5 minutes",
                    estimated_impact=f"Save {(total_time - queries[0].duration)*1000:.1f}ms per request",
                    code_locations=self._extract_code_locations(queries),
                    query_examples=[queries[0].sql]
                ))
    
    def get_duplicate_queries(self) -> Dict[str, List[CapturedQuery]]:
        """Get completely duplicate queries."""
        seen = {}
        duplicates = defaultdict(list)
        
        for query in self.queries:
            sql_normalized = query.sql.strip()
            if sql_normalized in seen:
                if sql_normalized not in duplicates:
                    duplicates[sql_normalized] = [seen[sql_normalized]]
                duplicates[sql_normalized].append(query)
            else:
                seen[sql_normalized] = query
        
        return dict(duplicates)
    
    def _extract_commonly_used_fields(self, query: CapturedQuery) -> List[str]:
        """Extract commonly used fields that should be included in only()."""
        # Common fields that are usually needed
        common_fields = ['id', 'name', 'title', 'slug', 'status', 'created_at', 'updated_at']
        
        # Try to extract field names from the SQL SELECT clause
        sql_upper = query.sql.upper()
        actual_fields = []
        
        # Look for explicit field selections in the SQL
        import re
        
        # Pattern to match field selections like "table"."field"
        field_matches = re.findall(r'"[^"]*"\."([^"]*)"', query.sql)
        for field in field_matches:
            if field and field.lower() not in ['id'] and len(field) < 50:  # Reasonable field name
                clean_field = field.lower().replace('_', '_')
                if clean_field not in actual_fields:
                    actual_fields.append(clean_field)
        
        # If we found actual fields, use a smart subset
        if actual_fields:
            # Always include id, then add other fields up to a reasonable limit
            result_fields = ['id']
            
            # Add common fields that exist in the actual fields
            for field in common_fields[1:]:  # Skip 'id' since we already added it
                if field in actual_fields and field not in result_fields:
                    result_fields.append(field)
                if len(result_fields) >= 5:  # Limit to 5 fields
                    break
            
            # If we still have room, add other actual fields
            for field in actual_fields:
                if field not in result_fields and len(result_fields) < 5:
                    result_fields.append(field)
            
            return result_fields
        else:
            # Fallback to common fields
            return common_fields[:4]  # Return first 4 common fields
    
    def _truncate_sql(self, sql: str, max_length: int = 200) -> str:
        """Truncate SQL for display in logs."""
        # Clean up whitespace
        sql = ' '.join(sql.split())
        
        if len(sql) <= max_length:
            return sql
        
        # Try to truncate at a meaningful boundary
        truncated = sql[:max_length]
        
        # Find last complete word/clause
        last_space = truncated.rfind(' ')
        if last_space > max_length * 0.8:  # If we have a reasonable breaking point
            truncated = truncated[:last_space]
        
        return truncated + "..."