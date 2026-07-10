import { useState, useMemo } from "react";
import {
  Table2,
  Code2,
  ArrowUpDown,
  Loader2,
  AlertCircle,
  FileSpreadsheet,
  Copy,
  Check,
} from "lucide-react";
import type { QueryResult } from "../types";
import { ExplainView } from "./ExplainView";

interface ResultsPanelProps {
  results: QueryResult | null;
  error: string | null;
  isRunning: boolean;
  isExplain?: boolean;
}

type ViewMode = "table" | "json";
type SortConfig = { column: number; direction: "asc" | "desc" } | null;

function rowsAsObjects(results: QueryResult): Record<string, string | null>[] {
  const seen = new Map<string, number>();
  const keys = results.columns.map((column) => {
    const count = (seen.get(column) ?? 0) + 1;
    seen.set(column, count);
    return count === 1 ? column : `${column} (${count})`;
  });

  return results.rows.map((row) =>
    Object.fromEntries(keys.map((key, index) => [key, row[index] ?? null])),
  );
}

export function ResultsPanel({
  results,
  error,
  isRunning,
  isExplain,
}: ResultsPanelProps) {
  const [viewMode, setViewMode] = useState<ViewMode>("table");
  const [sortConfig, setSortConfig] = useState<SortConfig>(null);
  const [copied, setCopied] = useState(false);
  const [copyError, setCopyError] = useState(false);

  // Only materialized for the JSON view; the copy button builds it on click.
  const jsonRows = useMemo(
    () => (results && viewMode === "json" ? rowsAsObjects(results) : []),
    [results, viewMode],
  );

  // ── Sort data ──────────────────────────────────────────────────────────
  const sortedRows = useMemo(() => {
    if (!results || !sortConfig) return results?.rows ?? [];
    const { column, direction } = sortConfig;
    return [...results.rows].sort((a, b) => {
      // NULLs sort like empty strings (first ascending, last descending).
      const va = a[column] ?? "";
      const vb = b[column] ?? "";
      const na = Number(va);
      const nb = Number(vb);
      if (!isNaN(na) && !isNaN(nb)) {
        return direction === "asc" ? na - nb : nb - na;
      }
      return direction === "asc"
        ? va.localeCompare(vb)
        : vb.localeCompare(va);
    });
  }, [results, sortConfig]);

  const handleSort = (colIdx: number) => {
    setSortConfig((prev) => {
      if (prev?.column === colIdx) {
        return prev.direction === "asc"
          ? { column: colIdx, direction: "desc" }
          : null;
      }
      return { column: colIdx, direction: "asc" };
    });
  };

  const copyAsJson = async () => {
    if (!results) return;
    try {
      await navigator.clipboard.writeText(
        JSON.stringify(rowsAsObjects(results), null, 2),
      );
      setCopyError(false);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      setCopied(false);
      setCopyError(true);
      setTimeout(() => setCopyError(false), 2500);
    }
  };

  // ── Loading State ──────────────────────────────────────────────────────
  if (isRunning) {
    return (
      <div role="status" aria-live="polite" className="h-full flex items-center justify-center bg-surface text-zinc-500">
        <div className="flex items-center gap-3">
          <Loader2 className="w-5 h-5 animate-spin text-accent" />
          <span className="text-sm">
            {isExplain ? "Analyzing query plan…" : "Executing query…"}
          </span>
        </div>
      </div>
    );
  }

  // ── Error State ────────────────────────────────────────────────────────
  if (error) {
    return (
      <div className="h-full flex items-start p-4 bg-surface">
        <div role="alert" className="flex items-start gap-3 bg-red-500/10 border border-red-500/20 rounded-lg p-4 max-w-full">
          <AlertCircle className="w-5 h-5 text-red-400 flex-shrink-0 mt-0.5" />
          <div>
            <div className="text-sm font-medium text-red-400 mb-1">
              Query Error
            </div>
            <pre className="text-xs text-red-300/80 whitespace-pre-wrap break-all font-mono">
              {error}
            </pre>
          </div>
        </div>
      </div>
    );
  }

  // ── Empty State ────────────────────────────────────────────────────────
  if (!results) {
    const isMac =
      typeof navigator !== "undefined" &&
      /Mac|iPod|iPhone|iPad/.test(navigator.userAgent);
    const modKey = isMac ? "⌘" : "Ctrl";

    return (
      <div className="h-full flex items-center justify-center bg-surface text-zinc-600">
        <div className="text-center">
          <FileSpreadsheet className="w-8 h-8 mx-auto mb-3 text-zinc-700" />
          <p className="text-sm">Run a query to see results</p>
          <p className="text-xs text-zinc-700 mt-1">
            Press {modKey}+Enter to execute
          </p>
        </div>
      </div>
    );
  }

  // ── Explain View ───────────────────────────────────────────────────────
  if (isExplain) {
    return <ExplainView results={results} />;
  }

  // ── Results View ───────────────────────────────────────────────────────
  return (
    <div className="h-full flex flex-col bg-surface">
      {/* ── Results Header ──────────────────────────────────────────── */}
      <div className="flex items-center justify-between px-3 py-1.5 border-b border-zinc-800 bg-surface-100 flex-shrink-0">
        <div className="flex items-center gap-1">
          <button
            onClick={() => setViewMode("table")}
            aria-pressed={viewMode === "table"}
            className={`flex items-center gap-1.5 px-2 py-1 rounded text-xs font-medium transition-colors
              ${viewMode === "table" ? "bg-zinc-700 text-zinc-200" : "text-zinc-500 hover:text-zinc-300"}`}
          >
            <Table2 className="w-3 h-3" />
            Table
          </button>
          <button
            onClick={() => setViewMode("json")}
            aria-pressed={viewMode === "json"}
            className={`flex items-center gap-1.5 px-2 py-1 rounded text-xs font-medium transition-colors
              ${viewMode === "json" ? "bg-zinc-700 text-zinc-200" : "text-zinc-500 hover:text-zinc-300"}`}
          >
            <Code2 className="w-3 h-3" />
            JSON
          </button>
        </div>
        <div className="flex items-center gap-3">
          <button
            onClick={() => void copyAsJson()}
            className={`flex items-center gap-1 px-2 py-1 rounded text-xs hover:bg-zinc-800 transition-colors ${
              copyError ? "text-red-400" : "text-zinc-500 hover:text-zinc-300"
            }`}
            title={copyError ? "Clipboard access failed" : "Copy results as JSON"}
          >
            {copied ? (
              <Check className="w-3 h-3 text-emerald-500" />
            ) : (
              <Copy className="w-3 h-3" />
            )}
            {copyError ? "Copy failed" : copied ? "Copied" : "Copy JSON"}
          </button>
          <span className="text-xxs text-zinc-500">
            {results.row_count} row{results.row_count !== 1 ? "s" : ""} ·{" "}
            {results.elapsed_ms}ms
          </span>
        </div>
      </div>

      {/* ── Table View ──────────────────────────────────────────────── */}
      {viewMode === "table" && (
        <div className="flex-1 overflow-auto">
          <table className="data-grid w-full border-collapse">
            <thead>
              <tr>
                <th className="bg-surface-100 text-left px-3 py-1.5 text-xxs font-semibold text-zinc-500 border-b border-zinc-800 w-10">
                  #
                </th>
                {results.columns.map((col, i) => (
                  <th
                    key={i}
                    onClick={() => handleSort(i)}
                    className="bg-surface-100 text-left px-3 py-1.5 text-xs font-semibold text-zinc-400
                      border-b border-zinc-800 cursor-pointer hover:text-zinc-200 hover:bg-zinc-800/50
                      transition-colors select-none whitespace-nowrap"
                  >
                    <span className="flex items-center gap-1.5">
                      {col}
                      {sortConfig?.column === i && (
                        <ArrowUpDown className="w-3 h-3 text-accent" />
                      )}
                    </span>
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {sortedRows.map((row, ri) => (
                <tr
                  key={ri}
                  className="hover:bg-zinc-800/30 transition-colors border-b border-zinc-800/30"
                >
                  <td className="px-3 py-1 text-xxs text-zinc-600 tabular-nums">
                    {ri + 1}
                  </td>
                  {row.map((cell, ci) => (
                    <td
                      key={ci}
                      className="px-3 py-1 text-xs text-zinc-300 max-w-xs truncate"
                      title={cell ?? "NULL"}
                    >
                      {cell === null ? (
                        <span className="text-zinc-600 italic">NULL</span>
                      ) : cell === "" ? (
                        <span className="text-zinc-700 italic">empty</span>
                      ) : (
                        cell
                      )}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* ── JSON View ───────────────────────────────────────────────── */}
      {viewMode === "json" && (
        <div className="flex-1 overflow-auto p-3">
          <pre className="text-xs text-zinc-300 font-mono whitespace-pre-wrap">
            {JSON.stringify(jsonRows, null, 2)}
          </pre>
        </div>
      )}
    </div>
  );
}
