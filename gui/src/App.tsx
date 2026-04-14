import { useState, useCallback, useEffect } from "react";
import type { ConnectionState, EditorTab, NavigationView } from "./types";
import * as cmd from "./commands";
import { Navigation } from "./components/Navigation";
import { ConnectionDialog } from "./components/ConnectionDialog";
import { SavedConnections } from "./components/SavedConnections";
import { Layout } from "./components/Layout";
import { SchemaExplorer } from "./components/SchemaExplorer";
import { DockerDiscovery } from "./components/DockerDiscovery";
import { SettingsPage } from "./components/SettingsPage";
import { StatusBar } from "./components/StatusBar";

let tabCounter = 1;
function newTab(): EditorTab {
  const id = `tab-${tabCounter++}`;
  return {
    id,
    title: `Query ${tabCounter - 1}`,
    sql: "",
    results: null,
    error: null,
    isRunning: false,
  };
}

export default function App() {
  const [view, setView] = useState<NavigationView>("home");
  const [connection, setConnection] = useState<ConnectionState | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [connectError, setConnectError] = useState<string | null>(null);
  const [tabs, setTabs] = useState<EditorTab[]>([newTab()]);
  const [activeTabId, setActiveTabId] = useState(tabs[0].id);
  const [tables, setTables] = useState<string[]>([]);
  const [namedQueriesVersion, setNamedQueriesVersion] = useState(0);
  const activeTab = tabs.find((t) => t.id === activeTabId) || tabs[0];

  // ── Redirect database views when not connected ────────────────────────
  useEffect(() => {
    if (!connection && (view === "query" || view === "schema" || view === "settings")) {
      setView("home");
    }
  }, [connection, view]);

  // ── Connection ─────────────────────────────────────────────────────────
  const performConnect = useCallback(
    async (connectFn: () => Promise<ConnectionState>) => {
      setConnecting(true);
      setConnectError(null);
      try {
        const state = await connectFn();
        setConnection(state);
        setView("query"); // Jump to editor after connecting
        try {
          const result = await cmd.listTables();
          if (result.rows.length > 0) {
            setTables(result.rows.map((r) => r[1])); // Column 1 = Name
          }
        } catch {
          /* tables will be empty */
        }
      } catch (e) {
        setConnectError(String(e));
      } finally {
        setConnecting(false);
      }
    },
    [],
  );

  const handleConnect = useCallback(
    async (url: string) => {
      await performConnect(() => cmd.connectToDatabase(url));
    },
    [performConnect],
  );

  const handleConnectRecent = useCallback(
    async (index: number) => {
      await performConnect(() => cmd.connectRecentConnection(index));
    },
    [performConnect],
  );

  const handleConnectSession = useCallback(
    async (name: string) => {
      await performConnect(() => cmd.connectSavedSession(name));
    },
    [performConnect],
  );

  const handleDisconnect = useCallback(async () => {
    try {
      await cmd.disconnectFromDatabase();
    } catch {
      /* ignore */
    }
    setConnection(null);
    setTables([]);
    setView("home");
  }, []);

  // ── Tabs ───────────────────────────────────────────────────────────────
  const addTab = useCallback(() => {
    const t = newTab();
    setTabs((prev) => [...prev, t]);
    setActiveTabId(t.id);
  }, []);

  const closeTab = useCallback(
    (id: string) => {
      setTabs((prev) => {
        const next = prev.filter((t) => t.id !== id);
        if (next.length === 0) {
          const t = newTab();
          setActiveTabId(t.id);
          return [t];
        }
        if (activeTabId === id) {
          setActiveTabId(next[next.length - 1].id);
        }
        return next;
      });
    },
    [activeTabId],
  );

  const updateTabSql = useCallback((id: string, sql: string) => {
    setTabs((prev) => prev.map((t) => (t.id === id ? { ...t, sql } : t)));
  }, []);

  const loadSnippet = useCallback(
    (title: string, sql: string) => {
      setTabs((prev) =>
        prev.map((t) =>
          t.id === activeTabId
            ? { ...t, title, sql, error: null, results: null, isRunning: false }
            : t,
        ),
      );
    },
    [activeTabId],
  );

  // ── Query Execution ────────────────────────────────────────────────────
  const runQuery = useCallback(
    async (id: string) => {
      const tab = tabs.find((t) => t.id === id);
      if (!tab || !tab.sql.trim()) return;

      setTabs((prev) =>
        prev.map((t) =>
          t.id === id ? { ...t, isRunning: true, error: null, results: null } : t,
        ),
      );

      try {
        const result = await cmd.executeQuery(tab.sql);
        setTabs((prev) =>
          prev.map((t) =>
            t.id === id
              ? { ...t, isRunning: false, results: result, error: null, isExplain: false }
              : t,
          ),
        );
      } catch (e) {
        setTabs((prev) =>
          prev.map((t) =>
            t.id === id ? { ...t, isRunning: false, error: String(e), results: null } : t,
          ),
        );
      }
    },
    [tabs],
  );

  const runExplain = useCallback(
    async (id: string) => {
      const tab = tabs.find((t) => t.id === id);
      if (!tab || !tab.sql.trim()) return;

      setTabs((prev) =>
        prev.map((t) =>
          t.id === id ? { ...t, isRunning: true, error: null, results: null } : t,
        ),
      );

      try {
        const result = await cmd.explainQuery(tab.sql);
        setTabs((prev) =>
          prev.map((t) =>
            t.id === id
              ? { ...t, isRunning: false, results: result, error: null, isExplain: true }
              : t,
          ),
        );
      } catch (e) {
        setTabs((prev) =>
          prev.map((t) =>
            t.id === id ? { ...t, isRunning: false, error: String(e), results: null } : t,
          ),
        );
      }
    },
    [tabs],
  );

  // ── Table / Preset Actions ─────────────────────────────────────────────
  const handleTableSelect = useCallback(
    (tableName: string) => {
      const sql = `SELECT * FROM ${tableName} LIMIT 100;`;
      updateTabSql(activeTabId, sql);
      setView("query");
    },
    [activeTabId, updateTabSql],
  );

  const handleLoadSnippet = useCallback(
    (title: string, sql: string) => {
      loadSnippet(title, sql);
      setView("query");
    },
    [loadSnippet],
  );

  const handleSaveCurrentPreset = useCallback(async () => {
    const sql = activeTab.sql.trim();
    if (!sql) return;

    const suggestedName =
      activeTab.title
        .trim()
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "_")
        .replace(/^_+|_+$/g, "") || "query_preset";

    const name = window.prompt("Preset name", suggestedName)?.trim();
    if (!name) return;

    try {
      await cmd.saveNamedQuery(name, sql, false);
      setNamedQueriesVersion((version) => version + 1);
    } catch (e) {
      window.alert(`Failed to save preset: ${String(e)}`);
    }
  }, [activeTab]);

  const handleRefreshTables = useCallback(async () => {
    try {
      const result = await cmd.listTables();
      if (result.rows.length > 0) {
        setTables(result.rows.map((r) => r[1]));
      }
    } catch {
      /* ignore */
    }
  }, []);

  // ── Check for existing connection on mount ─────────────────────────────
  useEffect(() => {
    cmd.getConnectionState().then((state) => {
      if (state) {
        setConnection(state);
        setView("query");
      }
    });
  }, []);

  // ── Listen for native menu events from Tauri ───────────────────────────
  useEffect(() => {
    const handler = (menuId: string) => {
      // Handle dynamic tray events: connect_recent_N, connect_session_NAME
      if (menuId.startsWith("connect_recent_")) {
        const idx = parseInt(menuId.slice("connect_recent_".length), 10);
        if (!isNaN(idx)) handleConnectRecent(idx);
        return;
      }
      if (menuId.startsWith("connect_session_")) {
        const name = menuId.slice("connect_session_".length);
        if (name) handleConnectSession(name);
        return;
      }

      switch (menuId) {
        case "view_connect":
          setView("home");
          break;
        case "view_saved":
          setView("saved");
          break;
        case "view_docker":
          setView("docker");
          break;
        case "view_query":
          if (connection) setView("query");
          break;
        case "view_schema":
          if (connection) setView("schema");
          break;
        case "view_settings":
          if (connection) setView("settings");
          break;
        case "new_tab":
          if (connection) {
            addTab();
            setView("query");
          }
          break;
        case "close_tab":
          if (connection) closeTab(activeTabId);
          break;
        case "run_query":
          if (connection) runQuery(activeTabId);
          break;
        case "explain_query":
          if (connection) runExplain(activeTabId);
          break;
        case "save_preset":
          if (connection) handleSaveCurrentPreset();
          break;
        case "disconnect":
          handleDisconnect();
          break;
      }
    };
    (window as unknown as Record<string, unknown>).__DBCRUST_MENU__ = handler;
    return () => {
      delete (window as unknown as Record<string, unknown>).__DBCRUST_MENU__;
    };
  }, [connection, activeTabId, addTab, closeTab, runQuery, runExplain, handleSaveCurrentPreset, handleDisconnect, handleConnectRecent, handleConnectSession]);

  // ── Render ─────────────────────────────────────────────────────────────
  return (
    <div className="h-screen flex flex-col bg-surface-300 animate-fade-in">
      <div className="flex-1 flex min-h-0">
        {/* ── Navigation Rail ──────────────────────────────────────── */}
        <Navigation
          connected={!!connection}
          activeView={view}
          onViewChange={setView}
          onDisconnect={handleDisconnect}
          connectionType={connection?.database_type}
          tables={tables}
        />

        {/* ── Main Content ─────────────────────────────────────────── */}
        <div className="flex-1 min-w-0 flex flex-col">
          <div className="flex-1 min-h-0">
            {/* ── Connect (always available) ────────────────────────── */}
            {view === "home" && (
              <ConnectionDialog
                onConnectUrl={handleConnect}
                onConnectRecent={handleConnectRecent}
                onConnectSession={handleConnectSession}
                connecting={connecting}
                error={connectError}
              />
            )}

            {/* ── Docker Discovery (always available) ───────────────── */}
            {view === "docker" && (
              <DockerDiscovery onConnect={handleConnect} connected={!!connection} />
            )}

            {/* ── Saved Connections (always available) ───────────── */}
            {view === "saved" && (
              <SavedConnections
                onConnectUrl={handleConnect}
                onConnectRecent={handleConnectRecent}
                onConnectSession={handleConnectSession}
                connecting={connecting}
              />
            )}

            {/* ── Query Editor (connected) ──────────────────────────── */}
            {view === "query" && connection && (
              <Layout
                connection={connection}
                tables={tables}
                tabs={tabs}
                activeTab={activeTab}
                activeTabId={activeTabId}
                namedQueriesVersion={namedQueriesVersion}
                onTabSelect={setActiveTabId}
                onTabClose={closeTab}
                onTabAdd={addTab}
                onSqlChange={updateTabSql}
                onRunQuery={runQuery}
                onRunExplain={runExplain}
                onSaveCurrentPreset={handleSaveCurrentPreset}
                onDisconnect={handleDisconnect}
                onTableSelect={handleTableSelect}
                onLoadSnippet={handleLoadSnippet}
              />
            )}

            {/* ── Schema Explorer (connected) ───────────────────────── */}
            {view === "schema" && connection && (
              <SchemaExplorer
                connection={connection}
                tables={tables}
                onRefreshTables={handleRefreshTables}
                onTableSelect={handleTableSelect}
              />
            )}

            {/* ── Settings (connected) ──────────────────────────────── */}
            {view === "settings" && connection && <SettingsPage />}
          </div>

          {/* ── Status Bar ────────────────────────────────────────── */}
          {connection && (
            <StatusBar connection={connection} activeTab={activeTab} currentView={view} />
          )}
        </div>
      </div>
    </div>
  );
}
