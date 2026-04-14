import { useState, useEffect, useRef, useMemo } from "react";
import {
  Database,
  Clock,
  Bookmark,
  Loader2,
  AlertCircle,
  Server,
  ChevronRight,
} from "lucide-react";
import * as cmd from "../commands";
import type {
  RecentConnection,
  SavedSession,
  DatabaseTypeInfo,
} from "../types";
import { VaultWizard } from "./VaultWizard";

interface ConnectionDialogProps {
  onConnectUrl: (url: string) => void;
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

export function ConnectionDialog({
  onConnectUrl,
  onConnectRecent,
  onConnectSession,
  connecting,
  error,
}: ConnectionDialogProps) {
  const [url, setUrl] = useState("");
  const [recentConnections, setRecentConnections] = useState<
    RecentConnection[]
  >([]);
  const [sessions, setSessions] = useState<SavedSession[]>([]);
  const [dbTypes, setDbTypes] = useState<DatabaseTypeInfo[]>([]);
  const [selectedType, setSelectedType] = useState<string | null>(null);
  const [showVaultWizard, setShowVaultWizard] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    cmd.listRecentConnections().then(setRecentConnections).catch(() => {});
    cmd.listSessions().then(setSessions).catch(() => {});
    cmd.getDatabaseTypes().then(setDbTypes).catch(() => {});
  }, []);

  // Add Vault to the type list if not already present
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

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = url.trim();
    if (!trimmed) return;

    // If it's a bare vault:// URL without database+role, open the wizard
    if (trimmed.startsWith("vault://")) {
      const afterScheme = trimmed.slice("vault://".length);
      // Full URL has role@mount/db — check if it's complete
      const hasRole = afterScheme.includes("@");
      const hasDb = afterScheme.includes("/") && afterScheme.split("/").filter(Boolean).length >= 2;
      if (!hasRole || !hasDb) {
        // Incomplete vault URL → open wizard
        setShowVaultWizard(true);
        return;
      }
    }
    onConnectUrl(trimmed);
  };

  const handleTypeSelect = (scheme: string) => {
    if (scheme === "vault") {
      setSelectedType("vault");
      setShowVaultWizard(true);
      return;
    }
    setShowVaultWizard(false);
    setSelectedType(selectedType === scheme ? null : scheme);
    const dt = allTypes.find((t) => t.scheme === scheme);
    if (!url && dt?.placeholder) setUrl(dt.placeholder);
  };

  return (
    <div className="min-h-full bg-surface-300 flex items-center justify-center p-4 animate-fade-in overflow-auto">
      <div className="w-full max-w-2xl">
        {/* ── Header ──────────────────────────────────────────────────── */}
        <div className="text-center mb-10">
          <div className="inline-flex items-center gap-3 mb-4">
            <div className="w-12 h-12 rounded-xl bg-accent/20 flex items-center justify-center">
              <Database className="w-6 h-6 text-accent" />
            </div>
            <h1 className="text-3xl font-bold text-zinc-100 tracking-tight">
              DBCrust
            </h1>
          </div>
          <p className="text-zinc-500 text-sm">
            Connect to PostgreSQL, MySQL, SQLite, MongoDB, ClickHouse,
            Elasticsearch, and more
          </p>
        </div>

        {/* ── Connection Form ─────────────────────────────────────────── */}
        <div className="bg-surface rounded-xl border border-zinc-800 shadow-2xl overflow-hidden">
          {/* Database Type Selector */}
          <div className="px-6 pt-5 pb-3">
            <div className="flex flex-wrap gap-2 mb-4">
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
                initialMount="database"
                onConnect={(vaultUrl) => {
                  setShowVaultWizard(false);
                  onConnectUrl(vaultUrl);
                }}
                onCancel={() => {
                  setShowVaultWizard(false);
                  setSelectedType(null);
                }}
                connecting={connecting}
              />
              {error && (
                <div className="mt-3 flex items-start gap-2 text-red-400 text-xs bg-red-500/10 border border-red-500/20 rounded-lg p-3">
                  <AlertCircle className="w-4 h-4 mt-0.5 flex-shrink-0" />
                  <span className="break-all">{error}</span>
                </div>
              )}
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

              {error && (
                <div className="mt-3 flex items-start gap-2 text-red-400 text-xs bg-red-500/10 border border-red-500/20 rounded-lg p-3">
                  <AlertCircle className="w-4 h-4 mt-0.5 flex-shrink-0" />
                  <span className="break-all">{error}</span>
                </div>
              )}
            </form>
          )}

          {/* ── Recent Connections ──────────────────────────────────────── */}
          {recentConnections.length > 0 && (
            <div className="border-t border-zinc-800 px-6 py-4">
              <h3 className="text-xs font-semibold text-zinc-500 uppercase tracking-wider mb-3 flex items-center gap-2">
                <Clock className="w-3.5 h-3.5" /> Recent Connections
              </h3>
              <div className="space-y-1 max-h-40 overflow-y-auto">
                {recentConnections.slice(0, 8).map((c, i) => (
                  <button
                    key={i}
                    onClick={() => onConnectRecent(i)}
                    disabled={connecting}
                    className="w-full text-left px-3 py-2 rounded-lg text-sm hover:bg-zinc-800
                      transition-colors flex items-center justify-between group disabled:opacity-50"
                  >
                    <div className="flex items-center gap-3 min-w-0">
                      <span className="text-base">
                        {DB_ICONS[c.database_type] ?? "🔗"}
                      </span>
                      <div className="min-w-0">
                        <div className="text-zinc-300 truncate font-medium">
                          {c.display_name}
                        </div>
                        <div className="text-xxs text-zinc-600">
                          {c.timestamp}
                        </div>
                      </div>
                    </div>
                    <ChevronRight className="w-4 h-4 text-zinc-600 opacity-0 group-hover:opacity-100 transition-opacity" />
                  </button>
                ))}
              </div>
            </div>
          )}

          {/* ── Saved Sessions ─────────────────────────────────────────── */}
          {sessions.length > 0 && (
            <div className="border-t border-zinc-800 px-6 py-4">
              <h3 className="text-xs font-semibold text-zinc-500 uppercase tracking-wider mb-3 flex items-center gap-2">
                <Bookmark className="w-3.5 h-3.5" /> Saved Sessions
              </h3>
              <div className="space-y-1 max-h-40 overflow-y-auto">
                {sessions.map((s) => (
                  <button
                    key={s.name}
                    onClick={() => onConnectSession(s.name)}
                    disabled={connecting}
                    className="w-full text-left px-3 py-2 rounded-lg text-sm hover:bg-zinc-800
                      transition-colors flex items-center justify-between group disabled:opacity-50"
                  >
                    <div className="flex items-center gap-3 min-w-0">
                      <Server className="w-4 h-4 text-zinc-500" />
                      <div className="min-w-0">
                        <div className="text-zinc-300 truncate font-medium">
                          {s.name}
                        </div>
                        <div className="text-xxs text-zinc-600">
                          {s.target}
                        </div>
                      </div>
                    </div>
                    <span className="text-xxs text-zinc-600 bg-zinc-800 px-2 py-0.5 rounded">
                      {s.database_type}
                    </span>
                  </button>
                ))}
              </div>
            </div>
          )}
        </div>

        {/* ── Footer ──────────────────────────────────────────────────── */}
        <p className="text-center text-zinc-700 text-xs mt-6">
          Supports SSH tunneling • Docker containers • HashiCorp Vault • File
          formats (Parquet, CSV, JSON)
        </p>
      </div>
    </div>
  );
}
