import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
} from "react";
import CodeMirror from "@uiw/react-codemirror";
import { schemaCompletionSource } from "@codemirror/lang-sql";
import {
  acceptCompletion,
  autocompletion,
  completionStatus,
  startCompletion,
} from "@codemirror/autocomplete";
import { keymap, EditorView, type ViewUpdate } from "@codemirror/view";
import { Prec } from "@codemirror/state";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags } from "@lezer/highlight";
import {
  buildKeywordCompletionSource,
  createColumnCompletionSource,
  createSqlCompletionConfig,
  getSqlDialect,
} from "../sqlAutocomplete";
import {
  resolveSqlExecutionTarget,
  type SqlExecutionTarget,
} from "../sqlExecution";

export interface EditorHandle {
  getExecutionTarget: () => SqlExecutionTarget | null;
}

interface EditorProps {
  sql: string;
  tables: string[];
  databaseType?: string;
  onChange: (sql: string) => void;
  onRun: (sqlOverride?: string) => void;
  onExplain: (sqlOverride?: string) => void;
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

export const Editor = forwardRef<EditorHandle, EditorProps>(function Editor(
  {
    sql: value,
    tables,
    databaseType,
    onChange,
    onRun,
    onExplain,
    isRunning,
  },
  ref,
) {
  const viewRef = useRef<EditorView | null>(null);
  const runRef = useRef(onRun);
  const explainRef = useRef(onExplain);
  const [executionTarget, setExecutionTarget] = useState<SqlExecutionTarget | null>(
    resolveSqlExecutionTarget(value, value.length, value.length),
  );

  useEffect(() => {
    runRef.current = onRun;
    explainRef.current = onExplain;
  }, [onRun, onExplain]);

  const updateExecutionTarget = (next: SqlExecutionTarget | null) => {
    setExecutionTarget((previous) => {
      if (
        previous?.mode === next?.mode &&
        previous?.label === next?.label &&
        previous?.sql === next?.sql &&
        previous?.statementIndex === next?.statementIndex &&
        previous?.statementCount === next?.statementCount
      ) {
        return previous;
      }
      return next;
    });
  };

  const getExecutionTarget = () => {
    const view = viewRef.current;
    if (!view) {
      return resolveSqlExecutionTarget(value, value.length, value.length);
    }

    const selection = view.state.selection.main;
    return resolveSqlExecutionTarget(
      view.state.doc.toString(),
      selection.from,
      selection.to,
    );
  };

  useImperativeHandle(
    ref,
    () => ({
      getExecutionTarget,
    }),
    [value],
  );

  useEffect(() => {
    if (!viewRef.current) {
      updateExecutionTarget(resolveSqlExecutionTarget(value, value.length, value.length));
    }
  }, [value]);

  const isMac =
    typeof navigator !== "undefined" &&
    /Mac|iPod|iPhone|iPad/.test(navigator.userAgent);
  const modKey = isMac ? "⌘" : "Ctrl";

  const sqlCompletionConfig = useMemo(
    () => createSqlCompletionConfig(tables, databaseType),
    [tables, databaseType],
  );
  const dialect = useMemo(() => getSqlDialect(databaseType), [databaseType]);
  const columnCompletionSource = useMemo(
    () => createColumnCompletionSource(tables),
    [tables],
  );
  const schemaCompletion = useMemo(
    () => schemaCompletionSource(sqlCompletionConfig),
    [sqlCompletionConfig],
  );
  const keywordCompletion = useMemo(
    () => buildKeywordCompletionSource(dialect),
    [dialect],
  );

  // Stable extensions — using refs for callbacks to avoid re-creating
  const extensions = useMemo(
    () => [
      dialect.extension,
      syntaxHighlighting(darkSqlHighlighting),
      darkThemeOverrides,
      autocompletion({
        override: [columnCompletionSource, schemaCompletion, keywordCompletion],
        activateOnTyping: true,
        maxRenderedOptions: 200,
      }),
      // High-priority keymap so it overrides basicSetup bindings
      Prec.highest(
        keymap.of([
          {
            key: "Tab",
            run: (view) => {
              if (completionStatus(view.state) === "active") {
                return acceptCompletion(view);
              }

              if (startCompletion(view)) {
                return true;
              }

              return false;
            },
          },
          {
            key: "Ctrl-Enter",
            mac: "Cmd-Enter",
            run: () => {
              const target = getExecutionTarget();
              runRef.current(target?.sql);
              return true;
            },
          },
          {
            key: "Ctrl-Shift-Enter",
            mac: "Cmd-Shift-Enter",
            run: () => {
              const target = getExecutionTarget();
              explainRef.current(target?.sql);
              return true;
            },
          },
        ]),
      ),
      EditorView.lineWrapping,
    ],
    [columnCompletionSource, dialect.extension, keywordCompletion, schemaCompletion],
  );

  const handleUpdate = (update: ViewUpdate) => {
    if (!update.docChanged && !update.selectionSet) return;

    const selection = update.state.selection.main;
    updateExecutionTarget(
      resolveSqlExecutionTarget(
        update.state.doc.toString(),
        selection.from,
        selection.to,
      ),
    );
  };

  return (
    <div className="h-full flex flex-col bg-surface">
      {/* ── Hint Bar ──────────────────────────────────────────────────── */}
      <div className="flex items-center justify-between gap-3 px-3 py-1.5 border-b border-zinc-800/50 bg-surface-100">
        <div className="min-w-0 flex items-center gap-2">
          <span className="text-xxs text-zinc-600 font-medium">SQL Workspace</span>
          {executionTarget && (
            <span className="inline-flex items-center rounded-full border border-zinc-700 bg-surface-300 px-2 py-0.5 text-[10px] font-medium text-zinc-400 truncate max-w-[220px]">
              {executionTarget.label}
            </span>
          )}
        </div>
        <div className="flex items-center gap-3 text-xxs text-zinc-500 flex-wrap justify-end">
          <span>
            <kbd className="px-1 py-0.5 rounded bg-zinc-800 text-zinc-400 font-mono text-[10px]">
              Tab
            </kbd>{" "}
            Autocomplete
          </span>
          <span>
            <kbd className="px-1 py-0.5 rounded bg-zinc-800 text-zinc-400 font-mono text-[10px]">
              {modKey}+F
            </kbd>{" "}
            Find
          </span>
          <span>
            <kbd className="px-1 py-0.5 rounded bg-zinc-800 text-zinc-400 font-mono text-[10px]">
              {modKey}+↵
            </kbd>{" "}
            Run current
          </span>
          <span>
            <kbd className="px-1 py-0.5 rounded bg-zinc-800 text-zinc-400 font-mono text-[10px]">
              {modKey}+⇧+↵
            </kbd>{" "}
            Explain current
          </span>
        </div>
      </div>

      {/* ── CodeMirror ────────────────────────────────────────────────── */}
      <div className="flex-1 overflow-hidden">
        <CodeMirror
          value={value}
          onChange={onChange}
          onCreateEditor={(view) => {
            viewRef.current = view;
            const selection = view.state.selection.main;
            updateExecutionTarget(
              resolveSqlExecutionTarget(
                view.state.doc.toString(),
                selection.from,
                selection.to,
              ),
            );
          }}
          onUpdate={handleUpdate}
          theme="dark"
          extensions={extensions}
          basicSetup={{
            lineNumbers: true,
            highlightActiveLineGutter: true,
            highlightActiveLine: true,
            foldGutter: true,
            bracketMatching: true,
            closeBrackets: true,
            autocompletion: false,
            indentOnInput: true,
            tabSize: 2,
          }}
          editable={!isRunning}
          className="h-full"
        />
      </div>
    </div>
  );
});
