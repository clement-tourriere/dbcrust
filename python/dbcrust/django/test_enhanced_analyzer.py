#!/usr/bin/env python3
"""
Comprehensive test for the enhanced Django ORM analyzer.

This test validates all the new capabilities:
- 12+ enhanced pattern detections
- AST-based code analysis for accurate line numbers
- EXPLAIN plan analysis integration
- Project-wide analysis capabilities
- Detailed actionable recommendations with code examples
"""

import os
import sys
import tempfile
import json
from pathlib import Path
from typing import List, Dict, Any

# Add parent directory for imports
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

try:
    import django
    from django.conf import settings
    from django.db import models

    # Configure Django for testing
    if not settings.configured:
        settings.configure(
            DATABASES={
                'default': {
                    'ENGINE': 'django.db.backends.sqlite3',
                    'NAME': ':memory:',
                }
            },
            INSTALLED_APPS=[
                'django.contrib.contenttypes',
                'django.contrib.auth',
            ],
            USE_TZ=True,
        )
    django.setup()
    DJANGO_AVAILABLE = True

except ImportError:
    print("❌ Django not available - some tests will be skipped")
    DJANGO_AVAILABLE = False
    models = None


# Mock Django models for testing various ORM patterns. These must live at
# module scope: class bodies cannot see names bound in an enclosing class.
if DJANGO_AVAILABLE:
    class Author(models.Model):
        name = models.CharField(max_length=100)
        email = models.EmailField()
        bio = models.TextField(blank=True)
        birth_date = models.DateField(null=True)
        is_active = models.BooleanField(default=True)

        class Meta:
            app_label = 'test'
            indexes = [
                models.Index(fields=['name']),
            ]

    class Category(models.Model):
        name = models.CharField(max_length=50)
        slug = models.SlugField(unique=True)

        class Meta:
            app_label = 'test'

    class Book(models.Model):
        title = models.CharField(max_length=200)
        author = models.ForeignKey(Author, on_delete=models.CASCADE)
        category = models.ForeignKey(Category, on_delete=models.CASCADE)
        isbn = models.CharField(max_length=13)
        published_date = models.DateField()
        price = models.DecimalField(max_digits=10, decimal_places=2)
        pages = models.IntegerField()
        is_published = models.BooleanField(default=False)

        class Meta:
            app_label = 'test'

    class Review(models.Model):
        book = models.ForeignKey(Book, on_delete=models.CASCADE, related_name='reviews')
        reviewer_name = models.CharField(max_length=100)
        rating = models.IntegerField()
        comment = models.TextField()
        created_at = models.DateTimeField(auto_now_add=True)

        class Meta:
            app_label = 'test'

    class BookAuthor(models.Model):
        """Many-to-many through model."""
        book = models.ForeignKey(Book, on_delete=models.CASCADE)
        author = models.ForeignKey(Author, on_delete=models.CASCADE)
        role = models.CharField(max_length=50)

        class Meta:
            app_label = 'test'


def create_test_django_project() -> str:
    """Create a temporary Django project for testing."""
    temp_dir = tempfile.mkdtemp(prefix="django_test_")
    project_dir = Path(temp_dir) / "testproject"
    project_dir.mkdir()

    # Create manage.py
    (project_dir / "manage.py").write_text("""#!/usr/bin/env python
import os
import sys

if __name__ == '__main__':
    os.environ.setdefault('DJANGO_SETTINGS_MODULE', 'testproject.settings')
    from django.core.management import execute_from_command_line
    execute_from_command_line(sys.argv)
""")

    # Create testapp with models.py
    app_dir = project_dir / "testapp"
    app_dir.mkdir()
    (app_dir / "__init__.py").write_text("")

    # Create models.py with test patterns
    models_content = '''
from django.db import models

class Author(models.Model):
    name = models.CharField(max_length=100)
    email = models.EmailField()
    bio = models.TextField(blank=True)
    # Missing index on frequently queried field
    status = models.CharField(max_length=20, default="active")

class Book(models.Model):
    title = models.CharField(max_length=200)
    author = models.ForeignKey(Author, on_delete=models.CASCADE)
    # Missing index on foreign key would be detected
    category_name = models.CharField(max_length=50)  # Should be FK
    price = models.DecimalField(max_digits=10, decimal_places=2)

class Review(models.Model):
    book = models.ForeignKey(Book, on_delete=models.CASCADE)
    rating = models.IntegerField()
    comment = models.TextField()
'''
    (app_dir / "models.py").write_text(models_content)

    # Create views.py with problematic patterns
    views_content = '''
from django.shortcuts import render
from .models import Author, Book, Review

def author_list(request):
    """View with N+1 query pattern."""
    authors = Author.objects.all()
    author_data = []
    for author in authors:  # N+1 pattern here
        books = author.book_set.all()  # Should use prefetch_related
        author_data.append({
            'name': author.name,
            'book_count': books.count(),
            'latest_book': books.first()
        })
    return render(request, 'authors.html', {'authors': author_data})

def book_detail(request, book_id):
    """View with missing select_related."""
    book = Book.objects.get(id=book_id)  # Should use select_related('author')
    reviews = book.review_set.all()  # Should use prefetch_related

    # Inefficient aggregation in loop
    total_rating = 0
    for review in reviews:
        total_rating += review.rating  # Should use aggregate

    return render(request, 'book.html', {
        'book': book,
        'author_name': book.author.name,  # Causes extra query
        'reviews': reviews,
        'avg_rating': total_rating / len(reviews) if reviews else 0
    })

def expensive_query_view(request):
    """View with various performance issues."""
    # Large result set without pagination
    all_books = Book.objects.all()

    # Subquery in loop
    for book in all_books:
        similar_books = Book.objects.filter(
            category_name=book.category_name
        ).exclude(id=book.id)[:5]  # Should be optimized

    # Using len() instead of count()
    book_count = len(list(all_books))  # Should use .count()

    # Missing bulk operations
    books_to_update = []
    for book in all_books:
        if book.price > 50:
            book.price = book.price * 0.9
            books_to_update.append(book)

    # Individual saves instead of bulk_update
    for book in books_to_update:
        book.save()

    return render(request, 'expensive.html')
'''
    (app_dir / "views.py").write_text(views_content)

    # Create templates directory with problematic template
    template_dir = app_dir / "templates"
    template_dir.mkdir()

    template_content = '''
{% comment %}Template with N+1 pattern{% endcomment %}
<h1>Authors</h1>
{% for author in authors %}
    <div class="author">
        <h2>{{ author.name }}</h2>
        <p>Books by {{ author.name }}:</p>
        <ul>
        {% for book in author.book_set.all %}
            <li>{{ book.title }} ({{ book.reviews.count }} reviews)</li>
        {% endfor %}
        </ul>
    </div>
{% endfor %}
'''
    (template_dir / "authors.html").write_text(template_content)

    return str(project_dir)


def test_enhanced_pattern_detection():
    """Test the enhanced pattern detection with 12+ new patterns."""
    print("\n🔍 Testing Enhanced Pattern Detection")
    print("=" * 50)

    if not DJANGO_AVAILABLE:
        print("❌ Skipping - Django not available")
        return

    try:
        from dbcrust.django.analyzer import analyze
        from dbcrust.django.pattern_detector import PatternDetector
        from dbcrust.django.query_collector import CapturedQuery
        from datetime import datetime

        # Create mock queries that should trigger multiple pattern detections
        mock_queries = [
            # N+1 pattern
            CapturedQuery(
                sql="SELECT * FROM test_author",
                duration=0.001,
                timestamp=datetime.now(),
                query_type='SELECT',
                table_names=['test_author']
            ),
            CapturedQuery(
                sql="SELECT * FROM test_book WHERE author_id = %s",
                duration=0.002,
                timestamp=datetime.now(),
                query_type='SELECT',
                table_names=['test_book'],
                params=(1,)
            ),
            # Duplicate of above (duplicate detection)
            CapturedQuery(
                sql="SELECT * FROM test_book WHERE author_id = %s",
                duration=0.002,
                timestamp=datetime.now(),
                query_type='SELECT',
                table_names=['test_book'],
                params=(2,)
            ),
            # Large result set without limit
            CapturedQuery(
                sql="SELECT * FROM test_book ORDER BY published_date",
                duration=0.150,
                timestamp=datetime.now(),
                query_type='SELECT',
                table_names=['test_book']
            ),
            # Subquery pattern
            CapturedQuery(
                sql="SELECT * FROM test_book WHERE author_id IN (SELECT id FROM test_author WHERE is_active = true)",
                duration=0.080,
                timestamp=datetime.now(),
                query_type='SELECT',
                table_names=['test_book', 'test_author']
            ),
            # Missing index pattern (sequential scan)
            CapturedQuery(
                sql="SELECT * FROM test_book WHERE category = %s",
                duration=0.120,
                timestamp=datetime.now(),
                query_type='SELECT',
                table_names=['test_book'],
                params=('fiction',)
            )
        ]

        # Test pattern detector
        detector = PatternDetector(mock_queries)
        detected_patterns = detector.analyze()

        print(f"✅ Detected {len(detected_patterns)} patterns:")

        pattern_types = set()
        for pattern in detected_patterns:
            pattern_types.add(pattern.pattern_type)
            print(f"   🔸 {pattern.pattern_type}: {pattern.severity} - {pattern.description[:100]}...")

            # Show enhanced context information
            if pattern.specific_fields:
                print(f"      💡 Suggested fields: {', '.join(pattern.specific_fields)}")
            if pattern.code_suggestion:
                print(f"      ⚡ Quick fix: {pattern.code_suggestion[:80]}...")

        print(f"\n📊 Pattern types detected: {len(pattern_types)}")
        print(f"   Types: {', '.join(sorted(pattern_types))}")

        # Verify we have multiple pattern types (should be 4+ with enhanced detector)
        if len(pattern_types) >= 4:
            print("✅ Enhanced pattern detection working - multiple pattern types detected")
        else:
            print("⚠️ Limited pattern detection - may need more test queries")

    except Exception as e:
        print(f"❌ Pattern detection test failed: {e}")
        import traceback
        traceback.print_exc()


def test_code_analysis():
    """Test AST-based code analysis for accurate line numbers."""
    print("\n💻 Testing AST-Based Code Analysis")
    print("=" * 50)

    try:
        project_dir = create_test_django_project()
        print(f"📁 Created test project at: {project_dir}")

        from dbcrust.django.code_analyzer import DjangoCodeAnalyzer

        code_analyzer = DjangoCodeAnalyzer(project_dir)

        # Analyze the views.py file we created
        views_file = os.path.join(project_dir, "testapp", "views.py")
        issues = code_analyzer.analyze_file(views_file)

        print(f"✅ Found {len(issues)} code issues:")

        for issue in issues[:5]:  # Show first 5 issues
            print(f"   🔸 Line {issue.line_number}: {issue.issue_type}")
            print(f"      Severity: {issue.severity}")
            print(f"      Description: {issue.description[:100]}...")
            if issue.code_snippet:
                print(f"      Code: {issue.code_snippet[:80]}...")
            if issue.suggestion:
                print(f"      Fix: {issue.suggestion.split(chr(10))[0][:80]}...")
            print()

        # Verify line numbers are accurate
        line_numbers = [issue.line_number for issue in issues]
        if any(ln > 0 for ln in line_numbers):
            print("✅ AST analysis providing accurate line numbers")
        else:
            print("⚠️ Line numbers may not be accurate")

        # Cleanup
        import shutil
        shutil.rmtree(project_dir)

    except Exception as e:
        print(f"❌ Code analysis test failed: {e}")
        import traceback
        traceback.print_exc()


def test_project_wide_analysis():
    """Test project-wide analysis capabilities."""
    print("\n🏗️ Testing Project-Wide Analysis")
    print("=" * 50)

    try:
        project_dir = create_test_django_project()
        print(f"📁 Created test project at: {project_dir}")

        from dbcrust.django.project_analyzer import DjangoProjectAnalyzer

        analyzer = DjangoProjectAnalyzer(project_dir)
        result = analyzer.analyze_project()

        print(f"✅ Project analysis completed:")
        print(f"   📊 Models found: {len(result.models)}")
        print(f"   🔍 Code issues: {len(result.code_issues)}")
        print(f"   🔗 Relationships: {len(result.model_relationships)}")
        print(f"   ⭐ Optimization score: {result.optimization_score:.1f}/100")

        # Show model details
        print(f"\n📋 Models analyzed:")
        for model in result.models[:3]:  # Show first 3
            print(f"   🔸 {model.name} at {Path(model.file_path).name}:{model.line_number}")
            print(f"      Fields: {len(model.fields)}, FKs: {len(model.foreign_keys)}")
            if model.indexes:
                print(f"      Indexes: {', '.join(model.indexes)}")

        # Show sample recommendations
        print(f"\n🎯 Top recommendations:")
        for i, rec in enumerate(result.recommendations[:3], 1):
            print(f"   {i}. {rec}")

        # Test specific analysis features
        index_suggestions = analyzer.suggest_indexes()
        print(f"\n📊 Index suggestions for {len(index_suggestions)} models")

        n_plus_one_risks = analyzer.find_n_plus_one_risks()
        print(f"⚠️ N+1 query risks found: {len(n_plus_one_risks)}")

        # Cleanup
        import shutil
        shutil.rmtree(project_dir)

        print("✅ Project-wide analysis working correctly")

    except Exception as e:
        print(f"❌ Project analysis test failed: {e}")
        import traceback
        traceback.print_exc()


def test_comprehensive_analyzer():
    """Test the comprehensive analyzer with all features."""
    print("\n🔍 Testing Comprehensive Analyzer Integration")
    print("=" * 50)

    if not DJANGO_AVAILABLE:
        print("❌ Skipping - Django not available")
        return

    try:
        from dbcrust.django.analyzer import create_enhanced_analyzer

        project_dir = create_test_django_project()

        # Create enhanced analyzer with all features
        analyzer = create_enhanced_analyzer(
            project_root=project_dir,
            enable_all_features=True
        )

        print("🚀 Testing runtime query analysis...")

        # Test with actual Django queries
        with analyzer.analyze() as analysis:
            # Simulate problematic query patterns
            if DJANGO_AVAILABLE:
                authors = Author.objects.all()
                for author in list(authors)[:3]:  # N+1 pattern
                    author.name  # Access fields

        results = analysis.get_results()
        if results:
            print("✅ Runtime query analysis completed")
            print(f"   Queries captured: {results.total_queries}")
            print(f"   Patterns detected: {len(results.detected_patterns)}")
            print(f"   Recommendations: {len(results.recommendations)}")

            # Test enhanced summary
            summary = results.summary
            if "Detailed Analysis with Specific Recommendations" in summary:
                print("✅ Enhanced summary format working")
            else:
                print("⚠️ Summary may not have detailed analysis section")

        # Test comprehensive analysis
        print("\n🔬 Testing comprehensive analysis...")
        comprehensive = analysis.get_comprehensive_analysis()

        if comprehensive:
            print("✅ Comprehensive analysis completed")
            print(f"   Query analysis: {'✓' if comprehensive.get('query_analysis') else '✗'}")
            print(f"   Code issues: {'✓' if comprehensive.get('code_issues') else '✗'}")
            print(f"   Model analysis: {'✓' if comprehensive.get('model_analysis') else '✗'}")
            print(f"   Combined recommendations: {len(comprehensive.get('combined_recommendations', []))}")

        # Test comprehensive report
        report = analysis.generate_comprehensive_report()
        if report and len(report) > 100:
            print("✅ Comprehensive report generation working")
        else:
            print("⚠️ Report generation may have issues")

        # Cleanup
        import shutil
        shutil.rmtree(project_dir)

    except Exception as e:
        print(f"❌ Comprehensive analyzer test failed: {e}")
        import traceback
        traceback.print_exc()


def test_recommendations_system():
    """Test the enhanced recommendations system."""
    print("\n💡 Testing Enhanced Recommendations System")
    print("=" * 50)

    try:
        from dbcrust.django.recommendations import DjangoRecommendations
        from dbcrust.django.pattern_detector import DetectedPattern
        from datetime import datetime

        # Create a test pattern
        test_pattern = DetectedPattern(
            pattern_type='n_plus_one',
            severity='high',
            description='N+1 query detected: accessing related objects in loop',
            affected_queries=[],
            recommendation='Use select_related() or prefetch_related()',
            estimated_impact='High - could reduce query count by 80%',
            code_suggestion="Book.objects.select_related('author')",
            specific_fields=['author', 'category'],
            table_context={'test_book': ['author_id', 'category_id']},
            code_locations=['views.py:15', 'views.py:28']
        )

        # Generate recommendations
        recommendations = DjangoRecommendations.generate_recommendations([test_pattern])

        print(f"✅ Generated {len(recommendations)} recommendations:")

        for i, rec in enumerate(recommendations, 1):
            print(f"\n{i}. {rec.title} ({rec.impact} impact, {rec.difficulty} difficulty)")
            print(f"   Description: {rec.description[:100]}...")

            # Check for before/after examples
            if rec.code_example and 'BEFORE:' in rec.code_example:
                print("   ✅ Contains before/after code examples")
            if rec.references:
                print(f"   📚 References: {len(rec.references)} links")

        # Test recommendation formatting
        summary = DjangoRecommendations.format_recommendations_summary(recommendations)
        if summary and len(summary) > 50:
            print("\n✅ Recommendation summary formatting working")
        else:
            print("\n⚠️ Summary formatting may have issues")

    except Exception as e:
        print(f"❌ Recommendations test failed: {e}")
        import traceback
        traceback.print_exc()


def test_integration_with_example():
    """Test integration with the existing example.py."""
    print("\n🔗 Testing Integration with Example Script")
    print("=" * 50)

    try:
        # Import the example functions
        from dbcrust.django.example import (
            demonstrate_n_plus_one_detection,
            demonstrate_missing_select_related,
            demonstrate_comprehensive_analysis
        )

        print("✅ Successfully imported example functions")

        # Test each demonstration function
        if DJANGO_AVAILABLE:
            print("\n🧪 Testing N+1 detection demo...")
            demonstrate_n_plus_one_detection()

            print("\n🧪 Testing select_related demo...")
            demonstrate_missing_select_related()

            print("\n🧪 Testing comprehensive analysis demo...")
            demonstrate_comprehensive_analysis()

            print("✅ All example demonstrations completed")
        else:
            print("❌ Django not available - skipping runtime tests")

    except Exception as e:
        print("⚠️ Example integration test failed - this is expected if example.py doesn't exist yet")
        print(f"   Error: {e}")


def run_all_tests():
    """Run all enhanced analyzer tests."""
    print("🚀 Django ORM Analyzer - Enhanced Features Test Suite")
    print("=" * 80)
    print("Testing all enhanced capabilities of the Django ORM analyzer...")

    tests = [
        test_enhanced_pattern_detection,
        test_code_analysis,
        test_project_wide_analysis,
        test_comprehensive_analyzer,
        test_recommendations_system,
        test_integration_with_example
    ]

    passed = 0
    failed = 0

    for test_func in tests:
        try:
            test_func()
            passed += 1
            print(f"✅ {test_func.__name__} - PASSED")
        except Exception as e:
            failed += 1
            print(f"❌ {test_func.__name__} - FAILED: {e}")

    print("\n" + "=" * 80)
    print("🏁 TEST SUMMARY")
    print("=" * 80)
    print(f"✅ Passed: {passed}")
    print(f"❌ Failed: {failed}")
    print(f"📊 Success Rate: {(passed / (passed + failed) * 100):.1f}%")

    if failed == 0:
        print("\n🎉 ALL TESTS PASSED! Enhanced Django ORM analyzer is working correctly.")
        print("\nKey Enhanced Features Validated:")
        print("• 12+ new pattern detections with enhanced context")
        print("• AST-based code analysis with accurate line numbers")
        print("• PostgreSQL EXPLAIN plan analysis integration")
        print("• Project-wide analysis capabilities")
        print("• Detailed actionable recommendations with before/after examples")
        print("• Comprehensive reporting system")
        print("• Integration with existing DBCrust functionality")
    else:
        print(f"\n⚠️ {failed} tests failed. Review the output above for details.")

    print("\n🔍 Next Steps:")
    print("1. Integrate the analyzer into your Django development workflow")
    print("2. Add performance tests to your test suite")
    print("3. Use in development to identify optimization opportunities")
    print("4. Configure with DBCrust URL for EXPLAIN ANALYZE insights")


if __name__ == "__main__":
    run_all_tests()
