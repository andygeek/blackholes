use crate::services::mcps::AgentMcpServerConfig;
use anyhow::{Context as _, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead as _, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
};
use uuid::Uuid;

static ACTIVE_AGENT_SIDECARS: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();

fn active_agent_sidecars() -> &'static Mutex<HashSet<u32>> {
    ACTIVE_AGENT_SIDECARS.get_or_init(|| Mutex::new(HashSet::new()))
}

struct ActiveAgentSidecar(u32);

impl ActiveAgentSidecar {
    fn register(process_id: u32) -> Self {
        active_agent_sidecars()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(process_id);
        Self(process_id)
    }
}

impl Drop for ActiveAgentSidecar {
    fn drop(&mut self) {
        active_agent_sidecars()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.0);
    }
}

/// Stop every provider sidecar and its process group during application shutdown.
/// Normal per-agent stops use the same process-group boundary but allow a
/// longer graceful-shutdown window.
pub fn terminate_all_agent_processes() {
    let process_ids = active_agent_sidecars()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .copied()
        .collect::<Vec<_>>();

    #[cfg(unix)]
    {
        for process_id in &process_ids {
            signal_sidecar_process_group(*process_id, libc::SIGTERM);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        for process_id in process_ids {
            if sidecar_process_group_exists(process_id) {
                signal_sidecar_process_group(process_id, libc::SIGKILL);
            }
        }
    }

    #[cfg(windows)]
    for process_id in process_ids {
        let _ = Command::new("taskkill")
            .args(["/PID", &process_id.to_string(), "/T", "/F"])
            .status();
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentProvider {
    #[default]
    Claude,
    Codex,
    Gemini,
    OpenCode,
}

impl AgentProvider {
    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::OpenCode => "opencode",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Gemini => "Gemini",
            Self::OpenCode => "OpenCode",
        }
    }

    pub fn model_brand_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "OpenAI",
            Self::Gemini => "Gemini",
            Self::OpenCode => "OpenCode",
        }
    }

    pub fn supports_model_selection(self) -> bool {
        true
    }

    /// Whether this provider can switch between Blackholes' standard and full-access modes.
    /// Keep this explicit so the composer never advertises a control that a future runtime
    /// cannot honor.
    pub fn supports_permission_mode(self) -> bool {
        match self {
            Self::Claude | Self::Codex | Self::Gemini | Self::OpenCode => true,
        }
    }

    pub fn from_setting(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("codex") => Self::Codex,
            Some("gemini") => Self::Gemini,
            Some("opencode") => Self::OpenCode,
            _ => Self::Claude,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentAuthMode {
    #[default]
    System,
    Isolated,
}

impl AgentAuthMode {
    pub fn id(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Isolated => "isolated",
        }
    }

    pub fn from_setting(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("isolated") => Self::Isolated,
            _ => Self::System,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentAvatarColor {
    #[serde(alias = "orange", alias = "coral", alias = "peach")]
    #[default]
    Mercury,
    #[serde(alias = "sage", alias = "mint", alias = "sky")]
    Earthy,
    #[serde(alias = "amber", alias = "lavender", alias = "rose")]
    Saturny,
}

impl AgentAvatarColor {
    pub fn id(self) -> &'static str {
        match self {
            Self::Mercury => "mercury",
            Self::Earthy => "earthy",
            Self::Saturny => "saturny",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Mercury => "Mercury",
            Self::Earthy => "Earthy",
            Self::Saturny => "Saturny",
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum OrchestratorChatScope {
    Global,
    GlobalAgent(Uuid),
    Project(Uuid),
    ProjectAgent { project_id: Uuid, agent_id: Uuid },
    Task(Uuid),
    TaskAgent { task_id: Uuid, agent_id: Uuid },
}

impl OrchestratorChatScope {
    pub fn is_global(self) -> bool {
        matches!(self, Self::Global | Self::GlobalAgent(_))
    }

    pub fn project_id(self) -> Option<Uuid> {
        match self {
            Self::Project(project_id) | Self::ProjectAgent { project_id, .. } => Some(project_id),
            _ => None,
        }
    }

    pub fn task_id(self) -> Option<Uuid> {
        match self {
            Self::Task(task_id) | Self::TaskAgent { task_id, .. } => Some(task_id),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OrchestratorScopeContext {
    pub kind: &'static str,
    pub name: String,
    pub agent_id: Option<Uuid>,
    pub global_agent_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub project_name: Option<String>,
    pub task_id: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrchestratorChatRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchestratorChatAttachment {
    pub id: Uuid,
    pub media_type: String,
    pub data: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentHistoryMessage {
    pub role: OrchestratorChatRole,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchestratorChatMessage {
    pub id: Uuid,
    pub role: OrchestratorChatRole,
    pub content: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub attachments: Vec<OrchestratorChatAttachment>,
    #[serde(default)]
    pub revision_group_id: Option<Uuid>,
    #[serde(default)]
    pub activities: Vec<OrchestratorChatActivity>,
    #[serde(default)]
    pub handoffs: Vec<OrchestratorChatHandoff>,
    #[serde(default)]
    pub interrupted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchestratorChatBranch {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub session_id: Option<String>,
    pub messages: Vec<OrchestratorChatMessage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchestratorBranchChoice {
    pub branch_id: Uuid,
    pub message_id: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchestratorBranchGroup {
    pub id: Uuid,
    pub choices: Vec<OrchestratorBranchChoice>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OrchestratorBranchNavigation {
    pub position: usize,
    pub total: usize,
    pub previous_branch_id: Option<Uuid>,
    pub next_branch_id: Option<Uuid>,
}

pub struct OrchestratorEditFork {
    pub source_session_id: Option<String>,
    pub user_turn_index: usize,
    pub revision_group_id: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchestratorChatActivity {
    pub agent: String,
    pub tool: String,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub background: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchestratorChatHandoff {
    pub scope: String,
    pub project_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub label: String,
    #[serde(default)]
    pub identity: AgentAvatarColor,
    #[serde(default)]
    pub navigation: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OrchestratorChatState {
    pub session_id: Option<String>,
    pub provider: AgentProvider,
    pub auth_mode: AgentAuthMode,
    /// Legacy runtime marker retained so existing chat files migrate without
    /// losing their native Claude session.
    pub profile: Option<String>,
    pub cwd: Option<PathBuf>,
    pub messages: Vec<OrchestratorChatMessage>,
    pub branch_id: Option<Uuid>,
    pub parent_branch_id: Option<Uuid>,
    pub branches: Vec<OrchestratorChatBranch>,
    pub branch_groups: Vec<OrchestratorBranchGroup>,
    pub avatar_color: AgentAvatarColor,
}

impl OrchestratorChatState {
    fn migrate_runtime_profile(&mut self) {
        if self.profile.as_deref() == Some("claude-work") {
            self.provider = AgentProvider::Claude;
            self.auth_mode = AgentAuthMode::Isolated;
            // The legacy profile lived in ~/.claude-work; the new account is
            // rooted under Blackholes Application Support, so its session id
            // cannot be resumed safely across those credential stores.
            self.session_id = None;
            self.profile = Some("claude:isolated".to_string());
        }
        self.ensure_branch_id();
    }

    pub fn prepare_runtime(&mut self, provider: AgentProvider, auth_mode: AgentAuthMode) {
        let runtime_profile = format!("{}:{}", provider.id(), auth_mode.id());
        if self.profile.as_deref() != Some(runtime_profile.as_str()) {
            self.session_id = None;
            self.profile = Some(runtime_profile);
        }
        self.provider = provider;
        self.auth_mode = auth_mode;
        self.ensure_branch_id();
    }

    fn ensure_branch_id(&mut self) -> Uuid {
        *self.branch_id.get_or_insert_with(Uuid::new_v4)
    }

    fn parent_of_branch(&self, branch_id: Uuid) -> Option<Uuid> {
        if self.branch_id == Some(branch_id) {
            return self.parent_branch_id;
        }
        self.branches
            .iter()
            .find(|branch| branch.id == branch_id)
            .and_then(|branch| branch.parent_id)
    }

    fn active_ancestry(&self) -> Vec<Uuid> {
        let Some(mut branch_id) = self.branch_id else {
            return Vec::new();
        };
        let mut ancestry = Vec::new();
        let mut visited = HashSet::new();
        while visited.insert(branch_id) {
            ancestry.push(branch_id);
            let Some(parent_id) = self.parent_of_branch(branch_id) else {
                break;
            };
            branch_id = parent_id;
        }
        ancestry
    }

    fn upsert_inactive_branch(&mut self, branch: OrchestratorChatBranch) {
        self.branches.retain(|candidate| candidate.id != branch.id);
        self.branches.push(branch);
    }

    pub fn prepare_edit(
        &mut self,
        message_id: Uuid,
        edited_message_id: Uuid,
    ) -> Option<OrchestratorEditFork> {
        let message_index = self.messages.iter().position(|message| {
            message.id == message_id && matches!(message.role, OrchestratorChatRole::User)
        })?;
        let user_turn_index = self.messages[..message_index]
            .iter()
            .filter(|message| matches!(message.role, OrchestratorChatRole::User))
            .count();
        let current_branch_id = self.ensure_branch_id();
        let original_message_id = self.messages[message_index].id;
        let revision_group_id = self.messages[message_index]
            .revision_group_id
            .unwrap_or_else(Uuid::new_v4);
        self.messages[message_index].revision_group_id = Some(revision_group_id);

        let group = if let Some(group) = self
            .branch_groups
            .iter_mut()
            .find(|group| group.id == revision_group_id)
        {
            group
        } else {
            self.branch_groups.push(OrchestratorBranchGroup {
                id: revision_group_id,
                choices: vec![OrchestratorBranchChoice {
                    branch_id: current_branch_id,
                    message_id: original_message_id,
                }],
            });
            self.branch_groups.last_mut()?
        };
        if !group
            .choices
            .iter()
            .any(|choice| choice.branch_id == current_branch_id)
        {
            group.choices.push(OrchestratorBranchChoice {
                branch_id: current_branch_id,
                message_id: original_message_id,
            });
        }

        let source_session_id = self.session_id.clone();
        let current_branch = OrchestratorChatBranch {
            id: current_branch_id,
            parent_id: self.parent_branch_id,
            session_id: self.session_id.clone(),
            messages: self.messages.clone(),
        };
        self.upsert_inactive_branch(current_branch);

        let edited_branch_id = Uuid::new_v4();
        self.branch_groups
            .iter_mut()
            .find(|group| group.id == revision_group_id)?
            .choices
            .push(OrchestratorBranchChoice {
                branch_id: edited_branch_id,
                message_id: edited_message_id,
            });
        self.messages.truncate(message_index);
        self.branch_id = Some(edited_branch_id);
        self.parent_branch_id = Some(current_branch_id);
        self.session_id = None;

        Some(OrchestratorEditFork {
            source_session_id,
            user_turn_index,
            revision_group_id,
        })
    }

    pub fn switch_branch(&mut self, target_branch_id: Uuid) -> bool {
        let current_branch_id = self.ensure_branch_id();
        if current_branch_id == target_branch_id {
            return false;
        }
        let Some(target_index) = self
            .branches
            .iter()
            .position(|branch| branch.id == target_branch_id)
        else {
            return false;
        };
        let target = self.branches.remove(target_index);
        self.upsert_inactive_branch(OrchestratorChatBranch {
            id: current_branch_id,
            parent_id: self.parent_branch_id,
            session_id: self.session_id.clone(),
            messages: self.messages.clone(),
        });
        self.branch_id = Some(target.id);
        self.parent_branch_id = target.parent_id;
        self.session_id = target.session_id;
        self.messages = target.messages;
        true
    }

    pub fn branch_navigation(
        &self,
        message: &OrchestratorChatMessage,
    ) -> Option<OrchestratorBranchNavigation> {
        let group_id = message.revision_group_id?;
        let group = self
            .branch_groups
            .iter()
            .find(|group| group.id == group_id)?;
        if group.choices.len() < 2 {
            return None;
        }
        let ancestry = self.active_ancestry();
        let selected = ancestry.iter().find_map(|branch_id| {
            group
                .choices
                .iter()
                .position(|choice| choice.branch_id == *branch_id)
        })?;
        Some(OrchestratorBranchNavigation {
            position: selected + 1,
            total: group.choices.len(),
            previous_branch_id: selected
                .checked_sub(1)
                .map(|index| group.choices[index].branch_id),
            next_branch_id: group
                .choices
                .get(selected + 1)
                .map(|choice| choice.branch_id),
        })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeTurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub web_search_requests: u64,
    pub cost_usd: f64,
    pub num_turns: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeUsageTotals {
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub web_search_requests: u64,
    pub cost_usd: f64,
    pub num_turns: u64,
}

impl ClaudeUsageTotals {
    fn add_turn(&mut self, usage: ClaudeTurnUsage) {
        self.requests = self.requests.saturating_add(1);
        self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(usage.output_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(usage.cache_read_input_tokens);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .saturating_add(usage.cache_creation_input_tokens);
        self.web_search_requests = self
            .web_search_requests
            .saturating_add(usage.web_search_requests);
        self.cost_usd += usage.cost_usd;
        self.num_turns = self.num_turns.saturating_add(usage.num_turns);
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeRateLimitWindow {
    pub utilization: Option<f64>,
    pub resets_at: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeModelRateLimit {
    pub display_name: String,
    pub utilization: Option<f64>,
    pub resets_at: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeExtraUsage {
    pub is_enabled: bool,
    pub monthly_limit: Option<f64>,
    pub used_credits: Option<f64>,
    pub utilization: Option<f64>,
    pub currency: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeRateLimits {
    pub five_hour: Option<ClaudeRateLimitWindow>,
    pub seven_day: Option<ClaudeRateLimitWindow>,
    pub seven_day_oauth_apps: Option<ClaudeRateLimitWindow>,
    pub seven_day_opus: Option<ClaudeRateLimitWindow>,
    pub seven_day_sonnet: Option<ClaudeRateLimitWindow>,
    pub model_scoped: Vec<ClaudeModelRateLimit>,
    pub extra_usage: Option<ClaudeExtraUsage>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudePlanUsage {
    pub subscription_type: Option<String>,
    pub rate_limits_available: bool,
    pub rate_limits: Option<ClaudeRateLimits>,
    pub windows: Vec<PlanUsageWindow>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlanUsageWindow {
    pub label: String,
    pub minutes: Option<u64>,
    pub utilization: Option<f64>,
    pub resets_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct OrchestratorChatStore {
    version: u8,
    global: OrchestratorChatState,
    global_enabled: bool,
    global_agents: HashMap<Uuid, OrchestratorChatState>,
    global_agent_order: Vec<Uuid>,
    projects: HashMap<Uuid, OrchestratorChatState>,
    project_agents: HashMap<Uuid, HashMap<Uuid, OrchestratorChatState>>,
    project_agent_order: HashMap<Uuid, Vec<Uuid>>,
    tasks: HashMap<Uuid, OrchestratorChatState>,
    task_agents: HashMap<Uuid, HashMap<Uuid, OrchestratorChatState>>,
    task_agent_order: HashMap<Uuid, Vec<Uuid>>,
    disabled_projects: HashSet<Uuid>,
    usage_totals: ClaudeUsageTotals,
    provider_usage_totals: HashMap<String, ClaudeUsageTotals>,
    latest_plan_usage: Option<ClaudePlanUsage>,
    usage_updated_at: Option<DateTime<Utc>>,
}

impl Default for OrchestratorChatStore {
    fn default() -> Self {
        Self {
            version: 8,
            global: OrchestratorChatState::default(),
            global_enabled: true,
            global_agents: HashMap::new(),
            global_agent_order: Vec::new(),
            projects: HashMap::new(),
            project_agents: HashMap::new(),
            project_agent_order: HashMap::new(),
            tasks: HashMap::new(),
            task_agents: HashMap::new(),
            task_agent_order: HashMap::new(),
            disabled_projects: HashSet::new(),
            usage_totals: ClaudeUsageTotals::default(),
            provider_usage_totals: HashMap::new(),
            latest_plan_usage: None,
            usage_updated_at: None,
        }
    }
}

impl OrchestratorChatStore {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(path)
            .with_context(|| format!("Unable to read {}", path.display()))?;
        let value: Value = serde_json::from_str(&content)
            .with_context(|| format!("Unable to parse {}", path.display()))?;
        let mut store = if value.get("global").is_some() || value.get("version").is_some() {
            serde_json::from_value(value)
                .with_context(|| format!("Unable to parse {}", path.display()))?
        } else {
            Self {
                global: serde_json::from_value(value)
                    .with_context(|| format!("Unable to migrate {}", path.display()))?,
                ..Self::default()
            }
        };
        store.version = 8;
        let known_global_agents = store.global_agents.keys().copied().collect::<HashSet<_>>();
        store
            .global_agent_order
            .retain(|id| known_global_agents.contains(id));
        let unordered_global_agents = store
            .global_agents
            .keys()
            .filter(|id| !store.global_agent_order.contains(id))
            .copied()
            .collect::<Vec<_>>();
        store.global_agent_order.extend(unordered_global_agents);
        normalize_scoped_agent_order(&store.project_agents, &mut store.project_agent_order);
        normalize_scoped_agent_order(&store.task_agents, &mut store.task_agent_order);
        store.for_each_chat_mut(OrchestratorChatState::migrate_runtime_profile);
        Ok(store)
    }

    pub fn chat(&self, scope: OrchestratorChatScope) -> Option<&OrchestratorChatState> {
        match scope {
            OrchestratorChatScope::Global => Some(&self.global),
            OrchestratorChatScope::GlobalAgent(agent_id) => self.global_agents.get(&agent_id),
            OrchestratorChatScope::Project(workspace_id) => self.projects.get(&workspace_id),
            OrchestratorChatScope::ProjectAgent {
                project_id,
                agent_id,
            } => self.project_agents.get(&project_id)?.get(&agent_id),
            OrchestratorChatScope::Task(task_id) => self.tasks.get(&task_id),
            OrchestratorChatScope::TaskAgent { task_id, agent_id } => {
                self.task_agents.get(&task_id)?.get(&agent_id)
            }
        }
    }

    pub fn chat_mut(&mut self, scope: OrchestratorChatScope) -> &mut OrchestratorChatState {
        let chat = match scope {
            OrchestratorChatScope::Global => &mut self.global,
            OrchestratorChatScope::GlobalAgent(agent_id) => {
                self.global_agents.entry(agent_id).or_default()
            }
            OrchestratorChatScope::Project(workspace_id) => {
                self.projects.entry(workspace_id).or_default()
            }
            OrchestratorChatScope::ProjectAgent {
                project_id,
                agent_id,
            } => self
                .project_agents
                .entry(project_id)
                .or_default()
                .entry(agent_id)
                .or_default(),
            OrchestratorChatScope::Task(task_id) => self.tasks.entry(task_id).or_default(),
            OrchestratorChatScope::TaskAgent { task_id, agent_id } => self
                .task_agents
                .entry(task_id)
                .or_default()
                .entry(agent_id)
                .or_default(),
        };
        chat.migrate_runtime_profile();
        chat
    }

    pub fn reset(&mut self, scope: OrchestratorChatScope) {
        let avatar_color = self
            .chat(scope)
            .map(|chat| chat.avatar_color)
            .unwrap_or_default();
        *self.chat_mut(scope) = OrchestratorChatState {
            avatar_color,
            ..OrchestratorChatState::default()
        };
    }

    pub fn avatar_color(&self, scope: OrchestratorChatScope) -> AgentAvatarColor {
        self.chat(scope)
            .map(|chat| chat.avatar_color)
            .unwrap_or_default()
    }

    pub fn set_avatar_color(&mut self, scope: OrchestratorChatScope, color: AgentAvatarColor) {
        self.chat_mut(scope).avatar_color = color;
    }

    pub fn has_agent(&self, scope: OrchestratorChatScope) -> bool {
        match scope {
            OrchestratorChatScope::Global => self.global_enabled,
            OrchestratorChatScope::GlobalAgent(agent_id) => {
                self.global_agents.contains_key(&agent_id)
            }
            OrchestratorChatScope::Project(workspace_id) => {
                self.projects.contains_key(&workspace_id)
                    && !self.disabled_projects.contains(&workspace_id)
            }
            OrchestratorChatScope::ProjectAgent {
                project_id,
                agent_id,
            } => self
                .project_agents
                .get(&project_id)
                .is_some_and(|agents| agents.contains_key(&agent_id)),
            OrchestratorChatScope::Task(task_id) => self.tasks.contains_key(&task_id),
            OrchestratorChatScope::TaskAgent { task_id, agent_id } => self
                .task_agents
                .get(&task_id)
                .is_some_and(|agents| agents.contains_key(&agent_id)),
        }
    }

    pub fn assign_agent(&mut self, scope: OrchestratorChatScope) {
        match scope {
            OrchestratorChatScope::Global => self.global_enabled = true,
            OrchestratorChatScope::GlobalAgent(agent_id) => {
                if !self.global_agent_order.contains(&agent_id) {
                    self.global_agent_order.push(agent_id);
                }
            }
            OrchestratorChatScope::Project(workspace_id) => {
                self.disabled_projects.remove(&workspace_id);
            }
            OrchestratorChatScope::ProjectAgent {
                project_id,
                agent_id,
            } => {
                let order = self.project_agent_order.entry(project_id).or_default();
                if !order.contains(&agent_id) {
                    order.push(agent_id);
                }
            }
            OrchestratorChatScope::Task(_) => {}
            OrchestratorChatScope::TaskAgent { task_id, agent_id } => {
                let order = self.task_agent_order.entry(task_id).or_default();
                if !order.contains(&agent_id) {
                    order.push(agent_id);
                }
            }
        }
        let _ = self.chat_mut(scope);
    }

    pub fn remove_agent(&mut self, scope: OrchestratorChatScope) {
        match scope {
            OrchestratorChatScope::Global => {
                self.global_enabled = false;
                self.global = OrchestratorChatState::default();
            }
            OrchestratorChatScope::GlobalAgent(agent_id) => {
                self.global_agents.remove(&agent_id);
                self.global_agent_order.retain(|id| *id != agent_id);
            }
            OrchestratorChatScope::Project(workspace_id) => {
                self.disabled_projects.insert(workspace_id);
                self.projects.remove(&workspace_id);
            }
            OrchestratorChatScope::ProjectAgent {
                project_id,
                agent_id,
            } => {
                if let Some(agents) = self.project_agents.get_mut(&project_id) {
                    agents.remove(&agent_id);
                    if agents.is_empty() {
                        self.project_agents.remove(&project_id);
                    }
                }
                if let Some(order) = self.project_agent_order.get_mut(&project_id) {
                    order.retain(|id| *id != agent_id);
                    if order.is_empty() {
                        self.project_agent_order.remove(&project_id);
                    }
                }
            }
            OrchestratorChatScope::Task(task_id) => {
                self.tasks.remove(&task_id);
            }
            OrchestratorChatScope::TaskAgent { task_id, agent_id } => {
                if let Some(agents) = self.task_agents.get_mut(&task_id) {
                    agents.remove(&agent_id);
                    if agents.is_empty() {
                        self.task_agents.remove(&task_id);
                    }
                }
                if let Some(order) = self.task_agent_order.get_mut(&task_id) {
                    order.retain(|id| *id != agent_id);
                    if order.is_empty() {
                        self.task_agent_order.remove(&task_id);
                    }
                }
            }
        }
    }

    pub fn remove_project(&mut self, workspace_id: Uuid) {
        self.projects.remove(&workspace_id);
        self.project_agents.remove(&workspace_id);
        self.project_agent_order.remove(&workspace_id);
        self.disabled_projects.remove(&workspace_id);
    }

    pub fn remove_task(&mut self, task_id: Uuid) {
        self.tasks.remove(&task_id);
        self.task_agents.remove(&task_id);
        self.task_agent_order.remove(&task_id);
    }

    pub fn create_global_agent(&mut self) -> Uuid {
        let agent_id = Uuid::new_v4();
        let avatar_color = match self.global_agent_order.len() % 3 {
            0 => AgentAvatarColor::Earthy,
            1 => AgentAvatarColor::Saturny,
            _ => AgentAvatarColor::Mercury,
        };
        self.global_agent_order.push(agent_id);
        self.global_agents.insert(
            agent_id,
            OrchestratorChatState {
                avatar_color,
                ..OrchestratorChatState::default()
            },
        );
        agent_id
    }

    pub fn global_agent_ids(&self) -> &[Uuid] {
        &self.global_agent_order
    }

    pub fn has_global_agents(&self) -> bool {
        self.global_enabled || !self.global_agent_order.is_empty()
    }

    pub fn restore_default_global_agent(&mut self) {
        self.global = OrchestratorChatState {
            avatar_color: AgentAvatarColor::Earthy,
            ..OrchestratorChatState::default()
        };
        self.global_enabled = true;
    }

    pub fn create_project_agent(&mut self, project_id: Uuid) -> Uuid {
        create_scoped_agent(
            &mut self.project_agents,
            &mut self.project_agent_order,
            project_id,
        )
    }

    pub fn create_task_agent(&mut self, task_id: Uuid) -> Uuid {
        create_scoped_agent(&mut self.task_agents, &mut self.task_agent_order, task_id)
    }

    pub fn project_agent_ids(&self, project_id: Uuid) -> &[Uuid] {
        self.project_agent_order
            .get(&project_id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn task_agent_ids(&self, task_id: Uuid) -> &[Uuid] {
        self.task_agent_order
            .get(&task_id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn usage_totals(&self) -> &ClaudeUsageTotals {
        &self.usage_totals
    }

    pub fn provider_usage_totals(&self, provider: AgentProvider) -> ClaudeUsageTotals {
        self.provider_usage_totals.get(provider.id()).cloned().unwrap_or_else(|| {
            if provider == AgentProvider::Claude { self.usage_totals.clone() }
            else { ClaudeUsageTotals::default() }
        })
    }

    pub fn record_provider_usage(&mut self, provider: AgentProvider, usage: Option<ClaudeTurnUsage>) {
        if let Some(usage) = usage {
            let mut totals = self.provider_usage_totals(provider);
            totals.add_turn(usage);
            self.provider_usage_totals.insert(provider.id().into(), totals);
            self.usage_updated_at = Some(Utc::now());
        }
    }

    pub fn latest_plan_usage(&self) -> Option<&ClaudePlanUsage> {
        self.latest_plan_usage.as_ref()
    }

    pub fn usage_updated_at(&self) -> Option<DateTime<Utc>> {
        self.usage_updated_at
    }

    pub fn record_usage(
        &mut self,
        turn_usage: Option<ClaudeTurnUsage>,
        plan_usage: Option<ClaudePlanUsage>,
    ) {
        let changed = turn_usage.is_some() || plan_usage.is_some();
        if let Some(turn_usage) = turn_usage {
            self.usage_totals.add_turn(turn_usage);
        }
        if let Some(plan_usage) = plan_usage {
            self.latest_plan_usage = Some(plan_usage);
        }
        if changed {
            self.usage_updated_at = Some(Utc::now());
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Unable to create {}", parent.display()))?;
        }
        let temporary = path.with_extension("json.tmp");
        let content = serde_json::to_vec_pretty(self)?;
        fs::write(&temporary, content)
            .with_context(|| format!("Unable to write {}", temporary.display()))?;
        fs::rename(&temporary, path)
            .with_context(|| format!("Unable to replace {}", path.display()))?;
        Ok(())
    }

    fn for_each_chat_mut(&mut self, mut callback: impl FnMut(&mut OrchestratorChatState)) {
        callback(&mut self.global);
        self.global_agents.values_mut().for_each(&mut callback);
        self.projects.values_mut().for_each(&mut callback);
        self.project_agents
            .values_mut()
            .flat_map(HashMap::values_mut)
            .for_each(&mut callback);
        self.tasks.values_mut().for_each(&mut callback);
        self.task_agents
            .values_mut()
            .flat_map(HashMap::values_mut)
            .for_each(callback);
    }
}

fn normalize_scoped_agent_order(
    agents_by_scope: &HashMap<Uuid, HashMap<Uuid, OrchestratorChatState>>,
    order_by_scope: &mut HashMap<Uuid, Vec<Uuid>>,
) {
    order_by_scope.retain(|scope_id, _| agents_by_scope.contains_key(scope_id));
    for (scope_id, agents) in agents_by_scope {
        let order = order_by_scope.entry(*scope_id).or_default();
        order.retain(|agent_id| agents.contains_key(agent_id));
        let unordered = agents
            .keys()
            .filter(|id| !order.contains(id))
            .copied()
            .collect::<Vec<_>>();
        order.extend(unordered);
    }
}

fn create_scoped_agent(
    agents_by_scope: &mut HashMap<Uuid, HashMap<Uuid, OrchestratorChatState>>,
    order_by_scope: &mut HashMap<Uuid, Vec<Uuid>>,
    scope_id: Uuid,
) -> Uuid {
    let agent_id = Uuid::new_v4();
    let order = order_by_scope.entry(scope_id).or_default();
    let avatar_color = match order.len() % 3 {
        0 => AgentAvatarColor::Earthy,
        1 => AgentAvatarColor::Saturny,
        _ => AgentAvatarColor::Mercury,
    };
    order.push(agent_id);
    agents_by_scope.entry(scope_id).or_default().insert(
        agent_id,
        OrchestratorChatState {
            avatar_color,
            ..OrchestratorChatState::default()
        },
    );
    agent_id
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentRuntimeEvent {
    Started {
        #[allow(dead_code)]
        request_id: Option<String>,
    },
    Session {
        session_id: String,
    },
    Delta {
        text: String,
        #[serde(default)]
        prompt_id: Option<Uuid>,
    },
    Tool {
        name: String,
        #[serde(default)]
        agent: Option<String>,
        input: Option<Value>,
        #[serde(default)]
        prompt_id: Option<Uuid>,
    },
    BackgroundTask {
        task_id: String,
        status: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        task_type: String,
        #[serde(default)]
        summary: String,
        #[serde(default)]
        output_file: String,
        #[serde(default)]
        ambient: bool,
        #[serde(default)]
        prompt_id: Option<Uuid>,
    },
    Diagnostic {
        message: String,
    },
    Done {
        session_id: Option<String>,
        result: Option<String>,
        error: Option<String>,
        #[serde(default)]
        is_error: bool,
        turn_usage: Option<ClaudeTurnUsage>,
        plan_usage: Option<ClaudePlanUsage>,
        #[serde(default)]
        prompt_id: Option<Uuid>,
    },
    Error {
        message: String,
    },
}

#[derive(Serialize)]
struct AgentRuntimeRequest {
    request_id: String,
    provider: AgentProvider,
    auth_mode: AgentAuthMode,
    auth_profile_dir: PathBuf,
    agent_name: String,
    prompt_id: Uuid,
    message: String,
    history: Vec<AgentHistoryMessage>,
    images: Vec<OrchestratorChatAttachment>,
    session_id: Option<String>,
    fork_at_user_turn: Option<usize>,
    model: Option<String>,
    effort: Option<String>,
    full_access: bool,
    skills: Vec<String>,
    available_mcp_servers: Vec<String>,
    enabled_mcp_servers: Vec<String>,
    configured_mcp_servers: Vec<AgentMcpServerConfig>,
    skills_plugin_path: PathBuf,
    cwd: PathBuf,
    additional_directories: Vec<PathBuf>,
    blackholes_mcp_command: PathBuf,
    language: &'static str,
    scope: OrchestratorScopeContext,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentRuntimeControl {
    Steer {
        prompt_id: Uuid,
        message: String,
        images: Vec<OrchestratorChatAttachment>,
    },
    Interrupt,
}

pub struct AgentTurnStream {
    pub events: flume::Receiver<AgentRuntimeEvent>,
    pub cancel: flume::Sender<()>,
    pub control: flume::Sender<AgentRuntimeControl>,
}

pub fn stream_agent_turn(
    provider: AgentProvider,
    auth_mode: AgentAuthMode,
    auth_profile_dir: PathBuf,
    agent_name: String,
    prompt_id: Uuid,
    message: String,
    history: Vec<AgentHistoryMessage>,
    images: Vec<OrchestratorChatAttachment>,
    session_id: Option<String>,
    fork_at_user_turn: Option<usize>,
    model: Option<String>,
    effort: Option<String>,
    full_access: bool,
    skills: Vec<String>,
    available_mcp_servers: Vec<String>,
    enabled_mcp_servers: Vec<String>,
    configured_mcp_servers: Vec<AgentMcpServerConfig>,
    skills_plugin_path: PathBuf,
    cwd: PathBuf,
    additional_directories: Vec<PathBuf>,
    language: &'static str,
    scope: OrchestratorScopeContext,
) -> Result<AgentTurnStream> {
    let script = locate_sidecar_script()?;
    let node = locate_node_binary();
    let request = AgentRuntimeRequest {
        request_id: Uuid::new_v4().to_string(),
        provider,
        auth_mode,
        auth_profile_dir,
        agent_name,
        prompt_id,
        message,
        history,
        images,
        session_id,
        fork_at_user_turn,
        model,
        effort,
        full_access,
        skills,
        available_mcp_servers,
        enabled_mcp_servers,
        configured_mcp_servers,
        skills_plugin_path,
        cwd,
        additional_directories,
        blackholes_mcp_command: std::env::current_exe()
            .context("Unable to locate the Blackholes executable for its MCP server")?,
        language,
        scope,
    };
    let payload = serde_json::to_vec(&request)?;
    let (sender, receiver) = flume::unbounded();
    let (cancel, cancel_receiver) = flume::bounded(1);
    let (control, control_receiver) = flume::unbounded();

    std::thread::Builder::new()
        .name(format!("blackholes-{}-agent", provider.id()))
        .spawn(move || {
            let result = run_sidecar_process(
                &node,
                &script,
                &payload,
                &sender,
                cancel_receiver,
                control_receiver,
            );
            if let Err(error) = result {
                let _ = sender.send(AgentRuntimeEvent::Error {
                    message: format!("No se pudo ejecutar {}: {error:#}", provider.display_name()),
                });
            }
        })?;

    Ok(AgentTurnStream {
        events: receiver,
        cancel,
        control,
    })
}

fn run_sidecar_process(
    node: &Path,
    script: &Path,
    payload: &[u8],
    sender: &flume::Sender<AgentRuntimeEvent>,
    cancel: flume::Receiver<()>,
    control: flume::Receiver<AgentRuntimeControl>,
) -> Result<()> {
    let mut command = Command::new(node);
    command
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("Unable to start {}", node.display()))?;

    let process_id = child.id();
    let _active_sidecar = ActiveAgentSidecar::register(process_id);
    std::thread::Builder::new()
        .name("blackholes-agent-cancel".into())
        .spawn(move || {
            if cancel.recv().is_ok() {
                terminate_sidecar_process(process_id);
            }
        })
        .context("Unable to start the agent cancellation watcher")?;

    let mut stdin = child
        .stdin
        .take()
        .context("Agent sidecar stdin is unavailable")?;
    stdin.write_all(payload)?;
    stdin.write_all(b"\n")?;
    stdin.flush()?;
    std::thread::Builder::new()
        .name("blackholes-agent-control".into())
        .spawn(move || {
            for command in control {
                if serde_json::to_writer(&mut stdin, &command).is_err()
                    || stdin.write_all(b"\n").is_err()
                    || stdin.flush().is_err()
                {
                    break;
                }
            }
        })
        .context("Unable to start the agent control writer")?;

    let stdout = child
        .stdout
        .take()
        .context("Agent sidecar stdout is unavailable")?;
    let mut reached_terminal_event = false;
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<AgentRuntimeEvent>(&line) {
            Ok(event) => {
                reached_terminal_event |= matches!(
                    event,
                    AgentRuntimeEvent::Done { .. } | AgentRuntimeEvent::Error { .. }
                );
                if sender.send(event).is_err() {
                    let _ = child.kill();
                    return Ok(());
                }
            }
            Err(error) => {
                let _ = sender.send(AgentRuntimeEvent::Diagnostic {
                    message: format!("Salida no reconocida del SDK: {error}"),
                });
            }
        }
    }

    let mut stderr = String::new();
    if let Some(mut reader) = child.stderr.take() {
        let _ = reader.read_to_string(&mut stderr);
    }
    let status = child.wait()?;
    if !status.success() && !reached_terminal_event {
        return Err(anyhow!(
            "the sidecar exited with {status}: {}",
            stderr.trim()
        ));
    }
    if !reached_terminal_event {
        return Err(anyhow!("the sidecar ended without a result"));
    }
    Ok(())
}

#[cfg(unix)]
fn terminate_sidecar_process(process_id: u32) {
    // The sidecar is its own process-group leader, so this also stops the
    // provider runtime, subagents, and shell commands spawned for this turn.
    signal_sidecar_process_group(process_id, libc::SIGTERM);
    std::thread::sleep(std::time::Duration::from_millis(1_200));
    if sidecar_process_group_exists(process_id) {
        signal_sidecar_process_group(process_id, libc::SIGKILL);
    }
}

#[cfg(unix)]
fn signal_sidecar_process_group(process_id: u32, signal: i32) {
    let Ok(process_group) = i32::try_from(process_id).map(|id| -id) else {
        return;
    };
    unsafe {
        libc::kill(process_group, signal);
    }
}

#[cfg(unix)]
fn sidecar_process_group_exists(process_id: u32) -> bool {
    let Ok(process_group) = i32::try_from(process_id).map(|id| -id) else {
        return false;
    };
    (unsafe { libc::kill(process_group, 0) }) == 0
}

#[cfg(windows)]
fn terminate_sidecar_process(process_id: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &process_id.to_string(), "/T", "/F"])
        .status();
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct AgentModelCatalog {
    pub models: Vec<AgentModelInfo>,
    pub default_model: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AgentModelInfo {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub efforts: Vec<String>,
}

/// Metadata discovery uses the same bundled runtime and auth environment as turns.
/// Bounded stdout is drained concurrently so a large model catalog cannot deadlock.
pub fn refresh_agent_models(provider: AgentProvider, auth_mode: AgentAuthMode, profile: PathBuf, cwd: PathBuf,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Result<AgentModelCatalog> {
    if cancel.load(std::sync::atomic::Ordering::Relaxed) { bail!("Model discovery cancelled"); }
    let script = locate_sidecar_script()?.with_file_name("models.mjs");
    let mut command = Command::new(locate_node_binary());
    command.arg(script).current_dir(&cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command.spawn().context("Unable to start model discovery")?;
    let process_id = child.id();
    let _active_sidecar = ActiveAgentSidecar::register(process_id);
    let stdout = child.stdout.take().context("Model discovery stdout unavailable")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(1024 * 1024 + 1).read_to_end(&mut bytes).map(|_| bytes)
    });
    let payload = serde_json::to_vec(&serde_json::json!({
        "provider": provider.id(), "auth_mode": auth_mode.id(), "auth_profile_dir": profile, "cwd": cwd,
    }))?;
    let write_result = child.stdin.take().context("Model discovery stdin unavailable")
        .and_then(|mut stdin| stdin.write_all(&payload).map_err(Into::into));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
    let mut success = false;
    if write_result.is_ok() {
        loop {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) { break; }
            match child.try_wait() {
                Ok(Some(status)) => { success = status.success(); break; }
                Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(50)),
                _ => break,
            }
        }
    }
    // Reap the whole process group, including any provider helpers still holding pipes.
    terminate_sidecar_process(process_id);
    let _ = child.wait();
    let bytes = reader.join().map_err(|_| anyhow!("Model discovery reader failed"))??;
    if !success || bytes.len() > 1024 * 1024 { bail!("Model catalog unavailable"); }
    let catalog: AgentModelCatalog = serde_json::from_slice(&bytes).context("Invalid model catalog")?;
    if catalog.models.len() > 2000 { bail!("Model catalog is too large"); }
    Ok(catalog)
}

/// Fetch plan metadata without a conversation, prompt, tools, or token usage.
/// Runs on a background executor; the deadline also covers SDK startup/cleanup.
pub fn refresh_agent_plan_usage(provider: AgentProvider, auth_mode: AgentAuthMode, profile: PathBuf) -> Result<ClaudePlanUsage> {
    let script = locate_sidecar_script()?.with_file_name("usage.mjs");
    let mut command = Command::new(locate_node_binary());
    command.arg(script).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command.spawn().context("Unable to start usage query")?;
    let process_id = child.id();
    let _active_sidecar = ActiveAgentSidecar::register(process_id);
    let payload = serde_json::to_vec(&serde_json::json!({
        "provider": provider.id(), "auth_mode": auth_mode.id(), "auth_profile_dir": profile,
    }))?;
    let write_result = child.stdin.take().context("Usage stdin unavailable")
        .and_then(|mut stdin| stdin.write_all(&payload).map_err(Into::into));
    if let Err(error) = write_result {
        terminate_sidecar_process(process_id);
        let _ = child.wait();
        return Err(error);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => {
                terminate_sidecar_process(process_id);
                let _ = child.wait();
                return Err(anyhow!("Usage query timed out or exited unexpectedly"));
            }
        }
    };
    if !status.success() { return Err(anyhow!("Usage query unavailable")); }
    let mut output = String::new();
    child.stdout.take().context("Usage stdout unavailable")?.take(65536).read_to_string(&mut output)?;
    serde_json::from_str(&output).context("Invalid usage response")
}

fn locate_sidecar_script() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("BLACKHOLES_AGENT_SIDECAR")
        .or_else(|| std::env::var_os("BLACKHOLES_CLAUDE_SIDECAR"))
    {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(anyhow!("{} does not exist", path.display()));
    }

    if let Ok(executable) = std::env::current_exe()
        && let Some(contents) = executable.parent().and_then(Path::parent)
    {
        let bundled = contents.join("Resources/agent-sidecar/index.mjs");
        if bundled.is_file() {
            return Ok(bundled);
        }
    }

    let development = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("agent-sidecar")
        .join("index.mjs");
    if development.is_file() {
        return Ok(development);
    }

    Err(anyhow!(
        "Agent runtime sidecar was not found; set BLACKHOLES_AGENT_SIDECAR"
    ))
}

fn locate_node_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("BLACKHOLES_NODE_BINARY") {
        return PathBuf::from(path);
    }

    if let Ok(executable) = std::env::current_exe() {
        if let Some(contents) = executable.parent().and_then(Path::parent) {
            let bundled = contents.join("Resources/node/bin/node");
            if bundled.is_file() {
                return bundled;
            }
        }
    }

    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            let candidate = directory.join("node");
            if candidate.is_file() {
                return candidate;
            }
        }
    }

    for candidate in ["/opt/homebrew/bin/node", "/usr/local/bin/node"] {
        let candidate = PathBuf::from(candidate);
        if candidate.is_file() {
            return candidate;
        }
    }

    if let Some(user_home) = std::env::var_os("HOME").map(PathBuf::from) {
        for candidate in [
            user_home.join(".volta/bin/node"),
            user_home.join(".asdf/shims/node"),
            user_home.join(".local/share/fnm/aliases/default/bin/node"),
        ] {
            if candidate.is_file() {
                return candidate;
            }
        }

        let nvm_versions = user_home.join(".nvm/versions/node");
        if let Ok(entries) = fs::read_dir(nvm_versions) {
            let mut candidates = entries
                .flatten()
                .map(|entry| entry.path().join("bin/node"))
                .filter(|path| path.is_file())
                .collect::<Vec<_>>();
            candidates.sort();
            if let Some(candidate) = candidates.pop() {
                return candidate;
            }
        }
    }

    PathBuf::from("node")
}

#[derive(Debug)]
pub enum AgentAuthEvent {
    Output { text: String },
    OpenUrl { url: String },
    Completed,
    Error { message: String },
}

pub struct AgentAuthStream {
    pub events: flume::Receiver<AgentAuthEvent>,
    pub input: flume::Sender<String>,
    pub cancel: flume::Sender<()>,
}

pub fn start_agent_authentication(
    provider: AgentProvider,
    profiles_root: &Path,
) -> Result<AgentAuthStream> {
    let profile_dir = profiles_root.join(provider.id());
    fs::create_dir_all(&profile_dir)
        .with_context(|| format!("Unable to create {}", profile_dir.display()))?;
    if provider == AgentProvider::Gemini {
        prepare_gemini_oauth_profile(&profile_dir)?;
    }

    let (program, args) = authentication_command(provider)?;
    let mut command = if authentication_binary_needs_node(&program) {
        let mut command = Command::new(locate_node_binary());
        command.arg(&program);
        command
    } else {
        Command::new(&program)
    };
    command
        .args(&args)
        .current_dir(&profile_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match provider {
        AgentProvider::Claude => {
            command.env("CLAUDE_CONFIG_DIR", &profile_dir);
        }
        AgentProvider::Codex => {
            command.env("CODEX_HOME", &profile_dir);
        }
        AgentProvider::Gemini => {
            command
                .env("GEMINI_CLI_HOME", &profile_dir)
                .env("GEMINI_DEFAULT_AUTH_TYPE", "oauth-personal");
        }
        AgentProvider::OpenCode => {
            command
                .env("XDG_DATA_HOME", profile_dir.join("data"))
                .env("XDG_CONFIG_HOME", profile_dir.join("config"))
                .env("XDG_CACHE_HOME", profile_dir.join("cache"));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("Unable to start {} authentication", provider.display_name()))?;
    let process_id = child.id();
    let active_process = ActiveAgentSidecar::register(process_id);
    let stdin = child
        .stdin
        .take()
        .context("Authentication stdin is unavailable")?;
    let stdout = child
        .stdout
        .take()
        .context("Authentication stdout is unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("Authentication stderr is unavailable")?;
    let (event_sender, events) = flume::unbounded();
    let (input, input_receiver) = flume::unbounded::<String>();
    let (cancel, cancel_receiver) = flume::bounded(1);

    // Gemini asks for terminal consent before opening OAuth whenever stdin is
    // not a TTY. Clicking "Authenticate" in Blackholes is that explicit
    // consent, so answer it here instead of exposing a hidden CLI prompt that
    // would otherwise leave the UI waiting forever.
    if provider == AgentProvider::Gemini {
        input
            .send("y".to_string())
            .context("Unable to confirm Gemini browser authentication")?;
    }

    for (name, reader) in [
        ("stdout", Box::new(stdout) as Box<dyn Read + Send>),
        ("stderr", Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let sender = event_sender.clone();
        std::thread::Builder::new()
            .name(format!("blackholes-auth-{name}"))
            .spawn(move || stream_auth_output(reader, sender))?;
    }
    std::thread::Builder::new()
        .name("blackholes-auth-input".into())
        .spawn(move || write_auth_input(stdin, input_receiver))?;
    std::thread::Builder::new()
        .name("blackholes-auth-cancel".into())
        .spawn(move || {
            if cancel_receiver.recv().is_ok() {
                terminate_sidecar_process(process_id);
            }
        })?;
    std::thread::Builder::new()
        .name(format!("blackholes-{}-auth", provider.id()))
        .spawn(move || {
            let _active_process = active_process;
            match child.wait() {
                Ok(status) if status.success() => {
                    let _ = event_sender.send(AgentAuthEvent::Completed);
                }
                Ok(status) => {
                    let _ = event_sender.send(AgentAuthEvent::Error {
                        message: format!(
                            "{} authentication exited with {status}",
                            provider.display_name()
                        ),
                    });
                }
                Err(error) => {
                    let _ = event_sender.send(AgentAuthEvent::Error {
                        message: format!("Authentication process failed: {error}"),
                    });
                }
            }
        })?;

    Ok(AgentAuthStream {
        events,
        input,
        cancel,
    })
}

pub fn authenticate_agent_mcp(
    provider: AgentProvider,
    auth_mode: AgentAuthMode,
    profiles_root: &Path,
    workspace_id: Uuid,
    server: &AgentMcpServerConfig,
    cancellation: flume::Receiver<()>,
) -> Result<String> {
    let AgentMcpServerConfig::Http {
        name,
        url,
        oauth_client_id,
        oauth_callback_port,
    } = server
    else {
        return Err(anyhow!(
            "only remote HTTP MCP servers use browser authentication"
        ));
    };

    let connection_dir = profiles_root
        .join("mcp-connections")
        .join(provider.id())
        .join(auth_mode.id())
        .join(workspace_id.to_string());
    fs::create_dir_all(&connection_dir)
        .with_context(|| format!("Unable to create {}", connection_dir.display()))?;

    match provider {
        AgentProvider::Claude => {
            // Claude scopes the temporary registration to a Blackholes-owned
            // directory. The OAuth credential itself stays in the same Claude
            // profile used by the agents, so project files are never modified.
            let mut remove = agent_cli_command(provider)?;
            remove
                .args(["mcp", "remove", "--scope", "local", name])
                .current_dir(&connection_dir);
            configure_agent_cli_profile(&mut remove, provider, auth_mode, profiles_root)?;
            let _ = remove.output();

            let mut add = agent_cli_command(provider)?;
            add.args(["mcp", "add", "--scope", "local", "--transport", "http"]);
            if let Some(client_id) = oauth_client_id.as_deref() {
                add.args(["--client-id", client_id]);
            }
            if let Some(callback_port) = oauth_callback_port {
                add.args(["--callback-port", &callback_port.to_string()]);
            }
            add.arg(name).arg(url).current_dir(&connection_dir);
            configure_agent_cli_profile(&mut add, provider, auth_mode, profiles_root)?;
            ensure_cli_success("register the MCP server with Claude", add.output()?)?;

            let mut login = agent_cli_command(provider)?;
            login
                .args(["mcp", "login", name])
                .current_dir(&connection_dir);
            configure_agent_cli_profile(&mut login, provider, auth_mode, profiles_root)?;
            ensure_cli_success(
                "authenticate the MCP server with Claude",
                cancellable_cli_output(login, &cancellation)?,
            )?;
        }
        AgentProvider::Codex => {
            // Pass the project registration as an ephemeral config override.
            // Codex persists only the OAuth credential in the selected profile;
            // Blackholes remains the owner of the project-level server config.
            let mut login = agent_cli_command(provider)?;
            login.args([
                "-c",
                &format!("mcp_servers.{name}.url={}", serde_json::to_string(url)?),
            ]);
            if let Some(client_id) = oauth_client_id.as_deref() {
                login.args([
                    "-c",
                    &format!(
                        "mcp_servers.{name}.oauth_client_id={}",
                        serde_json::to_string(client_id)?
                    ),
                ]);
            }
            login
                .args(["mcp", "login", name])
                .current_dir(&connection_dir);
            configure_agent_cli_profile(&mut login, provider, auth_mode, profiles_root)?;
            ensure_cli_success(
                "authenticate the MCP server with Codex",
                cancellable_cli_output(login, &cancellation)?,
            )?;
        }
        AgentProvider::Gemini | AgentProvider::OpenCode => {
            return Err(anyhow!(
                "{} does not expose MCP OAuth login to Blackholes",
                provider.display_name()
            ));
        }
    }

    Ok(format!(
        "MCP {name} authenticated for {}",
        provider.display_name()
    ))
}

fn agent_cli_command(provider: AgentProvider) -> Result<Command> {
    let binary_name = match provider {
        AgentProvider::Claude => "claude",
        AgentProvider::Codex => "codex",
        AgentProvider::Gemini => "gemini",
        AgentProvider::OpenCode => "opencode",
    };
    let binary = locate_agent_binary(binary_name)?;
    if authentication_binary_needs_node(&binary) {
        let mut command = Command::new(locate_node_binary());
        command.arg(binary);
        Ok(command)
    } else {
        Ok(Command::new(binary))
    }
}

fn configure_agent_cli_profile(
    command: &mut Command,
    provider: AgentProvider,
    auth_mode: AgentAuthMode,
    profiles_root: &Path,
) -> Result<()> {
    if auth_mode != AgentAuthMode::Isolated {
        return Ok(());
    }
    let profile_dir = profiles_root.join(provider.id());
    fs::create_dir_all(&profile_dir)
        .with_context(|| format!("Unable to create {}", profile_dir.display()))?;
    match provider {
        AgentProvider::Claude => {
            command.env("CLAUDE_CONFIG_DIR", profile_dir);
        }
        AgentProvider::Codex => {
            command.env("CODEX_HOME", profile_dir);
        }
        AgentProvider::Gemini => {
            command.env("GEMINI_CLI_HOME", profile_dir);
        }
        AgentProvider::OpenCode => {
            command
                .env("XDG_DATA_HOME", profile_dir.join("data"))
                .env("XDG_CONFIG_HOME", profile_dir.join("config"))
                .env("XDG_CACHE_HOME", profile_dir.join("cache"));
        }
    }
    Ok(())
}

fn ensure_cli_success(action: &str, output: std::process::Output) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = [stderr.trim(), stdout.trim()]
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or("the provider did not return an error message");
    let detail = detail.chars().take(1_200).collect::<String>();
    Err(anyhow!("Could not {action}: {detail}"))
}

fn cancellable_cli_output(
    mut command: Command,
    cancellation: &flume::Receiver<()>,
) -> Result<std::process::Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .context("Unable to start MCP browser authentication")?;
    loop {
        if cancellation.try_recv().is_ok() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!("MCP authentication was cancelled"));
        }
        if child.try_wait()?.is_some() {
            return child
                .wait_with_output()
                .context("Unable to finish MCP browser authentication");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn authentication_command(provider: AgentProvider) -> Result<(PathBuf, Vec<&'static str>)> {
    let (binary, args) = match provider {
        AgentProvider::Claude => ("claude", vec!["auth", "login"]),
        AgentProvider::Codex => ("codex", vec!["login"]),
        AgentProvider::Gemini => ("gemini", vec!["--list-sessions"]),
        AgentProvider::OpenCode => ("opencode", vec!["auth", "login", "--provider", "opencode"]),
    };
    Ok((locate_agent_binary(binary)?, args))
}

fn locate_agent_binary(binary: &str) -> Result<PathBuf> {
    if let Some(sidecar_root) = locate_sidecar_script()?.parent() {
        let bundled = sidecar_root.join("node_modules/.bin").join(binary);
        if bundled.is_file() {
            return Ok(bundled);
        }
        if binary == "claude" {
            let anthropic_modules = sidecar_root.join("node_modules/@anthropic-ai");
            if let Ok(entries) = fs::read_dir(anthropic_modules) {
                if let Some(bundled) = entries.flatten().find_map(|entry| {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    let candidate = entry.path().join("claude");
                    (name.starts_with("claude-agent-sdk-") && candidate.is_file())
                        .then_some(candidate)
                }) {
                    return Ok(bundled);
                }
            }
        }
    }
    let mut directories = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_default();
    directories.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        directories.extend([
            home.join(".local/bin"),
            home.join(".volta/bin"),
            home.join(".asdf/shims"),
        ]);
    }
    directories
        .into_iter()
        .map(|directory| directory.join(binary))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| anyhow!("{} is not installed", binary))
}

fn authentication_binary_needs_node(binary: &Path) -> bool {
    fs::read(binary).ok().is_some_and(|bytes| {
        let prefix = &bytes[..bytes.len().min(256)];
        prefix.starts_with(b"#!")
            && String::from_utf8_lossy(&prefix)
                .to_ascii_lowercase()
                .contains("node")
    })
}

fn prepare_gemini_oauth_profile(profile_dir: &Path) -> Result<()> {
    // Gemini treats GEMINI_CLI_HOME as the home root and keeps its user
    // configuration and OAuth credentials in the .gemini directory below it.
    let settings_dir = profile_dir.join(".gemini");
    fs::create_dir_all(&settings_dir)?;
    let settings_path = settings_dir.join("settings.json");
    let mut settings = fs::read_to_string(&settings_path)
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    settings["security"]["auth"]["selectedType"] = Value::String("oauth-personal".into());
    fs::write(&settings_path, serde_json::to_vec_pretty(&settings)?)?;
    Ok(())
}

fn write_auth_input(mut stdin: impl Write, input: flume::Receiver<String>) {
    while let Ok(value) = input.recv() {
        if stdin.write_all(value.as_bytes()).is_err()
            || stdin.write_all(b"\n").is_err()
            || stdin.flush().is_err()
        {
            break;
        }
    }
}

fn stream_auth_output(mut reader: Box<dyn Read + Send>, sender: flume::Sender<AgentAuthEvent>) {
    let url_pattern = regex::Regex::new(r#"https?://[^\s\x1b<>\"']+"#)
        .expect("the authentication URL pattern must be valid");
    let ansi_pattern = regex::Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]")
        .expect("the ANSI escape pattern must be valid");
    let mut buffer = [0_u8; 4096];
    let mut rolling = String::new();
    loop {
        let Ok(read) = reader.read(&mut buffer) else {
            break;
        };
        if read == 0 {
            break;
        }
        let raw = String::from_utf8_lossy(&buffer[..read]);
        let clean = ansi_pattern.replace_all(&raw, "").replace('\r', "\n");
        rolling.push_str(&clean);
        for found in url_pattern.find_iter(&rolling) {
            let url = found
                .as_str()
                .trim_end_matches([')', ']', '}', '.', ','])
                .to_string();
            let _ = sender.send(AgentAuthEvent::OpenUrl { url });
        }
        if rolling.len() > 16_384 {
            rolling = rolling.split_off(rolling.len() - 8_192);
        }
        if sender.send(AgentAuthEvent::Output { text: clean }).is_err() {
            break;
        }
    }
}
