import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "vite";

const frontendRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const outputDirectory = resolve(frontendRoot, "../assets/generated");
const entries = [
  ["chat", "src/chat/main.tsx", "BlackholesChat"],
  ["navigation", "src/navigation/main.tsx", "BlackholesNavigation"],
  ["quick-open", "src/quick-open/main.tsx", "BlackholesQuickOpen"],
  ["editor", "src/chat/editor-runtime.ts", "BlackholesEditor"],
];

for (const [name, entry, globalName] of entries) {
  await build({
    root: frontendRoot,
    publicDir: false,
    logLevel: "info",
    worker: { format: "iife" },
    plugins: [{
      name: "embedded-monaco-workers",
      enforce: "pre",
      transform(source, id) {
        const language = id.match(/monaco-editor\/esm\/vs\/languages\/features\/(json|css|html|typescript)\/workerManager\.js$/)?.[1];
        if (!language) return;
        // Monaco's default ESM URLs are not meaningful inside embedded IIFEs.
        // Route those fallback factories through our bundled blob workers too.
        const code = source.replace(/new Worker\(new URL\('[^']+\.worker\.js', import\.meta\.url\), \{ type: "module" \}\)/g,
          `globalThis.MonacoEnvironment.getWorker("workerMain.js", "${language}")`);
        if (code === source) throw new Error(`Monaco ${language} worker factory changed; update the embedded adapter`);
        return { code, map: null };
      },
    }],
    define: {
      "process.env.NODE_ENV": JSON.stringify("production"),
    },
    build: {
      target: "safari16",
      outDir: outputDirectory,
      emptyOutDir: name === "chat",
      minify: true,
      sourcemap: false,
      lib: {
        entry: resolve(frontendRoot, entry),
        formats: ["iife"],
        name: globalName,
        cssFileName: name,
        fileName: () => `${name}.js`,
      },
    },
  });
}
