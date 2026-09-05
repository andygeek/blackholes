import type * as Monaco from "monaco-editor";
let loading: Promise<typeof Monaco> | null = null;
export function loadMonaco(): Promise<typeof Monaco> {
  if (!loading) loading = new Promise((resolve, reject) => {
    // Defer parsing the editor until a file/diff actually needs it.
    requestAnimationFrame(() => {
      try {
        const payload = document.getElementById("editor-runtime-source");
        if (!payload?.textContent) throw new Error("Embedded editor bundle is missing");
        const script = document.createElement("script");
        script.textContent = new TextDecoder().decode(Uint8Array.from(atob(payload.textContent.trim()), char => char.charCodeAt(0)));
        document.head.appendChild(script);
        script.remove();
        const api = (window as unknown as { blackholesMonaco?: typeof Monaco }).blackholesMonaco;
        if (!api) throw new Error("Unable to initialize the editor");
        payload.remove();
        resolve(api);
      } catch (error) { loading = null; reject(error); }
    });
  });
  return loading;
}
