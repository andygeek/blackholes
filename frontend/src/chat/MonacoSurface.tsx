import { useEffect, useRef, useState } from "react";
import type * as Monaco from "monaco-editor";
import { ArrowDown, ArrowUp, Columns2, List, Search, WrapText } from "lucide-react";
import { loadMonaco } from "./monaco-loader";
import { postNative } from "../shared/native";

const views = new Map<string, Monaco.editor.ICodeEditorViewState>();
const languageFor = (file: string) => {
  const extension = file.split(".").pop()?.toLowerCase() || "";
  return ({ ts: "typescript", tsx: "typescript", js: "javascript", jsx: "javascript", mjs: "javascript", cjs: "javascript", json: "json", md: "markdown", rs: "rust", rb: "ruby", py: "python", dart: "dart", go: "go", yml: "yaml", yaml: "yaml", toml: "ini", sh: "shell", bash: "shell", zsh: "shell", css: "css", scss: "scss", html: "html", vue: "html", sql: "sql", java: "java", kt: "kotlin", swift: "swift", c: "c", h: "cpp", cpp: "cpp", xml: "xml" } as Record<string, string>)[extension]
    || (/dockerfile/i.test(file) ? "dockerfile" : "plaintext");
};
const baseOptions: Monaco.editor.IStandaloneEditorConstructionOptions = {
  fontFamily: '"SFMono-Regular", Menlo, Monaco, monospace', fontSize: 12, lineHeight: 20,
  minimap: { enabled: false }, scrollBeyondLastLine: false, smoothScrolling: false,
  padding: { top: 12, bottom: 12 }, lineNumbersMinChars: 4, glyphMargin: false,
  renderLineHighlight: "line", roundedSelection: false, overviewRulerBorder: false,
  folding: true, showFoldingControls: "mouseover", automaticLayout: false,
  bracketPairColorization: { enabled: true }, guides: { indentation: true },
  stickyScroll: { enabled: false }, links: false, wordWrap: "off",
  scrollbar: { verticalScrollbarSize: 10, horizontalScrollbarSize: 10, useShadows: false },
  unicodeHighlight: { ambiguousCharacters: false },
};

export function MonacoSurface({ file, content, original, requestId, theme, language }: {
  file: string; content: string; original?: string; requestId: number;
  theme: "light" | "dark"; language: "en" | "es";
}) {
  const container = useRef<HTMLDivElement>(null);
  const instance = useRef<Monaco.editor.IStandaloneCodeEditor | Monaco.editor.IStandaloneDiffEditor | null>(null);
  const api = useRef<typeof Monaco | null>(null);
  const [error, setError] = useState("");
  const [ready, setReady] = useState(false);
  const [position, setPosition] = useState({ lineNumber: 1, column: 1 });
  const [inline, setInline] = useState(false);
  const [wrap, setWrap] = useState(false);
  const [diffNotice, setDiffNotice] = useState(false);
  const isDiff = original !== undefined;
  const tr = (en: string, es: string) => language === "en" ? en : es;
  const current = useRef({ content, original, file, theme });
  current.current = { content, original, file, theme };
  const codeEditor = () => {
    const value = instance.current;
    return value && ("getModifiedEditor" in value ? value.getModifiedEditor() : value);
  };

  useEffect(() => {
    let disposed = false;
    let cleanup: (() => void) | undefined;
    setReady(false); setError(""); setDiffNotice(false);
    loadMonaco().then(monaco => {
      if (disposed || !container.current) return;
      api.current = monaco;
      const initial = current.current;
      monaco.editor.setTheme(`blackholes-${initial.theme}`);
      const syntax = languageFor(initial.file);
      const basename = initial.file.split("/").pop() || "file.txt";
      const model = monaco.editor.createModel(initial.content, syntax,
        monaco.Uri.from({ scheme: "inmemory", path: `/working/${requestId}/${basename}` }));
      model.updateOptions({ tabSize: 2, insertSpaces: true });
      // Large documents keep rendering/undo, with expensive suggestions disabled.
      const large = initial.content.length > 1024 * 1024;
      const options = { ...baseOptions, quickSuggestions: !large, wordBasedSuggestions: large ? "off" as const : "currentDocument" as const };
      const disposables: Monaco.IDisposable[] = [];
      let before: Monaco.editor.ITextModel | undefined;
      let editor: Monaco.editor.IStandaloneCodeEditor | Monaco.editor.IStandaloneDiffEditor;
      if (initial.original !== undefined) {
        before = monaco.editor.createModel(initial.original, syntax,
          monaco.Uri.from({ scheme: "inmemory", path: `/head/${requestId}/${basename}` }));
        const diff = monaco.editor.createDiffEditor(container.current, {
          ...options, readOnly: true, originalEditable: false, renderSideBySide: true,
          renderSideBySideInlineBreakpoint: 640, enableSplitViewResizing: true,
          hideUnchangedRegions: { enabled: true, contextLineCount: 4, minimumLineCount: 8, revealLineCount: 20 },
          diffAlgorithm: "advanced", maxComputationTime: 2000, maxFileSize: 8,
          renderOverviewRuler: false,
        });
        diff.setModel({ original: before, modified: model });
        disposables.push(diff.onDidUpdateDiff(() => {
          setDiffNotice(diff.getLineChanges() === null);
        }));
        editor = diff;
      } else {
        const code = monaco.editor.create(container.current, { ...options, model });
        const saved = views.get(initial.file);
        if (saved) code.restoreViewState(saved);
        disposables.push(code.onDidChangeModelContent(() => {
          // Send changes before a navigation command can replace the document.
          // Rust owns the debounced, atomic disk save; React never copies file
          // contents into component state on every keystroke.
          postNative({ type: "update_file_content", request_id: requestId, content: model.getValue() });
        }));
        code.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => postNative({ type: "save_active_file" }));
        editor = code;
      }
      instance.current = editor;
      const code = "getModifiedEditor" in editor ? editor.getModifiedEditor() : editor;
      disposables.push(code.onDidChangeCursorPosition(event => setPosition(event.position)));
      let frame = 0;
      const resize = new ResizeObserver(() => {
        cancelAnimationFrame(frame);
        frame = requestAnimationFrame(() => {
          if (container.current) editor.layout({ width: container.current.clientWidth, height: container.current.clientHeight });
        });
      });
      resize.observe(container.current);
      editor.layout(); setReady(true);
      cleanup = () => {
        resize.disconnect(); cancelAnimationFrame(frame);
        if (!before) {
          const state = code.saveViewState();
          if (state) { views.delete(initial.file); views.set(initial.file, state); }
          if (views.size > 12) views.delete(views.keys().next().value!);
        }
        disposables.forEach(value => value.dispose());
        editor.dispose(); model.dispose(); before?.dispose(); instance.current = null;
      };
    }).catch(reason => { if (!disposed) setError(String(reason)); });
    return () => { disposed = true; cleanup?.(); };
  }, [requestId, isDiff]);

  useEffect(() => { api.current?.editor.setTheme(`blackholes-${theme}`); }, [theme]);
  useEffect(() => {
    const diff = instance.current;
    if (!diff || !("getModifiedEditor" in diff)) return;
    const pair = diff.getModel();
    if (pair && pair.modified.getValue() !== content) pair.modified.setValue(content);
    if (pair && original !== undefined && pair.original.getValue() !== original) pair.original.setValue(original);
  }, [content, original]);
  useEffect(() => {
    const editor = instance.current;
    if (!editor) return;
    if ("getModifiedEditor" in editor) editor.updateOptions({ renderSideBySide: !inline, diffWordWrap: wrap ? "on" : "off" });
    else editor.updateOptions({ wordWrap: wrap ? "on" : "off" });
  }, [inline, wrap, ready]);

  return <div className="monaco-surface">
    <div className="code-toolbar">
      <span>{isDiff ? tr("HEAD ↔ Working tree", "HEAD ↔ Cambios locales") : tr("Editor", "Editor")}</span>
      {isDiff && <>
        <button disabled={!ready} title={tr("Previous change", "Cambio anterior")} aria-label={tr("Previous change", "Cambio anterior")}
          onClick={() => { const value = instance.current; if (value && "goToDiff" in value) value.goToDiff("previous"); }}><ArrowUp size={14} /></button>
        <button disabled={!ready} title={tr("Next change", "Siguiente cambio")} aria-label={tr("Next change", "Siguiente cambio")}
          onClick={() => { const value = instance.current; if (value && "goToDiff" in value) value.goToDiff("next"); }}><ArrowDown size={14} /></button>
        <button aria-pressed={inline} title={tr("Toggle inline comparison", "Alternar comparación en línea")} aria-label={tr("Toggle inline comparison", "Alternar comparación en línea")} onClick={() => setInline(!inline)}>{inline ? <List size={14} /> : <Columns2 size={14} />}</button>
      </>}
      <button aria-pressed={wrap} title={tr("Word wrap", "Ajustar líneas")} aria-label={tr("Word wrap", "Ajustar líneas")} onClick={() => setWrap(!wrap)}><WrapText size={14} /></button>
      <button disabled={!ready} title={tr("Find in file · ⌘F", "Buscar en archivo · ⌘F")} aria-label={tr("Find in file", "Buscar en archivo")} onClick={() => codeEditor()?.getAction("actions.find")?.run()}><Search size={14} /></button>
    </div>
    <div className="monaco-container" ref={container} />
    {!ready && <div className={`monaco-loading${error ? " is-error" : ""}`} role="status">{error || tr("Loading editor…", "Cargando editor…")}</div>}
    <footer className="code-statusbar">
      <span>{isDiff ? tr("Read-only comparison", "Comparación de solo lectura") : `Ln ${position.lineNumber}, Col ${position.column}`}</span>
      {diffNotice && <span>{tr("Comparison is not available", "La comparación no está disponible")}</span>}
      <span>UTF-8</span><span>{languageFor(file)}</span>
    </footer>
  </div>;
}
