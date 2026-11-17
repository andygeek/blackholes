use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

/// A copy of the app whose bundle identifier extends the released one, such as
/// `dev.blackholes.rust.preview`, is a separate instance named by that suffix.
/// It keeps its own data and leaves the updater and the agents' shared
/// configuration to the main installation, so both can run side by side.
const INSTANCE_BUNDLE_PREFIX: &str = "dev.blackholes.rust.";
/// Names the instance for what its terminals start: an agent's hooks may run
/// the main installation's executable.
pub const INSTANCE_ENVIRONMENT: &str = "BLACKHOLES_INSTANCE";

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub database: PathBuf,
    pub session: PathBuf,
    pub agent_profiles: PathBuf,
    pub task_workspaces: PathBuf,
    pub default_projects: PathBuf,
    pub events_socket: PathBuf,
    /// The name of a separate instance; `None` for the main installation.
    pub instance: Option<String>,
}

impl AppPaths {
    /// Hooks only send an event. They must not create/migrate app state or
    /// open a database while a provider is starting or submitting a prompt.
    pub fn events_socket_path() -> Result<PathBuf> {
        Ok(project_dirs(instance()?.as_deref())?.data_dir().join("blackholes-events.sock"))
    }

    pub fn discover() -> Result<Self> {
        let instance = instance()?;
        let data_dir = project_dirs(instance.as_deref())?.data_dir().to_path_buf();
        fs::create_dir_all(&data_dir)
            .with_context(|| format!("Unable to create {}", data_dir.display()))?;

        let task_workspaces = data_dir.join("task-workspaces");
        fs::create_dir_all(&task_workspaces)?;

        let agent_profiles = data_dir.join("agent-profiles");
        fs::create_dir_all(&agent_profiles)?;

        let default_projects = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir.clone())
            .join(match &instance {
                Some(name) => format!("Blackholes_projects-{name}"),
                None => "Blackholes_projects".to_owned(),
            });

        Ok(Self {
            database: data_dir.join("blackholes-local.db"),
            session: data_dir.join("app-session.json"),
            agent_profiles,
            task_workspaces,
            default_projects,
            events_socket: data_dir.join("blackholes-events.sock"),
            data_dir,
            instance,
        })
    }
}

fn project_dirs(instance: Option<&str>) -> Result<ProjectDirs> {
    let application = match instance {
        Some(name) => format!("Blackholes Rust {name}"),
        None => "Blackholes Rust".to_owned(),
    };
    ProjectDirs::from("dev", "blackholes", &application)
        .context("Unable to resolve the Application Support directory")
}

fn instance() -> Result<Option<String>> {
    instance_from(std::env::var_os(INSTANCE_ENVIRONMENT), bundle_identifier())
}

fn instance_from(environment: Option<OsString>, bundle_identifier: Option<String>) -> Result<Option<String>> {
    let name = match environment.filter(|name| !name.is_empty()) {
        Some(name) => name.to_string_lossy().into_owned(),
        None => match bundle_identifier.as_deref().and_then(|identifier| identifier.strip_prefix(INSTANCE_BUNDLE_PREFIX)) {
            Some(name) => name.to_owned(),
            None => return Ok(None),
        },
    };
    // The name becomes part of folder names. A wrong one must fail, never fall back to the main data.
    if name.is_empty() || name.len() > 32
        || !name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        bail!("Invalid Blackholes instance name {name:?}: use up to 32 lowercase letters, digits or hyphens");
    }
    Ok(Some(name))
}

#[cfg(target_os = "macos")]
fn bundle_identifier() -> Option<String> {
    objc2_foundation::NSBundle::mainBundle().bundleIdentifier().map(|identifier| identifier.to_string())
}

#[cfg(not(target_os = "macos"))]
fn bundle_identifier() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::instance_from;

    #[test]
    fn only_an_extended_bundle_identifier_or_the_environment_names_an_instance() {
        let bundle = |identifier: &str| instance_from(None, Some(identifier.into()));
        assert_eq!(instance_from(None, None).unwrap(), None);
        assert_eq!(bundle("dev.blackholes.rust").unwrap(), None);
        assert_eq!(bundle("com.example.fork").unwrap(), None);
        assert_eq!(bundle("dev.blackholes.rust.preview").unwrap().as_deref(), Some("preview"));
        assert_eq!(instance_from(Some("".into()), Some("dev.blackholes.rust".into())).unwrap(), None);
        // An instance's terminal runs the main installation's executable for the agents' hooks.
        assert_eq!(
            instance_from(Some("preview".into()), Some("dev.blackholes.rust".into())).unwrap().as_deref(),
            Some("preview"),
        );
        assert!(bundle("dev.blackholes.rust.").is_err());
        assert!(bundle("dev.blackholes.rust.a.b").is_err());
        assert!(instance_from(Some("../main".into()), None).is_err());
    }
}
