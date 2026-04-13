import { useState, useEffect, useCallback, useMemo } from "react";
import {
  Table2,
  ChevronRight,
  ChevronDown,
  Search,
  RefreshCw,
  LogOut,
  Columns3,
  Key,
  Link2,
  Loader2,
  Code2,
  Bookmark,
  BookmarkPlus,
  Shield,
  GitBranch,
  Boxes,
  Trash2,
} from "lucide-react";
import * as cmd from "../commands";
import type { ConnectionState, NamedQuery, TableDetail } from "../types";
import { formatConnectionTarget } from "../connectionDisplay";
import {
  getVisibleDjangoPresetGroups,
  getVisibleDjangoPresets,
} from "../queryPresets";

interface SidebarProps {
  connection: ConnectionState;
  tables: string[];
  onTableSelect: (tableName: string) => void;
  onLoadSnippet: (title: string, sql: string) => void;
  namedQueriesVersion: number;
  onDisconnect: () => void;
}

export function Sidebar({
  connection,
  tables,
  onTableSelect,
  onLoadSnippet,
  namedQueriesVersion,
  onDisconnect,
}: SidebarProps) {
  const [search, setSearch] = useState("");
  const [expandedTable, setExpandedTable] = useState<string | null>(null);
  const [tableDetail, setTableDetail] = useState<TableDetail | null>(null);
  const [loadingDetail, setLoadingDetail] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [localTables, setLocalTables] = useState(tables);
  const [namedQueries, setNamedQueries] = useState<NamedQuery[]>([]);
  const [presetMessage, setPresetMessage] = useState<string | null>(null);
  const [savingPreset, setSavingPreset] = useState<string | null>(null);
  const [deletingPreset, setDeletingPreset] = useState<string | null>(null);

  useEffect(() => {
    setLocalTables(tables);
  }, [tables]);

  useEffect(() => {
    if (!presetMessage) return;
    const timeout = window.setTimeout(() => setPresetMessage(null), 2400);
    return () => window.clearTimeout(timeout);
  }, [presetMessage]);

  const loadNamedQueries = useCallback(async () => {
    try {
      const queries = await cmd.listNamedQueries();
      queries.sort((a, b) => a.name.localeCompare(b.name) || a.scope.localeCompare(b.scope));
      setNamedQueries(queries);
    } catch {
      setNamedQueries([]);
    }
  }, []);

  useEffect(() => {
    loadNamedQueries().catch(() => {});
  }, [connection.database_type, namedQueriesVersion, loadNamedQueries]);

  const filteredTables = localTables.filter((t) =>
    t.toLowerCase().includes(search.toLowerCase()),
  );
  const djangoPresetGroups = useMemo(
    () => getVisibleDjangoPresetGroups(localTables),
    [localTables],
  );
  const visibleDjangoPresets = useMemo(
    () => getVisibleDjangoPresets(localTables),
    [localTables],
  );
  const hasDjangoToolkit = djangoPresetGroups.length > 0;

  const handleSavePreset = useCallback(
    async (name: string, query: string) => {
      setSavingPreset(name);
      try {
        await cmd.saveNamedQuery(name, query, false);
        setPresetMessage(`Saved preset ${name}`);
        await loadNamedQueries();
      } catch (error) {
        setPresetMessage(`Failed to save ${name}: ${String(error)}`);
      } finally {
        setSavingPreset(null);
      }
    },
    [loadNamedQueries],
  );

  const handleDeletePreset = useCallback(
    async (preset: NamedQuery) => {
      setDeletingPreset(preset.key);
      try {
        await cmd.deleteNamedQueryEntry(preset.key);
        setPresetMessage(`Deleted preset ${preset.name}`);
        await loadNamedQueries();
      } catch (error) {
        setPresetMessage(`Failed to delete ${preset.name}: ${String(error)}`);
      } finally {
        setDeletingPreset(null);
      }
    },
    [loadNamedQueries],
  );

  const handleSaveAllDjangoPresets = useCallback(async () => {
    for (const preset of visibleDjangoPresets) {
      await handleSavePreset(preset.name, preset.query);
    }
  }, [handleSavePreset, visibleDjangoPresets]);

  const handleRefresh = useCallback(async () => {
    setRefreshing(true);
    try {
      const result = await cmd.listTables();
      if (result.rows.length > 0) {
        setLocalTables(result.rows.map((r) => r[0]));
      }
    } catch {
      /* ignore */
    }
    setRefreshing(false);
  }, []);

  const toggleTable = useCallback(
    async (tableName: string) => {
      if (expandedTable === tableName) {
        setExpandedTable(null);
        setTableDetail(null);
        return;
      }
      setExpandedTable(tableName);
      setLoadingDetail(true);
      try {
        const detail = await cmd.describeTable(tableName);
        setTableDetail(detail);
      } catch {
        setTableDetail(null);
      }
      setLoadingDetail(false);
    },
    [expandedTable],
  );

  const DB_EMOJI: Record<string, string> = {
    PostgreSQL: "🐘",
    MySQL: "🐬",
    SQLite: "📦",
    ClickHouse: "⚡",
    MongoDB: "🍃",
    Elasticsearch: "🔍",
    Parquet: "📊",
    CSV: "📄",
    JSON: "🧾",
    DuckDB: "🦆",
  };

  return (
    <div className="h-full flex flex-col bg-surface-200 border-r border-zinc-800">
      {/* ── Connection Info ──────────────────────────────────────────── */}
      <div className="p-3 border-b border-zinc-800">
        <div className="flex items-center justify-between mb-1">
          <div className="flex items-center gap-2 min-w-0">
            <span className="text-base">
              {DB_EMOJI[connection.database_type] ?? "🔗"}
            </span>
            <div className="min-w-0">
              <div className="text-xs font-semibold text-zinc-200 truncate">
                {connection.database_name}
              </div>
              <div className="text-xxs text-zinc-500 truncate">
                {formatConnectionTarget(connection)}
              </div>
            </div>
          </div>
          <button
            onClick={onDisconnect}
            className="p-1.5 rounded-md text-zinc-500 hover:text-red-400 hover:bg-zinc-800 transition-colors"
            title="Disconnect"
          >
            <LogOut className="w-3.5 h-3.5" />
          </button>
        </div>
        <div className="flex items-center gap-1.5 mt-1">
          <span className="w-2 h-2 rounded-full bg-emerald-500" />
          <span className="text-xxs text-emerald-500 font-medium">
            Connected
          </span>
          <span className="text-xxs text-zinc-600 ml-auto">
            {connection.database_type}
          </span>
        </div>
      </div>

      <div className="border-b border-zinc-800 px-2 py-2 space-y-2">
        <div className="px-1">
          <div className="flex items-center gap-1.5 text-xxs font-semibold text-zinc-500 uppercase tracking-wider">
            <Bookmark className="w-3 h-3" />
            Saved Presets
          </div>
          <p className="mt-1 text-xxs text-zinc-600 leading-relaxed">
            Reuse named queries across this workspace. Scoped presets stay tied to the current database family.
          </p>
        </div>

        {presetMessage && (
          <div className="mx-1 rounded-md border border-zinc-800 bg-surface-300 px-2 py-1.5 text-xxs text-zinc-500">
            {presetMessage}
          </div>
        )}

        {namedQueries.length === 0 ? (
          <div className="mx-1 rounded-md border border-dashed border-zinc-800 px-2 py-2 text-xxs text-zinc-600">
            No saved presets yet. Use Save Preset in the top bar or save a toolkit query below.
          </div>
        ) : (
          <div className="space-y-1">
            {namedQueries.map((preset) => (
              <div
                key={preset.key}
                className="rounded-md border border-zinc-800 bg-surface-300 px-2 py-2"
              >
                <div className="flex items-start gap-2">
                  <button
                    onClick={() => onLoadSnippet(preset.name, preset.query)}
                    className="flex-1 text-left min-w-0"
                  >
                    <div className="text-xs text-zinc-300 truncate">{preset.name}</div>
                    <div className="mt-1 text-xxs text-zinc-600 truncate">
                      {preset.scope}
                    </div>
                  </button>
                  <button
                    onClick={() => handleDeletePreset(preset)}
                    disabled={deletingPreset === preset.key}
                    className="rounded p-1 text-zinc-600 hover:bg-zinc-800 hover:text-zinc-300 disabled:opacity-40"
                    title="Delete preset"
                  >
                    <Trash2 className="w-3 h-3" />
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {hasDjangoToolkit && (
        <div className="border-b border-zinc-800 px-2 py-2 space-y-2">
          <div className="px-1 flex items-start justify-between gap-2">
            <div>
              <div className="flex items-center gap-1.5 text-xxs font-semibold text-zinc-500 uppercase tracking-wider">
                <Code2 className="w-3 h-3" />
                Django Toolkit
              </div>
              <p className="mt-1 text-xxs text-zinc-600 leading-relaxed">
                Schema-aware query packs for Django internals. Load them into the editor or save them as presets.
              </p>
            </div>
            <button
              onClick={() => void handleSaveAllDjangoPresets()}
              disabled={savingPreset !== null}
              className="flex items-center gap-1 rounded-md border border-zinc-800 px-2 py-1 text-xxs text-zinc-400 hover:border-zinc-700 hover:bg-zinc-800 hover:text-zinc-200 disabled:opacity-40"
              title="Save all visible Django presets"
            >
              <BookmarkPlus className="w-3 h-3" />
              Save All
            </button>
          </div>

          {djangoPresetGroups.map((group) => {
            const GroupIcon =
              group.id === "models"
                ? Boxes
                : group.id === "migrations"
                  ? GitBranch
                  : Shield;

            return (
              <div
                key={group.id}
                className="rounded-md border border-zinc-800 bg-surface-300 px-2 py-2"
              >
                <div className="flex items-center gap-1.5 text-xs font-medium text-zinc-300">
                  <GroupIcon className="w-3.5 h-3.5 text-zinc-500" />
                  {group.title}
                </div>
                <p className="mt-1 text-xxs text-zinc-600 leading-relaxed">
                  {group.description}
                </p>
                <div className="mt-2 space-y-1">
                  {group.presets.map((preset) => (
                    <div
                      key={preset.name}
                      className="rounded border border-zinc-800/70 bg-surface px-2 py-2"
                    >
                      <div className="flex items-start gap-2">
                        <button
                          onClick={() => onLoadSnippet(preset.label, preset.query)}
                          className="flex-1 text-left min-w-0"
                        >
                          <div className="text-xs text-zinc-300">{preset.label}</div>
                          <div className="mt-1 text-xxs text-zinc-600 leading-relaxed">
                            {preset.description}
                          </div>
                        </button>
                        <button
                          onClick={() => void handleSavePreset(preset.name, preset.query)}
                          disabled={savingPreset === preset.name}
                          className="rounded p-1 text-zinc-600 hover:bg-zinc-800 hover:text-zinc-300 disabled:opacity-40"
                          title="Save as named preset"
                        >
                          <BookmarkPlus className="w-3 h-3" />
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            );
          })}
        </div>
      )}

      {/* ── Search ───────────────────────────────────────────────────── */}
      <div className="p-2">
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

      {/* ── Tables Header ────────────────────────────────────────────── */}
      <div className="flex items-center justify-between px-3 py-1.5">
        <h3 className="text-xxs font-semibold text-zinc-500 uppercase tracking-wider flex items-center gap-1.5">
          <Table2 className="w-3 h-3" />
          Tables
          <span className="text-zinc-600 font-normal">
            ({filteredTables.length})
          </span>
        </h3>
        <button
          onClick={handleRefresh}
          disabled={refreshing}
          className="p-1 rounded text-zinc-600 hover:text-zinc-400 hover:bg-zinc-800 transition-colors"
          title="Refresh"
        >
          <RefreshCw
            className={`w-3 h-3 ${refreshing ? "animate-spin" : ""}`}
          />
        </button>
      </div>

      {/* ── Table List ────────────────────────────────────────────────── */}
      <div className="flex-1 overflow-y-auto px-1">
        {filteredTables.length === 0 ? (
          <div className="px-3 py-8 text-center text-zinc-600 text-xs">
            {search ? "No matching tables" : "No tables found"}
          </div>
        ) : (
          <div className="space-y-px">
            {filteredTables.map((table) => (
              <div key={table}>
                {/* Table Row */}
                <div className="flex items-center group">
                  <button
                    onClick={() => toggleTable(table)}
                    className="p-1 text-zinc-600 hover:text-zinc-400"
                  >
                    {expandedTable === table ? (
                      <ChevronDown className="w-3.5 h-3.5" />
                    ) : (
                      <ChevronRight className="w-3.5 h-3.5" />
                    )}
                  </button>
                  <button
                    onClick={() => onTableSelect(table)}
                    className="flex-1 text-left px-1 py-1 rounded text-xs text-zinc-300
                      hover:bg-zinc-800 hover:text-zinc-100 transition-colors truncate font-mono"
                    title={`SELECT * FROM ${table} LIMIT 100`}
                  >
                    {table}
                  </button>
                </div>

                {/* Expanded Detail */}
                {expandedTable === table && (
                  <div className="ml-6 mb-1 animate-fade-in">
                    {loadingDetail ? (
                      <div className="flex items-center gap-2 py-2 text-xs text-zinc-500">
                        <Loader2 className="w-3 h-3 animate-spin" />
                        Loading...
                      </div>
                    ) : tableDetail ? (
                      <div className="space-y-0.5 py-1">
                        {tableDetail.columns.map((col) => (
                          <div
                            key={col.name}
                            className="flex items-center gap-2 px-2 py-0.5 rounded text-xxs hover:bg-zinc-800/50"
                          >
                            <Columns3 className="w-3 h-3 text-zinc-600 flex-shrink-0" />
                            <span className="text-zinc-400 font-mono truncate">
                              {col.name}
                            </span>
                            <span className="text-zinc-600 ml-auto truncate text-right">
                              {col.data_type}
                            </span>
                            {!col.nullable && (
                              <span className="text-amber-600 text-xxs">
                                NN
                              </span>
                            )}
                          </div>
                        ))}
                        {tableDetail.indexes.length > 0 && (
                          <div className="mt-1 pt-1 border-t border-zinc-800/50">
                            {tableDetail.indexes.map((idx) => (
                              <div
                                key={idx.name}
                                className="flex items-center gap-2 px-2 py-0.5 text-xxs"
                              >
                                <Key
                                  className={`w-3 h-3 flex-shrink-0 ${idx.is_primary ? "text-amber-500" : "text-zinc-600"}`}
                                />
                                <span className="text-zinc-500 font-mono truncate">
                                  {idx.name}
                                </span>
                              </div>
                            ))}
                          </div>
                        )}
                        {tableDetail.foreign_keys.length > 0 && (
                          <div className="mt-1 pt-1 border-t border-zinc-800/50">
                            {tableDetail.foreign_keys.map((fk) => (
                              <div
                                key={fk.name}
                                className="flex items-center gap-2 px-2 py-0.5 text-xxs"
                              >
                                <Link2 className="w-3 h-3 text-blue-500 flex-shrink-0" />
                                <span className="text-zinc-500 font-mono truncate">
                                  {fk.definition || fk.name}
                                </span>
                              </div>
                            ))}
                          </div>
                        )}
                      </div>
                    ) : (
                      <div className="py-2 text-xs text-zinc-600">
                        Failed to load details
                      </div>
                    )}
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
