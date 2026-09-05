import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
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
  toolName,
} from "../runtime.mjs";

const promptParts = (text, images = []) => [
  ...(String(text || "").trim() ? [{ type: "text", text: String(text) }] : []),
  ...images.map((image) => ({
    type: "image",
    mimeType: image.media_type,
    data: image.data,
  })),
];

export const runGemini = async ({ request, emit, signal, setStopper, setController }) => {
  if (request.auth_mode === "isolated") {
    const isolatedCredentials = join(request.auth_profile_dir, ".gemini", "oauth_creds.json");
    if (!existsSync(isolatedCredentials)) {
      throw new Error(request.language === "en"
        ? "Gemini is not authenticated for the Blackholes account. Open Settings → Agent runtime and choose ‘Authenticate / change account…’."
        : "Gemini no está autenticado en la cuenta de Blackholes. Abre Ajustes → Motor de agentes y elige «Autenticar / cambiar cuenta…»."
      );
    }
  }

  const args = ["--acp", "--approval-mode", request.full_access ? "yolo" : "auto_edit"];
  if (request.model) args.push("--model", request.model);
  for (const directory of [
    ...(request.additional_directories || []),
    ...(request.skills_plugin_path ? [request.skills_plugin_path] : []),
  ]) args.push("--include-directories", directory);

  const child = spawn(packageBinary("gemini"), args, {
    cwd: request.cwd,
    env: providerEnvironment(request),
    stdio: ["pipe", "pipe", "pipe"],
  });
  let sessionId = request.session_id || null;
  let acceptUpdates = false;
  let promptActive = false;
  let stopping = false;
  let finalResponse = "";
  let usage = null;
  let backgroundViolation = null;
  const queuedPrompts = [];
  const shellTasks = new Map();

  const rpc = new JsonRpcProcess(child, {
    onDiagnostic: (message) => emit({ type: "diagnostic", message }),
    onRequest: async (method, params) => {
      if (method !== "session/request_permission") {
        throw new Error(`Unsupported Gemini ACP request: ${method}`);
      }
      if (requestsBackgroundExecution(params || {})) {
        return { outcome: { outcome: "cancelled" } };
      }
      const wantedKinds = request.full_access
        ? ["allow_always", "allow_once"]
        : ["reject_once", "reject_always"];
      const option = wantedKinds
        .map((kind) => params.options?.find((candidate) => candidate.kind === kind))
        .find(Boolean);
      if (!option) return { outcome: { outcome: "cancelled" } };
      return { outcome: { outcome: "selected", optionId: option.optionId } };
    },
    onNotification: (method, params) => {
      if (method !== "session/update" || params.sessionId !== sessionId || !acceptUpdates) return;
      const update = params.update || {};
      if (update.sessionUpdate === "agent_message_chunk" && update.content?.type === "text") {
        const text = update.content.text || "";
        if (text) {
          finalResponse += text;
          emit({ type: "delta", text });
        }
      } else if (update.sessionUpdate === "tool_call") {
        const taskId = update.toolCallId || update.tool_call_id || update.id || `gemini-command-${Date.now()}`;
        const tool = toolName(update.title || update.kind);
        if (tool === "Bash") {
          const description = typeof update.rawInput?.command === "string"
            ? update.rawInput.command
            : update.title || "Shell command";
          const backgroundRequested = requestsBackgroundExecution(update.rawInput || {});
          shellTasks.set(taskId, description);
          emit({
            type: "background_task",
            task_id: taskId,
            status: backgroundRequested ? "blocked" : "foreground",
            description,
            task_type: "local_bash",
            summary: backgroundRequested ? "Blocked by Blackholes" : "",
            output_file: "",
            ambient: false,
          });
          if (backgroundRequested) {
            backgroundViolation = request.language === "en"
              ? "Gemini tried to start a background process. Run it in the foreground or use a visible Blackholes terminal."
              : "Gemini intentó iniciar un proceso en segundo plano. Ejecútalo en primer plano o usa una terminal visible de Blackholes.";
            rpc.notify("session/cancel", { sessionId });
          }
        }
        emit({
          type: "tool",
          name: tool,
          input: update.rawInput || null,
        });
      } else if (update.sessionUpdate === "tool_call_update") {
        const taskId = update.toolCallId || update.tool_call_id || update.id;
        if (taskId && shellTasks.has(taskId)) {
          const rawStatus = String(update.status || update.kind || "").toLowerCase();
          const status = rawStatus.includes("fail") || rawStatus.includes("error")
            ? "failed"
            : rawStatus.includes("cancel") || rawStatus.includes("stop")
              ? "stopped"
              : rawStatus.includes("complete") || rawStatus.includes("success")
                ? "completed"
                : "foreground";
          emit({
            type: "background_task",
            task_id: taskId,
            status,
            description: shellTasks.get(taskId) || "Shell command",
            task_type: "local_bash",
            summary: update.title || "",
            output_file: "",
            ambient: false,
          });
        }
      }
    },
  });

  setStopper(() => rpc.stop());
  signal.addEventListener("abort", () => rpc.stop(), { once: true });

  try {
    await rpc.request("initialize", {
      protocolVersion: 1,
      clientInfo: { name: "blackholes", title: "Blackholes", version: "1" },
      clientCapabilities: {
        auth: { terminal: false },
        fs: { readTextFile: false, writeTextFile: false },
        terminal: false,
      },
    });

    const mcpServers = [{
      name: "blackholes",
      command: request.blackholes_mcp_command,
      args: ["mcp"],
      env: Object.entries(sourceEnvironment(request)).map(([name, value]) => ({ name, value })),
    }];
    if (sessionId && !Number.isInteger(request.fork_at_user_turn)) {
      await rpc.request("session/load", { sessionId, cwd: request.cwd, mcpServers });
    } else {
      const created = await rpc.request("session/new", { cwd: request.cwd, mcpServers });
      sessionId = created.sessionId;
    }
    if (!sessionId) throw new Error("Gemini ACP did not return a session id.");
    emit({ type: "session", session_id: sessionId });

    await rpc.request("session/set_mode", {
      sessionId,
      modeId: request.full_access ? "yolo" : "auto_edit",
    }).catch(() => {});
    if (request.model) {
      await rpc.request("session/set_model", { sessionId, modelId: request.model });
    }

    setController(async (control) => {
      if (control.type === "interrupt") {
        stopping = true;
        if (promptActive) rpc.notify("session/cancel", { sessionId });
        return;
      }
      if (control.type !== "steer" || stopping) return;
      const parts = promptParts(control.message, normalizedImages(control));
      if (!parts.length) return;
      queuedPrompts.push(parts);
      if (promptActive) rpc.notify("session/cancel", { sessionId });
    });

    const skills = Array.isArray(request.skills) && request.skills.length
      ? `\nEnabled Blackholes skills: ${request.skills.join(", ")}. Read matching SKILL.md files under ${request.skills_plugin_path}/skills before using them.`
      : "";
    queuedPrompts.push(promptParts(
      `${systemText(request)}${skills}\n\nUser request:\n${promptWithHistory(request)}`,
      normalizedImages(request),
    ));
    acceptUpdates = true;

    while (!stopping && queuedPrompts.length) {
      const parts = queuedPrompts.shift();
      promptActive = true;
      const result = await rpc.request("session/prompt", { sessionId, prompt: parts });
      promptActive = false;
      if (backgroundViolation) throw new Error(backgroundViolation);
      if (result.usage) usage = result.usage;
      if (result.stopReason === "refusal") throw new Error("Gemini refused the request.");
      if (result.stopReason === "max_tokens") {
        emit({ type: "diagnostic", message: "Gemini reached the output token limit." });
      }
      if (result.stopReason === "max_turn_requests") {
        throw new Error("Gemini reached its maximum number of agent turns.");
      }
    }

    if (!stopping) {
      emit({
        type: "done",
        session_id: sessionId,
        result: finalResponse,
        error: null,
        is_error: false,
        turn_usage: usage ? genericUsage(usage) : null,
        plan_usage: null,
      });
    }
  } finally {
    rpc.stop();
  }
};
