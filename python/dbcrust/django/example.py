#!/usr/bin/env python3
"""
Example usage of DBCrust Django ORM Query Analyzer.

This script demonstrates how to use the analyzer to detect
N+1 queries and other performance issues in Django applications.
"""

import os
import sys

# Add the parent directory to Python path for imports
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

# Mock Django setup for demonstration
try:
    import django
    from django.conf import settings
    
    # Configure Django settings
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
    
    # Create mock models for demonstration
    from django.db import models
    
    class Author(models.Model):
        name = models.CharField(max_length=100)
        email = models.EmailField()
        bio = models.TextField(blank=True)
        
        class Meta:
            app_label = 'example'
    
    class Book(models.Model):
        title = models.CharField(max_length=200)
        author = models.ForeignKey(Author, on_delete=models.CASCADE)
        isbn = models.CharField(max_length=13)
        published_date = models.DateField()
        price = models.DecimalField(max_digits=10, decimal_places=2)
        
        class Meta:
            app_label = 'example'
    
    class Review(models.Model):
        book = models.ForeignKey(Book, on_delete=models.CASCADE, related_name='reviews')
        reviewer_name = models.CharField(max_length=100)
        rating = models.IntegerField()
        comment = models.TextField()
        
        class Meta:
            app_label = 'example'
    
    print("✅ Django models configured successfully")
    DJANGO_AVAILABLE = True
    
except ImportError as e:
    print(f"❌ Django not available: {e}")
    print("This example requires Django to be installed.")
    DJANGO_AVAILABLE = False
    Author = Book = Review = None


def demonstrate_n_plus_one_detection():
    """Demonstrate N+1 query detection."""
    if not DJANGO_AVAILABLE:
        print("Skipping N+1 demonstration - Django not available")
        return
    
    print("\n" + "="*60)
    print("🔍 DEMONSTRATING N+1 QUERY DETECTION")
    print("="*60)
    
    try:
        from dbcrust.django import analyzer
        
        # This will be detected as N+1 queries
        with analyzer.analyze(transaction_safe=False) as analysis:
            print("Executing N+1 query pattern...")
            
            # Simulate N+1 pattern: fetch all books, then author for each
            books = Book.objects.all()  # 1 query
            for book in books:  # N queries (one per book)
                author_name = book.author.name
                print(f"Book: {book.title} by {author_name}")
        
        results = analysis.get_results()
        if results:
            print("\n📊 ANALYSIS RESULTS:")
            print(results.summary)
            
            # Show detected patterns
            n_plus_one_patterns = [p for p in results.detected_patterns 
                                 if p.pattern_type == 'n_plus_one']
            
            if n_plus_one_patterns:
                print(f"\n🚨 Found {len(n_plus_one_patterns)} N+1 patterns!")
                for pattern in n_plus_one_patterns:
                    print(f"   - {pattern.description}")
                    print(f"   - Recommendation: {pattern.recommendation}")
                    if pattern.code_suggestion:
                        print(f"   - Fix: {pattern.code_suggestion}")
            else:
                print("✅ No N+1 patterns detected (queries may be mocked)")
        
    except Exception as e:
        print(f"❌ Error during analysis: {e}")
        print("This is expected in a demo environment without actual database queries")


def demonstrate_missing_select_related():
    """Demonstrate missing select_related detection."""
    if not DJANGO_AVAILABLE:
        print("Skipping select_related demonstration - Django not available")
        return
    
    print("\n" + "="*60)
    print("🔍 DEMONSTRATING MISSING SELECT_RELATED DETECTION")
    print("="*60)
    
    try:
        from dbcrust.django import analyzer
        
        with analyzer.analyze(transaction_safe=False) as analysis:
            print("Executing queries that could benefit from select_related...")
            
            # This could use select_related('author')
            books = Book.objects.filter(published_date__year=2023)
            for book in books:
                # Each access to book.author will trigger a new query
                print(f"{book.title} by {book.author.name} ({book.author.email})")
        
        results = analysis.get_results()
        if results:
            print("\n📊 ANALYSIS RESULTS:")
            print(results.summary)
            
            # Show recommendations
            if results.recommendations:
                print(f"\n💡 OPTIMIZATION RECOMMENDATIONS:")
                for rec in results.recommendations:
                    print(f"   - {rec.title} ({rec.impact} impact)")
                    print(f"     {rec.description}")
        
    except Exception as e:
        print(f"❌ Error during analysis: {e}")


def demonstrate_comprehensive_analysis():
    """Demonstrate comprehensive query analysis."""
    if not DJANGO_AVAILABLE:
        print("Skipping comprehensive demonstration - Django not available")
        return
    
    print("\n" + "="*60)
    print("🔍 COMPREHENSIVE QUERY ANALYSIS DEMONSTRATION")
    print("="*60)
    
    try:
        from dbcrust.django import analyzer
        
        # Analyze multiple performance anti-patterns
        with analyzer.analyze(
            transaction_safe=False,
            enable_explain=False  # Disabled for demo
        ) as analysis:
            print("Executing various query patterns...")
            
            # Pattern 1: N+1 queries
            print("1. N+1 pattern with books and authors")
            books = Book.objects.all()
            for book in books[:5]:  # Limit for demo
                author_info = f"{book.author.name} ({book.author.email})"
                print(f"   {book.title} by {author_info}")
            
            # Pattern 2: Missing prefetch_related
            print("2. Missing prefetch_related with reviews")
            for book in books[:3]:
                review_count = book.reviews.count()
                print(f"   {book.title} has {review_count} reviews")
            
            # Pattern 3: Inefficient count
            print("3. Potentially inefficient operations")
            all_authors = Author.objects.all()
            author_count = len(list(all_authors))  # Could use .count()
            print(f"   Total authors: {author_count}")
            
            # Pattern 4: Large result set without limit
            print("4. Query without LIMIT")
            all_reviews = Review.objects.all()  # Should use pagination
            print(f"   Processing {len(list(all_reviews))} reviews")
            
        results = analysis.get_results()
        if results:
            print("\n" + "="*60)
            print("📊 COMPREHENSIVE ANALYSIS RESULTS")
            print("="*60)
            print(results.summary)
            
            # Detailed breakdown
            if results.detected_patterns:
                print(f"\n🔍 DETECTED PATTERNS ({len(results.detected_patterns)} total):")
                pattern_types = {}
                for pattern in results.detected_patterns:
                    if pattern.pattern_type not in pattern_types:
                        pattern_types[pattern.pattern_type] = []
                    pattern_types[pattern.pattern_type].append(pattern)
                
                for pattern_type, patterns in pattern_types.items():
                    print(f"\n   {pattern_type.replace('_', ' ').title()} ({len(patterns)} issues):")
                    for pattern in patterns:
                        severity_icon = {
                            'critical': '🚨',
                            'high': '⚠️',
                            'medium': '🔔',
                            'low': '💡'
                        }.get(pattern.severity, '❓')
                        
                        print(f"     {severity_icon} {pattern.description}")
                        if pattern.recommendation:
                            print(f"        💡 {pattern.recommendation}")
                        if pattern.estimated_impact:
                            print(f"        📈 Impact: {pattern.estimated_impact}")
            
            # Export results for further analysis
            try:
                analysis.export_results("demo_analysis_results.json")
                print(f"\n💾 Results exported to demo_analysis_results.json")
            except Exception as e:
                print(f"❌ Could not export results: {e}")
        
    except Exception as e:
        print(f"❌ Error during comprehensive analysis: {e}")
        import traceback
        traceback.print_exc()


def show_usage_examples():
    """Show various usage examples."""
    print("\n" + "="*60)
    print("📖 USAGE EXAMPLES")
    print("="*60)
    
    print("""
1. Basic Usage:
   ```python
   from dbcrust.django import analyzer
   
   with analyzer.analyze() as analysis:
       MyModel.objects.all()  # Your Django queries here
   
   results = analysis.get_results()
   print(results.summary)
   ```

2. With DBCrust Integration:
   ```python
   with analyzer.analyze(dbcrust_url="postgres://localhost/mydb") as analysis:
       # Complex queries that benefit from EXPLAIN analysis
       expensive_query = MyModel.objects.select_related('foreign_key')
   
   results = analysis.get_results()
   if results.dbcrust_analysis:
       print(results.dbcrust_analysis['performance_report'])
   ```

3. In Tests:
   ```python
   def test_view_performance(self):
       with analyzer.analyze() as analysis:
           response = self.client.get('/my-view/')
       
       results = analysis.get_results()
       # Assert no N+1 queries
       n_plus_one = [p for p in results.detected_patterns 
                    if p.pattern_type == 'n_plus_one']
       self.assertEqual(len(n_plus_one), 0)
   ```

4. Advanced Configuration:
   ```python
   from dbcrust.django import DjangoAnalyzer
   
   analyzer = DjangoAnalyzer(
       dbcrust_url="postgres://localhost/mydb",
       transaction_safe=True,      # Rollback after analysis
       enable_explain=True,        # Run EXPLAIN ANALYZE
       database_alias='default'    # Django database to use
   )
   
   with analyzer.analyze() as analysis:
       # Your code here
       pass
   ```
""")


def main():
    """Main demonstration function."""
    print("🚀 DBCrust Django ORM Query Analyzer - Demonstration")
    print("=" * 80)
    
    if not DJANGO_AVAILABLE:
        print("❌ Django is not available. Please install Django to run this demo.")
        print("   pip install django")
        return
    
    try:
        from dbcrust.django import analyzer
        print("✅ DBCrust Django analyzer imported successfully")
    except ImportError as e:
        print(f"❌ Could not import DBCrust Django analyzer: {e}")
        print("This is expected if you're not running within a DBCrust installation.")
        return
    
    # Run demonstrations
    demonstrate_n_plus_one_detection()
    demonstrate_missing_select_related()
    demonstrate_comprehensive_analysis()
    show_usage_examples()
    
    print("\n" + "="*80)
    print("✅ DEMONSTRATION COMPLETE")
    print("="*80)
    print("""
Key Features Demonstrated:
• N+1 query detection
• Missing select_related detection
• Missing prefetch_related detection
• Inefficient count operations
• Large result set warnings
• Comprehensive reporting
• Export functionality

Next Steps:
1. Integrate the analyzer into your Django development workflow
2. Add performance tests to your test suite
3. Use in development to identify optimization opportunities
4. Configure with DBCrust URL for EXPLAIN ANALYZE insights

For more information, see the README.md file or the full documentation.
""")


if __name__ == "__main__":
    main()