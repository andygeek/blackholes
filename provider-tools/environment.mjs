import { accessSync, constants } from "node:fs";
import { isAbsolute } from "node:path";

export const providerEnvironment = () => {
  // Inherit the computer account, including custom CLI profile variables.
  // Rust supplies the user's login-shell PATH; keep its Node/Bun and CLI selection.
  return Object.fromEntries(
    Object.entries(process.env).filter(([, value]) => typeof value === "string"),
  );
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
