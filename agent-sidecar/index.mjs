import { createInterface } from "node:readline";
import { runClaude } from "./providers/claude.mjs";
import { runCodex } from "./providers/codex.mjs";
import { runGemini } from "./providers/gemini.mjs";
import { runOpenCode } from "./providers/opencode.mjs";

const adapters = {
  claude: runClaude,
  codex: runCodex,
  gemini: runGemini,
  opencode: runOpenCode,
};

const abortController = new AbortController();
let activeStopper = null;
let activeController = null;
const queuedControls = [];
let controlChain = Promise.resolve();
let stopRequested = false;

const emit = (message) => process.stdout.write(`${JSON.stringify(message)}\n`);
const stopAgent = () => {
  if (stopRequested) return;
  stopRequested = true;
  abortController.abort();
  try {
    activeStopper?.();
  } catch {
    // Rust owns the process group and force-stops descendants if needed.
  }
};

const dispatchControl = (control) => {
  if (!activeController) {
    queuedControls.push(control);
    return;
  }
  controlChain = controlChain
    .then(() => activeController(control))
    .catch((error) => emit({
      type: "diagnostic",
      message: error instanceof Error ? error.message : String(error),
    }));
};

const setController = (controller) => {
  activeController = controller;
  for (const control of queuedControls.splice(0)) dispatchControl(control);
};

process.once("SIGTERM", stopAgent);
process.once("SIGINT", stopAgent);

const run = async () => {
  const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
  let resolveRequest;
  let rejectRequest;
  const firstRequest = new Promise((resolve, reject) => {
    resolveRequest = resolve;
    rejectRequest = reject;
  });
  let receivedRequest = false;
  lines.on("line", (line) => {
    if (!line.trim()) return;
    try {
      const message = JSON.parse(line);
      if (!receivedRequest) {
        receivedRequest = true;
        resolveRequest(message);
      } else {
        dispatchControl(message);
      }
    } catch (error) {
      if (!receivedRequest) rejectRequest(error);
      else emit({ type: "diagnostic", message: `Invalid runtime control: ${error.message || error}` });
    }
  });
  lines.once("close", () => {
    if (!receivedRequest) rejectRequest(new Error("The agent runtime received an empty request."));
  });

  const request = await firstRequest;
  const adapter = adapters[request.provider];
  if (!adapter) throw new Error(`Unsupported agent provider: ${request.provider}`);

  emit({ type: "started", request_id: request.request_id });
  await adapter({
    request,
    emit,
    signal: abortController.signal,
    setStopper: (stopper) => { activeStopper = stopper; },
    setController,
  });
  lines.close();
  process.stdin.pause();
};

run().catch((error) => {
  if (stopRequested) {
    process.exitCode = 0;
    return;
  }
  emit({
    type: "error",
    message: error instanceof Error ? error.message : String(error),
  });
  process.exitCode = 1;
});
