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
  shortLabel: string;
}

function NavButton({
  active,
  label,
  shortLabel,
  onClick,
  children,
  title,
}: {
  active: boolean;
  label: string;
  shortLabel: string;
  onClick: () => void;
  children: React.ReactNode;
  title?: string;
}) {
  return (
    <button
      onClick={onClick}
      className={`nav-item relative w-full rounded-xl px-2 py-2 transition-all
        ${
          active
            ? "bg-accent/10 text-accent"
            : "text-zinc-500 hover:text-zinc-200 hover:bg-zinc-800/50"
        }`}
      title={title ?? label}
    >
      {active && (
        <span className="absolute left-0 top-2 bottom-2 w-0.5 bg-accent rounded-r" />
      )}
      <div className="flex flex-col items-center gap-1.5 text-center">
        {children}
        <span className="text-[10px] font-medium leading-none">{shortLabel}</span>
      </div>
    </button>
  );
}

export function Navigation({
  connected,
  activeView,
  onViewChange,
  onDisconnect,
  connectionType,
  tables,
}: NavigationProps) {
  const hasDjango = tables ? getVisibleDjangoPresetGroups(tables).length > 0 : false;

  const connectionItems: NavItem[] = [
    { view: "home", icon: Plug, label: "New Connection", shortLabel: "Connect" },
    { view: "saved", icon: Star, label: "Saved Connections", shortLabel: "Saved" },
    { view: "docker", icon: Boxes, label: "Docker Discovery", shortLabel: "Docker" },
  ];

  const databaseItems: NavItem[] = [
    { view: "query", icon: Code2, label: "Query Workspace", shortLabel: "Query" },
    { view: "schema", icon: Table2, label: "Schema Explorer", shortLabel: "Schema" },
    { view: "settings", icon: Settings, label: "Settings", shortLabel: "Settings" },
  ];

  return (
    <nav className="nav-rail w-20 h-full bg-surface-300 border-r border-zinc-800/50 flex flex-col items-center py-3 px-2 gap-3 flex-shrink-0">
      <div className="w-full space-y-1">
        <div className="px-2 text-[9px] text-zinc-600 uppercase tracking-[0.24em] font-semibold select-none">
          Connect
        </div>
        {connectionItems.map(({ view, icon: Icon, label, shortLabel }) => (
          <NavButton
            key={view}
            active={activeView === view}
            label={label}
            shortLabel={shortLabel}
            onClick={() => onViewChange(view)}
          >
            <Icon className="w-[18px] h-[18px]" />
          </NavButton>
        ))}
      </div>

      {connected && <div className="w-10 border-t border-zinc-700/50" />}

      {connected && (
        <div className="w-full space-y-1">
          <div className="px-2 text-[9px] text-zinc-600 uppercase tracking-[0.24em] font-semibold select-none">
            Workspace
          </div>

          <div
            className="mx-1 mb-1 rounded-xl border border-zinc-800 bg-surface-200 px-2 py-2 text-center"
            title={connectionType ?? "Database"}
          >
            <div className="text-base leading-none">
              {connectionType ? (DB_EMOJI[connectionType] ?? "🔗") : "🔗"}
            </div>
            <div className="mt-1 text-[10px] text-zinc-500 truncate">
              {connectionType ?? "Database"}
            </div>
          </div>

          {databaseItems.map(({ view, icon: Icon, label, shortLabel }) => (
            <NavButton
              key={view}
              active={activeView === view}
              label={label}
              shortLabel={shortLabel}
              onClick={() => onViewChange(view)}
              title={
                view === "query" && hasDjango
                  ? "Query Workspace (Django toolkit available)"
                  : label
              }
            >
              <div className="relative">
                <Icon className="w-[18px] h-[18px]" />
                {view === "query" && hasDjango && (
                  <span
                    className="absolute -top-1 -right-1 w-3.5 h-3.5 rounded-full bg-emerald-600 border border-surface-300 flex items-center justify-center text-[7px] text-white font-bold"
                    title="Django toolkit available"
                  >
                    D
                  </span>
                )}
              </div>
            </NavButton>
          ))}
        </div>
      )}

      <div className="flex-1" />

      {connected && (
        <button
          onClick={onDisconnect}
          className="w-full rounded-xl px-2 py-2 text-zinc-600 hover:text-red-400 hover:bg-red-500/10 transition-all"
          title="Disconnect from database"
        >
          <div className="flex flex-col items-center gap-1 text-center">
            <div className="relative">
              <LogOut className="w-4 h-4" />
              <span className="absolute -top-0.5 -right-0.5 w-2 h-2 rounded-full bg-emerald-500 border border-surface-300" />
            </div>
            <span className="text-[10px] font-medium leading-none">Disconnect</span>
          </div>
        </button>
      )}
    </nav>
  );
}
