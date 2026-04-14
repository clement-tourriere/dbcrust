export function isSystemTableName(
  tableName: string,
  databaseType?: string,
): boolean {
  const normalized = tableName.trim().toLowerCase();

  if (!normalized) return false;

  switch (databaseType) {
    case "PostgreSQL":
      return (
        normalized.startsWith("pg_") ||
        normalized.startsWith("information_schema") ||
        normalized.startsWith("pg_toast")
      );
    case "MySQL":
      return (
        normalized.startsWith("information_schema") ||
        normalized.startsWith("performance_schema") ||
        normalized === "mysql" ||
        normalized.startsWith("mysql.") ||
        normalized === "sys" ||
        normalized.startsWith("sys.")
      );
    case "SQLite":
      return normalized.startsWith("sqlite_");
    case "ClickHouse":
      return normalized === "system" || normalized.startsWith("system.");
    case "MongoDB":
      return normalized.startsWith("system.");
    case "Elasticsearch":
      return normalized.startsWith(".");
    case "DuckDB":
      return normalized.startsWith("duckdb_") || normalized.startsWith("sqlite_");
    default:
      return (
        normalized.startsWith("pg_") ||
        normalized.startsWith("information_schema") ||
        normalized.startsWith("sqlite_") ||
        normalized.startsWith("system.")
      );
  }
}

export function sortTablesForUi(
  tables: readonly string[],
  databaseType?: string,
): string[] {
  return Array.from(new Set(tables.filter(Boolean))).sort((left, right) => {
    const leftSystem = isSystemTableName(left, databaseType) ? 1 : 0;
    const rightSystem = isSystemTableName(right, databaseType) ? 1 : 0;

    if (leftSystem !== rightSystem) {
      return leftSystem - rightSystem;
    }

    return left.localeCompare(right, undefined, {
      numeric: true,
      sensitivity: "base",
    });
  });
}

export function extractTableNames(
  rows: readonly string[][],
  databaseType?: string,
): string[] {
  return sortTablesForUi(
    rows
      .map((row) => row[1])
      .filter((tableName): tableName is string => Boolean(tableName?.trim())),
    databaseType,
  );
}
