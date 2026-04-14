export type SqlExecutionMode = "selection" | "statement";

export interface SqlExecutionTarget {
  mode: SqlExecutionMode;
  sql: string;
  label: string;
  statementIndex: number;
  statementCount: number;
}

interface StatementRange {
  start: number;
  end: number;
}

function isWhitespace(char: string): boolean {
  return /\s/.test(char);
}

function lineNumberAt(doc: string, position: number): number {
  let line = 1;
  for (let index = 0; index < position && index < doc.length; index += 1) {
    if (doc[index] === "\n") {
      line += 1;
    }
  }
  return line;
}

function matchDollarTag(doc: string, position: number): string | null {
  const rest = doc.slice(position);
  const match = rest.match(/^\$[A-Za-z_][A-Za-z0-9_]*\$/) ?? rest.match(/^\$\$/);
  return match?.[0] ?? null;
}

function pushStatement(
  doc: string,
  statements: StatementRange[],
  rawStart: number,
  rawEnd: number,
) {
  let start = rawStart;
  let end = rawEnd;

  while (start < end && isWhitespace(doc[start])) {
    start += 1;
  }

  while (end > start && isWhitespace(doc[end - 1])) {
    end -= 1;
  }

  if (start < end) {
    statements.push({ start, end });
  }
}

export function splitSqlStatements(doc: string): StatementRange[] {
  const statements: StatementRange[] = [];

  let singleQuoted = false;
  let doubleQuoted = false;
  let lineComment = false;
  let blockComment = false;
  let dollarTag: string | null = null;
  let segmentStart = 0;
  let index = 0;

  while (index < doc.length) {
    const current = doc[index];
    const next = doc[index + 1];

    if (lineComment) {
      if (current === "\n") {
        lineComment = false;
      }
      index += 1;
      continue;
    }

    if (blockComment) {
      if (current === "*" && next === "/") {
        blockComment = false;
        index += 2;
        continue;
      }
      index += 1;
      continue;
    }

    if (dollarTag) {
      if (doc.startsWith(dollarTag, index)) {
        index += dollarTag.length;
        dollarTag = null;
        continue;
      }
      index += 1;
      continue;
    }

    if (singleQuoted) {
      if (current === "'" && next === "'") {
        index += 2;
        continue;
      }
      if (current === "'") {
        singleQuoted = false;
      }
      index += 1;
      continue;
    }

    if (doubleQuoted) {
      if (current === '"' && next === '"') {
        index += 2;
        continue;
      }
      if (current === '"') {
        doubleQuoted = false;
      }
      index += 1;
      continue;
    }

    if (current === "-" && next === "-") {
      lineComment = true;
      index += 2;
      continue;
    }

    if (current === "/" && next === "*") {
      blockComment = true;
      index += 2;
      continue;
    }

    if (current === "'") {
      singleQuoted = true;
      index += 1;
      continue;
    }

    if (current === '"') {
      doubleQuoted = true;
      index += 1;
      continue;
    }

    if (current === "$") {
      const tag = matchDollarTag(doc, index);
      if (tag) {
        dollarTag = tag;
        index += tag.length;
        continue;
      }
    }

    if (current === ";") {
      pushStatement(doc, statements, segmentStart, index);
      segmentStart = index + 1;
    }

    index += 1;
  }

  pushStatement(doc, statements, segmentStart, doc.length);
  return statements;
}

function nearestStatementIndex(
  statements: StatementRange[],
  cursor: number,
): number {
  let bestIndex = 0;
  let bestDistance = Number.POSITIVE_INFINITY;

  for (let index = 0; index < statements.length; index += 1) {
    const statement = statements[index];
    const distance =
      cursor < statement.start
        ? statement.start - cursor
        : cursor > statement.end
          ? cursor - statement.end
          : 0;

    if (distance < bestDistance) {
      bestDistance = distance;
      bestIndex = index;
    }
  }

  return bestIndex;
}

export function resolveSqlExecutionTarget(
  doc: string,
  selectionFrom: number,
  selectionTo: number,
): SqlExecutionTarget | null {
  const statements = splitSqlStatements(doc);

  if (!doc.trim() && statements.length === 0) {
    return null;
  }

  if (selectionFrom !== selectionTo) {
    const selectedSql = doc.slice(selectionFrom, selectionTo).trim();
    if (selectedSql) {
      const startLine = lineNumberAt(doc, selectionFrom);
      const endLine = lineNumberAt(doc, selectionTo);
      return {
        mode: "selection",
        sql: selectedSql,
        label:
          startLine === endLine
            ? `Selection · line ${startLine}`
            : `Selection · lines ${startLine}-${endLine}`,
        statementIndex: 1,
        statementCount: Math.max(statements.length, 1),
      };
    }
  }

  if (statements.length === 0) {
    const sql = doc.trim();
    return sql
      ? {
          mode: "statement",
          sql,
          label: "Current statement",
          statementIndex: 1,
          statementCount: 1,
        }
      : null;
  }

  const cursor = selectionTo;
  const statementIndex = nearestStatementIndex(statements, cursor);
  const statement = statements[statementIndex];
  const sql = doc.slice(statement.start, statement.end).trim();

  if (!sql) {
    return null;
  }

  return {
    mode: "statement",
    sql,
    label:
      statements.length === 1
        ? "Current statement"
        : `Statement ${statementIndex + 1} of ${statements.length}`,
    statementIndex: statementIndex + 1,
    statementCount: statements.length,
  };
}
