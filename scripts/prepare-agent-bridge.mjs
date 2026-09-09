// Install integration libraries without any provider's CLI distribution.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

export function assertNoBundledAgents(root) {
  const modules = join(root, "node_modules");
  const forbidden = [
    ".bin/codex", ".bin/claude", ".bin/gemini", ".bin/opencode", ".bin/agy",
    "@openai/codex", "@openai/codex-sdk", "@google/gemini-cli", "opencode-ai",
    "@anthropic-ai/claude-code", "@anthropic-ai/claude-agent-sdk/cli.js",
  ];
  const anthropic = join(modules, "@anthropic-ai");
  if (existsSync(anthropic)) {
    forbidden.push(...readdirSync(anthropic).filter(name => name.startsWith("claude-agent-sdk-"))
      .map(name => `@anthropic-ai/${name}`));
  }
  const found = forbidden.find(name => existsSync(join(modules, name)));
  if (found) throw new Error(`Provider CLI must not be bundled: ${found}. Install bridge libraries with --omit=optional.`);
}

export function prepareAgentBridge(root) {
  const digest = createHash("sha256").update("bridge-without-provider-clis-v1");
  for (const name of ["package.json", "package-lock.json"]) digest.update(readFileSync(join(root, name)));
  const fingerprint = digest.digest("hex");
  const marker = join(root, "node_modules/.blackholes-bridge");
  if (!existsSync(marker) || readFileSync(marker, "utf8") !== fingerprint
      || !existsSync(join(root, "node_modules/@anthropic-ai/claude-agent-sdk/sdk.mjs"))
      || !existsSync(join(root, "node_modules/@opencode-ai/sdk/dist/index.js"))) {
    execFileSync("npm", ["ci", "--omit=dev", "--omit=optional", "--ignore-scripts", "--no-audit", "--no-fund", "--prefix", root], { stdio: "inherit" });
    assertNoBundledAgents(root);
    writeFileSync(marker, fingerprint);
  }
  assertNoBundledAgents(root);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  prepareAgentBridge(resolve(process.argv[2] || "agent-sidecar"));
}
