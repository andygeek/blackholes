import { readFileSync } from "node:fs";
import { spawn } from "node:child_process";
import { JsonRpcProcess } from "./json-rpc.mjs";
import { packageBinary, providerEnvironment } from "./runtime.mjs";

// Account metadata only: no prompts, threads, tools, or billable generations.
const request = JSON.parse(readFileSync(0, "utf8"));
const environment = providerEnvironment(request);
const abortController = new AbortController();
let dispose = () => {};
let timer;
const empty = { subscription_type: null, rate_limits_available: false, windows: [] };
const percentage = value => typeof value === "number" && Number.isFinite(value) ? Math.min(100, Math.max(0, value)) : null;

async function claudeUsage() {
  const { query } = await import("@anthropic-ai/claude-agent-sdk");
  let finishInput;
  const input = { [Symbol.asyncIterator]: () => ({ next: () => new Promise(resolve => { finishInput = resolve; }) }) };
  const agent = query({ prompt: input, options: {
    env: environment, persistSession: false, settingSources: [], tools: [],
    mcpServers: {}, strictMcpConfig: true, abortController, stderr: () => {},
  } });
  dispose = () => { finishInput?.({ done: true }); agent.close(); };
  const usage = await agent.usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET();
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
  const child = spawn(packageBinary("codex"), ["app-server", "--stdio"], {
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
  // Do not forward credentials or raw SDK diagnostics.
  process.exitCode = 1;
} finally {
  clearTimeout(timer);
  dispose();
  abortController.abort();
}
