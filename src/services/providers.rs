use anyhow::{Context as _, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
};

static ACTIVE_PROVIDER_HELPERS: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();

fn active_provider_helpers() -> &'static Mutex<HashSet<u32>> {
    ACTIVE_PROVIDER_HELPERS.get_or_init(|| Mutex::new(HashSet::new()))
}

struct ActiveProviderHelper(u32);

impl ActiveProviderHelper {
    fn register(process_id: u32) -> Self {
        active_provider_helpers()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(process_id);
        Self(process_id)
    }
}

impl Drop for ActiveProviderHelper {
    fn drop(&mut self) {
        active_provider_helpers()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.0);
    }
}

/// Stop usage metadata helpers, including their child CLIs, on shutdown.
pub fn terminate_provider_helpers() {
    let process_ids = active_provider_helpers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .copied()
        .collect::<Vec<_>>();

    #[cfg(unix)]
    {
        for process_id in &process_ids {
            signal_helper_process_group(*process_id, libc::SIGTERM);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        for process_id in process_ids {
            if helper_process_group_exists(process_id) {
                signal_helper_process_group(process_id, libc::SIGKILL);
            }
        }
    }

    #[cfg(windows)]
    for process_id in process_ids {
        let _ = Command::new("taskkill")
            .args(["/PID", &process_id.to_string(), "/T", "/F"])
            .status();
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentProvider {
    #[default]
    Claude,
    Codex,
    Gemini,
    OpenCode,
}

impl AgentProvider {
    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::OpenCode => "opencode",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Gemini => "Gemini",
            Self::OpenCode => "OpenCode",
        }
    }

}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderPlanUsage {
    pub subscription_type: Option<String>,
    pub rate_limits_available: bool,
    pub windows: Vec<PlanUsageWindow>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlanUsageWindow {
    pub label: String,
    pub limit_id: Option<String>,
    pub minutes: Option<u64>,
    pub utilization: Option<f64>,
    pub resets_at: Option<String>,
}

#[cfg(unix)]
fn terminate_helper_process(process_id: u32) {
    // Helpers lead their own process groups so their child CLIs also stop.
    signal_helper_process_group(process_id, libc::SIGTERM);
    std::thread::sleep(std::time::Duration::from_millis(1_200));
    if helper_process_group_exists(process_id) {
        signal_helper_process_group(process_id, libc::SIGKILL);
    }
}

#[cfg(unix)]
fn signal_helper_process_group(process_id: u32, signal: i32) {
    let Ok(process_group) = i32::try_from(process_id).map(|id| -id) else {
        return;
    };
    unsafe {
        libc::kill(process_group, signal);
    }
}

#[cfg(unix)]
fn helper_process_group_exists(process_id: u32) -> bool {
    let Ok(process_group) = i32::try_from(process_id).map(|id| -id) else {
        return false;
    };
    (unsafe { libc::kill(process_group, 0) }) == 0
}

#[cfg(windows)]
fn terminate_helper_process(process_id: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &process_id.to_string(), "/T", "/F"])
        .status();
}

/// Fetch plan metadata without a conversation, prompt, tools, or token usage.
/// Runs on a background executor; the deadline also covers provider startup/cleanup.
pub fn refresh_agent_plan_usage(provider: AgentProvider) -> Result<ProviderPlanUsage> {
    let script = locate_usage_script()?;
    let cwd = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let agent = super::installed_agents::InstalledAgent::resolve(provider.id(), &cwd)?;
    let mut command = Command::new(locate_node_binary());
    agent.configure_sidecar(&mut command);
    command
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command.spawn().context("Unable to start usage query")?;
    let process_id = child.id();
    let _active_helper = ActiveProviderHelper::register(process_id);
    let payload = serde_json::to_vec(&serde_json::json!({
        "provider": provider.id(),
    }))?;
    let write_result = child
        .stdin
        .take()
        .context("Usage stdin unavailable")
        .and_then(|mut stdin| stdin.write_all(&payload).map_err(Into::into));
    if let Err(error) = write_result {
        terminate_helper_process(process_id);
        let _ = child.wait();
        return Err(error);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => {
                terminate_helper_process(process_id);
                let _ = child.wait();
                return Err(anyhow!("Usage query timed out or exited unexpectedly"));
            }
        }
    };
    if !status.success() {
        return Err(anyhow!("Usage query unavailable"));
    }
    let mut output = String::new();
    child
        .stdout
        .take()
        .context("Usage stdout unavailable")?
        .take(65536)
        .read_to_string(&mut output)?;
    serde_json::from_str(&output).context("Invalid usage response")
}

fn locate_usage_script() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("BLACKHOLES_PROVIDER_USAGE_SCRIPT") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(anyhow!("{} does not exist", path.display()));
    }

    if let Ok(executable) = std::env::current_exe()
        && let Some(contents) = executable.parent().and_then(Path::parent)
    {
        let bundled = contents.join("Resources/provider-tools/usage.mjs");
        if bundled.is_file() {
            return Ok(bundled);
        }
    }

    let development = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("provider-tools")
        .join("usage.mjs");
    if development.is_file() {
        return Ok(development);
    }

    Err(anyhow!(
        "Provider usage helper was not found; set BLACKHOLES_PROVIDER_USAGE_SCRIPT"
    ))
}

pub(crate) fn locate_node_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("BLACKHOLES_NODE_BINARY") {
        return PathBuf::from(path);
    }

    if let Ok(executable) = std::env::current_exe() {
        if let Some(contents) = executable.parent().and_then(Path::parent) {
            let bundled = contents.join("Resources/node/bin/node");
            if bundled.is_file() {
                return bundled;
            }
        }
    }

    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            let candidate = directory.join("node");
            if candidate.is_file() {
                return candidate;
            }
        }
    }

    for candidate in ["/opt/homebrew/bin/node", "/usr/local/bin/node"] {
        let candidate = PathBuf::from(candidate);
        if candidate.is_file() {
            return candidate;
        }
    }

    if let Some(user_home) = std::env::var_os("HOME").map(PathBuf::from) {
        for candidate in [
            user_home.join(".volta/bin/node"),
            user_home.join(".asdf/shims/node"),
            user_home.join(".local/share/fnm/aliases/default/bin/node"),
        ] {
            if candidate.is_file() {
                return candidate;
            }
        }

        let nvm_versions = user_home.join(".nvm/versions/node");
        if let Ok(entries) = fs::read_dir(nvm_versions) {
            let mut candidates = entries
                .flatten()
                .map(|entry| entry.path().join("bin/node"))
                .filter(|path| path.is_file())
                .collect::<Vec<_>>();
            candidates.sort();
            if let Some(candidate) = candidates.pop() {
                return candidate;
            }
        }
    }

    PathBuf::from("node")
}
