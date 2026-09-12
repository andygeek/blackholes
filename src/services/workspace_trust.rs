//! Record workspace trust for isolated task workspaces.
//!
//! Every task gets its own directory, named after the task it belongs to, and
//! each agent keys workspace trust on the exact absolute path. A new task is
//! therefore always unknown, so the terminal opens by asking the developer to
//! confirm a directory Blackholes just created from their own repositories.
//! Granting the trust up front keeps a launched task unattended.
//!
//! Trust is granted only for paths inside the managed task workspaces, and it
//! is dropped again when a task or repository is removed, so a deleted task
//! leaves nothing behind in the developer's configuration.
//!
//! OpenCode gates tools through its own permission prompts and keeps no
//! directory trust list, so it needs nothing here.
use crate::services::external_integrations::{read_file, write_if_changed};
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use toml_edit::{DocumentMut, Item, Table, value};

/// Permissions for a trust store Blackholes has to create itself.
const NEW_CONFIG_MODE: u32 = 0o600;

/// How an agent records the directories it trusts.
#[derive(Clone, Copy)]
enum TrustStore {
    /// `.claude.json`: `projects."<path>".hasTrustDialogAccepted`.
    Claude,
    /// `config.toml`: `[projects."<path>"] trust_level = "trusted"`.
    Codex,
    /// `trustedFolders.json`: `{"<path>": "TRUST_FOLDER"}`.
    Gemini,
    /// `settings.json`: `trustedWorkspaces: ["<path>"]`.
    Antigravity,
}

struct TrustProfile {
    agent: &'static str,
    store: TrustStore,
    config: PathBuf,
}

/// Trust `workspaces` on behalf of every agent that keeps a trust list.
///
/// Never fails the caller: a task has to stay usable when a configuration file
/// is malformed or owned by another tool, and the developer can still answer
/// the prompt by hand.
pub fn grant(agent_profiles: &Path, workspaces: &[PathBuf]) {
    apply(agent_profiles, workspaces, true);
}

/// Forget `workspaces`, so removing a task also removes its path from every
/// trust store it was added to.
pub fn revoke(agent_profiles: &Path, workspaces: &[PathBuf]) {
    apply(agent_profiles, workspaces, false);
}

fn apply(agent_profiles: &Path, workspaces: &[PathBuf], trusted: bool) {
    let keys = trust_keys(workspaces);
    if keys.is_empty() {
        return;
    }
    for profile in profiles(agent_profiles) {
        let result = match profile.store {
            TrustStore::Claude => update_claude(&profile.config, &keys, trusted),
            TrustStore::Codex => update_codex(&profile.config, &keys, trusted),
            TrustStore::Gemini => update_gemini(&profile.config, &keys, trusted),
            TrustStore::Antigravity => update_antigravity(&profile.config, &keys, trusted),
        };
        match result {
            Ok(true) => tracing::debug!(
                agent = profile.agent,
                config = %profile.config.display(),
                trusted,
                "Updated workspace trust",
            ),
            Ok(false) => {}
            Err(error) => tracing::warn!(
                agent = profile.agent,
                config = %profile.config.display(),
                "Could not update workspace trust: {error:#}",
            ),
        }
    }
}

/// The paths an agent may match on. A workspace is recorded both as Blackholes
/// stores it and as the filesystem resolves it, because the two differ whenever
/// a parent directory is a symlink, and the agent only sees the directory it
/// was handed.
fn trust_keys(workspaces: &[PathBuf]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut keys = Vec::new();
    for workspace in workspaces {
        let resolved = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.clone());
        for candidate in [workspace, &resolved] {
            if !candidate.is_absolute() {
                continue;
            }
            if let Some(text) = candidate.to_str()
                && seen.insert(text.to_string())
            {
                keys.push(text.to_string());
            }
        }
    }
    keys
}

/// Every trust store on this computer, following the same profile layout as
/// session restoration and MCP setup.
fn profiles(agent_profiles: &Path) -> Vec<TrustProfile> {
    let Some(home) = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()) else {
        return Vec::new();
    };
    let mut profiles = vec![
        profile("Claude Code", TrustStore::Claude, home.join(".claude.json")),
        profile("Codex", TrustStore::Codex, home.join(".codex/config.toml")),
        profile(
            "Gemini",
            TrustStore::Gemini,
            home.join(".gemini/trustedFolders.json"),
        ),
        profile(
            "Antigravity",
            TrustStore::Antigravity,
            home.join(".gemini/antigravity-cli/settings.json"),
        ),
    ];
    for (variable, agent, store, file) in [
        (
            "CLAUDE_CONFIG_DIR",
            "Claude Code",
            TrustStore::Claude,
            ".claude.json",
        ),
        ("CODEX_HOME", "Codex", TrustStore::Codex, "config.toml"),
        (
            "GEMINI_CLI_HOME",
            "Gemini",
            TrustStore::Gemini,
            "trustedFolders.json",
        ),
    ] {
        if let Some(root) = env::var_os(variable)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            && root.is_absolute()
        {
            profiles.push(profile(agent, store, root.join(file)));
        }
    }
    // The secondary profiles already supported by session restoration.
    for (directory, agent, store, file) in [
        (
            ".claude-work",
            "Claude Code",
            TrustStore::Claude,
            ".claude.json",
        ),
        (".codex-work", "Codex", TrustStore::Codex, "config.toml"),
    ] {
        let root = home.join(directory);
        if root.is_dir() {
            profiles.push(profile(agent, store, root.join(file)));
        }
    }
    // Earlier installations pointed CLAUDE_CONFIG_DIR at ~/.claude.
    let legacy = home.join(".claude/.claude.json");
    if legacy.is_file() {
        profiles.push(profile("Claude Code", TrustStore::Claude, legacy));
    }
    // Profiles Blackholes itself isolates for a separate agent login.
    for (id, agent, store, file) in [
        ("claude", "Claude Code", TrustStore::Claude, ".claude.json"),
        ("codex", "Codex", TrustStore::Codex, "config.toml"),
        (
            "gemini",
            "Gemini",
            TrustStore::Gemini,
            "trustedFolders.json",
        ),
    ] {
        let root = agent_profiles.join(id);
        if root.is_dir() {
            profiles.push(profile(agent, store, root.join(file)));
        }
    }

    let mut seen = HashSet::new();
    profiles.retain(|profile| seen.insert(profile.config.clone()));
    // Only touch a store the developer already has. Creating a configuration
    // directory would invent a profile for an agent that was never installed.
    profiles.retain(|profile| {
        profile.config.is_file() || profile.config.parent().is_some_and(Path::is_dir)
    });
    profiles
}

fn profile(agent: &'static str, store: TrustStore, config: PathBuf) -> TrustProfile {
    TrustProfile {
        agent,
        store,
        config,
    }
}

fn update_claude(path: &Path, keys: &[String], trusted: bool) -> Result<bool> {
    let before = read_file(path)?;
    let mut document = parse_json(before.as_deref())?;
    let root = document
        .as_object_mut()
        .context("The Claude configuration must be an object")?;
    if trusted {
        let projects = root
            .entry("projects")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .context("The Claude projects field must be an object")?;
        for key in keys {
            let project = projects
                .entry(key.clone())
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("A Claude project entry must be an object")?;
            // Trust and the first-run project tour are separate prompts, and a
            // directory this new would raise both.
            project.insert("hasTrustDialogAccepted".into(), json!(true));
            project.insert("hasCompletedProjectOnboarding".into(), json!(true));
        }
    } else {
        let Some(projects) = root.get_mut("projects").and_then(Value::as_object_mut) else {
            return Ok(false);
        };
        for key in keys {
            projects.remove(key);
        }
    }
    write_json(path, before.as_deref(), &document)
}

fn update_codex(path: &Path, keys: &[String], trusted: bool) -> Result<bool> {
    let before = read_file(path)?;
    let mut document = before
        .as_deref()
        .unwrap_or("")
        .parse::<DocumentMut>()
        .map_err(|_| anyhow!("Invalid TOML; the existing configuration was preserved"))?;
    if trusted {
        if document.get("projects").is_none() {
            // Implicit, so the file keeps only the `[projects."<path>"]`
            // headers Codex writes itself.
            let mut table = Table::new();
            table.set_implicit(true);
            document["projects"] = Item::Table(table);
        }
        let projects = document["projects"]
            .as_table_like_mut()
            .context("The Codex projects field must be a table")?;
        for key in keys {
            if projects.get(key).is_none() {
                projects.insert(key, Item::Table(Table::new()));
            }
            let project = projects
                .get_mut(key)
                .and_then(Item::as_table_like_mut)
                .context("A Codex project entry must be a table")?;
            project.insert("trust_level", value("trusted"));
        }
    } else {
        let Some(projects) = document
            .get_mut("projects")
            .and_then(Item::as_table_like_mut)
        else {
            return Ok(false);
        };
        for key in keys {
            projects.remove(key);
        }
    }
    let after = document.to_string();
    write_if_changed(path, before.as_deref(), &after, config_mode(path))
}

fn update_gemini(path: &Path, keys: &[String], trusted: bool) -> Result<bool> {
    let before = read_file(path)?;
    let mut document = parse_json(before.as_deref())?;
    let root = document
        .as_object_mut()
        .context("The Gemini trusted folders file must be an object")?;
    for key in keys {
        if trusted {
            root.insert(key.clone(), json!("TRUST_FOLDER"));
        } else {
            root.remove(key);
        }
    }
    write_json(path, before.as_deref(), &document)
}

fn update_antigravity(path: &Path, keys: &[String], trusted: bool) -> Result<bool> {
    let before = read_file(path)?;
    let mut document = parse_json(before.as_deref())?;
    let root = document
        .as_object_mut()
        .context("The Antigravity settings must be an object")?;
    if trusted {
        let workspaces = root
            .entry("trustedWorkspaces")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .context("The Antigravity trusted workspaces must be an array")?;
        for key in keys {
            if !workspaces
                .iter()
                .any(|entry| entry.as_str() == Some(key.as_str()))
            {
                workspaces.push(json!(key));
            }
        }
    } else {
        let Some(workspaces) = root
            .get_mut("trustedWorkspaces")
            .and_then(Value::as_array_mut)
        else {
            return Ok(false);
        };
        workspaces.retain(|entry| match entry.as_str() {
            Some(text) => !keys.iter().any(|key| key == text),
            None => true,
        });
    }
    write_json(path, before.as_deref(), &document)
}

fn parse_json(before: Option<&str>) -> Result<Value> {
    let text = before
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("{}");
    serde_json::from_str(text)
        .map_err(|_| anyhow!("Invalid JSON; the existing configuration was preserved"))
}

fn write_json(path: &Path, before: Option<&str>, document: &Value) -> Result<bool> {
    // Skip a rewrite that would only renormalize spacing and key order. These
    // files belong to the agents, and one of them is the developer's whole
    // Claude history.
    if let Some(before) = before
        && serde_json::from_str::<Value>(before).ok().as_ref() == Some(document)
    {
        return Ok(false);
    }
    let after = serde_json::to_string_pretty(document)? + "\n";
    write_if_changed(path, before, &after, config_mode(path))
}

/// Keep the permissions an existing store already has; a new one starts private.
fn config_mode(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o777)
        .unwrap_or(NEW_CONFIG_MODE)
}
