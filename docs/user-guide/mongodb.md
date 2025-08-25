# MongoDB Guide

DBCrust provides comprehensive MongoDB support with a familiar SQL-like interface that translates to native MongoDB operations. This guide covers everything from basic connections to advanced querying and database management.

## 🚀 Getting Started with MongoDB

### Connection Methods

DBCrust supports multiple MongoDB connection methods:

=== "Standard MongoDB"

    ```bash
    # Local MongoDB instance
    dbcrust mongodb://localhost:27017/myapp

    # With authentication
    dbcrust mongodb://user:password@localhost:27017/myapp

    # With connection options
    dbcrust mongodb://user:pass@localhost:27017/myapp?authSource=admin
    ```

=== "MongoDB Atlas"

    ```bash
    # MongoDB Atlas with SRV record
    dbcrust mongodb+srv://user:password@cluster.mongodb.net/myapp

    # Atlas with specific options
    dbcrust mongodb+srv://user:pass@cluster.mongodb.net/myapp?retryWrites=true
    ```

=== "Docker Container"

    ```bash
    # Interactive container selection
    dbcrust docker://

    # Direct MongoDB container connection
    dbcrust docker://mongodb-dev
    dbcrust docker://mongo-container/myapp
    ```

=== "Session Management"

    ```bash
    # Save MongoDB connection as session
    dbcrust mongodb://localhost:27017/myapp
    \ss mongo_local

    # Reconnect using session
    dbcrust session://mongo_local
    ```

### First Steps

After connecting to MongoDB, explore your database:

```sql
-- List all collections (equivalent to "tables" in SQL)
\collections

-- Examine a collection's structure
\dc users

-- List database statistics
\mstats

-- Simple query to see data
SELECT * FROM users LIMIT 5;
```

## 🔍 SQL-to-MongoDB Query Translation

DBCrust's key feature is translating familiar SQL syntax into MongoDB operations seamlessly.

### Basic SELECT Queries

```sql
-- SQL syntax that everyone knows
SELECT * FROM users;

-- Translates to MongoDB find operation
-- db.users.find({})
```

### Advanced WHERE Clause Support

#### Comparison Operators

```sql
-- Equality and comparison
SELECT * FROM users WHERE age = 25;
-- → {"age": 25}

SELECT * FROM products WHERE price > 100;
-- → {"price": {"$gt": 100}}

SELECT * FROM orders WHERE created_at >= '2024-01-01';
-- → {"created_at": {"$gte": "2024-01-01"}}
```

#### LIKE Pattern Matching

```sql
-- SQL LIKE patterns translate to MongoDB regex
SELECT * FROM users WHERE name LIKE 'John%';
-- → {"name": {"$regex": "John.*", "$options": "i"}}

SELECT * FROM products WHERE description LIKE '%wireless%';
-- → {"description": {"$regex": ".*wireless.*", "$options": "i"}}

SELECT * FROM codes WHERE code LIKE 'A_B%';
-- → {"code": {"$regex": "A.B.*", "$options": "i"}}
```

**Pattern Translation:**
- `%` (wildcard) → `.*` (regex any characters)
- `_` (single char) → `.` (regex single character)
- Case-insensitive by default with `$options: "i"`
- Regex special characters are automatically escaped

#### IN Operator

```sql
-- Multiple value matching
SELECT * FROM orders WHERE status IN ('pending', 'processing', 'shipped');
-- → {"status": {"$in": ["pending", "processing", "shipped"]}}

SELECT * FROM users WHERE age IN (18, 21, 25, 30);
-- → {"age": {"$in": [18, 21, 25, 30]}}
```

#### OR Conditions

```sql
-- Multiple conditions with OR logic
SELECT * FROM users WHERE age > 65 OR status = 'premium';
-- → {"$or": [{"age": {"$gt": 65}}, {"status": "premium"}]}

SELECT * FROM products WHERE price < 10 OR category = 'clearance';
-- → {"$or": [{"price": {"$lt": 10}}, {"category": "clearance"}]}
```

#### BETWEEN Range Queries

```sql
-- Range queries for numeric and date values
SELECT * FROM products WHERE price BETWEEN 100 AND 500;
-- → {"price": {"$gte": 100, "$lte": 500}}

SELECT * FROM events WHERE event_date BETWEEN '2024-01-01' AND '2024-12-31';
-- → {"event_date": {"$gte": "2024-01-01", "$lte": "2024-12-31"}}
```

#### NULL Handling

```sql
-- Check for missing or null fields
SELECT * FROM users WHERE email IS NULL;
-- → {"$or": [{"email": null}, {"email": {"$exists": false}}]}

SELECT * FROM profiles WHERE bio IS NOT NULL;
-- → {"bio": {"$exists": true, "$ne": null}}
```

### Complex Query Examples

```sql
-- Combining multiple conditions
SELECT * FROM shipwrecks
WHERE feature_type LIKE '%Visible%'
  AND depth BETWEEN 0 AND 50
  AND watlev IN ('always dry', 'covers and uncovers')
  OR coordinates IS NOT NULL;

-- Pattern matching with range filtering
SELECT * FROM products
WHERE name LIKE 'iPhone%'
  AND price BETWEEN 500 AND 1500
  AND status != 'discontinued';

-- Multi-condition search
SELECT * FROM users
WHERE (age > 18 OR verified = true)
  AND email IS NOT NULL
  AND role IN ('admin', 'moderator', 'premium');
```

## 🛠 Database Management

DBCrust supports MongoDB database and collection management using familiar SQL syntax.

### Database Operations

```sql
-- Create a new database
CREATE DATABASE analytics;
-- MongoDB creates databases implicitly when first collection is created

-- Drop an existing database
DROP DATABASE old_database;
-- Permanently removes the database and all its collections

-- Switch to a different database
\c production
```

### Collection Operations

```sql
-- Create a new collection
CREATE COLLECTION user_profiles;
-- Creates an empty collection in the current database

-- Drop an existing collection
DROP COLLECTION temp_data;
-- Permanently removes the collection and all its documents

-- List all collections
\collections
```

### Collection Management Workflow

```sql
-- Connect to MongoDB
dbcrust mongodb://localhost:27017/myapp

-- Create a new database
CREATE DATABASE ecommerce;

-- Switch to the new database
\c ecommerce

-- Create collections for our application
CREATE COLLECTION users;
CREATE COLLECTION products;
CREATE COLLECTION orders;
CREATE COLLECTION reviews;

-- Verify collections were created
\collections

-- Clean up test collections later
DROP COLLECTION temp_testing;
```

## 📊 MongoDB-Specific Commands

Beyond SQL translation, DBCrust provides native MongoDB commands for advanced operations.

### Collection Exploration

```sql
-- List all collections with details
\collections

-- Describe collection structure and sample data
\dc users

-- Get detailed database statistics
\mstats
```

### Index Management

```sql
-- List all indexes in the database
\dmi

-- Create an index on a field
\cmi users email
\cmi products name

-- Create a text index for search
\cmi articles content text
\cmi products description text

-- Drop an index
\ddmi users email_1
```

### Text Search

DBCrust provides powerful text search capabilities:

```sql
-- Search for documents containing specific terms
\search articles "mongodb tutorial"

-- Search across multiple fields
\search products "wireless bluetooth"

-- Search with complex terms
\search users "john developer senior"
```

**Text Search Features:**
- Full-text search using MongoDB `$text` operator
- Automatic index utilization if text indexes exist
- Multi-word search term support
- Results limited to 10 by default for performance

### Advanced MongoDB Queries

For complex operations, use native MongoDB syntax:

```sql
-- MongoDB find with specific filter
\find users {"active": true, "age": {"$gte": 18}}

-- MongoDB aggregation pipeline
\aggregate orders [
  {"$match": {"status": "completed"}},
  {"$group": {"_id": "$user_id", "total": {"$sum": "$amount"}}},
  {"$sort": {"total": -1}},
  {"$limit": 10}
]

-- Complex find with projection and limit
\find products {"category": "electronics"} {"name": 1, "price": 1} 20
```

## 🏗 Data Types and Schema Handling

MongoDB's flexible schema is fully supported by DBCrust with intelligent type handling.

### Automatic Type Detection

DBCrust automatically detects and handles MongoDB's rich data types:

```javascript
// Example MongoDB document
{
  "_id": ObjectId("507f1f77bcf86cd799439011"),
  "name": "John Doe",           // String
  "age": 30,                    // Integer
  "salary": 75000.50,           // Double
  "active": true,               // Boolean
  "tags": ["developer", "senior"], // Array
  "profile": {                  // Nested Document
    "bio": "Software engineer",
    "location": "San Francisco"
  },
  "created_at": ISODate("2024-01-15T10:30:00Z"), // Date
  "metadata": null              // Null
}
```

### Column Detection and Display

DBCrust samples documents to create a unified column view:

```sql
SELECT * FROM users LIMIT 3;
```

**Output:**
```
╭─────────────────────┬────────────┬─────┬─────────────┬────────┬─────────────────┬──────────────────────┬─────────────────────╮
│ _id                 │ name       │ age │ salary      │ active │ tags            │ profile              │ created_at          │
├─────────────────────┼────────────┼─────┼─────────────┼────────┼─────────────────┼──────────────────────┼─────────────────────┤
│ 507f1f77bcf86cd7... │ John Doe   │ 30  │ 75000.5     │ true   │ ["developer"... │ {"bio":"Software"... │ 2024-01-15T10:30... │
│ 507f1f88bcf86cd7... │ Jane Smith │ 28  │ 82000.0     │ true   │ ["manager",...  │ {"bio":"Product"...  │ 2024-01-14T15:22... │
│ 507f1f99bcf86cd7... │ Bob Wilson │ 35  │ 68000.0     │ false  │ []              │ {}                   │ 2024-01-13T09:15... │
╰─────────────────────┴────────────┴─────┴─────────────┴────────┴─────────────────┴──────────────────────┴─────────────────────╯
```

### Handling Missing Fields

MongoDB's flexible schema means documents may have different fields:

```sql
-- Query handles documents with missing fields gracefully
SELECT name, email, phone FROM users;
-- Shows empty string for missing fields
```

## 🔧 Advanced Features

### ObjectId Handling

DBCrust automatically handles MongoDB ObjectIds:

```sql
-- Query by ObjectId (automatically detected and converted)
SELECT * FROM users WHERE _id = '507f1f77bcf86cd799439011';
-- → {"_id": ObjectId("507f1f77bcf86cd799439011")}

-- ObjectIds display as hex strings for readability
```

### Array and Nested Document Querying

```sql
-- Query array fields
SELECT * FROM users WHERE tags IN ('developer', 'senior');
-- Works with MongoDB array matching

-- Query nested document fields (use dot notation in native MongoDB commands)
\find users {"profile.location": "San Francisco"}
```

### Performance Optimization

#### Index Usage

```sql
-- Create indexes for better query performance
\cmi users email
\cmi orders user_id
\cmi products {"name": 1, "category": 1}  -- Compound index

-- Create text indexes for search
\cmi articles content text
```

#### Query Limits

```sql
-- Always use LIMIT for large collections
SELECT * FROM large_collection LIMIT 100;

-- Default limits are applied automatically for safety
```

## 🐳 Docker Integration

DBCrust seamlessly integrates with MongoDB Docker containers:

### Container Discovery

```bash
# Interactive container selection
dbcrust docker://

# Expected output:
# Available database containers:
# 1. postgres-dev (postgres:13) - Port 5432 → 5433
# 2. mysql-test (mysql:8.0) - Port 3306 → 3307
# 3. mongodb-cache (mongo:7) - Port 27017 → 27018
# 4. clickhouse-analytics (clickhouse:latest) - Port 8123 → 8124
#
# Select container (1-4): 3
```

### Supported MongoDB Images

DBCrust automatically detects these MongoDB container images:

- `mongo` (official MongoDB)
- `mongodb` (alternative official)
- `bitnami/mongodb` (Bitnami MongoDB)
- Any image containing "mongo" in the name

### Docker Environment Variables

DBCrust reads standard MongoDB Docker environment variables:

| Variable | Purpose | Example |
|----------|---------|---------|
| `MONGO_INITDB_ROOT_USERNAME` | Root username | `admin` |
| `MONGO_INITDB_ROOT_PASSWORD` | Root password | `secret123` |
| `MONGO_INITDB_DATABASE` | Initial database | `myapp` |

## 🔒 Authentication and Security

### Connection String Authentication

```bash
# Basic authentication
dbcrust mongodb://user:password@localhost:27017/myapp

# Authentication database specification
dbcrust mongodb://user:pass@localhost:27017/myapp?authSource=admin

# SSL/TLS connection
dbcrust mongodb://user:pass@localhost:27017/myapp?ssl=true
```

### Session-based Security

```bash
# Save secure production connection
dbcrust mongodb+srv://user:pass@cluster.mongodb.net/prod
\ss mongo_production

# Reconnect without exposing credentials
dbcrust session://mongo_production
```

## 🚨 Troubleshooting

### Common Connection Issues

#### Connection Refused

```bash
# Error: Connection refused
# Solution: Check if MongoDB is running
docker ps | grep mongo
mongosh --eval "db.adminCommand('ping')"
```

#### Authentication Failed

```bash
# Error: Authentication failed
# Solutions:
# 1. Check credentials
dbcrust mongodb://correct_user:correct_pass@localhost:27017/myapp

# 2. Specify authentication database
dbcrust mongodb://user:pass@localhost:27017/myapp?authSource=admin
```

#### Database Not Found

```sql
-- Error: Database doesn't exist
-- Solution: Create the database first
CREATE DATABASE myapp;
\c myapp
```

### Query Issues

#### Empty Results

```sql
-- If queries return no results, check:
-- 1. Collection exists
\collections

-- 2. Collection has data
SELECT COUNT(*) FROM users;

-- 3. Query conditions are correct
SELECT * FROM users LIMIT 5;
```

#### Performance Issues

```sql
-- For slow queries:
-- 1. Check if indexes exist
\dmi

-- 2. Create appropriate indexes
\cmi users email
\cmi orders user_id

-- 3. Use LIMIT for large result sets
SELECT * FROM large_collection LIMIT 100;
```

## 📋 Best Practices

### Query Optimization

1. **Always use LIMIT** for exploratory queries:
   ```sql
   SELECT * FROM users LIMIT 10;
   ```

2. **Create indexes** for frequently queried fields:
   ```sql
   \cmi users email
   \cmi orders {"user_id": 1, "created_at": -1}
   ```

3. **Use specific field selection** when possible:
   ```sql
   SELECT name, email FROM users WHERE active = true;
   ```

### Database Design

1. **Use meaningful collection names:**
   ```sql
   CREATE COLLECTION user_profiles;  -- Good
   CREATE COLLECTION data;          -- Bad
   ```

2. **Create appropriate indexes early:**
   ```sql
   CREATE COLLECTION users;
   \cmi users email
   \cmi users {"name": 1, "created_at": -1}
   ```

3. **Plan for text search needs:**
   ```sql
   CREATE COLLECTION articles;
   \cmi articles content text
   \cmi articles title text
   ```

### Session Management

1. **Save frequently used connections:**
   ```sql
   dbcrust mongodb://localhost:27017/myapp
   \ss local_dev

   dbcrust mongodb+srv://user:pass@cluster.mongodb.net/prod
   \ss production
   ```

2. **Use descriptive session names:**
   ```bash
   dbcrust session://mongo_local_dev    # Good
   dbcrust session://db1               # Bad
   ```

## 🎯 Complete Workflow Example

Here's a complete example showing MongoDB usage in DBCrust:

```bash
# 1. Connect to MongoDB
dbcrust mongodb://localhost:27017/ecommerce

# 2. Explore the database
\collections
\mstats

# 3. Examine collection structures
\dc users
\dc products
\dc orders

# 4. Create indexes for performance
\cmi users email
\cmi products name
\cmi orders user_id
\cmi products description text

# 5. Query with advanced filtering
SELECT * FROM users
WHERE email LIKE '%@company.com'
  AND active = true
  AND role IN ('admin', 'manager')
LIMIT 20;

# 6. Perform text search
\search products "wireless bluetooth headphones"

# 7. Use native MongoDB aggregation
\aggregate orders [
  {"$match": {"status": "completed"}},
  {"$group": {"_id": "$product_id", "total_sales": {"$sum": "$amount"}}},
  {"$sort": {"total_sales": -1}},
  {"$limit": 10}
]

# 8. Database management
CREATE COLLECTION user_sessions;
DROP COLLECTION temp_imports;

# 9. Save session for future use
\ss ecommerce_local
```

---

<div align="center">
    <strong>Ready to master MongoDB with DBCrust?</strong><br>
    <a href="/dbcrust/reference/backslash-commands/" class="md-button md-button--primary">Command Reference</a>
    <a href="/dbcrust/reference/url-schemes/" class="md-button">Connection Guide</a>
</div>
