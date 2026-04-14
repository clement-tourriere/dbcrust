// ══════════════════════════════════════════════════════════════════════════════
// Type definitions matching the Tauri backend API
// ══════════════════════════════════════════════════════════════════════════════

export interface ConnectionState {
  connected: boolean;
  database_type: string;
  database_name: string;
  host: string;
  port: number;
  username: string;
  url: string;
}

export interface QueryResult {
  columns: string[];
  rows: string[][];
  row_count: number;
  elapsed_ms: number;
}

export interface TableDetail {
  name: string;
  schema: string;
  columns: ColumnDetail[];
  indexes: IndexDetail[];
  foreign_keys: ForeignKeyDetail[];
}

export interface ColumnDetail {
  name: string;
  data_type: string;
  nullable: boolean;
  default_value: string | null;
}

export interface IndexDetail {
  name: string;
  index_type: string;
  is_primary: boolean;
  is_unique: boolean;
}

export interface ForeignKeyDetail {
  name: string;
  definition: string;
}

export interface RecentConnection {
  display_name: string;
  connection_url: string;
  database_type: string;
  timestamp: string;
  success: boolean;
}

export interface SavedSession {
  name: string;
  host: string;
  port: number;
  user: string;
  dbname: string;
  database_type: string;
  target: string;
}

export interface NamedQuery {
  key: string;
  name: string;
  query: string;
  scope: string;
}

export interface AppConfig {
  default_limit: number;
  expanded_display: boolean;
  autocomplete_enabled: boolean;
  show_banner: boolean;
  show_server_info: boolean;
  pager_enabled: boolean;
  query_timeout_seconds: number;
  explain_mode: boolean;
}

export interface DatabaseTypeInfo {
  name: string;
  scheme: string;
  default_port: number | null;
  placeholder: string;
}

export interface EditorTab {
  id: string;
  title: string;
  sql: string;
  results: QueryResult | null;
  error: string | null;
  isRunning: boolean;
  isExplain?: boolean;
}

export type NavigationView = 'home' | 'saved' | 'query' | 'schema' | 'docker' | 'settings';

export interface DockerContainer {
  id: string;
  name: string;
  image: string;
  status: string;
  database_type: string | null;
  host_port: number | null;
  container_port: number | null;
  is_running: boolean;
}
