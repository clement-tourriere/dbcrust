import { useState, useEffect, useCallback } from "react";
import {
  Boxes,
  RefreshCw,
  Loader2,
  AlertCircle,
  ChevronRight,
  Wifi,
  WifiOff,
} from "lucide-react";
import * as cmd from "../commands";
import type { DockerContainer } from "../types";

const DB_EMOJI: Record<string, string> = {
  PostgreSQL: "🐘",
  MySQL: "🐬",
  SQLite: "📦",
  ClickHouse: "⚡",
  MongoDB: "🍃",
  Elasticsearch: "🔍",
};

interface DockerDiscoveryProps {
  onConnect: (url: string) => Promise<void>;
  connected: boolean;
  connecting: boolean;
  error: string | null;
}

export function DockerDiscovery({ onConnect, connecting, error: connectError }: DockerDiscoveryProps) {
  const [containers, setContainers] = useState<DockerContainer[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [connectingId, setConnectingId] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await cmd.discoverDockerContainers();
      setContainers(result);
    } catch (e) {
      setError(String(e));
      setContainers([]);
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // Clear spinner when parent reports connection finished (success or error)
  useEffect(() => {
    if (!connecting && connectingId) {
      setConnectingId(null);
    }
  }, [connecting, connectingId]);

  const handleConnect = useCallback(
    async (container: DockerContainer) => {
      setConnectingId(container.id);
      const url = `docker://${container.name}`;
      try {
        await onConnect(url);
      } finally {
        setConnectingId(null);
      }
    },
    [onConnect],
  );

  return (
    <div className="h-full overflow-auto bg-surface-300">
      <div className="max-w-4xl mx-auto p-8 animate-fade-in">
        {/* Header */}
        <div className="flex items-center justify-between mb-8">
          <div>
            <h1 className="text-2xl font-bold text-zinc-100 flex items-center gap-3">
              <div className="w-10 h-10 rounded-xl bg-cyan-500/10 flex items-center justify-center text-xl">
                🐳
              </div>
              Docker Discovery
            </h1>
            <p className="text-sm text-zinc-500 mt-2 ml-[52px]">
              Automatically detect database containers running on your machine.
              Click to connect instantly.
            </p>
          </div>
          <button
            onClick={refresh}
            disabled={loading}
            className="flex items-center gap-2 px-4 py-2 rounded-lg text-sm font-medium
              bg-zinc-800 hover:bg-zinc-700 text-zinc-300 disabled:opacity-50 transition-all"
          >
            <RefreshCw
              className={`w-4 h-4 ${loading ? "animate-spin" : ""}`}
            />
            Refresh
          </button>
        </div>

        {/* Loading */}
        {loading && containers.length === 0 && (
          <div className="flex items-center justify-center py-20 text-zinc-500">
            <Loader2 className="w-6 h-6 animate-spin mr-3" />
            <span className="text-sm">Scanning for database containers…</span>
          </div>
        )}

        {/* Error */}
        {error && !loading && (
          <div className="bg-surface rounded-xl border border-zinc-800 p-8 text-center">
            <AlertCircle className="w-10 h-10 text-zinc-600 mx-auto mb-4" />
            <h3 className="text-sm font-medium text-zinc-300 mb-2">
              Could not discover containers
            </h3>
            <p className="text-xs text-zinc-500 max-w-md mx-auto mb-4">
              {error.includes("Docker not available")
                ? "Docker doesn't appear to be running. Start Docker Desktop or the Docker daemon and try again."
                : error.includes("No database containers")
                  ? "No database containers were found. Start a container with PostgreSQL, MySQL, MongoDB, etc. and try again."
                  : error}
            </p>
            <button
              onClick={refresh}
              className="px-4 py-2 rounded-lg text-sm font-medium bg-zinc-800 hover:bg-zinc-700 text-zinc-300 transition-all"
            >
              Try Again
            </button>
          </div>
        )}

        {/* Container List */}
        {!loading && !error && containers.length === 0 && (
          <div className="bg-surface rounded-xl border border-zinc-800 p-8 text-center">
            <Boxes className="w-10 h-10 text-zinc-600 mx-auto mb-4" />
            <h3 className="text-sm font-medium text-zinc-300 mb-2">
              No database containers found
            </h3>
            <p className="text-xs text-zinc-500 max-w-md mx-auto">
              Start a Docker container running PostgreSQL, MySQL, SQLite,
              MongoDB, ClickHouse, or Elasticsearch to see it here.
            </p>
          </div>
        )}

        {containers.length > 0 && (
          <div className="space-y-3">
            {containers.map((container) => (
              <div
                key={container.id}
                className={`bg-surface rounded-xl border transition-all
                  ${container.is_running ? "border-zinc-800 hover:border-zinc-700" : "border-zinc-800/50 opacity-60"}`}
              >
                <div className="p-5 flex items-center gap-4">
                  {/* DB Type Icon */}
                  <div className="w-12 h-12 rounded-xl bg-zinc-800 flex items-center justify-center text-xl flex-shrink-0">
                    {container.database_type
                      ? (DB_EMOJI[container.database_type] ?? "🔗")
                      : "🔗"}
                  </div>

                  {/* Info */}
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2 mb-1">
                      <h3 className="text-sm font-semibold text-zinc-100 truncate">
                        {container.name}
                      </h3>
                      {container.is_running ? (
                        <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-emerald-500/10 text-emerald-400 text-xxs font-medium">
                          <Wifi className="w-2.5 h-2.5" />
                          Running
                        </span>
                      ) : (
                        <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-zinc-800 text-zinc-500 text-xxs font-medium">
                          <WifiOff className="w-2.5 h-2.5" />
                          Stopped
                        </span>
                      )}
                    </div>
                    <div className="flex items-center gap-3 text-xs text-zinc-500">
                      <span className="font-mono truncate max-w-xs">
                        {container.image}
                      </span>
                      {container.database_type && (
                        <>
                          <span className="text-zinc-700">·</span>
                          <span>{container.database_type}</span>
                        </>
                      )}
                      {container.host_port && (
                        <>
                          <span className="text-zinc-700">·</span>
                          <span>
                            Port {container.host_port}
                            {container.container_port &&
                              container.host_port !==
                                container.container_port &&
                              ` → ${container.container_port}`}
                          </span>
                        </>
                      )}
                    </div>
                    <div className="text-xxs text-zinc-600 mt-1 truncate">
                      {container.status}
                    </div>
                  </div>

                  {/* Connect Button */}
                  <button
                    onClick={() => handleConnect(container)}
                    disabled={!container.is_running || connecting}
                    className="flex items-center gap-2 px-4 py-2 rounded-lg text-sm font-medium
                      bg-accent hover:bg-accent-hover text-white disabled:opacity-40
                      disabled:cursor-not-allowed transition-all flex-shrink-0"
                  >
                    {connectingId === container.id ? (
                      <Loader2 className="w-4 h-4 animate-spin" />
                    ) : (
                      <ChevronRight className="w-4 h-4" />
                    )}
                    Connect
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}

        {/* Connection Error */}
        {connectError && !connecting && (
          <div className="mt-4 bg-red-500/10 border border-red-500/20 rounded-xl p-4 flex items-start gap-3">
            <AlertCircle className="w-5 h-5 text-red-400 flex-shrink-0 mt-0.5" />
            <div>
              <h4 className="text-sm font-medium text-red-300">Connection failed</h4>
              <p className="text-xs text-red-400/80 mt-1">{connectError}</p>
            </div>
          </div>
        )}

        {/* Help Footer */}
        <div className="mt-8 text-center text-xxs text-zinc-700">
          Supports PostgreSQL, MySQL, SQLite, ClickHouse, MongoDB,
          Elasticsearch · OrbStack & Docker Desktop
        </div>
      </div>
    </div>
  );
}
