# DBCrust Audit — June 2026

Full-project audit covering the Rust core, all database backends, the CLI/REPL,
the new AI-queries feature (branch `ai_queries`), the Python/Django toolkit,
the Tauri GUI, and engineering health (tests/CI/docs/packaging). Findings were
produced by seven parallel deep-dive reviews plus compiler/test ground truth
(`cargo clippy`, `cargo test`, `ruff`, `tsc`), each finding verified against
the actual code path.

**Severity scale**: P0 = security/data-loss/crash · P1 = wrong behavior users
hit · P2 = quality/UX defect · P3 = polish.

**Status**: ✅ fixed in this pass (see "Fixes applied" below) · ⏳ open.

---

## Executive summary

The architecture is sound — the strum-driven command system, the
`DatabaseClient` trait layer, genai delegation for AI, and the GUI's
lazy-loaded views are all good bones. The problems are concentrated in four
places:

1. **Silent data corruption in query rewriting** (✅ fixed): the auto-LIMIT
   appended `LIMIT 100` to anything *containing* "select" — `INSERT … SELECT`
   silently inserted 100 rows, `DELETE`s with subqueries broke, and the Python
   API returned truncated results.
2. **The Django toolkit's core promises were broken**: the N+1 detector never
   matched real Django SQL (✅ fixed), several detectors flagged *every*
   request (✅ worst ones fixed), the middleware was unsafe under any
   concurrency (✅ fixed), `transaction_safe=True` silently rolled back user
   writes (✅ default flipped), and the EXPLAIN pipeline is dead end-to-end
   (⏳ needs the redesign below).
3. **Nothing enforced quality**: ~359 Rust tests and ~155 Python tests exist,
   but no CI workflow ran any of them; the Python suite wasn't even runnable
   from a fresh clone (✅ CI added, suite now runs and passes).
4. **First-hour crashes/landmines in the REPL and the new AI feature**:
   Tab-completion stack overflow on `\ns` (✅), `\config` printing decrypted
   Vault passwords (✅), `\ai setup` deadlocking the REPL (✅), Ctrl-C killing
   the process mid-query (⏳ design below), inverted exit codes in `-c` mode
   (✅).

Counts from the audit: 6 P0s (all fixed), ~25 P1s (13 fixed, 12 open with fix
sketches below), ~35 P2/P3s (12 fixed, rest catalogued).

---

## Fixes applied in this pass

All applied to the working tree on `ai_queries`, uncommitted. Verified by
`cargo test` (all green), the Python suite (155 passed — it could not run at
all before), `cargo clippy`, and `tsc`.

### Rust core
| Severity | Fix | Where |
|---|---|---|
| P0 | Auto-LIMIT only applies to statements whose *leading keyword* is SELECT; word-boundary LIMIT detection; unit tests for INSERT…SELECT / CTAS / DELETE-subquery / data-modifying CTE | `src/db.rs` (`add_default_limit`, `leading_sql_keyword`) |
| P0 | `\config` no longer Debug-dumps the whole `Config` (it embedded **decrypted Vault passwords**); curated summary instead | `src/commands.rs` (`ShowConfig`) |
| P0 | `\ns` Tab-completion infinite mutual recursion (stack-overflow abort) — rerouted to direct argument completion | `src/completion.rs` |
| P0 | `\ai setup` / `\ai model` REPL deadlock: config `MutexGuard` created in the match scrutinee outlived the arms that re-lock it | `src/cli_core.rs` |
| P1 | URL username/password were sent **percent-encoded** to the server (`p%40ss` instead of `p@ss`); now decoded like the database name | `src/database.rs` |
| P1 | Config file was regenerated on **every launch** (`has_all_fields` required a `[connection]` section the writer never writes) and the rewrite silently dropped `[complex_display]`; both fixed | `src/config.rs` |
| P1 | `-c` exit codes: SQL errors exited 0 (broke `dbcrust -c … && deploy`), `\q` exited 1; now non-zero on failure, clean stop on `\q` | `src/cli_core.rs`, `CommandModeOutcome` |
| P1 | `query_timeout_seconds` config was never read (hardcoded 30s); now wired into the PostgreSQL execution paths, `0` disables | `src/database.rs`, `src/database_postgresql.rs`, `src/cli_core.rs` |
| P1 | Empty-Enter silently re-executed the loaded script (`\ed`/`\i`) forever; buffer now cleared after one run | `src/cli_core.rs` |
| P1 | App dumped real query data to `dbcrust_data_analysis.txt` / `dbcrust_format_crash.txt` in the user's CWD; now logged via tracing only | `src/format.rs` |
| P2 | `sqlite:///opt/data/app.db` was treated as *relative* unless the path started with /home, /Users, /tmp, /var; single leading `/` is now uniformly absolute; paths percent-decoded | `src/database.rs` |
| P2 | `.pgpass` was created world-readable then chmod'd (TOCTOU window); now created `0600` atomically | `src/pgpass.rs` |

### Backends / display
| Severity | Fix | Where |
|---|---|---|
| P1 | 9 raw byte-slice truncations panicked on multibyte UTF-8 (accents/CJK/emoji crashed the whole query); char-safe `truncate_str_bytes` helper + tests | `src/complex_display.rs`, `src/json_display.rs`, `src/database_elasticsearch.rs` |
| P1 | DataFusion rendered Date/Timestamp/Decimal/List/Struct columns as the **entire column's debug dump in every cell**; replaced with Arrow's per-row `ArrayFormatter` (also fixes all missing types) | `src/database_datafusion.rs` |
| P1 | MongoDB `\d` indexes queried `system.indexes` (removed in MongoDB 3.0 — always empty); now uses `list_indexes()` with name/unique/primary mapping | `src/database_mongodb.rs` |
| P1 | Table/schema names interpolated unescaped into metadata SQL across MySQL/SQLite/ClickHouse (broke on names with quotes; injection foothold); literals escaped, SQLite identifiers double-quoted | `src/database_{mysql,sqlite,clickhouse}.rs`, helpers in `src/database.rs` |
| P3 | `get_server_info` reported hardcoded "DataFusion 50.0.0" (real: 53); now uses `DATAFUSION_VERSION` | `src/database_datafusion.rs` |

### AI feature / schema TUI
| Severity | Fix | Where |
|---|---|---|
| P1 | `is_select_query` read-only guard was a prefix check — `WITH d AS (DELETE …) SELECT`, `SELECT 1; DROP TABLE t`, and `EXPLAIN ANALYZE DELETE` all auto-executed in AutoSelect mode; now rejects write keywords and multi-statements (tests added) | `src/ai/streaming.rs` |
| P2 | User could be asked to confirm SQL they never saw (`show_generated_sql=false`); SQL now always printed before any confirm, and Confirm defaults to **No** for writes | `src/cli_core.rs` |
| P2 | Curated "Missing API key — run `\ai setup`" error was unreachable (auth resolver swallowed it); pre-flight added | `src/ai/mod.rs` |
| P2 | `\ai setup` env-var hint printed the **full API key** to the terminal (scrollback/tmux logs); placeholder now | `src/ai/key_storage.rs` |
| P3 | `extract_sql` only stripped ```` ```sql ```` fences — ```` ```postgresql ```` etc. produced guaranteed syntax errors; any fence tag now stripped | `src/ai/streaming.rs` |
| P3 | Schema TUI: integer underflow panic when a pane is squeezed below 2 rows (`saturating_sub`); Ctrl-C toggled constraint display instead of quitting (now always quits) | `src/schema_tui/ui.rs`, `app.rs` |

### Python / Django
| Severity | Fix | Where |
|---|---|---|
| P0 | One shared analyzer/collector served all concurrent requests (threaded runserver/gunicorn/ASGI corrupted or lost results); analyzer is now created **per request**, report uses the request's own queries, double-`__exit__` made idempotent | `python/dbcrust/django/middleware.py`, `analyzer.py` |
| P0 | `transaction_safe` defaulted to **True** — `with analyze():` silently rolled back every write made inside; default flipped to False, loud warnings documented, middleware warns if enabled | `analyzer.py`, `middleware.py` |
| P0 | `dbcrust.django.transaction()` promised commit/rollback but both were no-ops (silent non-atomicity); now raises `NotImplementedError` pointing to `django.db.transaction.atomic`; `Connection.rollback()` likewise | `database_helper.py`, `connector.py` |
| P1 | N+1 detector **never fired on real Django SQL** (regexes didn't tolerate quoted/qualified columns or `%s` placeholders) — verified, fixed, plus FK-field extraction bug (`_ID` capture group) and `rstrip('s')` mangling ('address'→'addre') | `pattern_detector.py` |
| P1 | False-positive storm: `_is_count_only_pattern` (operator-precedence bug) flagged every SELECT; `_is_foreign_key_lookup` flagged every query pair; both constrained to plausible shapes | `pattern_detector.py` |
| P1 | Django app config declared `name='dbcrust'` → `dbcrust_analyze` and the django-side commands were undiscoverable, duplicate-label crash if both apps listed; now `name='dbcrust.django'`, `label='dbcrust_django'`; stale `default_app_config` removed | `django/apps.py`, `__init__.py` |
| P2 | Packaging: `[tool.maturin.dependencies]` (not a real key) removed; duplicate console-scripts removed; `dbcrust[django]` extra + dev dependency-group added; demo/test/scratch files excluded from the wheel; `abi3-py38` → `abi3-py310` to match `requires-python >=3.10` | `pyproject.toml`, `Cargo.toml` |
| P2 | Grade D/F reports logged at ERROR (one Sentry event per slow dev page); capped at WARNING. Severity `max()` compared strings alphabetically ("medium" outranked "critical"); ranked properly | `middleware.py`, `analyzer.py` |
| P2 | Test suite was unrunnable (no pytest/django anywhere, `mise run py:test` pointed at a nonexistent dir); root `conftest.py` stubs the native module when unbuilt, mise task fixed, `[tool.pytest.ini_options]` added — **155 tests now run and pass** | `conftest.py`, `mise.toml`, `pyproject.toml` |

### GUI
| Severity | Fix | Where |
|---|---|---|
| P0 | Recent-connections clicked by **filtered** index but resolved against the full list — searching could connect you to a different database; original index now carried through | `ConnectionDialog.tsx`, `SavedConnections.tsx` |
| P1 | Connecting from the Saved Connections view failed **silently** (no error prop/UI); error banner added | `SavedConnections.tsx`, `App.tsx` |
| P1 | Schema Explorer "Quick Queries" buttons all loaded the same default SELECT regardless of label; each now loads the SQL it displays | `SchemaExplorer.tsx`, `App.tsx` |
| P2 | `useState(newTab())` evaluated on every render → tab titles like "Query 53"; lazy initializer. Menu accelerator bypassed the running-state guard → concurrent/duplicate execution; `isRunning` guard added | `App.tsx` |

### Engineering health
| Severity | Fix | Where |
|---|---|---|
| P1 | **No CI quality gates existed** (4 workflows only build/package/deploy); added `ci.yml`: rustfmt + clippy `-D warnings` + cargo test (ubuntu/macos), pytest + ruff error-class, GUI tsc+build via bun | `.github/workflows/ci.yml` |
| P2 | `Cargo.toml` lacked `rust-version` (edition 2024 needs ≥1.85 — `cargo install` failed cryptically), `license`, `repository`, `readme`; added | `Cargo.toml` |
| P2 | CLAUDE.md/AGENTS.md misled tooling: wrong storage filenames (`sessions.toml`/`recent.toml` are real), nonexistent `tests/` dir, stale "known failing test" (it passes), `pre-commit` (hooks are hk), wrong docs path; all corrected | `CLAUDE.md`, `AGENTS.md` |
| — | Existing clippy warnings fixed so CI can enforce `-D warnings` | `src/pgpass.rs` |

---

## Open findings (prioritized, with fix sketches)

### A. CLI/REPL — the gap to "psql replacement"

1. **[P1] Ctrl-C kills the whole CLI mid-query; the interrupt plumbing is dead
   code.** `interrupt_flag` is threaded through every API but *nothing ever
   sets it* — no `ctrlc`/`tokio::signal` handler exists anywhere
   (`src/cli_core.rs:1040`). The same flag is decorative for AI streaming
   (`src/ai/streaming.rs`), so cancelling generation also kills the process
   and leaves the terminal in dim mode.
   *Fix sketch*: install `tokio::signal::ctrl_c()` once at REPL start; race
   query futures via `tokio::select!`; on interrupt, issue a server-side
   cancel (sqlx exposes the PG backend PID — `SELECT pg_cancel_backend($1)`
   on a second pool connection). For AI: `generate_handle.abort()` + treat
   partial output as cancelled (never offer it for execution —
   `src/cli_core.rs:1449-1466` currently would, with default Yes).

2. **[P1] Interactive transactions/`SET` are unreliable: every statement runs
   on a fresh pooled connection** (`database_postgresql.rs` `fetch_all(&self.pool)`,
   pool of 8). `BEGIN`/`INSERT`/`COMMIT` can land on three different backends;
   completion metadata queries share the pool and can run inside a user's
   open transaction. No transaction state in the prompt.
   *Fix sketch*: pin one dedicated `PoolConnection` for the REPL session
   (metadata keeps the pool); render `=*>`/`=!>` prompt states. This is the
   single highest-leverage CLI architecture change.

3. **[P1] No multiline continuation and no multi-statement execution.** No
   reedline `Validator`, so Enter submits incomplete SQL; multi-statement
   buffers fail in sqlx's prepared path ("cannot insert multiple commands");
   `\i` only loads the buffer (docs say it executes).
   *Fix sketch*: implement `Validator` signalling Incomplete until a
   statement-terminating `;` outside strings/comments/dollar-quotes (the
   sql_parser modules already exist); split statements and execute
   sequentially; make `\i` run statement-by-statement.

4. **[P1] `\s <name>` is a stub** — prints "Session found" instead of
   connecting (`commands.rs:1492`); docs claim it connects. There is no
   in-REPL way to switch servers at all.
   *Fix sketch*: rebuild `Database` from the session URL and swap it into
   `db_arc` (the reconnect logic exists in `cli_core::handle_database_connection`).

5. **[P2] Completion blocks the REPL thread on live DB round-trips with no
   timeout, and caches never invalidate after DDL** (`completion.rs:172-263`).
   `metadata_timeout_seconds` is still unused. *Fix*: wrap fetches in
   `tokio::time::timeout`, clear caches after any DDL executes.

6. **[P2] Backslash-command output bypasses the pager** (`cli_core.rs` —
   `CommandResult::Output` is printed directly; a 500-table `\dt` scrolls).
   One-line: route through `Self::page_or_print`.

7. **[P2] Named-query args split on whitespace** (quoted args unreachable —
   `cli_core.rs:216`, `commands.rs:683`), `\ns` collapses whitespace inside
   string literals (`commands.rs:697,805`), `\find` breaks on any JSON with
   spaces (`commands.rs:1080`). *Fix*: shell-style tokenizer for args; store
   the raw remainder for `\ns`; balanced-brace parsing for `\find`.

8. **[P2] Table alignment breaks on non-ASCII** (byte length used as display
   width, `format.rs:31-46,199-213`). *Fix*: `unicode-width` (textwrap
   already pulls it in).

9. **[P2] History capped at 50 entries, not configurable**
   (`cli_core.rs:1131`); cleanup functions exist but are never called.

10. **[P2] "connection refused" is treated as an auth failure** → password
    prompt when the server is simply down (`cli_core.rs:673-694`). Prefer
    typed driver error codes (`28P01` etc.).

11. **[P3] Vault lease handling is fiction**: real `lease_duration`/`lease_id`
    are discarded, 3600s hardcoded, no renewal call exists, and the
    "needs renewal" check underflows once expired (`vault_client.rs:513-555`,
    `config.rs:2516-2540`). Deserialize the real lease and renew via
    `sys/leases/renew`.

12. **[P3] Latent deadlock in `DatabaseAwareCompleter`** (re-locks the same
    mutex on the same thread; currently unreachable —
    `command_completion.rs:175-217`). Drop the outer guard before the inner
    lock before any refactor makes it reachable.

### B. AI feature (remaining)

13. **[P2] MongoDB prompt instructs JSON-array pipelines the executor cannot
    run** (`prompt_templates.rs:81-89` vs `database_mongodb.rs:1336`): every
    `??` against MongoDB fails. Change the prompt to emit
    `db.<collection>.aggregate(...)` syntax.
14. **[P2] Schema context rebuilds with up to 50 sequential
    `get_table_details` round-trips per `??`, holding the DB mutex, no
    caching, no per-table column cap** (`schema_context.rs:55-90`). Cache DDL
    keyed by (db, table), invalidate on `\c`.
15. **[P2] AI conversation history survives `\c`** — follow-ups reference the
    old database's schema (`cli_core.rs`). Clear on connection change.
16. **[P2] genai pulls a second reqwest (0.13) + `aws-lc-sys` + `cmake`**,
    contradicting the repo's explicit ring-only/cross-compile constraints
    (Cargo.toml comments). Track upstream for a ring feature or patch;
    document the ARM64-cross regression. The `=0.7.0-beta.2` exact pin needs
    a tracking task.
17. **[P3] Schema TUI**: failed detail loads retry a blocking DB query every
    render frame (~10×/s — `schema_tui/app.rs:81-90`; cache failures);
    initial selection is a header row; original panic hook is dropped after
    first `\sv`.

### C. Django toolkit (remaining — see rework plan)

18. **[P1] The EXPLAIN pipeline is dead end-to-end** (the feature the package
    is named for): (a) `query.sql` contains raw `%s` placeholders — EXPLAIN
    is a syntax error for any parameterized query; (b) for placeholder-free
    queries `rows[0][0]` is the header row "QUERY PLAN" → `json.loads` fails;
    (c) failures are logged at DEBUG so nobody sees them
    (`dbcrust_integration.py:87-139`). `query_plan_analyzer.py` (560 lines)
    is unreachable. *Fix direction is in the rework plan below: run EXPLAIN
    through Django's own connection with bound params.*
19. **[P1] PyO3 surface holds the GIL across every blocking call** (no
    `py.allow_threads` anywhere in `src/lib.rs`) — one slow query stalls all
    Python threads; per-object Tokio runtimes; `close()` never closes;
    `PyCursor` stringifies all values. Wrap `block_on` in `allow_threads`,
    share a lazy global runtime, implement real `close()`.
20. **[P1] Factually wrong advice still shipped** in `recommendations.py`:
    `'MAX_CONNS'/'MIN_CONNS'` (invalid psycopg options — copying them breaks
    the app), `select_for_update(of=['balance'])` (FieldError), bare
    `.distinct('field')` without order_by (ProgrammingError),
    `cache.delete_pattern` (django-redis-only), ORDER-BY index advice that
    extracts the *table* name as the field. Delete/correct each.
21. **[P2] Per-query `traceback.extract_stack()` with no cap** (50–200ms
    middleware overhead on query-heavy pages; unbounded `self.queries`).
    Depth-limit + cap with truncation flag.
22. **[P2] Duplicate detection ignores params** (N+1 loops are reported as
    "duplicates" with caching advice; `get_normalized_sql` is literally
    `pass`). Key on (sql, params).

### D. GUI (remaining — see rework plan)

23. **[P1] No cancellation, no timeout, global `op_lock` serializes
    everything** — a slow query freezes autocomplete, sidebar, and even
    Disconnect; the Settings "query timeout" field is fiction
    (`gui/src-tauri/src/lib.rs:29,57-101,919-941`). The core already has
    `execute_query_with_interrupt` — wire it, add a Cancel button, scope the
    lock to mutating ops.
24. **[P1] NULL and empty string are indistinguishable** (`rows: string[][]`;
    backend serializes NULL as `""`, grid renders `""` as *NULL*). Change IPC
    rows to `(string | null)[][]`.
25. **[P1] One shared CodeMirror instance across tabs: undo bleeds one tab's
    SQL into another** (verified against the installed @uiw/react-codemirror —
    doc swaps enter the history). Keep an `EditorState` per tab.
26. **[P1] Cmd+Enter double-bound** (native menu accelerator runs the whole
    buffer; editor keymap runs the statement under cursor; hint bar promises
    the latter). Remove the accelerator or route it through
    `getExecutionTarget()`. (The duplicate-execution hazard itself is fixed.)
27. **[P1] Explain view parses PostgreSQL JSON plans with a SQLite text
    parser and invents a letter grade** from substring matches
    (`ExplainView.tsx:37-136`). Parse the JSON plan tree; drop the grade.
28. **[P1] Results grid renders every row into the DOM** (no virtualization),
    and the silent LIMIT-100 is never surfaced ("100 rows" looks like the
    whole table). Virtualize + "truncated — load more" chip (backend must
    report that the limit was injected).
29. **[P2] `eval`-based menu→frontend bridge with unescaped session names**
    (JS injection from `sessions.toml`; a name with `'` silently breaks the
    menu) + `"csp": null`. Replace with Tauri `emit`/`listen`; set a real CSP.
    Also: remote Google Fonts import (offline = silent fallback; breaks under
    CSP).
30. **[P2] Clicking a table/preset overwrites the active tab's SQL; tabs are
    lost on quit; tray menu rebuilt on every window move/resize event.**
31. **[P2] `window.prompt/alert/confirm` for core flows** (unreliable in
    wry/WKWebView — Save Preset can silently no-op). Small in-app modal.

### E. Backends (remaining)

32. **[P1] MongoDB fake-SQL layer**: projection ignored (`SELECT name` returns
    all fields), `BETWEEN` always fails (`" AND "` checked first), `LIMIT 10;`
    falls back to 100 (`database_mongodb.rs:1042-1331`).
33. **[P1] ClickHouse results parsed from TSV text**: `\N` shown literally,
    escapes never unescaped, blanket `.trim()` corrupts data
    (`database_clickhouse.rs:422-485`). Switch to `FORMAT JSON`.
34. **[P1] DataFusion glob patterns are discarded** — `parquet:///data/2024-*.parquet`
    registers the **whole parent directory** (`database_datafusion.rs:74-96`).
    Pass the glob to `ListingTableUrl`.
35. **[P2] NULL renders four different ways across backends** (`""` /
    `"NULL"` / `\N`); centralize one configurable sentinel.
36. **[P2] SQLite/MySQL DECIMAL precision loss** (f64 tried before
    string/Decimal for NUMERIC columns).

### F. Engineering health (remaining)

37. **[P1] Test-isolation heuristic is fragile**: `cargo test -- --test-threads=1`
    runs on the main thread and writes the developer's real
    `~/.config/dbcrust/` (`config.rs:873-902`). Set `RUST_TEST_MODE=1` via
    `[env]` in `.cargo/config.toml`, or inject the config dir explicitly.
38. **[P1] `bump-version.yml` cannot trigger the release workflow**
    (GITHUB_TOKEN-pushed tags don't fire `push: tags`), and `cz bump`'s
    pre-bump hooks (`hk`, GPG) aren't available in CI. Use a PAT/App token;
    install hk or gate the hooks.
39. **[P2] Coverage is skewed away from the riskiest code**: completion.rs
    (2.6k lines, 0 tests), db.rs (1 test), mongodb/elasticsearch/datafusion
    (0), both TUIs (~0), entire GUI (0, no framework). Priority order for new
    tests: statement splitting/Validator (once built), completion
    classification, MongoDB SQL translation, DataFusion rendering (golden
    files), GUI vitest for `sqlAutocomplete`/`connectionDisplay`.
40. **[P2] Docs drift**: `configuration-reference.md` documents ≥8 config keys
    that don't exist and wrong file/section names;
    `backslash-commands.md` omits 23 implemented commands (`\ai`, `\sv`, all
    Mongo/ES commands…) and documents `\quit`/`\help`/`\?` aliases that don't
    exist; the AI feature has zero docs; README still carries MkDocs
    branding. Best fix: **generate** the command reference from
    `CommandShortcut::iter()` (strum already exposes command/description/
    category) and the config reference from `save_with_documentation()`.
41. **[P2] Nine Mongo/ES commands bypass the strum `CommandShortcut` enum**
    (`collections`, `dc`, `dmi`, `cmi`, `ddmi`, `mstats`, `find`,
    `aggregate`, `search` — `commands.rs:1036-1130`) — invisible to help and
    completion, violating the repo's own Critical Rule #1.
42. **[P2] `UrlScheme` enum knows 7 of ~14 supported schemes** — shell
    completion never offers `mongodb://`, `clickhouse://`, `parquet://` etc.
    (`url_scheme.rs:66-81`). Derive from `DatabaseType::url_schemes()` instead.
43. **[P3] Stale `uv.lock`** (locked at dbcrust 0.16.0) — regenerate;
    `.dbcrust` "encryption" is machine-derived obfuscation (no user secret,
    fixed salt) — relabel honestly or move to OS keychain; GUI version frozen
    at 0.1.0 and absent from `.cz.toml` version_files.

---

## Rework proposals

### 1. Django toolkit — redesign the core, keep the shell (est. 1–2 weeks)

Roughly a third of the 14k LoC is dead, demo, or misleading. The shape that
fits what Django developers actually expect (debug-toolbar/silk/nplusone
users):

- **Collector**: `contextvars.ContextVar`-based `QueryCollector` (works under
  threads *and* ASGI), per-request analyzer (done in this pass), opt-in
  stack capture with depth limit, query cap.
- **Detectors**: replace the 19 regex heuristics with 4–5 high-precision
  ones: N+1 grouped by (normalized SQL + stack origin + distinct param sets),
  duplicate-with-params, slow query, large-result, missing-index *only* via
  real EXPLAIN. Delete count-only/exists/select_for_update heuristics (they
  cannot be inferred from SQL).
- **EXPLAIN through Django's own connection**: `connection.cursor()` +
  `cursor.execute("EXPLAIN (ANALYZE, FORMAT JSON) " + sql, params)` — the
  driver binds params, multi-DB aliases work, no second native client, no
  LIMIT injection. This single change resurrects `query_plan_analyzer.py`.
- **CI story**: a pytest plugin exposing `assert_max_queries(n)` /
  "raise on N+1" mode — nplusone's killer feature, and the reason teams adopt
  these tools.
- **Keep**: consolidated report/grading UX, settings→URL conversion in
  `utils.py`, the AST code analyzer.
- **Drop**: `query_plan_analyzer`'s dead path (until EXPLAIN works),
  `debug_logging.py`, `troubleshoot_logging.py`, demo files (already excluded
  from the wheel), one of the two duplicated `dbcrust.py` management commands
  (the `dbcrust/management/` one is the live one today).

### 2. GUI — reliability pass, then depth; no visual rewrite (3 milestones)

The bones are better than they look: lazy views, real-metadata autocomplete,
single-source backend reusing the dbcrust core. It fails on trust, not looks.

- **M1 Correctness (~1 week)**: query cancellation wired to
  `execute_query_with_interrupt` + Cancel button + real timeout; typed
  nullable rows (`(string|null)[][]`); per-tab CodeMirror state (undo bleed);
  event-based menu bridge + CSP; resolve the Cmd+Enter double-binding; scope
  `op_lock` to writes. (Filtered-index, silent errors, Quick Queries, tab
  titles, double-execution: fixed in this pass.)
- **M2 UX depth**: virtualized grid + truncation chip + CSV export + cell
  inspector; query history; save-session UI (backend command already exists,
  frontend never calls it); structured connection form with masked password;
  tabs persisted to localStorage; open table-SELECTs in a new tab.
- **M3 Structure (only then)**: small store (zustand) + connection registry
  keyed by ID on both sides (`HashMap<ConnId, DbThread>`) to unlock
  multi-connection — the one change that genuinely requires rearchitecting.
  Nothing in M1–M2 is thrown away by it.
- **Visual tune-up, not redesign**: root font 13px → 14–15px (the rem scale
  collapses to 8–10px text everywhere), fix zinc-600/700-on-black contrast
  (≈2.5:1, far below AA), real SVG DB icons instead of emoji, bundle fonts
  locally.

### 3. CLI session model (the "psql-credible" core, est. ~1 week)

One design change unlocks several open P1s: a **dedicated session
connection** owned by the REPL (pool reserved for metadata), plus a SIGINT
handler driving server-side cancellation, plus a reedline `Validator` with
statement splitting. Items A1–A3 above are one coherent project, and most of
the remaining daily-driver gaps (`\watch`, `\timing`, `\o`/CSV output,
transaction state in the prompt) become straightforward once it lands.

---

## Suggested sequencing

1. **Land this pass** (all fixes verified green) and let the new CI gate
   `main`.
2. **CLI session model** (rework 3) — biggest daily-driver payoff.
3. **Django EXPLAIN-through-Django + detector consolidation** (rework 1) —
   it's the product's stated identity ("engineered for Django developers").
4. **GUI M1** reliability pass.
5. **Docs generation** from strum/`save_with_documentation` (kills the two
   worst drift sources mechanically), plus an AI-assistant docs page before
   `ai_queries` merges.
6. Backend polish batch (Mongo SQL layer, ClickHouse JSON format, DataFusion
   globs, NULL unification).
