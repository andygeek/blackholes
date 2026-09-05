use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

pub const DEFAULT_PROJECT_ICON: &str = "layers";
pub const DEFAULT_TASK_ICON: &str = "list-todo";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Language {
    #[default]
    English,
    Spanish,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppTheme {
    Light,
    #[default]
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceLayout {
    Empty,
    SingleRepository,
    MultiRepository,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceColor {
    #[default]
    Slate,
    Coral,
    Peach,
    Amber,
    Sage,
    Mint,
    Sky,
    Lavender,
    Rose,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Repository {
    pub id: Uuid,
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Workspace {
    pub id: Uuid,
    pub name: String,
    pub display_name: Option<String>,
    pub icon: String,
    pub color: WorkspaceColor,
    pub root_path: Option<PathBuf>,
    pub layout: WorkspaceLayout,
    pub repositories: Vec<Repository>,
    pub ignored_repository_paths: Vec<PathBuf>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Workspace {
    pub fn label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }

    pub fn terminal_root(&self, repository_id: Option<Uuid>) -> Option<PathBuf> {
        repository_id
            .and_then(|id| self.repositories.iter().find(|repo| repo.id == id))
            .map(|repo| repo.path.clone())
            .or_else(|| self.root_path.clone())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    #[default]
    Shell,
    Codex,
    Claude,
    Gemini,
}

impl AgentKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Shell => "Terminal",
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Gemini => "Gemini",
        }
    }

    pub fn command(self) -> Option<(&'static str, Vec<String>)> {
        match self {
            Self::Shell => None,
            Self::Claude => Some((
                "claude",
                vec![
                    "--settings".into(),
                    r#"{"preferredNotifChannel":"terminal_bell"}"#.into(),
                ],
            )),
            Self::Codex => Some((
                "codex",
                vec![
                    "-c".into(),
                    "tui.notifications=true".into(),
                    "-c".into(),
                    "tui.notification_condition=\"always\"".into(),
                    "-c".into(),
                    "tui.notification_method=\"bel\"".into(),
                ],
            )),
            Self::Gemini => Some(("gemini", vec![])),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionState {
    Restored,
    #[default]
    Idle,
    Working,
    Attention,
    Exited,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodexProfile {
    #[default]
    Default,
    Work,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexSession {
    pub id: String,
    pub profile: CodexProfile,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClaudeProfile {
    #[default]
    Default,
    Work,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeSession {
    pub id: String,
    pub profile: ClaudeProfile,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskRepository {
    pub repository_id: Uuid,
    pub worktree_path: PathBuf,
    pub branch: String,
    pub base_branch: Option<String>,
    pub base_revision: String,
    pub copy_local_changes: bool,
    pub copy_environment_files: bool,
    pub copied_environment_files: Vec<String>,
    pub setup_command: Option<String>,
    pub prepared_at: Option<DateTime<Utc>>,
    pub added_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskSession {
    pub id: Uuid,
    pub repository_id: Option<Uuid>,
    pub terminal_local_id: Uuid,
    pub agent: AgentKind,
    pub label: String,
    pub state: SessionState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub exited_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectTask {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub icon: String,
    pub color: WorkspaceColor,
    pub sort_order: i64,
    pub worktree_root_path: PathBuf,
    pub repositories: Vec<TaskRepository>,
    pub sessions: Vec<TaskSession>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ProjectTask {
    pub fn repository_path(&self, repository_id: Option<Uuid>) -> Option<PathBuf> {
        repository_id
            .and_then(|id| {
                self.repositories
                    .iter()
                    .find(|repo| repo.repository_id == id)
            })
            .map(|repo| repo.worktree_path.clone())
            .or_else(|| Some(self.worktree_root_path.clone()))
    }

    pub fn branch(&self) -> Option<&str> {
        self.repositories.first().map(|repo| repo.branch.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DockNode {
    Panel {
        terminal_id: Uuid,
    },
    Split {
        axis: SplitAxis,
        ratio: f32,
        first: Box<DockNode>,
        second: Box<DockNode>,
    },
}

impl DockNode {
    pub fn terminal_ids(&self, output: &mut Vec<Uuid>) {
        match self {
            Self::Panel { terminal_id } => output.push(*terminal_id),
            Self::Split { first, second, .. } => {
                first.terminal_ids(output);
                second.terminal_ids(output);
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DockTab {
    pub id: Uuid,
    pub title: String,
    pub root: DockNode,
    pub active_terminal_id: Uuid,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DockState {
    pub tabs: Vec<DockTab>,
    pub active_tab_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TerminalDescriptor {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub task_id: Option<Uuid>,
    pub repository_id: Option<Uuid>,
    pub agent: AgentKind,
    pub label: String,
    pub cwd: PathBuf,
    pub state: SessionState,
    #[serde(default)]
    pub codex_session: Option<CodexSession>,
    #[serde(default)]
    pub claude_session: Option<ClaudeSession>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppSession {
    pub selected_workspace_id: Option<Uuid>,
    pub selected_task_id: Option<Uuid>,
    pub selected_repository_id: Option<Uuid>,
    pub expanded_workspace_ids: Vec<Uuid>,
    pub expanded_task_ids: Vec<Uuid>,
    #[serde(default)]
    pub navigation_expansion_initialized: bool,
    #[serde(default)]
    pub unseen_task_ids: Vec<Uuid>,
    pub terminals: Vec<TerminalDescriptor>,
    pub docks: std::collections::HashMap<String, DockState>,
    pub language: Language,
    #[serde(default)]
    pub theme: AppTheme,
    pub sidebar_width: f32,
}

impl Default for AppSession {
    fn default() -> Self {
        Self {
            selected_workspace_id: None,
            selected_task_id: None,
            selected_repository_id: None,
            expanded_workspace_ids: Vec::new(),
            expanded_task_ids: Vec::new(),
            navigation_expansion_initialized: false,
            unseen_task_ids: Vec::new(),
            terminals: Vec::new(),
            docks: Default::default(),
            language: Language::English,
            theme: AppTheme::Dark,
            sidebar_width: 260.0,
        }
    }
}

pub fn dock_key(workspace_id: Uuid, task_id: Option<Uuid>, repository_id: Option<Uuid>) -> String {
    format!(
        "{}:{}:{}",
        workspace_id,
        task_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "project".into()),
        repository_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "root".into())
    )
}
