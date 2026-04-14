import { useEffect, useMemo, useRef } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { sql, PostgreSQL } from "@codemirror/lang-sql";
import { keymap, EditorView } from "@codemirror/view";
import { Prec } from "@codemirror/state";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags } from "@lezer/highlight";

interface EditorProps {
  sql: string;
  onChange: (sql: string) => void;
  onRun: () => void;
  onExplain: () => void;
  isRunning: boolean;
}

// ── Dark syntax highlighting for SQL ─────────────────────────────────────────
const darkSqlHighlighting = HighlightStyle.define([
  { tag: tags.keyword, color: "#c084fc", fontWeight: "600" },
  { tag: tags.string, color: "#4ade80" },
  { tag: tags.number, color: "#fbbf24" },
  { tag: tags.comment, color: "#71717a", fontStyle: "italic" },
  { tag: tags.lineComment, color: "#71717a", fontStyle: "italic" },
  { tag: tags.blockComment, color: "#71717a", fontStyle: "italic" },
  { tag: tags.operator, color: "#60a5fa" },
  { tag: tags.punctuation, color: "#a1a1aa" },
  { tag: tags.paren, color: "#a1a1aa" },
  { tag: tags.squareBracket, color: "#a1a1aa" },
  { tag: tags.brace, color: "#a1a1aa" },
  { tag: tags.variableName, color: "#e4e4e7" },
  { tag: tags.typeName, color: "#67e8f9" },
  { tag: tags.bool, color: "#fb923c" },
  { tag: tags.null, color: "#fb923c" },
  { tag: tags.special(tags.string), color: "#34d399" },
  { tag: tags.definition(tags.variableName), color: "#f9a8d4" },
  { tag: tags.name, color: "#e4e4e7" },
  { tag: tags.separator, color: "#a1a1aa" },
  { tag: tags.labelName, color: "#fbbf24" },
]);

// ── Additional dark theme tweaks ─────────────────────────────────────────────
const darkThemeOverrides = EditorView.theme({
  ".cm-content": {
    caretColor: "#3b82f6",
    fontFamily: "'JetBrains Mono', 'SF Mono', 'Fira Code', monospace",
    fontSize: "13px",
    lineHeight: "1.6",
  },
  "&.cm-focused .cm-cursor": {
    borderLeftColor: "#3b82f6",
  },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": {
    backgroundColor: "rgba(59, 130, 246, 0.25) !important",
  },
  ".cm-gutters": {
    backgroundColor: "#18181b",
    color: "#52525b",
    border: "none",
    borderRight: "1px solid #27272a",
  },
  ".cm-activeLineGutter": {
    backgroundColor: "#27272a",
    color: "#a1a1aa",
  },
  ".cm-activeLine": {
    backgroundColor: "rgba(39, 39, 42, 0.4)",
  },
  ".cm-matchingBracket": {
    backgroundColor: "rgba(59, 130, 246, 0.2)",
    outline: "1px solid rgba(59, 130, 246, 0.4)",
  },
  ".cm-tooltip.cm-tooltip-autocomplete": {
    backgroundColor: "#1e1e22",
    border: "1px solid #3f3f46",
  },
  ".cm-tooltip.cm-tooltip-autocomplete ul li[aria-selected]": {
    backgroundColor: "rgba(59, 130, 246, 0.2)",
  },
});

export function Editor({
  sql: value,
  onChange,
  onRun,
  onExplain,
  isRunning,
}: EditorProps) {
  const runRef = useRef(onRun);
  const explainRef = useRef(onExplain);

  useEffect(() => {
    runRef.current = onRun;
    explainRef.current = onExplain;
  }, [onRun, onExplain]);

  // Stable extensions — using refs for callbacks to avoid re-creating
  const extensions = useMemo(
    () => [
      sql({ dialect: PostgreSQL }),
      syntaxHighlighting(darkSqlHighlighting),
      darkThemeOverrides,
      // High-priority keymap so it overrides basicSetup bindings
      Prec.highest(
        keymap.of([
          {
            key: "Ctrl-Enter",
            mac: "Cmd-Enter",
            run: () => {
              runRef.current();
              return true;
            },
          },
          {
            key: "Ctrl-Shift-Enter",
            mac: "Cmd-Shift-Enter",
            run: () => {
              explainRef.current();
              return true;
            },
          },
        ]),
      ),
      EditorView.lineWrapping,
    ],
    [],
  );

  const isMac =
    typeof navigator !== "undefined" &&
    /Mac|iPod|iPhone|iPad/.test(navigator.userAgent);
  const modKey = isMac ? "⌘" : "Ctrl";

  return (
    <div className="h-full flex flex-col bg-surface">
      {/* ── Hint Bar ──────────────────────────────────────────────────── */}
      <div className="flex items-center justify-between px-3 py-1 border-b border-zinc-800/50 bg-surface-100">
        <span className="text-xxs text-zinc-600 font-medium">SQL Editor</span>
        <div className="flex items-center gap-3 text-xxs text-zinc-500">
          <span>
            <kbd className="px-1 py-0.5 rounded bg-zinc-800 text-zinc-400 font-mono text-[10px]">
              {modKey}+↵
            </kbd>{" "}
            Run
          </span>
          <span>
            <kbd className="px-1 py-0.5 rounded bg-zinc-800 text-zinc-400 font-mono text-[10px]">
              {modKey}+⇧+↵
            </kbd>{" "}
            Explain
          </span>
        </div>
      </div>

      {/* ── CodeMirror ────────────────────────────────────────────────── */}
      <div className="flex-1 overflow-hidden">
        <CodeMirror
          value={value}
          onChange={onChange}
          theme="dark"
          extensions={extensions}
          basicSetup={{
            lineNumbers: true,
            highlightActiveLineGutter: true,
            highlightActiveLine: true,
            foldGutter: true,
            bracketMatching: true,
            closeBrackets: true,
            autocompletion: true,
            indentOnInput: true,
            tabSize: 2,
          }}
          editable={!isRunning}
          className="h-full"
        />
      </div>
    </div>
  );
}
