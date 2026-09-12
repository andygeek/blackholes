//! First-launch/update setup for external CLI clients. No provider processes,
//! logins, shell-profile changes, or repository installation scripts are needed.
use crate::paths::AppPaths;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    env, fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Mutex,
};
use toml_edit::{DocumentMut, Item, Table, value};

const SKILL: &str = include_str!("../../integrations/skills/blackholes/SKILL.md");
const MARKER: &str = "managed-by: blackholes-ai-integrations";
const MAX_CONFIG_BYTES: u64 = 8 * 1024 * 1024;
static SYNC_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Default, Serialize)]
pub struct IntegrationStatus {
    pub running: bool,
    pub profiles: Vec<ProfileStatus>,
    pub error: Option<String>,
    pub install_required: bool,
}

#[derive(Clone, Serialize)]
pub struct ProfileStatus {
    pub client: &'static str,
    pub config_path: PathBuf,
    pub configured: bool,
    pub changed: bool,
    pub error: Option<String>,
    pub skill_warning: Option<String>,
}

struct Profile {
    client: &'static str,
    config: PathBuf,
    skills: PathBuf,
}

pub fn synchronize(paths: &AppPaths) -> IntegrationStatus {
    let _guard = SYNC_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut status = IntegrationStatus::default();
    let result = (|| -> Result<()> {
        let executable = env::current_exe().context("Could not locate Blackholes")?;
        // Disk images and Gatekeeper's temporary copies disappear after installation.
        if executable.starts_with("/Volumes")
            || executable
                .components()
                .any(|part| part.as_os_str() == "AppTranslocation")
        {
            status.install_required = true;
            return Ok(());
        }
        let home = directories::BaseDirs::new()
            .context("Could not locate the user home folder")?
            .home_dir()
            .to_path_buf();
        let launcher = paths.data_dir.join("integrations/blackholes-mcp");
        let launcher_contents = format!(
            "#!/bin/sh\n# {MARKER}\nexec {} mcp \"$@\"\n",
            shell_quote(&executable.to_string_lossy()),
        );
        let before = read_file(&launcher)?;
        if before.as_deref().is_some_and(|text| !text.contains(MARKER)) {
            bail!("The Blackholes MCP launcher has custom contents; it was preserved");
        }
        write_if_changed(&launcher, before.as_deref(), &launcher_contents, 0o700)?;
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700))?;

        for profile in profiles(&home) {
            let result = if profile.client == "Codex" {
                configure_codex(&profile.config, &launcher)
            } else {
                let config_dir = profile
                    .config
                    .parent()
                    .filter(|directory| *directory != home);
                configure_claude(&profile.config, &launcher, config_dir)
            };
            let mut report = ProfileStatus {
                client: profile.client,
                config_path: profile.config,
                configured: result.is_ok(),
                changed: result.as_ref().copied().unwrap_or(false),
                error: result.err().map(|error| error.to_string()),
                skill_warning: None,
            };
            // A custom skill is preserved and does not prevent the MCP from working.
            if report.configured {
                match install_skill(&profile.skills) {
                    Ok(changed) => report.changed |= changed,
                    Err(error) => report.skill_warning = Some(error.to_string()),
                }
            }
            status.profiles.push(report);
        }
        Ok(())
    })();
    status.error = result.err().map(|error| error.to_string());
    status
}

fn profiles(home: &Path) -> Vec<Profile> {
    // Prepare default profiles even on a clean machine, before a CLI is first used.
    let mut profiles = vec![
        Profile {
            client: "Codex",
            config: home.join(".codex/config.toml"),
            skills: home.join(".codex/skills"),
        },
        Profile {
            client: "Claude Code",
            config: home.join(".claude.json"),
            skills: home.join(".claude/skills"),
        },
    ];
    for (variable, client, file) in [
        ("CODEX_HOME", "Codex", "config.toml"),
        ("CLAUDE_CONFIG_DIR", "Claude Code", ".claude.json"),
    ] {
        if let Some(root) = env::var_os(variable)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            && root.is_absolute()
        {
            profiles.push(Profile {
                client,
                config: root.join(file),
                skills: root.join("skills"),
            });
        }
    }
    // Also cover the secondary profiles already supported by session restoration.
    for (directory, client, file) in [
        (".codex-work", "Codex", "config.toml"),
        (".claude-work", "Claude Code", ".claude.json"),
    ] {
        let root = home.join(directory);
        if root.is_dir() {
            profiles.push(Profile {
                client,
                config: root.join(file),
                skills: root.join("skills"),
            });
        }
    }
    // Earlier versions of the script explicitly used CLAUDE_CONFIG_DIR=~/.claude.
    let legacy = home.join(".claude/.claude.json");
    if legacy.is_file() {
        profiles.push(Profile {
            client: "Claude Code",
            config: legacy,
            skills: home.join(".claude/skills"),
        });
    }
    let mut seen = std::collections::HashSet::new();
    profiles.retain(|profile| seen.insert(profile.config.clone()));
    profiles
}

fn configure_codex(path: &Path, launcher: &Path) -> Result<bool> {
    let before = read_file(path)?;
    let mut document = before
        .as_deref()
        .unwrap_or("")
        .parse::<DocumentMut>()
        .map_err(|_| anyhow::anyhow!("Invalid TOML; the existing configuration was preserved"))?;
    if document.get("mcp_servers").is_none() {
        document["mcp_servers"] = Item::Table(Table::new());
    }
    let servers = document["mcp_servers"]
        .as_table_like_mut()
        .context("mcp_servers must be a table; the existing configuration was preserved")?;
    if let Some(server) = servers.get("blackholes") {
        let command = server.get("command").and_then(Item::as_str);
        let args = server.get("args").and_then(Item::as_array);
        let legacy = args.is_some_and(|args| {
            args.len() == 1 && args.get(0).and_then(toml_edit::Value::as_str) == Some("mcp")
        });
        if !owned_command(command, legacy, launcher) || server.get("url").is_some() {
            bail!("A custom MCP named blackholes already exists; it was preserved");
        }
        if server.get("enabled").and_then(Item::as_bool) == Some(false) {
            bail!("The Blackholes MCP was disabled in this profile; that preference was preserved");
        }
    } else {
        servers.insert("blackholes", Item::Table(Table::new()));
    }
    let server = servers
        .get_mut("blackholes")
        .and_then(Item::as_table_like_mut)
        .context("The blackholes MCP entry must be a table")?;
    server.insert("command", value(launcher.to_string_lossy().as_ref()));
    server.insert("args", value(toml_edit::Array::new()));
    if server.get("env").is_none() {
        server.insert("env", Item::Table(Table::new()));
    }
    let environment = server
        .get_mut("env")
        .and_then(Item::as_table_like_mut)
        .context("The Blackholes MCP environment must be a table")?;
    environment.insert("BLACKHOLES_AGENT_PROVIDER", value("codex"));
    environment.insert(
        "BLACKHOLES_AGENT_CONFIG_DIR",
        value(
            path.parent()
                .context("Missing Codex profile directory")?
                .to_string_lossy()
                .as_ref(),
        ),
    );
    write_if_changed(path, before.as_deref(), &document.to_string(), 0o600)
}

fn configure_claude(path: &Path, launcher: &Path, config_dir: Option<&Path>) -> Result<bool> {
    let before = read_file(path)?;
    let mut document: Value = serde_json::from_str(before.as_deref().unwrap_or("{}"))
        .map_err(|_| anyhow::anyhow!("Invalid JSON; the existing configuration was preserved"))?;
    let root = document
        .as_object_mut()
        .context("The Claude configuration must be an object")?;
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("mcpServers must be an object")?;
    if let Some(server) = servers.get("blackholes") {
        let legacy = server.get("args") == Some(&json!(["mcp"]));
        if !owned_command(
            server.get("command").and_then(Value::as_str),
            legacy,
            launcher,
        ) || server.get("url").is_some()
        {
            bail!("A custom MCP named blackholes already exists; it was preserved");
        }
    }
    let server = servers
        .entry("blackholes")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("The blackholes MCP entry must be an object")?;
    server.insert("type".into(), "stdio".into());
    server.insert("command".into(), json!(launcher));
    server.insert("args".into(), json!([]));
    let environment = server
        .entry("env")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("The Blackholes MCP environment must be an object")?;
    environment.insert("BLACKHOLES_AGENT_PROVIDER".into(), "claude".into());
    environment.insert(
        "BLACKHOLES_AGENT_CONFIG_DIR".into(),
        json!(config_dir.unwrap_or(Path::new(""))),
    );
    // Avoid rewriting the document on every launch just to normalize JSON spacing.
    if let Some(before) = &before
        && serde_json::from_str::<Value>(before).ok().as_ref() == Some(&document)
    {
        return Ok(false);
    }
    write_if_changed(
        path,
        before.as_deref(),
        &(serde_json::to_string_pretty(&document)? + "\n"),
        0o600,
    )
}

fn owned_command(command: Option<&str>, legacy_args: bool, launcher: &Path) -> bool {
    let Some(command) = command.map(Path::new) else {
        return false;
    };
    command == launcher
        || (legacy_args
            && matches!(
                command.file_name().and_then(|name| name.to_str()),
                Some("Blackholes" | "blackholes-rust"),
            ))
}

fn install_skill(skills: &Path) -> Result<bool> {
    let path = skills.join("blackholes/SKILL.md");
    let before = read_file(&path)?;
    if before.as_deref().is_some_and(|text| !text.contains(MARKER)) {
        bail!("Your custom Blackholes skill was preserved. The MCP is configured independently");
    }
    write_if_changed(&path, before.as_deref(), SKILL, 0o600)
}

pub(crate) fn read_file(path: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            bail!("{} is not a regular file; it was preserved", path.display());
        }
        Ok(metadata) if metadata.len() > MAX_CONFIG_BYTES => {
            bail!(
                "{} exceeds the configuration size limit; it was preserved",
                path.display()
            );
        }
        Ok(_) => fs::read_to_string(path)
            .map(Some)
            .with_context(|| format!("Could not read {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Could not inspect {}", path.display())),
    }
}

pub(crate) fn write_if_changed(
    path: &Path,
    before: Option<&str>,
    after: &str,
    mode: u32,
) -> Result<bool> {
    if before == Some(after) {
        return Ok(false);
    }
    let directory = path.parent().context("Missing configuration directory")?;
    fs::create_dir_all(directory)?;
    let temporary = directory.join(format!(".blackholes-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)?;
        file.write_all(after.as_bytes())?;
        file.sync_all()?;
        if read_file(path)?.as_deref() != before {
            bail!(
                "{} changed during setup; retry after the other client finishes saving",
                path.display()
            );
        }
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.with_context(|| format!("Could not update {}", path.display()))?;
    Ok(true)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
