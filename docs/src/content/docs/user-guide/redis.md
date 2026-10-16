---
title: "Redis & Valkey"
description: Query Redis (and Valkey, KeyDB, Dragonfly) with redis-cli commands, browse keys with SCAN, and use --read-only safely.
---

# Redis & Valkey

DBCrust talks to Redis as a regular connection. There is no SQL: you type the commands you
would type in `redis-cli`, and the replies come back as tables you can sort, export with
`-o json` / `-o csv`, or pipe.

```sh
dbcrust redis://localhost:6379
dbcrust redis://localhost:6379/2 -c 'HGETALL user:42'
dbcrust 'rediss://app:secret@cache.example.com:6380/0' --read-only -c 'INFO keyspace'
```

Redis joined the **default build** (`bundled`) in this release; the `standard` preset does not
contain it. See [Build features](/user-guide/development/#build-features).

## Connecting

```text
redis://[user[:password]@]host[:port][/db]
rediss://[user[:password]@]host[:port][/db]        # TLS
redis+unix:///path/to/redis.sock?db=2&user=app     # unix socket (also unix://)
```

| Part | Meaning |
|---|---|
| `host`, `port` | default `127.0.0.1:6379`; IPv6 literals in brackets (`redis://[::1]:6379`) |
| `user` | the **ACL user** (Redis 6+); omit for the `default` user |
| `password` | prompted for and saved to the secret store when the server asks for one — same flow as the other engines (`\savepass`, `session://`) |
| `/db` | the logical database number, `0` by default |
| `rediss://` | TLS with certificate and hostname verification (no way to turn them off); `?tls=true` also works |
| `valkey://`, `valkeys://` | aliases of `redis://`, `rediss://` — Valkey, KeyDB and Dragonfly speak the same protocol |

Options in the query string: `connect_timeout=<seconds>` (default 10), `max_rows=<n>` (rows
rendered from one reply, default 10000), `scan_limit=<n>` (keys listed by `\dt`, default
1000), `sample_size=<n>` (members shown by `\d key`, default 20).

Saved sessions keep the user, the database number, TLS and the socket path; the password
stays in the secret store, bound to that exact host, port, database, user and TLS mode.
Redis servers in Docker are found by image name (`redis`, `valkey`, `keydb`, `dragonfly`);
`docker://my-redis` works and reads `REDIS_PASSWORD` from the container environment.

## Commands

The query is **one command line**, split exactly like `redis-cli` does:

```text
GET user:42
SET "greeting" "hello world"
SET k 'single quoted'
SET k "tab\there \"quoted\" \x41"
SCAN 0 MATCH user:* COUNT 100
HGETALL user:42
ZRANGE leaderboard 0 9 WITHSCORES
```

* whitespace separates arguments; `"double quotes"` honour `\n \r \t \b \a \\ \" \xHH`;
  `'single quotes'` only honour `\'`;
* arguments are binary-safe (`\xff` is a byte);
* a trailing `;` is the statement terminator, not part of the last argument;
* in scripts (`-f file`, stdin) **one command per line**; blank lines and `#` comments are skipped.

Not supported because they change how the shared connection behaves: `SUBSCRIBE`/`PSUBSCRIBE`/
`SSUBSCRIBE`, `MONITOR`, `SYNC`/`PSYNC`, `MULTI`/`EXEC`/`WATCH`, `SELECT` (use `\c 3` or the
`/3` of the URL), `AUTH`, `HELLO`, `CLIENT REPLY|TRACKING|CACHING`. They answer *Feature not
supported* without being sent. Use `redis-cli` for those.

Ctrl-C and the `query_timeout` setting stop waiting for a command. Redis cannot cancel a
command that is already running, so a slow `KEYS *` keeps running on the server.

## How replies become tables

| Reply | Columns |
|---|---|
| string, integer, `OK`, double, boolean | `value` |
| nil | `value` = `NULL` |
| list of values | `index`, `value` (a set: `value`) |
| `HGETALL`, `CONFIG GET`, `XINFO STREAM`, RESP3 maps | `field`, `value` |
| `… WITHSCORES`, `ZPOPMIN`/`ZPOPMAX` | `member`, `score` |
| `SCAN`, `SSCAN` | `cursor`, `value` |
| `HSCAN` / `ZSCAN` | `cursor`, `field`, `value` / `cursor`, `member`, `score` |
| `XRANGE`, `XREVRANGE` | `id`, `field`, `value` (one row per field) |
| `XREAD` | `stream`, `id`, `field`, `value` |
| `XINFO GROUPS`, `XINFO CONSUMERS` | one column per field |
| `INFO` | `section`, `key`, `value` |
| anything nested | the nested part as JSON text in the `value` cell |

Binary values that are not UTF-8 are shown with `\xNN` escapes instead of being dropped. A
server error (`WRONGTYPE`, `NOPERM`, …) is a query error with the server's message.

The driver reads a whole reply before returning it, so `max_rows` bounds what DBCrust
*renders*, not what the server sends: bound the request itself for big collections
(`LRANGE k 0 99`, `HSCAN k 0 COUNT 100`, `XRANGE s - + COUNT 50`). A reply cut by `max_rows`
is logged (`tracing` warning) and flagged by the library API (`RedisClient::execute_limited`).

## Browsing keys

Redis has no tables; DBCrust shows the keyspace of the connected database:

| Command | What it does |
|---|---|
| `\dt` | up to `scan_limit` keys found with `SCAN` (**never `KEYS`**), with each key's `TYPE` in the Type column |
| `\d user:42` | the key's `type`, `ttl`, `memory` (`MEMORY USAGE`), `length` (`STRLEN`/`LLEN`/`HLEN`/`SCARD`/`ZCARD`/`XLEN`), `encoding`, then a sample of `sample_size` members (hash fields, list elements, set/zset members with score, stream entries) |
| `\l` | databases that hold keys, with their key counts |
| `\c 3` | connect to database 3 (a new connection; `SELECT` is not sent) |

Tab completion offers the common command names at the start of a line and the sampled key
names afterwards. Keys with spaces or quotes are inserted quoted.

## Read-only

`--read-only` (and `read_only_default`) accept only commands on an **allowlist**; everything
else — every write, every administration command, every command DBCrust has never heard of,
module commands included — is refused with exit code 4.

Allowed: `GET MGET STRLEN GETRANGE SUBSTR LCS EXISTS TYPE TTL PTTL EXPIRETIME PEXPIRETIME DUMP
RANDOMKEY DBSIZE SCAN SORT_RO`, bitmaps/HyperLogLog/geo reads (`BITCOUNT BITPOS GETBIT
BITFIELD_RO PFCOUNT GEOPOS GEODIST GEOHASH GEOSEARCH GEORADIUS_RO`), hashes (`HGET HMGET HGETALL
HKEYS HVALS HLEN HEXISTS HSTRLEN HRANDFIELD HSCAN`), lists (`LRANGE LLEN LINDEX LPOS`), sets
(`SMEMBERS SCARD SISMEMBER SMISMEMBER SRANDMEMBER SINTER SUNION SDIFF SINTERCARD SSCAN`), sorted
sets (`ZRANGE ZRANGEBYSCORE ZRANGEBYLEX ZREV* ZCARD ZCOUNT ZLEXCOUNT ZSCORE ZMSCORE ZRANK
ZRANDMEMBER ZINTER ZUNION ZDIFF ZSCAN`), streams (`XRANGE XREVRANGE XLEN XPENDING`, `XREAD`
without `BLOCK`, `XINFO STREAM|GROUPS|CONSUMERS`), and server reads (`PING ECHO TIME INFO
LASTSAVE ROLE`, `MEMORY USAGE|STATS|DOCTOR`, `OBJECT ENCODING|IDLETIME|REFCOUNT|FREQ`, `CLIENT
INFO|GETNAME|ID`, `COMMAND …`, `PUBSUB CHANNELS|NUMSUB|NUMPAT`).

Always refused, with a reason: `FLUSHALL`, `FLUSHDB`, `CONFIG` (including `GET`), `DEBUG`,
`SHUTDOWN`, `MODULE`, `SCRIPT`, `FUNCTION`, `EVAL*`, `FCALL*`, `KEYS` (use `SCAN`), `MONITOR`,
`SUBSCRIBE` and friends, `PUBLISH`, every blocking command (`BLPOP`, `BRPOP`, `BLMOVE`,
`BZPOP*`, `XREAD … BLOCK`, `WAIT`), `SORT` (it can `STORE`; use `SORT_RO`), `ACL`, `AUTH`,
`HELLO`, `SELECT`, `MULTI`/`EXEC`/`WATCH`, `SLOWLOG`.

This is a client-side textual guard. For a hard guarantee connect as an ACL user that can
only read:

```text
ACL SETUSER dbcrust-ro on >PASSWORD ~* resetchannels -@all +@read -@dangerous +info +scan +type +ttl +pttl +object|encoding +memory|usage
```

Connections built for the credential broker (`dbcrust_harden_read_only`) apply the same
allowlist inside the driver.

:::note[Agents]
The credential broker does not enroll Redis yet (`unsupported_engine: Redis enrollment for
agents is not supported yet`): agent access needs a command classifier of the broker's own.
Until then agents can only use Redis through a connection you run with `--read-only`.
:::

## Not supported yet

Redis Cluster and Sentinel, client TLS certificates and private-CA bundles (`rediss://`
verifies against the platform and Mozilla roots), `insecure` TLS, RESP3, Pub/Sub, transactions,
per-command cancellation, `EXPLAIN`.
