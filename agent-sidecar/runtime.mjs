import { mkdirSync } from "node:fs";
import { delimiter, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const numberOrZero = (value) => Number.isFinite(value) ? value : 0;

export const sourceEnvironment = (request) => {
  const environment = {
    BLACKHOLES_AGENT_SOURCE_SCOPE: request.scope?.kind || "global",
    BLACKHOLES_AGENT_SOURCE_ID: request.scope?.agent_id || "",
  };
  if (request.scope?.global_agent_id) environment.BLACKHOLES_AGENT_SOURCE_GLOBAL_ID = request.scope.global_agent_id;
  if (request.scope?.project_id) environment.BLACKHOLES_AGENT_SOURCE_PROJECT_ID = request.scope.project_id;
  if (request.scope?.task_id) environment.BLACKHOLES_AGENT_SOURCE_TASK_ID = request.scope.task_id;
  return environment;
};

export const providerEnvironment = (request) => {
  const environment = Object.fromEntries(
    Object.entries(process.env).filter(([, value]) => typeof value === "string"),
  );
  // Finder's PATH does not include Homebrew/nvm or the app's bundled Node.
  // All CLI shebangs and SDK subprocesses must use this exact runtime too.
  environment.PATH = [...new Set([dirname(process.execPath), ...((environment.PATH || "").split(delimiter))].filter(Boolean))].join(delimiter);
  if (request.auth_mode !== "isolated") return environment;

  const profile = request.auth_profile_dir;
  mkdirSync(profile, { recursive: true });
  switch (request.provider) {
    case "claude": environment.CLAUDE_CONFIG_DIR = profile; break;
    case "codex": environment.CODEX_HOME = profile; break;
    case "gemini": environment.GEMINI_CLI_HOME = profile; break;
    case "opencode":
      environment.XDG_DATA_HOME = join(profile, "data");
      environment.XDG_CONFIG_HOME = join(profile, "config");
      environment.XDG_CACHE_HOME = join(profile, "cache");
      for (const directory of [environment.XDG_DATA_HOME, environment.XDG_CONFIG_HOME, environment.XDG_CACHE_HOME]) {
        mkdirSync(directory, { recursive: true });
      }
      break;
  }
  return environment;
};

export const packageBinary = (name) => {
  const sidecarRoot = dirname(fileURLToPath(import.meta.url));
  return join(sidecarRoot, "node_modules", ".bin", name);
};

const scopePrompt = (scope, agentName) => {
  if (scope?.kind === "task") {
    return `You are ${agentName}. You own the Blackholes task "${scope.name}" (task id ${scope.task_id}) in project "${scope.project_name}". After verifying the Blackholes MCP, work directly in this isolated task worktree. Do not create another task or delegate to an internal agent. Use shell and file tools yourself. When the work completes, call the Blackholes notify_task_ready MCP tool before your final response.`;
  }
  if (scope?.kind === "project") {
    return `You are ${agentName}, the agent for project "${scope.name}" (project id ${scope.project_id}). This scope is context, not a permission boundary: you have the same shell, filesystem, network, and GitHub capabilities as a task agent. By default, inspect, review, edit, build, and test directly in this project's repositories as needed for the user's request. A Blackholes task, worktree, or handoff is not a prerequisite for repository work.`;
  }
  return `You are ${agentName}, the global Blackholes agent. Coordinate work across projects using persistent Blackholes agents. Resolve the intended project and repositories through the Blackholes MCP. For project implementation, normally hand off to that project's agent; for requested task implementation, hand off to the task's agent. Answer questions and perform lightweight discovery directly. This is workflow routing, not a permission boundary: you have the same shell, filesystem, network, and GitHub capabilities as a task agent, and may work directly when the user asks you to. A task or isolated worktree is not required for direct project work.`;
};

export const systemText = (request) => `You are ${request.agent_name || "Mercury"}, one of the Black Bots inside the Blackholes desktop app.
${scopePrompt(request.scope, request.agent_name || "Mercury")}

At the beginning of every user turn, before any other tool, call the Blackholes get_current_context MCP tool. This is the required availability check. If that tool is unavailable or its server cannot start, stop and report that the required Blackholes MCP is not available; do not fall back to filesystem discovery or UI automation.
Use the Blackholes MCP for project/task/worktree/note/navigation/notification orchestration. Global, project, and task scopes receive the same runtime tool access; never claim that Bash, files, GitHub, or networking are blocked merely because the current scope is global or project. When full access is selected in Blackholes, use the available shell, filesystem, network, and provider tools without adding scope-based restrictions.
Tasks and isolated worktrees are optional workflows. Use them when the user requests a task or isolation, has selected an existing task, or the project's own instructions explicitly require that methodology. Otherwise, work in the intended project repositories without requiring a task or an isolation opt-out, following the execution routing below. Read and respect the project's instructions, the requested scope, and the selected permission mode. Preserve unrelated changes and do not switch branches or move work into a task without the user's direction. For an isolated workflow, resolve the project and necessary repository ids, search for a clearly matching task, and create_task only when needed. Implementation belongs in its returned worktrees, not the original checkouts. This policy supersedes older Blackholes-generated blanket task requirements and blanket optional-delegation defaults; it does not override user-authored project rules.

Execution routing (apply before implementation):
- A request such as "create the task and start it", "crea la tarea y comiénzala", or "crea una tarea para X y hazlo" already authorizes creation and immediate execution by the new task's persistent Blackholes agent. From a global or project chat, resolve the repositories, create the task, then call handoff_to_agent with its taskId. Do not ask a redundant delegation question, and do not start exploring the new worktree, editing, building, or testing the implementation in the sending chat before handing off. Do only the discovery needed to choose the correct project, repositories, base branch, and brief.
- If a global or project chat is asked to implement an existing task, resolve that task and hand off to its taskId instead of doing its implementation in the sending chat. If already the agent of that same task, implement directly; never hand off to yourself, create a duplicate task, or bounce the work back to a project/global agent.
- For implementation in a named project without a requested task, the global agent normally uses handoff_to_agent with projectId. Do not create a task merely to delegate. A project agent receiving work for its own project implements directly unless the user requests a task workflow. Questions, explanations, status checks, and lightweight discovery can be answered directly.
- "Create a task" alone authorizes creation, not starting implementation. Create it and present its navigation card; ask whether to start it only if execution intent is unclear. Ask a concise clarification when the target project/repository or requested execution is genuinely ambiguous, not when the user already said to start.
- An explicit request to work here, not delegate, or only prepare/plan takes precedence. Direct work remains supported with the same permissions. Delegation never expands authorization for tests, servers, remote writes, or other actions.
- A handoff prompt must describe the actual implementation, task/project IDs, attached repositories/worktrees, relevant requirements, acceptance criteria, known decisions, and user constraints, including any limits on testing or remote actions. Tell the recipient to begin the implementation, not to create or delegate the task again. Read-only findings needed for execution belong in the brief.
Never create or invoke provider-native internal subagents. Blackholes already provides persistent agents visible in the sidebar with their own conversation. For a handoff use handoff_to_agent with projectId for project work or taskId for isolated task work. After an accepted handoff, stop the delegated implementation, briefly report the transfer, and let the user follow the receiving agent's card. started:true means the app launched the destination turn; queued:true means the app accepted it behind that agent's current work, not a failure. Do not tell the user to start an accepted or queued handoff manually. If the app rejects the handoff, report the specific failure; do not claim success or silently implement in the sending chat. If the result is a timeout or unknown state, do not claim it never started or blindly retry: check destination activity first to avoid duplicate work. Do not use Claude Agent, Codex collaboration agents, Gemini subagents, OpenCode task agents, or equivalent hidden/temporary agent tools.
Run shell commands in the foreground and wait for them to finish before completing the response. Never start detached or provider-managed background commands with run_in_background, is_background, nohup, disown, a trailing ampersand, detached screen/tmux sessions, or equivalent mechanisms. Blackholes cannot safely preserve invisible provider child processes after a chat turn ends. If the user explicitly needs a long-lived server or process, explain that it must be started in a visible Blackholes terminal. Never promise to keep working or report a command result after the final response.
A new user message supersedes discretionary work left over from the previous turn. If a command is interrupted when that message arrives, do not resume it unless the new request actually requires it; answer the current message first. For questions asking for an explanation or recommendation, do not delay the answer to finish an unrelated install, build, or broad test suite from earlier work. Keep necessary verification targeted and use conservative test-runner parallelism so the agent does not saturate the machine. Do not wrap long-running verification in git stash/git stash pop: an interruption can strand the task's changes in the stash.
Enabled MCP servers for this scope: ${Array.isArray(request.enabled_mcp_servers) ? request.enabled_mcp_servers.join(", ") : "blackholes"}. Do not use an MCP that is not in this list.
If the user asks to open or go to a task/project, call open_task or open_project so the app can show a navigation card. Never claim the view changed automatically.
Do not create or modify remote GitHub, GitLab, ClickUp, Jira, or other resources unless explicitly requested. Be concise, provide meaningful progress, and reply in ${request.language === "en" ? "English" : "Spanish"} unless asked otherwise.`;

export const promptWithHistory = (request) => {
  if (request.session_id || !Array.isArray(request.history) || request.history.length === 0) return request.message;
  const transcript = request.history
    .map((entry) => `${entry.role === "assistant" ? "Assistant" : "User"}: ${entry.content}`)
    .join("\n\n");
  return `The visible Blackholes conversation before switching or creating this provider session was:\n\n${transcript}\n\nCurrent user request:\n${request.message}`;
};

export const normalizedImages = (request) => Array.isArray(request.images)
  ? request.images.filter((image) => (
    ["image/png", "image/jpeg", "image/gif", "image/webp"].includes(image?.media_type) &&
    typeof image?.data === "string" && image.data.length > 0
  ))
  : [];

const detachedShellPattern = /(^|[;\n]\s*)(nohup|disown|daemonize|setsid)\b|\bscreen\s+-[^\n]*d|\btmux\s+(?:new-session|new-window)\b[^\n]*\s-d(?:\s|$)|\bstart\s+\/b\b/i;

const hasDetachedAmpersand = (command) => {
  let quote = null;
  for (let index = 0; index < command.length; index += 1) {
    const character = command[index];
    if (character === "\\" && quote !== "'") {
      index += 1;
      continue;
    }
    if (quote) {
      if (character === quote) quote = null;
      continue;
    }
    if (character === "'" || character === '"') {
      quote = character;
      continue;
    }
    if (character !== "&") continue;
    const previous = command[index - 1] || "";
    const next = command[index + 1] || "";
    if (!"&>".includes(previous) && !"&>".includes(next)) return true;
  }
  return false;
};

const commandValues = (value, key = "") => {
  if (typeof value === "string") {
    return ["command", "cmd", "script", "shell_command"].includes(key.toLowerCase()) ? [value] : [];
  }
  if (Array.isArray(value)) return value.flatMap((entry) => commandValues(entry, key));
  if (!value || typeof value !== "object") return [];
  return Object.entries(value).flatMap(([childKey, child]) => commandValues(child, childKey));
};

export const requestsBackgroundExecution = (input) => {
  if (!input || typeof input !== "object") return false;
  const stack = [input];
  while (stack.length) {
    const value = stack.pop();
    if (!value || typeof value !== "object") continue;
    if (value.run_in_background === true || value.is_background === true || value.background === true || value.detached === true) {
      return true;
    }
    stack.push(...(Array.isArray(value) ? value : Object.values(value)));
  }
  return commandValues(input).some((command) => (
    detachedShellPattern.test(command) || hasDetachedAmpersand(command)
  ));
};

export const toolName = (name = "") => {
  const lower = name.toLowerCase();
  if (lower.includes("shell") || lower.includes("command") || lower === "bash") return "Bash";
  if (lower.includes("read")) return "Read";
  if (lower.includes("write") || lower.includes("edit") || lower.includes("patch")) return "Edit";
  if (lower.includes("search") || lower.includes("grep") || lower.includes("glob")) return "Grep";
  return name || "Tool";
};

export const genericUsage = (usage = {}) => ({
  input_tokens: numberOrZero(usage.input_tokens ?? usage.inputTokens),
  output_tokens: numberOrZero(usage.output_tokens ?? usage.outputTokens),
  cache_read_input_tokens: numberOrZero(
    usage.cached_input_tokens ??
    usage.cache_read_input_tokens ??
    usage.cachedInputTokens ??
    usage.cacheReadInputTokens ??
    usage.cachedReadTokens,
  ),
  cache_creation_input_tokens: numberOrZero(
    usage.cache_creation_input_tokens ??
    usage.cacheWriteInputTokens ??
    usage.cacheCreationInputTokens ??
    usage.cachedWriteTokens,
  ),
  web_search_requests: 0,
  cost_usd: numberOrZero(usage.cost_usd ?? usage.cost),
  num_turns: 1,
});
