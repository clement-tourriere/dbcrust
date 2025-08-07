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
            
            # New pattern recommendations
            elif pattern.pattern_type == "subqueries_in_loops":
                recommendations.extend(DjangoRecommendations._subqueries_in_loops_recommendations(pattern))
            elif pattern.pattern_type == "missing_database_index":
                recommendations.extend(DjangoRecommendations._database_index_recommendations(pattern))
            elif pattern.pattern_type == "inefficient_aggregations":
                recommendations.extend(DjangoRecommendations._aggregation_recommendations(pattern))
            elif pattern.pattern_type == "missing_bulk_operations":
                recommendations.extend(DjangoRecommendations._bulk_operations_recommendations(pattern))
            elif pattern.pattern_type == "inefficient_exists_check":
                recommendations.extend(DjangoRecommendations._exists_check_recommendations(pattern))
            elif pattern.pattern_type == "missing_select_for_update":
                recommendations.extend(DjangoRecommendations._select_for_update_recommendations(pattern))
            elif pattern.pattern_type == "transaction_issues":
                recommendations.extend(DjangoRecommendations._transaction_recommendations(pattern))
            elif pattern.pattern_type == "connection_pool_risk":
                recommendations.extend(DjangoRecommendations._connection_pool_recommendations(pattern))
            elif pattern.pattern_type == "inefficient_distinct":
                recommendations.extend(DjangoRecommendations._distinct_recommendations(pattern))
            elif pattern.pattern_type == "missing_values_optimization":
                recommendations.extend(DjangoRecommendations._values_optimization_recommendations(pattern))
            elif pattern.pattern_type == "redundant_queries":
                recommendations.extend(DjangoRecommendations._redundant_queries_recommendations(pattern))
            elif pattern.pattern_type == "missing_query_caching":
                recommendations.extend(DjangoRecommendations._query_caching_recommendations(pattern))
        
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
    def _subqueries_in_loops_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for subqueries in loops."""
        return [
            Recommendation(
                title="Optimize Subqueries Executed in Loop",
                description=f"Detected {len(pattern.affected_queries)} subqueries likely executed in a loop pattern",
                code_before="""# Inefficient: Building subqueries in loop
items = []
for category in categories:
    # Each iteration builds a larger IN clause
    related_items = Item.objects.filter(
        category=category,
        status__in=get_active_statuses()  # This grows each time
    )
    items.extend(related_items)""",
                code_after="""# Efficient: Single query approach
# Option 1: Single query with all IDs
category_ids = [cat.id for cat in categories]
all_active_statuses = get_active_statuses()
items = Item.objects.filter(
    category__id__in=category_ids,
    status__in=all_active_statuses
)

# Option 2: Use prefetch_related for relationships
categories = Category.objects.prefetch_related(
    Prefetch('items', 
             queryset=Item.objects.filter(status__in=all_active_statuses))
)""",
                explanation="""Subqueries in loops often create exponentially growing query complexity.
The pattern indicates queries with increasing parameter counts, suggesting that
data is being accumulated in loops rather than fetched efficiently.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#in",
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#prefetch-related"
                ],
                difficulty="medium",
                impact="high"
            )
        ]
    
    @staticmethod
    def _database_index_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for missing database indexes."""
        fields = pattern.specific_fields or ['field']
        fields_str = ', '.join(f"'{f}'" for f in fields)
        
        return [
            Recommendation(
                title="Add Database Index for Query Performance",
                description=f"Slow query detected on fields that would benefit from indexes: {fields_str}",
                code_before="""# Model without index
class Product(models.Model):
    name = models.CharField(max_length=100)
    category = models.CharField(max_length=50)  # Frequently filtered
    price = models.DecimalField(max_digits=10, decimal_places=2)
    created_at = models.DateTimeField(auto_now_add=True)  # Frequently ordered

# Slow query due to missing indexes
products = Product.objects.filter(category='electronics').order_by('-created_at')""",
                code_after="""# Model with appropriate indexes
class Product(models.Model):
    name = models.CharField(max_length=100)
    category = models.CharField(max_length=50, db_index=True)  # Single field index
    price = models.DecimalField(max_digits=10, decimal_places=2)
    created_at = models.DateTimeField(auto_now_add=True, db_index=True)
    
    class Meta:
        indexes = [
            # Composite index for common query patterns
            models.Index(fields=['category', '-created_at']),
            # Partial index for active items only
            models.Index(fields=['price'], condition=Q(active=True)),
        ]

# Same query now uses indexes
products = Product.objects.filter(category='electronics').order_by('-created_at')""",
                explanation="""Database indexes dramatically improve query performance by providing
fast lookup paths. Without indexes, databases must scan entire tables (sequential scan).
With proper indexes, lookups become logarithmic time complexity.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/ref/models/options/#indexes",
                    "https://docs.djangoproject.com/en/stable/ref/models/fields/#db-index"
                ],
                difficulty="easy",
                impact="critical"
            )
        ]
    
    @staticmethod
    def _aggregation_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for inefficient aggregations."""
        return [
            Recommendation(
                title="Combine Multiple Aggregations into Single Query",
                description="Multiple aggregation queries detected that could be combined",
                code_before="""# Inefficient: Multiple aggregation queries
total_orders = Order.objects.count()
total_revenue = Order.objects.aggregate(Sum('total'))['total__sum']
avg_order_value = Order.objects.aggregate(Avg('total'))['total__avg']
max_order = Order.objects.aggregate(Max('total'))['total__max']
orders_this_month = Order.objects.filter(
    created_at__month=timezone.now().month
).count()""",
                code_after="""# Efficient: Single aggregation query
from django.db.models import Count, Sum, Avg, Max, Q
from django.utils import timezone

stats = Order.objects.aggregate(
    total_orders=Count('id'),
    total_revenue=Sum('total'),
    avg_order_value=Avg('total'),
    max_order=Max('total'),
    orders_this_month=Count(
        'id', 
        filter=Q(created_at__month=timezone.now().month)
    )
)

# Access results
total_orders = stats['total_orders']
total_revenue = stats['total_revenue']
avg_order_value = stats['avg_order_value']
max_order = stats['max_order']
orders_this_month = stats['orders_this_month']""",
                explanation="""Django's aggregate() method can compute multiple aggregations
in a single database query. This is much more efficient than running separate
queries for each aggregation, especially on large datasets.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/topics/db/aggregation/",
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#aggregate"
                ],
                difficulty="easy",
                impact="high"
            )
        ]
    
    @staticmethod
    def _bulk_operations_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for missing bulk operations."""
        operation_count = len(pattern.affected_queries)
        
        return [
            Recommendation(
                title="Use Bulk Operations for Multiple Database Changes",
                description=f"Detected {operation_count} individual operations that could be batched",
                code_before="""# Inefficient: Individual operations
for item_data in items_to_create:
    Item.objects.create(
        name=item_data['name'],
        category=item_data['category'],
        price=item_data['price']
    )

# Inefficient: Individual updates
for item in items_to_update:
    item.price = calculate_new_price(item)
    item.save()

# Inefficient: Individual deletes
for item_id in item_ids_to_delete:
    Item.objects.filter(id=item_id).delete()""",
                code_after="""# Efficient: Bulk operations
from django.db import transaction

# Bulk create
items_to_create = [
    Item(name=data['name'], category=data['category'], price=data['price'])
    for data in items_data
]
Item.objects.bulk_create(items_to_create, batch_size=1000)

# Bulk update (Django 2.2+)
items_to_update = Item.objects.filter(id__in=update_ids)
for item in items_to_update:
    item.price = calculate_new_price(item)
Item.objects.bulk_update(items_to_update, ['price'], batch_size=1000)

# Bulk delete
Item.objects.filter(id__in=item_ids_to_delete).delete()

# For complex operations, use transactions
with transaction.atomic():
    # Multiple operations as single transaction
    Item.objects.bulk_create(new_items)
    Item.objects.filter(id__in=update_ids).update(status='processed')""",
                explanation="""Bulk operations reduce database round trips and improve performance
significantly. Individual operations require separate database calls, while
bulk operations can process hundreds of records in a single query.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#bulk-create",
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#bulk-update"
                ],
                difficulty="easy",
                impact="critical"
            )
        ]
    
    @staticmethod
    def _exists_check_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for inefficient exists checks."""
        return [
            Recommendation(
                title="Use exists() Instead of count() for Existence Checks",
                description="Inefficient existence check using count() detected",
                code_before="""# Inefficient: Using count() for existence
if MyModel.objects.filter(status='active').count() > 0:
    # do something

# Also inefficient
if len(MyModel.objects.filter(status='active')) > 0:
    # do something

# Very inefficient
active_items = MyModel.objects.filter(status='active')
if active_items.count():
    # This still hits the database even after the filter""",
                code_after="""# Efficient: Using exists()
if MyModel.objects.filter(status='active').exists():
    # do something

# For multiple checks, you might want to cache the queryset
queryset = MyModel.objects.filter(status='active')
if queryset.exists():
    # Now safe to iterate if needed
    for item in queryset:
        process_item(item)

# exists() also works well with complex queries
if MyModel.objects.filter(
    status='active',
    created_at__gte=yesterday,
    category__in=['electronics', 'books']
).exists():
    # Complex existence check optimized""",
                explanation="""The exists() method returns a boolean and stops as soon as it
finds the first matching record. count() must count all matching records,
which is unnecessary when you only need to know if any records exist.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#exists"
                ],
                difficulty="easy",
                impact="medium"
            )
        ]
    
    @staticmethod
    def _select_for_update_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for missing select_for_update."""
        return [
            Recommendation(
                title="Prevent Race Conditions with select_for_update()",
                description="SELECT followed by UPDATE pattern detected - potential race condition",
                code_before="""# Race condition risk: Another process could modify between read and write
def transfer_money(from_account_id, to_account_id, amount):
    from_account = Account.objects.get(id=from_account_id)
    to_account = Account.objects.get(id=to_account_id)
    
    if from_account.balance >= amount:
        from_account.balance -= amount
        to_account.balance += amount
        from_account.save()
        to_account.save()
    else:
        raise InsufficientFunds()""",
                code_after="""# Safe: Using select_for_update to prevent race conditions
from django.db import transaction

def transfer_money(from_account_id, to_account_id, amount):
    with transaction.atomic():
        # Lock rows to prevent concurrent modifications
        from_account = Account.objects.select_for_update().get(id=from_account_id)
        to_account = Account.objects.select_for_update().get(id=to_account_id)
        
        if from_account.balance >= amount:
            from_account.balance -= amount
            to_account.balance += amount
            from_account.save()
            to_account.save()
        else:
            raise InsufficientFunds()

# Alternative: select_for_update with specific fields
with transaction.atomic():
    # Only lock specific fields if using PostgreSQL
    account = Account.objects.select_for_update(of=['balance']).get(id=account_id)
    account.balance += deposit_amount
    account.save()

# select_for_update with timeout (PostgreSQL)
with transaction.atomic():
    try:
        account = Account.objects.select_for_update(nowait=True).get(id=account_id)
        # Process immediately or skip if locked
    except DatabaseError:
        # Handle the case where row is already locked
        pass""",
                explanation="""select_for_update() creates a database lock on selected rows,
preventing other transactions from modifying them until the current transaction
commits. This prevents race conditions in read-modify-write operations.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#select-for-update",
                    "https://docs.djangoproject.com/en/stable/topics/db/transactions/"
                ],
                difficulty="medium",
                impact="high"
            )
        ]
    
    @staticmethod
    def _transaction_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for transaction issues."""
        operation_count = len(pattern.affected_queries)
        
        return [
            Recommendation(
                title="Wrap Related Database Operations in Transactions",
                description=f"Detected {operation_count} related operations that should be wrapped in a transaction",
                code_before="""# No transaction: Risk of partial completion
def create_order_with_items(order_data, items_data):
    # If any of these fail, we get inconsistent state
    order = Order.objects.create(**order_data)
    
    for item_data in items_data:
        OrderItem.objects.create(order=order, **item_data)
    
    # Update inventory
    for item_data in items_data:
        product = Product.objects.get(id=item_data['product_id'])
        product.stock -= item_data['quantity']
        product.save()
    
    # Send confirmation email
    send_order_confirmation(order)""",
                code_after="""# With transaction: All-or-nothing guarantee
from django.db import transaction

@transaction.atomic
def create_order_with_items(order_data, items_data):
    # All operations succeed or all are rolled back
    order = Order.objects.create(**order_data)
    
    # Bulk create items for better performance
    order_items = [
        OrderItem(order=order, **item_data)
        for item_data in items_data
    ]
    OrderItem.objects.bulk_create(order_items)
    
    # Update inventory in bulk
    product_ids = [item['product_id'] for item in items_data]
    products = {p.id: p for p in Product.objects.select_for_update().filter(id__in=product_ids)}
    
    for item_data in items_data:
        product = products[item_data['product_id']]
        product.stock -= item_data['quantity']
    
    Product.objects.bulk_update(products.values(), ['stock'])
    
    return order

# Manual transaction control for complex logic
def complex_operation():
    with transaction.atomic():
        # Create savepoint for partial rollback
        savepoint = transaction.savepoint()
        
        try:
            # Risky operation
            risky_database_operation()
        except SomeException:
            # Rollback to savepoint, not entire transaction
            transaction.savepoint_rollback(savepoint)
            # Handle error or try alternative
        else:
            transaction.savepoint_commit(savepoint)""",
                explanation="""Database transactions ensure ACID properties: Atomicity, Consistency,
Isolation, and Durability. Without transactions, partial failures can leave
your database in an inconsistent state.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/topics/db/transactions/",
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#bulk-update"
                ],
                difficulty="medium",
                impact="critical"
            )
        ]
    
    @staticmethod
    def _connection_pool_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for connection pool issues."""
        return [
            Recommendation(
                title="Optimize Database Connection Pool Settings",
                description="Rapid query execution detected - may exhaust connection pool",
                code_before="""# settings.py - Default connection settings (not optimized)
DATABASES = {
    'default': {
        'ENGINE': 'django.db.backends.postgresql',
        'NAME': 'mydb',
        'USER': 'myuser',
        'PASSWORD': 'mypass',
        'HOST': 'localhost',
        'PORT': '5432',
        # No connection pooling configuration
    }
}

# Code that creates many rapid connections
def process_items_inefficiently():
    for item_id in range(1000):
        # Each query might create a new connection
        item = Item.objects.get(id=item_id)
        process_item(item)""",
                code_after="""# settings.py - Optimized connection settings
DATABASES = {
    'default': {
        'ENGINE': 'django.db.backends.postgresql',
        'NAME': 'mydb',
        'USER': 'myuser',
        'PASSWORD': 'mypass',
        'HOST': 'localhost',
        'PORT': '5432',
        'CONN_MAX_AGE': 600,  # Keep connections open for 10 minutes
        'OPTIONS': {
            'MAX_CONNS': 20,  # Maximum connections in pool
            'MIN_CONNS': 5,   # Minimum connections to maintain
        },
    }
}

# Better: Batch processing to reduce connection pressure
def process_items_efficiently():
    # Process in batches to reuse connections
    batch_size = 100
    for start in range(0, 1000, batch_size):
        end = start + batch_size
        items = Item.objects.filter(id__range=(start, end))
        
        # Process batch with single connection
        for item in items:
            process_item(item)

# Alternative: Use connection pooling library
# pip install django-db-pool
DATABASES = {
    'default': {
        'ENGINE': 'dj_db_conn_pool.backends.postgresql',
        'POOL_OPTIONS': {
            'POOL_SIZE': 10,
            'MAX_OVERFLOW': 10,
            'RECYCLE': 24 * 60 * 60,  # 24 hours
        }
        # ... other settings
    }
}""",
                explanation="""Connection pooling reuses database connections instead of creating
new ones for each query. This reduces overhead and prevents connection pool
exhaustion under high load.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/ref/databases/#connection-management",
                    "https://docs.djangoproject.com/en/stable/ref/settings/#conn-max-age"
                ],
                difficulty="medium",
                impact="high"
            )
        ]
    
    @staticmethod
    def _distinct_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for inefficient distinct usage."""
        return [
            Recommendation(
                title="Optimize DISTINCT Usage for Better Performance",
                description="Inefficient DISTINCT usage detected",
                code_before="""# Inefficient: DISTINCT on all columns
products = Product.objects.distinct()

# Inefficient: DISTINCT without proper indexes
categories = Product.objects.values('category').distinct()

# Inefficient: DISTINCT after complex joins
results = Order.objects.select_related('customer', 'items__product').distinct()""",
                code_after="""# Efficient: DISTINCT on specific fields only
categories = Product.objects.values_list('category', flat=True).distinct()

# Better: Use distinct with specific fields (PostgreSQL)
products = Product.objects.distinct('category')

# Even better: Use aggregation when appropriate
category_counts = Product.objects.values('category').annotate(
    count=Count('id')
).order_by('category')

# For complex queries, consider filtering first
# Instead of distinct on joined data:
recent_orders = Order.objects.filter(
    created_at__gte=last_week
).select_related('customer')

# Get distinct customers separately if needed
customer_ids = recent_orders.values_list('customer_id', flat=True).distinct()
customers = Customer.objects.filter(id__in=customer_ids)

# Alternative: Use prefetch_related for better performance
orders_with_items = Order.objects.prefetch_related(
    'items__product'
).filter(created_at__gte=last_week)""",
                explanation="""DISTINCT operations can be expensive, especially on all columns.
It's often better to be specific about which fields need to be distinct,
or to restructure queries to avoid the need for DISTINCT.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#distinct",
                    "https://docs.djangoproject.com/en/stable/topics/db/aggregation/"
                ],
                difficulty="medium",
                impact="medium"
            )
        ]
    
    @staticmethod
    def _values_optimization_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for values() optimization."""
        return [
            Recommendation(
                title="Use values() or values_list() for Field-Specific Queries",
                description="Query fetching many fields when only a few are needed",
                code_before="""# Inefficient: Loading full model instances when only need specific fields
user_emails = []
for user in User.objects.all():
    user_emails.append(user.email)  # Only need email, but loading entire User object

# Inefficient: Using all fields for simple operations
def get_user_names():
    names = []
    for user in User.objects.filter(is_active=True):
        names.append(f"{user.first_name} {user.last_name}")
    return names

# Inefficient: Loading relations when not needed
products = Product.objects.select_related('category').all()
for product in products:
    print(product.name)  # Not using category data""",
                code_after="""# Efficient: Use values_list for single fields
user_emails = User.objects.values_list('email', flat=True)

# Efficient: Use values for multiple fields
user_names = User.objects.filter(is_active=True).values_list(
    'first_name', 'last_name'
)
formatted_names = [f"{first} {last}" for first, last in user_names]

# Efficient: Only select needed fields
product_info = Product.objects.values('id', 'name', 'price')
for product in product_info:
    print(f"{product['name']}: ${product['price']}")

# Efficient: Combine with annotations for calculated fields
user_stats = User.objects.values('first_name', 'last_name').annotate(
    order_count=Count('orders'),
    total_spent=Sum('orders__total')
).filter(is_active=True)

# Efficient: Use iterator for large datasets
def process_user_emails():
    for email in User.objects.values_list('email', flat=True).iterator():
        send_newsletter(email)""",
                explanation="""values() and values_list() return dictionaries/tuples instead of model
instances, using less memory and reducing database transfer time. This is
especially important when processing large datasets or when you only need
specific fields.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#values",
                    "https://docs.djangoproject.com/en/stable/ref/models/querysets/#values-list"
                ],
                difficulty="easy",
                impact="medium"
            )
        ]
    
    @staticmethod
    def _redundant_queries_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for redundant queries."""
        duplicate_count = len(pattern.affected_queries)
        
        return [
            Recommendation(
                title="Eliminate Redundant Database Queries",
                description=f"Same query executed {duplicate_count} times - cache results",
                code_before="""# Redundant: Same query multiple times
def get_user_permissions(user_id):
    user = User.objects.get(id=user_id)
    return user.user_permissions.all()

def view_function(request):
    user_id = request.user.id
    
    # Each call makes the same database query
    perms1 = get_user_permissions(user_id)
    perms2 = get_user_permissions(user_id)  # Redundant!
    perms3 = get_user_permissions(user_id)  # Redundant!
    
    # Process permissions
    return render(request, 'template.html', {
        'can_edit': 'edit' in perms1,
        'can_delete': 'delete' in perms2,
        'can_view': 'view' in perms3,
    })""",
                code_after="""# Efficient: Cache the result
def get_user_permissions(user_id, _cache={}):
    if user_id not in _cache:
        user = User.objects.get(id=user_id)
        _cache[user_id] = user.user_permissions.all()
    return _cache[user_id]

# Better: Store in variable
def view_function(request):
    user_id = request.user.id
    
    # Query once, reuse result
    permissions = get_user_permissions(user_id)
    permission_names = set(permissions.values_list('codename', flat=True))
    
    return render(request, 'template.html', {
        'can_edit': 'edit' in permission_names,
        'can_delete': 'delete' in permission_names,
        'can_view': 'view' in permission_names,
    })

# Best: Use Django's built-in caching
from django.core.cache import cache

def get_user_permissions_cached(user_id):
    cache_key = f'user_permissions_{user_id}'
    permissions = cache.get(cache_key)
    
    if permissions is None:
        user = User.objects.get(id=user_id)
        permissions = list(user.user_permissions.values_list('codename', flat=True))
        cache.set(cache_key, permissions, timeout=300)  # Cache for 5 minutes
    
    return permissions""",
                explanation="""Redundant queries waste database resources and slow down your application.
Caching query results, either in variables or using Django's cache framework,
eliminates unnecessary database calls.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/topics/cache/",
                    "https://docs.djangoproject.com/en/stable/topics/db/optimization/#don-t-retrieve-things-you-don-t-need"
                ],
                difficulty="easy",
                impact="medium"
            )
        ]
    
    @staticmethod
    def _query_caching_recommendations(pattern: DetectedPattern) -> List[Recommendation]:
        """Generate recommendations for query result caching."""
        return [
            Recommendation(
                title="Implement Query Result Caching for Expensive Operations",
                description="Expensive queries detected that would benefit from caching",
                code_before="""# Expensive query repeated multiple times
def get_popular_products():
    return Product.objects.annotate(
        order_count=Count('orderitem'),
        avg_rating=Avg('reviews__rating')
    ).filter(
        order_count__gt=100,
        avg_rating__gte=4.0
    ).order_by('-order_count')

def homepage_view(request):
    popular = get_popular_products()  # Expensive query
    # ... other logic
    
def api_view(request):
    popular = get_popular_products()  # Same expensive query!
    # ... API logic""",
                code_after="""# Efficient: Cache expensive query results
from django.core.cache import cache
from django.conf import settings
import hashlib

def get_popular_products():
    # Create cache key based on query
    cache_key = 'popular_products_v1'
    
    # Try to get from cache first
    popular = cache.get(cache_key)
    
    if popular is None:
        # Cache miss - execute expensive query
        popular = Product.objects.annotate(
            order_count=Count('orderitem'),
            avg_rating=Avg('reviews__rating')
        ).filter(
            order_count__gt=100,
            avg_rating__gte=4.0
        ).order_by('-order_count')
        
        # Convert to list to make it serializable
        popular = list(popular)
        
        # Cache for 1 hour
        cache.set(cache_key, popular, timeout=3600)
    
    return popular

# Advanced: Cache with parameters
def get_products_by_category(category_slug, min_rating=4.0):
    # Create unique cache key including parameters
    cache_key = f'products_{category_slug}_{min_rating}'
    
    products = cache.get(cache_key)
    if products is None:
        products = list(Product.objects.filter(
            category__slug=category_slug,
            avg_rating__gte=min_rating
        ).select_related('category'))
        
        cache.set(cache_key, products, timeout=1800)  # 30 minutes
    
    return products

# Cache invalidation when data changes
from django.db.models.signals import post_save, post_delete
from django.dispatch import receiver

@receiver([post_save, post_delete], sender=Product)
def invalidate_product_cache(sender, **kwargs):
    # Clear related cache entries when products change
    cache.delete_pattern('popular_products_*')
    cache.delete_pattern('products_*')

# Using cache_page decorator for view-level caching
from django.views.decorators.cache import cache_page

@cache_page(60 * 15)  # Cache for 15 minutes
def product_list_view(request, category_slug):
    products = get_products_by_category(category_slug)
    return render(request, 'products.html', {'products': products})""",
                explanation="""Query result caching stores expensive query results in memory (Redis/Memcached)
or database cache. This dramatically improves performance for queries that are
expensive but don't change frequently.""",
                references=[
                    "https://docs.djangoproject.com/en/stable/topics/cache/",
                    "https://docs.djangoproject.com/en/stable/topics/db/optimization/#use-queryset-extra"
                ],
                difficulty="medium",
                impact="critical"
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