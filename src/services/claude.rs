use crate::{model::ClaudeProfile, paths::AppPaths};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
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
const MAXIMUM_HOOK_INPUT_BYTES: u64 = 64 * 1024;

#[derive(Debug, Deserialize)]
struct ClaudeHookInput {
    session_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSessionBridgePayload {
    pub terminal_id: Uuid,
    pub session_id: String,
    pub profile: ClaudeProfile,
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
    let hook = serde_json::from_str::<ClaudeHookInput>(&input)?;
    if !valid_session_id(&hook.session_id) {
        bail!("Claude returned an invalid session id");
    }

    let payload = ClaudeSessionBridgePayload {
        terminal_id,
        session_id: hook.session_id,
        profile: active_claude_profile(),
    };
    let message = format!(
        "{CLAUDE_SESSION_EVENT_PREFIX}{}",
        serde_json::to_string(&payload)?
    );
    let paths = AppPaths::discover()?;
    UnixDatagram::unbound()?.send_to(message.as_bytes(), paths.events_socket)?;
    Ok(())
}

fn claude_homes() -> Vec<PathBuf> {
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

    for event in ["SessionStart", "UserPromptSubmit"] {
        let handlers = hooks
            .entry(event)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .with_context(|| format!("The Claude {event} hooks must be a JSON array"))?;
        handlers.retain(|entry| !is_managed_hook(entry));
        handlers.push(json!({
            "hooks": [{
                "type": "command",
                "command": command,
                "timeout": 3
            }]
        }));
    }

    let mut updated = serde_json::to_vec_pretty(&document)?;
    updated.push(b'\n');
    if updated == existing {
        return Ok(());
    }
    write_atomically(claude_home, &settings_path, &updated)
}

fn is_managed_hook(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|handlers| {
            handlers.iter().any(|handler| {
                handler
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|command| command.contains(MANAGED_HOOK_MARKER))
            })
        })
}

fn write_atomically(directory: &Path, destination: &Path, contents: &[u8]) -> Result<()> {
    let temporary = directory.join(format!(".blackholes-claude-hook-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
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
