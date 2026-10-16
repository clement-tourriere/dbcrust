---
title: "SQL Server & Azure SQL"
description: Connect to Microsoft SQL Server and Azure SQL with T-SQL, TLS by default, named instances, SHOWPLAN plans and a --read-only guard.
---

# SQL Server & Azure SQL

DBCrust talks to Microsoft SQL Server and Azure SQL over TDS (developed and tested against
SQL Server 2022), with **SQL authentication**. You type T-SQL; results come back as tables you can sort, export
with `-o json` / `-o csv`, or pipe.

```sh
dbcrust 'mssql://sa:Passw0rd!@localhost:1433/master'
dbcrust 'mssql://app@sql.example.com/Sales' --read-only -c 'SELECT TOP 10 * FROM dbo.Orders'
dbcrust 'sqlserver://app:secret@myserver.database.windows.net/mydb?encrypt=strict'
```

SQL Server joined the **default build** (`bundled`) in this release; the `standard` preset
does not contain it. See [Build features](/user-guide/development/#build-features).

## Connecting

```text
mssql://[user[:password]@]host[:port][/database][?options]
sqlserver://…                      # same thing
mssql://user@host%5CINSTANCE/db    # named instance (also ?instance=INSTANCE)
```

| Part | Meaning |
|---|---|
| `host`, `port` | default port `1433`; IPv6 literals in brackets (`mssql://sa@[::1]:1433`) |
| `user`, `password` | a **SQL Server login**. The password is prompted for and saved to the secret store when the server rejects the login — same flow as the other engines (`\savepass`, `session://`). Percent-encode special characters (`p%40ss`) |
| `/database` | the database to open; empty opens the login's default database |
| `\INSTANCE` | a named instance. A backslash cannot sit in a URL host, so write `%5C` — DBCrust also accepts a literal `host\INSTANCE` on the command line. The instance's port is resolved through **SQL Server Browser** (UDP 1434); you cannot combine an instance with a port |

Options in the query string (case-insensitive):

| Option | Default | Effect |
|---|---|---|
| `encrypt` | `true` | `true`: the whole session over TLS with the certificate **verified**; `false`: only the login exchange is encrypted (the server may still insist on more); `strict`: TLS before the TDS handshake (TDS 8.0, SQL Server 2022+ / Azure). `mssql+tls://` is a spelling of `encrypt=true` |
| `trustServerCertificate` | `false` | `true` accepts **any** certificate, so anyone on the network path can impersonate the server. Logged as a warning on every connection. Never use it across an untrusted network |
| `ca_cert` | – | path of a PEM or DER file holding the CA to trust *in addition to* the system roots. The safe answer for a private CA or a self-signed development server. Cannot be combined with `trustServerCertificate` |
| `hostNameInCertificate` | – | the name the server certificate was issued to, when it differs from `host` |
| `application_name` | `dbcrust` | shows up in `sys.dm_exec_sessions.program_name` |
| `intent` | – | `readonly` sets `ApplicationIntent=ReadOnly`, which only **routes** to a readable availability-group secondary; it is not read-only enforcement |
| `connect_timeout` | `15` | seconds for TCP connect + TLS + login |
| `max_rows` | `100000` | rows kept from one result set; `0` = unlimited (see [Limits](#limits-timeouts-and-cancellation)) |

Because certificates are verified by default, a stock Docker or developer SQL Server (which
presents a self-signed certificate) is refused with a message that names the ways forward.
Prefer installing a proper certificate and pinning its CA with `?ca_cert=/path/to/ca.pem`.
SQL Server's *auto-generated* certificate is an old X.509 version that rustls rejects even
when pinned, so for that one the only option is `?trustServerCertificate=true` — on a
network you trust.

A saved session keeps the host, port or instance, login, database and the `encrypt` /
`trustServerCertificate` choices. The password stays in the secret store, **bound to that exact
host, port, database, login, TLS mode, instance and certificate-trust choice**: a stored
password is never offered to another instance of the same host, nor to a connection that
stopped verifying the certificate. SQL Server containers in Docker are found by image name
(`mcr.microsoft.com/mssql/server`, `azure-sql-edge`); `docker://my-mssql/master` reads
`MSSQL_SA_PASSWORD` from the container environment and uses the `sa` login. The container's
certificate is self-signed, so a development container needs
`docker://my-mssql/master?trustServerCertificate=true`.

## Running T-SQL

One connection serves the whole session, so `SET` options, temporary tables (`#t`),
variables and open transactions persist from one statement to the next, as in `sqlcmd`:

```sql
CREATE TABLE #big (id int);
INSERT INTO #big VALUES (1), (2);
SELECT COUNT(*) FROM #big;
```

* Statements are separated by `;` in scripts (`-f`, stdin); the `GO` batch separator is a
  `sqlcmd`/SSMS feature and is **not** part of T-SQL, so DBCrust does not interpret it.
* A batch returns its **first result set**. Further result sets are read and dropped (a
  warning is logged); later statements of the batch still run.
* A statement without a result set answers `Command(s) completed successfully.`; a plain
  single `INSERT`/`UPDATE`/`DELETE`/`MERGE` answers a `Rows affected` count.
* `USE otherdb` works and updates the prompt; `\c otherdb` opens a fresh session on that
  database instead.
* The default row limit (`default_limit`, `--max-rows N`) is applied as `SELECT TOP (n)` to a plain single
  `SELECT` (T-SQL has no `LIMIT`); queries that already bound themselves (`TOP`, `OFFSET …
  FETCH`) or combine several (`UNION`, `INTO`, `FOR JSON`) are left alone.

### Plans: `EXPLAIN`

`EXPLAIN SELECT …`, the `\e` explain mode and the AI assistant's `explain` tool compile the
statement under `SET SHOWPLAN_XML ON` and return the **estimated plan XML** (a `QUERY PLAN`
column). The statement is *not executed*, so it is safe to plan a write. `EXPLAIN ANALYZE`
(actual plans) is not supported. The interactive plan tree is PostgreSQL-only; SQL Server plans
are shown as XML you can open in SSMS (`.sqlplan`) or paste into Plan Explorer.

## Values and types

A cell is SQL `NULL` exactly when the server sent NULL; an empty string stays an empty string.

| SQL Server type | Shown as |
|---|---|
| `tinyint`, `smallint`, `int`, `bigint` | the number |
| `bit` | `1` / `0` |
| `decimal(p,s)`, `numeric(p,s)` | exact, with the scale's trailing zeros (`12.3450`); no floating point involved |
| `float`, `real` | shortest round-trip form (`1.5`, `0.1`) |
| `money`, `smallmoney` | four decimals (`12.5000`) |
| `char`, `varchar`, `nchar`, `nvarchar`, `text`, `ntext` | the text; a cell is cut at 1,000,000 characters with its size noted |
| `date` | `2024-02-29` |
| `time(n)` | `13:14:15.123` (`n` fraction digits) |
| `datetime2(n)` | `2024-02-29 13:14:15.1234567` |
| `datetime`, `smalldatetime` | `2024-02-29 13:14:15.003`, `2024-02-29 13:14:00` |
| `datetimeoffset(n)` | local time and offset, as SSMS shows it (`2024-02-29 13:14:15 +05:30`) |
| `uniqueidentifier` | upper-case, hyphenated (`6F9619FF-8B86-D011-B42D-00C04FC964FF`) |
| `binary`, `varbinary`, `image` | `0x…` upper-case hex; cut at 1,024 bytes with the total size |
| `xml` | the XML text |

CLR types (`geography`, `geometry`, `hierarchyid`) are not decoded; select them through
`.ToString()` or `STAsText()` for now.

## Browsing

Metadata comes from the catalog views of the current database (`sys.schemas`, `sys.objects`,
`sys.columns`, `sys.indexes`, `sys.foreign_keys`, `sys.check_constraints`); user-controlled
names always travel as parameters.

| Command | What it does |
|---|---|
| `\l` | databases (`sys.databases`) with state, collation and compatibility level |
| `\dt` | tables and views; objects of schemas other than `dbo` are listed as `schema.name` |
| `\d` | lists the tables and views |
| `\d table` / `\d schema.table` | columns with type, nullability, defaults (`IDENTITY(1,1)`, computed `AS …`), then indexes (as `CREATE INDEX` statements), check constraints, outgoing foreign keys and the tables that reference this one. Names are bracket-quoted: `\d [Order Items]` |
| `\du` | server logins (`sys.server_principals`) |
| `\di` | indexes of all user tables |
| `\c db` | open a new session on database `db` |

Tab completion knows the tables and columns of the current database and quotes names that
need it.

## Read-only

`--read-only` (and `read_only_default`) accept only a **single `SELECT`** (or a `WITH … SELECT`;
an `EXPLAIN` in front is the plan shorthand above). Anything else is refused with exit code 4.
The guard lexes T-SQL — `-- line` and nested `/* … */` comments, `'strings'` with `''`,
`N'…'`, `[bracketed]` and `"quoted"` identifiers — so keywords inside strings, comments or
quoted names do not count.

Refused wherever they appear:

* **writes and DDL**: `INSERT UPDATE DELETE MERGE TRUNCATE CREATE ALTER DROP`,
  `SELECT … INTO` (any `INTO`), `GRANT DENY REVOKE`;
* **procedures**: `EXEC`/`EXECUTE`, every `sp_*` and `xp_*` name (so `xp_cmdshell` and
  `sp_executesql` can never be reached, even as `[xp_cmdshell]` or `master.dbo.xp_cmdshell`);
* **rowsets that leave the database**: `OPENROWSET`, `OPENQUERY`, `OPENDATASOURCE`, `OPENXML`,
  `BULK`, four-part (linked-server) names, and the file readers `fn_get_audit_file`,
  `fn_trace_*`, `fn_xe_*`, `fn_dump_dblog`;
* **administration and session control**: `DBCC BACKUP RESTORE KILL SHUTDOWN RECONFIGURE
  CHECKPOINT WAITFOR`, `USE`, `SET`, `DECLARE`, transactions (`BEGIN COMMIT ROLLBACK SAVE`),
  flow control (`IF WHILE GOTO RETURN`), `PRINT RAISERROR THROW`, cursors (`OPEN CLOSE
  DEALLOCATE`), Service Broker (`RECEIVE`, `SEND ON`, `END/GET/MOVE CONVERSATION`),
  `ENABLE`/`DISABLE TRIGGER`;
* **side effects of a query**: `NEXT VALUE FOR` (advances a sequence) and the `UPDLOCK`,
  `XLOCK`, `TABLOCKX` table hints;
* more than one statement (`SELECT 1; SELECT 2`), the `GO` separator, unterminated strings,
  comments or brackets.

Reserved words cannot be unquoted identifiers, so a legitimate query never trips these
rules; to select a column that is named like a keyword, bracket it (`[update]`). The `OPTION
(USE HINT …)` / `USE PLAN` query hints contain `USE` and are refused too. Join hints such as
`MERGE JOIN` are fine.

Connections built for the credential broker (`dbcrust_harden_read_only`) apply the same guard
inside the driver.

:::caution[It is a text check, not a permission]
SQL Server has **no session-level read-only mode** (`ApplicationIntent=ReadOnly` only routes
connections). A `SELECT` can still call a scalar CLR function with side effects and can read
whatever the login can read. For a hard guarantee connect as a login that cannot write:

```sql
CREATE LOGIN dbcrust_ro WITH PASSWORD = '…';
CREATE USER dbcrust_ro FOR LOGIN dbcrust_ro;
ALTER ROLE db_datareader ADD MEMBER dbcrust_ro;
DENY INSERT, UPDATE, DELETE, EXECUTE, ALTER ON SCHEMA::dbo TO dbcrust_ro;
```

or point DBCrust at a `READ_ONLY` database or a readable secondary.
:::

:::note[Agents]
The credential broker does not enroll SQL Server yet (`unsupported_engine: SQL Server
enrollment for agents is not supported yet: use a read-only login and wait for the read-only
probe`). Enrollment needs a probe that proves the login cannot write, which is follow-up work.
Until then agents can only use SQL Server through a connection you run with `--read-only`
*and* a read-only login.
:::

## Limits, timeouts and cancellation

* **Row cap**: `max_rows` (default 100 000) bounds the rows read from the first result set.
  When it is reached the server is told to stop sending (a cancel) and DBCrust logs a warning;
  the rest of that batch is cancelled with it.
* **Statement timeout**: the `query_timeout` setting (and, for broker-built connections,
  the hardening timeout) is enforced by the client. A statement past its deadline is
  **cancelled with a TDS attention signal**, which the server acknowledges; the session
  survives. If the server does not answer within 5 seconds the connection is dropped and the
  next statement reconnects (temp tables and open transactions are lost with it).
* **Ctrl-C** cancels the running statement the same way.
* A SQL error (syntax, permissions, constraint) is a query error with the server's message,
  number and line; the session stays usable. A broken connection is reconnected on the next
  statement.

## Not supported yet

* **Windows / integrated authentication** (Kerberos, NTLM) and **Azure Active Directory**
  sign-in (access tokens): SQL logins only.
* Client TLS certificates, `MultiSubnetFailover`, `encrypt=strict` on servers that do not
  support TDS 8.0.
* Named instances through an SSH tunnel (the instance is resolved over UDP; tunnel the
  instance's own TCP port with a plain `host:port` URL instead).
* CLR and spatial types, `sql_variant` details, table-valued parameters, bulk insert.
* `EXPLAIN ANALYZE` / actual plans and the interactive plan visualizer (estimated plan XML only).
* Multiple result sets (the first is returned).
* Agent enrollment through the credential broker.
