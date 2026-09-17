use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

/// Stop authentication and metadata helpers, including their child CLIs, on shutdown.
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

    pub fn from_setting(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("codex") => Self::Codex,
            Some("gemini") => Self::Gemini,
            Some("opencode") => Self::OpenCode,
            _ => Self::Claude,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentAuthMode {
    #[default]
    System,
    Isolated,
}

impl AgentAuthMode {
    pub fn id(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Isolated => "isolated",
        }
    }

    pub fn from_setting(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("isolated") => Self::Isolated,
            _ => Self::System,
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
pub fn refresh_agent_plan_usage(
    provider: AgentProvider,
    auth_mode: AgentAuthMode,
    profile: PathBuf,
) -> Result<ProviderPlanUsage> {
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
        "provider": provider.id(), "auth_mode": auth_mode.id(), "auth_profile_dir": profile,
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

#[derive(Debug)]
pub enum AgentAuthEvent {
    Output { text: String },
    OpenUrl { url: String },
    Completed,
    Error { message: String },
}

pub struct AgentAuthStream {
    pub events: flume::Receiver<AgentAuthEvent>,
    pub input: flume::Sender<String>,
    pub cancel: flume::Sender<()>,
}

pub fn start_agent_authentication(
    provider: AgentProvider,
    profiles_root: &Path,
) -> Result<AgentAuthStream> {
    let profile_dir = profiles_root.join(provider.id());
    fs::create_dir_all(&profile_dir)
        .with_context(|| format!("Unable to create {}", profile_dir.display()))?;
    if provider == AgentProvider::Gemini {
        prepare_gemini_oauth_profile(&profile_dir)?;
    }

    let (event_sender, events) = flume::unbounded();
    let (input, input_receiver) = flume::unbounded::<String>();
    let (cancel, cancel_receiver) = flume::bounded(1);
    // Authenticate is explicit consent to Gemini's non-TTY browser sign-in.
    if provider == AgentProvider::Gemini {
        input.send("y".into())?;
    }
    std::thread::Builder::new()
        .name(format!("blackholes-{}-auth", provider.id()))
        .spawn(move || {
            if let Err(error) = run_agent_authentication(
                provider,
                profile_dir,
                event_sender.clone(),
                input_receiver,
                cancel_receiver,
            ) {
                let _ = event_sender.send(AgentAuthEvent::Error {
                    message: format!("{error:#}"),
                });
            }
        })?;
    Ok(AgentAuthStream {
        events,
        input,
        cancel,
    })
}

fn run_agent_authentication(
    provider: AgentProvider,
    profile_dir: PathBuf,
    event_sender: flume::Sender<AgentAuthEvent>,
    input_receiver: flume::Receiver<String>,
    cancel_receiver: flume::Receiver<()>,
) -> Result<()> {
    let (program, args) = authentication_command(provider);
    let agent = super::installed_agents::InstalledAgent::resolve(program, &profile_dir)?;
    if cancel_receiver.try_recv() != Err(flume::TryRecvError::Empty) {
        return Ok(());
    }
    let mut command = agent.command();
    command
        .args(&args)
        .current_dir(&profile_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match provider {
        AgentProvider::Claude => {
            command.env("CLAUDE_CONFIG_DIR", &profile_dir);
        }
        AgentProvider::Codex => {
            command.env("CODEX_HOME", &profile_dir);
        }
        AgentProvider::Gemini => {
            command
                .env("GEMINI_CLI_HOME", &profile_dir)
                .env("GEMINI_DEFAULT_AUTH_TYPE", "oauth-personal");
        }
        AgentProvider::OpenCode => {
            command
                .env("XDG_DATA_HOME", profile_dir.join("data"))
                .env("XDG_CONFIG_HOME", profile_dir.join("config"))
                .env("XDG_CACHE_HOME", profile_dir.join("cache"));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("Unable to start {} authentication", provider.display_name()))?;
    let process_id = child.id();
    let active_process = ActiveProviderHelper::register(process_id);
    let stdin = child
        .stdin
        .take()
        .context("Authentication stdin is unavailable")?;
    let stdout = child
        .stdout
        .take()
        .context("Authentication stdout is unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("Authentication stderr is unavailable")?;
    for (name, reader) in [
        ("stdout", Box::new(stdout) as Box<dyn Read + Send>),
        ("stderr", Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let sender = event_sender.clone();
        std::thread::Builder::new()
            .name(format!("blackholes-auth-{name}"))
            .spawn(move || stream_auth_output(reader, sender))?;
    }
    std::thread::Builder::new()
        .name("blackholes-auth-input".into())
        .spawn(move || write_auth_input(stdin, input_receiver))?;
    std::thread::Builder::new()
        .name("blackholes-auth-cancel".into())
        .spawn(move || {
            if cancel_receiver.recv().is_ok() {
                terminate_helper_process(process_id);
            }
        })?;
    let _active_process = active_process;
    let status = child.wait().context("Authentication process failed")?;
    if !status.success() {
        bail!(
            "{} authentication exited with {status}",
            provider.display_name()
        );
    }
    let _ = event_sender.send(AgentAuthEvent::Completed);
    Ok(())
}

fn authentication_command(provider: AgentProvider) -> (&'static str, Vec<&'static str>) {
    match provider {
        AgentProvider::Claude => ("claude", vec!["auth", "login"]),
        AgentProvider::Codex => ("codex", vec!["login"]),
        AgentProvider::Gemini => ("gemini", vec!["--list-sessions"]),
        AgentProvider::OpenCode => ("opencode", vec!["auth", "login", "--provider", "opencode"]),
    }
}

fn prepare_gemini_oauth_profile(profile_dir: &Path) -> Result<()> {
    // Gemini treats GEMINI_CLI_HOME as the home root and keeps its user
    // configuration and OAuth credentials in the .gemini directory below it.
    let settings_dir = profile_dir.join(".gemini");
    fs::create_dir_all(&settings_dir)?;
    let settings_path = settings_dir.join("settings.json");
    let mut settings = fs::read_to_string(&settings_path)
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    settings["security"]["auth"]["selectedType"] = Value::String("oauth-personal".into());
    fs::write(&settings_path, serde_json::to_vec_pretty(&settings)?)?;
    Ok(())
}

fn write_auth_input(mut stdin: impl Write, input: flume::Receiver<String>) {
    while let Ok(value) = input.recv() {
        if stdin.write_all(value.as_bytes()).is_err()
            || stdin.write_all(b"\n").is_err()
            || stdin.flush().is_err()
        {
            break;
        }
    }
}

fn stream_auth_output(mut reader: Box<dyn Read + Send>, sender: flume::Sender<AgentAuthEvent>) {
    let url_pattern = regex::Regex::new(r#"https?://[^\s\x1b<>\"']+"#)
        .expect("the authentication URL pattern must be valid");
    let ansi_pattern = regex::Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]")
        .expect("the ANSI escape pattern must be valid");
    let mut buffer = [0_u8; 4096];
    let mut rolling = String::new();
    loop {
        let Ok(read) = reader.read(&mut buffer) else {
            break;
        };
        if read == 0 {
            break;
        }
        let raw = String::from_utf8_lossy(&buffer[..read]);
        let clean = ansi_pattern.replace_all(&raw, "").replace('\r', "\n");
        rolling.push_str(&clean);
        for found in url_pattern.find_iter(&rolling) {
            let url = found
                .as_str()
                .trim_end_matches([')', ']', '}', '.', ','])
                .to_string();
            let _ = sender.send(AgentAuthEvent::OpenUrl { url });
        }
        if rolling.len() > 16_384 {
            rolling = rolling.split_off(rolling.len() - 8_192);
        }
        if sender.send(AgentAuthEvent::Output { text: clean }).is_err() {
            break;
        }
    }
}
