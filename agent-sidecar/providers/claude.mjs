import { forkSession, getSessionMessages, query } from "@anthropic-ai/claude-agent-sdk";
import { randomUUID } from "node:crypto";
import {
  normalizedImages,
  numberOrZero,
  promptWithHistory,
  providerEnvironment,
  requestsBackgroundExecution,
  sourceEnvironment,
  systemText,
} from "../runtime.mjs";

const isDirectUserPrompt = (entry) => {
  if (entry?.type !== "user" || entry.parent_tool_use_id) return false;
  const content = entry.message?.content;
  if (typeof content === "string") return true;
  if (!Array.isArray(content)) return false;
  return content.some((block) => block?.type === "text" || block?.type === "image") &&
    !content.some((block) => block?.type === "tool_result");
};

const resumeSession = async (request) => {
  const sourceSessionId = request.session_id || null;
  if (!sourceSessionId || !Number.isInteger(request.fork_at_user_turn)) return sourceSessionId;
  if (request.fork_at_user_turn === 0) return null;
  const transcript = await getSessionMessages(sourceSessionId, { dir: request.cwd });
  const directPrompts = transcript
    .map((entry, index) => ({ entry, index }))
    .filter(({ entry }) => isDirectUserPrompt(entry));
  const target = directPrompts[request.fork_at_user_turn];
  if (!target) throw new Error("No se encontró el turno original desde el que bifurcar la conversación.");
  const anchor = transcript[target.index - 1];
  if (!anchor?.uuid) return null;
  const fork = await forkSession(sourceSessionId, {
    dir: request.cwd,
    upToMessageId: anchor.uuid,
    title: "Blackholes edit",
  });
  return fork.sessionId;
};

const userMessage = (text, images = [], uuid = randomUUID()) => {
  const content = images.map((image) => ({
    type: "image",
    source: { type: "base64", media_type: image.media_type, data: image.data },
  }));
  if (text.trim()) content.push({ type: "text", text });
  return {
    type: "user",
    message: { role: "user", content },
    parent_tool_use_id: null,
    uuid,
  };
};

class InputQueue {
  constructor() {
    this.values = [];
    this.waiters = [];
    this.closed = false;
  }

  push(value) {
    if (this.closed) return;
    const waiter = this.waiters.shift();
    if (waiter) waiter({ value, done: false });
    else this.values.push(value);
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    for (const waiter of this.waiters.splice(0)) waiter({ value: undefined, done: true });
  }

  [Symbol.asyncIterator]() {
    return {
      next: () => {
        if (this.values.length) return Promise.resolve({ value: this.values.shift(), done: false });
        if (this.closed) return Promise.resolve({ value: undefined, done: true });
        return new Promise((resolve) => this.waiters.push(resolve));
      },
    };
  }
};

const assistantText = (message) => {
  if (message?.type !== "assistant" || message.parent_tool_use_id || !Array.isArray(message.message?.content)) return "";
  return message.message.content
    .filter((block) => block?.type === "text")
    .map((block) => block.text || "")
    .join("");
};

const assistantTools = (message) => {
  if (message?.type !== "assistant" || !Array.isArray(message.message?.content)) return [];
  return message.message.content
    .filter((block) => block?.type === "tool_use")
    .map((block) => ({
      name: block.name,
      input: block.input,
      agent: message.parent_tool_use_id ? "black-bot" : null,
    }));
};

const turnUsage = (message) => Object.values(message?.modelUsage || {}).reduce((usage, model) => ({
  input_tokens: usage.input_tokens + numberOrZero(model?.inputTokens),
  output_tokens: usage.output_tokens + numberOrZero(model?.outputTokens),
  cache_read_input_tokens: usage.cache_read_input_tokens + numberOrZero(model?.cacheReadInputTokens),
  cache_creation_input_tokens: usage.cache_creation_input_tokens + numberOrZero(model?.cacheCreationInputTokens),
  web_search_requests: usage.web_search_requests + numberOrZero(model?.webSearchRequests),
  cost_usd: numberOrZero(message?.total_cost_usd),
  num_turns: numberOrZero(message?.num_turns),
}), {
  input_tokens: 0,
  output_tokens: 0,
  cache_read_input_tokens: 0,
  cache_creation_input_tokens: 0,
  web_search_requests: 0,
  cost_usd: numberOrZero(message?.total_cost_usd),
  num_turns: numberOrZero(message?.num_turns),
});

const resetTimestamp = (value) => {
  if (!Number.isFinite(value)) return null;
  const timestamp = new Date(value < 1_000_000_000_000 ? value * 1000 : value);
  return Number.isNaN(timestamp.getTime()) ? null : timestamp.toISOString();
};

const readPlanUsage = async (agentQuery) => {
  let timeout;
  try {
    return await Promise.race([
      agentQuery.usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET(),
      new Promise((_, reject) => {
        timeout = setTimeout(() => reject(new Error("Claude usage request timed out.")), 4000);
      }),
    ]);
  } finally {
    if (timeout) clearTimeout(timeout);
  }
};

const mergePlanUsage = (usage, account, eventRateLimits) => {
  const limits = { ...(usage?.rate_limits || {}) };
  for (const [name, event] of Object.entries(eventRateLimits)) {
    const previous = limits[name];
    // Partial rate-limit events must not erase percentages returned by /usage.
    if (!Number.isFinite(event.utilization) && Number.isFinite(previous?.utilization)) continue;
    limits[name] = {
      ...previous,
      utilization: Number.isFinite(event.utilization) ? event.utilization : previous?.utilization ?? null,
      resets_at: event.resets_at ?? previous?.resets_at ?? null,
    };
  }
  return {
    ...(usage || {}),
    subscription_type: usage?.subscription_type || account?.subscriptionType || null,
    rate_limits_available: Boolean(usage?.rate_limits_available) || Object.keys(eventRateLimits).length > 0,
    rate_limits: Object.keys(limits).length ? limits : null,
  };
};

const permissionHooks = () => ({
  PreToolUse: [{
    hooks: [async (input) => {
      if (input.hook_event_name !== "PreToolUse") return {};
      if (input.agent_id || input.tool_name === "Agent") {
        return { hookSpecificOutput: {
          hookEventName: "PreToolUse",
          permissionDecision: "deny",
          permissionDecisionReason: "Blackholes uses persistent project and task agents; this chat must not create an internal subagent.",
        } };
      }
      if (input.tool_name === "Bash" && requestsBackgroundExecution(input.tool_input || {})) {
        return { hookSpecificOutput: {
          hookEventName: "PreToolUse",
          permissionDecision: "deny",
          permissionDecisionReason: "Blackholes does not allow invisible background shell processes in agent chats. Run the command in the foreground or ask the user to start it in a visible Blackholes terminal.",
        } };
      }
      return { hookSpecificOutput: { hookEventName: "PreToolUse", permissionDecision: "allow" } };
    }],
  }],
});

export const runClaude = async ({ request, emit, signal, setStopper, setController }) => {
  Object.assign(process.env, providerEnvironment(request));
  process.env.CLAUDE_CODE_FORCE_SESSION_PERSISTENCE = "1";
  const abortController = new AbortController();
  signal.addEventListener("abort", () => abortController.abort(), { once: true });
  let sessionId = await resumeSession(request);
  let streamedText = "";
  let completeText = "";
  let separateNextTextBlock = false;
  let outputPromptId = null;
  let stopping = false;
  let redirectedOnce = false;
  const eventRateLimits = {};
  const blockedTaskIds = new Set();
  const activeTaskIds = new Set();
  const ignoredTaskIds = new Set();
  const inputQueue = new InputQueue();
  const initialPrompt = userMessage(promptWithHistory(request), normalizedImages(request), request.prompt_id);
  let expectedPromptId = initialPrompt.uuid;
  inputQueue.push(initialPrompt);
  const configuredMcpServers = Object.fromEntries(
    (request.configured_mcp_servers || []).map((server) => {
      if (server.transport === "http") {
        const config = { type: "http", url: server.url };
        if (server.oauth_client_id) {
          config.oauth = {
            clientId: server.oauth_client_id,
            ...(server.oauth_callback_port ? { callbackPort: server.oauth_callback_port } : {}),
          };
        }
        return [server.name, config];
      }
      return [server.name, {
        command: server.command,
        args: server.args || [],
        env: server.env || {},
      }];
    }),
  );

  const options = {
    cwd: request.cwd,
    additionalDirectories: request.additional_directories || [],
    mcpServers: { blackholes: {
      command: request.blackholes_mcp_command,
      args: ["mcp"],
      env: sourceEnvironment(request),
    }, ...configuredMcpServers },
    includePartialMessages: true,
    persistSession: true,
    abortController,
    permissionMode: request.full_access ? "bypassPermissions" : "default",
    allowDangerouslySkipPermissions: Boolean(request.full_access),
    allowedTools: [
      "Read", "Write", "Edit", "Glob", "Grep", "Bash",
      ...(request.enabled_mcp_servers || ["blackholes"]).map((name) => `mcp__${name}__*`),
    ],
    plugins: request.skills_plugin_path ? [{ type: "local", path: request.skills_plugin_path }] : [],
    skills: Array.isArray(request.skills) ? request.skills : [],
    settingSources: ["user", "project", "local"],
    strictMcpConfig: false,
    systemPrompt: { type: "preset", preset: "claude_code", append: systemText(request) },
    hooks: permissionHooks(),
    stderr: (line) => emit({ type: "diagnostic", message: line }),
  };
  if (request.model) options.model = request.model;
  if (request.effort) options.effort = request.effort;
  if (sessionId) options.resume = sessionId;

  const agentQuery = query({ prompt: inputQueue, options });
  setStopper(() => {
    stopping = true;
    inputQueue.close();
    abortController.abort();
    agentQuery.close();
  });
  setController(async (control) => {
    if (control.type === "interrupt") {
      stopping = true;
      inputQueue.close();
      await agentQuery.interrupt({ cancelQueued: true }).catch(() => {});
      return;
    }
    if (control.type !== "steer" || stopping) return;
    const text = String(control.message || "").trim();
    const images = normalizedImages(control);
    if (!text && images.length === 0) return;
    const redirectedPrompt = userMessage(text, images, control.prompt_id);
    redirectedOnce = true;
    expectedPromptId = redirectedPrompt.uuid;
    outputPromptId = null;
    for (const taskId of activeTaskIds) ignoredTaskIds.add(taskId);
    activeTaskIds.clear();
    await agentQuery.interrupt({ cancelQueued: false }).catch((error) => {
      emit({ type: "diagnostic", message: `Claude interrupt: ${error.message || error}` });
    });
    streamedText = "";
    completeText = "";
    separateNextTextBlock = false;
    inputQueue.push(redirectedPrompt);
  });
  const accountInfoPromise = agentQuery.accountInfo().catch((error) => {
    emit({ type: "diagnostic", message: `Claude account information is unavailable: ${error.message || error}` });
    return null;
  });
  let planUsagePromise = null;

  for await (const message of agentQuery) {
    if (message.type === "system" && message.subtype === "init" && message.session_id) {
      sessionId = message.session_id;
      emit({ type: "session", session_id: sessionId });
      planUsagePromise = readPlanUsage(agentQuery).catch(() => null);
      continue;
    }
    if (message.type === "system" && message.subtype === "task_started") {
      activeTaskIds.add(message.task_id);
      const backgrounded = message.is_backgrounded === true;
      emit({
        type: "background_task",
        task_id: message.task_id,
        status: backgrounded ? "blocked" : "foreground",
        description: message.description || "",
        task_type: message.task_type || "",
        summary: backgrounded ? "Blocked by Blackholes" : "",
        output_file: "",
        ambient: Boolean(message.ambient || message.skip_transcript),
        prompt_id: expectedPromptId,
      });
      if (backgrounded && !message.ambient && !message.skip_transcript) {
        blockedTaskIds.add(message.task_id);
        await agentQuery.stopTask(message.task_id).catch((error) => {
          emit({ type: "diagnostic", message: `Unable to stop Claude background task ${message.task_id}: ${error.message || error}` });
        });
      }
      continue;
    }
    if (message.type === "system" && message.subtype === "task_progress") {
      if (ignoredTaskIds.has(message.task_id)) continue;
      emit({
        type: "background_task",
        task_id: message.task_id,
        status: blockedTaskIds.has(message.task_id) ? "blocked" : "running",
        description: message.description || "",
        task_type: message.subagent_type ? "local_agent" : "local_bash",
        summary: message.summary || "",
        output_file: "",
        ambient: false,
        prompt_id: expectedPromptId,
      });
      continue;
    }
    if (message.type === "system" && message.subtype === "task_updated") {
      if (ignoredTaskIds.has(message.task_id)) continue;
      const becameBackground = message.patch?.is_backgrounded === true;
      const status = becameBackground
        ? "blocked"
        : message.patch?.status === "killed"
          ? "stopped"
          : message.patch?.status || "running";
      emit({
        type: "background_task",
        task_id: message.task_id,
        status,
        description: message.patch?.description || "",
        task_type: "",
        summary: becameBackground ? "Blocked by Blackholes" : message.patch?.error || "",
        output_file: "",
        ambient: false,
        prompt_id: expectedPromptId,
      });
      if (becameBackground && !blockedTaskIds.has(message.task_id)) {
        blockedTaskIds.add(message.task_id);
        await agentQuery.stopTask(message.task_id).catch((error) => {
          emit({ type: "diagnostic", message: `Unable to stop Claude background task ${message.task_id}: ${error.message || error}` });
        });
      }
      continue;
    }
    if (message.type === "system" && message.subtype === "task_notification") {
      activeTaskIds.delete(message.task_id);
      if (ignoredTaskIds.delete(message.task_id)) continue;
      blockedTaskIds.delete(message.task_id);
      emit({
        type: "background_task",
        task_id: message.task_id,
        status: message.status || "completed",
        description: "",
        task_type: "",
        summary: message.summary || "",
        output_file: message.output_file || "",
        ambient: Boolean(message.ambient || message.skip_transcript),
        prompt_id: expectedPromptId,
      });
      continue;
    }
    if (message.type === "system" && message.subtype === "background_tasks_changed") {
      for (const task of message.tasks || []) {
        if (ignoredTaskIds.has(task.task_id)) {
          await agentQuery.stopTask(task.task_id).catch(() => {});
          continue;
        }
        emit({
          type: "background_task",
          task_id: task.task_id,
          status: task.ambient ? "running" : "blocked",
          description: task.description || "",
          task_type: task.task_type || "",
          summary: task.ambient ? "" : "Blocked by Blackholes",
          output_file: "",
          ambient: Boolean(task.ambient),
          prompt_id: expectedPromptId,
        });
        if (!task.ambient && !blockedTaskIds.has(task.task_id)) {
          blockedTaskIds.add(task.task_id);
          await agentQuery.stopTask(task.task_id).catch((error) => {
            emit({ type: "diagnostic", message: `Unable to stop Claude background task ${task.task_id}: ${error.message || error}` });
          });
        }
      }
      continue;
    }
    if (message.type === "tool_progress" && message.task_id) {
      if (ignoredTaskIds.has(message.task_id)) continue;
      emit({
        type: "background_task",
        task_id: message.task_id,
        status: blockedTaskIds.has(message.task_id) ? "blocked" : "running",
        description: message.tool_name || "",
        task_type: message.subagent_type ? "local_agent" : "local_bash",
        summary: `${Math.max(0, Number(message.elapsed_time_seconds) || 0)}s`,
        output_file: "",
        ambient: false,
        prompt_id: expectedPromptId,
      });
      continue;
    }
    if (message.type === "rate_limit_event") {
      const info = message.rate_limit_info || {};
      if (["five_hour", "seven_day", "seven_day_opus", "seven_day_sonnet"].includes(info.rateLimitType)) {
        eventRateLimits[info.rateLimitType] = {
          utilization: Number.isFinite(info.utilization) ? info.utilization : null,
          resets_at: resetTimestamp(info.resetsAt),
        };
      }
      continue;
    }
    if (message.type === "stream_event" && !message.parent_tool_use_id) {
      if (message.user_message_uuid) outputPromptId = message.user_message_uuid;
      if (outputPromptId !== expectedPromptId) continue;
      if (message.event?.type === "content_block_delta" && message.event.delta?.type === "text_delta") {
        if (separateNextTextBlock && streamedText) {
          streamedText += "\n\n";
          emit({ type: "delta", text: "\n\n", prompt_id: outputPromptId });
        }
        separateNextTextBlock = false;
        const text = message.event.delta.text || "";
        streamedText += text;
        emit({ type: "delta", text, prompt_id: outputPromptId });
      }
      continue;
    }
    if (message.type === "assistant") {
      if (message.user_message_uuid) outputPromptId = message.user_message_uuid;
      if (outputPromptId !== expectedPromptId) continue;
      completeText = assistantText(message) || completeText;
      for (const tool of assistantTools(message)) {
        emit({ type: "tool", ...tool, prompt_id: outputPromptId });
        separateNextTextBlock = true;
      }
      continue;
    }
    if (message.type === "result") {
      const resultPromptId = message.user_message_uuid || null;
      const fatalUnscopedResult = Boolean(message.is_error && !resultPromptId && !redirectedOnce);
      if (!fatalUnscopedResult && resultPromptId !== expectedPromptId) {
        emit({
          type: "diagnostic",
          message: resultPromptId
            ? `Ignoring Claude result for superseded prompt ${resultPromptId}.`
            : "Ignoring a synthetic Claude result that is not associated with the active user prompt.",
        });
        continue;
      }
      if (resultPromptId) {
        await new Promise((resolve) => setTimeout(resolve, 75));
        if (resultPromptId !== expectedPromptId) {
          emit({ type: "diagnostic", message: `Ignoring Claude result for interrupted prompt ${resultPromptId}.` });
          continue;
        }
      }
      const result = typeof message.result === "string" ? message.result : "";
      if (!streamedText && completeText) emit({ type: "delta", text: completeText, prompt_id: resultPromptId });
      const [planUsage, accountInfo] = await Promise.all([
        planUsagePromise || Promise.resolve(null),
        accountInfoPromise,
      ]);
      emit({
        type: "done",
        session_id: message.session_id || sessionId,
        result,
        error: message.is_error ? result || `Claude terminó con ${message.subtype || "un error"}.` : "",
        is_error: Boolean(message.is_error),
        turn_usage: turnUsage(message),
        plan_usage: mergePlanUsage(planUsage, accountInfo, eventRateLimits),
        prompt_id: resultPromptId,
      });
      inputQueue.close();
      break;
    }
  }
};
