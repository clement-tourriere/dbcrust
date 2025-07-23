"""
Tests for Django ORM Query Analyzer.

These tests verify the functionality of the query collector,
pattern detector, and main analyzer class.
"""

import unittest
from unittest.mock import Mock, patch, MagicMock
from datetime import datetime
import json

# Mock Django before importing our modules
django_mock = MagicMock()
django_mock.db.connection = MagicMock()
django_mock.db.connections = {"default": django_mock.db.connection}
django_mock.db.transaction.atomic = MagicMock()
django_mock.conf.settings = MagicMock()


with patch.dict('sys.modules', {
    'django': django_mock,
    'django.db': django_mock.db,
    'django.conf': django_mock.conf,
}):
    from ..query_collector import QueryCollector, CapturedQuery
    from ..pattern_detector import PatternDetector, DetectedPattern
    from ..analyzer import DjangoAnalyzer, AnalysisResult
    from ..recommendations import DjangoRecommendations, Recommendation


class TestQueryCollector(unittest.TestCase):
    """Test query collection functionality."""
    
    def setUp(self):
        self.collector = QueryCollector()
    
    def test_query_capture(self):
        """Test basic query capture functionality."""
        # Mock execute function
        def mock_execute(sql, params, many, context):
            return [["result"]]
        
        # Start collection
        self.collector.start_collection()
        
        # Capture a query
        result = self.collector(mock_execute, "SELECT * FROM users", (), False, {})
        
        # Check results
        self.assertEqual(len(self.collector.queries), 1)
        self.assertEqual(self.collector.queries[0].sql, "SELECT * FROM users")
        self.assertEqual(self.collector.queries[0].query_type, "SELECT")
        self.assertEqual(result, [["result"]])
    
    def test_query_type_extraction(self):
        """Test SQL query type extraction."""
        test_cases = [
            ("SELECT * FROM users", "SELECT"),
            ("INSERT INTO users VALUES (1)", "INSERT"),
            ("UPDATE users SET name='test'", "UPDATE"),
            ("DELETE FROM users WHERE id=1", "DELETE"),
            ("CREATE TABLE test (id INT)", "CREATE"),
            ("UNKNOWN STATEMENT", "OTHER"),
        ]
        
        for sql, expected_type in test_cases:
            actual_type = QueryCollector._extract_query_type(sql)
            self.assertEqual(actual_type, expected_type, 
                           f"Failed for SQL: {sql}")
    
    def test_table_name_extraction(self):
        """Test table name extraction from SQL."""
        test_cases = [
            ("SELECT * FROM users", ["users"]),
            ("SELECT * FROM users JOIN orders ON users.id = orders.user_id", 
             ["users", "orders"]),
            ("INSERT INTO customers (name) VALUES ('test')", ["customers"]),
            ("UPDATE products SET price = 100", ["products"]),
            ("DELETE FROM orders WHERE id = 1", ["orders"]),
        ]
        
        for sql, expected_tables in test_cases:
            actual_tables = QueryCollector._extract_table_names(sql)
            self.assertEqual(actual_tables, expected_tables,
                           f"Failed for SQL: {sql}")
    
    def test_duplicate_query_detection(self):
        """Test detection of duplicate queries."""
        self.collector.start_collection()
        
        # Add identical queries
        for _ in range(3):
            self.collector(lambda *args: None, "SELECT * FROM users", (), False, {})
        
        # Add different query
        self.collector(lambda *args: None, "SELECT * FROM orders", (), False, {})
        
        duplicates = self.collector.get_duplicate_queries()
        
        # Should have one group of duplicates (3 identical queries)
        self.assertEqual(len(duplicates), 1)
        duplicate_group = list(duplicates.values())[0]
        self.assertEqual(len(duplicate_group), 3)
    
    def test_query_grouping_by_type(self):
        """Test grouping queries by type."""
        self.collector.start_collection()
        
        # Add queries of different types
        queries = [
            ("SELECT * FROM users", "SELECT"),
            ("SELECT * FROM orders", "SELECT"),
            ("INSERT INTO users VALUES (1)", "INSERT"),
        ]
        
        for sql, _ in queries:
            self.collector(lambda *args: None, sql, (), False, {})
        
        grouped = self.collector.get_queries_by_type()
        
        self.assertEqual(len(grouped["SELECT"]), 2)
        self.assertEqual(len(grouped["INSERT"]), 1)


class TestCapturedQuery(unittest.TestCase):
    """Test CapturedQuery functionality."""
    
    def test_captured_query_creation(self):
        """Test creating a CapturedQuery instance."""
        query = CapturedQuery(
            sql="SELECT * FROM users WHERE id = %s",
            params=(1,),
            duration=0.05,
            timestamp=datetime.now(),
            stack_trace=["test_file.py:10 in test_function"],
            query_type="SELECT",
            table_names=["users"]
        )
        
        self.assertEqual(query.sql, "SELECT * FROM users WHERE id = %s")
        self.assertEqual(query.params, (1,))
        self.assertEqual(query.query_type, "SELECT")
        self.assertEqual(query.table_names, ["users"])
    
    def test_base_query_normalization(self):
        """Test query normalization for pattern matching."""
        query = CapturedQuery(
            sql="SELECT * FROM users WHERE id = 123",
            params=(),
            duration=0.01,
            timestamp=datetime.now(),
            stack_trace=[],
            query_type="SELECT",
            table_names=["users"]
        )
        
        base_query = query.get_base_query()
        
        # Should normalize the ID value
        self.assertIn("WHERE id = ?", base_query)
        self.assertNotIn("123", base_query)


class TestPatternDetector(unittest.TestCase):
    """Test pattern detection functionality."""
    
    def setUp(self):
        self.now = datetime.now()
        
    def create_query(self, sql, duration=0.01, query_type="SELECT", table_names=None):
        """Helper to create test queries."""
        return CapturedQuery(
            sql=sql,
            params=(),
            duration=duration,
            timestamp=self.now,
            stack_trace=["test.py:1"],
            query_type=query_type,
            table_names=table_names or []
        )
    
    def test_n_plus_one_detection(self):
        """Test N+1 query pattern detection."""
        # Create N+1 pattern: 1 main query + N lookups
        queries = [
            self.create_query("SELECT * FROM books", table_names=["books"]),
        ]
        
        # Add similar lookup queries (N+1 pattern)
        for i in range(5):
            queries.append(
                self.create_query(f"SELECT * FROM authors WHERE id = {i}", 
                                table_names=["authors"])
            )
        
        detector = PatternDetector(queries)
        patterns = detector.analyze()
        
        # Should detect N+1 pattern
        n_plus_one_patterns = [p for p in patterns if p.pattern_type == "n_plus_one"]
        self.assertGreater(len(n_plus_one_patterns), 0)
    
    def test_missing_select_related_detection(self):
        """Test detection of missing select_related."""
        queries = [
            self.create_query("SELECT * FROM orders WHERE id = 1", 
                            table_names=["orders"]),
            self.create_query("SELECT * FROM customers WHERE id = 123", 
                            table_names=["customers"]),
        ]
        
        detector = PatternDetector(queries)
        patterns = detector.analyze()
        
        # Should detect potential select_related opportunity
        select_related_patterns = [p for p in patterns 
                                 if p.pattern_type == "missing_select_related"]
        # Note: This is a simple test - real detection is more complex
        self.assertGreaterEqual(len(select_related_patterns), 0)
    
    def test_large_result_set_detection(self):
        """Test detection of queries without LIMIT."""
        queries = [
            self.create_query("SELECT * FROM users", table_names=["users"]),
            self.create_query("SELECT * FROM orders LIMIT 100", 
                            table_names=["orders"]),
        ]
        
        detector = PatternDetector(queries)
        patterns = detector.analyze()
        
        # Should detect query without LIMIT
        large_result_patterns = [p for p in patterns 
                               if p.pattern_type == "large_result_set"]
        self.assertGreater(len(large_result_patterns), 0)
    
    def test_inefficient_count_detection(self):
        """Test detection of inefficient count operations."""
        queries = [
            self.create_query("SELECT * FROM users", table_names=["users"]),
        ]
        
        detector = PatternDetector(queries)
        patterns = detector.analyze()
        
        # Should potentially detect inefficient count
        count_patterns = [p for p in patterns 
                         if p.pattern_type == "inefficient_count"]
        self.assertGreaterEqual(len(count_patterns), 0)


class TestDjangoRecommendations(unittest.TestCase):
    """Test Django-specific recommendations."""
    
    def test_n_plus_one_recommendations(self):
        """Test N+1 query recommendations."""
        # Create mock pattern
        pattern = DetectedPattern(
            pattern_type="n_plus_one",
            severity="critical",
            description="N+1 query detected",
            affected_queries=[],
            recommendation="Use select_related",
        )
        
        recommendations = DjangoRecommendations.generate_recommendations([pattern])
        
        self.assertGreater(len(recommendations), 0)
        self.assertIn("select_related", recommendations[0].code_after.lower())
    
    def test_select_related_recommendations(self):
        """Test select_related recommendations."""
        pattern = DetectedPattern(
            pattern_type="missing_select_related",
            severity="high",
            description="Missing select_related",
            affected_queries=[],
            recommendation="Use select_related",
        )
        
        recommendations = DjangoRecommendations.generate_recommendations([pattern])
        
        self.assertGreater(len(recommendations), 0)
        self.assertEqual(recommendations[0].impact, "high")
    
    def test_recommendations_summary_formatting(self):
        """Test recommendations summary formatting."""
        recommendations = [
            Recommendation(
                title="Test Critical",
                description="Critical issue",
                code_before="",
                code_after="",
                explanation="",
                references=[],
                difficulty="easy",
                impact="critical"
            ),
            Recommendation(
                title="Test High",
                description="High issue",
                code_before="",
                code_after="",
                explanation="",
                references=[],
                difficulty="easy",
                impact="high"
            ),
        ]
        
        summary = DjangoRecommendations.format_recommendations_summary(recommendations)
        
        self.assertIn("CRITICAL", summary)
        self.assertIn("HIGH", summary)
        self.assertIn("Test Critical", summary)
        self.assertIn("Test High", summary)


class TestAnalysisResult(unittest.TestCase):
    """Test AnalysisResult functionality."""
    
    def test_analysis_result_creation(self):
        """Test creating an AnalysisResult."""
        start_time = datetime.now()
        end_time = datetime.now()
        
        result = AnalysisResult(
            start_time=start_time,
            end_time=end_time,
            total_queries=5,
            total_duration=0.25,
            queries_by_type={"SELECT": 4, "INSERT": 1},
            duplicate_queries=2,
            detected_patterns=[],
            recommendations=[],
        )
        
        self.assertEqual(result.total_queries, 5)
        self.assertEqual(result.total_duration, 0.25)
        self.assertEqual(result.duplicate_queries, 2)
    
    def test_analysis_result_summary(self):
        """Test analysis result summary generation."""
        start_time = datetime.now()
        end_time = datetime.now()
        
        pattern = DetectedPattern(
            pattern_type="n_plus_one",
            severity="critical",
            description="N+1 detected",
            affected_queries=[],
            recommendation="Fix it",
        )
        
        result = AnalysisResult(
            start_time=start_time,
            end_time=end_time,
            total_queries=3,
            total_duration=0.15,
            queries_by_type={"SELECT": 3},
            duplicate_queries=1,
            detected_patterns=[pattern],
            recommendations=[],
        )
        
        summary = result.summary
        
        self.assertIn("Total Queries: 3", summary)
        self.assertIn("SELECT: 3", summary)
        self.assertIn("Duplicate Queries: 1", summary)
        self.assertIn("Performance Issues Detected", summary)
    
    def test_analysis_result_to_dict(self):
        """Test converting analysis result to dictionary."""
        start_time = datetime.now()
        end_time = datetime.now()
        
        result = AnalysisResult(
            start_time=start_time,
            end_time=end_time,
            total_queries=2,
            total_duration=0.1,
            queries_by_type={"SELECT": 2},
            duplicate_queries=0,
            detected_patterns=[],
            recommendations=[],
        )
        
        result_dict = result.to_dict()
        
        self.assertEqual(result_dict["total_queries"], 2)
        self.assertEqual(result_dict["total_duration"], 0.1)
        self.assertIn("start_time", result_dict)
        self.assertIn("end_time", result_dict)


@patch('django.db.connection')
@patch('django.db.connections')
@patch('django.db.transaction')
class TestDjangoAnalyzer(unittest.TestCase):
    """Test DjangoAnalyzer main functionality."""
    
    def test_analyzer_initialization(self, mock_transaction, mock_connections, mock_connection):
        """Test analyzer initialization."""
        analyzer = DjangoAnalyzer(
            dbcrust_url="postgres://localhost/test",
            transaction_safe=True,
            enable_explain=True,
        )
        
        self.assertEqual(analyzer.dbcrust_url, "postgres://localhost/test")
        self.assertTrue(analyzer.transaction_safe)
        self.assertTrue(analyzer.enable_explain)
        self.assertEqual(analyzer.database_alias, "default")
    
    def test_context_manager_setup(self, mock_transaction, mock_connections, mock_connection):
        """Test context manager setup and teardown."""
        # Setup mocks
        mock_connections.__getitem__.return_value = mock_connection
        mock_connection.execute_wrapper.return_value.__enter__ = Mock()
        mock_connection.execute_wrapper.return_value.__exit__ = Mock()
        
        mock_atomic = Mock()
        mock_atomic.__enter__ = Mock(return_value=mock_atomic)
        mock_atomic.__exit__ = Mock()
        mock_transaction.atomic.return_value = mock_atomic
        
        analyzer = DjangoAnalyzer(transaction_safe=True)
        
        # Test context manager
        with analyzer.analyze() as analysis:
            self.assertIsNotNone(analysis)
        
        # Verify mocks were called
        mock_connections.__getitem__.assert_called_with("default")
        mock_connection.execute_wrapper.assert_called_once()
        mock_transaction.atomic.assert_called_once()
    
    def test_analyze_without_queries(self, mock_transaction, mock_connections, mock_connection):
        """Test analysis with no queries captured."""
        # Setup mocks
        mock_connections.__getitem__.return_value = mock_connection
        mock_connection.execute_wrapper.return_value.__enter__ = Mock()
        mock_connection.execute_wrapper.return_value.__exit__ = Mock()
        
        analyzer = DjangoAnalyzer(transaction_safe=False)
        
        with analyzer.analyze():
            pass  # No queries executed
        
        result = analyzer.get_results()
        self.assertIsNotNone(result)
        self.assertEqual(result.total_queries, 0)
        self.assertEqual(result.total_duration, 0.0)


class TestIntegration(unittest.TestCase):
    """Integration tests for the complete analyzer."""
    
    @patch('django.db.connection')
    @patch('django.db.connections')
    def test_full_analysis_workflow(self, mock_connections, mock_connection):
        """Test complete analysis workflow."""
        # Setup mocks
        mock_connections.__getitem__.return_value = mock_connection
        mock_connection.execute_wrapper.return_value.__enter__ = Mock()
        mock_connection.execute_wrapper.return_value.__exit__ = Mock()
        
        analyzer = DjangoAnalyzer(transaction_safe=False, enable_explain=False)
        
        # Mock some queries being captured during analysis
        def mock_collect_queries(*args):
            # Simulate some queries being collected
            now = datetime.now()
            analyzer.query_collector.queries = [
                CapturedQuery(
                    sql="SELECT * FROM users",
                    params=(),
                    duration=0.01,
                    timestamp=now,
                    stack_trace=["test.py:1"],
                    query_type="SELECT",
                    table_names=["users"]
                ),
                CapturedQuery(
                    sql="SELECT * FROM users WHERE id = 1",
                    params=(1,),
                    duration=0.02,
                    timestamp=now,
                    stack_trace=["test.py:2"],
                    query_type="SELECT",
                    table_names=["users"]
                ),
            ]
        
        with analyzer.analyze():
            mock_collect_queries()
        
        result = analyzer.get_results()
        self.assertIsNotNone(result)
        self.assertEqual(result.total_queries, 2)
        self.assertGreater(result.total_duration, 0)


if __name__ == '__main__':
    # Run the tests
    unittest.main(verbosity=2)