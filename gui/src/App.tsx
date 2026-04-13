import { useState, useCallback, useEffect } from "react";
import type { ConnectionState, EditorTab } from "./types";
import * as cmd from "./commands";
import { ConnectionDialog } from "./components/ConnectionDialog";
import { Layout } from "./components/Layout";

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
  const [connection, setConnection] = useState<ConnectionState | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [connectError, setConnectError] = useState<string | null>(null);
  const [tabs, setTabs] = useState<EditorTab[]>([newTab()]);
  const [activeTabId, setActiveTabId] = useState(tabs[0].id);
  const [tables, setTables] = useState<string[]>([]);
  const [namedQueriesVersion, setNamedQueriesVersion] = useState(0);
  const activeTab = tabs.find((t) => t.id === activeTabId) || tabs[0];

  // ── Connection ─────────────────────────────────────────────────────────
  const performConnect = useCallback(async (connectFn: () => Promise<ConnectionState>) => {
    setConnecting(true);
    setConnectError(null);
    try {
      const state = await connectFn();
      setConnection(state);
      // Fetch tables after connecting
      try {
        const result = await cmd.listTables();
        if (result.rows.length > 0) {
          setTables(result.rows.map((r) => r[0]));
        }
      } catch {
        /* tables will be empty */
      }
    } catch (e) {
      setConnectError(String(e));
    } finally {
      setConnecting(false);
    }
  }, []);

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

  const loadSnippet = useCallback((title: string, sql: string) => {
    setTabs((prev) =>
      prev.map((t) =>
        t.id === activeTabId
          ? { ...t, title, sql, error: null, results: null, isRunning: false }
          : t,
      ),
    );
  }, [activeTabId]);

  // ── Query Execution ────────────────────────────────────────────────────
  const runQuery = useCallback(
    async (id: string) => {
      const tab = tabs.find((t) => t.id === id);
      if (!tab || !tab.sql.trim()) return;

      setTabs((prev) =>
        prev.map((t) =>
          t.id === id
            ? { ...t, isRunning: true, error: null, results: null }
            : t,
        ),
      );

      try {
        const result = await cmd.executeQuery(tab.sql);
        setTabs((prev) =>
          prev.map((t) =>
            t.id === id
              ? { ...t, isRunning: false, results: result, error: null }
              : t,
          ),
        );
      } catch (e) {
        setTabs((prev) =>
          prev.map((t) =>
            t.id === id
              ? { ...t, isRunning: false, error: String(e), results: null }
              : t,
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
          t.id === id
            ? { ...t, isRunning: true, error: null, results: null }
            : t,
        ),
      );

      try {
        const result = await cmd.explainQuery(tab.sql);
        setTabs((prev) =>
          prev.map((t) =>
            t.id === id
              ? { ...t, isRunning: false, results: result, error: null }
              : t,
          ),
        );
      } catch (e) {
        setTabs((prev) =>
          prev.map((t) =>
            t.id === id
              ? { ...t, isRunning: false, error: String(e), results: null }
              : t,
          ),
        );
      }
    },
    [tabs],
  );

  // ── Insert table SQL ───────────────────────────────────────────────────
  const handleTableSelect = useCallback(
    (tableName: string) => {
      const sql = `SELECT * FROM ${tableName} LIMIT 100;`;
      updateTabSql(activeTabId, sql);
    },
    [activeTabId, updateTabSql],
  );

  const handleLoadSnippet = useCallback(
    (title: string, sql: string) => {
      loadSnippet(title, sql);
    },
    [loadSnippet],
  );

  const handleSaveCurrentPreset = useCallback(async () => {
    const sql = activeTab.sql.trim();
    if (!sql) return;

    const suggestedName = activeTab.title
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

  // ── Check for existing connection on mount ─────────────────────────────
  useEffect(() => {
    cmd.getConnectionState().then((state) => {
      if (state) setConnection(state);
    });
  }, []);

  // ── Render ─────────────────────────────────────────────────────────────
  if (!connection) {
    return (
      <ConnectionDialog
        onConnectUrl={handleConnect}
        onConnectRecent={handleConnectRecent}
        onConnectSession={handleConnectSession}
        connecting={connecting}
        error={connectError}
      />
    );
  }

  return (
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
  );
}
