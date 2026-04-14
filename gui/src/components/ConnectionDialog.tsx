import { useState, useEffect, useRef, useMemo, useCallback } from "react";
import {
  Database,
  Clock,
  Bookmark,
  Loader2,
  AlertCircle,
  ChevronRight,
  Search,
  Trash2,
  Star,
  Plus,
} from "lucide-react";
import * as cmd from "../commands";
import type {
  RecentConnection,
  SavedSession,
  DatabaseTypeInfo,
} from "../types";
import { VaultWizard } from "./VaultWizard";

interface ConnectionDialogProps {
  onConnectUrl: (url: string, options?: { vaultAddr?: string }) => void;
  onConnectRecent: (index: number) => void;
  onConnectSession: (name: string) => void;
  connecting: boolean;
  error: string | null;
}

const DB_ICONS: Record<string, string> = {
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
  Docker: "🐳",
  Vault: "🔐",
};

type Tab = "new" | "saved" | "recent";

function getVaultWizardMount(url: string): string {
  const trimmed = url.trim();
  if (!trimmed.startsWith("vault://")) return "database";

  const withoutScheme = trimmed.slice("vault://".length);
  const beforePath = withoutScheme.split("/")[0] ?? "";
  const mountPath = beforePath.includes("@")
    ? beforePath.split("@")[1]
    : beforePath;

  return mountPath?.trim() || "database";
}

export function ConnectionDialog({
  onConnectUrl,
  onConnectRecent,
  onConnectSession,
  connecting,
  error,
}: ConnectionDialogProps) {
  const [tab, setTab] = useState<Tab>("new");
  const [url, setUrl] = useState("");
  const [search, setSearch] = useState("");
  const [recentConnections, setRecentConnections] = useState<RecentConnection[]>([]);
  const [sessions, setSessions] = useState<SavedSession[]>([]);
  const [dbTypes, setDbTypes] = useState<DatabaseTypeInfo[]>([]);
  const [selectedType, setSelectedType] = useState<string | null>(null);
  const [showVaultWizard, setShowVaultWizard] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    cmd.listRecentConnections().then(setRecentConnections).catch(() => {});
    cmd.listSessions().then(setSessions).catch(() => {});
    cmd.getDatabaseTypes().then(setDbTypes).catch(() => {});
  }, []);

  // Focus the right input when switching tabs
  useEffect(() => {
    if (tab === "new") inputRef.current?.focus();
    if (tab === "saved" || tab === "recent") searchRef.current?.focus();
  }, [tab]);

  const allTypes = useMemo(() => {
    const types = [...dbTypes];
    if (!types.some((t) => t.name === "Vault")) {
      types.push({
        name: "Vault",
        scheme: "vault",
        default_port: null,
        placeholder: "vault://role@database/db_name",
      });
    }
    return types;
  }, [dbTypes]);

  const placeholder =
    allTypes.find((t) => t.scheme === selectedType)?.placeholder ??
    "postgres://user:pass@localhost:5432/mydb";

  // Filtered lists
  const filteredSessions = useMemo(
    () =>
      sessions.filter(
        (s) =>
          !search ||
          s.name.toLowerCase().includes(search.toLowerCase()) ||
          s.target.toLowerCase().includes(search.toLowerCase()) ||
          s.database_type.toLowerCase().includes(search.toLowerCase()),
      ),
    [sessions, search],
  );

  const filteredRecent = useMemo(
    () =>
      recentConnections.filter(
        (c) =>
          !search ||
          c.display_name.toLowerCase().includes(search.toLowerCase()) ||
          c.database_type.toLowerCase().includes(search.toLowerCase()),
      ),
    [recentConnections, search],
  );

  const handleSubmit = useCallback(
    (e: React.FormEvent) => {
      e.preventDefault();
      const trimmed = url.trim();
      if (!trimmed) return;
      if (trimmed.startsWith("vault://")) {
        const afterScheme = trimmed.slice("vault://".length);
        const hasRole = afterScheme.includes("@");
        const hasDb =
          afterScheme.includes("/") &&
          afterScheme.split("/").filter(Boolean).length >= 2;
        if (!hasRole || !hasDb) {
          setShowVaultWizard(true);
          return;
        }
      }
      onConnectUrl(trimmed);
    },
    [url, onConnectUrl],
  );

  const handleTypeSelect = useCallback(
    (scheme: string) => {
      if (scheme === "vault") {
        setSelectedType("vault");
        setShowVaultWizard(true);
        return;
      }
      setShowVaultWizard(false);
      setSelectedType(selectedType === scheme ? null : scheme);
      const dt = allTypes.find((t) => t.scheme === scheme);
      if (!url && dt?.placeholder) setUrl(dt.placeholder);
    },
    [selectedType, url, allTypes],
  );

  const handleDeleteSession = useCallback(
    async (name: string, e: React.MouseEvent) => {
      e.stopPropagation();
      if (!window.confirm(`Delete saved session "${name}"?`)) return;
      try {
        await cmd.deleteSession(name);
        setSessions((prev) => prev.filter((s) => s.name !== name));
      } catch {
        /* ignore */
      }
    },
    [],
  );

  // Auto-select tab with saved sessions if they exist and no recent
  useEffect(() => {
    if (sessions.length > 0 && recentConnections.length === 0) {
      setTab("saved");
    }
  }, [sessions, recentConnections]);

  return (
    <div className="min-h-full bg-surface-300 flex items-center justify-center p-4 animate-fade-in overflow-auto">
      <div className="w-full max-w-2xl">
        {/* ── Header ──────────────────────────────────────────────────── */}
        <div className="text-center mb-8">
          <div className="inline-flex items-center gap-3 mb-3">
            <div className="w-11 h-11 rounded-xl bg-accent/20 flex items-center justify-center">
              <Database className="w-5 h-5 text-accent" />
            </div>
            <h1 className="text-2xl font-bold text-zinc-100 tracking-tight">
              DBCrust
            </h1>
          </div>
          <p className="text-zinc-500 text-sm">
            Connect to PostgreSQL, MySQL, SQLite, MongoDB, ClickHouse,
            Elasticsearch &amp; more
          </p>
        </div>

        {/* ── Main Card ───────────────────────────────────────────────── */}
        <div className="bg-surface rounded-xl border border-zinc-800 shadow-2xl overflow-hidden">
          {/* ── Tabs ──────────────────────────────────────────────────── */}
          <div className="flex border-b border-zinc-800">
            {(
              [
                { id: "new" as const, icon: Plus, label: "New Connection" },
                {
                  id: "saved" as const,
                  icon: Star,
                  label: `Saved${sessions.length ? ` (${sessions.length})` : ""}`,
                },
                {
                  id: "recent" as const,
                  icon: Clock,
                  label: `Recent${recentConnections.length ? ` (${recentConnections.length})` : ""}`,
                },
              ] as const
            ).map(({ id, icon: Icon, label }) => (
              <button
                key={id}
                onClick={() => {
                  setTab(id);
                  setSearch("");
                }}
                className={`flex-1 flex items-center justify-center gap-2 px-4 py-3 text-xs font-medium transition-all
                  ${
                    tab === id
                      ? "text-accent border-b-2 border-accent bg-accent/5"
                      : "text-zinc-500 hover:text-zinc-300 hover:bg-zinc-800/30"
                  }`}
              >
                <Icon className="w-3.5 h-3.5" />
                {label}
              </button>
            ))}
          </div>

          {/* ── Error Banner ──────────────────────────────────────────── */}
          {error && (
            <div className="mx-6 mt-4 flex items-start gap-2 text-red-400 text-xs bg-red-500/10 border border-red-500/20 rounded-lg p-3">
              <AlertCircle className="w-4 h-4 mt-0.5 flex-shrink-0" />
              <span className="break-all">{error}</span>
            </div>
          )}

          {/* ── Tab: New Connection ─────────────────────────────────── */}
          {tab === "new" && (
            <div className="animate-fade-in">
              {/* Type selector */}
              <div className="px-6 pt-5 pb-3">
                <div className="flex flex-wrap gap-2">
                  {allTypes.map((dt) => (
                    <button
                      key={dt.scheme}
                      onClick={() => handleTypeSelect(dt.scheme)}
                      className={`px-3 py-1.5 rounded-lg text-xs font-medium transition-all ${
                        selectedType === dt.scheme
                          ? "bg-accent text-white"
                          : "bg-zinc-800 text-zinc-400 hover:bg-zinc-700 hover:text-zinc-300"
                      }`}
                    >
                      <span className="mr-1.5">
                        {DB_ICONS[dt.name] ?? "🔗"}
                      </span>
                      {dt.name}
                    </button>
                  ))}
                </div>
              </div>

              {/* Vault Wizard OR URL Input */}
              {showVaultWizard ? (
                <div className="px-6 pb-5">
                  <VaultWizard
                    initialMount={getVaultWizardMount(url)}
                    onConnect={(vaultUrl, nextVaultAddr) => {
                      setShowVaultWizard(false);
                      onConnectUrl(vaultUrl, { vaultAddr: nextVaultAddr });
                    }}
                    onCancel={() => {
                      setShowVaultWizard(false);
                      setSelectedType(null);
                    }}
                    connecting={connecting}
                  />
                </div>
              ) : (
                <form onSubmit={handleSubmit} className="px-6 pb-5">
                  <div className="relative">
                    <input
                      ref={inputRef}
                      type="text"
                      value={url}
                      onChange={(e) => setUrl(e.target.value)}
                      placeholder={placeholder}
                      disabled={connecting}
                      className="w-full bg-surface-300 border border-zinc-700 rounded-lg px-4 py-3 text-zinc-100
                        placeholder-zinc-600 font-mono text-sm focus:outline-none focus:border-accent
                        focus:ring-1 focus:ring-accent/50 disabled:opacity-50 transition-all"
                      autoComplete="off"
                      spellCheck={false}
                    />
                    <button
                      type="submit"
                      disabled={connecting || !url.trim()}
                      className="absolute right-2 top-1/2 -translate-y-1/2 px-4 py-1.5 rounded-md text-sm font-medium
                        bg-accent hover:bg-accent-hover text-white disabled:opacity-40
                        disabled:cursor-not-allowed transition-all flex items-center gap-2"
                    >
                      {connecting ? (
                        <Loader2 className="w-4 h-4 animate-spin" />
                      ) : (
                        <ChevronRight className="w-4 h-4" />
                      )}
                      Connect
                    </button>
                  </div>
                </form>
              )}
            </div>
          )}

          {/* ── Tab: Saved Sessions ────────────────────────────────── */}
          {tab === "saved" && (
            <div className="animate-fade-in">
              {/* Search */}
              <div className="px-6 pt-4 pb-2">
                <div className="relative">
                  <Search className="w-3.5 h-3.5 absolute left-3 top-1/2 -translate-y-1/2 text-zinc-500" />
                  <input
                    ref={searchRef}
                    type="text"
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                    placeholder="Search saved sessions…"
                    className="w-full bg-surface-300 border border-zinc-700 rounded-lg pl-9 pr-3 py-2.5
                      text-sm text-zinc-200 placeholder-zinc-600
                      focus:outline-none focus:border-accent transition-colors"
                  />
                </div>
              </div>

              {/* Session list */}
              <div className="px-4 pb-4 max-h-[400px] overflow-y-auto">
                {filteredSessions.length === 0 ? (
                  <div className="text-center py-10 text-zinc-600">
                    <Bookmark className="w-8 h-8 mx-auto mb-3 text-zinc-700" />
                    <p className="text-sm">
                      {search
                        ? "No matching sessions"
                        : "No saved sessions yet"}
                    </p>
                    <p className="text-xs text-zinc-700 mt-1">
                      {!search &&
                        "Save a session after connecting to quickly reconnect later"}
                    </p>
                  </div>
                ) : (
                  <div className="space-y-1.5">
                    {filteredSessions.map((s) => (
                      <button
                        key={s.name}
                        onClick={() => onConnectSession(s.name)}
                        disabled={connecting}
                        className="w-full text-left px-4 py-3 rounded-lg hover:bg-zinc-800
                          transition-colors flex items-center gap-3 group disabled:opacity-50
                          border border-transparent hover:border-zinc-700"
                      >
                        <div className="w-10 h-10 rounded-lg bg-zinc-800 flex items-center justify-center text-lg flex-shrink-0">
                          {DB_ICONS[s.database_type] ?? "🔗"}
                        </div>
                        <div className="flex-1 min-w-0">
                          <div className="text-sm text-zinc-200 font-semibold truncate">
                            {s.name}
                          </div>
                          <div className="text-xxs text-zinc-500 truncate mt-0.5">
                            {s.target}
                          </div>
                          <div className="flex items-center gap-2 mt-1">
                            <span className="text-xxs text-zinc-600 bg-zinc-800/80 px-1.5 py-0.5 rounded">
                              {s.database_type}
                            </span>
                            {s.host && !s.host.startsWith("DOCKER:") && (
                              <span className="text-xxs text-zinc-600">
                                {s.host}:{s.port}
                              </span>
                            )}
                            {s.dbname && (
                              <span className="text-xxs text-zinc-600">
                                / {s.dbname}
                              </span>
                            )}
                          </div>
                        </div>
                        <div className="flex items-center gap-1 flex-shrink-0">
                          <span
                            role="button"
                            onClick={(e) => handleDeleteSession(s.name, e)}
                            className="p-1.5 rounded-md text-zinc-700 hover:text-red-400 hover:bg-red-500/10 opacity-0 group-hover:opacity-100 transition-all"
                            title="Delete session"
                          >
                            <Trash2 className="w-3.5 h-3.5" />
                          </span>
                          <ChevronRight className="w-4 h-4 text-zinc-700 group-hover:text-zinc-400 transition-colors" />
                        </div>
                      </button>
                    ))}
                  </div>
                )}
              </div>
            </div>
          )}

          {/* ── Tab: Recent Connections ─────────────────────────────── */}
          {tab === "recent" && (
            <div className="animate-fade-in">
              {/* Search */}
              <div className="px-6 pt-4 pb-2">
                <div className="relative">
                  <Search className="w-3.5 h-3.5 absolute left-3 top-1/2 -translate-y-1/2 text-zinc-500" />
                  <input
                    ref={searchRef}
                    type="text"
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                    placeholder="Search recent connections…"
                    className="w-full bg-surface-300 border border-zinc-700 rounded-lg pl-9 pr-3 py-2.5
                      text-sm text-zinc-200 placeholder-zinc-600
                      focus:outline-none focus:border-accent transition-colors"
                  />
                </div>
              </div>

              {/* Recent list */}
              <div className="px-4 pb-4 max-h-[400px] overflow-y-auto">
                {filteredRecent.length === 0 ? (
                  <div className="text-center py-10 text-zinc-600">
                    <Clock className="w-8 h-8 mx-auto mb-3 text-zinc-700" />
                    <p className="text-sm">
                      {search
                        ? "No matching connections"
                        : "No recent connections"}
                    </p>
                  </div>
                ) : (
                  <div className="space-y-1.5">
                    {filteredRecent.map((c, i) => (
                      <button
                        key={`${c.display_name}-${i}`}
                        onClick={() => onConnectRecent(i)}
                        disabled={connecting}
                        className="w-full text-left px-4 py-3 rounded-lg hover:bg-zinc-800
                          transition-colors flex items-center gap-3 group disabled:opacity-50
                          border border-transparent hover:border-zinc-700"
                      >
                        <div className="w-10 h-10 rounded-lg bg-zinc-800 flex items-center justify-center text-lg flex-shrink-0">
                          {DB_ICONS[c.database_type] ?? "🔗"}
                        </div>
                        <div className="flex-1 min-w-0">
                          <div className="text-sm text-zinc-200 font-medium truncate">
                            {c.display_name}
                          </div>
                          <div className="flex items-center gap-2 mt-1">
                            <span className="text-xxs text-zinc-600 bg-zinc-800/80 px-1.5 py-0.5 rounded">
                              {c.database_type}
                            </span>
                            <span className="text-xxs text-zinc-600">
                              {c.timestamp}
                            </span>
                            {!c.success && (
                              <span className="text-xxs text-red-500 bg-red-500/10 px-1.5 py-0.5 rounded">
                                failed
                              </span>
                            )}
                          </div>
                        </div>
                        <ChevronRight className="w-4 h-4 text-zinc-700 group-hover:text-zinc-400 transition-colors flex-shrink-0" />
                      </button>
                    ))}
                  </div>
                )}
              </div>
            </div>
          )}
        </div>

        {/* ── Footer ──────────────────────────────────────────────────── */}
        <p className="text-center text-zinc-700 text-xs mt-6">
          SSH tunneling · Docker · HashiCorp Vault · Parquet, CSV, JSON
        </p>
      </div>
    </div>
  );
}
