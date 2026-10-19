# DBCrust desktop app — architecture

A native, GPU-rendered database workbench built with [GPUI Kit](https://gpui-kit.com)
(`gpui-kit = "=0.7.1"`) on top of the shared DBCrust crates the CLI is built on
(`dbcrust-app`, `dbcrust-core`, `dbcrust-drivers`, `dbcrust-docker`,
`dbcrust-secrets`). It never depends on the CLI crate (`dbcrust`: REPL,
`inquire`, TUIs, broker): logic both front ends need belongs in those crates.

```
gui/
├── core/   dbcrust-gui-core — no GPUI. Workers, storage, pure models. `cargo test -p dbcrust-gui-core`
└── src/    dbcrust-gui      — GPUI app (lib + `dbcrust-gui` bin + `dbcrust-gui-screenshots` bin)
```

## Backend (`gui/core`)

| Module | Role |
|---|---|
| `worker.rs` | `ConnectionHandle`: one OS thread + current-thread tokio runtime + `LocalSet` per live connection. Owns the `Database` (and its Docker relay) for its whole life; commands are serialized; cancellation is out-of-band with a fresh token per request (installed as the thread's DBCrust interrupt flag). `start_connect` / `start_test` return a `ConnectAttempt` (phases as a `watch` channel, a cancel handle, the outcome with the Docker route or a `DockerFailure`). |
| `connect.rs` | URL / session / recent / Docker / Vault resolution and `Database` construction from a snapshot of the store's config (no config file reads while connecting). Docker routes are probed outside every lock; only the read-only process flag is gated (async, cancellable). Tests never write recents or the Vault credential cache. |
| `store.rs` | `ConfigStore`: the single in-process owner of DBCrust's `Config` (settings, sessions, recents, named queries). `save_settings()` keeps documentation comments. Saved queries (every scope) are changed in memory under the store lock and `named_queries.toml` is written after it is released, one change at a time; a failed write is rolled back, and a legacy `config.toml` entry is dropped only once its replacement is written; readers (`all_named_queries`, `named_queries`, `saved_query`) use a catalogue republished after each change and never wait for the lock. `update_named_query` creates, edits, renames and re-scopes in one mutation, refusing to overwrite and keeping `created_at`. Sessions, recents and the Vault cache are changed by their `Config` helpers on a copy, outside the lock (`update_sessions`, `update_recents`; one lock per file), then published, so the lock is never held for file I/O. `vault_cache()` lists the Vault credential cache without secrets and never changes it. |
| `statements.rs` | What "Run" executes: `StatementIndex` (built from `dbcrust_core::sql_buffer::split_statement_spans_with`, cached per revision) gives the selection, the statement that owns the caret, or everything. Smart mode also splits at blank lines between plain read queries (parsed with sqlparser; anything else needs `;`). Shared by run, highlight, provenance, Format and completion; the CLI keeps `;`-only splitting. |
| `schema.rs` | Parsing metadata output, identifier quoting, table references. |
| `export.rs` | CSV / TSV / JSON / Markdown rendering of result sets. |
| `completion.rs` | SQL completion candidates (keywords, functions, tables, columns, aliases). |
| `explain.rs` | Plan view model (PostgreSQL JSON → tree via `dbcrust_core::explain_plan`, SQLite/MySQL/DataFusion tabular plans). |
| `history.rs` | In-memory query history per connection. |
| `gui_state.rs` | Desktop-only persisted state (`~/.config/dbcrust/gui_state.toml`): open drafts (with the file or saved query they save to), theme, sizes, menu bar / tray behaviour, Connections page metadata. |
| `docker.rs`, `vault.rs` | Discovery for the Connections page (each on a short-lived runtime thread). `docker::discover_snapshot()` returns the containers with the daemon that listed them; `DockerDaemon::connect_request` pins a connect to it. How a container is reached (published port → OrbStack domain → container address → relay through `docker exec`) lives in the `dbcrust-docker` crate: `crates/docker/src/docker_route.rs`, `crates/docker/src/docker_tunnel.rs`. |
| `launcher.rs` | The Connections page's pure model: what the list holds (saved / recent / Docker, search, filters, dedup, selection rules), Docker publishing facts, connect-phase log, environment tags and their `gui_state.toml` part (`LauncherState`), removing a recent entry. |
| `env.rs` | Login-shell `PATH` import with timeout. |

Rules:
- Every async backend API returns a `Send + 'static` future that resolves on any executor — await it from `cx.spawn`. Never block the UI thread on I/O.
- Errors are `GuiError` (`Clone`, user-facing messages). `GuiError::Cancelled { abandoned }` is a notice, not a failure.
- Results are honest: `ResultSet::auto_limit` says DBCrust appended its default `LIMIT`; `is_truncated()` says the limit was reached.

## App (`gui/src`)

| Module | Role | Contract |
|---|---|---|
| `app_state.rs` | Global `Entity<AppState>`: live connections (`LiveConnection { handle, summary, schema, details, accent }`), active connection, store. Emits `AppEvent`. | `AppState::global(cx)`, `connect`, `start_attempt` / `cancel_attempt` / `dismiss_attempt` / `attempt_for` (Connections page attempts), `open_workspace`, `disconnect`, `refresh_schema`, `table_details`, `open_query`, `open_table`, `open_settings`, `open_home`, `storage_changed` |
| `actions.rs` | Every command as a GPUI action. | Views register handlers with `.on_action(cx.listener(..))`. |
| `commands.rs` | `AppCommand` catalogue (strum `EnumIter`) → key bindings, native menus, command palette. | |
| `workspace/` | Root view: title bar, resizable sidebar (Explorer), tab bar + tab content, status bar. Reacts to `AppEvent`s. `tabs.rs`: the tab strip (title, connection name + engine badge, inline rename, context menu). `lifecycle.rs`: drafts autosave, close-to-tray (hide the window, keep everything), the quit coordinator, confirm-before-disconnect. | `Workspace::new(window, cx)`, `request_quit`, `request_disconnect` |
| `home/` | The **Connections** page (first tab): two panes — navigation list (search, All/Saved/Recent/Docker, Vault action) and details / New-Edit form with a sticky footer. See below. | `HomeView::new(window, cx)`, `focus_new_connection`, `focus_primary`, `refresh` |
| `query/mod.rs` | Query tab: toolbar, SQL editor, results split. | `QueryTab::new(Option<ConnectionId>, Option<String>, String, window, cx)`, `title`, `connection`, `set_connection`, `sql`, `status`, `cursor`, `run_current`, `run_all`, `explain(analyze)`, `cancel`, `set_sql`; emits `QueryTabEvent` |
| `query/editor.rs` | Editor setup + completion provider. | |
| `query/results/` | Results panel: virtualized data grid (`grid.rs`), sort/selection model (`model.rs`), row inspector sheet (`inspector.rs`), multiple result sets, messages, errors, copy/export. | `ResultsView::new(window, cx)`, `show(ResultsContent, Option<ConnectionId>, window, cx)` |
| `query/files.rs` | Save targets: a tab loaded from or saved to a `.sql` file or saved query is bound to it (⌘S writes there, a dot marks unsaved changes); drafts keep their target across restarts. | `save_query`, `save_as`, `draft_target`, `restore_target` |
| `query/explain/` | Plan visualizer: tree, selected node, deduplicated plan observations. | `ExplainView::new(Arc<ExplainRaw>, window, cx)` |
| `explorer/` | Sidebar: live connections and their schema trees, filter, hover actions, context menus, inline load errors with Retry. | `Explorer::new(window, cx)` |
| `inspector/` | Table inspector tab (columns, indexes, FKs, references, data preview, DDL; FK links open the referenced table). | `TableInspector::new(ConnectionId, TableEntry, window, cx)`, `title`, `connection`, `table` |
| `settings.rs`, `settings/` | Settings tab (`SettingsPage`: General, Appearance, Queries, Saved queries, Connections, SSH tunnels, Vault, Results display, AI assistant, Terminal (CLI), Logging, Files). `schema.rs` puts every `config_editor::schema()` field on exactly one page with the widget of its `FieldKind` (a test fails when one is missing or shown twice); `storage.rs` holds the `SettingsCache` controls read (never the store lock); an edit reaches the store at once, in memory (connections opened next see it), and only the `config.toml` write is deferred (one save at a time, `SAVE_DELAY` after a burst; a failed save stays pending; the quit saves until nothing is unsaved, within the quit deadline, on its own thread); Reload is refused while a change is unsaved; `saved_queries.rs` is the saved query manager (virtualized list + editor); `tunnels.rs` edits SSH patterns and tests hosts without running backtick commands. | `SettingsView::new`, `select_page(SettingsPage)`; `AppState::open_settings_page`, `settings::flush_on_quit` |
| `preferences.rs` | `Preferences` global for the desktop-only settings, persisted to `gui_state.toml` through an outbox (acknowledged only after a successful write, retried, flushed on quit). | `preferences::editor_font_size(cx)`, `restore_drafts(cx)`, `observe_global::<Preferences>` |
| `tray.rs`, `tray/` | Menu bar / system tray icon (`tray-icon`): menu model (pure, tested), native events forwarded through a channel into GPUI, hide/show of the main window (`NSWindow orderOut`, `ShowWindowAsync`). No tray on Linux. | `tray::init` / `install`, `show_main_window`, `close_action`, `request_disconnect` |
| `palette.rs` | Command palette (⌘K). | |
| `widgets.rs` | Shared presentation helpers: engine badge, connection accent colors, environment tag. | |
| `theme.rs` | Theme registry + a surface pass on every theme change (sidebar vs canvas, selection strength, readable muted text, scrim). | `theme::set_mode`, `surfaces(cx)`, `tint_text` |

Communication: views never hold each other. They call `AppState` methods; the workspace subscribes to `AppEvent` (open tabs, refresh) and to each tab's events (status bar, titles).

## Connections page (`gui/src/home/`)

The first tab. Layout (logical px): left pane 320 (280 below a 1100 px window), right pane fills, content column max 640, sticky 56 px footer, list rows 52, controls 32, detail heading 22, engine badge 32. While it is the active tab the Explorer is not drawn (`Workspace::explorer_visible`): the launcher's own list is the navigation, so there are never two lists of connections side by side.

| File | Role |
|---|---|
| `mod.rs` | `HomeView`: data (sessions, recents, Docker), filter + search → `Listing`, selection, keyboard (↑↓ ↵ ⇧↵ ⌘E ⌘⌫ ⌘↵ Esc), connect / test / save / remove, display metadata (`LauncherState`). |
| `list.rs` | Left pane: title + New, search, filter chips, one virtualized `gpui::list` (rows of different heights), fixed "Connect with Vault…". |
| `details.rs` | Right pane for a selection: name, engine, environment, access, Docker facts (container state, database port, publishing, route), footer actions. |
| `form.rs` | New / Edit form: searchable grouped engine `Select` (Servers / Files; Docker and Vault are sources), Name, real defaults, TLS summary + disclosure, environment, read-only, "Paste connection URL…", Test / Save / Connect. `save_session` persists without connecting. |
| `attempts.rs` | Attempt panels: phases + elapsed + Cancel, test result, failure with Retry / Edit connection / Connection details (routes tried, `ports: ["127.0.0.1::5432"]`). |
| `docker.rs` | Discovery and the loading / empty / unavailable states. |
| `vault.rs` | The Vault wizard dialog. |

Rules:
- **Attempts belong to targets.** `AppState::start_attempt` takes an `AttemptSpec { kind: Connect | Test, target, subject, request }`; `target` is the list entry's identity (`launcher::session_key` / `recent_key` / `docker_key`, `FORM_KEY` for the form, `…#test` for its test). A target never has two attempts running; a failure or test result stays on its target until retried or dismissed. Phases come from `ConnectionHandle::start_connect/start_test` (`ConnectPhase`), Cancel calls the backend's `ConnectCancel`. Connect failures do not raise the global notification (`AppEvent::ConnectFailed` is for connects started elsewhere).
- **Save ≠ Connect.** Save writes `sessions.toml` (no password: DBCrust never stores one there) without connecting; Connect records a recent entry only; Test records nothing. Environment label/colour and the read-only preference live in `gui_state.toml` (`launcher`), keyed by identity, never next to secrets.
- **Selection is stable.** Discovery (Docker, disk reloads) never changes the selection or the form; the last selection is restored once its item exists.
- Keys are handled at the page root (`on_key_down`) while the focus is in the list area or the search box, never inside the form; ⌘E / ⌘↵ arrive as the app's `ExplainStatement` / `RunStatement` actions, which mean nothing outside a query tab.

Screenshot states (`--only home-new|home-edit|home-docker|home-docker-published|home-docker-loading|home-docker-empty|home-docker-error|home-failed|home-connecting|home-first-run|home-engine-menu`) are not part of the docs set; they exist to look at each state. `--only settings-saved-queries` and `--only settings-ssh-tunnels` render the Settings managers with seeded data.

## Design language

Follow the GPUI Kit design guide (`skills/gpui-kit-design-guides`). In short:

- **Tokens, never raw colors**: `cx.theme().background / foreground / muted / muted_foreground / border / primary / accent / danger / success / warning …`. The only literal colors live in `widgets.rs` (engine badge hues, connection accents).
- **Desktop density**: compact rows (24–28px), 12–13px UI text, monospace (`cx.theme().mono_font_family`) for SQL, identifiers, values, and numbers. Generous whitespace only on the home page.
- **Hierarchy**: one primary action per surface (Run, Connect). Everything else `ghost`/`outline`, `small`/`xsmall`.
- **Identity**: every place a connection appears shows its accent dot + engine badge + label; read-only connections show a lock.
- **State is visible**: running (spinner + elapsed + Cancel), empty states with a next step, errors with the engine's message and the statement, NULL rendered distinctly from empty strings.
- **Keyboard first**: every command has an action; shortcuts appear in tooltips and the palette.
- **Copy**: name the object and the verb ("Delete session “prod”?", "Connect", "Run statement"). No "OK", no "Are you sure?".
