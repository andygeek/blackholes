import { createInterface } from "node:readline";
import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { spawn } from "node:child_process";
import { JsonRpcProcess } from "./json-rpc.mjs";
import { installedAgentBinary, providerEnvironment } from "./environment.mjs";

// Account metadata only: no prompts, threads, tools, or billable generations.
const request = JSON.parse(readFileSync(0, "utf8"));
const environment = providerEnvironment(request);
const abortController = new AbortController();
let dispose = () => {};
let timer;
const empty = { subscription_type: null, rate_limits_available: false, windows: [] };
const percentage = value => typeof value === "number" && Number.isFinite(value) ? Math.min(100, Math.max(0, value)) : null;

async function claudeUsage() {
  // Read the CLI's control protocol without sending a user message or starting a turn.
  const child = spawn(installedAgentBinary("claude"), [
    "--print", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
    "--no-session-persistence", "--setting-sources", "", "--tools", "",
    "--strict-mcp-config", "--mcp-config", JSON.stringify({ mcpServers: {} }),
  ], { env: environment, stdio: ["pipe", "pipe", "pipe"], signal: abortController.signal });
  const pending = new Map();
  const lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
  const fail = () => {
    for (const { reject } of pending.values()) reject(new Error("Usage query unavailable"));
    pending.clear();
  };
  child.on("error", fail);
  child.stdin.on("error", fail);
  child.on("exit", fail);
  child.stderr.resume();
  lines.on("line", line => {
    if (line.length > 65536) { fail(); return; }
    let event;
    try { event = JSON.parse(line); } catch { return; }
    if (event.type !== "control_response") return;
    const response = event.response;
    const request = pending.get(response?.request_id);
    if (!request) return;
    pending.delete(response.request_id);
    if (response.subtype === "success") request.resolve(response.response);
    else request.reject(new Error("Usage query unavailable"));
  });
  const requestControl = request => new Promise((resolve, reject) => {
    const request_id = randomUUID();
    pending.set(request_id, { resolve, reject });
    child.stdin.write(JSON.stringify({ type: "control_request", request_id, request }) + "\n", error => {
      if (error) { pending.delete(request_id); reject(error); }
    });
  });
  dispose = () => { fail(); lines.close(); child.stdin.destroy(); child.kill(); };
  await requestControl({ subtype: "initialize", hooks: {}, sdkMcpServers: [] });
  const usage = await requestControl({ subtype: "get_usage" });
  const windows = [];
  for (const [key, minutes, label] of [
    ["five_hour", 300, ""], ["seven_day", 10080, ""],
    ["seven_day_opus", 10080, "Opus"], ["seven_day_sonnet", 10080, "Sonnet"],
    ["seven_day_oauth_apps", 10080, "OAuth apps"],
  ]) {
    const window = usage.rate_limits?.[key];
    if (window) windows.push({ label, minutes, utilization: percentage(window.utilization), resets_at: window.resets_at ?? null });
  }
  return { subscription_type: usage.subscription_type ?? null, rate_limits_available: windows.some(w => w.utilization !== null), windows };
}

async function codexUsage() {
  const child = spawn(installedAgentBinary("codex"), ["app-server", "--stdio"], {
    env: environment, stdio: ["pipe", "pipe", "pipe"], signal: abortController.signal,
  });
  const rpc = new JsonRpcProcess(child, { onRequest: async () => { throw new Error("Interactive requests disabled"); } });
  dispose = () => rpc.stop();
  await rpc.request("initialize", { clientInfo: { name: "blackholes_usage", version: "1" }, capabilities: {} });
  rpc.notify("initialized", {});
  const { account, requiresOpenaiAuth } = await rpc.request("account/read", { refreshToken: false });
  if (!account && requiresOpenaiAuth !== false) throw new Error("Authentication required");
  if (account?.type !== "chatgpt") return { ...empty, subscription_type: account?.type === "apiKey" ? "API" : null };
  const report = await rpc.request("account/rateLimits/read", {});
  const buckets = report.rateLimitsByLimitId && Object.keys(report.rateLimitsByLimitId).length
    ? Object.values(report.rateLimitsByLimitId) : [report.rateLimits].filter(Boolean);
  const windows = buckets.flatMap(bucket => [bucket.primary, bucket.secondary].filter(Boolean).map(window => {
    const reset = typeof window.resetsAt === "number" ? new Date(window.resetsAt * 1000) : null;
    return {
      label: bucket.limitName || (bucket.limitId === "codex" ? "" : bucket.limitId) || "",
      minutes: window.windowDurationMins ?? null,
      utilization: percentage(window.usedPercent),
      resets_at: reset && Number.isFinite(reset.getTime()) ? reset.toISOString() : null,
    };
  }));
  return { subscription_type: account.planType ?? report.rateLimits?.planType ?? null,
    rate_limits_available: windows.some(w => w.utilization !== null), windows };
}

try {
  const load = { claude: claudeUsage, codex: codexUsage }[request.provider];
  // Other runtimes do not currently expose a supported plan-limit query.
  const usage = load ? await Promise.race([
    load(), new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("Usage timeout")), 12000); }),
  ]) : empty;
  process.stdout.write(JSON.stringify(usage));
} catch {
  // Do not forward credentials or raw provider diagnostics.
  process.exitCode = 1;
} finally {
  clearTimeout(timer);
  dispose();
  abortController.abort();
}
