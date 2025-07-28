"""
Django-specific recommendations for query optimization.

Provides detailed recommendations and code examples for fixing
detected performance issues in Django ORM queries.
"""

from dataclasses import dataclass
from typing import List, Dict, Optional
from .pattern_detector import DetectedPattern


@dataclass
class Recommendation:
    """A specific optimization recommendation."""
    title: str
    description: str
    code_before: Optional[str]
    code_after: Optional[str]
    explanation: str
    references: List[str]
    difficulty: str  # easy, medium, hard
    impact: str  # low, medium, high, critical


class DjangoRecommendations:
    """Generate Django-specific optimization recommendations."""
    
    @staticmethod
    def generate_recommendations(patterns: List[DetectedPattern]) -> List[Recommendation]:
        """Generate recommendations for detected patterns."""
        recommendations = []
        
        for pattern in patterns:
            if pattern.pattern_type == "n_plus_one":
                recommendations.extend(DjangoRecommendations._n_plus_one_recommendations(pattern))
            elif pattern.pattern_type == "missing_select_related":
                recommendations.extend(DjangoRecommendations._select_related_recommendations(pattern))
            elif pattern.pattern_type == "missing_prefetch_related":
                recommendations.extend(DjangoRecommendations._prefetch_related_recommendations(pattern))
            elif pattern.pattern_type == "inefficient_count":
                recommendations.extend(DjangoRecommendations._count_recommendations(pattern))
            elif pattern.pattern_type == "missing_only":
                recommendations.extend(DjangoRecommendations._only_defer_recommendations(pattern))
            elif pattern.pattern_type == "large_result_set":
                recommendations.extend(DjangoRecommendations._pagination_recommendations(pattern))
            elif pattern.pattern_type == "unnecessary_ordering":
                recommendations.extend(DjangoRecommendations._ordering_recommendations(pattern))
        
        return recommendations
    
    @staticmethod
    def _n_plus_one_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for N+1 query issues."""
        query_count = len(pattern.affected_queries)
        
        return [
            Recommendation(
                title="Fix N+1 Query Problem",
                description=f"Detected {query_count} queries that could be reduced to 1-2 queries",
                code_before="""# N+1 Problem: Each iteration triggers a new query
for book in Book.objects.all():
    print(book.author.name)  # Triggers a query for each book""",
                code_after="""# Solution 1: Use select_related for ForeignKey/OneToOne
for book in Book.objects.select_related('author'):
    print(book.author.name)  # No additional queries

# Solution 2: Use prefetch_related for ManyToMany/reverse FK
for author in Author.objects.prefetch_related('books'):
    for book in author.books.all():  # No additional queries
        print(book.title)""",
                explanation="""The N+1 query problem occurs when you fetch a list of objects and then 
access a related object for each one. This results in 1 query for the initial list 
plus N queries for each related object. Using select_related() or prefetch_related() 
can fetch all the data in 1-2 queries instead.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#select-related",
                    "https://docs.djangoproject.com/en/stable/topics/db/optimization/"
                ],
                difficulty="easy",
                impact="critical" if query_count > 10 else "high"
            )
        ]
    
    @staticmethod
    def _select_related_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for missing select_related."""
        recommendations = [
            Recommendation(
                title="Use select_related() for Foreign Key Relationships",
                description="Optimize foreign key lookups with select_related()",
                code_before="""# Without select_related: 2 queries
order = Order.objects.get(id=order_id)  # Query 1
customer_name = order.customer.name      # Query 2""",
                code_after="""# With select_related: 1 query
order = Order.objects.select_related('customer').get(id=order_id)
customer_name = order.customer.name  # No additional query""",
                explanation="""select_related() works by creating an SQL join and including the fields 
of the related object in the SELECT statement. This is perfect for ForeignKey and 
OneToOne relationships where you know you'll need the related object.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#select-related"
                ],
                difficulty="easy",
                impact="high"
            )
        ]
        
        # Only add chained select_related recommendation if we detect multi-level relationships
        if pattern.specific_fields and any('__' in field for field in pattern.specific_fields):
            recommendations.append(Recommendation(
                title="Chain Multiple select_related() Calls",
                description="Follow foreign keys through multiple relationships",
                code_before="""# Multiple queries for nested relationships
for order in Order.objects.all():
    print(order.customer.address.city)  # 3 queries per order!""",
                code_after="""# Single query with chained select_related
orders = Order.objects.select_related('customer__address')
for order in orders:
    print(order.customer.address.city)  # No additional queries""",
                explanation="""You can follow foreign keys through multiple levels using double 
underscores. This creates more complex joins but eliminates multiple queries.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/topics/db/optimization/#use-select-related-and-prefetch-related"
                ],
                difficulty="medium",
                impact="high"
            ))
        
        return recommendations
    
    @staticmethod
    def _prefetch_related_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for missing prefetch_related."""
        return [
            Recommendation(
                title="Use prefetch_related() for Many-to-Many and Reverse Foreign Keys",
                description="Optimize multiple related object lookups",
                code_before="""# Without prefetch_related: N+1 queries
for author in Author.objects.all():
    books = author.book_set.all()  # Query for each author""",
                code_after="""# With prefetch_related: 2 queries total
authors = Author.objects.prefetch_related('book_set')
for author in authors:
    books = author.book_set.all()  # No additional query""",
                explanation="""prefetch_related() does a separate lookup for each relationship and 
joins the results in Python. This is ideal for ManyToMany fields and reverse 
ForeignKey relationships where select_related() can't be used.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#prefetch-related"
                ],
                difficulty="easy",
                impact="high"
            ),
            Recommendation(
                title="Use Prefetch Objects for Complex Queries",
                description="Customize prefetch queries for better performance",
                code_before="""# Inefficient: Fetches all related objects
authors = Author.objects.prefetch_related('book_set')""",
                code_after="""# Efficient: Only fetch what you need
from django.db.models import Prefetch

recent_books = Book.objects.filter(
    published_date__year__gte=2020
).select_related('publisher')

authors = Author.objects.prefetch_related(
    Prefetch('book_set', 
             queryset=recent_books,
             to_attr='recent_books')
)""",
                explanation="""Prefetch objects allow you to customize the queryset used for 
prefetching. This lets you filter, order, or apply select_related to the 
prefetched objects, significantly improving performance.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#prefetch-objects"
                ],
                difficulty="hard",
                impact="high"
            )
        ]
    
    @staticmethod
    def _count_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for inefficient count operations."""
        return [
            Recommendation(
                title="Use .count() Instead of len() for Query Counts",
                description="Optimize count operations to avoid loading all objects",
                code_before="""# Inefficient: Loads all objects into memory
total = len(Book.objects.all())

# Also inefficient
if len(Book.objects.filter(author=author)) > 0:
    # do something""",
                code_after="""# Efficient: Database counts without loading objects
total = Book.objects.count()

# Better existence check
if Book.objects.filter(author=author).exists():
    # do something""",
                explanation="""Using .count() executes COUNT(*) in the database without loading 
any objects into Python memory. Similarly, .exists() is more efficient than 
checking length for existence tests.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#count",
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#exists"
                ],
                difficulty="easy",
                impact="medium"
            )
        ]
    
    @staticmethod
    def _only_defer_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for field optimization."""
        return [
            Recommendation(
                title="Use only() or defer() to Limit Retrieved Fields",
                description="Reduce data transfer by fetching only needed fields",
                code_before="""# Fetches all fields (potentially many)
users = User.objects.all()
for user in users:
    print(user.username)  # Only need username""",
                code_after="""# Option 1: only() - specify fields to include
users = User.objects.only('username', 'id')

# Option 2: defer() - specify fields to exclude
users = User.objects.defer('bio', 'profile_image', 'preferences')

# Option 3: values() for dictionaries
usernames = User.objects.values_list('username', flat=True)""",
                explanation="""When you only need specific fields, using only() or defer() can 
significantly reduce data transfer and memory usage. The values() and values_list() 
methods are even more efficient when you don't need model instances.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#only",
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#defer"
                ],
                difficulty="easy",
                impact="medium"
            )
        ]
    
    @staticmethod
    def _pagination_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for large result sets."""
        return [
            Recommendation(
                title="Implement Pagination for Large Result Sets",
                description="Prevent memory issues and improve performance with pagination",
                code_before="""# Dangerous: Could load thousands of records
all_orders = Order.objects.all()
for order in all_orders:
    process_order(order)""",
                code_after="""# Solution 1: Use Django's paginator
from django.core.paginator import Paginator

orders = Order.objects.all()
paginator = Paginator(orders, 100)  # 100 items per page

for page_num in paginator.page_range:
    page = paginator.page(page_num)
    for order in page:
        process_order(order)

# Solution 2: Use iterator() for large datasets
for order in Order.objects.all().iterator(chunk_size=1000):
    process_order(order)

# Solution 3: Use slice notation
for order in Order.objects.all()[:1000]:  # First 1000 only
    process_order(order)""",
                explanation="""Large result sets can cause memory issues and slow performance. 
Django's Paginator provides easy pagination, while iterator() streams results 
efficiently for large datasets that must be processed entirely.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/topics/pagination/",
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#iterator"
                ],
                difficulty="medium",
                impact="high"
            )
        ]
    
    @staticmethod
    def _ordering_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for ordering optimization."""
        return [
            Recommendation(
                title="Optimize ORDER BY Queries with Database Indexes",
                description="Add indexes to improve sorting performance",
                code_before="""# Slow without index on created_at
recent_posts = Post.objects.order_by('-created_at')[:10]""",
                code_after="""# In your model:
class Post(models.Model):
    created_at = models.DateTimeField(db_index=True)
    # Or for multiple field ordering:
    class Meta:
        indexes = [
            models.Index(fields=['-created_at', 'author']),
        ]

# Query remains the same but runs much faster
recent_posts = Post.objects.order_by('-created_at')[:10]""",
                explanation="""Database indexes on ORDER BY fields can dramatically improve query 
performance. For frequently used orderings, especially with LIMIT clauses, 
appropriate indexes are essential.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/ref/models/options/#indexes",
                    "https://docs.djangoproject.com/en/stable/ref/models/fields/#db-index"
                ],
                difficulty="medium",
                impact="medium"
            )
        ]
    
    @staticmethod
    def format_recommendations_summary(recommendations: List[Recommendation]) -> str:
        """Format recommendations into a readable summary."""
        if not recommendations:
            return "No specific optimization recommendations."
        
        # Group by impact
        critical = [r for r in recommendations if r.impact == "critical"]
        high = [r for r in recommendations if r.impact == "high"]
        medium = [r for r in recommendations if r.impact == "medium"]
        low = [r for r in recommendations if r.impact == "low"]
        
        summary = []
        
        if critical:
            summary.append(f"🚨 CRITICAL ({len(critical)} issues):")
            for rec in critical:
                summary.append(f"   - {rec.title}")
        
        if high:
            summary.append(f"⚠️  HIGH ({len(high)} issues):")
            for rec in high:
                summary.append(f"   - {rec.title}")
        
        if medium:
            summary.append(f"🔔 MEDIUM ({len(medium)} issues):")
            for rec in medium:
                summary.append(f"   - {rec.title}")
        
        if low:
            summary.append(f"💡 LOW ({len(low)} suggestions):")
            for rec in low:
                summary.append(f"   - {rec.title}")
        
        return "\n".join(summary)