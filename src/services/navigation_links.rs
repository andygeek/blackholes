//! Links an agent leaves in its own terminal to point the user to an agent session, a project
//! or a Worktree Group. The view never moves: the user opens a link when they choose.
use crate::model::{AppSession, ProjectTask, SuggestedLink, SuggestedLinkTarget, TerminalDescriptor, Workspace};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use uuid::Uuid;

pub const MAX_LINKS: usize = 3;
pub const MAX_REASON_CHARS: usize = 160;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SuggestNavigationRequest {
    #[serde(default)]
    pub links: Vec<LinkRequest>,
    /// Removes the links of the calling terminal.
    #[serde(default)]
    pub clear: bool,
}

/// One link: exactly one of the IDs, and an optional reason.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkRequest {
    #[serde(default)]
    pub terminal_id: Option<String>,
    #[serde(default)]
    pub provider_session_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

fn given(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|value| !value.is_empty())
}

impl SuggestNavigationRequest {
    pub fn validate(&self) -> Result<()> {
        if self.clear {
            ensure!(self.links.is_empty(), "Pass links or clear:true, not both");
            return Ok(());
        }
        ensure!(
            (1..=MAX_LINKS).contains(&self.links.len()),
            "Pass 1-{MAX_LINKS} links, or clear:true to remove this terminal's links"
        );
        for (index, link) in self.links.iter().enumerate() {
            let ids = [&link.terminal_id, &link.provider_session_id, &link.project_id, &link.group_id]
                .into_iter().filter(|value| given(value).is_some()).count();
            ensure!(ids == 1, "links[{index}] needs exactly one of terminalId, providerSessionId, projectId or groupId");
            if let Some(reason) = &link.reason {
                ensure!(
                    reason.chars().count() <= MAX_REASON_CHARS,
                    "links[{index}].reason is longer than {MAX_REASON_CHARS} characters"
                );
            }
        }
        Ok(())
    }
}

/// The reason as shown: one trimmed line, or nothing.
pub fn clean_reason(reason: Option<&str>) -> Option<String> {
    let line = reason?.split_whitespace().collect::<Vec<_>>().join(" ");
    (!line.is_empty()).then_some(line)
}

pub fn provider_session_id(terminal: &TerminalDescriptor) -> Option<&str> {
    terminal.claude_session.as_ref().map(|session| session.id.as_str())
        .or_else(|| terminal.codex_session.as_ref().map(|session| session.id.as_str()))
}

/// Resolves one link against what Blackholes has open. The calling session cannot link to itself.
pub fn resolve(
    link: &LinkRequest,
    session: &AppSession,
    workspaces: &[Workspace],
    tasks: &[ProjectTask],
    caller: Uuid,
) -> Result<SuggestedLinkTarget> {
    let id = |field: &str, value: &str| Uuid::parse_str(value)
        .with_context(|| format!("{field} \"{value}\" is not an ID"));
    let target = if let Some(value) = given(&link.terminal_id) {
        let terminal_id = id("terminalId", value)?;
        ensure!(
            session.terminals.iter().any(|terminal| terminal.id == terminal_id),
            "No open session has terminalId {terminal_id}; read them with list_agent_sessions. A closed session cannot be linked"
        );
        SuggestedLinkTarget::Session { terminal_id }
    } else if let Some(value) = given(&link.provider_session_id) {
        let terminal = session.terminals.iter()
            .find(|terminal| provider_session_id(terminal) == Some(value))
            .with_context(|| format!(
                "No open session holds providerSessionId {value}. A closed session cannot be linked: give the user its name instead"
            ))?;
        SuggestedLinkTarget::Session { terminal_id: terminal.id }
    } else if let Some(value) = given(&link.project_id) {
        let project_id = id("projectId", value)?;
        ensure!(workspaces.iter().any(|workspace| workspace.id == project_id), "No project has ID {project_id}");
        SuggestedLinkTarget::Project { project_id }
    } else {
        let value = given(&link.group_id).context("A link needs terminalId, providerSessionId, projectId or groupId")?;
        let group_id = id("groupId", value)?;
        ensure!(
            tasks.iter().any(|task| task.id == group_id),
            "No open Worktree Group has ID {group_id}. A closed (archived) group cannot be linked"
        );
        SuggestedLinkTarget::WorktreeGroup { group_id }
    };
    ensure!(target != SuggestedLinkTarget::Session { terminal_id: caller }, "That is the calling session itself");
    Ok(target)
}

/// Whether the target is still there to open.
pub fn target_exists(target: SuggestedLinkTarget, session: &AppSession, workspaces: &[Workspace], tasks: &[ProjectTask]) -> bool {
    match target {
        SuggestedLinkTarget::Session { terminal_id } => session.terminals.iter().any(|terminal| terminal.id == terminal_id),
        SuggestedLinkTarget::Project { project_id } => workspaces.iter().any(|workspace| workspace.id == project_id),
        SuggestedLinkTarget::WorktreeGroup { group_id } => tasks.iter().any(|task| task.id == group_id),
    }
}

/// Adds links after the ones a terminal already has, as `open_*` calls do one at a time: a target
/// already there moves to the end, and only the newest `MAX_LINKS` stay.
pub fn append(existing: Vec<SuggestedLink>, added: Vec<SuggestedLink>) -> Vec<SuggestedLink> {
    let mut links: Vec<SuggestedLink> = existing.into_iter()
        .filter(|link| !added.iter().any(|new| new.target == link.target))
        .collect();
    links.extend(added);
    let excess = links.len().saturating_sub(MAX_LINKS);
    links.drain(..excess);
    links
}

/// Drops the links of closed terminals and those pointing to a closed session.
pub fn forget_terminal(session: &mut AppSession, terminal_id: Uuid) {
    session.terminal_links.remove(&terminal_id);
    for links in session.terminal_links.values_mut() {
        links.retain(|link| link.target != SuggestedLinkTarget::Session { terminal_id });
    }
    session.terminal_links.retain(|_, links| !links.is_empty());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentKind, ClaudeProfile, ClaudeSession, SessionState};
    use chrono::Utc;

    fn terminal(id: Uuid, provider: Option<&str>) -> TerminalDescriptor {
        TerminalDescriptor {
            id, workspace_id: Uuid::nil(), task_id: None, repository_id: None, agent: AgentKind::Claude,
            label: "session".into(), custom_label: None, section_id: None, note: None, cwd: "/tmp".into(),
            state: SessionState::Idle, codex_session: None,
            claude_session: provider.map(|id| ClaudeSession { id: id.into(), profile: ClaudeProfile::Default }),
            agent_config_dir: None, model: None, created_at: Utc::now(), listed_in_agents: true, last_active_at: None,
        }
    }

    fn link(json: serde_json::Value) -> LinkRequest {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn requests_need_one_id_per_link_and_at_most_three_links() {
        let request = |json: serde_json::Value| serde_json::from_value::<SuggestNavigationRequest>(json).unwrap().validate();
        assert!(request(serde_json::json!({ "links": [{ "projectId": "x" }] })).is_ok());
        assert!(request(serde_json::json!({ "clear": true })).is_ok());
        assert!(request(serde_json::json!({})).is_err());
        assert!(request(serde_json::json!({ "clear": true, "links": [{ "projectId": "x" }] })).is_err());
        assert!(request(serde_json::json!({ "links": [{ "projectId": "x", "groupId": "y" }] })).is_err());
        assert!(request(serde_json::json!({ "links": [{ "reason": "only a reason" }] })).is_err());
        assert!(request(serde_json::json!({ "links": [{ "projectId": "a" }, { "projectId": "b" }, { "projectId": "c" }, { "projectId": "d" }] })).is_err());
        assert!(request(serde_json::json!({ "links": [{ "projectId": "x", "reason": "a".repeat(MAX_REASON_CHARS + 1) }] })).is_err());
        assert!(serde_json::from_value::<SuggestNavigationRequest>(serde_json::json!({ "links": [{ "taskId": "x" }] })).is_err());
        assert_eq!(clean_reason(Some("  Hizo el\n cambio  ")), Some("Hizo el cambio".into()));
        assert_eq!(clean_reason(Some(" \n ")), None);
    }

    #[test]
    fn links_resolve_sessions_by_either_id_and_refuse_the_caller() {
        let caller = Uuid::new_v4();
        let other = Uuid::new_v4();
        let session = AppSession { terminals: vec![terminal(caller, Some("mine")), terminal(other, Some("abc"))], ..AppSession::default() };
        let resolve = |json| resolve(&link(json), &session, &[], &[], caller);
        assert_eq!(resolve(serde_json::json!({ "providerSessionId": "abc" })).unwrap(), SuggestedLinkTarget::Session { terminal_id: other });
        assert_eq!(resolve(serde_json::json!({ "terminalId": other.to_string() })).unwrap(), SuggestedLinkTarget::Session { terminal_id: other });
        assert!(resolve(serde_json::json!({ "terminalId": caller.to_string() })).unwrap_err().to_string().contains("calling session"));
        assert!(resolve(serde_json::json!({ "providerSessionId": "gone" })).unwrap_err().to_string().contains("closed session"));
        assert!(resolve(serde_json::json!({ "projectId": Uuid::new_v4().to_string() })).is_err());
        assert!(resolve(serde_json::json!({ "groupId": "not-an-id" })).unwrap_err().to_string().contains("not an ID"));
    }

    #[test]
    fn appended_links_keep_the_newest_three_without_repeating_a_target() {
        let suggested = |project_id| crate::model::SuggestedLink {
            id: Uuid::new_v4(), target: SuggestedLinkTarget::Project { project_id }, reason: None, created_at: Utc::now(),
        };
        let ids: Vec<Uuid> = (0..4).map(|_| Uuid::new_v4()).collect();
        let targets = |links: &[crate::model::SuggestedLink]| links.iter().map(|link| link.target).collect::<Vec<_>>();
        let project = |index: usize| SuggestedLinkTarget::Project { project_id: ids[index] };
        let links = append(vec![suggested(ids[0]), suggested(ids[1])], vec![suggested(ids[0])]);
        assert_eq!(targets(&links), vec![project(1), project(0)]);
        let links = append(links, vec![suggested(ids[2]), suggested(ids[3])]);
        assert_eq!(targets(&links), vec![project(0), project(2), project(3)]);
    }

    #[test]
    fn closing_a_terminal_forgets_its_links_and_links_to_it() {
        let (caller, target, other) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let suggested = |target| crate::model::SuggestedLink { id: Uuid::new_v4(), target, reason: None, created_at: Utc::now() };
        let mut session = AppSession::default();
        session.terminal_links.insert(caller, vec![
            suggested(SuggestedLinkTarget::Session { terminal_id: target }),
            suggested(SuggestedLinkTarget::Project { project_id: other }),
        ]);
        session.terminal_links.insert(other, vec![suggested(SuggestedLinkTarget::Session { terminal_id: target })]);
        forget_terminal(&mut session, target);
        assert_eq!(session.terminal_links.len(), 1);
        assert_eq!(session.terminal_links[&caller].len(), 1);
        forget_terminal(&mut session, caller);
        assert!(session.terminal_links.is_empty());
    }
}
