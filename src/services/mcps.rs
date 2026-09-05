use crate::{
    model::Workspace,
    paths::AppPaths,
    services::orchestrator::{AgentAuthMode, AgentProvider},
};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct AgentMcpServer {
    pub name: String,
    pub source: String,
    pub required: bool,
    pub managed: bool,
    pub config: Option<AgentMcpServerConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum AgentMcpServerConfig {
    Http {
        name: String,
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oauth_client_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oauth_callback_port: Option<u16>,
    },
    Stdio {
        name: String,
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
}

impl AgentMcpServerConfig {
    pub fn name(&self) -> &str {
        match self {
            Self::Http { name, .. } | Self::Stdio { name, .. } => name,
        }
    }

    pub fn transport_label(&self) -> &'static str {
        match self {
            Self::Http { .. } => "Remote HTTP",
            Self::Stdio { .. } => "Local command",
        }
    }
}

pub struct AgentMcpService;

impl AgentMcpService {
    pub fn supports_external_servers(provider: AgentProvider) -> bool {
        matches!(provider, AgentProvider::Claude | AgentProvider::Codex)
    }

    pub fn list(
        paths: &AppPaths,
        provider: AgentProvider,
        auth_mode: AgentAuthMode,
        _workspaces: &[Workspace],
        _workspace_id: Option<Uuid>,
    ) -> Vec<AgentMcpServer> {
        let mut servers = BTreeMap::new();
        servers.insert(
            "blackholes".to_string(),
            AgentMcpServer {
                name: "blackholes".to_string(),
                source: "Blackholes · built in".to_string(),
                required: true,
                managed: false,
                config: None,
            },
        );
        if !Self::supports_external_servers(provider) {
            return servers.into_values().collect();
        }

        for config in profile_config_files(paths, provider, auth_mode) {
            add_configured_servers(&mut servers, provider, &config, false);
        }

        servers.into_values().collect()
    }
}

fn profile_config_files(
    paths: &AppPaths,
    provider: AgentProvider,
    auth_mode: AgentAuthMode,
) -> Vec<PathBuf> {
    let isolated = paths.agent_profiles.join(provider.id());
    let home = BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
    match (provider, auth_mode) {
        (AgentProvider::Codex, AgentAuthMode::Isolated) => vec![isolated.join("config.toml")],
        (AgentProvider::Codex, AgentAuthMode::System) => vec![
            std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .or_else(|| home.map(|home| home.join(".codex")))
                .unwrap_or_default()
                .join("config.toml"),
        ],
        (AgentProvider::Claude, AgentAuthMode::Isolated) => vec![
            isolated.join(".claude.json"),
            isolated.join("settings.json"),
        ],
        (AgentProvider::Claude, AgentAuthMode::System) => {
            if let Some(config_dir) = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from) {
                vec![
                    config_dir.join(".claude.json"),
                    config_dir.join("settings.json"),
                ]
            } else if let Some(home) = home {
                vec![
                    home.join(".claude.json"),
                    home.join(".claude/settings.json"),
                ]
            } else {
                Vec::new()
            }
        }
        (AgentProvider::Gemini | AgentProvider::OpenCode, _) => Vec::new(),
    }
}

fn add_configured_servers(
    servers: &mut BTreeMap<String, AgentMcpServer>,
    provider: AgentProvider,
    path: &Path,
    project_config: bool,
) {
    let names = match provider {
        AgentProvider::Codex => codex_mcp_names(path),
        AgentProvider::Claude => json_mcp_names(path),
        AgentProvider::Gemini | AgentProvider::OpenCode => Vec::new(),
    };
    for name in names {
        if name == "blackholes" || servers.contains_key(&name) {
            continue;
        }
        servers.insert(
            name.clone(),
            AgentMcpServer {
                name,
                source: format!(
                    "{} · {} · {}",
                    provider.display_name(),
                    if project_config { "project" } else { "profile" },
                    path.display()
                ),
                required: false,
                managed: false,
                config: None,
            },
        );
    }
}

fn codex_mcp_names(path: &Path) -> Vec<String> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| {
            let section = line
                .trim()
                .strip_prefix("[mcp_servers.")?
                .strip_suffix(']')?;
            let name = section.split('.').next()?.trim().trim_matches(['\"', '\'']);
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect()
}

fn json_mcp_names(path: &Path) -> Vec<String> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&content) else {
        return Vec::new();
    };
    ["mcpServers", "mcp_servers", "mcp"]
        .into_iter()
        .find_map(|key| value.get(key).and_then(Value::as_object))
        .map(|servers| servers.keys().cloned().collect())
        .unwrap_or_default()
}
