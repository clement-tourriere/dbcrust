import { useState, useCallback } from "react";
import {
  ChevronRight,
  ChevronLeft,
  Loader2,
  AlertCircle,
  Shield,
  Database,
  Key,
  Check,
} from "lucide-react";
import * as cmd from "../commands";

interface VaultWizardProps {
  /** Initial mount path typed by the user (from the URL input), e.g. "database" */
  initialMount: string;
  onConnect: (url: string) => void;
  onCancel: () => void;
  connecting: boolean;
}

type Step = "databases" | "roles" | "confirm";

export function VaultWizard({
  initialMount,
  onConnect,
  onCancel,
  connecting,
}: VaultWizardProps) {
  const [step, setStep] = useState<Step>("databases");
  const [mountPath, setMountPath] = useState(initialMount || "database");
  const [databases, setDatabases] = useState<string[]>([]);
  const [roles, setRoles] = useState<string[]>([]);
  const [selectedDb, setSelectedDb] = useState<string | null>(null);
  const [selectedRole, setSelectedRole] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // ── Step 1: Load databases ─────────────────────────────────────────────
  const loadDatabases = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const dbs = await cmd.listVaultDatabases(mountPath);
      setDatabases(dbs);
      if (dbs.length === 0) {
        setError("No accessible databases found in this Vault mount.");
      }
    } catch (e) {
      setError(String(e));
    }
    setLoading(false);
  }, [mountPath]);

  // ── Step 2: Load roles ─────────────────────────────────────────────────
  const loadRoles = useCallback(
    async (dbName: string) => {
      setLoading(true);
      setError(null);
      setSelectedDb(dbName);
      try {
        const r = await cmd.listVaultRoles(mountPath, dbName);
        setRoles(r);
        if (r.length === 1) {
          // Auto-select if only one role
          setSelectedRole(r[0]);
          setStep("confirm");
        } else if (r.length === 0) {
          setError(`No roles available for database '${dbName}'.`);
        } else {
          setStep("roles");
        }
      } catch (e) {
        setError(String(e));
      }
      setLoading(false);
    },
    [mountPath],
  );

  const handleConfirm = useCallback(() => {
    if (!selectedDb || !selectedRole) return;
    const url = `vault://${selectedRole}@${mountPath}/${selectedDb}`;
    onConnect(url);
  }, [selectedDb, selectedRole, mountPath, onConnect]);

  // ── Start: load databases on first render or mount change ──────────────
  const hasStarted = databases.length > 0 || loading || error;
  if (!hasStarted) {
    // Kick off loading
    loadDatabases();
  }

  return (
    <div className="space-y-4 animate-fade-in">
      {/* ── Header ────────────────────────────────────────────────────── */}
      <div className="flex items-center gap-3 mb-2">
        <div className="w-9 h-9 rounded-lg bg-amber-500/10 flex items-center justify-center">
          <Shield className="w-4 h-4 text-amber-400" />
        </div>
        <div>
          <h3 className="text-sm font-semibold text-zinc-200">
            HashiCorp Vault Connection
          </h3>
          <p className="text-xxs text-zinc-500">
            Select a database and role to obtain dynamic credentials
          </p>
        </div>
      </div>

      {/* ── Mount path ────────────────────────────────────────────────── */}
      <div>
        <label className="text-xxs text-zinc-500 font-medium block mb-1">
          Secrets Engine Mount Path
        </label>
        <div className="flex gap-2">
          <input
            type="text"
            value={mountPath}
            onChange={(e) => setMountPath(e.target.value)}
            placeholder="database"
            className="flex-1 bg-surface-300 border border-zinc-700 rounded-md px-3 py-1.5
              text-sm text-zinc-200 font-mono placeholder-zinc-600
              focus:outline-none focus:border-accent transition-colors"
          />
          <button
            onClick={loadDatabases}
            disabled={loading || !mountPath.trim()}
            className="px-3 py-1.5 rounded-md text-xs font-medium bg-zinc-700
              hover:bg-zinc-600 text-zinc-300 disabled:opacity-40 transition-all"
          >
            {loading && step === "databases" ? (
              <Loader2 className="w-3.5 h-3.5 animate-spin" />
            ) : (
              "Load"
            )}
          </button>
        </div>
      </div>

      {/* ── Breadcrumbs ───────────────────────────────────────────────── */}
      <div className="flex items-center gap-1.5 text-xxs text-zinc-600">
        <span
          className={
            step === "databases" ? "text-accent font-medium" : "text-zinc-400"
          }
        >
          1. Database
        </span>
        <ChevronRight className="w-3 h-3" />
        <span
          className={
            step === "roles" ? "text-accent font-medium" : ""
          }
        >
          2. Role
        </span>
        <ChevronRight className="w-3 h-3" />
        <span
          className={
            step === "confirm" ? "text-accent font-medium" : ""
          }
        >
          3. Connect
        </span>
      </div>

      {/* ── Error ─────────────────────────────────────────────────────── */}
      {error && (
        <div className="flex items-start gap-2 text-xs bg-red-500/10 border border-red-500/20 rounded-lg p-3 text-red-400">
          <AlertCircle className="w-4 h-4 mt-0.5 flex-shrink-0" />
          <span className="break-all">{error}</span>
        </div>
      )}

      {/* ── Step 1: Database selection ─────────────────────────────────── */}
      {step === "databases" && !loading && databases.length > 0 && (
        <div className="space-y-1 max-h-48 overflow-y-auto">
          {databases.map((db) => (
            <button
              key={db}
              onClick={() => loadRoles(db)}
              className="w-full text-left px-3 py-2.5 rounded-lg text-sm hover:bg-zinc-800
                transition-colors flex items-center justify-between group"
            >
              <div className="flex items-center gap-2.5">
                <Database className="w-4 h-4 text-zinc-500" />
                <span className="text-zinc-200 font-medium">{db}</span>
              </div>
              <ChevronRight className="w-4 h-4 text-zinc-600 opacity-0 group-hover:opacity-100 transition-opacity" />
            </button>
          ))}
        </div>
      )}

      {/* ── Step 2: Role selection ─────────────────────────────────────── */}
      {step === "roles" && !loading && roles.length > 0 && (
        <>
          <div className="flex items-center gap-2 text-xs text-zinc-500 mb-1">
            <button
              onClick={() => {
                setStep("databases");
                setSelectedRole(null);
                setError(null);
              }}
              className="flex items-center gap-1 text-zinc-400 hover:text-zinc-200 transition-colors"
            >
              <ChevronLeft className="w-3 h-3" />
              Back
            </button>
            <span>·</span>
            <span>
              Database:{" "}
              <span className="text-zinc-300 font-medium font-mono">
                {selectedDb}
              </span>
            </span>
          </div>
          <div className="space-y-1 max-h-48 overflow-y-auto">
            {roles.map((role) => (
              <button
                key={role}
                onClick={() => {
                  setSelectedRole(role);
                  setStep("confirm");
                }}
                className="w-full text-left px-3 py-2.5 rounded-lg text-sm hover:bg-zinc-800
                  transition-colors flex items-center justify-between group"
              >
                <div className="flex items-center gap-2.5">
                  <Key className="w-4 h-4 text-zinc-500" />
                  <span className="text-zinc-200 font-medium">{role}</span>
                </div>
                <ChevronRight className="w-4 h-4 text-zinc-600 opacity-0 group-hover:opacity-100 transition-opacity" />
              </button>
            ))}
          </div>
        </>
      )}

      {/* ── Step 3: Confirm ───────────────────────────────────────────── */}
      {step === "confirm" && selectedDb && selectedRole && (
        <div className="space-y-3">
          <div className="flex items-center gap-2 text-xs text-zinc-500">
            <button
              onClick={() => {
                setStep("roles");
                setError(null);
              }}
              className="flex items-center gap-1 text-zinc-400 hover:text-zinc-200 transition-colors"
            >
              <ChevronLeft className="w-3 h-3" />
              Back
            </button>
          </div>

          <div className="bg-surface-300 border border-zinc-700 rounded-lg p-4 space-y-2">
            <div className="flex items-center gap-2 text-xs">
              <Shield className="w-3.5 h-3.5 text-amber-400" />
              <span className="text-zinc-500">Mount:</span>
              <span className="text-zinc-200 font-mono">{mountPath}</span>
            </div>
            <div className="flex items-center gap-2 text-xs">
              <Database className="w-3.5 h-3.5 text-blue-400" />
              <span className="text-zinc-500">Database:</span>
              <span className="text-zinc-200 font-mono">{selectedDb}</span>
            </div>
            <div className="flex items-center gap-2 text-xs">
              <Key className="w-3.5 h-3.5 text-emerald-400" />
              <span className="text-zinc-500">Role:</span>
              <span className="text-zinc-200 font-mono">{selectedRole}</span>
            </div>
            <div className="pt-2 border-t border-zinc-700 mt-2">
              <code className="text-xxs text-zinc-500 font-mono">
                vault://{selectedRole}@{mountPath}/{selectedDb}
              </code>
            </div>
          </div>

          <button
            onClick={handleConfirm}
            disabled={connecting}
            className="w-full flex items-center justify-center gap-2 px-4 py-2.5 rounded-lg text-sm font-medium
              bg-accent hover:bg-accent-hover text-white disabled:opacity-40
              disabled:cursor-not-allowed transition-all"
          >
            {connecting ? (
              <Loader2 className="w-4 h-4 animate-spin" />
            ) : (
              <Check className="w-4 h-4" />
            )}
            Connect with Dynamic Credentials
          </button>
        </div>
      )}

      {/* ── Loading ───────────────────────────────────────────────────── */}
      {loading && (
        <div className="flex items-center justify-center py-6 text-zinc-500 text-sm">
          <Loader2 className="w-4 h-4 animate-spin mr-2" />
          {step === "databases"
            ? "Fetching databases from Vault…"
            : "Fetching roles…"}
        </div>
      )}

      {/* ── Cancel ────────────────────────────────────────────────────── */}
      <button
        onClick={onCancel}
        className="w-full text-center text-xs text-zinc-600 hover:text-zinc-400 py-1 transition-colors"
      >
        Cancel
      </button>
    </div>
  );
}
