import * as monaco from "monaco-editor";
import EditorWorker from "monaco-editor/editor/editor.worker.js?worker&inline";
import JsonWorker from "monaco-editor/languages/features/json/json.worker.js?worker&inline";
import CssWorker from "monaco-editor/languages/features/css/css.worker.js?worker&inline";
import HtmlWorker from "monaco-editor/languages/features/html/html.worker.js?worker&inline";
import TsWorker from "monaco-editor/languages/features/typescript/ts.worker.js?worker&inline";

// Bundled blob workers keep diff computation off the UI thread without a server/CDN.
self.MonacoEnvironment = { getWorker: (_id, label) => {
  if (label === "json") return new JsonWorker();
  if (["css", "scss", "less"].includes(label)) return new CssWorker();
  if (["html", "handlebars", "razor"].includes(label)) return new HtmlWorker();
  if (["typescript", "javascript"].includes(label)) return new TsWorker();
  return new EditorWorker();
} };
// This is a single-file editor, not a project language server. Avoid false
// missing-import/type diagnostics for files that have not been loaded here.
monaco.typescript.typescriptDefaults.setDiagnosticsOptions({ noSemanticValidation: true });
monaco.typescript.javascriptDefaults.setDiagnosticsOptions({ noSemanticValidation: true });
monaco.editor.defineTheme("blackholes-dark", {
  base: "vs-dark", inherit: true, rules: [],
  colors: { "editor.background": "#17191d", "editorLineNumber.foreground": "#636b79", "editor.lineHighlightBackground": "#ffffff05", "editorGutter.background": "#17191d" },
});
monaco.editor.defineTheme("blackholes-light", {
  base: "vs", inherit: true, rules: [],
  colors: { "editor.background": "#ffffff", "editorLineNumber.foreground": "#a0a5ae", "editor.lineHighlightBackground": "#f6f7f9", "editorGutter.background": "#ffffff" },
});
Object.assign(window, { blackholesMonaco: monaco });
