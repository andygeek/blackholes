import { readFileSync } from "node:fs";
import { spawn } from "node:child_process";
import { delimiter, dirname } from "node:path";
import { JsonRpcProcess } from "./json-rpc.mjs";
import { packageBinary, providerEnvironment } from "./runtime.mjs";

// Metadata only. No prompt, generation, tool call or interactive authentication.
// Run in a dedicated process so account environments never leak between providers.
const request = JSON.parse(readFileSync(0, "utf8"));
const environment = providerEnvironment(request);
const abort = new AbortController();
let dispose = () => {};
const timer = setTimeout(() => { abort.abort(); dispose(); }, 20_000);
const entry = (id, label, efforts = [], extra = {}) => ({
  id, label: label || id,
  efforts: [...new Set(efforts.filter(value => typeof value === "string" && value.length < 100))],
  ...extra,
});

async function claudeModels() {
  const { query } = await import("@anthropic-ai/claude-agent-sdk");
  let finish;
  const input = { [Symbol.asyncIterator]: () => ({ next: () => new Promise(resolve => { finish = resolve; }) }) };
  const agent = query({ prompt: input, options: {
    cwd: request.cwd, env: environment, persistSession: false,
    settingSources: ["user", "project", "local"], tools: [], mcpServers: {},
    strictMcpConfig: true, abortController: abort, stderr: () => {},
  } });
  dispose = () => { finish?.({ done: true }); agent.close(); };
  const models = await agent.supportedModels();
  return { models: models.map(model => entry(model.value, model.displayName,
    model.supportsEffort === false ? [] : model.supportedEffortLevels || [],
    { aliases: model.resolvedModel ? [model.resolvedModel] : [] })), default_model: null };
}

function rpcProvider(binary, args) {
  const child = spawn(packageBinary(binary), args, {
    cwd: request.cwd, env: environment, stdio: ["pipe", "pipe", "pipe"], signal: abort.signal,
  });
  const rpc = new JsonRpcProcess(child, {
    onRequest: async () => { throw new Error("Interactive requests are disabled for model discovery"); },
  });
  dispose = () => rpc.stop();
  return rpc;
}

async function codexModels() {
  const rpc = rpcProvider("codex", ["app-server", "--stdio"]);
  await rpc.request("initialize", { clientInfo: { name: "blackholes_models", version: "1" }, capabilities: {} });
  rpc.notify("initialized", {});
  const account = await rpc.request("account/read", { refreshToken: false });
  if (!account.account && account.requiresOpenaiAuth !== false) throw new Error("Authentication required");
  const models = [];
  const seenCursors = new Set();
  let cursor = null;
  let defaultModel = null;
  do {
    const page = await rpc.request("model/list", { limit: 100, cursor, includeHidden: false });
    if (!Array.isArray(page.data)) throw new Error("Invalid model catalog");
    for (const model of page.data) {
      if (model.hidden) continue;
      const id = model.model || model.id;
      models.push(entry(id, model.displayName,
        (model.supportedReasoningEfforts || []).map(effort => effort.reasoningEffort)));
      if (model.isDefault) defaultModel = id;
    }
    cursor = page.nextCursor;
    if (cursor && seenCursors.has(cursor)) throw new Error("Repeated catalog cursor");
    seenCursors.add(cursor);
    if (models.length > 2000) throw new Error("Catalog is too large");
  } while (cursor);
  return { models, default_model: defaultModel };
}

async function geminiModels() {
  const rpc = rpcProvider("gemini", ["--acp"]);
  await rpc.request("initialize", {
    protocolVersion: 1, clientInfo: { name: "blackholes_models", version: "1" },
    clientCapabilities: { auth: { terminal: false }, fs: { readTextFile: false, writeTextFile: false }, terminal: false },
  });
  // ACP exposes the account/configuration-aware picker on session/new.
  // The empty metadata session is never prompted or kept as a Blackholes chat.
  const session = await rpc.request("session/new", { cwd: request.cwd, mcpServers: [] });
  if (!Array.isArray(session.models?.availableModels)) throw new Error("Model discovery unsupported");
  return { models: session.models.availableModels.map(model => entry(
    model.modelId || model.value, model.name || model.title)), default_model: session.models.currentModelId || null };
}

async function openCodeModels() {
  Object.assign(process.env, environment, {
    PATH: `${dirname(packageBinary("opencode"))}${delimiter}${environment.PATH || ""}`,
  });
  const { createOpencode } = await import("@opencode-ai/sdk/v2");
  const runtime = await createOpencode({ signal: abort.signal, timeout: 15_000, port: 0,
    config: { autoupdate: false, share: "disabled" } });
  dispose = () => runtime.server.close();
  const response = await runtime.client.provider.list({ directory: request.cwd });
  if (response.error || !Array.isArray(response.data?.connected)) throw new Error("Provider discovery failed");
  const connected = new Set(response.data.connected);
  const models = response.data.all.filter(provider => connected.has(provider.id)).flatMap(provider =>
    Object.values(provider.models || {}).filter(model => model.status !== "deprecated").map(model =>
      entry(`${provider.id}/${model.id}`, `${model.name || model.id} · ${provider.name || provider.id}`,
        Object.entries(model.variants || {}).filter(([, config]) => !config.disabled).map(([name]) => name))));
  // Provider defaults are not necessarily the active agent's configured model.
  return { models, default_model: null };
}

try {
  const load = { claude: claudeModels, codex: codexModels, gemini: geminiModels, opencode: openCodeModels }[request.provider];
  if (!load) throw new Error("Unsupported provider");
  const result = await load();
  const seen = new Set();
  result.models = result.models.filter(model => {
    if (typeof model.id !== "string" || !model.id || model.id === "automatic" || seen.has(model.id)) return false;
    seen.add(model.id); return true;
  });
  const output = JSON.stringify(result);
  if (Buffer.byteLength(output) > 1024 * 1024) throw new Error("Catalog is too large");
  process.stdout.write(output);
} catch {
  // Never relay raw SDK errors, authentication information or credentials.
  process.exitCode = 1;
} finally {
  clearTimeout(timer);
  dispose();
  abort.abort();
}
