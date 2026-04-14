//! DBCrust GUI — Tauri backend
//! Direct Rust bridge to dbcrust core library.
//! Database operations run on dedicated threads with LocalSet to handle !Send futures.

use dbcrust::config::Config;
use dbcrust::database::{ConnectionInfo, DatabaseType, DatabaseTypeExt};
use dbcrust::db::Database;
use dbcrust::docker::DockerClient;
use dbcrust::password_sanitizer::sanitize_connection_url;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tauri::menu::{
    AboutMetadataBuilder, MenuBuilder, MenuItem, MenuItemBuilder, PredefinedMenuItem,
    SubmenuBuilder,
};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{image::Image, Manager, State, WindowEvent, Wry};

// ══════════════════════════════════════════════════════════════════════════════
// Application State
// ══════════════════════════════════════════════════════════════════════════════

pub struct AppState {
    db: std::sync::Mutex<Option<Database>>,
    config: std::sync::Mutex<Config>,
    connection_url: std::sync::Mutex<Option<String>>,
    /// Serializes database operations to prevent concurrent take_db/put_db races.
    op_lock: std::sync::Mutex<()>,
    tray_toggle_item: std::sync::Mutex<Option<MenuItem<Wry>>>,
    quitting: AtomicBool,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            db: std::sync::Mutex::new(None),
            config: std::sync::Mutex::new(Config::load()),
            connection_url: std::sync::Mutex::new(None),
            op_lock: std::sync::Mutex::new(()),
            tray_toggle_item: std::sync::Mutex::new(None),
            quitting: AtomicBool::new(false),
        }
    }
}

const MENU_QUIT_APP: &str = "quit_app";
const MENU_TRAY_TOGGLE: &str = "tray_toggle";
const MENU_TRAY_QUIT: &str = "tray_quit";

fn main_window<M: Manager<Wry>>(manager: &M) -> Option<tauri::WebviewWindow<Wry>> {
    manager
        .get_webview_window("main")
        .or_else(|| manager.webview_windows().into_values().next())
}

fn sync_tray_toggle_item<M: Manager<Wry>>(manager: &M) {
    let is_visible = main_window(manager)
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false);
    let text = if is_visible {
        "Hide DBCrust"
    } else {
        "Show DBCrust"
    };

    if let Some(item) = manager
        .state::<AppState>()
        .tray_toggle_item
        .lock()
        .unwrap()
        .as_ref()
    {
        let _ = item.set_text(text);
    }
}

fn show_main_window<M: Manager<Wry>>(manager: &M) {
    if let Some(window) = main_window(manager) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    sync_tray_toggle_item(manager);
}

fn hide_main_window<M: Manager<Wry>>(manager: &M) {
    if let Some(window) = main_window(manager) {
        let _ = window.hide();
    }
    sync_tray_toggle_item(manager);
}

fn toggle_main_window<M: Manager<Wry>>(manager: &M) {
    let is_visible = main_window(manager)
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false);

    if is_visible {
        hide_main_window(manager);
    } else {
        show_main_window(manager);
    }
}

fn apply_window_icon(app: &tauri::App<Wry>) {
    if let Some(icon) = app.default_window_icon().cloned() {
        for window in app.webview_windows().into_values() {
            let _ = window.set_icon(icon.clone());
        }
    }
}

fn load_tray_icon() -> tauri::Result<Image<'static>> {
    Image::from_bytes(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/branding/dbcrust-icon-tray-32.png"
    )))
    .map(|image| image.to_owned())
}

/// Take the Database out of state (brief lock). Returns it for use on a dedicated thread.
fn take_db(state: &AppState) -> Result<Database, String> {
    state
        .db
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "Not connected to any database".to_string())
}

/// Put the Database back into state after use.
fn put_db(state: &AppState, db: Database) {
    *state.db.lock().unwrap() = Some(db);
}

/// Run an async database operation on a dedicated thread with LocalSet.
/// This handles the !Send futures from dbcrust's async methods.
fn run_db<T: Send + 'static>(
    mut db: Database,
    op: impl FnOnce(&mut Database) -> std::pin::Pin<Box<dyn std::future::Future<Output = T> + '_>>
        + Send
        + 'static,
) -> (Database, T) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        let result = local.block_on(&rt, op(&mut db));
        (db, result)
    })
    .join()
    .expect("Database thread panicked")
}

// ══════════════════════════════════════════════════════════════════════════════
// API Response Types
// ══════════════════════════════════════════════════════════════════════════════

#[derive(Serialize, Clone, Debug)]
pub struct ConnectionResponse {
    pub connected: bool,
    pub database_type: String,
    pub database_name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub url: String,
}

#[derive(Serialize, Debug)]
pub struct QueryResponse {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub row_count: usize,
    pub elapsed_ms: u128,
}

#[derive(Serialize, Debug)]
pub struct TableDetailResponse {
    pub name: String,
    pub schema: String,
    pub columns: Vec<ColumnDetailResponse>,
    pub indexes: Vec<IndexDetailResponse>,
    pub foreign_keys: Vec<ForeignKeyDetailResponse>,
}

#[derive(Serialize, Debug)]
pub struct ColumnDetailResponse {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub default_value: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct IndexDetailResponse {
    pub name: String,
    pub index_type: String,
    pub is_primary: bool,
    pub is_unique: bool,
}

#[derive(Serialize, Debug)]
pub struct ForeignKeyDetailResponse {
    pub name: String,
    pub definition: String,
}

#[derive(Serialize, Debug)]
pub struct RecentConnectionResponse {
    pub display_name: String,
    pub connection_url: String,
    pub database_type: String,
    pub timestamp: String,
    pub success: bool,
}

#[derive(Serialize, Debug)]
pub struct SessionResponse {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub dbname: String,
    pub database_type: String,
    pub target: String,
}

#[derive(Serialize, Debug)]
pub struct NamedQueryResponse {
    pub key: String,
    pub name: String,
    pub query: String,
    pub scope: String,
}

#[derive(Serialize, Debug)]
pub struct ConfigResponse {
    pub default_limit: usize,
    pub expanded_display: bool,
    pub autocomplete_enabled: bool,
    pub show_banner: bool,
    pub show_server_info: bool,
    pub pager_enabled: bool,
    pub query_timeout_seconds: u64,
    pub explain_mode: bool,
}

#[derive(Serialize, Debug)]
pub struct DockerContainerResponse {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    pub database_type: Option<String>,
    pub host_port: Option<u16>,
    pub container_port: Option<u16>,
    pub is_running: bool,
}

#[derive(Serialize, Debug)]
pub struct DatabaseTypeInfo {
    pub name: String,
    pub scheme: String,
    pub default_port: Option<u16>,
    pub placeholder: String,
}

// ── Helper: build QueryResponse from Vec<Vec<String>> ────────────────────────
fn to_query_response(results: Vec<Vec<String>>, elapsed: u128) -> QueryResponse {
    if results.is_empty() {
        return QueryResponse {
            columns: vec![],
            rows: vec![],
            row_count: 0,
            elapsed_ms: elapsed,
        };
    }
    let columns = results[0].clone();
    let rows: Vec<Vec<String>> = results.into_iter().skip(1).collect();
    let row_count = rows.len();
    QueryResponse {
        columns,
        rows,
        row_count,
        elapsed_ms: elapsed,
    }
}

fn database_type_placeholder(database_type: &DatabaseType) -> &'static str {
    match database_type {
        DatabaseType::PostgreSQL => "postgres://user:pass@localhost:5432/mydb",
        DatabaseType::MySQL => "mysql://user:pass@localhost:3306/mydb",
        DatabaseType::SQLite => "sqlite:///path/to/database.db",
        DatabaseType::ClickHouse => "clickhouse://user:pass@localhost:8123/default",
        DatabaseType::MongoDB => "mongodb://user:pass@localhost:27017/mydb",
        DatabaseType::Elasticsearch => "elasticsearch://localhost:9200",
        DatabaseType::Parquet => "parquet:///path/to/data.parquet",
        DatabaseType::CSV => "csv:///path/to/data.csv",
        DatabaseType::JSON => "json:///path/to/data.json",
        DatabaseType::DuckDB => "duckdb:///path/to/database.duckdb",
    }
}

fn complete_vault_url(connection_info: &ConnectionInfo, fallback_url: &str) -> String {
    if let (Some(vault_mount), Some(vault_database), Some(vault_role)) = (
        connection_info.options.get("vault_mount"),
        connection_info.options.get("vault_database"),
        connection_info.options.get("vault_role"),
    ) {
        if vault_role.is_empty() {
            format!("vault://{vault_mount}/{vault_database}")
        } else {
            format!("vault://{vault_role}@{vault_mount}/{vault_database}")
        }
    } else {
        fallback_url.to_string()
    }
}

fn connect_standard_database(
    url: &str,
    limit: usize,
    expanded: bool,
) -> Result<(Database, Option<ConnectionInfo>), String> {
    let url = url.to_string();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async move {
            Database::from_url(&url, Some(limit), Some(expanded))
                .await
                .map(|db| (db, None))
                .map_err(|e| e.to_string())
        })
    })
    .join()
    .map_err(|_| "Connection thread panicked".to_string())?
    .map_err(|e| format!("Connection failed: {e}"))
}

fn connect_docker_database(
    url: &str,
    limit: usize,
    expanded: bool,
) -> Result<(Database, Option<ConnectionInfo>), String> {
    let url = url.to_string();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async move {
            Database::from_docker_url_with_tracking(&url, Some(limit), Some(expanded))
                .await
                .map_err(|e| e.to_string())
        })
    })
    .join()
    .map_err(|_| "Connection thread panicked".to_string())?
    .map_err(|e| format!("Connection failed: {e}"))
}

fn connect_vault_database(
    url: &str,
    limit: usize,
    expanded: bool,
) -> Result<(Database, Option<ConnectionInfo>), String> {
    let url = url.to_string();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async move {
            let (role, mount_path, database_name) = dbcrust::vault_client::parse_vault_url(&url)
                .ok_or_else(|| format!("Invalid vault URL format: {url}"))?;

            let db_name = database_name.ok_or_else(|| {
                "Vault GUI connections require an explicit database name in the URL".to_string()
            })?;
            let role_name = role.ok_or_else(|| {
                "Vault GUI connections require an explicit role name in the URL".to_string()
            })?;

            let mut vault_config = Config::load();
            let (credentials, _) = dbcrust::vault_client::get_dynamic_credentials_with_caching(
                &mount_path,
                &db_name,
                &role_name,
                &mut vault_config,
            )
            .await
            .map_err(|e| format!("Failed to get Vault credentials: {e}"))?;

            let db_config = dbcrust::vault_client::get_vault_database_config(&mount_path, &db_name)
                .await
                .map_err(|e| format!("Failed to get database config from Vault: {e}"))?;

            let connection_url_template = db_config
                .connection_details
                .connection_url
                .as_ref()
                .ok_or_else(|| "No connection URL found in Vault database config".to_string())?;

            let postgres_url = dbcrust::vault_client::construct_postgres_url(
                connection_url_template,
                &credentials.username,
                &credentials.password,
            )
            .map_err(|e| format!("Failed to construct connection URL: {e}"))?;

            let mut database = Database::from_url(&postgres_url, Some(limit), Some(expanded))
                .await
                .map_err(|e| format!("Failed to connect with Vault credentials: {e}"))?;

            let original_connection_info = ConnectionInfo::parse_url(connection_url_template)
                .map_err(|e| format!("Failed to parse Vault connection URL template: {e}"))?;

            let mut options = HashMap::new();
            options.insert("vault_mount".to_string(), mount_path.clone());
            options.insert("vault_database".to_string(), db_name.clone());
            options.insert("vault_role".to_string(), role_name.clone());

            let connection_info = ConnectionInfo {
                database_type: DatabaseType::PostgreSQL,
                host: original_connection_info.host.clone(),
                port: original_connection_info.port,
                username: Some(credentials.username.clone()),
                password: Some(credentials.password),
                database: original_connection_info.database.clone(),
                file_path: None,
                options,
                docker_container: None,
            };

            database.set_connection_info_override(connection_info.clone());
            Ok::<(Database, Option<ConnectionInfo>), String>((database, Some(connection_info)))
        })
    })
    .join()
    .map_err(|_| "Connection thread panicked".to_string())?
    .map_err(|e| format!("Connection failed: {e}"))
}

fn connect_with_url(state: &AppState, url: String) -> Result<ConnectionResponse, String> {
    let (limit, expanded) = {
        let config = state.config.lock().unwrap();
        (config.default_limit, config.expanded_display_default)
    };

    let (db, connection_info) = if url.starts_with("vault://") {
        connect_vault_database(&url, limit, expanded)?
    } else if url.starts_with("docker://") {
        connect_docker_database(&url, limit, expanded)?
    } else {
        connect_standard_database(&url, limit, expanded)?
    };

    let current_url = if url.starts_with("vault://") {
        connection_info
            .as_ref()
            .map(|info| complete_vault_url(info, &url))
            .unwrap_or_else(|| url.clone())
    } else {
        url.clone()
    };
    let sanitized_current_url = sanitize_connection_url(&current_url);

    let response = ConnectionResponse {
        connected: true,
        database_type: db.get_database_type().display_name().to_string(),
        database_name: db.get_current_db(),
        host: db.get_host(),
        port: db.get_port(),
        username: db.get_username(),
        url: sanitized_current_url.clone(),
    };

    {
        let mut config = state.config.lock().unwrap();
        if url.starts_with("vault://") {
            if let Some(ref resolved_info) = connection_info {
                let history_url = complete_vault_url(resolved_info, &url);
                let _ = config.add_recent_connection_with_options(
                    history_url,
                    DatabaseType::PostgreSQL,
                    true,
                    resolved_info.options.clone(),
                );
            }
        } else if let Some(ref resolved_info) = connection_info {
            let history_url = sanitize_connection_url(&resolved_info.to_url());
            let _ = config.add_recent_connection_auto_display(
                history_url,
                resolved_info.database_type.clone(),
                true,
            );
        } else {
            let _ = config.add_recent_connection_auto_display(
                sanitize_connection_url(&url),
                db.get_database_type(),
                true,
            );
        }
    }

    *state.db.lock().unwrap() = Some(db);
    *state.connection_url.lock().unwrap() = Some(sanitized_current_url);

    Ok(response)
}

// ══════════════════════════════════════════════════════════════════════════════
// Connection Commands
// ══════════════════════════════════════════════════════════════════════════════

#[tauri::command]
fn connect(state: State<'_, AppState>, url: String) -> Result<ConnectionResponse, String> {
    let _op = state.op_lock.lock().unwrap();
    connect_with_url(state.inner(), url)
}

#[tauri::command]
fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    let _op = state.op_lock.lock().unwrap();
    *state.db.lock().unwrap() = None;
    *state.connection_url.lock().unwrap() = None;
    Ok(())
}

#[tauri::command]
fn get_connection_state(state: State<'_, AppState>) -> Result<Option<ConnectionResponse>, String> {
    let db_guard = state.db.lock().unwrap();
    match db_guard.as_ref() {
        Some(db) => {
            let url = state
                .connection_url
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_default();
            Ok(Some(ConnectionResponse {
                connected: true,
                database_type: db.get_database_type().display_name().to_string(),
                database_name: db.get_current_db(),
                host: db.get_host(),
                port: db.get_port(),
                username: db.get_username(),
                url,
            }))
        }
        None => Ok(None),
    }
}

#[tauri::command]
fn get_database_types() -> Vec<DatabaseTypeInfo> {
    let mut database_types: Vec<_> = DatabaseType::supported_types()
        .into_iter()
        .map(|database_type| DatabaseTypeInfo {
            name: database_type.display_name().to_string(),
            scheme: database_type.url_scheme().to_string(),
            default_port: database_type.default_port(),
            placeholder: database_type_placeholder(&database_type).to_string(),
        })
        .collect();

    database_types.push(DatabaseTypeInfo {
        name: "Docker".into(),
        scheme: "docker".into(),
        default_port: None,
        placeholder: "docker://container_name/mydb".into(),
    });

    database_types
}

// ══════════════════════════════════════════════════════════════════════════════
// Query Commands
// ══════════════════════════════════════════════════════════════════════════════

#[tauri::command]
fn execute_query(state: State<'_, AppState>, sql: String) -> Result<QueryResponse, String> {
    let _op = state.op_lock.lock().unwrap();
    let db = take_db(&state)?;
    let start = Instant::now();

    let (db, result) = run_db(db, move |db| {
        Box::pin(async move {
            db.execute_query(&sql)
                .await
                .map_err(|e| format!("Query error: {e}"))
        })
    });

    put_db(&state, db);
    let results = result?;
    Ok(to_query_response(results, start.elapsed().as_millis()))
}

#[tauri::command]
fn explain_query(state: State<'_, AppState>, sql: String) -> Result<QueryResponse, String> {
    let _op = state.op_lock.lock().unwrap();
    let db = take_db(&state)?;
    let start = Instant::now();

    let (db, result) = run_db(db, move |db| {
        Box::pin(async move {
            db.execute_explain_query_raw(&sql)
                .await
                .map_err(|e| format!("Explain error: {e}"))
        })
    });

    put_db(&state, db);
    let results = result?;
    Ok(to_query_response(results, start.elapsed().as_millis()))
}

// ══════════════════════════════════════════════════════════════════════════════
// Schema Commands
// ══════════════════════════════════════════════════════════════════════════════

#[tauri::command]
fn list_databases(state: State<'_, AppState>) -> Result<QueryResponse, String> {
    let _op = state.op_lock.lock().unwrap();
    let db = take_db(&state)?;
    let start = Instant::now();

    let (db, result) = run_db(db, |db| {
        Box::pin(async { db.list_databases().await.map_err(|e| format!("{e}")) })
    });

    put_db(&state, db);
    Ok(to_query_response(result?, start.elapsed().as_millis()))
}

#[tauri::command]
fn list_tables(state: State<'_, AppState>) -> Result<QueryResponse, String> {
    let _op = state.op_lock.lock().unwrap();
    let db = take_db(&state)?;
    let start = Instant::now();

    let (db, result) = run_db(db, |db| {
        Box::pin(async { db.list_tables().await.map_err(|e| format!("{e}")) })
    });

    put_db(&state, db);
    Ok(to_query_response(result?, start.elapsed().as_millis()))
}

#[tauri::command]
fn describe_table(
    state: State<'_, AppState>,
    table_name: String,
) -> Result<TableDetailResponse, String> {
    let _op = state.op_lock.lock().unwrap();
    let db = take_db(&state)?;

    let (db, result) = run_db(db, move |db| {
        Box::pin(async move {
            db.get_table_details(&table_name)
                .await
                .map_err(|e| format!("{e}"))
        })
    });

    put_db(&state, db);
    let details = result?;

    Ok(TableDetailResponse {
        name: details.name,
        schema: details.schema,
        columns: details
            .columns
            .iter()
            .map(|c| ColumnDetailResponse {
                name: c.name.clone(),
                data_type: c.data_type.clone(),
                nullable: c.nullable,
                default_value: c.default_value.clone(),
            })
            .collect(),
        indexes: details
            .indexes
            .iter()
            .map(|i| IndexDetailResponse {
                name: i.name.clone(),
                index_type: i.index_type.clone(),
                is_primary: i.is_primary,
                is_unique: i.is_unique,
            })
            .collect(),
        foreign_keys: details
            .foreign_keys
            .iter()
            .map(|fk| ForeignKeyDetailResponse {
                name: fk.name.clone(),
                definition: fk.definition.clone(),
            })
            .collect(),
    })
}

#[tauri::command]
fn list_users(state: State<'_, AppState>) -> Result<QueryResponse, String> {
    let _op = state.op_lock.lock().unwrap();
    let db = take_db(&state)?;
    let start = Instant::now();

    let (db, result) = run_db(db, |db| {
        Box::pin(async { db.list_users().await.map_err(|e| format!("{e}")) })
    });

    put_db(&state, db);
    Ok(to_query_response(result?, start.elapsed().as_millis()))
}

#[tauri::command]
fn list_indexes(state: State<'_, AppState>) -> Result<QueryResponse, String> {
    let _op = state.op_lock.lock().unwrap();
    let db = take_db(&state)?;
    let start = Instant::now();

    let (db, result) = run_db(db, |db| {
        Box::pin(async { db.list_indexes().await.map_err(|e| format!("{e}")) })
    });

    put_db(&state, db);
    Ok(to_query_response(result?, start.elapsed().as_millis()))
}

// ══════════════════════════════════════════════════════════════════════════════
// Session & History Commands
// ══════════════════════════════════════════════════════════════════════════════

#[tauri::command]
fn list_recent_connections(
    state: State<'_, AppState>,
) -> Result<Vec<RecentConnectionResponse>, String> {
    let config = state.config.lock().unwrap();
    let recent = config.get_recent_connections();
    Ok(recent
        .iter()
        .map(|c| RecentConnectionResponse {
            display_name: c.display_name.clone(),
            connection_url: c.connection_url.clone(),
            database_type: c.database_type.display_name().to_string(),
            timestamp: c.timestamp.format("%Y-%m-%d %H:%M").to_string(),
            success: c.success,
        })
        .collect())
}

#[tauri::command]
fn list_sessions(state: State<'_, AppState>) -> Result<Vec<SessionResponse>, String> {
    let config = state.config.lock().unwrap();
    let sessions = config.list_sessions();
    Ok(sessions
        .iter()
        .map(|(name, s)| SessionResponse {
            name: name.clone(),
            host: s.host.clone(),
            port: s.port,
            user: s.user.clone(),
            dbname: s.dbname.clone(),
            database_type: s.database_type.display_name().to_string(),
            target: s.reconnect_target(),
        })
        .collect())
}

#[tauri::command]
fn connect_saved_session(
    state: State<'_, AppState>,
    name: String,
) -> Result<ConnectionResponse, String> {
    let _op = state.op_lock.lock().unwrap();
    let session = {
        let config = state.config.lock().unwrap();
        config
            .get_session(&name)
            .cloned()
            .ok_or_else(|| format!("Session '{name}' not found"))?
    };

    let url = session.reconstruct_connection_url()?;
    connect_with_url(state.inner(), url)
}

#[tauri::command]
fn connect_recent_connection(
    state: State<'_, AppState>,
    index: usize,
) -> Result<ConnectionResponse, String> {
    let _op = state.op_lock.lock().unwrap();
    let recent_connection = {
        let config = state.config.lock().unwrap();
        config
            .get_recent_connections()
            .get(index)
            .cloned()
            .ok_or_else(|| format!("Recent connection at index {index} not found"))?
    };

    let url = recent_connection.reconstruct_connection_url()?;
    connect_with_url(state.inner(), url)
}

#[tauri::command]
fn save_session(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let db_guard = state.db.lock().unwrap();
    let db = db_guard.as_ref().ok_or("Not connected")?;
    let conn_info = db
        .get_connection_info()
        .ok_or("Connection info not available")?;

    let mut config = state.config.lock().unwrap();
    config
        .save_session_from_connection_info(&name, conn_info)
        .map_err(|e| format!("Failed to save session: {e}"))
}

#[tauri::command]
fn delete_session(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config
        .delete_session(&name)
        .map_err(|e| format!("Failed to delete session: {e}"))?;
    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// Named Query Commands
// ══════════════════════════════════════════════════════════════════════════════

#[tauri::command]
fn list_named_queries(state: State<'_, AppState>) -> Result<Vec<NamedQueryResponse>, String> {
    let config = state.config.lock().unwrap();
    let db_guard = state.db.lock().unwrap();
    let db_type = db_guard.as_ref().map(|db| db.get_database_type());

    let mut result = Vec::new();

    // Scoped named queries
    for (name, query, scope) in config.list_available_named_queries(db_type.as_ref(), None) {
        let scope_str = match &scope {
            dbcrust::config::NamedQueryScope::Global => "global".to_string(),
            dbcrust::config::NamedQueryScope::DatabaseType(dt) => dt.display_name().to_string(),
            dbcrust::config::NamedQueryScope::Session(s) => format!("session:{s}"),
        };
        result.push(NamedQueryResponse {
            key: dbcrust::config::NamedQueriesStorage::generate_key(&name, &scope),
            name,
            query,
            scope: scope_str,
        });
    }

    // Legacy named queries
    for (name, query) in config.list_named_queries() {
        if !result.iter().any(|r| r.name == name) {
            result.push(NamedQueryResponse {
                key: format!("legacy::{name}"),
                name,
                query,
                scope: "global".to_string(),
            });
        }
    }

    Ok(result)
}

#[tauri::command]
fn save_named_query(
    state: State<'_, AppState>,
    name: String,
    query: String,
    global: bool,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();

    if global {
        config
            .add_named_query_with_scope(&name, &query, dbcrust::config::NamedQueryScope::Global)
            .map_err(|e| format!("{e}"))
    } else {
        let db_guard = state.db.lock().unwrap();
        let scope = match db_guard.as_ref() {
            Some(db) => dbcrust::config::NamedQueryScope::DatabaseType(db.get_database_type()),
            None => dbcrust::config::NamedQueryScope::Global,
        };
        config
            .add_named_query_with_scope(&name, &query, scope)
            .map_err(|e| format!("{e}"))
    }
}

#[tauri::command]
fn delete_named_query(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    config
        .delete_named_query(&name)
        .map_err(|e| format!("{e}"))?;
    Ok(())
}

#[tauri::command]
fn delete_named_query_entry(state: State<'_, AppState>, key: String) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();

    if let Some(name) = key.strip_prefix("legacy::") {
        config
            .delete_named_query(name)
            .map_err(|e| format!("{e}"))?;
        return Ok(());
    }

    let scope = config
        .get_named_query_with_scope(&key)
        .map(|query| query.scope.clone())
        .ok_or_else(|| format!("Named query '{key}' not found"))?;

    let name = key
        .rsplit("::")
        .next()
        .ok_or_else(|| format!("Invalid named query key: {key}"))?;

    config
        .delete_named_query_with_scope(name, &scope)
        .map_err(|e| format!("{e}"))?;
    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// Config Commands
// ══════════════════════════════════════════════════════════════════════════════

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> Result<ConfigResponse, String> {
    let config = state.config.lock().unwrap();
    Ok(ConfigResponse {
        default_limit: config.default_limit,
        expanded_display: config.expanded_display_default,
        autocomplete_enabled: config.autocomplete_enabled,
        show_banner: config.show_banner,
        show_server_info: config.show_server_info,
        pager_enabled: config.pager_enabled,
        query_timeout_seconds: config.query_timeout_seconds,
        explain_mode: config.explain_mode_default,
    })
}

#[tauri::command]
fn discover_docker_containers() -> Result<Vec<DockerContainerResponse>, String> {
    std::thread::spawn(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            let client = DockerClient::new().map_err(|e| format!("Docker not available: {e}"))?;
            let containers = client
                .list_database_containers()
                .await
                .map_err(|e| format!("{e}"))?;
            Ok(containers
                .into_iter()
                .map(|c| {
                    let is_running = c.status.contains("running") || c.status.contains("Up");
                    DockerContainerResponse {
                        id: c.id,
                        name: c.name,
                        image: c.image,
                        status: c.status,
                        database_type: c.database_type.map(|dt| dt.display_name().to_string()),
                        host_port: c.host_port,
                        container_port: c.container_port,
                        is_running,
                    }
                })
                .collect())
        })
    })
    .join()
    .map_err(|_| "Docker discovery thread panicked".to_string())?
}

// ══════════════════════════════════════════════════════════════════════════════
// Vault Discovery Commands
// ══════════════════════════════════════════════════════════════════════════════

#[tauri::command]
fn list_vault_databases(mount_path: String) -> Result<Vec<String>, String> {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            let all_databases = dbcrust::vault_client::list_vault_databases(&mount_path)
                .await
                .map_err(|e| format!("Failed to list Vault databases: {e}"))?;

            dbcrust::vault_client::filter_databases_with_available_roles(&mount_path, all_databases)
                .await
                .map_err(|e| format!("Failed to filter accessible databases: {e}"))
        })
    })
    .join()
    .map_err(|_| "Vault discovery thread panicked".to_string())?
}

#[tauri::command]
fn list_vault_roles(mount_path: String, database_name: String) -> Result<Vec<String>, String> {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            dbcrust::vault_client::get_available_roles_for_user(&mount_path, &database_name)
                .await
                .map_err(|e| format!("Failed to list Vault roles: {e}"))
        })
    })
    .join()
    .map_err(|_| "Vault roles thread panicked".to_string())?
}

#[tauri::command]
fn update_config(state: State<'_, AppState>, key: String, value: String) -> Result<(), String> {
    let mut config = state.config.lock().unwrap();
    match key.as_str() {
        "default_limit" => {
            config.default_limit = value.parse().map_err(|_| "Invalid number")?;
        }
        "expanded_display" => {
            config.expanded_display_default = value.parse().map_err(|_| "Invalid boolean")?;
        }
        "query_timeout_seconds" => {
            config.query_timeout_seconds = value.parse().map_err(|_| "Invalid number")?;
        }
        _ => return Err(format!("Unknown config key: {key}")),
    }
    config
        .save()
        .map_err(|e| format!("Failed to save config: {e}"))
}

// ══════════════════════════════════════════════════════════════════════════════
// Application Entry Point
// ══════════════════════════════════════════════════════════════════════════════

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(AppState::new())
        .setup(|app| {
            apply_window_icon(app);

            // ── Custom application menu ──────────────────────────────
            let about_metadata = AboutMetadataBuilder::new()
                .name(Some("DBCrust"))
                .version(Some("0.1.0"))
                .comments(Some("A modern database management tool"))
                .build();

            let app_submenu = SubmenuBuilder::new(app, "DBCrust")
                .about(Some(about_metadata))
                .separator()
                .services()
                .separator()
                .hide()
                .hide_others()
                .show_all()
                .separator()
                .item(
                    &MenuItemBuilder::with_id(MENU_QUIT_APP, "Quit DBCrust")
                        .accelerator("CmdOrCtrl+Q")
                        .build(app)?,
                )
                .build()?;

            let file_submenu = SubmenuBuilder::new(app, "File")
                .item(
                    &MenuItemBuilder::with_id("new_tab", "New Query Tab")
                        .accelerator("CmdOrCtrl+T")
                        .build(app)?,
                )
                .item(
                    &MenuItemBuilder::with_id("close_tab", "Close Tab")
                        .accelerator("CmdOrCtrl+W")
                        .build(app)?,
                )
                .separator()
                .item(
                    &MenuItemBuilder::with_id("disconnect", "Disconnect")
                        .accelerator("CmdOrCtrl+Shift+D")
                        .build(app)?,
                )
                .build()?;

            let edit_submenu = SubmenuBuilder::new(app, "Edit")
                .undo()
                .redo()
                .separator()
                .cut()
                .copy()
                .paste()
                .select_all()
                .build()?;

            let view_submenu = SubmenuBuilder::new(app, "View")
                .item(
                    &MenuItemBuilder::with_id("view_connect", "Connect")
                        .accelerator("CmdOrCtrl+1")
                        .build(app)?,
                )
                .item(
                    &MenuItemBuilder::with_id("view_docker", "Docker Discovery")
                        .accelerator("CmdOrCtrl+2")
                        .build(app)?,
                )
                .separator()
                .item(
                    &MenuItemBuilder::with_id("view_query", "Query Editor")
                        .accelerator("CmdOrCtrl+3")
                        .build(app)?,
                )
                .item(
                    &MenuItemBuilder::with_id("view_schema", "Schema Explorer")
                        .accelerator("CmdOrCtrl+4")
                        .build(app)?,
                )
                .item(
                    &MenuItemBuilder::with_id("view_settings", "Settings")
                        .accelerator("CmdOrCtrl+,")
                        .build(app)?,
                )
                .separator()
                .item(&PredefinedMenuItem::fullscreen(app, None)?)
                .build()?;

            let query_submenu = SubmenuBuilder::new(app, "Query")
                .item(
                    &MenuItemBuilder::with_id("run_query", "Run Query")
                        .accelerator("CmdOrCtrl+Return")
                        .build(app)?,
                )
                .item(
                    &MenuItemBuilder::with_id("explain_query", "Explain Query")
                        .accelerator("CmdOrCtrl+Shift+Return")
                        .build(app)?,
                )
                .separator()
                .item(
                    &MenuItemBuilder::with_id("save_preset", "Save as Preset…")
                        .accelerator("CmdOrCtrl+S")
                        .build(app)?,
                )
                .build()?;

            let window_submenu = SubmenuBuilder::new(app, "Window")
                .minimize()
                .maximize()
                .separator()
                .close_window()
                .build()?;

            let menu = MenuBuilder::new(app)
                .item(&app_submenu)
                .item(&file_submenu)
                .item(&edit_submenu)
                .item(&view_submenu)
                .item(&query_submenu)
                .item(&window_submenu)
                .build()?;

            app.set_menu(menu)?;

            let tray_toggle_item =
                MenuItemBuilder::with_id(MENU_TRAY_TOGGLE, "Show DBCrust").build(app)?;
            let tray_menu = MenuBuilder::new(app)
                .item(&tray_toggle_item)
                .separator()
                .item(&MenuItemBuilder::with_id(MENU_TRAY_QUIT, "Quit DBCrust").build(app)?)
                .build()?;

            *app.state::<AppState>().tray_toggle_item.lock().unwrap() = Some(tray_toggle_item);

            let tray_icon = if cfg!(target_os = "macos") {
                load_tray_icon()?
            } else {
                app.default_window_icon()
                    .cloned()
                    .ok_or_else(|| tauri::Error::AssetNotFound("default window icon".into()))?
            };

            TrayIconBuilder::with_id("main-tray")
                .menu(&tray_menu)
                .tooltip("DBCrust")
                .show_menu_on_left_click(false)
                .icon(tray_icon)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

            sync_tray_toggle_item(app);

            Ok(())
        })
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();

            match id {
                MENU_QUIT_APP | MENU_TRAY_QUIT => {
                    app.state::<AppState>()
                        .quitting
                        .store(true, Ordering::Relaxed);
                    app.exit(0);
                    return;
                }
                MENU_TRAY_TOGGLE => {
                    toggle_main_window(app);
                    return;
                }
                _ => {}
            }

            if let Some(webview) = app.webview_windows().values().next() {
                let _ = webview.eval(&format!(
                    "window.__DBCRUST_MENU__ && window.__DBCRUST_MENU__('{}')",
                    id
                ));
            }
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                if !window.state::<AppState>().quitting.load(Ordering::Relaxed) {
                    api.prevent_close();
                    hide_main_window(window);
                }
            }
            WindowEvent::Focused(_) | WindowEvent::Resized(_) | WindowEvent::Moved(_) => {
                sync_tray_toggle_item(window)
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            disconnect,
            get_connection_state,
            get_database_types,
            execute_query,
            explain_query,
            list_databases,
            list_tables,
            describe_table,
            list_users,
            list_indexes,
            discover_docker_containers,
            list_vault_databases,
            list_vault_roles,
            list_recent_connections,
            list_sessions,
            connect_saved_session,
            connect_recent_connection,
            save_session,
            delete_session,
            list_named_queries,
            save_named_query,
            delete_named_query,
            delete_named_query_entry,
            get_config,
            update_config,
        ])
        .run(tauri::generate_context!())
        .expect("error while running DBCrust GUI");
}
