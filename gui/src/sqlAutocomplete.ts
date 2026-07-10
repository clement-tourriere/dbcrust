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
import {
  formatColumnReference,
  formatIdentifier,
  formatTableReference,
  isSystemTableName,
  sortTablesForUi,
  splitQualifiedIdentifier,
} from "./tableMetadata";

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
  const trimmed = identifier.trim();
  const unquoted = trimIdentifierQuotes(trimmed);

  // A fully quoted Elasticsearch index such as "logs-2026.07" contains a dot
  // that is part of the name, not a schema separator.
  if (unquoted !== trimmed) {
    return unquoted.toLowerCase();
  }

  const parts = trimmed
    .split(".")
    .map((part) => trimIdentifierQuotes(part))
    .filter(Boolean);

  return parts.join(".").toLowerCase();
}

function buildTableLookup(tables: readonly string[]): Map<string, string> {
  const lookup = new Map<string, string>();
  for (const tableName of tables) {
    const normalized = normalizeTableReference(tableName);
    lookup.set(normalized, tableName);

    const normalizedParts = normalized.split(".");
    const tail = normalizedParts[normalizedParts.length - 1] ?? normalized;
    // Prefer an unqualified/default-schema object for an unqualified query.
    if (!lookup.has(tail) || !normalized.includes(".")) {
      lookup.set(tail, tableName);
    }
  }
  return lookup;
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
  return /\b(from|join|update|into|table|describe|desc|truncate)\s+[\w$".`\[\]\-:@#]*$/i.test(
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
    // Quote-aware split so a packed `analytics."v1.events"` keeps its quoted
    // part intact instead of splitting inside it.
    const qualifiedParts =
      databaseType === "PostgreSQL"
        ? splitQualifiedIdentifier(tableName)
        : [tableName];
    const completionLabel =
      qualifiedParts.length > 1
        ? qualifiedParts[qualifiedParts.length - 1]
        : tableName;
    const queryReference =
      completionLabel === tableName
        ? formatTableReference(tableName, databaseType)
        : formatIdentifier(completionLabel, databaseType);
    // CodeMirror treats dots in namespace keys as schema separators unless
    // escaped. Dots inside a part (Elasticsearch index names, quoted
    // PostgreSQL parts) belong to the name, so escape those.
    const namespaceKey = qualifiedParts
      .map((part) => part.replace(/\./g, "\\."))
      .join(".");

    namespace[namespaceKey] = {
      self: {
        label: completionLabel,
        apply: queryReference,
        type: "type",
        detail: systemObject ? "system object" : "table",
        boost: systemObject ? 10 : 80,
        sortText: `${systemObject ? "1" : "0"}:${tableName.toLowerCase()}`,
      },
      children: columns.map((columnName) => ({
        label: columnName,
        apply: formatColumnReference(columnName, databaseType, columns),
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
  databaseType?: string,
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
    const token = context.matchBefore(/[\w$".`\[\]\-:@#]*/);
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
            apply: `${qualifier}.${formatColumnReference(columnName, databaseType, columns)}`,
            type: "property",
            detail: tableName,
            boost: 60,
          })),
        validFor: /^[\w$".`\[\]\-:@#]*$/,
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
          apply: formatColumnReference(columnName, databaseType, columns),
          type: "property",
          detail: tableName,
          boost: 50,
        })),
      validFor: /^[\w$"`\[\]\-:@#]*$/,
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
