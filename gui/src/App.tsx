import { useState, useCallback, useEffect, lazy, Suspense } from "react";
import type { ConnectionState, EditorTab, NavigationView } from "./types";
import * as cmd from "./commands";
import { Navigation } from "./components/Navigation";
import { ConnectionDialog } from "./components/ConnectionDialog";
import { StatusBar } from "./components/StatusBar";
import { extractTableNames } from "./tableMetadata";

// Lazy-loaded views — keeps the initial bundle small.
// Layout is the heaviest (~450-520 KB) since it pulls in CodeMirror.
const Layout = lazy(() => import("./components/Layout"));
const SchemaExplorer = lazy(() => import("./components/SchemaExplorer"));
const DockerDiscovery = lazy(() => import("./components/DockerDiscovery"));
const SavedConnections = lazy(() => import("./components/SavedConnections"));
const SettingsPage = lazy(() => import("./components/SettingsPage"));

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

interface ConnectOptions {
  vaultAddr?: string;
}

const VAULT_ADDR_STORAGE_KEY = "dbcrust.vaultAddr";

function loadStoredVaultAddr(): string | null {
  if (typeof window === "undefined") return null;
  const stored = window.localStorage.getItem(VAULT_ADDR_STORAGE_KEY)?.trim();
  return stored || null;
}

export default function App() {
  const [view, setView] = useState<NavigationView>("home");
  const [connection, setConnection] = useState<ConnectionState | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [connectError, setConnectError] = useState<string | null>(null);
  const [tabs, setTabs] = useState<EditorTab[]>([newTab()]);
  const [activeTabId, setActiveTabId] = useState(tabs[0].id);
  const [tables, setTables] = useState<string[]>([]);
  const [tablesError, setTablesError] = useState<string | null>(null);
  const [namedQueriesVersion, setNamedQueriesVersion] = useState(0);
  const [vaultAddr, setVaultAddr] = useState<string | null>(() => loadStoredVaultAddr());
  const activeTab = tabs.find((t) => t.id === activeTabId) || tabs[0];

  const rememberVaultAddr = useCallback((nextVaultAddr?: string | null) => {
    const normalized = nextVaultAddr?.trim() || null;
    setVaultAddr(normalized);

    if (typeof window === "undefined") return;

    if (normalized) {
      window.localStorage.setItem(VAULT_ADDR_STORAGE_KEY, normalized);
    } else {
      window.localStorage.removeItem(VAULT_ADDR_STORAGE_KEY);
    }
  }, []);

  const loadTablesForConnection = useCallback(async (databaseType?: string) => {
    try {
      const result = await cmd.listTables();
      setTables(extractTableNames(result.rows, databaseType));
      setTablesError(null);
    } catch (error) {
      setTables([]);
      setTablesError(String(error));
    }
  }, []);

  // ── Redirect database views when not connected ────────────────────────
  useEffect(() => {
    if (!connection && (view === "query" || view === "schema" || view === "settings")) {
      setView("home");
    }
  }, [connection, view]);

  // ── Vault environment bootstrap ───────────────────────────────────────
  useEffect(() => {
    let cancelled = false;

    cmd
      .getVaultEnvironment()
      .then((environment) => {
        if (cancelled) return;
        if (!loadStoredVaultAddr() && environment.vault_addr) {
          rememberVaultAddr(environment.vault_addr);
        }
      })
      .catch(() => {
        /* ignore */
      });

    return () => {
      cancelled = true;
    };
  }, [rememberVaultAddr]);

  // ── Connection ─────────────────────────────────────────────────────────
  const performConnect = useCallback(
    async (connectFn: () => Promise<ConnectionState>) => {
      setConnecting(true);
      setConnectError(null);
      setTablesError(null);
      try {
        const state = await connectFn();
        setConnection(state);
        setView("query");
        await loadTablesForConnection(state.database_type);
      } catch (error) {
        setConnectError(String(error));
      } finally {
        setConnecting(false);
      }
    },
    [loadTablesForConnection],
  );

  const handleConnect = useCallback(
    async (url: string, options?: ConnectOptions) => {
      const effectiveVaultAddr = options?.vaultAddr?.trim() || vaultAddr || undefined;
      if (options?.vaultAddr) {
        rememberVaultAddr(options.vaultAddr);
      }

      await performConnect(() => cmd.connectToDatabase(url, effectiveVaultAddr));
    },
    [performConnect, rememberVaultAddr, vaultAddr],
  );

  const handleConnectRecent = useCallback(
    async (index: number) => {
      await performConnect(() =>
        cmd.connectRecentConnection(index, vaultAddr || undefined),
      );
    },
    [performConnect, vaultAddr],
  );

  const handleConnectSession = useCallback(
    async (name: string) => {
      await performConnect(() => cmd.connectSavedSession(name, vaultAddr || undefined));
    },
    [performConnect, vaultAddr],
  );

  const handleDisconnect = useCallback(async () => {
    try {
      await cmd.disconnectFromDatabase();
    } catch {
      /* ignore */
    }
    setConnection(null);
    setTables([]);
    setTablesError(null);
    setView("home");
  }, []);

  // ── Tabs ───────────────────────────────────────────────────────────────
  const addTab = useCallback(() => {
    const tab = newTab();
    setTabs((prev) => [...prev, tab]);
    setActiveTabId(tab.id);
  }, []);

  const closeTab = useCallback(
    (id: string) => {
      setTabs((prev) => {
        const next = prev.filter((tab) => tab.id !== id);
        if (next.length === 0) {
          const tab = newTab();
          setActiveTabId(tab.id);
          return [tab];
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
    setTabs((prev) => prev.map((tab) => (tab.id === id ? { ...tab, sql } : tab)));
  }, []);

  const loadSnippet = useCallback(
    (title: string, sql: string) => {
      setTabs((prev) =>
        prev.map((tab) =>
          tab.id === activeTabId
            ? { ...tab, title, sql, error: null, results: null, isRunning: false }
            : tab,
        ),
      );
    },
    [activeTabId],
  );

  // ── Query Execution ────────────────────────────────────────────────────
  const runQuery = useCallback(
    async (id: string, sqlOverride?: string) => {
      const tab = tabs.find((entry) => entry.id === id);
      const sqlToRun = (sqlOverride ?? tab?.sql ?? "").trim();
      if (!tab || !sqlToRun) return;

      setTabs((prev) =>
        prev.map((entry) =>
          entry.id === id
            ? { ...entry, isRunning: true, error: null, results: null }
            : entry,
        ),
      );

      try {
        const result = await cmd.executeQuery(sqlToRun);
        setTabs((prev) =>
          prev.map((entry) =>
            entry.id === id
              ? { ...entry, isRunning: false, results: result, error: null, isExplain: false }
              : entry,
          ),
        );
      } catch (error) {
        setTabs((prev) =>
          prev.map((entry) =>
            entry.id === id
              ? { ...entry, isRunning: false, error: String(error), results: null }
              : entry,
          ),
        );
      }
    },
    [tabs],
  );

  const runExplain = useCallback(
    async (id: string, sqlOverride?: string) => {
      const tab = tabs.find((entry) => entry.id === id);
      const sqlToRun = (sqlOverride ?? tab?.sql ?? "").trim();
      if (!tab || !sqlToRun) return;

      setTabs((prev) =>
        prev.map((entry) =>
          entry.id === id
            ? { ...entry, isRunning: true, error: null, results: null }
            : entry,
        ),
      );

      try {
        const result = await cmd.explainQuery(sqlToRun);
        setTabs((prev) =>
          prev.map((entry) =>
            entry.id === id
              ? { ...entry, isRunning: false, results: result, error: null, isExplain: true }
              : entry,
          ),
        );
      } catch (error) {
        setTabs((prev) =>
          prev.map((entry) =>
            entry.id === id
              ? { ...entry, isRunning: false, error: String(error), results: null }
              : entry,
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
    } catch (error) {
      window.alert(`Failed to save preset: ${String(error)}`);
    }
  }, [activeTab]);

  const handleRefreshTables = useCallback(async () => {
    await loadTablesForConnection(connection?.database_type);
  }, [connection?.database_type, loadTablesForConnection]);

  // ── Check for existing connection on mount ─────────────────────────────
  useEffect(() => {
    let cancelled = false;

    cmd.getConnectionState().then((state) => {
      if (!state || cancelled) return;
      setConnection(state);
      setView("query");
      loadTablesForConnection(state.database_type).catch(() => {
        /* ignore */
      });
    });

    return () => {
      cancelled = true;
    };
  }, [loadTablesForConnection]);

  // ── Listen for native menu events from Tauri ───────────────────────────
  useEffect(() => {
    const handler = (menuId: string) => {
      if (menuId.startsWith("connect_recent_")) {
        const index = parseInt(menuId.slice("connect_recent_".length), 10);
        if (!isNaN(index)) handleConnectRecent(index);
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
  }, [
    connection,
    activeTabId,
    addTab,
    closeTab,
    runQuery,
    runExplain,
    handleSaveCurrentPreset,
    handleDisconnect,
    handleConnectRecent,
    handleConnectSession,
  ]);

  // ── Render ─────────────────────────────────────────────────────────────

  const lazyFallback = (
    <div className="flex-1 flex items-center justify-center text-text-secondary">
      Loading…
    </div>
  );

  return (
    <div className="h-screen flex flex-col bg-surface-300 animate-fade-in">
      <div className="flex-1 flex min-h-0">
        <Navigation
          connected={!!connection}
          activeView={view}
          onViewChange={setView}
          onDisconnect={handleDisconnect}
          connectionType={connection?.database_type}
          tables={tables}
        />

        <div className="flex-1 min-w-0 flex flex-col">
          <div className="flex-1 min-h-0">
            {view === "home" && (
              <ConnectionDialog
                onConnectUrl={handleConnect}
                onConnectRecent={handleConnectRecent}
                onConnectSession={handleConnectSession}
                connecting={connecting}
                error={connectError}
              />
            )}

            {view === "docker" && (
              <Suspense fallback={lazyFallback}>
                <DockerDiscovery
                  onConnect={handleConnect}
                  connected={!!connection}
                  connecting={connecting}
                  error={connectError}
                />
              </Suspense>
            )}

            {view === "saved" && (
              <Suspense fallback={lazyFallback}>
                <SavedConnections
                  onConnectUrl={handleConnect}
                  onConnectRecent={handleConnectRecent}
                  onConnectSession={handleConnectSession}
                  connecting={connecting}
                />
              </Suspense>
            )}

            {view === "query" && connection && (
              <Suspense fallback={lazyFallback}>
                <Layout
                  connection={connection}
                  tables={tables}
                  tablesError={tablesError}
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
              </Suspense>
            )}

            {view === "schema" && connection && (
              <Suspense fallback={lazyFallback}>
                <SchemaExplorer
                  connection={connection}
                  tables={tables}
                  onRefreshTables={handleRefreshTables}
                  onTableSelect={handleTableSelect}
                />
              </Suspense>
            )}

            {view === "settings" && connection && (
              <Suspense fallback={lazyFallback}>
                <SettingsPage />
              </Suspense>
            )}
          </div>

          {connection && (
            <StatusBar connection={connection} activeTab={activeTab} currentView={view} />
          )}
        </div>
      </div>
    </div>
  );
}
