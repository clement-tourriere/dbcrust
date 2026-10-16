---
title: "Desktop GUI"
description: "DBCrust's native, GPU-accelerated desktop app: connect, explore schemas, write SQL with schema-aware completion, browse results, and read query plans."
---

DBCrust ships a native desktop app built with [GPUI Kit](https://gpui-kit.com) — the GPU-accelerated UI framework behind Zed's GPUI. It is pure Rust: no web view, no JavaScript runtime, and the same `dbcrust` core as the CLI, so every engine, connection URL, SSH tunnel pattern, Vault setup and saved session you use in the terminal works in the app too.

![The DBCrust desktop app: schema explorer, SQL editor and results grid](/dbcrust/screenshots/gui-query.png)

## Build and run

The app is built from source for now. mise installs the Rust toolchain (1.85+) for you — on Linux you also need the usual GPUI system libraries (`libxkbcommon-x11`, `libwayland`, `libfontconfig`, Vulkan). Without mise installed, run the same tasks with the repository's `./bin/mise` wrapper, e.g. `./bin/mise run gui:dev`.

```bash
mise run gui:dev        # run the app (dev build, optimised dependencies)
mise run gui:build      # optimised build → target/gui-release/dbcrust-gui
mise run gui:install    # install dbcrust-gui into ~/.cargo/bin
mise run gui:bundle     # macOS: target/gui-release/bundle/DBCrust.app
```

Builds run through [Mr Boxington](https://mr-boxington.jdx.dev/), so rebuilds — and builds in other worktrees and branches — restore cached crates instead of compiling them again. Without mise: `cargo run -p dbcrust-gui` from the repository root (uncached).

## Connections

![The Connections page: searchable list of saved, recent and Docker connections, with details and actions](/dbcrust/screenshots/gui-home.png)

The **Connections** tab (`⇧⌘H`) is where every connection starts. On the left, one searchable list holds your **saved** connections, **recent** ones and the database **Docker containers** on this computer. Filter it with *All / Saved / Recent / Docker* and move through it with the arrow keys. On the right are the details of the selected entry and its actions.

| Key | Action |
|---|---|
| `↵` / double-click | Connect |
| `⇧↵` | Connect read-only |
| `⌘E` | Edit the saved connection |
| `⌘⌫` | Remove it (asks first) |
| `⌘↵` (in the form) | Connect |
| `Esc` | Close the form, or cancel a connection attempt |

### New connection

![New connection form](/dbcrust/screenshots/gui-home-new.png)

**+ New** opens the form:

- **Engine**: a searchable list grouped into *Servers* (PostgreSQL, MySQL, ClickHouse, MongoDB, Elasticsearch, White Dragon, Redis, SQL Server, Redshift, libSQL, InfluxDB) and *Files* (SQLite, DuckDB, Parquet, CSV, JSON). The fields start with real defaults: `localhost`, the engine's port and default user.
- **Paste connection URL…** fills the form from any URL DBCrust understands (`postgres://…`, `docker://…`, file paths), and keeps options the form has no field for.
- **Name** is what the connection is called in the list when you save it.
- **Encryption** summarises the TLS/SSL mode; open it for the details.
- **Environment** (Local, Development, Staging, Production) labels the connection everywhere it appears, so production never looks like local.
- **Read-only** opens the connection read-only where the engine supports it (PostgreSQL `default_transaction_read_only`, SQLite `query_only`) and turns on DBCrust's write-statement guard. Read-only connections show a lock wherever they appear. If **Open connections read-only** is on in Settings, every connection is read-only.

The footer shows the connection target (password masked) and three actions:

- **Test** runs the full connection setup, authentication included, then closes it. It doesn't change your recent list, and never stores anything.
- **Save** keeps it in `sessions.toml`. Passwords are never stored there: DBCrust looks them up in `~/.dbcrust`, `~/.pgpass` or `~/.my.cnf`, or runs your `password_command`.
- **Connect** opens it and adds it to your recent connections; it does not save it.

While connecting, the page shows each step (for example *Checking direct access → Opening forwarding → Authenticating*) with the elapsed time and a **Cancel** button. A failure stays next to the connection it belongs to, with **Retry**, **Edit connection** and **Connection details**.

### Docker containers

![A Docker container that can't be reached, with the reason and how to fix it](/dbcrust/screenshots/gui-home-docker-failed.png)

Database containers of the Docker daemon you use (`docker context`: OrbStack, Docker Desktop, Colima…) are listed with their image, whether they run, and whether their database port is published. Credentials come from the container's environment.

DBCrust picks the route itself, fastest first: a published port, an OrbStack domain, the container's address, and when none of them answers (a VPN that swallows container networks, or Docker Desktop on macOS), a relay through Docker itself. The details show which route a live connection uses. If nothing works, you learn why within seconds, and **Connection details** shows each route that was tried and the one-line `ports: ["127.0.0.1::5432"]` change that fixes it. See [Docker integration](/dbcrust/advanced/docker-integration/#how-dbcrust-reaches-a-container).

### HashiCorp Vault

**Connect with Vault…** (bottom of the list) opens a wizard that detects your Vault address and token, lists the database configs and roles you can use, and connects with dynamic credentials.

You can keep **several connections open at once**. Each runs on its own worker thread, so a long query on one never blocks another. Every connection gets an accent colour shown next to its name in the explorer, tabs, editor toolbar and status bar.

## Keep it running in the menu bar

Closing the window doesn't quit DBCrust: it keeps running in the macOS menu bar or the Windows notification area, with its connections, tabs and running queries intact. The first time, DBCrust says so and offers to quit instead.

The menu bar icon shows how many connections are open and whether a query is running. It offers:

- **Open DBCrust** and **New query**.
- **Connect**, with your saved and recent connections.
- **Connections**: open, query or disconnect each one, or disconnect all.
- **Settings…** and **Quit DBCrust**.

Clicking the Dock icon also brings the window back. `⌘Q` quits for real: DBCrust asks first if a query is still running, saves your open queries, then closes every connection.

Turn this off in **Settings → General → Keep running in the menu bar** to make closing the window quit. On macOS you can also hide the Dock icon while the window is closed. On Linux there is no tray yet, so closing the window quits.

## Explore schemas

The **Explorer** sidebar lists every open connection with its tables, views and collections, grouped by schema when there is more than one. Type in the filter box to search across all connections. Expand a table to see its columns with type, primary-key, foreign-key and NOT NULL markers.

![The Explorer with a table expanded: columns with PK, FK and NN markers, and the row actions on hover](/dbcrust/screenshots/gui-explorer.png)

- **Double-click** a table to open the **table inspector**; hover a table for **Query data** and more actions.
- **Right-click** for *Select top 100*, *Count rows*, *Generate SELECT/INSERT*, *Copy name*, *Copy DDL* and more.
- On a connection: *New query*, *Refresh schema*, *Open database…* (opens another database of the same server as a new connection), *Save as session…*, the read-only guard, and *Disconnect*.

![Table inspector showing columns, keys and foreign keys](/dbcrust/screenshots/gui-inspector.png)

The inspector shows **Columns**, **Indexes**, **Constraints**, **Referenced by**, a **Data** preview and a reconstructed **DDL**. Foreign keys are links: click one to open the referenced table. **Count rows** runs only when you ask, and can be cancelled.

## Write and run SQL

Each query tab is bound to a connection (pick it in the toolbar). The editor has SQL syntax highlighting, line numbers, and **schema-aware completion**: tables after `FROM`/`JOIN`, columns of the tables in the statement (aliases and CTEs resolved — type `o.` after `FROM orders o`), keywords and functions of the engine's dialect. Identifiers are quoted correctly for each engine.

| Action | Shortcut (macOS / Linux & Windows) |
|---|---|
| Run the statement under the caret (or the selection) | `⌘↵` / `Ctrl+Enter` |
| Run all statements (stops at the first error) | `⇧⌘↵` / `Ctrl+Shift+Enter` |
| Cancel the running query | `⌘.` / `Ctrl+.` |
| Explain / Explain analyze | `⌘E` / `⇧⌘E` |
| Format SQL | `⌥⇧F` / `Alt+Shift+F` |
| Save (to the tab's file or saved query; asks for a new tab) | `⌘S` / `Ctrl+S` |
| Save as… / open a `.sql` file | `⇧⌘S` / `⌘O` |

- **Several queries in one tab.** Put the caret anywhere in a query and press `⌘↵`: only that query runs, and it is lightly highlighted so you can see what will run. Queries are separated by `;`, or simply by a blank line between `SELECT` queries:

  ```sql
  select * from orders where status = 'open'

  select count(*) from customers
  ```

  A blank line never splits anything else: `UPDATE`, `DELETE`, DDL and anything DBCrust can't parse as a plain read query need a `;` between them, so a blank line inside a statement can never cut it in two. If a run fails because two queries were sent together, the error says so. Prefer `;` only? **Settings → Queries → Statement separators**.
- **The toolbar says what runs**: *Run statement* (the one under the caret, "2 of 3"), *Run selection*, or *Run all* (*Run all in selection* when text is selected). Selecting several queries and pressing `⌘↵` runs them one after another. The connection picker next to it decides where, and shows when the connection is read-only.
- **Cancel** really cancels: PostgreSQL statements are cancelled server-side; other engines stop waiting for the result.
- **Run all** shows one result tab per statement.
- **History** keeps every run of the session (time, duration, row count, outcome); click an entry to load it back.
- **Saved queries** are the CLI's named queries (`named_queries.toml`), scoped to the engine or global. A tab opened from a saved query or a `.sql` file saves back to it with `⌘S`, and a dot on the tab marks unsaved changes.
- **Drafts are saved automatically** and restored on the next launch, still bound to their file or saved query. A restored tab reconnects to its connection as soon as you open it again.
- **Tabs name their connection** (`● shop PG`), so two tabs on different databases never look alike; a tab without one says *No connection*. Hover a tab for its connection and save target.
- **Rename a tab**: double-click its title, press `F2`, or right-click → *Rename tab*. `↵` keeps the name, `Esc` cancels. Renaming only changes the tab's label; `⌘S` still saves to the same file or saved query, and keeps your name.
- **Jump to a tab**: `⌘1`…`⌘9` (`⌘9` is the last one), or `⌘P` to search tabs by title or connection.
- **Closed a tab by mistake?** `⇧⌘T` reopens it, with its SQL and connection.
- **A failed run keeps what you had:** the error shows above the previous results, or above the statements that completed in a "Run all".
- **Results say when they are old:** after you edit the statement or switch connection, a banner marks them *Results from previous run*.

## Read results

- A virtualized grid that stays smooth with tens of thousands of rows. Numbers are right-aligned, `NULL` is shown distinctly from an empty string, and clicking a header sorts the loaded rows (numeric-aware, NULLs last).
- When DBCrust adds its default `LIMIT` (see `default_limit` in settings), the footer says so — add an explicit `LIMIT` to fetch more.
- One duration per statement; a "Run all" also shows the total.
- **Copy** a cell, a row, a column or everything as CSV, TSV, JSON or Markdown; **Export** to a file in the same formats. Both menus say exactly which rows they include.
- **Double-click a row** to inspect it field by field, with JSON values pretty-printed.

## Understand query plans

![EXPLAIN visualizer with plan tree, warnings and node details](/dbcrust/screenshots/gui-explain.png)

**Explain** renders the plan as a tree with the cost share of each node and estimated vs. actual rows (with **Explain analyze** on PostgreSQL). The side panel shows the **selected node** and the **plan observations**: full scans, temporary B-trees, badly misestimated row counts, each listed once and with advice that says when it matters. Click an observation to jump to its node. **Copy plan** copies PostgreSQL's JSON plan, ready for tools like explain.dalibo.com.

- PostgreSQL: full tree with costs, timings and buffers.
- SQLite: `EXPLAIN QUERY PLAN` tree.
- MySQL: JSON or classic plans.
- DataFusion files: logical and physical plans.
- Other engines show their native explain output.

**Explain analyze** executes the statement, so it is only offered for statements DBCrust classifies as read-only.

## Command palette and keyboard

![Command palette](/dbcrust/screenshots/gui-palette.png)

Press `⌘K` (`Ctrl+K`) to search every command, open connection, saved session, recent connection, table and named query. Every command also has a native menu entry and, where it makes sense, a shortcut:

| Shortcut | Command |
|---|---|
| `⌘K` / `⇧⌘P` | Command palette |
| `⌘T` / `⌘W` | New query tab / close tab |
| `⇧⌘T` | Reopen the last closed tab |
| `⌃Tab` / `⌃⇧Tab` | Next / previous tab |
| `⌘1`…`⌘9` | Go to tab 1–8 / the last tab |
| `⌘P` | Go to tab… (search by title or connection) |
| `F2` | Rename the current tab |
| `⌘N` | New connection |
| `⇧⌘H` | Connections page |
| `⌘B` | Toggle sidebar |
| `⌘0` | Focus explorer |
| `⌘R` | Refresh schema |
| `⌘,` | Settings |
| `⌥⌘T` | Toggle light / dark |

## Settings and themes

![Settings](/dbcrust/screenshots/gui-settings.png)

**Settings** (`⌘,`) covers every setting of `config.toml`, plus the data saved next to it:

- **General:** keep running in the menu bar when the window is closed, hide the Dock icon meanwhile (macOS), and reopen unsaved queries at launch.
- **Appearance:** System, Light or Dark (follows the OS by default), interface size, and bundled color themes — Catppuccin, Tokyo Night, Gruvbox and Solarized.
- **Queries:** default row limit, query timeout (applies immediately) and schema timeout, shared with the CLI; the editor font size; statement separators (`;` and blank lines between `SELECT` queries, or `;` only).
- **Saved queries:** every saved query, in every scope (see below).
- **Connections:** open new connections read-only, how many recent connections to keep, **Clear recent connections…**, and a link to the Connections page, where saved connections are managed.
- **SSH tunnels:** host patterns and their SSH servers (see below).
- **Vault:** credential caching and the credential process, and what the encrypted cache holds — mount, database, role and expiry, never a secret — with **Clear cached credentials…**.
- **Results display:** how vectors, JSON, arrays and other complex values are formatted.
- **AI assistant**, **Terminal (CLI)** and **Logging:** settings of the `dbcrust` command line (AI assistant, prompt, pager, EXPLAIN mode, history, log files). Settings read when a session starts say so.
- **Files:** where every storage file lives, **Reload from disk** after editing them by hand or with the CLI, and **Open config.toml** in your editor.

Changes apply as you make them and are written to `config.toml` a moment later, with its documentation comments preserved. Desktop-only state (theme, sidebar, drafts) lives in `gui_state.toml` next to it.

### Saved queries

The **Saved queries** page lists global, per-engine and per-session queries together. Search names, SQL and descriptions, filter by scope, and edit a query's name, scope, description and SQL (with highlighting, and the parameters it takes: `$1`, `$2`, `$*`, `$@`). **Save changes** (`⌘S`) refuses to overwrite another query of the same name and scope; renaming keeps the creation date. **Open in new tab** opens the SQL in a query tab that saves back to the query, **Duplicate** starts a copy, and **Delete…** asks first. Reach the page from **Manage saved queries…** in the command palette or at the bottom of a query tab's saved queries menu.

Saved queries are stored in `named_queries.toml`. Unscoped queries from an older `config.toml` move there as global queries the next time DBCrust starts.

### SSH tunnels

A pattern is a regular expression matched against a database host; a connection to a matching host goes through the pattern's SSH server (`[user[:password]@]host[:port]`). A target in backticks runs a command when connecting — the app never runs it while you edit or test. Passwords are masked in the list but stored in plain text in `config.toml`: prefer SSH keys or an agent. **Test a host** shows which pattern a hostname matches without connecting; if several match, only one is used and which one is not predictable, so keep patterns from overlapping.

![The app in light mode](/dbcrust/screenshots/gui-query-light.png)

## Files

| File | Contents |
|---|---|
| `~/.config/dbcrust/config.toml` | DBCrust settings, shared with the CLI |
| `sessions.toml`, `recent.toml`, `named_queries.toml` | Saved sessions, recent connections and named queries, shared with the CLI |
| `gui_state.toml` | Desktop app only: theme, sizes, menu bar behaviour, open drafts, connection labels |

Set `DBCRUST_CONFIG_DIR` to use another directory, exactly as with the CLI.

## Under the hood

```
gui/
├── core/   dbcrust-gui-core — no GPUI: per-connection workers, storage, query/result/plan models
└── src/    dbcrust-gui      — the GPUI Kit app
```

- **Workers:** each connection runs on its own thread with its own async runtime. Queries are serialized per connection, and cancellation is per request.
- **Backend tests:** `dbcrust-gui-core` is tested without any GUI: `mise run gui:test`.
- **Screenshots:** the images on this page are rendered headlessly by `mise run gui:screenshots` from a seeded demo database.

See `gui/ARCHITECTURE.md` in the repository for the module map.
