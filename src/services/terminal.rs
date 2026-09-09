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
    /// Event-driven fallback for TUIs that do not advertise their provider in an OSC title.
    /// Inspect only executable names, never prompt text or arbitrary command arguments.
    pub fn foreground_agent(process_id: i32) -> Option<AgentKind> {
        let output = std::process::Command::new("ps")
            .args(["-p", &process_id.to_string(), "-o", "comm=", "-o", "args="])
            .output()
            .ok()?;
        if !output.status.success() { return None; }
        let output = String::from_utf8(output.stdout).ok()?;
        let mut words = output.split_whitespace();
        let executable = words.next()?;
        let identify = |value: &str| match Path::new(value).file_name()?.to_str()? {
            "claude" => Some(AgentKind::Claude),
            "codex" => Some(AgentKind::Codex),
            "gemini" => Some(AgentKind::Gemini),
            "opencode" => Some(AgentKind::OpenCode),
            "agy" | "antigravity" => Some(AgentKind::Antigravity),
            _ => None,
        };
        if let Some(agent) = identify(executable) { return Some(agent); }
        // For interpreter-based CLIs, args repeats the interpreter before the script path.
        if matches!(Path::new(executable).file_name()?.to_str()?, "node" | "bun") {
            words.next()?;
            let script = words.next()?;
            return identify(script).or_else(|| {
                let parent = Path::new(script).parent()?;
                let package = if parent.ends_with("bin") { parent.parent()? } else { parent };
                identify(package.to_str()?)
            });
        }
        None
    }

    pub fn spawn(&self, descriptor: &TerminalDescriptor, skip_agent_permissions: bool) -> Result<SpawnedTerminal> {
        self.spawn_with_prompt(descriptor, skip_agent_permissions, None)
    }

    pub fn spawn_with_prompt(
        &self,
        descriptor: &TerminalDescriptor,
        skip_agent_permissions: bool,
        prompt: Option<&str>,
    ) -> Result<SpawnedTerminal> {
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

        let shell = super::installed_agents::login_shell();
        let mut command = CommandBuilder::new(&shell);
        command.arg("-l");
        if let Some(prompt) = prompt {
            let (program, mut args) = prompted_agent_command(descriptor, skip_agent_permissions, prompt)?;
            configure_task_mcp(descriptor, &mut args, &mut command)?;
            // Pass the brief as a process argument, not keystrokes: long/multiline
            // prompts must not hit the PTY line limit or shell-startup timing races.
            command.arg("-i");
            command.arg("-c");
            // Resolve the CLI after interactive login startup, exactly as when
            // typing its name manually. Never pin tasks to an app-owned binary.
            command.arg(format!("{}; exec {} -l", render_command(program, &args).trim_end_matches('\r'), shell_quote(&shell)));
        }
        command.cwd(&descriptor.cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("TERM_PROGRAM", "Blackholes");
        command.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
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
        command.env("BLACKHOLES_AGENT_PROVIDER", agent_name(descriptor.agent));
        command.env("BLACKHOLES_AGENT_CONFIG_DIR", descriptor.agent_config_dir.as_deref().unwrap_or(Path::new("")));
        if let Some(config_dir) = &descriptor.agent_config_dir {
            match descriptor.agent {
                AgentKind::Codex => command.env("CODEX_HOME", config_dir),
                AgentKind::Claude => command.env("CLAUDE_CONFIG_DIR", config_dir),
                AgentKind::Gemini => command.env("GEMINI_CLI_HOME", config_dir),
                _ => {},
            }
        }

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

        if prompt.is_none() && let Some((program, args)) = initial_agent_command(descriptor, skip_agent_permissions) {
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

fn prompted_agent_command(
    descriptor: &TerminalDescriptor,
    skip_permissions: bool,
    prompt: &str,
) -> Result<(&'static str, Vec<String>)> {
    if prompt.trim().is_empty() || prompt.contains('\0') {
        bail!("The initial prompt must be non-empty and contain no NUL");
    }
    if descriptor.agent == AgentKind::Antigravity {
        bail!("Automatic task prompts are not supported by the Antigravity terminal launcher. Select codex, claude, gemini, or opencode");
    }
    let (program, mut args) = initial_agent_command(descriptor, skip_permissions)
        .context("A task needs a coding agent, not a plain shell")?;
    if skip_permissions && descriptor.agent == AgentKind::Codex {
        // Trust only this task for this invocation; never rewrite ~/.codex/config.toml.
        let path = serde_json::to_string(&descriptor.cwd.to_string_lossy())?;
        args.extend(["-c".into(), format!("projects={{ {path} = {{ trust_level = \"trusted\" }} }}")]);
    }
    if skip_permissions && descriptor.agent == AgentKind::Claude {
        // Merge into the existing one-session settings argument, preserving notifications.
        if let Some(index) = args.iter().position(|arg| arg == "--settings") {
            let mut settings: serde_json::Value = serde_json::from_str(&args[index + 1])?;
            settings["skipDangerousModePermissionPrompt"] = true.into();
            args[index + 1] = serde_json::to_string(&settings)?;
        }
    }
    if skip_permissions && descriptor.agent == AgentKind::Gemini {
        args.push("--skip-trust".into());
    }
    match descriptor.agent {
        AgentKind::Gemini => args.push("--prompt-interactive".into()),
        AgentKind::OpenCode => args.push("--prompt".into()),
        _ => args.push("--".into()),
    }
    args.push(prompt.into());
    Ok((program, args))
}

fn configure_task_mcp(
    descriptor: &TerminalDescriptor,
    args: &mut Vec<String>,
    command: &mut CommandBuilder,
) -> Result<()> {
    let executable = env::current_exe().context("Could not locate the Blackholes MCP executable")?;
    let environment = serde_json::json!({
        "BLACKHOLES_WORKSPACE_ID": descriptor.workspace_id.to_string(),
        "BLACKHOLES_TASK_ID": descriptor.task_id.map(|id| id.to_string()).unwrap_or_default(),
        "BLACKHOLES_TERMINAL_ID": descriptor.id.to_string(),
        "BLACKHOLES_AGENT_PROVIDER": agent_name(descriptor.agent),
        "BLACKHOLES_AGENT_CONFIG_DIR": descriptor.agent_config_dir.as_deref().unwrap_or(Path::new("")),
    });
    let server = serde_json::json!({ "command": executable, "args": ["mcp"], "env": environment });
    let mut options = Vec::new();
    match descriptor.agent {
        AgentKind::Codex => {
            // A TOML inline table replaces a stale server entry for this invocation only.
            let env = environment.as_object().context("Invalid MCP environment")?.iter()
                .map(|(key, value)| format!("{key} = {value}"))
                .collect::<Vec<_>>().join(", ");
            options.extend(["-c".into(), format!(
                "mcp_servers.blackholes={{ command = {}, args = [\"mcp\"], env = {{ {env} }}, enabled = true }}",
                serde_json::to_string(&executable)?,
            )]);
        }
        AgentKind::Claude => options.extend([
            "--mcp-config".into(), serde_json::json!({ "mcpServers": { "blackholes": server } }).to_string(),
        ]),
        AgentKind::Gemini => {
            // Gemini uses workspace settings. Only add our server in the managed
            // task container, preserving any other settings and provider permissions.
            let directory = descriptor.cwd.join(".gemini");
            let path = directory.join("settings.json");
            let mut settings: serde_json::Value = match std::fs::read_to_string(&path) {
                Ok(content) => serde_json::from_str(&content).context("Invalid task Gemini settings")?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
                Err(error) => return Err(error.into()),
            };
            set_task_mcp_entry(&mut settings, "mcpServers", server)?;
            std::fs::create_dir_all(directory)?;
            std::fs::write(path, serde_json::to_vec_pretty(&settings)?)?;
        }
        AgentKind::OpenCode => {
            let mut settings: serde_json::Value = match env::var("OPENCODE_CONFIG_CONTENT") {
                Ok(value) if !value.trim().is_empty() => serde_json::from_str(&value)
                    .context("Invalid inherited OpenCode inline configuration")?,
                _ => serde_json::json!({}),
            };
            set_task_mcp_entry(&mut settings, "mcp", serde_json::json!({
                "type": "local", "command": [executable, "mcp"],
                "environment": environment, "enabled": true,
            }))?;
            command.env("OPENCODE_CONFIG_CONTENT", serde_json::to_string(&settings)?);
        }
        _ => {},
    }
    // Provider options precede the positional-prompt separator.
    args.splice(0..0, options);
    Ok(())
}

fn set_task_mcp_entry(settings: &mut serde_json::Value, key: &str, server: serde_json::Value) -> Result<()> {
    let settings = settings.as_object_mut().context("Agent settings must be an object")?;
    let servers = settings.entry(key).or_insert_with(|| serde_json::json!({}));
    servers.as_object_mut().context("Agent MCP settings must be an object")?
        .insert("blackholes".into(), server);
    Ok(())
}

fn initial_agent_command(
    descriptor: &TerminalDescriptor,
    skip_permissions: bool,
) -> Option<(&'static str, Vec<String>)> {
    let (program, mut args) = session_agent_command(descriptor)?;
    // Apply the current project preference at spawn time, not the preference
    // from the saved terminal. Disabling it must also affect resumed sessions.
    // Do not export permission settings into the shell or rewrite user configs.
    if skip_permissions && let Some(flag) = permission_bypass_flag(descriptor.agent) {
        args.push(flag.into());
    }
    Some((program, args))
}

fn permission_bypass_flag(agent: AgentKind) -> Option<&'static str> {
    match agent {
        AgentKind::Shell => None,
        AgentKind::Claude | AgentKind::Antigravity => Some("--dangerously-skip-permissions"),
        AgentKind::Codex => Some("--dangerously-bypass-approvals-and-sandbox"),
        AgentKind::Gemini => Some("--approval-mode=yolo"),
        // OpenCode auto-approves requests but preserves explicit deny rules.
        AgentKind::OpenCode => Some("--auto"),
    }
}

fn session_agent_command(descriptor: &TerminalDescriptor) -> Option<(&'static str, Vec<String>)> {
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

fn agent_name(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Shell => "shell",
        AgentKind::Codex => "codex",
        AgentKind::Claude => "claude",
        AgentKind::Gemini => "gemini",
        AgentKind::OpenCode => "opencode",
        AgentKind::Antigravity => "antigravity",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ClaudeSession, CodexSession, SessionState};

    fn descriptor(agent: AgentKind) -> TerminalDescriptor {
        TerminalDescriptor {
            id: uuid::Uuid::new_v4(),
            workspace_id: uuid::Uuid::new_v4(),
            task_id: None,
            repository_id: None,
            agent,
            label: String::new(),
            cwd: "/tmp".into(),
            state: SessionState::Restored,
            codex_session: None,
            claude_session: None,
            agent_config_dir: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn permission_bypass_is_opt_in_for_each_provider() {
        for (agent, flag) in [
            (AgentKind::Claude, "--dangerously-skip-permissions"),
            (AgentKind::Codex, "--dangerously-bypass-approvals-and-sandbox"),
            (AgentKind::Antigravity, "--dangerously-skip-permissions"),
            (AgentKind::OpenCode, "--auto"),
            (AgentKind::Gemini, "--approval-mode=yolo"),
        ] {
            let terminal = descriptor(agent);
            let normal = initial_agent_command(&terminal, false).unwrap();
            assert_eq!(normal, agent.command().unwrap());
            let mut expected = normal;
            expected.1.push(flag.into());
            assert_eq!(initial_agent_command(&terminal, true).unwrap(), expected);
        }
        for enabled in [false, true] {
            assert!(initial_agent_command(&descriptor(AgentKind::Shell), enabled).is_none());
        }
    }

    #[test]
    fn restored_sessions_keep_profile_and_id_with_current_preference() {
        for (agent, program, resume, flag) in [
            (AgentKind::Claude, "claude-work", "--resume", "--dangerously-skip-permissions"),
            (AgentKind::Codex, "codex-work", "resume", "--dangerously-bypass-approvals-and-sandbox"),
        ] {
            let mut terminal = descriptor(agent);
            // Task terminals use the same startup path as project terminals.
            terminal.task_id = Some(uuid::Uuid::new_v4());
            match agent {
                AgentKind::Claude => terminal.claude_session = Some(ClaudeSession {
                    id: "saved-session-id".into(), profile: ClaudeProfile::Work,
                }),
                AgentKind::Codex => terminal.codex_session = Some(CodexSession {
                    id: "saved-session-id".into(), profile: CodexProfile::Work,
                }),
                _ => unreachable!(),
            }
            for enabled in [true, false] {
                let mut args = agent.command().unwrap().1;
                args.extend([resume.into(), "saved-session-id".into()]);
                if enabled { args.push(flag.into()); }
                assert_eq!(initial_agent_command(&terminal, enabled), Some((program, args)));
            }
        }
    }
}
