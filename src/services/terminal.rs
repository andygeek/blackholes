use crate::model::{AgentKind, ClaudeProfile, CodexProfile, TerminalDescriptor};
use anyhow::{Context, Result, bail};
use parking_lot::Mutex;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    env,
    io::{Read, Write},
    path::Path,
    sync::Arc,
};

pub type SharedMasterPty = Arc<Mutex<Box<dyn MasterPty + Send>>>;
pub type SharedChild = Arc<Mutex<Box<dyn Child + Send + Sync>>>;

/// The native process-side resources for one terminal.
///
/// Rendering owns `reader` and `writer`; this value keeps only the PTY master
/// and child handles needed for resize and explicit shutdown. There is no
/// background status poller here: terminal output is push-driven by GPUI.
pub struct SpawnedTerminal {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub master: SharedMasterPty,
    pub child: SharedChild,
    pub process_id: Option<u32>,
}

#[derive(Clone, Default)]
pub struct TerminalService;

impl TerminalService {
    pub fn spawn(&self, descriptor: &TerminalDescriptor) -> Result<SpawnedTerminal> {
        if !descriptor.cwd.is_dir() {
            bail!(
                "terminal directory does not exist: {}",
                descriptor.cwd.display()
            );
        }
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("could not allocate a native PTY")?;

        let shell = login_shell();
        let mut command = CommandBuilder::new(&shell);
        command.arg("-l");
        command.cwd(&descriptor.cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("TERM_PROGRAM", "Blackholes");
        command.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        // Make the app's runtimes available in its terminals as well, without
        // installing anything into the user's shell profile or global PATH.
        if let Ok(executable) = env::current_exe() {
            if let Some(contents) = executable.parent().and_then(Path::parent) {
                let node_bin = contents.join("Resources/node/bin");
                if node_bin.join("node").is_file() {
                    let mut directories = vec![node_bin];
                    if let Some(path) = env::var_os("PATH") { directories.extend(env::split_paths(&path)); }
                    directories.push(contents.join("Resources/agent-sidecar/node_modules/.bin"));
                    if let Ok(path) = env::join_paths(directories) { command.env("PATH", path); }
                }
            }
        }
        // Advertise compatibility with the structured OSC 777 CLI-agent
        // protocol used by Warp's public Claude plugin. TERM_PROGRAM remains
        // truthful, while users who already have that plugin installed get
        // exact lifecycle events in Blackholes too.
        command.env("WARP_CLI_AGENT_PROTOCOL_VERSION", "1");
        command.env(
            "WARP_CLIENT_VERSION",
            format!("blackholes-v{}", env!("CARGO_PKG_VERSION")),
        );
        command.env(
            "BLACKHOLES_WORKSPACE_ID",
            descriptor.workspace_id.to_string(),
        );
        command.env("BLACKHOLES_TERMINAL_ID", descriptor.id.to_string());
        command.env("BLACKHOLES_AGENT", agent_name(descriptor.agent));

        if let Some(task_id) = descriptor.task_id {
            command.env("BLACKHOLES_TASK_ID", task_id.to_string());
            command.env("COMPOSE_PROJECT_NAME", compose_project_name(task_id));
        }
        if let Some(repository_id) = descriptor.repository_id {
            command.env("BLACKHOLES_REPOSITORY_ID", repository_id.to_string());
        }

        let child = pair
            .slave
            .spawn_command(command)
            .with_context(|| format!("could not start login shell {shell}"))?;
        let process_id = child.process_id();

        let reader = pair
            .master
            .try_clone_reader()
            .context("could not open terminal output")?;
        let mut writer = pair
            .master
            .take_writer()
            .context("could not open terminal input")?;

        if let Some((program, args)) = initial_agent_command(descriptor) {
            let initial_command = render_command(program, &args);
            writer
                .write_all(initial_command.as_bytes())
                .context("could not launch the selected coding agent")?;
            writer.flush().ok();
        }

        Ok(SpawnedTerminal {
            reader,
            writer,
            master: Arc::new(Mutex::new(pair.master)),
            child: Arc::new(Mutex::new(child)),
            process_id,
        })
    }
}

fn initial_agent_command(descriptor: &TerminalDescriptor) -> Option<(&'static str, Vec<String>)> {
    if descriptor.agent == AgentKind::Claude
        && let Some(session) = descriptor.claude_session.as_ref()
    {
        let program = match session.profile {
            ClaudeProfile::Default => "claude",
            ClaudeProfile::Work => "claude-work",
        };
        let mut args = AgentKind::Claude.command()?.1;
        args.push("--resume".into());
        args.push(session.id.clone());
        return Some((program, args));
    }
    if descriptor.agent == AgentKind::Codex
        && let Some(session) = descriptor.codex_session.as_ref()
    {
        let program = match session.profile {
            CodexProfile::Default => "codex",
            CodexProfile::Work => "codex-work",
        };
        let mut args = AgentKind::Codex.command()?.1;
        args.push("resume".into());
        args.push(session.id.clone());
        return Some((program, args));
    }
    descriptor.agent.command()
}

fn login_shell() -> String {
    env::var("SHELL")
        .ok()
        .filter(|shell| Path::new(shell).is_file())
        .unwrap_or_else(|| "/bin/zsh".into())
}

fn agent_name(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Shell => "shell",
        AgentKind::Codex => "codex",
        AgentKind::Claude => "claude",
        AgentKind::Gemini => "gemini",
    }
}

fn compose_project_name(task_id: uuid::Uuid) -> String {
    let compact = task_id.simple().to_string();
    format!("blackholes_{}", &compact[..12])
}

fn render_command(program: &str, args: &[String]) -> String {
    let mut rendered = shell_quote(program);
    for argument in args {
        rendered.push(' ');
        rendered.push_str(&shell_quote(argument));
    }
    rendered.push_str("\r");
    rendered
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "-._/:=".contains(character))
    {
        return value.into();
    }

    format!("'{}'", value.replace('\'', "'\\''"))
}
