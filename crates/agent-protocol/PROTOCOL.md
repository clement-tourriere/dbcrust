# DBCrust agent ⇄ broker protocol (version 1)

The wire contract between the credential-owning **broker** (`dbcrust broker
serve`) and the credential-free **thin client** (`dbcrust-agent`). Implemented
by the `dbcrust-agent-protocol` crate; this document is normative for other
implementations.

There is **deliberately no field** that can carry a URL, a host override, a
credential or a password. A client can only name aliases the database owner
enrolled. A unit test scans every message type for credential-looking keys.

## Transport

A byte stream — a Unix domain socket. One conversation per connection; the
client sends requests one at a time and the broker answers them in order.

## Framing

```
+--------------------------+-----------------------------+
| length : u32 big-endian  | body : UTF-8 JSON, `length` |
+--------------------------+-----------------------------+
```

* Maximum body size: **16 MiB** (`16 * 1024 * 1024` bytes, `MAX_FRAME_LEN`).
* A larger announced length MUST be rejected **before** allocating the body;
  the receiver answers `protocol_error` (when it still can) and closes.
* A stream ending inside a header or body is a truncated frame (connection error).
* Bodies are single JSON objects with a `"type"` discriminator (snake_case).
  An unknown `type` or an invalid body is `protocol_error`.

## Handshake and versioning

`PROTOCOL_VERSION = 1`. The first client frame MUST be `hello`; the broker
answers `welcome`. If `protocol` differs from the broker's version the broker
answers `error` with code `unsupported_version` and closes the connection.
Any other first message is a `protocol_error`.

## Client → broker

| `type` | fields | notes |
|---|---|---|
| `hello` | `protocol: u32`, `client: string` | `client` is a free-form label, audit only |
| `list_connections` | `id: u64` | |
| `list_tables` | `id`, `alias` | |
| `describe_table` | `id`, `alias`, `table` | `table` may be schema-qualified |
| `query` | `id`, `alias`, `sql`, `max_rows?: u32`, `timeout_secs?: u32` | limits can only be lowered, never raised, relative to the alias's |
| `explain` | `id`, `alias`, `sql` | answered with `query_result` |

## Broker → client

| `type` | fields |
|---|---|
| `welcome` | `protocol: u32`, `broker_version: string` |
| `connections` | `id`, `connections: ConnectionSummary[]` |
| `tables` | `id`, `tables: TableSummary[]` |
| `table_description` | `id`, `table: string`, `columns: ColumnInfo[]` |
| `query_result` | `id`, `columns: string[]`, `rows: (string\|null)[][]`, `row_count: u64`, `truncated: bool` |
| `error` | `id?: u64`, `code: ErrorCode`, `message: string` |

Every response except `welcome` carries the request's `id`. `error` omits `id`
(or sends none) only when the failure is not tied to a request: handshake
failures and undecodable frames.

```jsonc
// ConnectionSummary
{"alias": "prod", "engine": "postgresql", "access": "read_only",   // read_only | read_write
 "native_read_only": true, "description": "replica", "max_rows": 500, "timeout_secs": 30}
// TableSummary           (schema, kind optional)
{"schema": "public", "name": "users", "kind": "table"}
// ColumnInfo             (nullable, default optional)
{"name": "id", "data_type": "bigint", "nullable": false, "default": null, "primary_key": true}
```

Result cells are strings (the broker renders values); `null` is SQL NULL.
`truncated: true` means rows beyond the alias's row limit were dropped.

## Error codes

`unknown_alias`, `denied`, `read_only_violation`, `ddl_refused`,
`multi_statement_refused`, `limit_exceeded`, `timeout`, `query_error`,
`connection_error`, `broker_locked`, `unsupported_engine`, `protocol_error`,
`unsupported_version`, `internal`.

## Example

```
C→B  {"type":"hello","protocol":1,"client":"claude-code"}
B→C  {"type":"welcome","protocol":1,"broker_version":"0.38.0"}
C→B  {"type":"query","id":1,"alias":"prod","sql":"SELECT id FROM users","max_rows":10}
B→C  {"type":"query_result","id":1,"columns":["id"],"rows":[["1"],["2"]],"row_count":2,"truncated":false}
C→B  {"type":"query","id":2,"alias":"prod","sql":"DROP TABLE users"}
B→C  {"type":"error","id":2,"code":"ddl_refused","message":"DDL statements are refused for this alias"}
```
(each line above is one frame, prefixed on the wire by its 4-byte length)
