import { dirname, delimiter } from "node:path";
import { createOpencode } from "@opencode-ai/sdk";
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

const errorMessage = (error) => {
  if (!error) return "OpenCode runtime failed.";
  if (typeof error === "string") return error;
  return error.message || error.data?.message || JSON.stringify(error);
};

export const runOpenCode = async ({ request, emit, signal, setStopper, setController }) => {
  const environment = providerEnvironment(request);
  environment.PATH = `${dirname(packageBinary("opencode"))}${delimiter}${environment.PATH || ""}`;
  Object.assign(process.env, environment);
  const permission = request.full_access
    ? { edit: "allow", bash: "allow", webfetch: "allow", doom_loop: "allow", external_directory: "allow" }
    : { edit: "allow", bash: "ask", webfetch: "ask", doom_loop: "ask", external_directory: "deny" };
  const sourceEnv = sourceEnvironment(request);
  const runtime = await createOpencode({
    signal,
    timeout: 15_000,
    config: {
      ...(request.model ? { model: request.model } : {}),
      share: "disabled",
      autoupdate: false,
      permission,
      mcp: {
        blackholes: {
          type: "local",
          command: [request.blackholes_mcp_command, "mcp"],
          environment: sourceEnv,
          enabled: true,
        },
      },
      instructions: request.skills_plugin_path
        ? [`${request.skills_plugin_path}/skills/*/SKILL.md`]
        : [],
    },
  });
  const { client, server } = runtime;
  let sessionId = request.session_id || null;
  let promptActive = false;
  let stopping = false;
  let backgroundViolation = null;
  const queuedPrompts = [];
  setStopper(() => {
    stopping = true;
    if (sessionId) {
      void client.session.abort({ path: { id: sessionId }, query: { directory: request.cwd } });
    }
    server.close();
  });

  try {
    if (Number.isInteger(request.fork_at_user_turn)) sessionId = null;
    if (!sessionId) {
      const created = await client.session.create({
        body: { title: "Blackholes" },
        query: { directory: request.cwd },
      });
      if (created.error || !created.data?.id) throw new Error(errorMessage(created.error));
      sessionId = created.data.id;
      emit({ type: "session", session_id: sessionId });
    }

    setController(async (control) => {
      if (control.type === "interrupt") {
        stopping = true;
        if (promptActive) {
          await client.session.abort({ path: { id: sessionId }, query: { directory: request.cwd } });
        }
        return;
      }
      if (control.type !== "steer" || stopping) return;
      const parts = [
        ...(String(control.message || "").trim()
          ? [{ type: "text", text: String(control.message) }]
          : []),
        ...normalizedImages(control).map((image, index) => ({
          type: "file",
          mime: image.media_type,
          filename: `image-${index}`,
          url: `data:${image.media_type};base64,${image.data}`,
        })),
      ];
      if (!parts.length) return;
      queuedPrompts.push(parts);
      if (promptActive) {
        await client.session.abort({ path: { id: sessionId }, query: { directory: request.cwd } });
      }
    });

    queuedPrompts.push([
      { type: "text", text: promptWithHistory(request) },
      ...normalizedImages(request).map((image, index) => ({
        type: "file",
        mime: image.media_type,
        filename: `image-${index}`,
        url: `data:${image.media_type};base64,${image.data}`,
      })),
    ]);

    let usage = null;
    let finalResponse = "";
    while (!stopping && queuedPrompts.length) {
      const parts = queuedPrompts.shift();
      const subscription = await client.event.subscribe({ query: { directory: request.cwd } });
      const emittedTextByPart = new Map();
      let streamedResponse = "";
      let sessionError = null;
      const eventsTask = (async () => {
        for await (const event of subscription.stream) {
          const properties = event.properties || {};
          const eventSessionId = properties.sessionID || properties.part?.sessionID;
          if (eventSessionId && eventSessionId !== sessionId) continue;
          if (event.type === "message.part.updated") {
            const part = properties.part;
            if (part?.type === "text") {
              const previous = emittedTextByPart.get(part.id) || "";
              const delta = typeof properties.delta === "string"
                ? properties.delta
                : part.text?.startsWith(previous) ? part.text.slice(previous.length) : part.text || "";
              if (delta) {
                streamedResponse += delta;
                emit({ type: "delta", text: delta });
              }
              emittedTextByPart.set(part.id, part.text || `${previous}${delta}`);
            } else if (part?.type === "tool") {
              const tool = toolName(part.tool);
              const taskId = part.callID || part.callId || part.id || `opencode-command-${Date.now()}`;
              const rawStatus = String(part.state?.status || "").toLowerCase();
              if (tool === "Bash") {
                const input = part.state?.input || {};
                const description = typeof input.command === "string" ? input.command : part.tool || "Shell command";
                const backgroundRequested = requestsBackgroundExecution(input);
                const status = backgroundRequested
                  ? "blocked"
                  : rawStatus === "completed"
                    ? "completed"
                    : rawStatus === "error" || rawStatus === "failed"
                      ? "failed"
                      : "foreground";
                const taskSummary = backgroundRequested
                  ? "Blocked by Blackholes"
                  : part.state?.error
                    ? errorMessage(part.state.error)
                    : "";
                emit({
                  type: "background_task",
                  task_id: taskId,
                  status,
                  description,
                  task_type: "local_bash",
                  summary: taskSummary,
                  output_file: "",
                  ambient: false,
                });
                if (backgroundRequested && !backgroundViolation) {
                  backgroundViolation = request.language === "en"
                    ? "OpenCode tried to start a background process. Run it in the foreground or use a visible Blackholes terminal."
                    : "OpenCode intentó iniciar un proceso en segundo plano. Ejecútalo en primer plano o usa una terminal visible de Blackholes.";
                  void client.session.abort({ path: { id: sessionId }, query: { directory: request.cwd } });
                }
              }
              if (["pending", "running"].includes(rawStatus)) {
                emit({ type: "tool", name: tool, input: part.state?.input || null });
              }
            } else if (part?.type === "step-finish") {
              usage = {
                input_tokens: part.tokens?.input,
                output_tokens: part.tokens?.output,
                cached_input_tokens: part.tokens?.cache?.read,
                cache_creation_input_tokens: part.tokens?.cache?.write,
                cost: part.cost,
              };
            }
          } else if (event.type === "permission.updated") {
            emit({ type: "diagnostic", message: `OpenCode requested permission: ${properties.title || properties.type}` });
            const backgroundRequested = requestsBackgroundExecution(properties);
            await client.postSessionIdPermissionsPermissionId({
              path: { id: sessionId, permissionID: properties.id },
              body: { response: backgroundRequested ? "reject" : request.full_access ? "once" : "reject" },
              query: { directory: request.cwd },
            });
          } else if (event.type === "session.error") {
            sessionError = errorMessage(properties.error);
          } else if (event.type === "session.idle") {
            break;
          }
        }
      })();

      promptActive = true;
      const prompted = await client.session.prompt({
        path: { id: sessionId },
        query: { directory: request.cwd },
        body: { system: systemText(request), parts,
          ...(request.model ? { model: { providerID: request.model.split("/")[0], modelID: request.model.slice(request.model.indexOf("/") + 1) } } : {}),
          ...(request.effort ? { variant: request.effort } : {}),
        },
      });
      promptActive = false;
      await eventsTask;
      if (backgroundViolation) throw new Error(backgroundViolation);
      if (prompted.error && queuedPrompts.length === 0 && !stopping) {
        throw new Error(errorMessage(prompted.error));
      }
      if (sessionError && queuedPrompts.length === 0 && !stopping) throw new Error(sessionError);
      const response = (prompted.data?.parts || [])
        .filter((part) => part.type === "text")
        .map((part) => part.text || "")
        .join("");
      if (!streamedResponse && response) emit({ type: "delta", text: response });
      finalResponse = response || streamedResponse || finalResponse;
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
    server.close();
  }
};
