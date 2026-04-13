import { useCallback, useEffect, useRef } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { sql, PostgreSQL } from "@codemirror/lang-sql";
import { keymap } from "@codemirror/view";

interface EditorProps {
  sql: string;
  onChange: (sql: string) => void;
  onRun: () => void;
  onExplain: () => void;
  isRunning: boolean;
}

// Dark theme matching our app
const darkTheme = {
  "&": {
    backgroundColor: "#18181b",
    color: "#e4e4e7",
  },
  ".cm-content": {
    caretColor: "#3b82f6",
    fontFamily: "'JetBrains Mono', 'SF Mono', 'Fira Code', monospace",
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
};

import { EditorView } from "@codemirror/view";
const themeExtension = EditorView.theme(darkTheme);

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

  const extensions = useCallback(() => {
    return [
      sql({ dialect: PostgreSQL }),
      themeExtension,
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
      EditorView.lineWrapping,
    ];
  }, []);

  return (
    <div className="h-full flex flex-col bg-surface">
      {/* ── Hint Bar ──────────────────────────────────────────────────── */}
      <div className="flex items-center justify-between px-3 py-1 border-b border-zinc-800/50 bg-surface-100">
        <span className="text-xxs text-zinc-600">SQL Editor</span>
        <div className="flex items-center gap-3 text-xxs text-zinc-600">
          <span>
            <kbd className="px-1 py-0.5 rounded bg-zinc-800 text-zinc-400 font-mono">
              ⌘↵
            </kbd>{" "}
            Run
          </span>
          <span>
            <kbd className="px-1 py-0.5 rounded bg-zinc-800 text-zinc-400 font-mono">
              ⌘⇧↵
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
          extensions={extensions()}
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
