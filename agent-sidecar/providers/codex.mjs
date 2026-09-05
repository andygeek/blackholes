import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { JsonRpcProcess } from "../json-rpc.mjs";
import {
  genericUsage,
  normalizedImages,
  packageBinary,
  promptWithHistory,
  providerEnvironment,
  requestsBackgroundExecution,
  sourceEnvironment,
  systemText,
} from "../runtime.mjs";

const imageExtension = (mediaType) => ({
  "image/png": ".png",
  "image/jpeg": ".jpg",
  "image/gif": ".gif",
  "image/webp": ".webp",
}[mediaType] || ".png");

const turnInput = (text, images, directory, sequence) => {
  const input = [];
  if (String(text || "").trim()) input.push({ type: "text", text: String(text) });
  for (const [index, image] of images.entries()) {
    const path = join(directory, `image-${sequence}-${index}${imageExtension(image.media_type)}`);
    writeFileSync(path, Buffer.from(image.data, "base64"));
    input.push({ type: "localImage", path });
  }
  return input;
};

const toolEvent = (item) => {
  if (item?.type === "commandExecution") {
    return { type: "tool", name: "Bash", input: { command: item.command } };
  }
  if (item?.type === "mcpToolCall") {
    return { type: "tool", name: `mcp__${item.server}__${item.tool}`, input: item.arguments };
  }
  if (item?.type === "dynamicToolCall") {
    return { type: "tool", name: item.tool || "Tool", input: item.arguments };
  }
  if (item?.type === "webSearch") {
    return { type: "tool", name: "WebSearch", input: { query: item.query } };
  }
  if (item?.type === "fileChange") {
    return { type: "tool", name: "Edit", input: { changes: item.changes } };
  }
  return null;
};

export const runCodex = async ({ request, emit, signal, setStopper, setController }) => {
  const temporaryDirectory = mkdtempSync(join(tmpdir(), "blackholes-codex-"));
  const child = spawn(packageBinary("codex"), ["app-server", "--stdio"], {
    cwd: request.cwd,
    env: providerEnvironment(request),
    stdio: ["pipe", "pipe", "pipe"],
  });

  let threadId = null;
  let turnId = null;
  let finalResponse = "";
  let streamedText = false;
  let usage = null;
  let backgroundViolation = null;
  let settleTurn;
  let rejectTurn;
  let imageSequence = 0;
  const turnFinished = new Promise((resolve, reject) => {
    settleTurn = resolve;
    rejectTurn = reject;
  });

  const rpc = new JsonRpcProcess(child, {
    onDiagnostic: (message) => emit({ type: "diagnostic", message }),
    onClose: (error) => rejectTurn(error),
    onRequest: async (method, params) => {
      if (method.includes("requestApproval")) {
        if (requestsBackgroundExecution(params || {})) {
          return { decision: "decline" };
        }
        return { decision: request.full_access ? "acceptForSession" : "decline" };
      }
      throw new Error(`Unsupported Codex app-server request: ${method}`);
    },
    onNotification: (method, params) => {
      if (method === "turn/started" && !turnId) {
        turnId = params.turn?.id || null;
        return;
      }
      if (method === "item/agentMessage/delta" && params.turnId === turnId) {
        const text = params.delta || "";
        if (text) {
          streamedText = true;
          emit({ type: "delta", text });
        }
        return;
      }
      if (method === "item/started" && params.turnId === turnId) {
        if (params.item?.type === "commandExecution") {
          const backgroundRequested = requestsBackgroundExecution({ command: params.item.command || "" });
          emit({
            type: "background_task",
            task_id: params.item.id || `codex-command-${Date.now()}`,
            status: backgroundRequested ? "blocked" : "foreground",
            description: params.item.command || "",
            task_type: "local_bash",
            summary: backgroundRequested ? "Blocked by Blackholes" : "",
            output_file: "",
            ambient: false,
          });
          if (backgroundRequested) {
            backgroundViolation = "Codex intentó iniciar un proceso en segundo plano. Usa un comando en primer plano o una terminal visible de Blackholes.";
            void rpc.request("turn/interrupt", { threadId, turnId }).catch(() => {});
            rejectTurn(new Error(backgroundViolation));
            return;
          }
        }
        const event = toolEvent(params.item);
        if (event) emit(event);
        return;
      }
      if (method === "item/completed" && params.turnId === turnId) {
        const item = params.item;
        if (item?.type === "commandExecution") {
          const failed = item.status === "failed" || (Number.isFinite(item.exitCode) && item.exitCode !== 0);
          emit({
            type: "background_task",
            task_id: item.id || `codex-command-${Date.now()}`,
            status: failed ? "failed" : "completed",
            description: item.command || "",
            task_type: "local_bash",
            summary: failed ? (item.error?.message || "Command failed") : "",
            output_file: "",
            ambient: false,
          });
        }
        if (item?.type === "agentMessage" && item.text) {
          finalResponse = item.text;
          if (!streamedText) emit({ type: "delta", text: item.text });
        } else if (item?.type === "fileChange") {
          const event = toolEvent(item);
          if (event) emit(event);
        }
        return;
      }
      if (method === "item/commandExecution/outputDelta" && params.turnId === turnId) {
        if (params.delta) emit({ type: "diagnostic", message: params.delta });
        return;
      }
      if (method === "thread/tokenUsage/updated" && params.turnId === turnId) {
        usage = params.tokenUsage?.last || params.tokenUsage?.total || null;
        return;
      }
      if (method === "error" && params.turnId === turnId && !params.willRetry) {
        rejectTurn(new Error(params.error?.message || "Codex runtime failed."));
        return;
      }
      if (method === "turn/completed" && params.turn?.id === turnId) {
        if (params.turn.status === "failed") {
          rejectTurn(new Error(params.turn.error?.message || "Codex turn failed."));
        } else {
          settleTurn(params.turn);
        }
      }
    },
  });

  setStopper(() => rpc.stop());
  signal.addEventListener("abort", () => rpc.stop(), { once: true });

  try {
    await rpc.request("initialize", {
      clientInfo: { name: "blackholes", title: "Blackholes", version: "1" },
      capabilities: { experimentalApi: true },
    });
    rpc.notify("initialized", {});

    const skills = Array.isArray(request.skills) && request.skills.length
      ? `\nEnabled Blackholes skills: ${request.skills.join(", ")}. Their SKILL.md files are under ${request.skills_plugin_path}/skills; read a matching one before using it.`
      : "";
    const mcpServers = {
      blackholes: {
        command: request.blackholes_mcp_command,
        args: ["mcp"],
        env: sourceEnvironment(request),
        required: true,
      },
    };
    for (const server of request.configured_mcp_servers || []) {
      mcpServers[server.name] = server.transport === "http"
        ? {
            url: server.url,
            ...(server.oauth_client_id ? { oauth_client_id: server.oauth_client_id } : {}),
            ...(server.oauth_callback_port ? { oauth_callback_port: server.oauth_callback_port } : {}),
          }
        : {
            command: server.command,
            args: server.args || [],
            env: server.env || {},
          };
    }
    const enabledMcpServers = new Set(request.enabled_mcp_servers || ["blackholes"]);
    for (const name of request.available_mcp_servers || []) {
      if (name !== "blackholes" && !enabledMcpServers.has(name)) mcpServers[name] = { enabled: false };
    }
    const threadParams = {
      model: request.model || null,
      cwd: request.cwd,
      approvalPolicy: request.full_access ? "never" : "on-request",
      sandbox: request.full_access ? "danger-full-access" : "workspace-write",
      serviceName: "blackholes",
      developerInstructions: `${systemText(request)}${skills}`,
      config: {
        mcp_servers: mcpServers,
      },
    };
    const resumed = request.session_id && !Number.isInteger(request.fork_at_user_turn);
    const thread = resumed
      ? await rpc.request("thread/resume", { threadId: request.session_id, ...threadParams })
      : await rpc.request("thread/start", threadParams);
    threadId = thread.thread?.id || request.session_id;
    if (!threadId) throw new Error("Codex app-server did not return a thread id.");
    emit({ type: "session", session_id: threadId });

    const sandboxPolicy = request.full_access
      ? { type: "dangerFullAccess" }
      : {
          type: "workspaceWrite",
          writableRoots: request.additional_directories || [],
          networkAccess: false,
        };
    const input = turnInput(
      promptWithHistory(request),
      normalizedImages(request),
      temporaryDirectory,
      imageSequence++,
    );
    const started = await rpc.request("turn/start", {
      threadId,
      input,
      effort: request.effort || null,
      model: request.model || null,
      cwd: request.cwd,
      approvalPolicy: request.full_access ? "never" : "on-request",
      sandboxPolicy,
    });
    turnId = started.turn?.id;
    if (!turnId) throw new Error("Codex app-server did not return a turn id.");

    setController(async (control) => {
      if (control.type === "interrupt") {
        await rpc.request("turn/interrupt", { threadId, turnId }).catch(() => {});
        return;
      }
      if (control.type !== "steer") return;
      const input = turnInput(
        control.message,
        normalizedImages(control),
        temporaryDirectory,
        imageSequence++,
      );
      if (!input.length) return;
      await rpc.request("turn/steer", {
        threadId,
        expectedTurnId: turnId,
        input,
      });
    });

    await turnFinished;
    if (backgroundViolation) throw new Error(backgroundViolation);
    emit({
      type: "done",
      session_id: threadId,
      result: finalResponse,
      error: null,
      is_error: false,
      turn_usage: usage ? genericUsage(usage) : null,
      plan_usage: null,
    });
  } finally {
    rpc.stop();
    rmSync(temporaryDirectory, { recursive: true, force: true });
  }
};
