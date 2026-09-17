import { accessSync, constants, mkdirSync } from "node:fs";
import { isAbsolute, join } from "node:path";

export const providerEnvironment = (request) => {
  const environment = Object.fromEntries(
    Object.entries(process.env).filter(([, value]) => typeof value === "string"),
  );
  // Rust supplies the user's login-shell PATH. Keep its Node/Bun and CLI
  // selection; the app's private JavaScript engine is only for this bridge.
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

export const installedAgentBinary = (name) => {
  const executable = process.env.BLACKHOLES_AGENT_EXECUTABLE;
  if (!executable || !isAbsolute(executable)) {
    throw new Error(`${name} CLI is unavailable. Install it on your computer and retry.`);
  }
  try { accessSync(executable, constants.X_OK); }
  catch { throw new Error(`${name} CLI is no longer available at ${executable}. Check your installation and retry.`); }
  return executable;
};
