---
title: "Desktop GUI"
---

# Desktop GUI

DBCrust ships with a Tauri-based desktop application. The frontend is built with React, TypeScript, Tailwind CSS, and CodeMirror; the backend is the same Rust `dbcrust` core library used by the CLI.

## Prerequisites

You need **[mise](https://mise.jdx.dev/)** installed. Mise manages Bun (the JavaScript runtime used for the frontend) and other project tools automatically.

```bash
# Install mise (if you don't have it)
curl https://mise.run | sh

# From the project root — install all tools (Bun, etc.)
mise install

# Install GUI frontend dependencies
mise run gui:install
```

## Running in development

```bash
mise run gui:dev
```

This starts:

1. The Vite dev server on `http://localhost:5173` (with hot-reload)
2. The Tauri Rust backend, which opens a native window pointing at the dev server

Changes to `.tsx`/`.css` files are reflected instantly. Changes to Rust code in `gui/src-tauri/` trigger a recompile.

## Building for production

```bash
mise run gui:build
```

Produces platform-specific installers:

- **macOS** — `.app` bundle and `.dmg`
- **Linux** — `.deb` and `.AppImage`
- **Windows** — `.msi` and `.exe`

Output is in `gui/src-tauri/target/release/bundle/`.

## Application overview

### Home screen

When you launch the app, you see the **Home** view with:

- A connection form — pick a database type, enter host/port/user/password/database, and connect
- **Saved sessions** — one-click reconnection to previously saved connections
- **Recent connections** — your connection history
- **Docker discovery** — auto-detect running database containers and connect directly

### SQL editor

After connecting, the **Query** view gives you:

- **CodeMirror editor** with SQL syntax highlighting, bracket matching, auto-indent, and metadata-aware table/column completion with backend-correct identifier quoting
- **Run** (`Cmd+Enter` / `Ctrl+Enter`) and **Explain** (`Cmd+Shift+Enter` / `Ctrl+Shift+Enter`) buttons
- **Multiple tabs** — open as many query tabs as you need
- **Results table and JSON views** — sortable columns, row count, execution time, and copy-as-JSON
- **Error display** — inline error messages from the database

### EXPLAIN viewer

Click **Explain** (or use the keyboard shortcut) to see the query execution plan rendered as a structured table directly in the results panel.

### Schema explorer

The **Schema** view lets you browse:

- All database objects, including PostgreSQL tables across non-public schemas, MongoDB collections, and Elasticsearch indices
- **Columns or fields** — name, inferred/native type, nullability, and default value where supported
- **Indexes** — name, type, primary/unique
- **Foreign keys** — constraint name and definition

Click an object to see its full details. The explorer adapts its terminology for MongoDB and Elasticsearch, unions MongoDB fields across a bounded document sample, shows Elasticsearch field capabilities, and safely quotes reserved or punctuated identifiers for each backend in generated queries and autocomplete selections.

### Docker discovery

The **Docker** panel lists all running containers that look like databases (PostgreSQL, MySQL, MongoDB, etc.). Click to connect — DBCrust resolves the host port automatically.

### Settings

The **Settings** view shows the current configuration and lets you toggle options like:

- Default row limit
- Expanded display mode
- Autocompletion
- Banner and server info
- Pager
- Query timeout
- EXPLAIN mode

### System tray

On macOS the app installs a system tray icon. The tray menu shows:

- Connection status (database type, host, database name)
- Disconnect
- Show/hide the main window
- Quit

## Keyboard shortcuts

| Shortcut | Action |
|----------|--------|
| `Cmd/Ctrl + Enter` | Execute query |
| `Cmd/Ctrl + Shift + Enter` | Explain query |

## Project structure

```
gui/
├── src/                     # React + TypeScript frontend
│   ├── App.tsx              # main application shell
│   ├── commands.ts          # Tauri command wrappers
│   ├── types.ts             # TypeScript type definitions
│   ├── queryPresets.ts      # built-in query presets
│   ├── index.css            # Tailwind CSS styles
│   └── components/
│       ├── ConnectionDialog.tsx
│       ├── DockerDiscovery.tsx
│       ├── Editor.tsx           # CodeMirror SQL editor
│       ├── ExplainView.tsx
│       ├── Layout.tsx
│       ├── Navigation.tsx
│       ├── ResultsPanel.tsx
│       ├── SavedConnections.tsx
│       ├── SchemaExplorer.tsx
│       ├── SettingsPage.tsx
│       ├── Sidebar.tsx
│       └── StatusBar.tsx
├── src-tauri/               # Tauri Rust backend
│   ├── src/lib.rs           # all Tauri commands (bridges to dbcrust core)
│   ├── tauri.conf.json      # Tauri configuration
│   └── Cargo.toml
├── package.json             # Bun-managed dependencies
├── vite.config.ts
├── tailwind.config.js
└── tsconfig.json
```

## Useful mise tasks

| Task | Description |
|------|-------------|
| `mise run gui:dev` | Start frontend + Tauri in dev mode |
| `mise run gui:build` | Production build with platform installers |
| `mise run gui:frontend` | Start only the Vite dev server (no Tauri) |
| `mise run gui:build-frontend` | Build only the frontend |
| `mise run gui:build-rust` | Build only the Tauri Rust backend |
| `mise run gui:lint` | Clippy on the GUI Rust backend |
| `mise run gui:install` | Install frontend npm dependencies via Bun |
| `mise run gui:clean` | Remove `dist/`, `node_modules/`, and Tauri `target/` |

## Tech stack

| Layer | Technology |
|-------|-----------|
| Window framework | [Tauri 2](https://v2.tauri.app/) |
| Frontend | React 19, TypeScript 5, Vite 6 |
| CSS | Tailwind CSS 3 |
| SQL editor | [CodeMirror](https://codemirror.net/) via `@uiw/react-codemirror` |
| Icons | [Lucide React](https://lucide.dev/) |
| JS runtime | [Bun](https://bun.sh/) (managed by mise) |
| Backend | Rust — the same `dbcrust` crate used by the CLI |
