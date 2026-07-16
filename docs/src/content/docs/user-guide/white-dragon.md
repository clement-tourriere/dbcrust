---
title: "White Dragon Search"
description: Query White Dragon's generic WDSP v6 field and index schemas through SQL, qualifiers, or structured JSON.
---

# White Dragon Search

Connect DBCrust to a current White Dragon HTTP server:

```sh
dbcrust white-dragon://127.0.0.1:7700
```

This integration targets White Dragon's schema API v3 and generic WDSP v6 model. White
Dragon has one logical SQL table, `documents`. Field names, index paths, types, stored
values, sort columns, and aggregation columns all come from the mapping embedded in the
served splits—there are no built-in Git or patch aliases.

## Browse the served schema

```text
\d documents
```

On connection, DBCrust caches `GET /v1/schema`. The response contains every distinct
complete schema in the current standalone or distributed inventory. DBCrust unions their
source fields for browsing and completion while preserving all WDSP types:

- `text` and `keyword` as multi-valued `TEXT[]` and `KEYWORD[]`;
- `i64`, `u64`, and `f64` without losing width or signedness;
- `bool`, `datetime`, and `json`;
- conflicting definitions across heterogeneous schemas as `VARIANT<...>`.

`\d documents` also lists mapping-defined indexes such as `body.tokens` or
`patch.secrets`, including their index kind, case-folding, tokenizer, transforms, and
default-search status. Fields and index paths feed SQL completion; qualifier completion
uses only actual indexes plus valid wildcard paths such as `body.*`.

## Qualifier searches

A bare search uses the mapping's `default_search` set. Qualifiers name an index path:

```sh
dbcrust white-dragon://127.0.0.1:7700 --read-only \
  -c 'headline:storage OR visibility:public'

dbcrust white-dragon://127.0.0.1:7700 --read-only \
  -c 'patch.code:credential AND NOT patch.secrets:false-positive'

dbcrust white-dragon://127.0.0.1:7700 --read-only \
  -c 'body.*:/Immutable\s+splits/ case:sensitive'
```

A `field.*` qualifier searches that field's primary and sub-indexes independently. The
only schema-independent qualifier control is `case`; every searchable name otherwise
comes from `/v1/schema`.

## SQL searches and aggregations

White Dragon's compact SQL front end uses the same mapping-defined field/index paths.
DBCrust also supports selecting one or more stored source fields; it expands the hit query
for White Dragon and applies the requested projection locally:

```sql
SELECT headline, priority FROM documents
WHERE headline MATCH 'Dragon'
LIMIT 20;

SELECT * FROM documents
WHERE body.tokens MATCH 'Immutable'
  AND priority >= 8
  AND visibility IN ('public', 'internal')
LIMIT 20;

SELECT visibility, COUNT(*) FROM documents
WHERE headline MATCH 'Dragon'
GROUP BY visibility
ORDER BY COUNT(*) DESC
LIMIT 10;

SELECT month(published), COUNT(*) FROM documents
GROUP BY month(published)
ORDER BY month(published) DESC
LIMIT 12;
```

Pattern operators require textual indexes, equality requires an exact-compatible index,
and ordered comparisons require a range index. Aggregations require a compatible
columnar source field.

## Structured request policies

Policies not expressed by hit SQL—such as named-column result ordering—can be sent as the
complete `/v1/search` JSON object:

```sh
dbcrust white-dragon://127.0.0.1:7700 --read-only -o json -c '{
  "q": "headline:Dragon",
  "limit": 10,
  "sort": {"named_field": "rating", "direction": "desc"}
}'
```

Structured aggregations work with qualifier or SQL query strings:

```json
{
  "q": "headline:Dragon",
  "aggregation": {
    "terms": {"field": "visibility", "size": 10, "order": "count_desc"}
  }
}
```

```json
{
  "q": "headline:Dragon",
  "aggregation": {
    "date_histogram": {
      "field": "published",
      "interval": "month",
      "size": 12,
      "order": "key_desc"
    }
  }
}
```

## Result shape

For `SELECT *`, DBCrust flattens the hit envelope into `split_id`, `doc_id`, `id`, and
`score`, followed by every stored mapping field, then `snippet` and the matched `index`.
An explicit select list returns only those stored fields in the requested order. Optional
fields missing from a hit are SQL NULL. Text and keyword arrays and JSON values remain JSON
text in table cells. Empty result sets retain the appropriate projected or full schema.

If a mapping field collides with an envelope name such as `score` or `index`, DBCrust
displays it as `fields.score` or `fields.index` so both values remain visible.

White Dragon is search-only, so DBCrust's `--read-only` guard accepts SQL, qualifier, and
structured JSON searches while rejecting an empty request. Servers without schema API v3
are rejected instead of being interpreted with stale aliases or sampled metadata.
