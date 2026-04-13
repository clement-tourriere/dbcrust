import { Database, Clock, Rows3, Zap } from "lucide-react";
import type { ConnectionState, EditorTab } from "../types";
import { formatConnectionTarget } from "../connectionDisplay";

interface StatusBarProps {
  connection: ConnectionState;
  activeTab: EditorTab;
}

export function StatusBar({ connection, activeTab }: StatusBarProps) {
  return (
    <div className="h-6 flex items-center justify-between px-3 bg-surface-200 border-t border-zinc-800 text-xxs select-none">
      {/* ── Left ──────────────────────────────────────────────────────── */}
      <div className="flex items-center gap-4">
        <div className="flex items-center gap-1.5 text-zinc-400">
          <span className="w-2 h-2 rounded-full bg-emerald-500" />
          <Database className="w-3 h-3" />
          <span className="font-medium">{connection.database_type}</span>
          <span className="text-zinc-600">·</span>
          <span>{connection.database_name}</span>
        </div>
        <div className="text-zinc-600">
          {formatConnectionTarget(connection)}
        </div>
      </div>

      {/* ── Right ─────────────────────────────────────────────────────── */}
      <div className="flex items-center gap-4">
        {activeTab.isRunning && (
          <div className="flex items-center gap-1.5 text-amber-500">
            <Zap className="w-3 h-3" />
            <span>Executing…</span>
          </div>
        )}
        {activeTab.results && (
          <>
            <div className="flex items-center gap-1.5 text-zinc-500">
              <Rows3 className="w-3 h-3" />
              <span>
                {activeTab.results.row_count} row
                {activeTab.results.row_count !== 1 ? "s" : ""}
              </span>
            </div>
            <div className="flex items-center gap-1.5 text-zinc-500">
              <Clock className="w-3 h-3" />
              <span>{activeTab.results.elapsed_ms}ms</span>
            </div>
          </>
        )}
        {activeTab.error && (
          <span className="text-red-400">Error</span>
        )}
        <span className="text-zinc-700">DBCrust GUI v0.1.0</span>
      </div>
    </div>
  );
}
