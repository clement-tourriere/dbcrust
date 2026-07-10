import { useState, useRef, useCallback } from "react";
import type { ConnectionState, EditorTab } from "../types";
import { Sidebar } from "./Sidebar";
import { Editor, type EditorHandle } from "./Editor";
import { ResultsPanel } from "./ResultsPanel";
import { Plus, X, Play, Zap, BookmarkPlus } from "lucide-react";

interface LayoutProps {
  connection: ConnectionState;
  tables: string[];
  tablesError?: string | null;
  tabs: EditorTab[];
  activeTab: EditorTab;
  activeTabId: string;
  namedQueriesVersion: number;
  onTabSelect: (id: string) => void;
  onTabClose: (id: string) => void;
  onTabAdd: () => void;
  onSqlChange: (id: string, sql: string) => void;
  onRunQuery: (id: string, sqlOverride?: string) => void;
  onRunExplain: (id: string, sqlOverride?: string) => void;
  onSaveCurrentPreset: () => void;
  onDisconnect: () => void;
  onTableSelect: (tableName: string) => void;
  onLoadSnippet: (title: string, sql: string) => void;
  onRefreshTables: () => Promise<void>;
}

export function Layout({
  connection,
  tables,
  tablesError,
  tabs,
  activeTab,
  activeTabId,
  namedQueriesVersion,
  onTabSelect,
  onTabClose,
  onTabAdd,
  onSqlChange,
  onRunQuery,
  onRunExplain,
  onSaveCurrentPreset,
  onDisconnect,
  onTableSelect,
  onLoadSnippet,
  onRefreshTables,
}: LayoutProps) {
  const [sidebarWidth, setSidebarWidth] = useState(260);
  const [editorHeight, setEditorHeight] = useState<number | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<EditorHandle>(null);

  const getRunnableSql = useCallback(() => editorRef.current?.getExecutionTarget()?.sql, []);

  // ── Sidebar Resize ─────────────────────────────────────────────────────
  const handleSidebarResize = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      const startX = e.clientX;
      const startWidth = sidebarWidth;
      const onMove = (ev: MouseEvent) => {
        const newW = Math.max(180, Math.min(500, startWidth + ev.clientX - startX));
        setSidebarWidth(newW);
      };
      const onUp = () => {
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
      };
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
    },
    [sidebarWidth],
  );

  // ── Editor/Results Resize ──────────────────────────────────────────────
  const handleVerticalResize = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      const container = containerRef.current;
      if (!container) return;
      const rect = container.getBoundingClientRect();
      const startY = e.clientY;
      const startH = editorHeight ?? rect.height * 0.45;
      const onMove = (ev: MouseEvent) => {
        const newH = Math.max(100, Math.min(rect.height - 100, startH + ev.clientY - startY));
        setEditorHeight(newH);
      };
      const onUp = () => {
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
      };
      document.body.style.cursor = "row-resize";
      document.body.style.userSelect = "none";
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
    },
    [editorHeight],
  );

  return (
    <div className="h-full flex flex-col bg-surface-300 animate-fade-in">
      {/* ── Main Content ──────────────────────────────────────────────── */}
      <div className="flex-1 flex min-h-0">
        {/* ── Sidebar ──────────────────────────────────────────────────── */}
        <div style={{ width: sidebarWidth }} className="flex-shrink-0">
          <Sidebar
            connection={connection}
            tables={tables}
            tablesError={tablesError}
            onTableSelect={onTableSelect}
            onLoadSnippet={onLoadSnippet}
            namedQueriesVersion={namedQueriesVersion}
            onDisconnect={onDisconnect}
            onRefreshTables={onRefreshTables}
          />
        </div>

        {/* ── Sidebar Resize Handle ────────────────────────────────────── */}
        <div
          className="resize-handle resize-handle-h bg-zinc-800 hover:bg-accent"
          onMouseDown={handleSidebarResize}
          onKeyDown={(event) => {
            if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
            event.preventDefault();
            setSidebarWidth((width) =>
              Math.max(180, Math.min(500, width + (event.key === "ArrowLeft" ? -16 : 16))),
            );
          }}
          role="separator"
          aria-label="Resize schema sidebar"
          aria-orientation="vertical"
          aria-valuemin={180}
          aria-valuemax={500}
          aria-valuenow={sidebarWidth}
          tabIndex={0}
        />

        {/* ── Editor + Results Area ────────────────────────────────────── */}
        <div className="flex-1 flex flex-col min-w-0" ref={containerRef}>
          {/* ── Tab Bar ─────────────────────────────────────────────────── */}
          <div className="flex items-center bg-surface-200 border-b border-zinc-800 h-9 flex-shrink-0">
            <div
              className="flex items-center overflow-x-auto flex-1 min-w-0"
              role="tablist"
              aria-label="Query tabs"
            >
              {tabs.map((tab) => (
                <div
                  key={tab.id}
                  className={`group flex items-center h-9 border-r border-zinc-800 whitespace-nowrap transition-colors-fast
                    ${
                      tab.id === activeTabId
                        ? "bg-surface text-zinc-200 border-b-2 border-b-accent"
                        : "text-zinc-500 hover:text-zinc-300 hover:bg-surface-100"
                    }`}
                >
                  <button
                    onClick={() => onTabSelect(tab.id)}
                    role="tab"
                    aria-selected={tab.id === activeTabId}
                    className="flex items-center gap-1.5 pl-3 pr-1 h-full text-xs font-medium"
                  >
                    <span
                      aria-hidden="true"
                      className={`w-2 h-2 rounded-full ${
                        tab.isRunning
                          ? "bg-amber-500 animate-pulse-soft"
                          : tab.error
                            ? "bg-red-500"
                            : tab.results
                              ? "bg-emerald-500"
                              : "bg-zinc-600"
                      }`}
                    />
                    {tab.title}
                  </button>
                  <button
                    onClick={() => onTabClose(tab.id)}
                    className="mr-1 p-1 rounded hover:bg-zinc-700 opacity-40 group-hover:opacity-100 focus:opacity-100 transition-opacity"
                    aria-label={`Close ${tab.title}`}
                    title={`Close ${tab.title}`}
                  >
                    <X className="w-3 h-3" />
                  </button>
                </div>
              ))}
            </div>
            <button
              onClick={onTabAdd}
              className="px-2 h-9 text-zinc-500 hover:text-zinc-300 hover:bg-zinc-800 transition-colors flex items-center"
              title="New tab"
              aria-label="New query tab"
            >
              <Plus className="w-4 h-4" />
            </button>

            {/* ── Run Buttons ───────────────────────────────────────────── */}
            <div className="flex items-center gap-1 px-2 border-l border-zinc-800">
              <button
                onClick={() => onRunQuery(activeTabId, getRunnableSql())}
                disabled={activeTab.isRunning || !activeTab.sql.trim()}
                className="flex items-center gap-1.5 px-3 py-1 text-xs font-medium rounded
                  bg-emerald-600 hover:bg-emerald-500 text-white disabled:opacity-40
                  disabled:cursor-not-allowed transition-all"
                title="Run selected SQL or the statement under the cursor (Ctrl+Enter)"
              >
                <Play className="w-3 h-3" />
                Run Current
              </button>
              <button
                onClick={() => onRunExplain(activeTabId, getRunnableSql())}
                disabled={activeTab.isRunning || !activeTab.sql.trim()}
                className="flex items-center gap-1.5 px-2 py-1 text-xs font-medium rounded
                  bg-zinc-700 hover:bg-zinc-600 text-zinc-300 disabled:opacity-40
                  disabled:cursor-not-allowed transition-all"
                title="Explain the selected SQL or the statement under the cursor"
              >
                <Zap className="w-3 h-3" />
                Explain Current
              </button>
              <button
                onClick={onSaveCurrentPreset}
                disabled={!activeTab.sql.trim()}
                className="flex items-center gap-1.5 px-2 py-1 text-xs font-medium rounded
                  bg-zinc-800 hover:bg-zinc-700 text-zinc-300 disabled:opacity-40
                  disabled:cursor-not-allowed transition-all"
                title="Save current query as preset"
              >
                <BookmarkPlus className="w-3 h-3" />
                Save Preset
              </button>
            </div>
          </div>

          {/* ── Editor ──────────────────────────────────────────────────── */}
          <div
            style={{
              height: editorHeight ?? "45%",
              minHeight: 100,
            }}
            className="flex-shrink-0"
          >
            <Editor
              ref={editorRef}
              sql={activeTab.sql}
              tables={tables}
              databaseType={connection.database_type}
              onChange={(sql) => onSqlChange(activeTabId, sql)}
              onRun={(sqlOverride) => onRunQuery(activeTabId, sqlOverride)}
              onExplain={(sqlOverride) => onRunExplain(activeTabId, sqlOverride)}
              isRunning={activeTab.isRunning}
            />
          </div>

          {/* ── Vertical Resize Handle ─────────────────────────────────── */}
          <div
            className="resize-handle resize-handle-v bg-zinc-800 hover:bg-accent"
            onMouseDown={handleVerticalResize}
            onKeyDown={(event) => {
              if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
              const container = containerRef.current;
              if (!container) return;
              event.preventDefault();
              const height = container.getBoundingClientRect().height;
              setEditorHeight((current) => {
                const next = (current ?? height * 0.45) +
                  (event.key === "ArrowUp" ? -16 : 16);
                return Math.max(100, Math.min(height - 100, next));
              });
            }}
            role="separator"
            aria-label="Resize query editor and results"
            aria-orientation="horizontal"
            tabIndex={0}
          />

          {/* ── Results ─────────────────────────────────────────────────── */}
          <div className="flex-1 min-h-[100px]">
            <ResultsPanel
              results={activeTab.results}
              error={activeTab.error}
              isRunning={activeTab.isRunning}
              isExplain={activeTab.isExplain}
            />
          </div>
        </div>
      </div>

    </div>
  );
}

export default Layout;
