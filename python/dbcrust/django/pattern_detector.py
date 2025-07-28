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
                
                # Determine if this needs select_related or prefetch_related
                optimization_type = "select_related" if len(queries) < 10 else "prefetch_related"
                
                # Format field suggestions without backslashes in f-string
                field_suggestions = ', '.join(f"'{f}'" for f in specific_fields)
                
                self.detected_patterns.append(DetectedPattern(
                    pattern_type="n_plus_one",
                    severity="critical",
                    description=f"N+1 query pattern: {len(queries)} queries on {related_table or 'related table'} - use {optimization_type}({field_suggestions})",
                    affected_queries=queries,
                    recommendation=f"Use {optimization_type}({field_suggestions}) to fetch related objects in a single query",
                    code_suggestion=self._generate_enhanced_n_plus_one_suggestion(parent_table, related_table, queries, specific_fields),
                    estimated_impact=f"Reduce {len(queries)} queries to 1-2 queries",
                    specific_fields=specific_fields,
                    code_locations=code_locations,
                    table_context=table_context,
                    query_examples=query_examples
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
                    self.detected_patterns.append(DetectedPattern(
                        pattern_type="inefficient_count",
                        severity="medium",
                        description="Fetching all rows when only count is needed",
                        affected_queries=[query],
                        recommendation="Use .count() instead of len(queryset) or queryset.all()",
                        code_suggestion="queryset.count()  # Instead of len(queryset.all())",
                        estimated_impact="Reduce memory usage and query time"
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
                
                self.detected_patterns.append(DetectedPattern(
                    pattern_type="large_result_set",
                    severity="medium",
                    description="Query without LIMIT that might return large result set",
                    affected_queries=[query],
                    recommendation="Consider using pagination or limiting results",
                    code_suggestion="queryset[:100]  # or use pagination",
                    estimated_impact="Prevent memory issues with large datasets"
                ))
    
    def _detect_unnecessary_ordering(self):
        """Detect unnecessary ORDER BY clauses."""
        order_queries = [q for q in self.queries if 'ORDER BY' in q.sql.upper()]
        
        for query in order_queries:
            # Check if ordering is used without LIMIT (might be unnecessary)
            if 'LIMIT' not in query.sql.upper() and query.duration > 0.1:  # 100ms threshold
                self.detected_patterns.append(DetectedPattern(
                    pattern_type="unnecessary_ordering",
                    severity="low",
                    description="ORDER BY without LIMIT might be unnecessary",
                    affected_queries=[query],
                    recommendation="Remove ordering if not needed, or add index for ordered field",
                    estimated_impact="Reduce query execution time"
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
        """Extract code locations from stack traces."""
        locations = []
        for query in queries:
            if query.stack_trace:
                # Find the most relevant frame (skip Django internals)
                for frame in reversed(query.stack_trace):
                    if not any(skip in frame for skip in [
                        'django/', 'site-packages/', '__pycache__',
                        'dbcrust/django/', 'query_collector.py'
                    ]):
                        # Extract filename and line number
                        if ':' in frame:
                            location = frame.split(' in ')[0]  # Get "file:line" part
                            if location not in locations:
                                locations.append(location)
                        break
        return locations
    
    def _build_table_context(self, queries: List[CapturedQuery]) -> Dict[str, str]:
        """Build table to model name mapping context."""
        context = {}
        for query in queries:
            for table in query.table_names:
                # Convert table name to likely model name
                model_name = ''.join(word.capitalize() for word in table.split('_'))
                context[table] = model_name
        return context
    
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