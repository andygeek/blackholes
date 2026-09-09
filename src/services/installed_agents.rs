//! Resolve developer-owned CLIs. Never fall back to a Blackholes-bundled agent.
use anyhow::{Context, Result, bail};
use std::{
    env, ffi::OsString, io::Read, path::{Path, PathBuf},
    process::{Command, Stdio}, time::{Duration, Instant},
};

pub fn login_shell() -> String {
    env::var("SHELL").ok().filter(|shell| Path::new(shell).is_file())
        .unwrap_or_else(|| "/bin/zsh".into())
}

pub struct InstalledAgent {
    pub binary: PathBuf,
    pub path: OsString,
}

impl InstalledAgent {
    /// Call from a worker: interactive shell initialization can take time.
    pub fn resolve(name: &str, cwd: &Path) -> Result<Self> {
        let path = shell_path(cwd).or_else(|| env::var_os("PATH")).unwrap_or_default();
        let mut directories = env::split_paths(&path).collect::<Vec<_>>();
        // Finder may start the app with a minimal PATH. Shell configuration wins;
        // conventional installation locations are only additional search paths.
        if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
            directories.extend([
                home.join(".local/bin"), home.join(".opencode/bin"),
                home.join(".volta/bin"), home.join(".asdf/shims"), home.join(".bun/bin"),
            ]);
        }
        directories.extend([PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")]);
        directories.retain(|dir| dir.is_absolute() && !internal_agent_path(dir));
        let binary = directories.iter().map(|dir| dir.join(name))
            .find(|candidate| executable(candidate) && !internal_agent_path(candidate));
        let Some(binary) = binary else {
            bail!("{name} CLI was not found in your shell PATH. Install {name} on your computer and make it available in your terminal, then retry. Blackholes does not include its own copy.");
        };
        Ok(Self { binary, path: env::join_paths(directories).context("Invalid agent PATH")? })
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.binary);
        // Let native launchers and script shebangs use the user's own Node/Bun.
        command.env("PATH", &self.path);
        command
    }

    pub fn configure_sidecar(&self, command: &mut Command) {
        command.env("PATH", &self.path).env("BLACKHOLES_AGENT_EXECUTABLE", &self.binary);
    }
}

fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata().is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

fn internal_agent_path(path: &Path) -> bool {
    let mut roots = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("agent-sidecar")];
    if let Ok(executable) = env::current_exe()
        && let Some(contents) = executable.parent().and_then(Path::parent)
    {
        roots.push(contents.join("Resources/agent-sidecar"));
    }
    for name in ["BLACKHOLES_AGENT_SIDECAR", "BLACKHOLES_CLAUDE_SIDECAR"] {
        if let Some(script) = env::var_os(name).map(PathBuf::from)
            && let Some(parent) = script.parent()
        { roots.push(parent.to_path_buf()); }
    }
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    roots.iter().any(|root| path.starts_with(root) || canonical.starts_with(root.canonicalize().unwrap_or_else(|_| root.clone())))
        // Also exclude copies inherited from an older installed Blackholes app.
        || canonical.components().collect::<Vec<_>>().windows(2).any(|parts| {
            parts[0].as_os_str() == "agent-sidecar" && parts[1].as_os_str() == "node_modules"
        })
}

fn shell_path(cwd: &Path) -> Option<OsString> {
    use std::os::unix::{ffi::OsStringExt, process::CommandExt};
    // Absolute utilities and NUL markers tolerate startup banners and whitespace
    // in PATH. Only PATH is read; no agent is launched or profile file modified.
    const MARKER: &[u8] = b"\0BLACKHOLES_PATH\0";
    let mut command = Command::new(login_shell());
    command.args(["-l", "-i", "-c", "/usr/bin/printf '\\0BLACKHOLES_PATH\\0'; /usr/bin/printenv PATH; /usr/bin/printf '\\0BLACKHOLES_PATH\\0'"])
        .current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null())
        .process_group(0);
    let mut child = command.spawn().ok()?;
    let output = child.stdout.take()?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = output.take(128 * 1024 + 1).read_to_end(&mut bytes);
        let _ = sender.send(result.ok().map(|_| bytes));
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => break false,
        }
    };
    // Reap helpers that might keep stdout open after shell initialization.
    if let Ok(pid) = i32::try_from(child.id()) { unsafe { libc::kill(-pid, libc::SIGKILL); } }
    let _ = child.wait();
    let bytes = receiver.recv_timeout(Duration::from_millis(200)).ok()??;
    if !success || bytes.len() > 128 * 1024 { return None; }
    let start = bytes.windows(MARKER.len()).position(|part| part == MARKER)? + MARKER.len();
    let end = bytes[start..].windows(MARKER.len()).position(|part| part == MARKER)? + start;
    let path = bytes[start..end].strip_suffix(b"\n")?;
    Some(OsString::from_vec(path.to_vec()))
}
