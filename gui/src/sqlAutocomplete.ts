import type { Completion, CompletionSource } from "@codemirror/autocomplete";
import {
  MySQL,
  PostgreSQL,
  SQLite,
  StandardSQL,
  keywordCompletionSource,
  type SQLConfig,
  type SQLDialect,
  type SQLNamespace,
} from "@codemirror/lang-sql";
import * as cmd from "./commands";
import { isSystemTableName, sortTablesForUi } from "./tableMetadata";

const RESERVED_ALIASES = new Set([
  "WHERE",
  "JOIN",
  "LEFT",
  "RIGHT",
  "FULL",
  "INNER",
  "OUTER",
  "ON",
  "GROUP",
  "ORDER",
  "LIMIT",
  "OFFSET",
  "HAVING",
  "UNION",
  "EXCEPT",
  "INTERSECT",
  "RETURNING",
  "SET",
  "VALUES",
]);

function trimIdentifierQuotes(identifier: string): string {
  const trimmed = identifier.trim();

  if (
    (trimmed.startsWith('"') && trimmed.endsWith('"')) ||
    (trimmed.startsWith("`") && trimmed.endsWith("`")) ||
    (trimmed.startsWith("[") && trimmed.endsWith("]"))
  ) {
    return trimmed.slice(1, -1);
  }

  return trimmed;
}

function normalizeTableReference(identifier: string): string {
  const parts = identifier
    .split(".")
    .map((part) => trimIdentifierQuotes(part))
    .filter(Boolean);

  return (parts.length > 0 ? parts[parts.length - 1] : "").toLowerCase();
}

function buildTableLookup(tables: readonly string[]): Map<string, string> {
  return new Map(
    tables.map((tableName) => [normalizeTableReference(tableName), tableName]),
  );
}

function extractTableReferences(
  sql: string,
  tableLookup: Map<string, string>,
): Map<string, string> {
  const references = new Map<string, string>();
  const matcher = /\b(from|join|update|into)\s+((?:"[^"]+"|`[^`]+`|\[[^\]]+\]|[A-Za-z_][\w$]*)(?:\.(?:"[^"]+"|`[^`]+`|\[[^\]]+\]|[A-Za-z_][\w$]*))?)(?:\s+(?:as\s+)?((?:"[^"]+"|`[^`]+`|\[[^\]]+\]|[A-Za-z_][\w$]*)))?/gi;

  let match: RegExpExecArray | null = matcher.exec(sql);
  while (match) {
    const rawTable = match[2];
    const rawTableParts = rawTable.split(".");
    const tableTail =
      rawTableParts.length > 0 ? rawTableParts[rawTableParts.length - 1] : rawTable;
    const exactTableName =
      tableLookup.get(normalizeTableReference(rawTable)) ??
      trimIdentifierQuotes(tableTail);

    references.set(exactTableName.toLowerCase(), exactTableName);

    const rawAlias = match[3];
    if (rawAlias) {
      const alias = trimIdentifierQuotes(rawAlias);
      if (alias && !RESERVED_ALIASES.has(alias.toUpperCase())) {
        references.set(alias.toLowerCase(), exactTableName);
      }
    }

    match = matcher.exec(sql);
  }

  return references;
}

function isTableNameContext(sqlBeforeCursor: string): boolean {
  return /\b(from|join|update|into|table|describe|desc|truncate)\s+[\w$".`\[\]]*$/i.test(
    sqlBeforeCursor,
  );
}

export function getSqlDialect(databaseType?: string): SQLDialect {
  switch (databaseType) {
    case "PostgreSQL":
      return PostgreSQL;
    case "MySQL":
      return MySQL;
    case "SQLite":
      return SQLite;
    default:
      return StandardSQL;
  }
}

export function buildSqlSchema(
  tables: readonly string[],
  databaseType?: string,
  columnsByTable?: ReadonlyMap<string, readonly string[]>,
): SQLNamespace {
  const namespace: Record<string, SQLNamespace> = {};

  for (const tableName of sortTablesForUi(tables, databaseType)) {
    const systemObject = isSystemTableName(tableName, databaseType);
    const columns = columnsByTable?.get(tableName) ?? [];

    namespace[tableName] = {
      self: {
        label: tableName,
        type: "type",
        detail: systemObject ? "system object" : "table",
        boost: systemObject ? 10 : 80,
        sortText: `${systemObject ? "1" : "0"}:${tableName.toLowerCase()}`,
      },
      children: columns.map((columnName) => ({
        label: columnName,
        type: "property",
        detail: tableName,
        boost: systemObject ? 0 : 45,
      })),
    };
  }

  return namespace;
}

export function buildKeywordCompletionSource(
  dialect: SQLDialect,
): CompletionSource {
  return keywordCompletionSource(dialect, true, (label, type): Completion => ({
    label,
    type,
    boost: -20,
  }));
}

export function createColumnCompletionSource(
  tables: readonly string[],
): CompletionSource {
  const tableLookup = buildTableLookup(tables);
  const columnCache = new Map<string, Promise<string[]>>();

  async function getColumnsForTable(tableName: string): Promise<string[]> {
    const cacheKey = tableName.toLowerCase();
    if (!columnCache.has(cacheKey)) {
      columnCache.set(
        cacheKey,
        cmd
          .describeTable(tableName)
          .then((detail) => detail.columns.map((column) => column.name))
          .catch(() => []),
      );
    }

    return columnCache.get(cacheKey) ?? Promise.resolve([]);
  }

  return async (context) => {
    const token = context.matchBefore(/[\w$".`\[\]]*/);
    if (!token) return null;
    if (!context.explicit && token.from === token.to) return null;

    const sqlBeforeCursor = context.state.doc.sliceString(0, context.pos);
    if (isTableNameContext(sqlBeforeCursor)) {
      return null;
    }

    const references = extractTableReferences(sqlBeforeCursor, tableLookup);
    const typedValue = token.text ?? "";
    const lastDot = typedValue.lastIndexOf(".");

    if (lastDot >= 0) {
      const qualifier = typedValue.slice(0, lastDot);
      const columnPrefix = typedValue.slice(lastDot + 1).toLowerCase();
      const tableName =
        references.get(trimIdentifierQuotes(qualifier).toLowerCase()) ??
        tableLookup.get(normalizeTableReference(qualifier));

      if (!tableName) {
        return null;
      }

      const columns = await getColumnsForTable(tableName);
      return {
        from: token.from,
        options: columns
          .filter((columnName) => columnName.toLowerCase().includes(columnPrefix))
          .map((columnName): Completion => ({
            label: `${qualifier}.${columnName}`,
            type: "property",
            detail: tableName,
            boost: 60,
          })),
        validFor: /^[\w$".`\[\]]*$/,
      };
    }

    const referencedTables = Array.from(new Set(references.values()));
    if (referencedTables.length !== 1) {
      return null;
    }

    const [tableName] = referencedTables;
    const columns = await getColumnsForTable(tableName);
    const prefix = typedValue.toLowerCase();

    return {
      from: token.from,
      options: columns
        .filter((columnName) => !prefix || columnName.toLowerCase().includes(prefix))
        .map((columnName): Completion => ({
          label: columnName,
          type: "property",
          detail: tableName,
          boost: 50,
        })),
      validFor: /^[\w$"`\[\]]*$/,
    };
  };
}

export function createSqlCompletionConfig(
  tables: readonly string[],
  databaseType?: string,
): SQLConfig {
  return {
    dialect: getSqlDialect(databaseType),
    schema: buildSqlSchema(tables, databaseType),
    upperCaseKeywords: true,
  };
}
