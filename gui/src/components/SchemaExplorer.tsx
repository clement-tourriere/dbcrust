import { useState, useEffect, useCallback } from "react";
import {
  Search,
  RefreshCw,
  Table2,
  Columns3,
  Key,
  Link2,
  Loader2,
  Copy,
  Check,
  Play,
  Hash,
  ArrowRight,
} from "lucide-react";
import * as cmd from "../commands";
import type { ConnectionState, TableDetail } from "../types";

interface SchemaExplorerProps {
  connection: ConnectionState;
  tables: string[];
  onRefreshTables: () => void;
  onTableSelect: (tableName: string) => void;
}

export function SchemaExplorer({
  connection,
  tables,
  onRefreshTables,
  onTableSelect,
}: SchemaExplorerProps) {
  const [search, setSearch] = useState("");
  const [selectedTable, setSelectedTable] = useState<string | null>(null);
  const [tableDetail, setTableDetail] = useState<TableDetail | null>(null);
  const [loading, setLoading] = useState(false);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [refreshing, setRefreshing] = useState(false);

  const filteredTables = tables.filter((t) =>
    t.toLowerCase().includes(search.toLowerCase()),
  );

  const selectTable = useCallback(async (tableName: string) => {
    setSelectedTable(tableName);
    setLoading(true);
    setTableDetail(null);
    setDetailError(null);
    try {
      const detail = await cmd.describeTable(tableName);
      setTableDetail(detail);
    } catch (e) {
      setDetailError(String(e));
      setTableDetail(null);
    }
    setLoading(false);
  }, []);

  // Auto-select first table
  useEffect(() => {
    if (filteredTables.length > 0 && !selectedTable) {
      selectTable(filteredTables[0]);
    }
  }, [tables]);

  const handleRefresh = useCallback(async () => {
    setRefreshing(true);
    onRefreshTables();
    setRefreshing(false);
  }, [onRefreshTables]);

  const copyDDL = useCallback(() => {
    if (!selectedTable || !tableDetail) return;
    const cols = tableDetail.columns
      .map(
        (c) =>
          `  ${c.name} ${c.data_type}${c.nullable ? "" : " NOT NULL"}${c.default_value ? ` DEFAULT ${c.default_value}` : ""}`,
      )
      .join(",\n");
    const text = `CREATE TABLE ${selectedTable} (\n${cols}\n);`;
    navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }, [selectedTable, tableDetail]);

  return (
    <div className="h-full flex bg-surface-300 animate-fade-in">
      {/* ── Left: Table List ──────────────────────────────────────────── */}
      <div className="w-72 border-r border-zinc-800 flex flex-col bg-surface-200 flex-shrink-0">
        <div className="p-3 border-b border-zinc-800">
          <div className="flex items-center justify-between mb-2">
            <h2 className="text-xs font-semibold text-zinc-400 uppercase tracking-wider flex items-center gap-1.5">
              <Table2 className="w-3.5 h-3.5" />
              Tables
              <span className="text-zinc-600 font-normal">
                ({filteredTables.length})
              </span>
            </h2>
            <button
              onClick={handleRefresh}
              disabled={refreshing}
              className="p-1 rounded text-zinc-600 hover:text-zinc-400 hover:bg-zinc-800 transition-colors"
              title="Refresh"
            >
              <RefreshCw
                className={`w-3.5 h-3.5 ${refreshing ? "animate-spin" : ""}`}
              />
            </button>
          </div>
          <div className="relative">
            <Search className="w-3.5 h-3.5 absolute left-2.5 top-1/2 -translate-y-1/2 text-zinc-500" />
            <input
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Filter tables..."
              className="w-full bg-surface-300 border border-zinc-800 rounded-md pl-8 pr-3 py-1.5
                text-xs text-zinc-300 placeholder-zinc-600 focus:outline-none focus:border-zinc-600 transition-colors"
            />
          </div>
        </div>

        <div className="flex-1 overflow-y-auto py-1">
          {filteredTables.length === 0 ? (
            <div className="px-3 py-8 text-center text-zinc-600 text-xs">
              {search ? "No matching tables" : "No tables found"}
            </div>
          ) : (
            <div className="space-y-px px-1">
              {filteredTables.map((table) => (
                <button
                  key={table}
                  onClick={() => selectTable(table)}
                  className={`w-full text-left px-3 py-2 rounded-md text-xs font-mono truncate transition-all
                    ${
                      selectedTable === table
                        ? "bg-accent/10 text-accent border-l-2 border-accent"
                        : "text-zinc-400 hover:bg-zinc-800 hover:text-zinc-200"
                    }`}
                >
                  {table}
                </button>
              ))}
            </div>
          )}
        </div>
      </div>

      {/* ── Right: Table Details ──────────────────────────────────────── */}
      <div className="flex-1 overflow-auto">
        {!selectedTable ? (
          <div className="h-full flex items-center justify-center text-zinc-600">
            <div className="text-center">
              <Table2 className="w-10 h-10 mx-auto mb-3 text-zinc-700" />
              <p className="text-sm">Select a table to view its schema</p>
            </div>
          </div>
        ) : loading ? (
          <div className="h-full flex items-center justify-center text-zinc-500">
            <Loader2 className="w-5 h-5 animate-spin mr-2" />
            <span className="text-sm">Loading schema…</span>
          </div>
        ) : detailError ? (
          <div className="p-6">
            <div className="bg-red-500/10 border border-red-500/20 rounded-lg p-4 text-sm text-red-400">
              {detailError}
            </div>
          </div>
        ) : tableDetail ? (
          <div className="p-6 space-y-6 max-w-4xl">
            {/* Table Header */}
            <div className="flex items-center justify-between">
              <div>
                <h2 className="text-lg font-bold text-zinc-100 font-mono">
                  {selectedTable}
                </h2>
                <p className="text-xs text-zinc-500 mt-1">
                  {tableDetail.schema && `Schema: ${tableDetail.schema} · `}
                  {tableDetail.columns.length} columns ·{" "}
                  {tableDetail.indexes.length} indexes ·{" "}
                  {tableDetail.foreign_keys.length} foreign keys
                </p>
              </div>
              <div className="flex items-center gap-2">
                <button
                  onClick={copyDDL}
                  className="flex items-center gap-1.5 px-3 py-1.5 rounded-md text-xs font-medium
                    bg-zinc-800 hover:bg-zinc-700 text-zinc-400 hover:text-zinc-200 transition-all"
                >
                  {copied ? (
                    <Check className="w-3 h-3 text-emerald-500" />
                  ) : (
                    <Copy className="w-3 h-3" />
                  )}
                  {copied ? "Copied" : "Copy DDL"}
                </button>
                <button
                  onClick={() => onTableSelect(selectedTable)}
                  className="flex items-center gap-1.5 px-3 py-1.5 rounded-md text-xs font-medium
                    bg-emerald-600 hover:bg-emerald-500 text-white transition-all"
                >
                  <Play className="w-3 h-3" />
                  Query Table
                </button>
              </div>
            </div>

            {/* Columns */}
            <div className="bg-surface rounded-xl border border-zinc-800 overflow-hidden">
              <div className="px-4 py-3 border-b border-zinc-800 bg-surface-100">
                <h3 className="text-xs font-semibold text-zinc-400 uppercase tracking-wider flex items-center gap-1.5">
                  <Columns3 className="w-3.5 h-3.5" />
                  Columns
                  <span className="text-zinc-600 font-normal">
                    ({tableDetail.columns.length})
                  </span>
                </h3>
              </div>
              <table className="w-full text-xs">
                <thead>
                  <tr className="border-b border-zinc-800/50">
                    <th className="text-left px-4 py-2 text-zinc-500 font-semibold w-8">
                      #
                    </th>
                    <th className="text-left px-4 py-2 text-zinc-500 font-semibold">
                      Name
                    </th>
                    <th className="text-left px-4 py-2 text-zinc-500 font-semibold">
                      Type
                    </th>
                    <th className="text-left px-4 py-2 text-zinc-500 font-semibold w-20">
                      Nullable
                    </th>
                    <th className="text-left px-4 py-2 text-zinc-500 font-semibold">
                      Default
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {tableDetail.columns.map((col, i) => (
                    <tr
                      key={col.name}
                      className="border-b border-zinc-800/30 hover:bg-zinc-800/20 transition-colors"
                    >
                      <td className="px-4 py-2 text-zinc-600 tabular-nums">
                        {i + 1}
                      </td>
                      <td className="px-4 py-2 text-zinc-200 font-mono font-medium">
                        {col.name}
                      </td>
                      <td className="px-4 py-2 text-cyan-400 font-mono">
                        {col.data_type}
                      </td>
                      <td className="px-4 py-2">
                        {col.nullable ? (
                          <span className="text-zinc-600">YES</span>
                        ) : (
                          <span className="text-amber-500 font-medium">NO</span>
                        )}
                      </td>
                      <td className="px-4 py-2 text-zinc-500 font-mono truncate max-w-xs">
                        {col.default_value ?? (
                          <span className="text-zinc-700 italic">—</span>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>

            {/* Indexes */}
            {tableDetail.indexes.length > 0 && (
              <div className="bg-surface rounded-xl border border-zinc-800 overflow-hidden">
                <div className="px-4 py-3 border-b border-zinc-800 bg-surface-100">
                  <h3 className="text-xs font-semibold text-zinc-400 uppercase tracking-wider flex items-center gap-1.5">
                    <Key className="w-3.5 h-3.5" />
                    Indexes
                    <span className="text-zinc-600 font-normal">
                      ({tableDetail.indexes.length})
                    </span>
                  </h3>
                </div>
                <div className="divide-y divide-zinc-800/30">
                  {tableDetail.indexes.map((idx) => (
                    <div
                      key={idx.name}
                      className="px-4 py-3 flex items-center gap-3 hover:bg-zinc-800/20 transition-colors"
                    >
                      <Key
                        className={`w-4 h-4 flex-shrink-0 ${idx.is_primary ? "text-amber-500" : "text-zinc-600"}`}
                      />
                      <div className="min-w-0">
                        <div className="text-xs text-zinc-300 font-mono">
                          {idx.name}
                        </div>
                        <div className="text-xxs text-zinc-600 mt-0.5">
                          {idx.index_type}
                          {idx.is_primary && " · PRIMARY"}
                          {idx.is_unique && !idx.is_primary && " · UNIQUE"}
                        </div>
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            )}

            {/* Foreign Keys */}
            {tableDetail.foreign_keys.length > 0 && (
              <div className="bg-surface rounded-xl border border-zinc-800 overflow-hidden">
                <div className="px-4 py-3 border-b border-zinc-800 bg-surface-100">
                  <h3 className="text-xs font-semibold text-zinc-400 uppercase tracking-wider flex items-center gap-1.5">
                    <Link2 className="w-3.5 h-3.5" />
                    Foreign Keys
                    <span className="text-zinc-600 font-normal">
                      ({tableDetail.foreign_keys.length})
                    </span>
                  </h3>
                </div>
                <div className="divide-y divide-zinc-800/30">
                  {tableDetail.foreign_keys.map((fk) => (
                    <div
                      key={fk.name}
                      className="px-4 py-3 flex items-center gap-3 hover:bg-zinc-800/20 transition-colors"
                    >
                      <Link2 className="w-4 h-4 text-blue-500 flex-shrink-0" />
                      <div className="min-w-0">
                        <div className="text-xs text-zinc-300 font-mono">
                          {fk.name}
                        </div>
                        <div className="text-xxs text-zinc-500 font-mono mt-0.5 flex items-center gap-1">
                          <ArrowRight className="w-3 h-3" />
                          {fk.definition}
                        </div>
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            )}

            {/* Quick Queries */}
            <div className="bg-surface rounded-xl border border-zinc-800 overflow-hidden">
              <div className="px-4 py-3 border-b border-zinc-800 bg-surface-100">
                <h3 className="text-xs font-semibold text-zinc-400 uppercase tracking-wider flex items-center gap-1.5">
                  <Hash className="w-3.5 h-3.5" />
                  Quick Queries
                </h3>
              </div>
              <div className="p-3 flex flex-wrap gap-2">
                {[
                  `SELECT * FROM ${selectedTable} LIMIT 100`,
                  `SELECT COUNT(*) FROM ${selectedTable}`,
                  ...(connection.database_type === "PostgreSQL"
                    ? [`SELECT column_name, data_type FROM information_schema.columns WHERE table_name = '${selectedTable}'`]
                    : []),
                ].map((q) => (
                  <button
                    key={q}
                    onClick={() => onTableSelect(selectedTable)}
                    className="px-3 py-1.5 rounded-md bg-zinc-800 text-zinc-400 text-xs font-mono
                      hover:bg-zinc-700 hover:text-zinc-200 transition-all truncate max-w-sm"
                    title={q}
                  >
                    {q}
                  </button>
                ))}
              </div>
            </div>
          </div>
        ) : null}
      </div>
    </div>
  );
}

export default SchemaExplorer;
