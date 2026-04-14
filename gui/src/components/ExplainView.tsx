import { useMemo, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  Gauge,
  Search,
  Zap,
  TreePine,
  ArrowRight,
  Info,
  Database,
  Copy,
  Check,
} from "lucide-react";
import type { QueryResult } from "../types";

interface ExplainViewProps {
  results: QueryResult;
}

interface PlanStep {
  id: number;
  parent: number;
  detail: string;
  indent: number;
}

/** Strip ANSI escape sequences from a string */
function stripAnsi(str: string): string {
  return str.replace(
    /[\u001b\u009b][[\]()#;?]*(?:[0-9]{1,4}(?:;[0-9]{0,4})*)?[0-9A-ORZcf-nq-uy=><~]/g,
    "",
  );
}

/** Parse SQLite EXPLAIN QUERY PLAN rows into plan steps */
function parseSqlitePlan(results: QueryResult): PlanStep[] {
  const steps: PlanStep[] = [];
  for (const row of results.rows) {
    // SQLite EXPLAIN QUERY PLAN: id, parent, notused, detail
    if (row.length >= 4) {
      steps.push({
        id: parseInt(row[0]) || 0,
        parent: parseInt(row[1]) || 0,
        detail: row[3] || row[2] || "",
        indent: 0,
      });
    } else if (row.length >= 1) {
      // Fallback: single-column output
      steps.push({
        id: steps.length,
        parent: 0,
        detail: stripAnsi(row[0] || ""),
        indent: 0,
      });
    }
  }
  // Calculate indent from parent relationships
  const idMap = new Map(steps.map((s) => [s.id, s]));
  for (const step of steps) {
    let depth = 0;
    let current = step;
    while (current.parent !== 0 && idMap.has(current.parent) && depth < 10) {
      depth++;
      current = idMap.get(current.parent)!;
    }
    step.indent = depth;
  }
  return steps;
}

/** Detect the type of plan node and return an icon + color */
function getNodeStyle(detail: string): {
  icon: React.ComponentType<{ className?: string }>;
  color: string;
  bgColor: string;
  label: string;
} {
  const d = detail.toUpperCase();
  if (d.includes("SCAN") && !d.includes("INDEX")) {
    return {
      icon: Search,
      color: "text-amber-400",
      bgColor: "bg-amber-500/10",
      label: "Table Scan",
    };
  }
  if (d.includes("INDEX") || d.includes("USING")) {
    return {
      icon: Zap,
      color: "text-emerald-400",
      bgColor: "bg-emerald-500/10",
      label: "Index Scan",
    };
  }
  if (d.includes("SEARCH")) {
    return {
      icon: Search,
      color: "text-blue-400",
      bgColor: "bg-blue-500/10",
      label: "Search",
    };
  }
  if (d.includes("MERGE") || d.includes("JOIN")) {
    return {
      icon: TreePine,
      color: "text-purple-400",
      bgColor: "bg-purple-500/10",
      label: "Join",
    };
  }
  if (d.includes("SORT") || d.includes("ORDER")) {
    return {
      icon: ArrowRight,
      color: "text-cyan-400",
      bgColor: "bg-cyan-500/10",
      label: "Sort",
    };
  }
  if (d.includes("AGGREGATE") || d.includes("GROUP")) {
    return {
      icon: Database,
      color: "text-pink-400",
      bgColor: "bg-pink-500/10",
      label: "Aggregate",
    };
  }
  return {
    icon: Info,
    color: "text-zinc-400",
    bgColor: "bg-zinc-800",
    label: "Step",
  };
}

/** Detect warnings in a plan step */
function getWarnings(detail: string): string[] {
  const warnings: string[] = [];
  const d = detail.toUpperCase();
  if (d.includes("SCAN") && !d.includes("INDEX") && !d.includes("SEARCH")) {
    warnings.push("Full table scan — consider adding an index");
  }
  if (d.includes("TEMP B-TREE") || d.includes("TEMPORARY")) {
    warnings.push("Uses temporary storage for sorting");
  }
  if (d.includes("SUBQUERY") || d.includes("CORRELATED")) {
    warnings.push("Correlated subquery may be slow");
  }
  return warnings;
}

export function ExplainView({ results }: ExplainViewProps) {
  const steps = useMemo(() => parseSqlitePlan(results), [results]);
  const [copied, setCopied] = useState(false);

  const hasWarnings = steps.some((s) => getWarnings(s.detail).length > 0);
  const usesIndex = steps.some(
    (s) =>
      s.detail.toUpperCase().includes("INDEX") ||
      s.detail.toUpperCase().includes("USING"),
  );
  const hasFullScan = steps.some(
    (s) =>
      s.detail.toUpperCase().includes("SCAN") &&
      !s.detail.toUpperCase().includes("INDEX") &&
      !s.detail.toUpperCase().includes("SEARCH"),
  );

  // Simple performance score
  const score = hasFullScan ? (usesIndex ? 60 : 40) : usesIndex ? 95 : 80;
  const grade =
    score >= 90 ? "A" : score >= 80 ? "B" : score >= 60 ? "C" : "D";
  const gradeColor =
    score >= 80
      ? "text-emerald-400"
      : score >= 60
        ? "text-amber-400"
        : "text-red-400";
  const gradeBg =
    score >= 80
      ? "bg-emerald-500/10 border-emerald-500/20"
      : score >= 60
        ? "bg-amber-500/10 border-amber-500/20"
        : "bg-red-500/10 border-red-500/20";

  return (
    <div className="h-full overflow-auto p-4 space-y-4">
      {/* ── Score Header ──────────────────────────────────────────────── */}
      <div className="flex items-center gap-4">
        <div
          className={`flex items-center gap-3 px-4 py-3 rounded-xl border ${gradeBg}`}
        >
          <Gauge className={`w-5 h-5 ${gradeColor}`} />
          <div>
            <div className="flex items-center gap-2">
              <span className={`text-xl font-bold ${gradeColor}`}>
                {grade}
              </span>
              <span className="text-xs text-zinc-400">
                Score: {score}/100
              </span>
            </div>
            <div className="text-xxs text-zinc-500">
              {score >= 80
                ? "Good performance"
                : score >= 60
                  ? "Room for optimization"
                  : "Needs optimization"}
            </div>
          </div>
        </div>

        <div className="flex items-center gap-3">
          {usesIndex && (
            <div className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-emerald-500/10 text-emerald-400 text-xs">
              <CheckCircle2 className="w-3.5 h-3.5" />
              Uses Index
            </div>
          )}
          {hasFullScan && (
            <div className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-amber-500/10 text-amber-400 text-xs">
              <AlertTriangle className="w-3.5 h-3.5" />
              Full Table Scan
            </div>
          )}
          <div className="text-xs text-zinc-500">
            {steps.length} step{steps.length !== 1 ? "s" : ""} ·{" "}
            {results.elapsed_ms}ms
          </div>
        </div>

        <div className="ml-auto">
          <button
            onClick={() => {
              const lines: string[] = [
                `Query Plan Analysis — Grade: ${grade} (${score}/100)`,
                `Elapsed: ${results.elapsed_ms}ms · ${steps.length} step${steps.length !== 1 ? "s" : ""}`,
                usesIndex ? "✅ Uses index" : "",
                hasFullScan ? "⚠️  Full table scan" : "",
                "",
                "Plan:",
                ...steps.map(
                  (s, i) =>
                    `${"  ".repeat(s.indent)}${i + 1}. ${s.detail}`,
                ),
              ];
              const allWarnings = steps.flatMap((s) => getWarnings(s.detail));
              if (allWarnings.length > 0) {
                lines.push("", "Warnings:");
                allWarnings.forEach((w) => lines.push(`  • ${w}`));
              }
              navigator.clipboard.writeText(
                lines.filter((l) => l !== "").join("\n"),
              );
              setCopied(true);
              setTimeout(() => setCopied(false), 2000);
            }}
            className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
              bg-zinc-800 hover:bg-zinc-700 text-zinc-400 hover:text-zinc-200 transition-all"
          >
            {copied ? (
              <Check className="w-3.5 h-3.5 text-emerald-500" />
            ) : (
              <Copy className="w-3.5 h-3.5" />
            )}
            {copied ? "Copied" : "Copy"}
          </button>
        </div>
      </div>

      {/* ── Plan Tree ─────────────────────────────────────────────────── */}
      <div className="bg-surface rounded-xl border border-zinc-800 overflow-hidden">
        <div className="px-4 py-3 border-b border-zinc-800 bg-surface-100">
          <h3 className="text-xs font-semibold text-zinc-400 uppercase tracking-wider flex items-center gap-1.5">
            <TreePine className="w-3.5 h-3.5" />
            Query Execution Plan
          </h3>
        </div>
        <div className="divide-y divide-zinc-800/30">
          {steps.map((step, i) => {
            const style = getNodeStyle(step.detail);
            const warnings = getWarnings(step.detail);
            const NodeIcon = style.icon;

            return (
              <div
                key={i}
                className="px-4 py-3 hover:bg-zinc-800/20 transition-colors"
                style={{ paddingLeft: `${16 + step.indent * 24}px` }}
              >
                <div className="flex items-start gap-3">
                  {/* Connector line for nested items */}
                  {step.indent > 0 && (
                    <div className="flex items-center gap-1 text-zinc-700 flex-shrink-0 mr-1">
                      {"└─".repeat(1)}
                    </div>
                  )}

                  {/* Node icon */}
                  <div
                    className={`w-7 h-7 rounded-lg ${style.bgColor} flex items-center justify-center flex-shrink-0`}
                  >
                    <NodeIcon className={`w-3.5 h-3.5 ${style.color}`} />
                  </div>

                  {/* Content */}
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2">
                      <span
                        className={`text-xxs font-semibold uppercase tracking-wider ${style.color}`}
                      >
                        {style.label}
                      </span>
                      <span className="text-xxs text-zinc-600">
                        Step {i + 1}
                      </span>
                    </div>
                    <div className="text-sm text-zinc-200 font-mono mt-0.5">
                      {step.detail}
                    </div>

                    {/* Warnings */}
                    {warnings.map((w, wi) => (
                      <div
                        key={wi}
                        className="flex items-center gap-1.5 mt-1.5 text-xxs text-amber-400"
                      >
                        <AlertTriangle className="w-3 h-3" />
                        {w}
                      </div>
                    ))}
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      </div>

      {/* ── Warnings & Recommendations ─────────────────────────────── */}
      {hasWarnings && (
        <div className="bg-surface rounded-xl border border-zinc-800 overflow-hidden">
          <div className="px-4 py-3 border-b border-zinc-800 bg-surface-100">
            <h3 className="text-xs font-semibold text-amber-400 uppercase tracking-wider flex items-center gap-1.5">
              <AlertTriangle className="w-3.5 h-3.5" />
              Optimization Hints
            </h3>
          </div>
          <div className="p-4 space-y-2">
            {steps
              .flatMap((s) =>
                getWarnings(s.detail).map((w) => ({ step: s, warning: w })),
              )
              .map(({ step, warning }, i) => (
                <div
                  key={i}
                  className="flex items-start gap-3 text-sm text-zinc-300"
                >
                  <span className="text-amber-500 mt-0.5">•</span>
                  <div>
                    <span>{warning}</span>
                    {step.detail.match(/\b(\w+)\b/)?.[1] && (
                      <span className="text-zinc-500 text-xs ml-2">
                        on {step.detail.match(/(?:SCAN|SEARCH)\s+(\w+)/i)?.[1] ?? "table"}
                      </span>
                    )}
                  </div>
                </div>
              ))}
          </div>
        </div>
      )}

      {/* ── Raw Plan Data ──────────────────────────────────────────── */}
      <details className="bg-surface rounded-xl border border-zinc-800 overflow-hidden">
        <summary className="px-4 py-3 cursor-pointer text-xs font-semibold text-zinc-500 uppercase tracking-wider hover:text-zinc-300 transition-colors">
          Raw Plan Data
        </summary>
        <div className="px-4 pb-4">
          <table className="w-full text-xs font-mono">
            <thead>
              <tr>
                {results.columns.map((col, i) => (
                  <th
                    key={i}
                    className="text-left px-2 py-1.5 text-zinc-500 font-semibold border-b border-zinc-800"
                  >
                    {col}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {results.rows.map((row, ri) => (
                <tr
                  key={ri}
                  className="border-b border-zinc-800/30 hover:bg-zinc-800/20"
                >
                  {row.map((cell, ci) => (
                    <td key={ci} className="px-2 py-1.5 text-zinc-300">
                      {cell}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </details>
    </div>
  );
}
