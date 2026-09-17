use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub database: PathBuf,
    pub session: PathBuf,
    pub agent_profiles: PathBuf,
    pub task_workspaces: PathBuf,
    pub default_projects: PathBuf,
    pub events_socket: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self> {
        let project_dirs = ProjectDirs::from("dev", "blackholes", "Blackholes Rust")
            .context("Unable to resolve the macOS Application Support directory")?;
        let data_dir = project_dirs.data_dir().to_path_buf();
        fs::create_dir_all(&data_dir)
            .with_context(|| format!("Unable to create {}", data_dir.display()))?;

        let task_workspaces = data_dir.join("task-workspaces");
        fs::create_dir_all(&task_workspaces)?;

        let agent_profiles = data_dir.join("agent-profiles");
        fs::create_dir_all(&agent_profiles)?;

        let default_projects = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.clone())
            .join("Blackholes_projects");

        Ok(Self {
            database: data_dir.join("blackholes-local.db"),
            session: data_dir.join("app-session.json"),
            agent_profiles,
            task_workspaces,
            default_projects,
            events_socket: data_dir.join("blackholes-events.sock"),
            data_dir,
        })
    }
}
