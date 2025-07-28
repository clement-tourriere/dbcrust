#!/usr/bin/env python3
"""
Test script to demonstrate the enhanced Django analyzer with specific recommendations.

This simulates the type of queries your temp.py example would generate, showing
how the enhanced analyzer provides actionable, specific recommendations.
"""

import os
import sys
from datetime import datetime
from typing import List

# Add the parent directory to the path so we can import dbcrust modules
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from dbcrust.django.query_collector import CapturedQuery
from dbcrust.django.pattern_detector import PatternDetector
from dbcrust.django.analyzer import AnalysisResult
from dbcrust.django.recommendations import DjangoRecommendations


def create_test_queries() -> List[CapturedQuery]:
    """Create sample queries that would trigger the performance patterns."""
    queries = []
    base_timestamp = datetime.now()
    
    # Simulate the membership query from your example
    queries.append(CapturedQuery(
        sql='SELECT "accounts_membership"."id", "accounts_membership"."user_id", "accounts_membership"."account_id" FROM "accounts_membership" WHERE ("accounts_membership"."account_id" = %s AND "accounts_membership"."user_id" IN (SELECT U0."id" FROM "auth_user" U0 WHERE U0."email" LIKE %s)) LIMIT 1',
        params=(1, '%toto%'),
        duration=0.015,
        timestamp=base_timestamp,
        stack_trace=[
            '/path/to/your/project/temp.py:15 in <module>',
            '/path/to/your/project/accounts/core/models/membership.py:45 in filter',
            '/path/to/django/db/models/query.py:1234 in filter'
        ],
        query_type='SELECT',
        table_names=['accounts_membership', 'auth_user']
    ))
    
    # Simulate N+1 pattern - accessing user details for each membership
    for i in range(4):
        queries.append(CapturedQuery(
            sql='SELECT "auth_user"."id", "auth_user"."username", "auth_user"."email" FROM "auth_user" WHERE "auth_user"."id" = %s',
            params=(f'{i+1}',),
            duration=0.012,
            timestamp=base_timestamp,
            stack_trace=[
                f'/path/to/your/project/incidents/private/services/accesses/auto_access.py:{20+i} in grant_auto_access_to_new_member',
                '/path/to/django/db/models/query.py:567 in get'
            ],
            query_type='SELECT',
            table_names=['auth_user']
        ))
    
    # Simulate another N+1 pattern - accessing account details
    for i in range(3):
        queries.append(CapturedQuery(
            sql='SELECT "accounts_account"."id", "accounts_account"."name", "accounts_account"."settings" FROM "accounts_account" WHERE "accounts_account"."id" = %s',
            params=(f'{i+1}',),
            duration=0.008,
            timestamp=base_timestamp,
            stack_trace=[
                f'/path/to/your/project/incidents/private/services/accesses/auto_access.py:{30+i} in grant_auto_access_to_new_member',
                '/path/to/django/db/models/query.py:567 in get'
            ],
            query_type='SELECT',
            table_names=['accounts_account']
        ))
    
    return queries


def test_enhanced_analyzer():
    """Test the enhanced analyzer with sample queries."""
    print("🧪 Testing Enhanced Django Analyzer")
    print("=" * 50)
    
    # Create test queries
    queries = create_test_queries()
    
    print(f"📊 Generated {len(queries)} test queries")
    print(f"   - 1 main query")
    print(f"   - 4 user lookup queries (N+1 pattern)")
    print(f"   - 3 account lookup queries (N+1 pattern)")
    print()
    
    # Run pattern detection
    pattern_detector = PatternDetector(queries)
    detected_patterns = pattern_detector.analyze()
    
    print(f"🔍 Pattern Detection Results:")
    print(f"   - Found {len(detected_patterns)} performance patterns")
    for i, pattern in enumerate(detected_patterns, 1):
        print(f"   {i}. {pattern.pattern_type} ({pattern.severity})")
    print()
    
    # Generate recommendations
    recommendations = DjangoRecommendations.generate_recommendations(detected_patterns)
    
    # Create analysis result
    result = AnalysisResult(
        start_time=queries[0].timestamp,
        end_time=queries[-1].timestamp,
        total_queries=len(queries),
        total_duration=sum(q.duration for q in queries),
        queries_by_type={'SELECT': len(queries)},
        duplicate_queries=0,
        detected_patterns=detected_patterns,
        recommendations=recommendations
    )
    
    # Print the enhanced summary
    print("📋 Enhanced Analysis Summary:")
    print("=" * 50)
    print(result.summary)


if __name__ == "__main__":
    test_enhanced_analyzer()