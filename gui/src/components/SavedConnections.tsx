import { useState, useEffect, useCallback, useMemo } from "react";
import {
  Star,
  Clock,
  Search,
  Trash2,
  ChevronRight,
  Loader2,
  Bookmark,
  RefreshCw,
} from "lucide-react";
import * as cmd from "../commands";
import type { RecentConnection, SavedSession } from "../types";

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
};

interface SavedConnectionsProps {
  onConnectUrl: (url: string) => void;
  onConnectRecent: (index: number) => void;
  onConnectSession: (name: string) => void;
  connecting: boolean;
}

type Section = "saved" | "recent";

export function SavedConnections({
  onConnectRecent,
  onConnectSession,
  connecting,
}: SavedConnectionsProps) {
  const [section, setSection] = useState<Section>("saved");
  const [search, setSearch] = useState("");
  const [sessions, setSessions] = useState<SavedSession[]>([]);
  const [recent, setRecent] = useState<RecentConnection[]>([]);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [s, r] = await Promise.all([
        cmd.listSessions(),
        cmd.listRecentConnections(),
      ]);
      setSessions(s);
      setRecent(r);
      if (s.length === 0 && r.length > 0) setSection("recent");
    } catch {
      /* ignore */
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

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

  const q = search.toLowerCase();

  const filteredSessions = useMemo(
    () =>
      sessions.filter(
        (s) =>
          !q ||
          s.name.toLowerCase().includes(q) ||
          s.target.toLowerCase().includes(q) ||
          s.database_type.toLowerCase().includes(q) ||
          s.dbname.toLowerCase().includes(q),
      ),
    [sessions, q],
  );

  const filteredRecent = useMemo(
    () =>
      recent.filter(
        (c) =>
          !q ||
          c.display_name.toLowerCase().includes(q) ||
          c.database_type.toLowerCase().includes(q),
      ),
    [recent, q],
  );

  return (
    <div className="h-full bg-surface-300 overflow-auto animate-fade-in">
      <div className="max-w-3xl mx-auto p-8">
        {/* Header */}
        <div className="flex items-center justify-between mb-6">
          <div>
            <h1 className="text-2xl font-bold text-zinc-100 flex items-center gap-3">
              <div className="w-10 h-10 rounded-xl bg-amber-500/10 flex items-center justify-center">
                <Star className="w-5 h-5 text-amber-400" />
              </div>
              Saved Connections
            </h1>
            <p className="text-sm text-zinc-500 mt-2 ml-[52px]">
              Quickly reconnect to your databases. Search by name, host, or
              type.
            </p>
          </div>
          <button
            onClick={refresh}
            disabled={loading}
            className="flex items-center gap-2 px-3 py-2 rounded-lg text-sm font-medium
              bg-zinc-800 hover:bg-zinc-700 text-zinc-400 disabled:opacity-50 transition-all"
          >
            <RefreshCw
              className={`w-3.5 h-3.5 ${loading ? "animate-spin" : ""}`}
            />
            Refresh
          </button>
        </div>

        {/* Search */}
        <div className="relative mb-5">
          <Search className="w-4 h-4 absolute left-3.5 top-1/2 -translate-y-1/2 text-zinc-500" />
          <input
            type="text"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search connections…"
            className="w-full bg-surface border border-zinc-800 rounded-xl pl-10 pr-4 py-3
              text-sm text-zinc-200 placeholder-zinc-600
              focus:outline-none focus:border-accent focus:ring-1 focus:ring-accent/30 transition-all"
            autoFocus
          />
        </div>

        {/* Section toggle */}
        <div className="flex gap-1 mb-5 bg-surface rounded-lg p-1 border border-zinc-800">
          <button
            onClick={() => setSection("saved")}
            className={`flex-1 flex items-center justify-center gap-2 px-4 py-2 rounded-md text-sm font-medium transition-all
              ${
                section === "saved"
                  ? "bg-accent/10 text-accent"
                  : "text-zinc-500 hover:text-zinc-300"
              }`}
          >
            <Star className="w-3.5 h-3.5" />
            Saved Sessions
            {sessions.length > 0 && (
              <span className="text-xxs bg-zinc-800 text-zinc-400 px-1.5 py-0.5 rounded-full">
                {sessions.length}
              </span>
            )}
          </button>
          <button
            onClick={() => setSection("recent")}
            className={`flex-1 flex items-center justify-center gap-2 px-4 py-2 rounded-md text-sm font-medium transition-all
              ${
                section === "recent"
                  ? "bg-accent/10 text-accent"
                  : "text-zinc-500 hover:text-zinc-300"
              }`}
          >
            <Clock className="w-3.5 h-3.5" />
            Recent
            {recent.length > 0 && (
              <span className="text-xxs bg-zinc-800 text-zinc-400 px-1.5 py-0.5 rounded-full">
                {recent.length}
              </span>
            )}
          </button>
        </div>

        {/* Loading */}
        {loading && (
          <div className="flex items-center justify-center py-16 text-zinc-500">
            <Loader2 className="w-5 h-5 animate-spin mr-2" />
            Loading…
          </div>
        )}

        {/* Saved Sessions */}
        {!loading && section === "saved" && (
          <div>
            {filteredSessions.length === 0 ? (
              <div className="bg-surface rounded-xl border border-zinc-800 p-10 text-center">
                <Bookmark className="w-10 h-10 mx-auto mb-4 text-zinc-700" />
                <h3 className="text-sm font-medium text-zinc-300 mb-2">
                  {search ? "No matching sessions" : "No saved sessions"}
                </h3>
                <p className="text-xs text-zinc-600 max-w-sm mx-auto">
                  {!search &&
                    "Connect to a database, then save the session from the sidebar to see it here."}
                </p>
              </div>
            ) : (
              <div className="space-y-2">
                {filteredSessions.map((s) => (
                  <button
                    key={s.name}
                    onClick={() => onConnectSession(s.name)}
                    disabled={connecting}
                    className="w-full text-left bg-surface rounded-xl border border-zinc-800
                      hover:border-zinc-700 hover:bg-surface-100 transition-all
                      flex items-center gap-4 p-4 group disabled:opacity-50"
                  >
                    <div className="w-12 h-12 rounded-xl bg-zinc-800 flex items-center justify-center text-xl flex-shrink-0">
                      {DB_ICONS[s.database_type] ?? "🔗"}
                    </div>
                    <div className="flex-1 min-w-0">
                      <div className="text-sm font-semibold text-zinc-100 truncate">
                        {s.name}
                      </div>
                      <div className="text-xs text-zinc-500 truncate mt-0.5">
                        {s.target}
                      </div>
                      <div className="flex items-center gap-2 mt-1.5 flex-wrap">
                        <span className="text-xxs bg-zinc-800 text-zinc-400 px-2 py-0.5 rounded-md font-medium">
                          {s.database_type}
                        </span>
                        {s.user && (
                          <span className="text-xxs text-zinc-600">
                            {s.user}@
                          </span>
                        )}
                        {s.host &&
                          !s.host.startsWith("DOCKER:") && (
                            <span className="text-xxs text-zinc-600 font-mono">
                              {s.host}:{s.port}
                            </span>
                          )}
                        {s.dbname && (
                          <span className="text-xxs text-zinc-600 font-mono">
                            /{s.dbname}
                          </span>
                        )}
                      </div>
                    </div>
                    <div className="flex items-center gap-1.5 flex-shrink-0">
                      <span
                        role="button"
                        onClick={(e) => handleDeleteSession(s.name, e)}
                        className="p-2 rounded-lg text-zinc-700 hover:text-red-400 hover:bg-red-500/10
                          opacity-0 group-hover:opacity-100 transition-all"
                        title="Delete session"
                      >
                        <Trash2 className="w-4 h-4" />
                      </span>
                      {connecting ? (
                        <Loader2 className="w-4 h-4 animate-spin text-zinc-500" />
                      ) : (
                        <ChevronRight className="w-5 h-5 text-zinc-700 group-hover:text-zinc-400 transition-colors" />
                      )}
                    </div>
                  </button>
                ))}
              </div>
            )}
          </div>
        )}

        {/* Recent Connections */}
        {!loading && section === "recent" && (
          <div>
            {filteredRecent.length === 0 ? (
              <div className="bg-surface rounded-xl border border-zinc-800 p-10 text-center">
                <Clock className="w-10 h-10 mx-auto mb-4 text-zinc-700" />
                <h3 className="text-sm font-medium text-zinc-300 mb-2">
                  {search
                    ? "No matching connections"
                    : "No recent connections"}
                </h3>
              </div>
            ) : (
              <div className="space-y-2">
                {filteredRecent.map((c, i) => (
                  <button
                    key={`${c.display_name}-${i}`}
                    onClick={() => onConnectRecent(i)}
                    disabled={connecting}
                    className="w-full text-left bg-surface rounded-xl border border-zinc-800
                      hover:border-zinc-700 hover:bg-surface-100 transition-all
                      flex items-center gap-4 p-4 group disabled:opacity-50"
                  >
                    <div className="w-12 h-12 rounded-xl bg-zinc-800 flex items-center justify-center text-xl flex-shrink-0">
                      {DB_ICONS[c.database_type] ?? "🔗"}
                    </div>
                    <div className="flex-1 min-w-0">
                      <div className="text-sm font-medium text-zinc-200 truncate">
                        {c.display_name}
                      </div>
                      <div className="flex items-center gap-2 mt-1.5">
                        <span className="text-xxs bg-zinc-800 text-zinc-400 px-2 py-0.5 rounded-md font-medium">
                          {c.database_type}
                        </span>
                        <span className="text-xxs text-zinc-600">
                          {c.timestamp}
                        </span>
                        {!c.success && (
                          <span className="text-xxs text-red-400 bg-red-500/10 px-1.5 py-0.5 rounded-md">
                            failed
                          </span>
                        )}
                      </div>
                    </div>
                    {connecting ? (
                      <Loader2 className="w-4 h-4 animate-spin text-zinc-500 flex-shrink-0" />
                    ) : (
                      <ChevronRight className="w-5 h-5 text-zinc-700 group-hover:text-zinc-400 transition-colors flex-shrink-0" />
                    )}
                  </button>
                ))}
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
