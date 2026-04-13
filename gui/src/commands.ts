// ══════════════════════════════════════════════════════════════════════════════
// Tauri command wrappers — typed interface to the Rust backend
// ══════════════════════════════════════════════════════════════════════════════

import { invoke } from "@tauri-apps/api/core";
import type {
  ConnectionState,
  QueryResult,
  TableDetail,
  RecentConnection,
  SavedSession,
  NamedQuery,
  AppConfig,
  DatabaseTypeInfo,
} from "./types";

// ── Connection ───────────────────────────────────────────────────────────────

export async function connectToDatabase(url: string): Promise<ConnectionState> {
  return invoke<ConnectionState>("connect", { url });
}

export async function disconnectFromDatabase(): Promise<void> {
  return invoke("disconnect");
}

export async function getConnectionState(): Promise<ConnectionState | null> {
  return invoke<ConnectionState | null>("get_connection_state");
}

export function getDatabaseTypes(): Promise<DatabaseTypeInfo[]> {
  return invoke<DatabaseTypeInfo[]>("get_database_types");
}

// ── Queries ──────────────────────────────────────────────────────────────────

export async function executeQuery(sql: string): Promise<QueryResult> {
  return invoke<QueryResult>("execute_query", { sql });
}

export async function explainQuery(sql: string): Promise<QueryResult> {
  return invoke<QueryResult>("explain_query", { sql });
}

// ── Schema ───────────────────────────────────────────────────────────────────

export async function listDatabases(): Promise<QueryResult> {
  return invoke<QueryResult>("list_databases");
}

export async function listTables(): Promise<QueryResult> {
  return invoke<QueryResult>("list_tables");
}

export async function describeTable(tableName: string): Promise<TableDetail> {
  return invoke<TableDetail>("describe_table", { tableName });
}

export async function listUsers(): Promise<QueryResult> {
  return invoke<QueryResult>("list_users");
}

export async function listIndexes(): Promise<QueryResult> {
  return invoke<QueryResult>("list_indexes");
}

// ── Sessions & History ───────────────────────────────────────────────────────

export async function listRecentConnections(): Promise<RecentConnection[]> {
  return invoke<RecentConnection[]>("list_recent_connections");
}

export async function listSessions(): Promise<SavedSession[]> {
  return invoke<SavedSession[]>("list_sessions");
}

export async function connectSavedSession(
  name: string,
): Promise<ConnectionState> {
  return invoke<ConnectionState>("connect_saved_session", { name });
}

export async function connectRecentConnection(
  index: number,
): Promise<ConnectionState> {
  return invoke<ConnectionState>("connect_recent_connection", { index });
}

export async function saveSession(name: string): Promise<void> {
  return invoke("save_session", { name });
}

export async function deleteSession(name: string): Promise<void> {
  return invoke("delete_session", { name });
}

// ── Named Queries ────────────────────────────────────────────────────────────

export async function listNamedQueries(): Promise<NamedQuery[]> {
  return invoke<NamedQuery[]>("list_named_queries");
}

export async function saveNamedQuery(
  name: string,
  query: string,
  global: boolean,
): Promise<void> {
  return invoke("save_named_query", { name, query, global });
}

export async function deleteNamedQuery(name: string): Promise<void> {
  return invoke("delete_named_query", { name });
}

export async function deleteNamedQueryEntry(key: string): Promise<void> {
  return invoke("delete_named_query_entry", { key });
}

// ── Config ───────────────────────────────────────────────────────────────────

export async function getConfig(): Promise<AppConfig> {
  return invoke<AppConfig>("get_config");
}

export async function updateConfig(
  key: string,
  value: string,
): Promise<void> {
  return invoke("update_config", { key, value });
}
