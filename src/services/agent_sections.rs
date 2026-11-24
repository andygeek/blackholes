//! Organization of the Agents list: sections, session names, notes, pins and order.
//!
//! Everything here only changes how the Agents list is presented. The project tree,
//! worktrees, repositories and agent processes are never touched.
use crate::model::{AgentKind, AgentSection, AppSession, TerminalDescriptor};
use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer};
use uuid::Uuid;

pub const MAX_SECTIONS: usize = 32;
pub const MAX_SECTION_NAME_CHARS: usize = 48;
pub const MAX_SESSION_NAME_CHARS: usize = 72;
pub const MAX_NOTE_CHARS: usize = 280;
const MAX_CHANGES: usize = 256;

/// A batch of changes, applied in this order: deleted sections, section changes,
/// session changes, section order and session order. Nothing is applied if any part fails.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrganizeRequest {
    #[serde(default)]
    pub delete_sections: Vec<String>,
    #[serde(default)]
    pub sections: Vec<SectionChange>,
    #[serde(default)]
    pub sessions: Vec<SessionChange>,
    #[serde(default)]
    pub section_order: Vec<String>,
    #[serde(default)]
    pub session_order: Vec<String>,
}

/// With `id`, renames or folds that section. Without it, `name` selects the section with
/// that name (ignoring case) or creates it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SectionChange {
    #[serde(default)]
    pub id: Option<Uuid>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub collapsed: Option<bool>,
}

/// Omitted fields stay as they are; `null` (or an empty string) clears a name, note or section.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionChange {
    /// A terminal ID, or `current` for the session making the call.
    pub terminal_id: String,
    #[serde(default, deserialize_with = "present")]
    pub name: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub note: Option<Option<String>>,
    /// Section ID or name. A name that does not exist yet creates the section.
    #[serde(default, deserialize_with = "present")]
    pub section: Option<Option<String>>,
    #[serde(default)]
    pub pinned: Option<bool>,
}

/// Distinguishes an omitted field (`None`) from an explicit `null` (`Some(None)`).
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Default)]
pub struct OrganizeOutcome {
    /// Sessions whose shown name changed, with the new name.
    pub renamed: Vec<(Uuid, String)>,
}

/// Collapses whitespace and limits the length; `None` when nothing remains.
pub fn clean_text(value: &str, max_chars: usize) -> Option<String> {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let value: String = value.chars().take(max_chars).collect();
    (!value.is_empty()).then_some(value)
}

/// The session's name without the status glyphs agents put before their titles (such as
/// `✳` or a Braille spinner). A name the user typed is returned as typed.
pub fn shown_name(terminal: &TerminalDescriptor) -> String {
    match &terminal.custom_label {
        Some(name) => name.clone(),
        None => terminal.label.trim_start_matches(|character: char| !character.is_alphanumeric()).trim().to_string(),
    }
}

/// Agent sessions appear in the Agents list; plain shells never do.
pub fn is_agent(terminal: &TerminalDescriptor) -> bool {
    terminal.agent != AgentKind::Shell
}

/// Sets or clears the name the user gave a session and returns the name now shown.
/// Clearing it shows the agent's name until the agent sets a new title.
pub fn set_session_name(session: &mut AppSession, terminal_id: Uuid, name: Option<&str>) -> Option<String> {
    let name = name.and_then(|name| clean_text(name, MAX_SESSION_NAME_CHARS));
    let terminal = session.terminals.iter_mut().find(|terminal| terminal.id == terminal_id)?;
    match name {
        Some(name) => {
            terminal.custom_label = Some(name.clone());
            terminal.label = name;
        }
        None => {
            terminal.custom_label = None;
            terminal.label = terminal.agent.label().into();
        }
    }
    let label = terminal.label.clone();
    set_tab_title(session, terminal_id, &label);
    Some(label)
}

/// Retitles the tabs that hold only this terminal; split tabs keep their own title.
pub fn set_tab_title(session: &mut AppSession, terminal_id: Uuid, title: &str) {
    for dock in session.docks.values_mut() {
        for tab in &mut dock.tabs {
            let mut ids = Vec::new();
            tab.root.terminal_ids(&mut ids);
            if ids.contains(&terminal_id) && ids.len() == 1 {
                tab.title = title.chars().take(MAX_SESSION_NAME_CHARS).collect();
            }
        }
    }
}

pub fn set_session_note(session: &mut AppSession, terminal_id: Uuid, note: Option<&str>) {
    if let Some(terminal) = session.terminals.iter_mut().find(|terminal| terminal.id == terminal_id) {
        terminal.note = note.and_then(|note| clean_text(note, MAX_NOTE_CHARS));
    }
}

pub fn create_section(session: &mut AppSession, name: &str) -> Result<Uuid> {
    let Some(name) = clean_text(name, MAX_SECTION_NAME_CHARS) else {
        bail!("A section needs a name");
    };
    if let Some(section) = find_section_by_name(session, &name) {
        return Ok(section.id);
    }
    if session.agent_sections.len() >= MAX_SECTIONS {
        bail!("The Agents list can have at most {MAX_SECTIONS} sections");
    }
    let id = Uuid::new_v4();
    session.agent_sections.push(AgentSection { id, name, collapsed: false });
    Ok(id)
}

pub fn rename_section(session: &mut AppSession, section_id: Uuid, name: &str) -> Result<()> {
    let Some(name) = clean_text(name, MAX_SECTION_NAME_CHARS) else {
        bail!("A section needs a name");
    };
    if find_section_by_name(session, &name).is_some_and(|section| section.id != section_id) {
        bail!("Another section is already named \"{name}\"");
    }
    let Some(section) = session.agent_sections.iter_mut().find(|section| section.id == section_id) else {
        bail!("Unknown section {section_id}");
    };
    section.name = name;
    Ok(())
}

/// Removes the section; its sessions stay in the Agents list without a section.
pub fn delete_section(session: &mut AppSession, section_id: Uuid) {
    session.agent_sections.retain(|section| section.id != section_id);
    for terminal in &mut session.terminals {
        if terminal.section_id == Some(section_id) {
            terminal.section_id = None;
        }
    }
}

pub fn move_section(session: &mut AppSession, section_id: Uuid, offset: i32) {
    let Some(index) = session.agent_sections.iter().position(|section| section.id == section_id) else {
        return;
    };
    let target = (index as i64 + offset as i64).clamp(0, session.agent_sections.len() as i64 - 1) as usize;
    let section = session.agent_sections.remove(index);
    session.agent_sections.insert(target, section);
}

pub fn set_section_collapsed(session: &mut AppSession, section_id: Uuid, collapsed: bool) {
    if let Some(section) = session.agent_sections.iter_mut().find(|section| section.id == section_id) {
        section.collapsed = collapsed;
    }
}

/// Moves an agent session into a section, or out of every section with `None`.
pub fn set_session_section(session: &mut AppSession, terminal_id: Uuid, section_id: Option<Uuid>) -> Result<()> {
    if let Some(section_id) = section_id
        && !session.agent_sections.iter().any(|section| section.id == section_id)
    {
        bail!("Unknown section {section_id}");
    }
    let Some(terminal) = session.terminals.iter_mut().find(|terminal| terminal.id == terminal_id && is_agent(terminal)) else {
        bail!("Unknown agent session {terminal_id}");
    };
    terminal.section_id = section_id;
    Ok(())
}

pub fn set_pinned(session: &mut AppSession, terminal_id: Uuid, pinned: bool) {
    session.pinned_terminal_ids.retain(|id| *id != terminal_id);
    if pinned && session.terminals.iter().any(|terminal| terminal.id == terminal_id && is_agent(terminal)) {
        session.pinned_terminal_ids.push(terminal_id);
    }
}

/// Places `ids` in the slots their members already occupy, in the given order; unknown
/// members are appended. Everything else keeps its position.
pub fn merge_order<T: Clone + PartialEq>(order: &mut Vec<T>, ids: &[T]) {
    let mut requested = Vec::new();
    for id in ids {
        if !requested.contains(id) {
            requested.push(id.clone());
        }
    }
    let mut next = requested.iter();
    let mut merged = Vec::with_capacity(order.len() + requested.len());
    for id in order.iter() {
        if requested.contains(id) {
            if let Some(id) = next.next() {
                merged.push(id.clone());
            }
        } else {
            merged.push(id.clone());
        }
    }
    merged.extend(next.cloned());
    *order = merged;
}

/// Orders sessions manually: unpinned sessions through `agent_order` and pinned ones
/// through the pin order. Notices and the working state still sort ahead of this order.
pub fn order_sessions(session: &mut AppSession, terminal_ids: &[Uuid]) {
    let keys: Vec<String> = terminal_ids.iter().map(|id| format!("terminal:{id}")).collect();
    merge_order(&mut session.agent_order, &keys);
    let pinned: Vec<Uuid> = terminal_ids.iter().copied()
        .filter(|id| session.pinned_terminal_ids.contains(id)).collect();
    merge_order(&mut session.pinned_terminal_ids, &pinned);
}

fn find_section_by_name<'a>(session: &'a AppSession, name: &str) -> Option<&'a AgentSection> {
    let name = name.to_lowercase();
    session.agent_sections.iter().find(|section| section.name.to_lowercase() == name)
}

fn resolve_section(session: &AppSession, reference: &str) -> Option<Uuid> {
    let reference = reference.trim();
    Uuid::parse_str(reference).ok()
        .filter(|id| session.agent_sections.iter().any(|section| section.id == *id))
        .or_else(|| find_section_by_name(session, reference).map(|section| section.id))
}

fn resolve_terminal(session: &AppSession, reference: &str, caller: Option<Uuid>) -> Result<Uuid> {
    let reference = reference.trim();
    let id = if reference.eq_ignore_ascii_case("current") {
        let Some(caller) = caller else {
            bail!("\"current\" needs a call from a Blackholes terminal; pass its terminalId instead");
        };
        caller
    } else {
        let Ok(id) = Uuid::parse_str(reference) else {
            bail!("\"{reference}\" is not a terminal ID; read them with list_agent_sessions");
        };
        id
    };
    if !session.terminals.iter().any(|terminal| terminal.id == id && is_agent(terminal)) {
        bail!("No agent session has terminal ID {id}");
    }
    Ok(id)
}

/// Applies a batch atomically: on any error the session is left unchanged.
pub fn organize(session: &mut AppSession, request: &OrganizeRequest, caller: Option<Uuid>) -> Result<OrganizeOutcome> {
    let sizes = [request.delete_sections.len(), request.sections.len(), request.sessions.len(),
        request.section_order.len(), request.session_order.len()];
    if sizes.iter().any(|size| *size > MAX_CHANGES) {
        bail!("Send at most {MAX_CHANGES} entries per list");
    }
    if sizes.iter().all(|size| *size == 0) {
        bail!("Nothing to change: pass sections, sessions, sectionOrder, sessionOrder or deleteSections");
    }
    let mut next = session.clone();
    let mut outcome = OrganizeOutcome::default();

    for reference in &request.delete_sections {
        let Some(id) = resolve_section(&next, reference) else {
            bail!("Unknown section \"{reference}\"");
        };
        delete_section(&mut next, id);
    }

    for change in &request.sections {
        let id = match (change.id, change.name.as_deref()) {
            (Some(id), name) => {
                if !next.agent_sections.iter().any(|section| section.id == id) {
                    bail!("Unknown section {id}");
                }
                if let Some(name) = name {
                    rename_section(&mut next, id, name)?;
                }
                id
            }
            (None, Some(name)) => create_section(&mut next, name)?,
            (None, None) => bail!("Each section change needs an id or a name"),
        };
        if let Some(collapsed) = change.collapsed {
            set_section_collapsed(&mut next, id, collapsed);
        }
    }

    for change in &request.sessions {
        let terminal_id = resolve_terminal(&next, &change.terminal_id, caller)?;
        if let Some(name) = &change.name
            && let Some(label) = set_session_name(&mut next, terminal_id, name.as_deref())
        {
            outcome.renamed.retain(|(id, _)| *id != terminal_id);
            outcome.renamed.push((terminal_id, label));
        }
        if let Some(note) = &change.note {
            set_session_note(&mut next, terminal_id, note.as_deref());
        }
        if let Some(section) = &change.section {
            let section_id = match section.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
                None => None,
                Some(reference) => Some(match resolve_section(&next, reference) {
                    Some(id) => id,
                    None if Uuid::parse_str(reference).is_ok() => bail!("Unknown section {reference}"),
                    None => create_section(&mut next, reference)?,
                }),
            };
            set_session_section(&mut next, terminal_id, section_id)?;
        }
        if let Some(pinned) = change.pinned {
            set_pinned(&mut next, terminal_id, pinned);
        }
    }

    if !request.section_order.is_empty() {
        let mut ids = Vec::new();
        for reference in &request.section_order {
            let Some(id) = resolve_section(&next, reference) else {
                bail!("Unknown section \"{reference}\"");
            };
            ids.push(id);
        }
        let mut sections = Vec::with_capacity(next.agent_sections.len());
        for id in &ids {
            if let Some(section) = next.agent_sections.iter().find(|section| section.id == *id)
                && !sections.iter().any(|current: &AgentSection| current.id == *id)
            {
                sections.push(section.clone());
            }
        }
        // Sections left out keep their relative order after the listed ones.
        for section in &next.agent_sections {
            if !ids.contains(&section.id) {
                sections.push(section.clone());
            }
        }
        next.agent_sections = sections;
    }

    if !request.session_order.is_empty() {
        let mut ids = Vec::new();
        for reference in &request.session_order {
            ids.push(resolve_terminal(&next, reference, caller)?);
        }
        order_sessions(&mut next, &ids);
    }

    *session = next;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SessionState;

    fn terminal(agent: AgentKind) -> TerminalDescriptor {
        TerminalDescriptor {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            task_id: None,
            repository_id: None,
            agent,
            label: "✳ Title".into(),
            custom_label: None,
            section_id: None,
            note: None,
            cwd: "/tmp".into(),
            state: SessionState::Idle,
            codex_session: None,
            claude_session: None,
            agent_config_dir: None,
            model: None,
            created_at: chrono::Utc::now(),
        }
    }

    fn session(terminals: Vec<TerminalDescriptor>) -> AppSession {
        AppSession { terminals, ..AppSession::default() }
    }

    fn request(value: serde_json::Value) -> OrganizeRequest {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_section_name_creates_the_section_and_moves_the_session() {
        let agent = terminal(AgentKind::Claude);
        let id = agent.id;
        let mut state = session(vec![agent]);
        organize(&mut state, &request(serde_json::json!({
            "sessions": [{ "terminalId": id.to_string(), "section": "Reviews", "name": "  PR   123 ", "note": "Waiting for CI" }],
        })), None).unwrap();
        assert_eq!(state.agent_sections.len(), 1);
        assert_eq!(state.agent_sections[0].name, "Reviews");
        let terminal = &state.terminals[0];
        assert_eq!(terminal.section_id, Some(state.agent_sections[0].id));
        assert_eq!(terminal.custom_label.as_deref(), Some("PR 123"));
        assert_eq!(terminal.label, "PR 123");
        assert_eq!(terminal.note.as_deref(), Some("Waiting for CI"));
    }

    #[test]
    fn section_names_match_ignoring_case_and_ids_are_not_names() {
        let agent = terminal(AgentKind::Codex);
        let id = agent.id;
        let mut state = session(vec![agent]);
        let section = create_section(&mut state, "Release").unwrap();
        organize(&mut state, &request(serde_json::json!({
            "sessions": [{ "terminalId": id.to_string(), "section": "release" }],
        })), None).unwrap();
        assert_eq!(state.agent_sections.len(), 1);
        assert_eq!(state.terminals[0].section_id, Some(section));
        let missing = Uuid::new_v4().to_string();
        assert!(organize(&mut state, &request(serde_json::json!({
            "sessions": [{ "terminalId": id.to_string(), "section": missing }],
        })), None).is_err());
    }

    #[test]
    fn a_failed_batch_changes_nothing() {
        let agent = terminal(AgentKind::Claude);
        let id = agent.id;
        let mut state = session(vec![agent]);
        let result = organize(&mut state, &request(serde_json::json!({
            "sessions": [
                { "terminalId": id.to_string(), "section": "Kept?" },
                { "terminalId": Uuid::new_v4().to_string(), "name": "missing" },
            ],
        })), None);
        assert!(result.is_err());
        assert!(state.agent_sections.is_empty());
        assert_eq!(state.terminals[0].section_id, None);
    }

    #[test]
    fn null_clears_and_omitted_fields_stay() {
        let agent = terminal(AgentKind::Claude);
        let id = agent.id;
        let mut state = session(vec![agent]);
        organize(&mut state, &request(serde_json::json!({
            "sessions": [{ "terminalId": "current", "name": "Docs", "note": "Writing", "section": "Work", "pinned": true }],
        })), Some(id)).unwrap();
        organize(&mut state, &request(serde_json::json!({
            "sessions": [{ "terminalId": id.to_string(), "name": null, "section": null }],
        })), None).unwrap();
        let terminal = &state.terminals[0];
        assert_eq!(terminal.custom_label, None);
        assert_eq!(terminal.label, AgentKind::Claude.label());
        assert_eq!(terminal.section_id, None);
        assert_eq!(terminal.note.as_deref(), Some("Writing"));
        assert_eq!(state.pinned_terminal_ids, vec![id]);
        assert_eq!(state.agent_sections.len(), 1, "removing a session from a section keeps the section");
    }

    #[test]
    fn shells_and_unknown_current_callers_are_rejected() {
        let shell = terminal(AgentKind::Shell);
        let id = shell.id;
        let mut state = session(vec![shell]);
        assert!(organize(&mut state, &request(serde_json::json!({
            "sessions": [{ "terminalId": id.to_string(), "section": "Shells" }],
        })), None).is_err());
        assert!(organize(&mut state, &request(serde_json::json!({
            "sessions": [{ "terminalId": "current", "note": "x" }],
        })), None).is_err());
        assert!(serde_json::from_value::<OrganizeRequest>(serde_json::json!({ "unknown": [] })).is_err());
    }

    #[test]
    fn deleting_a_section_keeps_its_sessions_listed_without_it() {
        let agent = terminal(AgentKind::Claude);
        let id = agent.id;
        let mut state = session(vec![agent]);
        let section = create_section(&mut state, "Old").unwrap();
        set_session_section(&mut state, id, Some(section)).unwrap();
        organize(&mut state, &request(serde_json::json!({ "deleteSections": ["old"] })), None).unwrap();
        assert!(state.agent_sections.is_empty());
        assert_eq!(state.terminals.len(), 1);
        assert_eq!(state.terminals[0].section_id, None);
    }

    #[test]
    fn section_order_lists_given_sections_first() {
        let mut state = session(Vec::new());
        let a = create_section(&mut state, "A").unwrap();
        let b = create_section(&mut state, "B").unwrap();
        let c = create_section(&mut state, "C").unwrap();
        organize(&mut state, &request(serde_json::json!({ "sectionOrder": ["c", a.to_string()] })), None).unwrap();
        let ids: Vec<_> = state.agent_sections.iter().map(|section| section.id).collect();
        assert_eq!(ids, vec![c, a, b]);
        move_section(&mut state, b, -5);
        assert_eq!(state.agent_sections[0].id, b);
    }

    #[test]
    fn merge_order_only_moves_the_given_members() {
        let mut order = vec!["a", "b", "c", "d", "e"];
        merge_order(&mut order, &["d", "b", "z"]);
        assert_eq!(order, vec!["a", "d", "c", "b", "e", "z"]);
    }

    #[test]
    fn session_order_reorders_pinned_and_unpinned_sessions() {
        let terminals: Vec<_> = (0..3).map(|_| terminal(AgentKind::Claude)).collect();
        let ids: Vec<_> = terminals.iter().map(|terminal| terminal.id).collect();
        let mut state = session(terminals);
        state.agent_order = ids.iter().map(|id| format!("terminal:{id}")).collect();
        state.pinned_terminal_ids = vec![ids[0], ids[1]];
        organize(&mut state, &request(serde_json::json!({
            "sessionOrder": [ids[2].to_string(), ids[1].to_string(), ids[0].to_string()],
        })), None).unwrap();
        assert_eq!(state.pinned_terminal_ids, vec![ids[1], ids[0]]);
        assert_eq!(state.agent_order, vec![
            format!("terminal:{}", ids[2]), format!("terminal:{}", ids[1]), format!("terminal:{}", ids[0]),
        ]);
    }
}
