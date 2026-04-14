import {
  Code2,
  Table2,
  Boxes,
  Settings,
  LogOut,
  Plug,
  Star,
} from "lucide-react";
import type { NavigationView } from "../types";
import { getVisibleDjangoPresetGroups } from "../queryPresets";

interface NavigationProps {
  connected: boolean;
  activeView: NavigationView;
  onViewChange: (view: NavigationView) => void;
  onDisconnect: () => void;
  connectionType?: string;
  tables?: string[];
}

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

interface NavItem {
  view: NavigationView;
  icon: React.ComponentType<{ className?: string }>;
  label: string;
}

export function Navigation({
  connected,
  activeView,
  onViewChange,
  onDisconnect,
  connectionType,
  tables,
}: NavigationProps) {
  // Detect Django framework
  const hasDjango =
    tables ? getVisibleDjangoPresetGroups(tables).length > 0 : false;

  const connectionItems: NavItem[] = [
    { view: "home", icon: Plug, label: "New Connection" },
    { view: "saved", icon: Star, label: "Saved Connections" },
    { view: "docker", icon: Boxes, label: "Docker Discovery" },
  ];

  const databaseItems: NavItem[] = [
    { view: "query", icon: Code2, label: "Query Editor" },
    { view: "schema", icon: Table2, label: "Schema Explorer" },
    { view: "settings", icon: Settings, label: "Settings" },
  ];

  return (
    <nav className="nav-rail w-12 h-full bg-surface-300 border-r border-zinc-800/50 flex flex-col items-center py-2 flex-shrink-0">
      {/* Connection group */}
      <div className="flex flex-col items-center gap-1 w-full px-1">
        <span className="text-[8px] text-zinc-600 uppercase tracking-widest font-semibold mb-0.5 select-none">
          Link
        </span>
        {connectionItems.map(({ view, icon: Icon, label }) => (
          <button
            key={view}
            onClick={() => onViewChange(view)}
            className={`nav-item relative w-10 h-10 flex items-center justify-center rounded-lg transition-all
              ${
                activeView === view
                  ? "text-accent bg-accent/10"
                  : "text-zinc-500 hover:text-zinc-300 hover:bg-zinc-800/50"
              }`}
            title={label}
          >
            {activeView === view && (
              <span className="absolute left-0 top-2 bottom-2 w-0.5 bg-accent rounded-r" />
            )}
            <Icon className="w-[18px] h-[18px]" />
          </button>
        ))}
      </div>

      {/* Separator */}
      {connected && <div className="w-6 border-t border-zinc-700/50 my-2" />}

      {/* Database group (only when connected) */}
      {connected && (
        <div className="flex flex-col items-center gap-1 w-full px-1">
          {/* DB type badge */}
          <div
            className="w-10 h-6 flex items-center justify-center text-sm select-none"
            title={connectionType ?? "Database"}
          >
            {connectionType ? (DB_EMOJI[connectionType] ?? "🔗") : "🔗"}
          </div>

          {databaseItems.map(({ view, icon: Icon, label }) => (
            <button
              key={view}
              onClick={() => onViewChange(view)}
              className={`nav-item relative w-10 h-10 flex items-center justify-center rounded-lg transition-all
                ${
                  activeView === view
                    ? "text-accent bg-accent/10"
                    : "text-zinc-500 hover:text-zinc-300 hover:bg-zinc-800/50"
                }`}
              title={
                view === "query" && hasDjango
                  ? "Query Editor (Django toolkit available)"
                  : label
              }
            >
              {activeView === view && (
                <span className="absolute left-0 top-2 bottom-2 w-0.5 bg-accent rounded-r" />
              )}
              <Icon className="w-[18px] h-[18px]" />
              {/* Django indicator */}
              {view === "query" && hasDjango && (
                <span
                  className="absolute -top-0.5 -right-0.5 w-3.5 h-3.5 rounded-full bg-emerald-600 border border-surface-300 flex items-center justify-center text-[7px] text-white font-bold"
                  title="Django toolkit available"
                >
                  D
                </span>
              )}
            </button>
          ))}
        </div>
      )}

      <div className="flex-1" />

      {/* Disconnect */}
      {connected && (
        <button
          onClick={() => {
            if (window.confirm("Disconnect from the database?")) {
              onDisconnect();
            }
          }}
          className="group w-10 h-10 flex items-center justify-center rounded-lg text-zinc-600 hover:text-red-400 hover:bg-red-500/10 transition-all relative"
          title="Disconnect from database"
        >
          <LogOut className="w-4 h-4" />
          <span className="absolute -top-0.5 -right-0.5 w-2 h-2 rounded-full bg-emerald-500 border border-surface-300" />
        </button>
      )}
    </nav>
  );
}
