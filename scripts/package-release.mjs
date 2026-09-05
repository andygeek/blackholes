#!/usr/bin/env node
// SPDX-License-Identifier: MPL-2.0
// Produces signed/notarized GitHub Release assets; never uploads or publishes.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { closeSync, cpSync, existsSync, lstatSync, mkdirSync, mkdtempSync, openSync, readFileSync, readSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const run = (command, args, options = {}) => execFileSync(command, args, { cwd: root, stdio: "inherit", ...options });
const capture = (command, args) => run(command, args, { stdio: ["ignore", "pipe", "inherit"], encoding: "utf8" }).trim();
const requireEnv = (name) => {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`Set ${name} first. See docs/RELEASING.md.`);
  return value;
};
if (process.platform !== "darwin") throw new Error("Release packaging requires macOS.");
const identity = requireEnv("BLACKHOLES_SIGNING_IDENTITY");
if (!identity.startsWith("Developer ID Application:")) throw new Error("Use a Developer ID Application signing identity.");
const notaryProfile = requireEnv("BLACKHOLES_NOTARY_PROFILE");
const publicKey = requireEnv("BLACKHOLES_SPARKLE_PUBLIC_KEY");
if (!/^[A-Za-z0-9+/]{43}=$/.test(publicKey) || Buffer.from(publicKey, "base64").length !== 32) throw new Error("Expected a Sparkle Ed25519 PUBLIC key, not a private key.");
if (process.env.BLACKHOLES_ACK_RUNTIME_LICENSES !== "1") throw new Error("Review the agent runtime redistribution terms, then set BLACKHOLES_ACK_RUNTIME_LICENSES=1.");
if (capture("git", ["status", "--porcelain"])) throw new Error("Commit the release source before packaging so its source archive matches the app.");

const version = readFileSync(join(root, "Cargo.toml"), "utf8").match(/^version = "([0-9]+\.[0-9]+\.[0-9]+)"$/m)?.[1];
if (!version) throw new Error("Expected a stable major.minor.patch package version.");
const arch = process.arch === "arm64" ? "arm64" : process.arch === "x64" ? "x86_64" : null;
if (!arch) throw new Error("Unsupported release architecture.");
const repo = "https://github.com/andygeek/blackholes";
const sparkle = capture("bash", ["scripts/fetch-sparkle"]);
const nodeRuntime = capture("bash", ["scripts/fetch-node", arch]);
const signingPublicKey = capture(join(sparkle, "bin/generate_keys"), ["--account", "blackholes", "-p"]);
if (signingPublicKey !== publicKey) throw new Error("Public key does not match the Blackholes update-signing key in this Mac's Keychain.");
run("./scripts/build-release", []);
// Generated assets must be committed before release; never silently publish a different source snapshot.
if (capture("git", ["status", "--porcelain"])) throw new Error("Build changed generated files. Review, commit, and rerun packaging.");

const artifactRoot = join(root, "target", "release-artifacts");
mkdirSync(artifactRoot, { recursive: true });
const output = mkdtempSync(join(artifactRoot, `v${version}-${arch}-`));
const staging = join(output, "staging");
const app = join(staging, "Blackholes.app");
const contents = join(app, "Contents");
const resources = join(contents, "Resources");
mkdirSync(join(contents, "MacOS"), { recursive: true });
mkdirSync(join(contents, "Frameworks"), { recursive: true });
mkdirSync(resources, { recursive: true });
cpSync(join(root, "target/release/blackholes-rust"), join(contents, "MacOS/Blackholes"));
cpSync(join(root, "assets/app-icon.icns"), join(resources, "app-icon.icns"));
for (const file of ["LICENSE", "LICENSING.md", "THIRD_PARTY_NOTICES.md"]) cpSync(join(root, file), join(resources, file));
cpSync(join(root, "licenses"), join(resources, "licenses"), { recursive: true });
cpSync(join(root, "assets/fonts/OFL-Geist.txt"), join(resources, "licenses/OFL-Geist.txt"));
cpSync(join(sparkle, "LICENSE"), join(resources, "licenses/SPARKLE.txt"));
// Include Node and npm/npx, without developer headers or any local credentials.
const bundledNode = join(resources, "node");
mkdirSync(bundledNode);
for (const entry of ["bin", "lib", "LICENSE"]) {
  cpSync(join(nodeRuntime, entry), join(bundledNode, entry), { recursive: true, verbatimSymlinks: true });
}
cpSync(join(nodeRuntime, "LICENSE"), join(resources, "licenses/NODE.txt"));

// Explicit source allow-list avoids copying local .env/.npmrc, logs, and signing material.
const runtime = join(resources, "agent-sidecar");
mkdirSync(runtime, { recursive: true });
for (const entry of readdirSync(join(root, "agent-sidecar"))) {
  if (entry.endsWith(".mjs") || ["providers", "package.json", "package-lock.json"].includes(entry)) {
    cpSync(join(root, "agent-sidecar", entry), join(runtime, entry), { recursive: true });
  }
}
// Install into the clean package, not from a possibly credential-bearing local node_modules tree.
run("npm", ["ci", "--omit=dev", "--ignore-scripts", "--prefix", runtime]);
if (!existsSync(join(runtime, "node_modules/@anthropic-ai/claude-agent-sdk"))) throw new Error("Agent runtime dependency missing.");
run("/usr/bin/ditto", [join(sparkle, "Sparkle.framework"), join(contents, "Frameworks/Sparkle.framework")]);

const plist = {
  CFBundleName: "Blackholes", CFBundleDisplayName: "Blackholes", CFBundleExecutable: "Blackholes",
  CFBundleIdentifier: "dev.blackholes.rust", CFBundlePackageType: "APPL", CFBundleIconFile: "app-icon",
  CFBundleVersion: version, CFBundleShortVersionString: version, LSMinimumSystemVersion: "13.0",
  NSHighResolutionCapable: true, NSPrincipalClass: "NSApplication",
  SUFeedURL: `${repo}/releases/latest/download/appcast-${arch}.xml`, SUPublicEDKey: publicKey,
  SUEnableAutomaticChecks: true, SUScheduledCheckInterval: 14400,
  SUAutomaticallyUpdate: false, SUAllowsAutomaticUpdates: false,
  SUEnableSystemProfiling: false, SUVerifyUpdateBeforeExtraction: true,
};
const escapeXML = (s) => s.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");
writeFileSync(join(contents, "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict>\n${Object.entries(plist).map(([key,value]) => `<key>${key}</key>${typeof value === "boolean" ? `<${value}/>` : typeof value === "number" ? `<integer>${value}</integer>` : `<string>${escapeXML(value)}</string>`}`).join("\n")}\n</dict></plist>\n`);
const sourceCommit = capture("git", ["rev-parse", "HEAD"]);
writeFileSync(join(resources, "SOURCE.txt"), `Corresponding source for Blackholes ${version}:\n${repo}/archive/${sourceCommit}.tar.gz\nThird-party package sources and licenses are identified by the included lockfiles and notices.\n`);

// Sign inside out. Preserve upstream entitlements for helpers; never use --deep to sign.
const sign = (path, preserve = false) => run("/usr/bin/codesign", ["--force", "--sign", identity, "--timestamp", "--options", "runtime",
  ...(path === join(bundledNode, "bin/node") ? ["--entitlements", join(root, "assets/node-entitlements.plist")] : preserve ? ["--preserve-metadata=entitlements"] : []), path]);
const macho = new Set(["cffaedfe", "cefaedfe", "feedfacf", "feedface", "cafebabe", "bebafeca", "cafebabf", "bfbafeca"]);
const isMachO = (path) => {
  const fd = openSync(path, "r");
  try { const header = Buffer.alloc(4); return readSync(fd, header, 0, 4, 0) === 4 && macho.has(header.toString("hex")); }
  finally { closeSync(fd); }
};
const visit = (dir) => {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry), stat = lstatSync(path);
    if (stat.isSymbolicLink()) continue;
    if (stat.isDirectory()) { visit(path); if (/\.(app|xpc|framework)$/.test(entry)) sign(path, true); }
    else if (stat.isFile() && stat.size >= 4 && isMachO(path)) sign(path, true);
  }
};
visit(contents);
sign(app);
run("/usr/bin/codesign", ["--verify", "--deep", "--strict", "--verbose=2", app]);
const submission = join(staging, "notarization.zip");
run("/usr/bin/ditto", ["-c", "-k", "--keepParent", app, submission]);
const result = JSON.parse(capture("xcrun", ["notarytool", "submit", submission, "--keychain-profile", notaryProfile, "--wait", "--output-format", "json"]));
writeFileSync(join(output, "notarization.json"), JSON.stringify(result, null, 2));
if (result.id) run("xcrun", ["notarytool", "log", result.id, "--keychain-profile", notaryProfile, join(output, "notarization-log.json")]);
if (result.status !== "Accepted") throw new Error(`Apple did not accept this release. See ${output}/notarization-log.json`);
run("xcrun", ["stapler", "staple", app]);
run("xcrun", ["stapler", "validate", app]);
const downloads = join(output, "github");
mkdirSync(downloads);
const archive = join(downloads, `Blackholes-${version}-${arch}.zip`);
run("/usr/bin/ditto", ["-c", "-k", "--keepParent", app, archive]);
run(join(sparkle, "bin/generate_appcast"), ["--account", "blackholes", "--download-url-prefix", `${repo}/releases/download/v${version}/`, "-o", join(downloads, `appcast-${arch}.xml`), downloads]);
const source = join(downloads, `Blackholes-${version}-source.tar.gz`);
run("git", ["archive", "--format=tar.gz", `--prefix=Blackholes-${version}/`, "-o", source, "HEAD"]);
writeFileSync(join(downloads, "SHA256SUMS.txt"), readdirSync(downloads).sort().map((name) => `${createHash("sha256").update(readFileSync(join(downloads, name))).digest("hex")}  ${name}\n`).join(""));
console.log(`Ready for manual upload to GitHub Release v${version}: ${downloads}\nPublish only after reviewing the notarization log. Nothing was uploaded to GitHub.`);
