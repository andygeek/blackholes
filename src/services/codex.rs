use crate::{
    model::{CodexProfile, SessionState, TerminalNoticeKind},
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

const CODEX_SESSION_EVENT_PREFIX: &str = "codex-session:";
const MANAGED_HOOK_MARKER: &str = "blackholes-codex-session-hook";
const MAXIMUM_HOOK_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAXIMUM_HOOK_INPUT_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct CodexHookInput {
    session_id: String,
    #[serde(default)]
    hook_event_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionBridgePayload {
    pub terminal_id: Uuid,
    pub session_id: String,
    pub profile: CodexProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<SessionState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_kind: Option<TerminalNoticeKind>,
}

pub fn install_codex_session_hooks() -> Result<()> {
    let executable = env::current_exe().context("Unable to resolve the Blackholes executable")?;
    let command = format!(
        "{} codex-session-hook {}",
        shell_quote(executable.to_string_lossy().as_ref()),
        MANAGED_HOOK_MARKER,
    );

    for codex_home in codex_homes() {
        install_hook_in(&codex_home, &command).with_context(|| {
            format!(
                "Unable to install the Codex session hook in {}",
                codex_home.display()
            )
        })?;
    }
    Ok(())
}

pub fn run_codex_session_hook() -> Result<()> {
    // Hooks must never interfere with Codex. Missing Blackholes context, a
    // closed app, or malformed input simply means there is nothing to record.
    let _ = forward_codex_session();
    Ok(())
}

fn forward_codex_session() -> Result<()> {
    let terminal_id = env::var("BLACKHOLES_TERMINAL_ID")
        .context("The hook is not running inside a Blackholes terminal")?
        .parse::<Uuid>()
        .context("The Blackholes terminal id is invalid")?;
    let mut input = String::new();
    std::io::stdin()
        .take(MAXIMUM_HOOK_INPUT_BYTES)
        .read_to_string(&mut input)?;
    let hook = serde_json::from_str::<CodexHookInput>(&input)?;
    if !valid_session_id(&hook.session_id) {
        bail!("Codex returned an invalid session id");
    }

    let payload = CodexSessionBridgePayload {
        terminal_id,
        session_id: hook.session_id,
        profile: active_codex_profile(),
        state: super::agent_hooks::terminal_state(&hook.hook_event_name),
        notice_kind: super::agent_hooks::notice_kind(&hook.hook_event_name),
    };
    let message = format!(
        "{CODEX_SESSION_EVENT_PREFIX}{}",
        serde_json::to_string(&payload)?
    );
    UnixDatagram::unbound()?.send_to(message.as_bytes(), AppPaths::events_socket_path()?)?;
    Ok(())
}

fn codex_homes() -> Vec<PathBuf> {
    let Some(user_home) = env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let default_home = user_home.join(".codex");
    let work_home = user_home.join(".codex-work");
    let mut homes = vec![default_home.clone()];
    if work_home.is_dir() {
        homes.push(work_home);
    }
    if let Some(configured_home) = env::var_os("CODEX_HOME").map(PathBuf::from)
        && configured_home != default_home
        && !homes.contains(&configured_home)
    {
        homes.push(configured_home);
    }
    homes
}

fn active_codex_profile() -> CodexProfile {
    let configured_home = env::var_os("CODEX_HOME").map(PathBuf::from);
    let default_home = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".codex"));
    match (configured_home, default_home) {
        (Some(configured), Some(default)) if configured != default => CodexProfile::Work,
        _ => CodexProfile::Default,
    }
}

fn install_hook_in(codex_home: &Path, command: &str) -> Result<()> {
    fs::create_dir_all(codex_home)?;
    let hooks_path = codex_home.join("hooks.json");
    let existing = match fs::symlink_metadata(&hooks_path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            if metadata.len() > MAXIMUM_HOOK_FILE_BYTES {
                bail!("{} is larger than 2 MiB", hooks_path.display());
            }
            fs::read(&hooks_path)?
        }
        Ok(_) => bail!("{} is not a regular file", hooks_path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };

    let mut document = if existing.is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_slice::<Value>(&existing)
            .with_context(|| format!("{} is not valid JSON", hooks_path.display()))?
    };
    let root = document
        .as_object_mut()
        .context("The Codex hooks document must be a JSON object")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context("The Codex hooks field must be a JSON object")?;
    for event in [
        "SessionStart", "UserPromptSubmit", "Stop", "PermissionRequest", "Interrupt",
    ] {
        let handlers = hooks
            .entry(event)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .with_context(|| format!("The Codex {event} hooks must be a JSON array"))?;
        // No matcher: track every start source without changing the CLI's decisions.
        super::agent_hooks::upsert(handlers, MANAGED_HOOK_MARKER, command);
    }

    let mut updated = serde_json::to_vec_pretty(&document)?;
    updated.push(b'\n');
    if updated == existing {
        return Ok(());
    }
    write_atomically(codex_home, &hooks_path, &existing, &updated)
}

fn write_atomically(directory: &Path, destination: &Path, expected: &[u8], contents: &[u8]) -> Result<()> {
    let temporary = directory.join(format!(".blackholes-codex-hook-{}.tmp", Uuid::new_v4()));
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
