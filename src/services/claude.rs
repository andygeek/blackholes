use crate::{
    model::{ClaudeProfile, SessionState, TerminalNoticeKind},
    paths::AppPaths,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    env,
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::{fs::OpenOptionsExt, net::UnixDatagram},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const CLAUDE_SESSION_EVENT_PREFIX: &str = "claude-session:";
const MANAGED_HOOK_MARKER: &str = "blackholes-claude-session-hook";
const MAXIMUM_SETTINGS_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAXIMUM_HOOK_INPUT_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct ClaudeHookInput {
    session_id: String,
    #[serde(default)]
    hook_event_name: String,
    #[serde(default)]
    last_assistant_message: Option<String>,
    #[serde(default)]
    notification_type: Option<String>,
    /// Stop only. Kept loose so a format change cannot drop the whole event.
    #[serde(default)]
    background_tasks: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSessionBridgePayload {
    pub terminal_id: Uuid,
    pub session_id: String,
    pub profile: ClaudeProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<SessionState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_kind: Option<TerminalNoticeKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_preview: Option<String>,
    /// How many background tasks are still running, when the event knows:
    /// Stop reports them and a new session starts with none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_tasks: Option<usize>,
}

pub fn install_claude_session_hooks() -> Result<()> {
    let executable = env::current_exe().context("Unable to resolve the Blackholes executable")?;
    let command = format!(
        "{} claude-session-hook {}",
        shell_quote(executable.to_string_lossy().as_ref()),
        MANAGED_HOOK_MARKER,
    );

    for claude_home in claude_homes() {
        install_hooks_in(&claude_home, &command).with_context(|| {
            format!(
                "Unable to install the Claude session hooks in {}",
                claude_home.display()
            )
        })?;
    }
    Ok(())
}

pub fn run_claude_session_hook() -> Result<()> {
    // Hooks must never interfere with Claude. Missing Blackholes context, a
    // closed app, or malformed input simply means there is nothing to record.
    let _ = forward_claude_session();
    Ok(())
}

fn forward_claude_session() -> Result<()> {
    let terminal_id = env::var("BLACKHOLES_TERMINAL_ID")
        .context("The hook is not running inside a Blackholes terminal")?
        .parse::<Uuid>()
        .context("The Blackholes terminal id is invalid")?;
    let mut input = String::new();
    std::io::stdin()
        .take(MAXIMUM_HOOK_INPUT_BYTES)
        .read_to_string(&mut input)?;
    let message = bridge_message(&input, terminal_id, active_claude_profile())?;
    UnixDatagram::unbound()?.send_to(message.as_bytes(), AppPaths::events_socket_path()?)?;
    Ok(())
}

/// Turns one hook input into the event the app receives.
fn bridge_message(input: &str, terminal_id: Uuid, profile: ClaudeProfile) -> Result<String> {
    let hook = serde_json::from_str::<ClaudeHookInput>(input)?;
    if !valid_session_id(&hook.session_id) {
        bail!("Claude returned an invalid session id");
    }
    let event = hook.hook_event_name.as_str();
    let payload = ClaudeSessionBridgePayload {
        terminal_id,
        session_id: hook.session_id,
        profile,
        state: if event == "Notification"
            && hook.notification_type.as_deref().is_some_and(|kind| {
                !matches!(kind, "permission_prompt" | "idle_prompt" | "elicitation_dialog")
            })
        {
            None
        } else {
            super::agent_hooks::terminal_state(event)
        },
        // An idle reminder must not turn an already completed response yellow.
        notice_kind: if hook.notification_type.as_deref() == Some("idle_prompt") {
            None
        } else {
            super::agent_hooks::notice_kind(event)
        },
        response_preview: super::agent_notice::response_preview(event, hook.last_assistant_message.as_deref()),
        background_tasks: match event {
            "Stop" => Some(super::agent_hooks::background_tasks(&hook.background_tasks)),
            "SessionStart" => Some(0),
            _ => None,
        },
    };
    Ok(format!("{CLAUDE_SESSION_EVENT_PREFIX}{}", serde_json::to_string(&payload)?))
}

/// Claude configuration folders on this computer: default, work and `CLAUDE_CONFIG_DIR`.
pub fn claude_homes() -> Vec<PathBuf> {
    let Some(user_home) = env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let default_home = user_home.join(".claude");
    let work_home = user_home.join(".claude-work");
    let mut homes = vec![default_home.clone()];
    if work_home.is_dir() {
        homes.push(work_home);
    }
    if let Some(configured_home) = env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from)
        && configured_home != default_home
        && !homes.contains(&configured_home)
    {
        homes.push(configured_home);
    }
    homes
}

fn active_claude_profile() -> ClaudeProfile {
    let configured_home = env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
    let default_home = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".claude"));
    match (configured_home, default_home) {
        (Some(configured), Some(default)) if configured != default => ClaudeProfile::Work,
        _ => ClaudeProfile::Default,
    }
}

fn install_hooks_in(claude_home: &Path, command: &str) -> Result<()> {
    fs::create_dir_all(claude_home)?;
    let settings_path = claude_home.join("settings.json");
    let existing = match fs::symlink_metadata(&settings_path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            if metadata.len() > MAXIMUM_SETTINGS_FILE_BYTES {
                bail!("{} is larger than 2 MiB", settings_path.display());
            }
            fs::read(&settings_path)?
        }
        Ok(_) => bail!("{} is not a regular file", settings_path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };

    let mut document = if existing.is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_slice::<Value>(&existing)
            .with_context(|| format!("{} is not valid JSON", settings_path.display()))?
    };
    let root = document
        .as_object_mut()
        .context("The Claude settings document must be a JSON object")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context("The Claude hooks field must be a JSON object")?;

    for event in [
        "SessionStart", "UserPromptSubmit", "Stop", "StopFailure", "PermissionRequest", "Notification", "PreToolUse",
    ] {
        let handlers = hooks
            .entry(event)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .with_context(|| format!("The Claude {event} hooks must be a JSON array"))?;
        super::agent_hooks::upsert(handlers, MANAGED_HOOK_MARKER, command);
        if event == "PreToolUse" {
            if let Some(group) = handlers.last_mut() {
                group["matcher"] = Value::String("^(AskUserQuestion|ExitPlanMode)$".into());
            }
        }
    }

    let mut updated = serde_json::to_vec_pretty(&document)?;
    updated.push(b'\n');
    if updated == existing {
        return Ok(());
    }
    write_atomically(claude_home, &settings_path, &existing, &updated)
}

fn write_atomically(directory: &Path, destination: &Path, expected: &[u8], contents: &[u8]) -> Result<()> {
    let temporary = directory.join(format!(".blackholes-claude-hook-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        let current = fs::read(destination).or_else(|error| {
            if error.kind() == std::io::ErrorKind::NotFound { Ok(Vec::new()) } else { Err(error) }
        })?;
        if current != expected { bail!("Hook configuration changed during setup; retry later"); }
        fs::rename(&temporary, destination)?;
        Ok::<_, anyhow::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn valid_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id.len() <= 200
        && session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}


#[cfg(test)]
mod tests {
    use super::*;

    // Hook inputs captured from Claude Code 2.1.326, with ids and paths replaced.
    const STOP_WITH_TASK: &str = r#"{"session_id":"71987d0d-0000-4000-8000-000000000000","transcript_path":"/tmp/claude/session.jsonl","cwd":"/tmp/project","prompt_id":"c57afdb3-95f2-4415-b047-564fc47e8938","permission_mode":"auto","effort":{"level":"medium"},"hook_event_name":"Stop","stop_hook_active":false,"last_assistant_message":"Lo deje corriendo, quieres algo mas?","background_tasks":[{"id":"b85lra38y","type":"shell","status":"running","description":"Sleep 10 seconds then echo listo","command":"sleep 10; echo listo"}],"session_crons":[]}"#;
    const TASK_NOTIFICATION: &str = r#"{"session_id":"71987d0d-0000-4000-8000-000000000000","transcript_path":"/tmp/claude/session.jsonl","cwd":"/tmp/project","prompt_id":"9d3fcc5d-5871-4a2a-af5f-5df871861529","permission_mode":"auto","hook_event_name":"UserPromptSubmit","prompt":"<task-notification>\n<task-id>b85lra38y</task-id>\n<status>completed</status>\n</task-notification>"}"#;
    const STOP: &str = r#"{"session_id":"71987d0d-0000-4000-8000-000000000000","transcript_path":"/tmp/claude/session.jsonl","cwd":"/tmp/project","prompt_id":"9d3fcc5d-5871-4a2a-af5f-5df871861529","permission_mode":"auto","effort":{"level":"medium"},"hook_event_name":"Stop","stop_hook_active":false,"last_assistant_message":"DONE","background_tasks":[],"session_crons":[]}"#;

    fn payload(input: &str) -> ClaudeSessionBridgePayload {
        let message = bridge_message(input, Uuid::nil(), ClaudeProfile::Default).unwrap();
        serde_json::from_str(message.strip_prefix(CLAUDE_SESSION_EVENT_PREFIX).unwrap()).unwrap()
    }

    #[test]
    fn reports_background_work_left_running_by_a_finished_turn() {
        // The turn is announced as before; the count only marks the session.
        let stopped = payload(STOP_WITH_TASK);
        assert_eq!((stopped.state, stopped.notice_kind), (Some(SessionState::Attention), Some(TerminalNoticeKind::Completed)));
        assert_eq!(stopped.response_preview.as_deref(), Some("Lo deje corriendo, quieres algo mas?"));
        assert_eq!(stopped.background_tasks, Some(1));

        // Claude Code submits the finished task as a prompt: it is working
        // again, and the next Stop says what is left.
        let woke = payload(TASK_NOTIFICATION);
        assert_eq!((woke.state, woke.background_tasks), (Some(SessionState::Working), None));
        assert_eq!(payload(STOP).background_tasks, Some(0));
    }

    #[test]
    fn keeps_events_that_change_shape() {
        let odd = payload(r#"{"session_id":"abc","hook_event_name":"Stop","background_tasks":"soon","extra":{"a":1}}"#);
        assert_eq!((odd.state, odd.background_tasks), (Some(SessionState::Attention), Some(0)));
        assert_eq!(payload(r#"{"session_id":"abc","hook_event_name":"Stop"}"#).background_tasks, Some(0));
        assert_eq!(payload(r#"{"session_id":"abc","hook_event_name":"SessionStart"}"#).background_tasks, Some(0));
        assert_eq!(payload(r#"{"session_id":"abc","hook_event_name":"PermissionRequest"}"#).background_tasks, None);
        assert!(bridge_message(r#"{"session_id":"../x","hook_event_name":"Stop"}"#, Uuid::nil(), ClaudeProfile::Default).is_err());
    }
}
