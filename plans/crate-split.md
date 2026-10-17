# DBCrust crate split: dependency analysis and extraction plan

Research output, 2026-10-15. Read-only analysis: no tracked file, `Cargo.toml` or `Cargo.lock` was touched, no cargo build/test was run (only `cargo tree --locked --offline`).
Companion to `plans/agent-secure-connections-crates-connectors.md` (section B "Concrete target workspace", G.2/O4, G.5 Wave 2b) and `plans/connectors/README.md` ("Recommended common driver API").

**Snapshot caveat.** Analysed tree = `d605be6` + the other agent's uncommitted work (modified `cli.rs`, `cli_core.rs`, `config.rs`, `database*.rs`, `lib.rs`, `main.rs`, `safety.rs`; untracked `src/broker/`, ~6.0k lines at read time). Every `file:line` below refers to that working tree. The broker numbers and the `database*.rs`/`safety.rs`/`config.rs` line numbers will drift: re-run the graph script (Appendix A) before starting a step. Nothing in this plan should start while those files are being edited (see section 6, step 0).

---

## 0. Summary

| Item | Result |
|---|---|
| Size today | 89,176 lines in 64 top-level modules of one crate (`src/**/*.rs`, tests inline), plus gui 61k, crates/agent* 7.9k |
| Current dependency structure | **One strongly connected component of 43 of the 64 modules** (all 8 drivers, `database`, `db`, `config`, `docker`, `secrets`, `ai`, `broker`, `format`, `safety`, `password_sanitizer`, `commands`, `cli_core`…). Nothing can be extracted until it is broken |
| Cycles that must be broken | **30 upward module-level edges** between future crates, which collapse into 13 cycle groups (C1–C13, section 1.4) plus 3 non-cycle landmines (C14–C16: orphan rule, `datafusion::sql::sqlparser`, explain TUI/parser mix) |
| Target | The 13 library crates of the agreed list (core, secrets, 8 drivers, ai, docker, broker; "about 12" in G.2/O4) + one tiny composition crate (`dbcrust-drivers`, justification in 2.1) + root. About 13.7k lines in `dbcrust-core`, 5.2k secrets, 14.3k in 8 drivers, 4.9k docker, 3.3k ai, 5.4k broker; **root keeps ~42k** (CLI, REPL, config, PyO3). A "Phase B" (2.5) would take root to ~20k |
| Order | step 0 (freeze) → W0 manifests → P1–P9 prep PRs (all inside the single crate, each independently green and *mergeable in parallel*) → E1 secrets → E2 core → E3 drivers (8 PRs, parallel authoring) ∥ E4 docker ∥ E5 ai → E6 broker → F1–F3 features/CI |
| Single most important finding | After the prep PRs the extraction itself is mechanical (`git mv` + `crate::` → `dbcrust_core::` + `pub use` lines in `lib.rs`). All design effort is in the cycle breakers C1–C16 (section 1.4) |
| Biggest gotchas | (a) `cfg!(test)` stops working across crates (test isolation, 7.3); (b) `DatabaseError` `#[from] sqlx::Error` / `datafusion` cannot be re-created in driver crates (orphan rule); (c) `datafusion::sql::sqlparser` is used by `sql_buffer` and `regex_operators`, not only by the files driver; (d) `impl ComplexDisplayConfig` in the Elasticsearch driver (inherent impl on a foreign type); (e) gui-core uses `default-features = false` and root has *no* features today, so gating silently drops engines from the GUI unless gui-core opts in |

---

## 1. Module dependency graph

### 1.1 Method

A stdlib-only Python script (reproduced in Appendix A): every `use crate::…`, `use crate::{…}` group and inline `crate::x::` path in `src/**/*.rs` is attributed to the top-level module of the file (`src/ai/*.rs` → `ai`). References after the first `#[cfg(test)] mod …` in a file, and in `testkit.rs`/`e2e_tests.rs`/`testing.rs`, are flagged *test-only*. There are no `macro_rules!` shared between modules (one local macro in `broker/gateway.rs:809`), no `#[macro_use]`, no `$crate`. `super::` is used only for `use super::*` in test modules and inside `secrets/`, `broker/`, `ai/`, `explain_tui/`, `schema_tui/` (intra-module). Leaf modules with **no** `crate::` references: `sql_buffer`, `sql_parser`, `sql_context`, `pager`, `prompt`, `script`, `update`, `tls`, `named_queries`, `pgpass`, `highlighter`, `performance_analyzer`, `docker_tunnel` (non-test), `explain_tui` (except `performance_analyzer`).

External dependency counts come from `grep` of `crate_name::`/`use crate_name` outside `#[cfg(test)] mod tests`.

### 1.2 Module table (sorted by size)

"`crate::` deps (prod)" = modules referenced from non-test code. "Test-only" = referenced only from `#[cfg(test)]`. "Heavy ext" = the heavy third-party crates the module itself touches. "Target" = crate after the split (`*` = partially moved, see section 2.3).

| Module | Lines | `crate::` deps (prod) | Test-only | Heavy ext | Target |
|---|---:|---|---|---|---|
| `config` | 6198 | ai, complex_display, database, dbcrust_pass, docker, myconf, password_sanitizer, pgpass, secrets, vault_encryption, vector_display | — | — | root (paths→secrets, opts→core) |
| `broker` | 6011 | cli, config, database, db, password_sanitizer, safety, secrets | — | sqlx, inquire, libc | broker* |
| `commands` | 4701 | ai, cli, complex_display, config, config_editor, database, db, dbcrust_pass, docker, explain_tui, format, history_manager, myconf, named_queries, pgpass, prompt, schema_tui, script, secrets, vector_display | — | inquire, arboard | root |
| `cli_core` | 4164 | ai, broker, cli, commands, completion, config, config_editor, database, db, format, highlighter, history_manager, logging, pager, password_sanitizer, prompt, safety, secrets, secrets_cli, shell_completion, sql_buffer, update, vault_client, vector_display | — | elasticsearch, genai, crossterm, reedline, inquire | root |
| `ai` | 3598 | config, database, db, password_encryption, password_sanitizer, safety, secrets, tls | — | genai, keyring, reqwest | ai* |
| `secrets` | 3561 | config | — | inquire, keyring, argon2, aes_gcm, libc | secrets |
| `database_postgresql` | 3011 | complex_display, database, db, geojson_display, json_display, performance_analyzer, vector_display | — | sqlx, pgvector | driver-postgres |
| `completion` | 2924 | command_completion, commands, completion_provider, config, database, database_elasticsearch, db, history_manager, sql_parser, sql_parser_trait | — | reedline | root |
| `db` | 2745 | cli, config, database, docker, docker_route, docker_tunnel, explain_tui, pgpass, safety, ssh_tunnel | — | sqlx, inquire | root (DTOs→core) |
| `secrets_cli` | 2642 | ai, cli, config, dbcrust_pass, myconf, password_sanitizer, pgpass, secrets | password_encryption | inquire | root |
| `explain_tui` | 2387 | performance_analyzer | — | ratatui, crossterm | root (plan_tree→core) |
| `config_editor` | 2246 | ai, cli_core, complex_display, config, database, password_sanitizer, script, secrets, vector_display | — | genai, inquire | root |
| `schema_tui` | 2001 | database, db | — | ratatui, crossterm | root |
| `database_mongodb` | 1984 | complex_display, config, database, db, geojson_display, json_display, password_sanitizer | — | mongodb, bson | driver-mongodb |
| `docker` | 1978 | database, db, docker_endpoint, docker_route, docker_tunnel, password_sanitizer | — | bollard | docker |
| `database_elasticsearch` | 1822 | complex_display, database, db, password_sanitizer, regex_operators | — | elasticsearch | driver-elasticsearch |
| `database_white_dragon` | 1765 | database, db, sql_parser | — | reqwest | driver-white-dragon |
| `format` | 1694 | cli, db | — | prettytable | core* |
| `database_datafusion` | 1685 | database, db | — | datafusion | driver-files |
| `performance_analyzer` | 1636 | — | — | — | core* |
| `safety` | 1613 | database | — | — | core* |
| `database` | 1570 | database_clickhouse, database_datafusion, database_elasticsearch, database_mongodb, database_mysql, database_postgresql, database_sqlite, database_white_dragon, db, docker, password_sanitizer | — | sqlx, datafusion | core* |
| `database_mysql` | 1569 | complex_display, config, database, db, geojson_display, json_display, password_sanitizer, performance_analyzer, regex_operators | — | sqlx | driver-mysql |
| `database_sqlite` | 1478 | database, db, password_sanitizer, performance_analyzer, regex_operators | — | sqlx | driver-sqlite |
| `docker_tunnel` | 1329 | — | docker | bollard | docker |
| `vault_client` | 1305 | config, password_sanitizer | — | reqwest | root |
| `lib` | 1260 | ai, cli, cli_core, config, database, db, password_sanitizer, tls | — | pyo3 | root |
| `docker_route` | 1140 | docker, docker_tunnel | database | — | docker |
| `sql_buffer` | 1097 | — | — | datafusion, reedline | core* |
| `cli` | 1080 | password_sanitizer, shell_completion | — | usage | root (OutputFormat→core) |
| `database_clickhouse` | 1018 | complex_display, database, db, geojson_display, json_display, password_sanitizer, regex_operators | — | clickhouse, reqwest | driver-clickhouse |
| `dbcrust_pass` | 944 | password_encryption, secrets | — | — | secrets |
| `complex_display` | 915 | vector_display | — | — | core* |
| `command_completion` | 903 | commands, config, config_editor, db | — | reedline | root |
| `sql_parser` | 859 | — | — | — | core* |
| `sql_parser_mysql` | 811 | database, sql_parser, sql_parser_trait | — | — | core* |
| `sql_parser_postgresql` | 767 | database, sql_parser, sql_parser_trait | — | — | core* |
| `sql_parser_sqlite` | 695 | database, sql_parser, sql_parser_trait | — | — | core* |
| `password_sanitizer` | 687 | config | — | — | core* |
| `vector_display` | 682 | — | — | half | core* |
| `pgpass` | 681 | — | — | — | root |
| `sql_parser_trait` | 674 | database, sql_parser, sql_parser_mysql, sql_parser_postgresql, sql_parser_sqlite | — | — | core* |
| `url_scheme` | 657 | config, docker, password_sanitizer | — | — | root |
| `geojson_display` | 610 | complex_display | — | — | core* |
| `docker_endpoint` | 603 | docker | — | — | docker |
| `ssh_tunnel` | 601 | config, password_sanitizer | — | — | root |
| `json_display` | 501 | complex_display | — | — | core* |
| `highlighter` | 491 | — | — | reedline | root |
| `history_manager` | 450 | config, database, db | — | reedline | root |
| `sql_context` | 445 | — | — | — | core* |
| `myconf` | 369 | password_sanitizer | — | — | root |
| `regex_operators` | 327 | database | — | datafusion | core* |
| `shell_completion` | 310 | cli, cli_core, config, database, url_scheme | — | usage | root |
| `update` | 309 | — | — | inquire, reqwest | root |
| `vault_encryption` | 280 | secrets | — | aes_gcm | secrets |
| `password_encryption` | 243 | secrets | — | aes_gcm | secrets |
| `named_queries` | 191 | — | — | — | root |
| `script` | 176 | — | — | — | root |
| `prompt` | 167 | — | — | reedline | root |
| `main` | 157 | — | — | — | root |
| `completion_provider` | 155 | — | — | — | root |
| `logging` | 146 | config, password_sanitizer | — | — | root |
| `tls` | 82 | — | — | rustls | core* |
| `pager` | 69 | — | — | — | root |

Additional non-module facts:

* `src/main.rs` (157 lines) and `src/bin/dbc.rs` (6) use only `dbcrust::{cli, config, cli_core, tls, broker}` through the lib (main.rs:6-7, 102, 110, 125, 135-148).
* `src/lib.rs` (1260 lines) is mostly PyO3 (`#[cfg(feature = "python")]` on 40+ items, lib.rs:70-1230) and re-exports `Config`, `Database`, `logging::{debug,get_log_file_path_string}`, `myconf::*`, `pgpass::*` (lib.rs:83-87).
* `include_str!`: `agent_guide.md` (cli_core.rs:526), `docker_relay.pl` (docker_tunnel.rs:58), self-scanning tests `include_str!("database_postgresql.rs")` (database_postgresql.rs:2636) and `include_str!("password_encryption.rs")` (password_encryption.rs:237). `env!("CARGO_PKG_VERSION")`: database.rs:796, database_mongodb.rs:1712, update.rs, cli.rs, broker.

### 1.3 Condensed DAG

**Today** (module granularity, only the SCC and what hangs off it):

```text
                         ┌────────────────────── one SCC (43 modules) ──────────────────────┐
 leaves:  sql_buffer      │  database ⇄ db ⇄ config ⇄ secrets ⇄ ai ⇄ docker ⇄ password_sanitizer │
          sql_parser      │  database → {database_postgresql,mysql,sqlite,mongodb,clickhouse,    │
          sql_context     │              elasticsearch,white_dragon,datafusion}  (factory)       │
          tls  pager      │  database_* → db (TableDetails…), config (option keys)               │
          pgpass(80%)     │  format → db, cli    safety → database    broker → cli,config,db     │
                          └──────────────────────────────────────────────────────────────────────┘
 above the SCC:  commands ⇄ completion ⇄ command_completion,  cli_core ⇄ config_editor/shell_completion,
                 lib.rs (PyO3), main.rs, explain_tui, schema_tui, secrets_cli, vault_client, …
```

**Target** (crate granularity; `A → B` = A depends on B):

```text
 dbcrust-agent-protocol  ←────────────────────────────────────────────────┐
 dbcrust-secrets   (paths · store · backends · legacy readers)             │
 dbcrust-core      (model · driver API · sql/format/display · safety)      │   no workspace deps (wave A)
        ▲   ▲   ▲   ▲                                                      │
        │   │   │   └── dbcrust-secrets                                    │
        │   │   └────── dbcrust-driver-{postgres,mysql,sqlite,mongodb,     │   core only (wave B)
        │   │            clickhouse,elasticsearch,white-dragon,files}      │
        │   └────────── dbcrust-docker  (bollard · route · tunnel)         │   core only (wave B)
        └────────────── dbcrust-ai      (genai · agent loop)  → core + secrets (wave B)
 dbcrust-drivers  (registry / factory / `ConnectorNotBuilt`)  → core + enabled drivers (wave B')
 dbcrust-broker   → core + secrets + drivers + agent-protocol (wave C)
 dbcrust (root)   → everything; cdylib + rlib; bins dbcrust, dbc (wave D)
 dbcrust-gui-core → dbcrust (root, facade) → dbcrust-gui          (unchanged edges)
 dbcrust-agent    → agent-protocol only (unchanged; MUST NOT see core/secrets/drivers)
```

### 1.4 Cycles between future crates (all upward edges), with evidence

Layering used: `secrets`, `core` (layer 0) < `docker`, `drivers` (1) < `ai` (2) < `broker` (3) < `root` (4). An edge is a *cycle* if it points to a higher or sibling layer that is not in the crate's allowed set. The script prints these edges; the grouped result:

| # | Cycle / upward edge | Evidence (file:line) | Breaker |
|---|---|---|---|
| **C1** | `database` (core) → every driver, through the factory `create_database_client`; every driver → `database` | database.rs:882-922 (arms at 887, 891, 895, 899, 903, 908, 913, 918); consumers db.rs:3,751, broker/gateway.rs:26,194 | Move the factory (≈45 lines) out of `database.rs` into the new `dbcrust-drivers` crate (`src/drivers.rs` first, P6). Root `db.rs` and the broker call `dbcrust_drivers::create_database_client`. Per-engine `#[cfg(feature = …)]` lives *only* there |
| **C2** | `database` ⇄ `db`: `MetadataProvider::get_table_details -> crate::db::TableDetails`; all 8 drivers, `format`, `ai`, `broker`, gui build/consume `db::{TableDetails, ColumnInfo, IndexInfo, ForeignKeyInfo, CheckConstraintInfo, ReferencedByInfo, ColumnFilteringInfo}` while `db.rs` uses `database` 26× | database.rs:1196; database_postgresql.rs:9,102,130-132,213,240,261,281,297,317,395,419 (16 refs); database_sqlite.rs:5,26,53,241,291,306,328; database_mongodb.rs:93,110,164,181,197; database_mysql.rs:9,374,454,519; database_clickhouse.rs:10,231-259; database_elasticsearch.rs:245,308,403,419,439; database_white_dragon.rs:388,415,457,460,468; database_datafusion.rs:6,1083,1094; format.rs:2; broker/gateway.rs:433,990; ai/schema_context.rs:248; db.rs:2507-2563 (definitions), db.rs:103-128 (`ColumnFilteringInfo`) | New `model` module (core): the 6 DTO structs (`TableDetails`, `ColumnInfo`, `IndexInfo`, `ForeignKeyInfo`, `CheckConstraintInfo`, `ReferencedByInfo`), `ColumnFilteringInfo`, `unquote_metadata_identifier`, `metadata_table_parts` (db.rs:16-80). `db.rs` keeps `pub use` so paths like `crate::db::TableDetails` and `dbcrust::db::TableDetails` (gui/core/src/types.rs, schema.rs) are unchanged. Make the two `pub(crate)` helpers `pub` (used by database_mongodb.rs:1410,1419, completion.rs:341, broker/gateway.rs:736,752) |
| **C3** | `database` (core) ⇄ `docker`: `DatabaseError::Docker(#[from] crate::docker::DockerError)`; `ConnectionInfo::parse_url` calls `DockerClient::parse_docker_url`; `docker.rs` imports `ConnectionInfo`/`DatabaseType`. Also `config`, `url_scheme`, `commands`, gui use `DockerTarget`/`PINNED_DAEMON_OPTION` | database.rs:857, 954; docker.rs:11,1122,1141-1142; config.rs:729, 3026; url_scheme.rs:9; commands.rs:2090 | (a) `DockerTarget` (+ `PINNED_DAEMON_OPTION`, docker.rs:1019-1211, pure string parsing, no bollard) → core `docker_target`; `DockerClient::parse_docker_url` (docker.rs:1006-1016) becomes a one-line wrapper. (b) `DatabaseError::Docker(Box<dyn Error + Send + Sync>)`, rendered `"Docker error: {0}"`; the `impl From<DockerError> for DatabaseError` lives in `dbcrust-docker` (orphan rule OK: `DockerError` is local there). gui-core downcast at gui/core/src/docker.rs:278 keeps working because the boxed value is still a `DockerError` |
| **C4** | `DatabaseError` leaks driver error types: `#[from] sqlx::Error`, `#[from] datafusion::error::DataFusionError` | database.rs:875, 878 | Replace the two variants by `Driver { engine: &'static str, source: Box<dyn Error + Send + Sync> }` (message still redacted via `redact_text`, as at database.rs:874). `impl From<sqlx::Error> for DatabaseError` is **not** possible in a driver crate (neither type local), so each driver gets `fn db_err(e: sqlx::Error) -> DatabaseError` and `.map_err(db_err)?`. Affected `?` sites: postgresql 20 `.await?` + 14 `map_err`, mysql 15, sqlite 20, datafusion 7 (+27 existing `map_err`). No code outside `database.rs` names the variants (grep: none) |
| **C5** | `password_sanitizer` (core) → `config::is_password_command_option_key`; `config` → `password_sanitizer` 20+× | password_sanitizer.rs:45; config.rs:25-37, 126, 362, 399… | `options` module (core): `VAULT_OPTION_KEYS`, `PASSWORD_COMMAND_OPTION_KEYS`, `is_vault_option_key`, `is_password_command_option_key`, `is_dbcrust_internal_connection_option` (config.rs:15-37) **plus** `OPT_HARDEN_READ_ONLY`/`OPT_HARDEN_TIMEOUT_SECS` and their helpers (database.rs:66-90, referenced by config.rs:34-35). `config.rs` re-exports them (`pub use`) |
| **C6** | drivers → `config` (`is_dbcrust_internal_connection_option`) | database_mysql.rs:611; database_mongodb.rs:280 | Same `options` module (C5) |
| **C7** | `secrets` ⇄ `config`: `secrets/default.rs` reads the config dir and `is_test_mode`; `config.rs` embeds `secrets::SecretsConfig` and uses the store 7× | secrets/default.rs:21,43; config.rs:190-333 (store use), 1188 (`pub secrets: crate::secrets::SecretsConfig`) | `paths` module **inside `dbcrust-secrets`**: `is_test_mode`, `get_config_dir_override`, `set_config_dir_override`, `ensure_directory`, `get_config_dir_impl`, `write_file_atomically` (config.rs:3700-3822) and the body of `Config::get_config_directory` (config.rs:1444-1470). `Config::get_config_directory()` stays as a thin wrapper. Also moves with it: `hint_legacy_secret_used`/`warn_store_problem` (config.rs:221-249) as `secrets::hints`; its `database::one_shot_mode()` test is redundant because the CLI already does `set_interactive(prompts_allowed() && !one_shot)` (cli_core.rs:477) |
| **C8** | `ai` ⇄ `config`: `ai/key_storage.rs` uses `Config`, `hint_legacy_secret_used`, `is_test_mode`; `config.rs` embeds `ai::config::AiConfig`; `secrets_cli` reads `ai::key_storage::legacy` | ai/key_storage.rs:14,82,157,197; config.rs:1227,1288, 2476, 2488; secrets_cli/audit.rs:401-402, secrets_cli/migrate.rs:22 | `ai/config.rs` (203 lines, pure serde, `AiConfig`/`AiAuthMethod`) → core `ai_config`, so a config file with an `[ai]` table parses even without the `ai` feature. `key_storage::legacy` (key_storage.rs:169-320: file/keychain probes, only needs `suggested_providers()` at :185 as an argument) → `dbcrust_secrets::legacy::ai_keys`. After that `ai` needs only `core` + `secrets::paths` |
| **C9** | `ai` → `db::Database` (a root type): agent tool executor and schema context call ~10 `Database` methods | ai/agent.rs:27,409,436-521; ai/schema_context.rs:5,15,112,144,306 (exactly 10 methods: `is_explain_mode`, `toggle_explain_mode`, `execute_query_with_interrupt_no_column_selection`, `get_tables_and_views`, `get_database_type`, `get_current_db`, `get_table_details_in_schema`, `get_table_details_bulk`, `get_schemas`, `get_database_client`) | Trait `AiSession` defined in `dbcrust-ai`, implemented for `Database` in root (`impl ForeignTrait for LocalType`, orphan OK). `Arc<Mutex<Database>>` → `Arc<Mutex<dyn AiSession + Send>>`; `build_schema_context(&mut dyn AiSession)`. Call sites: lib.rs:1099-1141, cli_core.rs:2558-2581 |
| **C10** | `broker` → `cli` (`BrokerAction`, `BrokerModeArg`, `OutputFormat`), `config`, `db` | broker/cli.rs:14; broker/mod.rs:97-98; broker/e2e_tests.rs:1062,1082,1110,1126; broker/admin.rs:158; broker/gateway.rs:115 (`is_dbcrust_internal_connection_option`); broker/paths.rs:67 (`Config::get_config_directory`); broker/gateway.rs:433,736,752,990 (DTOs/`metadata_table_parts`) | `broker/cli.rs` (646 lines) + `mod.rs::run` + `e2e_tests.rs` stay in root as `src/broker.rs`/`src/broker_cli.rs`: they own the `usage`-derived argument types. The rest (`admin`, `audit`, `gateway`, `hardening`, `paths`, `registry`, `server`) depend only on core + secrets + drivers + agent-protocol after C2/C5/C7. `testkit.rs` (sqlx fixtures) goes with `gateway.rs` as a dev-only module |
| **C11** | `format` (core) → `cli::OutputFormat` and `db::ColumnFilteringInfo`; `db`, `commands`, `cli_core`, `secrets_cli`, `broker` use `OutputFormat` | format.rs:1,2,182-199; db.rs:182,764,1956-1960,2176; commands.rs:1460; cli_core.rs:63,417,457; cli.rs:651-664 (derives `usage::ValueEnum`) | `OutputFormat` (cli.rs:651-664) → core `output_format`, `#[cfg_attr(feature = "usage", derive(usage::ValueEnum))]` (the `usage-rs` crate is only `usage-argv` + a proc-macro). `cli.rs` re-exports it |
| **C12** | `docker` → `db::Database::connect_docker_container` | docker.rs:1022 (doc link), 1710, 1748, 1751 (test) | Doc link rewritten; the one test that constructs `Database` stays in root (`db.rs` tests) |
| **C13** | `completion` → `database_elasticsearch::ElasticsearchClient::needs_quoting` | completion.rs:334 | Pure 8-line predicate (database_elasticsearch.rs:701-709) → `core::database::elasticsearch_index_needs_quoting`; the driver delegates |
| **C14** | **Orphan-rule landmine**: `impl ComplexDisplayConfig { fn elasticsearch_default() }` written in the ES driver for a type from `complex_display` | database_elasticsearch.rs:1657-1665 (only such case in the tree; checked by script) | Move into `complex_display.rs` (core) as an inherent fn. Same script found no other foreign inherent/trait impls |
| **C15** | Non-cycle but crate-leaking externals: `sql_buffer` and `regex_operators` use `datafusion::sql::sqlparser::{ast,dialect,parser,tokenizer}`; `db.rs` mentions `sqlx::PgPool` (dead `get_pool()` returns `None`) and `sqlx::postgres::PgSslMode` (unused `_ssl_mode` param) | sql_buffer.rs:21-26; regex_operators.rs:26,31,32; db.rs:807,1387 | Core depends on `sqlparser = "=0.61.0"` directly (`features = ["std", "visitor"]`, matches the unified features seen in `cargo tree -e features`; same package already in `Cargo.lock`, no new lock entry). `get_pool` deleted (zero callers); `_ssl_mode` parameter dropped (single call site lib.rs:326 passes `None`) |
| **C16** | `explain_tui` mixes a pure plan parser with a ratatui UI; gui-core uses only the parser | explain_tui/mod.rs:29-30 re-exports `plan_tree::{PlanNode, PlanStatistics, parse_postgresql_plan}`; gui/core/src/explain.rs:16 | `plan_tree.rs` (775 lines, no ratatui/crossterm) → core `explain_plan`; root `explain_tui` keeps the TUI behind `explain-tui` and re-exports the parser unconditionally |

Intra-root cycles that are **not** a problem (all stay in the root crate): `cli` ⇄ `shell_completion` ⇄ `cli_core` (cli.rs:71, shell_completion.rs:16,148), `config_editor` ⇄ `cli_core` (config_editor.rs:1466, cli_core.rs: calls config_editor 714-749), `commands` ⇄ `completion` ⇄ `command_completion` (completion.rs:7-8, command_completion.rs:4). `docker.rs` ⇄ `docker_route.rs` ⇄ `docker_tunnel.rs` is a module-level cycle inside the future `dbcrust-docker` crate (fine).

Process-wide globals (not cycles, but they are why drivers read `database::…` and `complex_display::…`): `QUERY_TIMEOUT_SECONDS`, `ONE_SHOT_MODE`, `READ_ONLY_REQUESTED`, thread-local interrupt flag (database.rs:16-120; read by 4 drivers + `db.rs` 13×, gui-core worker.rs 5×, `cli_core` 33×), and `GLOBAL_COMPLEX_CONFIG`/vector config (complex_display.rs:114-135, vector_display.rs:16-26; written by commands.rs:2308-2655, config_editor.rs:1333-1354, cli_core.rs:219; read by database_postgresql.rs:663,1776,2072,2128). Both globals travel to core unchanged (one instance per process, shared by all crates because they are `rlib`s). Replacing them by `ConnectOptions` / render options is the follow-up in `plans/connectors/README.md` §5 and is deliberately **not** part of the split.

---

## 2. Crate assignment

### 2.1 Crates

Sizes are lines including inline tests, after the splits of 2.3.

| Crate (path) | Modules / files | Lines | Heavy external deps (allowed) | Must NOT depend on (checked by `cargo tree -p`) |
|---|---|---:|---|---|
| `dbcrust-secrets` (`crates/secrets`) | `secrets/*` (3561), `paths` (new, ~150), `password_encryption` (243), `dbcrust_pass` (944), `vault_encryption` (280), `legacy::ai_keys` (~150), `hints` | ≈5.2k | keyring, argon2, aes-gcm, zeroize, inquire (passphrase prompt), toml, libc, hostname, dirs, sha2 | core, any driver SDK, reqwest, genai, sqlx, datafusion, bollard, ratatui, reedline |
| `dbcrust-core` (`crates/core`) | `database` (split into `driver/{client,metadata,types}` files, re-exported flat), `model`, `options`, `docker_target`, `output_format`, `password_sanitizer`, `complex_display`, `vector_display`, `geojson_display`, `json_display`, `performance_analyzer`, `regex_operators`, `safety`, `format`, `sql_parser`, `sql_buffer` (pure), `explain_plan`, `ai_config`, `tls` (feature `tls`) | ≈13.7k | async-trait, serde(+json), thiserror, strum, url, percent-encoding, regex, chrono, prettytable-rs, half, nu-ansi-term, `sqlparser`; optional `usage` (derive only), optional `rustls` | sqlx, datafusion/arrow/parquet, mongodb/bson, clickhouse, elasticsearch, reqwest, genai, bollard, ratatui, crossterm, reedline, inquire, keyring, pyo3, arboard, argon2, tokio (no module going to core uses it; the only `tokio` mention is the `#[tokio::test]` network test of tls.rs:61, which moves to `dbcrust-ai`) |
| `dbcrust-driver-postgres` | `database_postgresql` (3011) | 3.0k | sqlx(postgres+chrono,json,uuid,ipnetwork,rust_decimal), pgvector, half, chrono, hex | other drivers, secrets, docker, ai |
| `dbcrust-driver-mysql` | `database_mysql` (1569) | 1.6k | sqlx(mysql+chrono,json,rust_decimal), chrono | idem |
| `dbcrust-driver-sqlite` | `database_sqlite` (1478) | 1.5k | sqlx(sqlite+regexp,chrono) | idem |
| `dbcrust-driver-mongodb` | `database_mongodb` (1984) | 2.0k | mongodb, bson, futures-util | idem |
| `dbcrust-driver-clickhouse` | `database_clickhouse` (1018) | 1.0k | clickhouse `=0.15.1`, reqwest 0.12 | idem |
| `dbcrust-driver-elasticsearch` | `database_elasticsearch` (1822) | 1.8k | elasticsearch, regex | idem |
| `dbcrust-driver-white-dragon` | `database_white_dragon` (1765) | 1.8k | reqwest 0.12 | idem |
| `dbcrust-driver-files` | `database_datafusion` (1685) | 1.7k | datafusion(parquet,sql,…), tempfile, futures-util | idem |
| `dbcrust-drivers` (`crates/drivers`) | `drivers.rs`: `create_database_client`, `compiled_engines()`, `is_compiled()` | ≈0.15k | the 8 driver crates, all optional | broker, root, secrets |
| `dbcrust-docker` | `docker` (1977−195), `docker_endpoint` (602), `docker_route` (1139), `docker_tunnel` (1328) + `docker_relay.pl` | ≈4.9k | bollard, futures-util, sha2, hex, dirs | any driver SDK, sqlx, genai |
| `dbcrust-ai` | `ai/*` minus `config.rs` and `key_storage::legacy` | ≈3.3k | genai, `reqwest_genai` alias (reqwest 0.13 `rustls-no-provider`), reqwest 0.12, base64, rand, uuid | sqlx, drivers, docker, ratatui, reedline |
| `dbcrust-broker` | `broker/{admin,audit,gateway,hardening,paths,registry,server,testkit}` | ≈5.4k | dbcrust-agent-protocol, libc, tokio(net), toml, sha2, chrono | ai, docker, reedline, inquire (see note), ratatui, genai |
| `dbcrust` (root) | everything else (2.3 lists the exact files) | ≈42k | reedline, inquire, usage-rs(completions), ratatui/crossterm/textwrap (`explain-tui`), arboard (`clipboard`), reqwest 0.12 (vault, update), pyo3 (`python`) | – |

Notes:

* `dbcrust-drivers` is **not** in the agreed 12-crate list but is in section B's target table (`dbcrust-drivers` — "Registry/composition root"). It is required by the broker (`create_database_client` at broker/gateway.rs:194): the broker must not depend on the root crate. It also keeps all per-engine `cfg(feature)` out of root, so the feature graph has one hub (section 4). If you want to keep it at 12 crates, the registry can live in `dbcrust-core` behind `driver-*` features, at the cost of core's fingerprint changing per feature set and of core depending on the driver crates (inverting the layering): not recommended.
* The broker's only `inquire` uses are in `broker/cli.rs:372-416` (password and confirm prompts), which stays root-side; so `dbcrust-broker` itself has no `inquire` and `cargo tree -p dbcrust-broker` must show none (check at E6). Its `sqlx` uses (`testkit.rs`, `e2e_tests.rs`) are dev-only.
* `dbcrust-secrets` owns `paths` on purpose (C7). The alternative is a 60-line `dbcrust-paths` micro-crate that both secrets and root depend on; equivalent, one more `Cargo.toml`, and it avoids "secrets owns config-dir logic" naming awkwardness. Pick at E1; the code move is identical.

### 2.2 Per-module assignment

This is the `Target` column of 1.2. Whole-file moves unless 2.3 says otherwise.

* **core:** `database`, `password_sanitizer`, `complex_display`, `vector_display`, `geojson_display`, `json_display`, `performance_analyzer`, `regex_operators`, `safety`, `format`, `sql_parser`, `tls`, `sql_buffer` (split), plus the extracted pieces of 2.3.
* **secrets:** `secrets/**`, `password_encryption`, `dbcrust_pass`, `vault_encryption`.
* **drivers:** `database_{postgresql,mysql,sqlite,mongodb,clickhouse,elasticsearch,white_dragon,datafusion}`.
* **docker:** `docker`, `docker_endpoint`, `docker_route`, `docker_tunnel`.
* **ai:** `ai/{agent,chatgpt_auth,conversation,key_storage,mod,model_listing,prompt_templates,schema_context,streaming}`.
* **broker:** `broker/{admin,audit,gateway,hardening,paths,registry,server,testkit}`.
* **root (stays):** `cli`, `cli_core`, `command_completion`, `commands`, `completion`, `completion_provider`, `config`, `config_editor`, `db`, `explain_tui` (UI), `schema_tui`, `highlighter`, `history_manager`, `lib`, `logging`, `main`, `bin/dbc`, `myconf`, `named_queries`, `pager`, `pgpass`, `prompt`, `script`, `secrets_cli`, `shell_completion`, `sql_context` (no in-tree user besides its own tests: keep as a root `pub mod` for API compatibility or delete), `sql_parser_trait`, `sql_parser_{mysql,postgresql,sqlite}` (used only by `completion`: 3.4k lines that do **not** need to sit on core's critical path), `ssh_tunnel`, `update`, `url_scheme`, `vault_client`, `broker/{cli,mod,e2e_tests}`.

### 2.3 Files that must be split (pure half / impure half)

| File | Pure half → | Impure half stays | Lines moved |
|---|---|---|---:|
| `database.rs` (1569) | Everything but the factory → core (`DatabaseType`, `DatabaseTypeExt`, `ConnectionInfo`, `ServerInfo`, `DatabaseError`, `MetadataProvider`, `DatabaseClient`, `StructuredQueryResult`, flags/hardening helpers, SQL quoting) | factory database.rs:882-922 → `dbcrust-drivers` | ~1.5k |
| `db.rs` (2745) | DTOs db.rs:2507-2563 + `ColumnFilteringInfo` :103-128 + `metadata_table_parts`/`unquote_metadata_identifier` :16-80 → core `model` | `Database` (session: docker connect :337-640, ssh tunnel, explain TUI :1727-1775, interactive column selection `inquire` :2393+) stays in root | ~170 |
| `config.rs` (6198) | option-key helpers :15-37 → core `options`; `paths` + `write_file_atomically` + `is_test_mode` :3700-3822 and `get_config_directory` :1444-1470 → secrets `paths`; `hint_legacy_secret_used`/`warn_store_problem` :221-249 → secrets `hints` | `Config` and every storage struct stay (they embed `SecretsConfig` from secrets, `AiConfig`/`ComplexDisplayConfig`/`VectorDisplayConfig` from core, `SSHTunnelConfig` local) | ~190 |
| `docker.rs` (1977) | `PINNED_DAEMON_OPTION` + `DockerTarget` :1019-1211 → core `docker_target` | `DockerClient`, `DockerError`, discovery | ~195 |
| `cli.rs` (1080) | `OutputFormat` :651-664 → core `output_format` | the whole usage-derive grammar | ~20 |
| `sql_buffer.rs` (1097) | everything except `SqlValidator` (:34-45, uses `reedline::Validator`) → core `sql::buffer` | `SqlValidator` → root `sql_validator.rs`, re-exported as `dbcrust::sql_buffer::SqlValidator` | ~1.07k |
| `explain_tui/` (2387) | `plan_tree.rs` (775) → core `explain_plan` | `app.rs`, `ui.rs`, `mod.rs` (ratatui/crossterm/textwrap) | 775 |
| `ai/` | `config.rs` (203) → core `ai_config`; `key_storage::legacy` (:169-320) → secrets `legacy::ai_keys` | rest → `dbcrust-ai` | ~350 |
| `tls.rs` (81) | `install_default_crypto_provider` + its first test → core (feature `tls`) | tests that use `genai::Client` and `reqwest_genai` (tls.rs:36-54) → `dbcrust-ai` | 40 |
| `broker/` | all but `cli.rs`, `mod.rs::run`, `e2e_tests.rs` → `dbcrust-broker` | those three | ~5.4k |
| `database_elasticsearch.rs` | – | `needs_quoting` body → core (C13); `impl ComplexDisplayConfig` → core (C14) | ~15 |

### 2.4 What `dbcrust-core` has to accommodate (plans/connectors/README.md)

The split must not make the Wave-3 API harder. Reserve these homes in core now (no behaviour change, files can be empty modules with the types commented in):

* `driver::client` / `driver::metadata`: the existing `DatabaseClient` (database.rs:1252) and `MetadataProvider` (:1174); new defaulted methods `capabilities()`, `verify_read_only()`, `execute_limited()`, `browse()` are additive. `StructuredQueryResult` is **not** modified (19 literal sites across drivers, broker, history_manager).
* `driver::capabilities`: `Enforcement`, `EngineFamily`, `QueryLanguage`, `LimitMode`, `CostUnit`, `DriverCapabilities` (§1 of the README), `driver::limits`: `ExecutionLimits`, `LimitedResult`, `BrowseRequest`, `driver::classifier`: `StatementClassifier` (the `check_read_only` `match` at safety.rs:31 becomes a shim over it), `driver::descriptor`: `DriverDescriptor` (§4; `cargo_feature: Option<&'static str>` is exactly what `dbcrust-drivers` needs), `driver::options`: `ConnectOptions`, `SecretMap` (§5).
* **`DatabaseType` stays a flat enum with every variant unconditional** (database.rs:137-149; `serde::{Serialize,Deserialize}` + `strum::EnumIter`). Disabled engines still deserialize from config.toml/sessions.toml/recent.toml; `DatabaseType::DuckDB` already exists and today routes to the DataFusion client (database.rs:918).
* **`DatabaseError` must be additive**: add `ConnectorNotBuilt { database_type, feature }` now (P5) and mark the enum `#[non_exhaustive]` in core so new variants never break gui/broker `match`es (nobody matches it exhaustively today, grep).
* **`ConnectionInfo` struct literals are the real compatibility risk.** `ConnectionInfo { … }` is constructed literally in 57 places (config.rs 15, datafusion 8, database.rs 6, postgresql 5, sqlite 5, db.rs 3, gui-core connect.rs 3, history_manager 5, …). Adding `secrets: SecretMap` (README §5) after the split would touch 5 crates. P8 first adds `impl Default for ConnectionInfo` and converts literals to `..Default::default()`; after that new fields are source-compatible.
* `tls` module in core (README "Prerequisites" 1): `tls::install_default_crypto_provider` must be callable by every connector; it is `#[cfg(feature = "tls")]` so the `rustls` build script is not on the critical path of crates that do not need it.

### 2.5 Phase B (optional, not required for the agreed split)

Root is still ~42k lines. If release time is the goal, three further extractions are mechanical once Phase A is merged: (1) `explain_tui` UI + `schema_tui` → `dbcrust-tui` (4.4k; `schema_tui` needs `Database`: use a `SchemaSource` trait like `AiSession`); (2) `config` + `config_editor` + `pgpass` + `myconf` + `ssh_tunnel` + `history_manager` + `logging` → `dbcrust-config` (≈10.7k; `config_editor` calls `cli_core::prompts_allowed` at config_editor.rs:1466 → inject; `genai::adapter::AdapterKind` at :844 → `dbcrust_ai::is_known_provider`); (3) `db.rs` → `dbcrust-runtime` (2.7k; inject `ExplainViewer` for db.rs:1749 and `ColumnPicker` for db.rs:2393+). Result: root ≈ 20k (CLI, REPL, completion, commands, PyO3). Section B's `dbcrust-runtime`/`dbcrust-cli` naming applies.

---

## 3. Public API surface and the root facade

### 3.1 Per crate

* **`dbcrust-secrets`**: `SecretStore`, `SecretBackend`, `KeychainBackend`, `MemoryBackend`, file backend types (`secrets/mod.rs:41-53` re-exports as they are today), `SecretRecord`, `RecordId`, `AgentPolicy`, `TargetBinding`, `SecretsConfig`, `SecretError`, `default_store`, `reset_default_store`, `with_store_override`, `set_interactive`, `is_interactive`; **new**: `paths::{is_test_mode, config_directory, set_config_dir_override, write_file_atomically}`, `hints::{warn_store_problem, hint_legacy_secret_used}`, `legacy::{dbcrust_pass, password_encryption, vault_encryption, ai_keys}`. **`fsutil::{read_bounded, write_private_atomic}` and `testing::FaultyBackend` are `pub(crate)` today but used from the root crate** (`secrets_cli/migrate.rs:513,617,886,1010` and tests :1038-1424; `ai/key_storage.rs:241` moves with the legacy reader): they become `pub` + `#[doc(hidden)]`, and `testing` is gated by a `test-support` cargo feature that root enables as a dev-dependency feature.
* **`dbcrust-core`**: `database::{DatabaseType, DatabaseTypeExt, ConnectionInfo, ServerInfo, DatabaseError, MetadataProvider, DatabaseClient, StructuredQueryResult, escape_sql_string, quote_sql_ident, set/get query_timeout, one_shot_mode, read_only_requested, set_thread_interrupt_flag, interrupt_flag, connection_read_only, connection_timeout_secs, is_hardened_connection, OPT_HARDEN_*}` (flat re-export so `dbcrust::database::X` is unchanged); `model::{TableDetails, ColumnInfo, IndexInfo, ForeignKeyInfo, CheckConstraintInfo, ReferencedByInfo, ColumnFilteringInfo, metadata_table_parts, unquote_metadata_identifier}`; `options::*`; `docker_target::{DockerTarget, PINNED_DAEMON_OPTION}`; `output_format::OutputFormat`; `password_sanitizer::*` (13 fns, listed at password_sanitizer.rs:6-360); `safety::{check_read_only, is_read_only_sql, side_effect_guard, classify_for_broker, StatementClass, RefusalKind, Refusal, BrokerStatement, ReadOnlyViolation, READ_ONLY_VIOLATION_MARKER, is_read_only_violation_message}`; `format::{render_query_results, format_query_results_*, format_table_details, analyze_format_crash}` (returns `prettytable::Table` in two fns, format.rs:72,121 → core keeps `prettytable-rs`); `complex_display`, `vector_display`, `geojson_display`, `json_display`, `performance_analyzer::{PerformanceLevel, PerformanceMetrics, PerformanceAnalyzer}`; `regex_operators::{RegexTarget, translate_regex_operators}`; `sql::{parser::SqlParser, TokenType, buffer::{split_statements, split_statement_spans, split_statement_spans_with, SplitMode, SqlDialect, Boundary, StatementSpan, is_buffer_complete, has_blank_line_separator, parses_as_single_statement, ends_in_line_comment}}`; `explain_plan::{PlanNode, PlanStatistics, parse_postgresql_plan}`; `ai_config::{AiConfig, AiAuthMethod, …}`; `tls::install_default_crypto_provider` (feature `tls`).
* **`dbcrust-driver-*`**: one public struct each: `PostgreSQLClient`, `MySqlClient`, `SqliteClient`, `MongoDBClient`, `ClickHouseClient`, `ElasticsearchClient`, `WhiteDragonClient`, `DataFusionClient`, each with `pub async fn new(ConnectionInfo) -> Result<Self, DatabaseError>` (today's constructors, database.rs:887-918). Anything else `pub` in those files becomes `pub(crate)` unless used by root (check with `cargo +nightly udeps`/`unreachable_pub` lint after the move; `ElasticsearchClient::needs_quoting` is already moved by C13; `MongoDBClient`'s helper calls `db::unquote_metadata_identifier` → core).
* **`dbcrust-drivers`**: `create_database_client`, `compiled_engines() -> &'static [DatabaseType]`, `is_compiled(&DatabaseType) -> bool`, `required_feature(&DatabaseType) -> &'static str`.
* **`dbcrust-docker`**: `DockerClient`, `DockerError`, `DockerEndpoint`, `EndpointSource`, `DockerPhase`, `DockerRoute`, `RouteAttempt`, `RouteKind`, `Unreachable`, `RouteBudgets`, `DockerContainerInfo`, `DockerDatabaseConnection`, `DockerConnected<T>`, `DockerTunnel`, `RelayTarget`, `PublishedBinding` (docker.rs:28-36 re-exports). `pub(crate)` items (`docker_route::{Pick, pick}`, `docker_endpoint::context_dir_name`) stay crate-private.
* **`dbcrust-ai`**: `generate`, `MessageRole`, `AiError`, `suggested_providers`, `key_storage::*` (non-legacy), `agent::{run_agent, DbToolExecutor, ProgressSink, FileProgress, StdoutProgress, NoOpProgress}`, `schema_context::*`, `prompt_templates::*`, `streaming::extract_sql`, `chatgpt_auth::*`, `AiSession` (new). The 16 `pub(crate)` items in ai (`key_storage::legacy` moves out; `schema_context::collect_table_names`, `format_table_ddl` at :143, :247) stay crate-private.
* **`dbcrust-broker`**: `BrokerError`, `paths`, `registry`, `admin`, `audit`, `gateway::Gateway`, `server`, `hardening` (today's `pub mod`s at broker/mod.rs:70-85). Root's `src/broker.rs` re-exports them so `dbcrust::broker::run` (main.rs:138) is unchanged.

### 3.2 Root facade: every `dbcrust::…` path used outside the root crate

Collected from `gui/`, `crates/` and the bins (`grep -rn 'dbcrust::' gui crates`: 120 use sites, plus the 7 in `main.rs`/`bin/dbc.rs`). Everything must resolve unchanged.

| Path | Used by | After the split |
|---|---|---|
| `dbcrust::config::{Config, Config::default/load/get_config_directory, SavedSession, RecentConnection, NamedQueriesStorage, NamedQueriesStorage::generate_key, NamedQuery, NamedQueryScope, StorageFile, LogLevel, set_config_dir_override, write_file_atomically}` | gui (13 files), gui-core store/launcher/connect/gui_state/worker, main.rs | `config.rs` stays in root. `set_config_dir_override`, `write_file_atomically`, `is_test_mode` become `pub use dbcrust_secrets::paths::{…}`; `is_password_command_option_key`/`is_dbcrust_internal_connection_option` become `pub use dbcrust_core::options::{…}` |
| `dbcrust::config_editor::{ConfigSection, FieldKind, FieldSpec}` | gui settings | root, unchanged |
| `dbcrust::database::{ConnectionInfo, ConnectionInfo::parse_url, DatabaseType, DatabaseTypeExt, set_thread_interrupt_flag, set_read_only_requested, read_only_requested, set_query_timeout_seconds}` | gui-core connect/worker/launcher/types/url_builder/schema/completion/explain/gui_state/docker, gui main | `pub use dbcrust_core::database;` in `lib.rs` |
| `dbcrust::db::{Database, FrontendMode, TableDetails, ColumnInfo, IndexInfo, ForeignKeyInfo, CheckConstraintInfo, ReferencedByInfo}` | gui-core worker/connect/types/schema, gui inspector | `db.rs` stays and adds `pub use dbcrust_core::model::{TableDetails, ColumnInfo, IndexInfo, ForeignKeyInfo, CheckConstraintInfo, ReferencedByInfo, ColumnFilteringInfo, metadata_table_parts};` |
| `dbcrust::docker::{DockerClient, DockerEndpoint, EndpointSource::{Context,DockerHostEnv}, DockerError, DockerPhase, DockerRoute, RouteAttempt, RouteKind, Unreachable, DockerTarget, DockerTarget::parse, PINNED_DAEMON_OPTION}` | gui-core docker/connect/launcher/url_builder/worker, gui home | root `docker.rs`: `pub use dbcrust_core::docker_target::{DockerTarget, PINNED_DAEMON_OPTION};` always; `#[cfg(feature = "docker")] pub use dbcrust_docker::*;`. gui-core must gate its `docker` module on `dbcrust/docker` |
| `dbcrust::explain_tui::{PlanNode, PlanStatistics, parse_postgresql_plan}` | gui-core explain.rs:16 | root `explain_tui` module always exists: `pub use dbcrust_core::explain_plan::*;` + `#[cfg(feature = "explain-tui")] mod app/ui/mod…` |
| `dbcrust::performance_analyzer::PerformanceLevel` | gui-core explain.rs:17 | `pub use dbcrust_core::performance_analyzer;` |
| `dbcrust::password_sanitizer::{redact_text, redact_url, redact_error, is_secret_key, sanitize_connection_url, sanitize_ssh_tunnel_string}` | gui-core error/connect/types/url_builder, gui home/tunnels | `pub use dbcrust_core::password_sanitizer;` |
| `dbcrust::safety::check_read_only` | gui-core worker.rs:793 | `pub use dbcrust_core::safety;` |
| `dbcrust::secrets::set_interactive` | gui main.rs:15 | `pub use dbcrust_secrets as secrets;` (plus `legacy`, `paths`) |
| `dbcrust::sql_buffer::{Boundary, SplitMode, SqlDialect, StatementSpan, split_statement_spans_with, has_blank_line_separator, parses_as_single_statement, …}` | gui-core statements.rs/sql_format.rs, gui query/run.rs, query/provenance.rs | root `sql_buffer.rs`: `pub use dbcrust_core::sql::buffer::*; pub use crate::sql_validator::SqlValidator;` |
| `dbcrust::tls::install_default_crypto_provider` | gui main.rs:7, main.rs:110,125, lib.rs:262,1172 | `pub use dbcrust_core::tls;` (core feature `tls` is on whenever root builds: root always enables it) |
| `dbcrust::vault_client::{parse_vault_url, …}` | gui-core connect/url_builder/vault | root, unchanged |
| `dbcrust::cli::{Args, CliCommand::Broker}`, `dbcrust::cli_core::CliCore`, `dbcrust::broker::run` | main.rs, bin/dbc.rs | root, unchanged |
| `dbcrust::{Config, Database, debug, get_log_file_path_string, get_mysql_config_path, lookup_mysql_password, save_mysql_config, get_pgpass_path, lookup_password}` | lib.rs:83-87, PyO3 | unchanged |
| PyO3: `#[pymodule]` (lib.rs:260), 6 `#[pyclass]` (:90-194), `run_cli_loop`/`run_command`, `module-name = "dbcrust._internal"` (pyproject.toml) | maturin | no change; the module body only calls `crate::database`, `crate::config`, `crate::db`, `crate::ai`, `crate::password_sanitizer`, `crate::tls` (lib.rs:109,539-543,976,1081,…) |

The mechanism that keeps source churn tiny: in root `lib.rs` replace `pub mod database;` etc. with

```rust
pub use dbcrust_core::{
    complex_display, database, format, geojson_display, json_display, password_sanitizer,
    performance_analyzer, regex_operators, safety, tls, vector_display,
};
pub use dbcrust_secrets as secrets; // + `pub use dbcrust_secrets::legacy::{dbcrust_pass, password_encryption, vault_encryption};`
```

so every `crate::database::…` in the remaining root modules keeps compiling untouched, and only the *moved* files need `crate::` → `dbcrust_core::` (mechanical `sed`; every `crate::` in a driver is a core module, see the edge table).

---

## 4. Feature map

### 4.1 Root features

Today root has `default = []` and `python = ["pyo3", "pyo3/macros"]` (Cargo.toml:`[features]`). Because all engines are unconditional, gui-core's `default-features = false` (gui/core/Cargo.toml) currently gets everything.

```toml
[features]
default  = ["bundled"]                                  # D3: shipped build == today's behaviour
bundled  = ["standard", "mongodb", "clickhouse", "elasticsearch", "white-dragon", "files",
            "docker", "ai", "explain-tui", "clipboard"]
standard = ["postgres", "mysql", "sqlite"]              # slim preset (D3); ssh/vault stay always-on (0 extra deps)
postgres = ["dbcrust-drivers/postgres"]
mysql    = ["dbcrust-drivers/mysql"]
sqlite   = ["dbcrust-drivers/sqlite"]
mongodb  = ["dbcrust-drivers/mongodb"]
clickhouse    = ["dbcrust-drivers/clickhouse"]
elasticsearch = ["dbcrust-drivers/elasticsearch"]
white-dragon  = ["dbcrust-drivers/white-dragon"]
files    = ["dbcrust-drivers/files"]                    # DataFusion; also serves DatabaseType::DuckDB until a real connector
docker   = ["dep:dbcrust-docker"]
ai       = ["dep:dbcrust-ai", "dbcrust-core/tls"]
explain-tui = ["dep:ratatui", "dep:crossterm", "dep:textwrap"]   # explain TUI + schema TUI
clipboard   = ["dep:arboard"]
python   = ["dep:pyo3", "pyo3/macros"]                  # unchanged; maturin keeps `features = ["python"]` and default stays on
```

`dbcrust-drivers` features forward 1:1 to `dep:dbcrust-driver-*`. `dbcrust-core/tls` is enabled by root unconditionally (cheap; ring is already in the tree) so `dbcrust::tls` always exists.

Migration rule from section B/G.2: **default stays `bundled`** until an announced release; prebuilt binaries/wheels/GUI use `bundled` explicitly (`maturin` `features = ["python", "bundled"]` once `default` flips to `standard`). New connectors (Redis…, DuckDB, AWS) are never added to `bundled` silently.

Python note: maturin passes `--features python` and keeps default features, so wheels keep every engine. The i686 job (`mbx check --locked -p dbcrust --lib --features python --target i686-unknown-linux-gnu`, ci.yml:62) then also compiles all new crates for i686: clickhouse stays exactly `=0.15.1` (workspace dependency), required reading in CLAUDE.md.

### 4.2 `#[cfg(feature = …)]` call sites in root (and elsewhere)

| Feature | Sites | What a disabled build does |
|---|---|---|
| **all engines** | none in root. All per-engine arms live in `dbcrust-drivers` (`create_database_client`, replaces database.rs:882-922). Consumers that must *hide* disabled engines call `dbcrust_drivers::is_compiled(&t)`: `url_scheme.rs` (its own strum enum `UrlScheme`, url_scheme.rs:63-…; `UnsupportedScheme` at :157, :198), `shell_completion::complete_connection_url` (shell_completion.rs:21), help text built from `DatabaseType::supported_types()` (database.rs:159), gui-core `url_builder.rs` engine list (87 `DatabaseType::` refs, variants are exhaustive: no cfg needed) | `create_database_client` returns `DatabaseError::ConnectorNotBuilt { database_type, feature }`: *"MongoDB support is not compiled into this build (cargo feature `mongodb`). Install a build that includes it (e.g. `cargo install dbcrust` default features or the release binary)."* Same exit code class as `UnsupportedScheme`/`FeatureNotSupported`; one-shot JSON envelope gets `"error": "connector_not_built"`. `DatabaseType` variants, config.toml, sessions.toml, `recent.toml` all keep parsing |
| **files** | `cli_core.rs` file-extension inference is runtime on `DatabaseType::{Parquet,CSV,JSON}` (no cfg); picker `file://` (cli_core) hidden through `is_compiled`; `DatabaseType::DuckDB` arm (database.rs:918) maps to the same feature | typed error as above |
| **docker** | db.rs:5-7 (imports), :153-160 `DockerConnection`, :170-178 struct fields `docker_tunnel/docker_route/docker_daemon`, :220-258 (`docker://` branches in `from_url*`), :336-640 (9 methods: `from_docker_url*`, `connect_docker_container`, `docker_daemon_pin`, `docker_route`, `resolve_docker_connection_info`, `select_docker_container`, `from_docker_url_with_tracking*`), :2622 `Drop` (aborts the relay); `cli_core.rs:1507-1514`; `commands.rs:160` + :979 + :2090 + :3127 (`ListDockerContainers`); `url_scheme.rs:9` (`DockerClient`); `config.rs:729,3026` use only core `DockerTarget` (no cfg); gui-core `docker.rs` (whole module), `connect.rs` (`DockerError` handling), `launcher.rs` | `docker://` → `ConnectorNotBuilt{feature:"docker"}`; `\docker` command and variant absent (strum list shrinks automatically because `#[cfg]` on enum variants is applied before the derive) |
| **ai** | cli_core.rs:92,97,227,228 and the whole AI wizard :2362-3053 (≈700 lines); commands.rs:1502,1537, :2959-3076; config_editor.rs:232-247 (schema rows use core `AiConfig` → no cfg) and :844-846 (`genai::adapter::AdapterKind` → `dbcrust_ai::is_known_provider` under `cfg`); lib.rs:975 `ai_config_status` and :1043 `run_ai_investigation` (both registered unconditionally at lib.rs:279-280 and imported *by name* from `dbcrust._internal` in python/dbcrust/__init__.py:26 and django/analyzer.py:502, so with `ai` off they stay as stubs: `ai_config_status` reports `enabled = false`, `run_ai_investigation` raises a "not compiled in" error); `secrets_cli/audit.rs:401-402`, `migrate.rs:22` (→ secrets `legacy::ai_keys`, no cfg) | `??`, `\ai`, `--ai-*` report "AI support is not compiled in (feature `ai`)"; `[ai]` in config.toml still parses (C8) |
| **explain-tui** | db.rs:187,771,1655,1749,1982-1993,2181 (`explain_tui_mode` flag + `execute_explain_with_tui`), commands.rs:1317-1338 (toggle command), :1401-1415 (`schema_tui`), lib.rs `pub mod explain_tui`/`schema_tui` | `\x` plan TUI toggles report "feature not compiled"; plain EXPLAIN formatting unaffected; gui-core keeps using `explain_plan` |
| **clipboard** | commands.rs:2030 (`Command::CopyExplainPlan`, `arboard::Clipboard`) only | command prints the plan JSON path/text instead (or "not compiled") |
| **python** | lib.rs:70-1230 (unchanged) | – |
| **broker** | `#[cfg(unix)]` today (broker/mod.rs); consider a `broker` feature later so `standard` slim builds can drop `dbcrust-broker` (libc, agent-protocol); not part of this plan | – |

Rules enforced by CI (section 5.5): `cargo check -p dbcrust --no-default-features` (zero connectors) must build; each single engine alone; `standard`; `bundled`; `python + bundled`; and `cargo tree -p dbcrust --no-default-features` must contain none of sqlx, datafusion, mongodb, bson, clickhouse, elasticsearch, genai, bollard, ratatui, arboard.

---

## 5. Workspace mechanics

### 5.1 Manifest layout

```toml
[workspace]
resolver = "3"                                   # explicit (edition 2024 root package already implies it)
members = ["crates/*", "gui", "gui/core"]       # crates/agent, agent-protocol + the new ones
# default-members cannot include GPUI crates (system libs); list explicitly, glob support for
# default-members is not guaranteed on the pinned cargo
default-members = [".", "crates/agent", "crates/secrets", "crates/core", "crates/drivers",
                   "crates/driver-postgres", …, "crates/docker", "crates/ai", "crates/broker"]

[workspace.package]
version      = "0.38.0"
edition      = "2024"
rust-version = "1.91"
license      = "MIT"
repository   = "https://github.com/clement-tourriere/dbcrust"
publish      = false

[workspace.dependencies]   # every third-party dep of root today (Cargo.toml:30-125) + internal path deps
sqlx = { version = "0.8", default-features = false }          # features chosen per crate
clickhouse = { version = "=0.15.1", default-features = false, features = ["lz4","rustls-tls-ring","rustls-tls-webpki-roots"] }
dbcrust-core = { path = "crates/core", default-features = false }
# … one line per internal crate

[workspace.lints.clippy]   # exactly the table at Cargo.toml `[lints.clippy]` (6 allows)
```

* Each crate: `version.workspace = true`, `edition.workspace = true`, `rust-version.workspace = true`, `[lints] workspace = true`, `dep = { workspace = true, features = […] }`. Internal deps always `default-features = false`.
* Lints: `gui`, `gui/core`, `crates/agent*` currently have no `[lints]`; inheriting the six *allow* entries cannot introduce new `-D warnings` failures.
* **Version/release**: `.cz.toml` `version_files` bumps `Cargo.toml:^version = ` — with `[workspace.package]` the only line that matches is the workspace one (the root `[package]` uses `version.workspace = true`, which does not match `^version = `). `gui/Cargo.toml` and `gui/core/Cargo.toml` entries must then be **removed** from `version_files` (they inherit via `version.workspace = true`; the anchored regex would match nothing there, which commitizen reports as an error or silently skips depending on version: do not rely on it). Bonus: `crates/agent*` are not in `version_files` today (they are `0.38.0` by hand); inheritance removes the drift. `pre_bump_hooks = ["cargo check", …]` regenerates `Cargo.lock`.
* **rust-version**: workspace `1.91`. The AWS crates later need `1.94.1` (aws-common.md): give them `rust-version = "1.94.1"` on the crate, **exclude them from `default-members`** and from the MSRV/i686 jobs (a `cargo +1.91 test --workspace` would fail to build them). `duckdb` needs 1.85.1 (fine).
* **Profiles** stay in the root manifest only (`release`, `fast-release`, `dev`, `gui-dev`, `gui-release`, `bench`: Cargo ignores profile tables in member manifests with a warning). `[profile.gui-dev.package."*"]` matches only non-member crates, so the new member crates get `opt-level = 1` like root today.
* `[package.metadata.cargo-machete] ignored = ["reqwest_genai"]` moves with the alias to `crates/ai/Cargo.toml`.
* `[lib] crate-type = ["cdylib","rlib"], required-features = ["python"]` stays on root only; every other crate is a plain `rlib`.

### 5.2 Per-crate dependency declarations (essential differences)

* sqlx is declared per driver with the *union of today's root features* first (zero behaviour risk), trimmed later: postgres `["postgres","runtime-tokio-rustls","chrono","json","uuid","ipnetwork","rust_decimal"]`, mysql `["mysql","runtime-tokio-rustls","chrono","json","uuid","rust_decimal"]`, sqlite `["sqlite","runtime-tokio","regexp","chrono"]`. The existing SQLx/MySQL `RUSTSEC-2023-0071` exception in `.cargo/audit.toml` still applies (the `rsa` crate stays in the lock while `dbcrust-driver-mysql` exists).
* `pgvector = { features = ["sqlx","halfvec"] }` only in `driver-postgres`.
* core: `sqlparser = { version = "=0.61.0", default-features = false, features = ["std","visitor"] }`; verify `cargo tree --locked -e features -i sqlparser` is unchanged and `Cargo.lock` gains no package.
* `dbcrust-gui-core`: `dbcrust = { path = "../..", default-features = false, features = ["bundled"] }` (or its own `default = ["bundled"]` forwarding). Use *the same feature set as root's default* so `cargo build` and `cargo build -p dbcrust-gui` share compiled artifacts (different feature sets recompile every crate once more, ~1 min of CPU).

### 5.3 CI / release / mise

| File | Change |
|---|---|
| `.github/workflows/ci.yml` `rust` job (lines 16-38) | `cargo fmt --check` unchanged; clippy/test are `--workspace --exclude dbcrust-gui --exclude dbcrust-gui-core` or rely on the extended `default-members`. Add a feature matrix job (zero, each engine, `standard`, `bundled`, `python+bundled`) with `cargo hack check -p dbcrust --each-feature --no-dev-deps` (or a hand matrix) |
| `rust-i686` (ci.yml:40-62) | command unchanged (`-p dbcrust --lib --features python`); now also type-checks the sub-crates on i686 |
| `security` | `check_cargo_cooldown.py --base …` must tolerate path packages (no `source`): confirm in the W0 PR; `cargo audit` unchanged |
| `gui-core`, `gui-app` jobs | unchanged commands; `-p dbcrust-gui-core` now builds root + sub-crates as deps |
| **new** `deps-guard` step | `cargo metadata --locked --format-version 1` + the allow-list check of 2.1 (Appendix A, B) |
| `build-binaries.yml` | maturin wheel builds (`--features python`, lines 77-280) and `cargo build --release --target … --bin dbcrust --bin dbc --bin dbcrust-agent` (:320, :355 with `cross`) are workspace-transparent. The sdist step (:42-50) must still include `crates/*` path dependencies: already true for `agent-protocol`; add an explicit "sdist builds offline" check at W0 |
| `mise.toml` | `build`, `build:dev`, `test`, `lint`, `install` work on default-members. Add `test:all = "cargo test --workspace --exclude dbcrust-gui"`, `check:features` (the matrix), `deps:check`. `gui:test` unchanged. `install` already installs `crates/agent` |
| `hk.pkl` | `cargo clippy --all-targets -- -D warnings` runs on default-members: OK after the extension; per-commit hook time grows linearly with crates touched |
| `CLAUDE.md` / `AGENTS.md` module map | one row per crate per extraction PR |
| `.cz.toml` | see 5.1 |

### 5.4 mbx cache and codegen: what compiles in parallel

Compile waves (after E1–E6; arrows are real `cargo` edges):

```text
wave A (no workspace deps) : dbcrust-secrets ∥ dbcrust-core ∥ dbcrust-agent-protocol          (5.2k ∥ 13.7k lines)
wave B                     : 8 drivers ∥ dbcrust-docker ∥ dbcrust-ai                          (≤3.0k each; docker 4.9k, ai 3.3k)
wave B'                    : dbcrust-drivers (needs the enabled drivers)
wave C                     : dbcrust-broker (core+secrets+drivers+protocol)
wave D                     : dbcrust root lib (42k lines, cgu=1) → bins dbcrust, dbc            ← new critical path
```

Third-party compile is independent of this and is already cached by mbx (dev baseline: 865 hits / 12 misses). What changes:

* **Dev, root-only edit** (e.g. `commands.rs`): only the ~42k-line root recompiles instead of 89k. Model (compile time ∝ lines; baseline root lib 33.8 s fresh, 26.8 s on touch for 89k lines → 0.30–0.38 ms/line): ≈ 13–16 s + link 5 s.
* **Dev, driver edit**: driver (1–3k lines: 1–4 s; sqlx macros dominate) + registry + broker + root re-check ≈ same as a root edit. The split does not speed this loop; Phase B (root → ~20k) does.
* **Dev, core edit**: invalidates everything (13.7k lines is the new "hot" crate). Keep core quiet: no application logic, no frequently edited string tables; `DatabaseType` additions rebuild all drivers (≈ today's cost).
* **Release (cgu=1 + thin LTO + opt-level=z)**: today 5 m 48 s wall with only ≈163 s of user+sys CPU (110.6 + 52.2): the wall time is a single-threaded crate. With the split, each crate gets its own cgu=1 codegen, concurrent across a wave. Critical path (model, same ms/line): core ≈ 1.0 min → `driver-postgres`/`files` ≈ 0.3 min → registry → root 42k ≈ 2.4 min → LTO/link ≈ 0.4 min ≈ **3 m 50 s–4 m 10 s (−28 to −34 %)**. Phase B (root ≈ 20k) → ≈ 3 m. These are model numbers; measure with `cargo build --release --timings` after E2 and E3 and update this table. LTO is thin, so cross-crate inlining of core helpers is preserved; no `#[inline]` annotations should be needed.
* **`mbx`**: confirm after E2 whether path/workspace crates are cacheable (`mise exec -- mbx explain`, `mise run cache:stats`). If mbx only caches registry crates, the gain remains Cargo's own per-crate fingerprinting and incremental, which is what matters for the dev loop.
* **Binary size/features**: no change at `default` (same code). A `standard` build drops DataFusion (+ mongodb, elasticsearch, clickhouse, genai, bollard, ratatui, arboard): `cargo bloat --crates` is still pending from Wave 2a (section B), so no size figure is claimed here.

### 5.5 Dependency guard (forbidden-dependency checks)

Per crate, assert with `cargo tree --locked -p <crate> -e normal --prefix none | sort -u` against the lists in 2.1. Minimal manual form for each step:

```sh
for c in dbcrust-core dbcrust-secrets dbcrust-driver-sqlite dbcrust-docker dbcrust-ai dbcrust-broker; do
  echo "== $c"; mise exec -- cargo tree --locked -p $c -e normal --prefix none | sort -u \
    | grep -E '^(sqlx|sqlx-[a-z]+|datafusion[a-z-]*|arrow[a-z-]*|parquet|mongodb|bson|clickhouse|elasticsearch|genai|bollard|ratatui|crossterm|reedline|inquire|keyring|pyo3|arboard|reqwest|argon2|aes-gcm) '
done      # expected hits: only what 2.1 allows for that crate
mise exec -- cargo tree --locked -p dbcrust-agent -e normal | grep -E 'dbcrust-(core|secrets|drivers)|sqlx|keyring'   # must stay empty
```

---

## 6. Ordered extraction steps

Conventions: **every step is one PR, builds green, no behaviour change**. "Verify" = the *common checklist* plus the step-specific items.

**Common checklist (run through `mise exec --`; `DBCRUST_CONFIG_DIR=$(mktemp -d) RUST_TEST_MODE=1`)**

1. `cargo fmt --check`
2. `cargo clippy --locked --all-targets -- -D warnings` (all default-members)
3. `cargo test --locked` (and `--workspace --exclude dbcrust-gui` from E1 on)
4. `cargo check --locked -p dbcrust --lib --features python` (and, for dependency-touching steps, the i686 command of ci.yml:62)
5. `mise run gui:test` (= `cargo test -p dbcrust-gui-core`) and `cargo check --locked -p dbcrust-gui`
6. `mise run py:test` (pure-Python suite) and, at E2/E3/F1, a `maturin develop` + `python -c "import dbcrust._internal"` + one-shot SQLite query smoke
7. `cargo tree --locked -p <new crate> …` forbidden-dependency check (5.5)
8. `python3 .github/scripts/check_cargo_cooldown.py --base HEAD` when `Cargo.lock` changed (it must report no new registry package for pure moves)
9. Layer lint (Appendix A) — no upward edge for the layers reached so far
10. Update CLAUDE.md module map row(s)

### Step 0 — freeze and baseline (no PR)

* Wait for the broker work (untracked `src/broker/`, modified `database*.rs`, `safety.rs`, `config.rs`, `cli_core.rs`, `lib.rs`, `cli.rs`, `main.rs`) to be committed; start every worktree from the same green commit. A mass move during in-flight edits hides behavioural review and conflicts in exactly the files below.
* Record `cargo build --timings` (dev + release) and `cargo tree -e normal | wc -l` (1773 lines today) as the baseline.

### W0 — workspace manifests (Cargo only)

* Files: `Cargo.toml` (+ `[workspace.package]`, `[workspace.dependencies]`, `[workspace.lints.clippy]`, `resolver = "3"`, `members = ["crates/*", "gui", "gui/core"]`), `gui/Cargo.toml`, `gui/core/Cargo.toml`, `crates/agent/Cargo.toml`, `crates/agent-protocol/Cargo.toml`, `.cz.toml`, `mise.toml`, CI.
* Diff: ≈250 lines of TOML. Risk: **low–medium**: dependency feature sets must not change. Verify: `cargo metadata --locked` before/after identical resolve for each member (`cargo tree -e features --workspace` diff), `Cargo.lock` byte-identical (or only `[[package]] dependencies` ordering), `cz bump --dry-run` bumps one line, sdist still builds.
* Owner: integrator.

### P1 — DTOs to `model` (inside root)

* Move db.rs:2507-2563 + :103-128 + :16-80 → `src/model.rs`; `db.rs` adds `pub use crate::model::*;`. Edit database.rs:1196 and format.rs:2 to `crate::model`.
* Diff: ~180 lines moved, ~10 edited. Risk: low. Parallel: yes.

### P2 — leaf extractions: `options`, `output_format`, `docker_target`

* `src/options.rs` ← config.rs:15-37, database.rs:66-90; `src/output_format.rs` ← cli.rs:651-664; `src/docker_target.rs` ← docker.rs:1019-1211 (+ `DockerClient::parse_docker_url` delegates; `database.rs:954` calls `docker_target::DockerTarget::parse`). `password_sanitizer.rs:45`, `database_mysql.rs:611`, `database_mongodb.rs:280`, `broker/gateway.rs:115` switch to `crate::options`.
* Diff: ~260 moved, ~30 edited. Risk: low. Parallel: yes (touches `config.rs` and `database.rs` hunks distinct from P1).

### P3 — `paths` and secret hints

* `src/secrets/paths.rs` (module of the existing secrets module) ← config.rs:3700-3822 + 1444-1470; `Config::get_config_directory`, `is_test_mode`, `set_config_dir_override`, `write_file_atomically` delegate (`pub use`). `secrets/default.rs:21,43` use `super::paths`. `hint_legacy_secret_used` and `warn_store_problem` (config.rs:221-249) → `secrets/hints.rs`; drop the `one_shot_mode()` test (redundant, cli_core.rs:477).
* Diff: ~200 moved, ~25 edited. Risk: **medium** (test isolation, 7.3): run the whole suite with `RUST_TEST_MODE` **unset** and a bare-thread canary test. Parallel: yes after P2 (shares config.rs but distinct hunks).

### P4 — AI decoupling

* `ai/config.rs` → `src/ai_config.rs` (+ `ai::config` re-export); `ai/key_storage.rs::legacy` → `src/secrets/legacy_ai_keys.rs` taking `suggested_providers: &[String]`; update secrets_cli audit.rs:401, migrate.rs:22; define `AiSession` trait (ai/mod.rs) with the ~10 methods of C9, `impl AiSession for Database` in `db.rs`; the two genai-dependent tests of tls.rs:36-54 move to `src/ai/` tests.
* Diff: ~400 moved, ~120 edited (agent.rs, schema_context.rs, lib.rs:1099-1141, cli_core.rs:2558-2581). Risk: medium (trait object over `Arc<Mutex<…>>`, `!Send` futures; gui worker is current-thread). Parallel: yes.

### P5 — `DatabaseError` decoupling

* database.rs:849-880: replace `SqlxError`/`DataFusionError`/`Docker(#[from])` by `Driver { engine, source }` and `Docker(Box<dyn Error + Send + Sync>)`; add `ConnectorNotBuilt` and `#[non_exhaustive]`. In the four drivers add `fn db_err(...)` and rewrite the `?` sites (≈62). Delete `Database::get_pool` (db.rs:1387) and `_ssl_mode` (db.rs:807, lib.rs:326). Add `impl From<DockerError> for DatabaseError` in docker.rs.
* Diff: ~150 lines edited. Risk: medium (error text for sqlx errors must stay identical: golden tests exist in drivers via `redact_text`; add one per driver). Parallel: no — owns `database.rs` error section and the four drivers' `?` sites; merge before P6.

### P6 — factory out of `database.rs`, ES/complex-display cleanups

* `src/drivers.rs` ← database.rs:882-922 (+ `is_compiled`/`compiled_engines`, `ConnectorNotBuilt` arms behind a macro; no features yet: everything `true`); `db.rs:3,751` and `broker/gateway.rs:26,194` call `crate::drivers::…`. C13 (`needs_quoting` → core module path `database::elasticsearch_index_needs_quoting`), C14 (`impl ComplexDisplayConfig` → complex_display.rs).
* Diff: ~120. Risk: low. Parallel: after P5.

### P7 — root-internal splits that unblock core

* `sql_buffer.rs`: `SqlValidator` → `src/sql_validator.rs`; `explain_tui/plan_tree.rs` → `src/explain_plan.rs` (`explain_tui` re-exports); `format.rs` and `regex_operators.rs` keep `datafusion::sql::sqlparser` for now (switched to the direct `sqlparser` dep in E2).
* Diff: ~850 moved, ~15 edited. Risk: low. Parallel: yes.

### P8 — `ConnectionInfo: Default` and literal cleanup

* `impl Default for ConnectionInfo` (PostgreSQL, everything `None`/empty); convert the 57 struct literals to `..Default::default()` (config.rs 15, datafusion 8, database.rs 6, …, gui-core 4). Zero behaviour change; makes the later `secrets: SecretMap` field (README §5) a one-file change.
* Diff: ~250 lines, mechanical. Risk: low. Parallel: no (touches many files: land early, rebase everything else once).

### P9 — broker front-end split (only after the broker lands)

* `broker/{cli,mod::run,e2e_tests}` stay root-side; the rest only references `crate::{database, config, db, safety, secrets, password_sanitizer, drivers}` through paths that move together (C10).
* Diff: ~60 lines. Risk: low. Owner: whoever owns the broker.

> **Layer check at this point:** the Appendix A script, run with the target-crate assignment, must report **zero upward edges**. From here on each extraction is `git mv` + manifests.

### E1 — `dbcrust-secrets`

* Move `src/secrets/**` (incl. `paths`, `hints`, `legacy_ai_keys`), `password_encryption.rs`, `dbcrust_pass.rs`, `vault_encryption.rs` → `crates/secrets/src/` (+ `legacy/` mod). Root: `pub use dbcrust_secrets as secrets;`, `pub use dbcrust_secrets::legacy::{…}`. Workspace member + `[workspace.dependencies] dbcrust-secrets`.
* Moved: ~5.2k lines; edited: ~60 (every `crate::password_encryption` etc. → `crate::legacy::…`; paths in tests). Risk: **medium** (7.3: `cfg!(test)`, `fsutil`/`testing` visibility, `include_str!("password_encryption.rs")` self-scan test).
* Verify: common + `cargo tree -p dbcrust-secrets` (no core, no reqwest) + `cargo test -p dbcrust-secrets` + the root suite without `RUST_TEST_MODE`.
* Win: first crate that compiles in parallel with the rest; keychain/argon2/aes-gcm leave root's compile; isolates the machine-key legacy reader.

### E2 — `dbcrust-core`

* Move `database.rs` (split into `driver/` files, re-exported flat), `model`, `options`, `docker_target`, `output_format`, `password_sanitizer`, `complex_display`, `vector_display`, `geojson_display`, `json_display`, `performance_analyzer`, `regex_operators`, `safety`, `format`, `sql_parser`, `sql_buffer` (pure), `explain_plan`, `ai_config`, `tls` → `crates/core/src/`. Root `lib.rs` gets the `pub use dbcrust_core::{…}` block of 3.2. `sed s/crate::db::/crate::model::/` etc. inside core. `sqlparser` direct dep; `usage` optional; `rustls` optional behind `tls`.
* Moved: ~13.7k lines; edited: ~300 (imports) + 12 `pub(crate)` → `pub`. Risk: **high** in volume, low in logic. Pitfalls to check: `include_str!` paths; `env!("CARGO_PKG_VERSION")` at database.rs:796 (now core's version = workspace version); strum derive needs `strum` in core; `#[cfg(test)]` tests in format.rs:1142,1430 that use `db::…` (now `model`).
* Verify: common + core forbidden-dep check + `Cargo.lock` gains **only** the local package entries + `cargo build --timings` recorded.
* Win: drivers and docker/ai can now compile in parallel with root; core is the only hot crate.

### E3.0 — `dbcrust-drivers` scaffold + empty driver crates

* Creates `crates/drivers` (from `src/drivers.rs`) and 8 stub driver crates (`Cargo.toml` + empty `lib.rs`) with all optional deps declared, workspace members and lock entries, root feature forwarding (4.1). After this PR **no later driver PR touches `Cargo.toml`/`Cargo.lock` of root or `crates/drivers`** except the arm switch.
* Diff: ~400 lines of TOML. Owner: integrator.

### E3.1…E3.8 — one driver per PR (order: sqlite, white-dragon, postgresql, mysql, clickhouse, elasticsearch, mongodb, files)

* `git mv src/database_X.rs crates/driver-X/src/lib.rs`; `sed s/crate::/dbcrust_core::/g`; switch the registry arm; delete `pub mod database_X;` from `lib.rs`; the root re-export `pub use dbcrust_driver_X as database_X;` is **optional** (nobody outside `database.rs` referenced them except `completion.rs:334`, already fixed).
* Moved: 1.0–3.0k each; edited: ~20 (imports). Risk: low (sqlite) → medium (postgresql: pgvector, `include_str!` source scan at database_postgresql.rs:2636, `DATABASE_URL`-gated tests; files: DataFusion feature set). Verify: common + `cargo tree -p dbcrust-driver-X` (only its SDK + core) + `cargo check -p dbcrust --no-default-features --features X`.
* Win per PR: its SDK's generic instantiations leave root's codegen; sqlx/datafusion monomorphisation is the expensive part of root today.
* `files` last: it is the only one that needs the `sqlparser` re-pin check (`datafusion` must still resolve `sqlparser 0.61.0`).

### E4 — `dbcrust-docker`

* Move `docker.rs` (minus `DockerTarget`), `docker_endpoint.rs`, `docker_route.rs`, `docker_tunnel.rs`, `docker_relay.pl` → `crates/docker/src/`. Root `docker.rs` becomes the facade of 3.2; gate on a temporary always-on dependency (feature comes in F1). Moved test: docker.rs:1710-1751 → root `db.rs` tests.
* Moved: ~4.9k; edited: ~40. Risk: low–medium (`include_str!("docker_relay.pl")` path; `pub(crate)` in `docker_route`/`docker_endpoint`). Parallel with E3.x and E5.

### E5 — `dbcrust-ai`

* Move `src/ai/**` (minus `config.rs`, legacy) → `crates/ai/src/`; move `genai` and the `reqwest_genai` alias (+ `cargo-machete` ignore) to its manifest; `AiSession` stays defined in the crate. Root `ai.rs`: `pub mod config { pub use dbcrust_core::ai_config::*; } pub use dbcrust_ai::*;`.
* Moved: ~3.3k; edited: ~50. Risk: medium (reqwest 0.12/0.13 duplicate versions and the TLS provider test move; `genai =0.7.0-beta.2` exact pin; `tls::install_default_crypto_provider` must be called before the first client: cli_core and lib.rs already do it).
* Verify: `cargo tree -p dbcrust-ai` (no sqlx, no drivers, no docker) + `cargo test -p dbcrust-ai` (includes the aws-lc regression test of tls.rs:36).

### E6 — `dbcrust-broker`

* Move `broker/{admin,audit,gateway,hardening,paths,registry,server,testkit}` (≈5.4k) → `crates/broker/src/`; keep `cli.rs`, `mod.rs::run`, `e2e_tests.rs` in root. Depends on `dbcrust-drivers` (`create_database_client`).
* Risk: medium (unix-only `#[cfg(unix)]` module list, hardening tests that spawn the broker binary via `env!("CARGO_BIN_EXE_…")`-like paths; `e2e_tests.rs` exercising `cli::run` must remain next to the CLI). Must wait for the broker author (sections G.4/G.6 evolve the module list).
* Verify: common + `cargo tree -p dbcrust-broker` (no ai, docker, reedline) + `cargo tree -p dbcrust-agent` still empty of core/secrets/drivers.

### F1 — feature gating

* `Cargo.toml` features (4.1); `#[cfg(feature = …)]` at the sites of 4.2; `dbcrust-drivers` arms `cfg`'d per feature with `ConnectorNotBuilt`; gui-core `features = ["bundled"]`; PyO3 `ai_config_status`/`run_ai_investigation` bodies gated by `ai` (symbols stay registered, see 4.2).
* Diff: ≈400 lines across 10 files. Risk: **medium-high** (combinatorics): the matrix of 4.2 is the test.
* Verify: `cargo check -p dbcrust --no-default-features` (+ each engine alone, `standard`, `bundled`, `python`), `cargo tree -p dbcrust --no-default-features` free of the heavy list, one regression test per disabled-engine error message and a config.toml containing `[ai]` and a `mongodb://` session parsing in the zero-feature build.

### F2 — CI/mise/docs

* Matrix job, deps-guard step, `mise` tasks, CLAUDE.md/AGENTS.md/`docs/src/content/docs/user-guide/development.md` (workspace map, feature table, "how to add a driver" = 1 crate + 1 registry arm + 1 feature). Risk: low.

### F3 — default preset decision (owner decision, optional)

* Flip `default = ["standard"]` only in an announced release; wheels/binaries/GUI build with `bundled` explicitly. Not part of this plan's green path (D3).

### Parallelisation plan

| Phase | Parallel worktrees (agents) | Shared files — single owner |
|---|---|---|
| after step 0 | **W0** (manifests) ∥ **P1** ∥ **P2** ∥ **P4** ∥ **P7** | `Cargo.toml`/`Cargo.lock`: W0 owner only. `src/lib.rs`: integrator (each PR adds only a `pub mod` line; lines are alphabetically spread, expect trivial conflicts). `database.rs`: P1 (:1196), P2 (:66-90, :954) touch different hunks; P5/P6 own the rest |
| then | **P3** ∥ **P8** (P8 lands first, then everyone rebases) ∥ **P5 → P6** (sequential) | `config.rs`: P2 + P3 touch distinct regions; P8 touches 15 literals: land P8 before E-steps |
| E-steps | **E1** ∥ **E2** may run in parallel *authoring* (both only need P1–P7) but merge E1 → E2 → E3.0 | integrator owns `Cargo.toml`, `Cargo.lock`, `crates/*/Cargo.toml`, `src/lib.rs` for the whole phase |
| after E3.0 | **E3.1…E3.8** (8 agents) ∥ **E4** ∥ **E5** | each driver PR touches `crates/driver-X/**`, one registry arm, one `lib.rs` line; integrator merges sequentially and regenerates `Cargo.lock` |
| last | **E6** (after the broker settles), **F1**, **F2** | F1 touches 10 files: single author |

Estimated calendar (engineering days, one contributor-equivalent each): W0 1, P1 0.5, P2 0.5, P3 1, P4 1.5, P5 1.5, P6 0.5, P7 0.5, P8 1, E1 1.5, E2 3, E3.0 1, E3.x 0.5–1.5 each (≈6 total), E4 1.5, E5 1.5, E6 1.5, F1 2, F2 1 ≈ **27 days of work, ≈ 12–14 days wall with 3 parallel agents**.

---

## 7. Expected wins and risks

### 7.1 Wins per step (qualitative; numbers are the line-share model of 5.4)

| Step | Win |
|---|---|
| P1–P8 | Module graph becomes a layered DAG *inside* one crate (checkable by script). No compile-time effect; drastically reduces risk of the later PRs |
| E1 `secrets` | Native crypto/keychain code compiles beside, not inside, root; secret-handling gets its own test target and forbidden-dep list (no HTTP stack, no drivers) |
| E2 `core` | First real parallelism: wave A; the `DatabaseClient`/`DatabaseError` boundary is explicit; GUI/broker/drivers stop reaching into `db`/`config` |
| E3 drivers | Each SDK (sqlx ×3, mongodb/bson, clickhouse, elasticsearch, reqwest, datafusion) compiles in its own unit; a driver edit never re-checks another driver; root loses ≈14.3k lines |
| E4/E5 | bollard and genai(+reqwest 0.13) leave root; AI/Docker become removable (F1) |
| E6 | Broker auditable as a crate with a short dependency list: the property D1 is built on (no ai/reedline/docker/inquire in the credential-holding process) |
| F1 | A real `standard` build (no DataFusion/mongodb/ES/clickhouse/genai/bollard/ratatui/arboard); `cargo tree` proves it; `ConnectorNotBuilt` gives a stable error for the broker and GUI |
| Whole | Release wall 5 m 48 s → model ≈ 3 m 50 s–4 m 10 s (Phase A), ≈ 3 m with Phase B; root-only dev rebuild ≈ halves; driver authors work in crates of 1–3k lines; Wave-3 connectors become "new crate + registry arm + feature" instead of the 17-file matches (`plans/connectors/README.md` §4) |

### 7.2 PyO3 `cdylib + rlib`

* The root stays the only cdylib. Sub-crates are `rlib`s linked into the extension; `pyo3` (with `extension-module`, `abi3-py310`) appears only in the root manifest. `maturin` (`module-name = "dbcrust._internal"`, `features = ["python"]`, pyproject.toml `[tool.maturin]`) builds the root package and does not need changes.
* `[lib] required-features = ["python"]` is documented in section B as unreliable gating; gui-core keeps depending on the lib. Do not "fix" it in this migration.
* PyO3 abi3 + a workspace with `crate-type = ["cdylib"]` on a crate that has `rlib` dependants works; what must not happen is a second cdylib in the workspace that links `libpython` symbols: nothing else is a cdylib.
* Statics shared across crates (query timeout, read-only, interrupt flag, complex-display config) are fine: the cdylib embeds exactly one copy of each `rlib`. If a future crate ever builds as a `dylib`/`cdylib` that is *also* linked by the root, those globals would duplicate: do not introduce it.
* i686: all new crates are compiled for `i686-unknown-linux-gnu` by the existing job. ClickHouse `=0.15.1` pin must be a `[workspace.dependencies]` entry used only by `driver-clickhouse`.

### 7.3 Test-mode heuristics and config isolation (highest-risk item)

`is_test_mode()` (config.rs:3752) is `cfg!(test) || RUST_TEST_MODE || thread name contains "test"`. `cfg!(test)` is true **only in the crate being tested**. After the move, `dbcrust_secrets::paths::is_test_mode()` is compiled non-test when root's tests run, so on any thread that is not named like the test (tokio workers, `spawn_blocking`, `std::thread::spawn`) the function returns `false` and code reaches the real `~/.config/dbcrust`, `recent.toml` and keychain: exactly the incident recorded in `test-config-isolation.md` memory. gui-core already works around the same limitation (gui/core/src/test_support.rs:5-24 calls `dbcrust::config::set_config_dir_override`; gui/core/src/rt.rs:13 uses `cfg!(test)` for thread names).

Mitigation (E1, verified in E2):

1. `dbcrust-secrets` gets a cargo feature `test-mode`; `is_test_mode()` becomes `cfg!(any(test, feature = "test-mode")) || RUST_TEST_MODE || thread-name`.
2. Every crate whose tests may touch config/secrets enables it through a `[dev-dependencies] dbcrust-secrets = { workspace = true, features = ["test-mode"] }` entry (root, ai, broker, gui-core, gui). Dev-dependency features are not unified into normal builds with resolver 2/3, so `cargo build`/release never see it.
3. `secrets::default_store()` keeps returning the in-memory store whenever `is_test_mode()`; `with_store_override` stays thread-local (secrets/default.rs).
4. Add a canary test in root: spawn a bare `std::thread` and assert `is_test_mode()` and that `default_store()` is in-memory; run CI once with `RUST_TEST_MODE` unset.
5. `--test-threads=1` on the main thread still needs `RUST_TEST_MODE=1` (CLAUDE.md §5) — unchanged.
6. `DBCRUST_CONFIG_DIR` handling moves with `paths` (const at config.rs:18); gui `screenshots.rs:83` and agent `socket.rs:32` read the env var directly and are unaffected.

### 7.4 `pub(crate)` becoming `pub`

Cross-crate users found: `db::unquote_metadata_identifier`, `db::metadata_table_parts` (db.rs:16,32), `config::is_test_mode` (config.rs:3752, 7 call sites: secrets/default.rs:43, ai/key_storage.rs:197, secrets_cli/audit.rs:84, migrate.rs:53,716), `config::{connection_binding, tls_mode_from_options, SSH_PASSWORD_SECRET_OPTION}` (config.rs:160,183,203, used by `secrets_cli/migrate.rs` — stay root, no change), `config::hint_legacy_secret_used` (→ secrets `pub`), `config::lookup_connection_password` (config.rs:309; only `cli_core.rs:1007` calls it, `dbcrust_pass.rs:9` merely mentions it in a doc comment: stays in root, no change), `secrets::fsutil::{read_bounded, write_private_atomic}` and `secrets::testing::FaultyBackend` (used by `secrets_cli/migrate.rs:513,617,886,1010,1038-1424`: become `pub` + `#[doc(hidden)]`, `testing` behind a `test-support` feature), `ai::key_storage::legacy::*` (16 `pub(crate)` items; they move to `dbcrust_secrets::legacy::ai_keys`, so `secrets_cli` reaches them through a `pub` path). Everything newly `pub` should be `#[doc(hidden)]` where it is an implementation seam, and add `unreachable_pub`/`missing_docs` lints per crate only after the split (not during).

### 7.5 strum command lists

`DatabaseType` (`strum::EnumIter`, database.rs:136) moves to core and drives `supported_types()`, URL scheme completion and GUI engine lists. `Command`/`CommandShortcut` (commands.rs:20, 295) stay in root; `#[cfg(feature = …)]` on *variants* is applied before the strum derive sees the tokens, so help/autocomplete shrink automatically — but every `match` arm over those variants needs the same cfg (compiler-enforced). Add a test that `Command::iter()` contains no variant whose feature is off. `strum` stays a dependency of core **and** root (CLAUDE.md rule 1). `crates/agent` does not use strum.

### 7.6 `include_str!` / assets

* `agent_guide.md` (cli_core.rs:526): root, unchanged.
* `docker_relay.pl` (docker_tunnel.rs:58): moves with `docker_tunnel.rs`; the file must sit next to it in `crates/docker/src/` (or the path becomes `../`): `cargo package`/maturin sdist include it only if it is under the crate directory.
* Self-scanning tests (`include_str!("database_postgresql.rs")` :2636, `password_encryption.rs:237`): relative to the file, move together; make sure the scan does not assume the old `src/` path in its assertion messages.
* `docs/` links to source paths (CLAUDE.md module map, development.md) need one pass.

### 7.7 Other risks

| Risk | Where | Mitigation |
|---|---|---|
| Orphan rule | C4, C14; any later `impl From<ForeignErr> for DatabaseError` | explicit `db_err` helpers; core owns inherent impls of its types; the script in Appendix A flagged only C14 |
| Exact pins (`genai =0.7.0-beta.2`, `clickhouse =0.15.1`, `usage-rs`) duplicated in several manifests | W0 | single `[workspace.dependencies]` entry; `cargo-machete`/`udeps` ignore lists moved |
| Feature unification across the workspace (`cargo build --workspace` unions sqlx features) | all | per-crate `cargo tree -p` checks (5.5) and per-feature CI (4.2); never use `--workspace` for the engine matrix |
| gui-core `default-features = false` silently losing engines | F1 | explicit `features = ["bundled"]`; a gui-core test asserts `dbcrust_drivers::compiled_engines()` ⊇ the engines its URL builder lists |
| Core churn re-triggering a 13.7k-line rebuild of everything | E2 | keep application logic out; if it bites, split `core` along 2.4's module lines into `model+driver-api` and `sql+format+display` (section B names them separately) |
| Mass `sed` of `crate::` hiding semantic differences | E2/E3 | each driver PR is one file; clippy + tests per crate; `cargo public-api`/diff of `cargo doc --no-deps` for core before/after |
| `Cargo.lock` churn and 24 h cooldown | W0, E* | only local path packages and dependency edges change; `check_cargo_cooldown.py` run per PR; no `cargo update` |
| `cargo install --git … dbcrust` (update.rs:106, docs installation.mdx:76) | F2 | works unchanged: a git checkout resolves workspace path dependencies. crates.io publishing is already impossible (`dbcrust-agent-protocol` is a path dependency without a version, Cargo.toml:115) and stays so; do not add `publish = true` to member crates |
| Python behaviour parity | E2, F1 | `mise run py:test` is pure Python and **does not** exercise the extension: add a maturin-built smoke (import + one SQLite query + error text for a disabled engine) to CI |
| `DatabaseType::DuckDB` ambiguity (alias to DataFusion today, real connector later in Wave 3) | F1 | keep it mapped to `files`; the registry arm is the only place to change later |

---

## Appendix A — graph / layer-lint script (not committed; belongs in `.github/scripts/` when adopted)

```python
# usage: python3 graph.py   (run from the repo root; stdlib only)
import re, os, json, collections
files=[os.path.join(d,f) for d,_,fs in os.walk('src') for f in fs if f.endswith('.rs')]
top=lambda p:(lambda parts: parts[0][:-3] if len(parts)==1 else parts[0])(p.split(os.sep)[1:])
edges=collections.defaultdict(lambda: collections.defaultdict(list)); lines=collections.Counter()
for p in sorted(files):
    t=top(p); txt=open(p).read(); ls=txt.split('\n'); lines[t]+=len(ls)
    tstart=None
    for i,l in enumerate(ls):
        if re.match(r'\s*#\[cfg\(test\)\]',l) and any(re.match(r'\s*(pub\s+)?mod\s+\w+',x) for x in ls[i+1:i+4]):
            tstart=i; break
    testfile=os.path.basename(p) in ('e2e_tests.rs','testkit.rs','testing.rs','test_support.rs')
    for m in re.finditer(r'crate::\{([^;]*?)\}\s*;',txt,re.S):          # use crate::{a, b::c};
        depth=0;cur='';items=[]
        for ch in m.group(1):
            depth+=ch=='{';depth-=ch=='}'
            if ch==',' and depth==0: items.append(cur);cur=''
            else: cur+=ch
        items.append(cur); ln=txt[:m.start()].count('\n')+1
        for it in items:
            mm=re.match(r'\s*(\w+)',it)
            if mm: edges[t][mm.group(1)].append((p,ln,(tstart is not None and ln>tstart) or testfile))
    for i,l in enumerate(ls,1):                                           # crate::x::...
        for m in re.finditer(r'crate::(\w+)',l):
            edges[t][m.group(1)].append((p,i,(tstart is not None and i>tstart) or testfile))
# ---- layer lint: edit `assign` / `allowed` for the step you are verifying
core='database password_sanitizer complex_display vector_display geojson_display json_display performance_analyzer regex_operators safety format sql_parser sql_buffer tls model options docker_target output_format explain_plan ai_config'.split()
assign={m:'core' for m in core}
assign.update({m:'secrets' for m in 'secrets password_encryption dbcrust_pass vault_encryption'.split()})
assign.update({m:'docker' for m in 'docker docker_endpoint docker_route docker_tunnel'.split()})
assign.update({'database_'+k:'driver' for k in 'postgresql mysql sqlite mongodb clickhouse elasticsearch white_dragon datafusion'.split()})
assign.update({'ai':'ai','broker':'broker','drivers':'drivers'})
allowed={'secrets':set(),'core':set(),'docker':{'core'},'driver':{'core'},'drivers':{'core','driver'},
         'ai':{'core','secrets'},'broker':{'core','secrets','drivers'},
         'root':{'core','secrets','docker','driver','drivers','ai','broker'}}
for s,dd in sorted(edges.items()):
    a=assign.get(s,'root')
    for d,v in sorted(dd.items()):
        b=assign.get(d,'root')
        if a!=b and b not in allowed[a]:
            prod=[x for x in v if not x[2]]
            print(f"UPWARD {s}[{a}] -> {d}[{b}]  prod={len(prod)} test={len(v)-len(prod)}  e.g. {v[0][0]}:{v[0][1]}")
```

**Appendix B — dependency allow-list check (CI, once crates exist)**: `cargo metadata --locked --format-version 1 --no-deps`, then for each package in `{dbcrust-core, dbcrust-secrets, dbcrust-driver-*, dbcrust-docker, dbcrust-ai, dbcrust-broker, dbcrust-agent}` assert the resolved `cargo tree -p <pkg> -e normal --prefix none` set is disjoint from the "must not depend on" column of 2.1 (and, for the agent, from `dbcrust-{core,secrets,drivers,docker,ai,broker}`); fail with the offending path from `cargo tree -i <crate> -p <pkg>`.
