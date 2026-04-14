import {
  Clock,
  Rows3,
  Zap,
  Home,
  Code2,
  Table2,
  Boxes,
  Settings,
} from "lucide-react";
import type { ConnectionState, EditorTab, NavigationView } from "../types";
import { formatConnectionTarget } from "../connectionDisplay";

interface StatusBarProps {
  connection: ConnectionState;
  activeTab: EditorTab;
  currentView?: NavigationView;
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

const VIEW_LABELS: Record<
  NavigationView,
  { icon: React.ComponentType<{ className?: string }>; label: string }
> = {
  home: { icon: Home, label: "Dashboard" },
  query: { icon: Code2, label: "Query Editor" },
  schema: { icon: Table2, label: "Schema Explorer" },
  docker: { icon: Boxes, label: "Docker Discovery" },
  settings: { icon: Settings, label: "Settings" },
};

export function StatusBar({
  connection,
  activeTab,
  currentView,
}: StatusBarProps) {
  const viewInfo = currentView ? VIEW_LABELS[currentView] : null;
  const ViewIcon = viewInfo?.icon;

  return (
    <div className="h-8 flex items-center justify-between px-3 bg-surface-200 border-t border-zinc-800 text-xs select-none flex-shrink-0">
      {/* ── Left ──────────────────────────────────────────────────────── */}
      <div className="flex items-center gap-3">
        <div className="flex items-center gap-1.5 text-zinc-400">
          <span className="w-2 h-2 rounded-full bg-emerald-500" />
          <span className="text-sm leading-none">
            {DB_EMOJI[connection.database_type] ?? "🔗"}
          </span>
          <span className="font-medium">{connection.database_type}</span>
          <span className="text-zinc-600">·</span>
          <span className="font-medium">{connection.database_name}</span>
        </div>
        <div className="text-zinc-600 hidden sm:block">
          {formatConnectionTarget(connection)}
        </div>
        {viewInfo && ViewIcon && (
          <div className="flex items-center gap-1 text-zinc-500 border-l border-zinc-700 pl-3">
            <ViewIcon className="w-3.5 h-3.5" />
            <span className="font-medium">{viewInfo.label}</span>
          </div>
        )}
      </div>

      {/* ── Right ─────────────────────────────────────────────────────── */}
      <div className="flex items-center gap-3">
        {activeTab.isRunning && (
          <div className="flex items-center gap-1.5 text-amber-500 font-medium">
            <Zap className="w-3.5 h-3.5" />
            <span>Executing…</span>
          </div>
        )}
        {activeTab.results && !activeTab.isRunning && (
          <>
            <div className="flex items-center gap-1.5 text-zinc-400">
              <Rows3 className="w-3.5 h-3.5" />
              <span>
                {activeTab.results.row_count} row
                {activeTab.results.row_count !== 1 ? "s" : ""}
              </span>
            </div>
            <div className="flex items-center gap-1.5 text-zinc-400">
              <Clock className="w-3.5 h-3.5" />
              <span>{activeTab.results.elapsed_ms}ms</span>
            </div>
          </>
        )}
        {activeTab.error && !activeTab.isRunning && (
          <span className="text-red-400 font-medium">Error</span>
        )}
      </div>
    </div>
  );
}
