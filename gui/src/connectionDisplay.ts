import type { ConnectionState } from "./types";

const FILE_BASED_DATABASES = new Set([
  "SQLite",
  "Parquet",
  "CSV",
  "JSON",
  "DuckDB",
]);

function stripScheme(url: string): string {
  const withoutScheme = url.replace(/^[a-z0-9+.-]+:\/\//i, "");
  return withoutScheme.startsWith("//") ? withoutScheme.slice(1) : withoutScheme;
}

export function formatConnectionTarget(connection: ConnectionState): string {
  if (!connection.url) {
    return `${connection.username}@${connection.host}:${connection.port}`;
  }

  if (FILE_BASED_DATABASES.has(connection.database_type)) {
    return stripScheme(connection.url);
  }

  if (
    connection.url.startsWith("vault://") ||
    connection.url.startsWith("docker://")
  ) {
    return connection.url;
  }

  if (connection.host) {
    const authPrefix = connection.username ? `${connection.username}@` : "";
    return `${authPrefix}${connection.host}:${connection.port}`;
  }

  return connection.url;
}
