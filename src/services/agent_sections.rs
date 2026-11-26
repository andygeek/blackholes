//! Organization of the Agents list: sections, session names, notes, pins and order.
//!
//! Everything here only changes how the Agents list is presented. The project tree,
//! worktrees, repositories and agent processes are never touched.
use crate::model::{AgentKind, AgentSection, AppSession, TerminalDescriptor};
use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer};
use uuid::Uuid;

pub const MAX_SECTIONS: usize = 64;
/// A section, its sub-sections and theirs.
pub const MAX_SECTION_DEPTH: usize = 3;
pub const MAX_SECTION_NAME_CHARS: usize = 48;
pub const MAX_SECTION_DESCRIPTION_CHARS: usize = 500;
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

/// With `id`, changes that section. Without it, `name` selects the section with that name
/// (ignoring case) or creates it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SectionChange {
    #[serde(default)]
    pub id: Option<Uuid>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub collapsed: Option<bool>,
    /// What the section holds, for agents choosing where a session goes; `null` clears it.
    #[serde(default, deserialize_with = "present")]
    pub description: Option<Option<String>>,
    /// Section ID or name that contains this one; `null` moves it to the top of the list.
    #[serde(default, deserialize_with = "present")]
    pub parent: Option<Option<String>>,
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

/// Last time the user opened the session or its agent was active; its creation when unknown.
pub fn last_active(terminal: &TerminalDescriptor) -> DateTime<Utc> {
    terminal.last_active_at.unwrap_or(terminal.created_at)
}

/// `3 days`, `5 hours` or `12 minutes`.
pub fn describe_age(age: chrono::Duration) -> String {
    let (count, unit) = if age.num_days() >= 1 {
        (age.num_days(), "day")
    } else if age.num_hours() >= 1 {
        (age.num_hours(), "hour")
    } else {
        (age.num_minutes().max(0), "minute")
    };
    format!("{count} {unit}{}", if count == 1 { "" } else { "s" })
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

/// Creates a section at the end of `parent` (or of the list), or returns the section that
/// already has this name, wherever it is.
pub fn create_section(session: &mut AppSession, name: &str, parent: Option<Uuid>) -> Result<Uuid> {
    let Some(name) = clean_text(name, MAX_SECTION_NAME_CHARS) else {
        bail!("A section needs a name");
    };
    if let Some(section) = find_section_by_name(session, &name) {
        return Ok(section.id);
    }
    if session.agent_sections.len() >= MAX_SECTIONS {
        bail!("The Agents list can have at most {MAX_SECTIONS} sections");
    }
    if let Some(parent) = parent {
        if !session.agent_sections.iter().any(|section| section.id == parent) {
            bail!("Unknown section {parent}");
        }
        if section_depth(session, parent) + 1 >= MAX_SECTION_DEPTH {
            bail!("Sections can be nested at most {MAX_SECTION_DEPTH} levels deep");
        }
    }
    let id = Uuid::new_v4();
    session.agent_sections.push(AgentSection { id, name, collapsed: false, description: None, parent_id: parent });
    Ok(id)
}

pub fn set_section_description(session: &mut AppSession, section_id: Uuid, description: Option<&str>) -> Result<()> {
    let Some(section) = session.agent_sections.iter_mut().find(|section| section.id == section_id) else {
        bail!("Unknown section {section_id}");
    };
    section.description = description.and_then(|text| clean_text(text, MAX_SECTION_DESCRIPTION_CHARS));
    Ok(())
}

fn parent_of(session: &AppSession, section_id: Uuid) -> Option<Uuid> {
    session.agent_sections.iter().find(|section| section.id == section_id)?.parent_id
}

/// 0 for a section at the top of the list.
pub fn section_depth(session: &AppSession, section_id: Uuid) -> usize {
    let mut depth = 0;
    let mut current = parent_of(session, section_id);
    while let Some(id) = current {
        depth += 1;
        if depth > MAX_SECTIONS {
            break;
        }
        current = parent_of(session, id);
    }
    depth
}

/// Levels below the section: 0 without sub-sections.
fn subtree_height(session: &AppSession, section_id: Uuid) -> usize {
    session.agent_sections.iter()
        .filter(|section| section.parent_id == Some(section_id))
        .map(|section| 1 + subtree_height(session, section.id))
        .max().unwrap_or(0)
}

/// The section and every section inside it.
pub fn section_and_descendants(session: &AppSession, section_id: Uuid) -> Vec<Uuid> {
    let mut ids = vec![section_id];
    let mut index = 0;
    while index < ids.len() {
        let id = ids[index];
        let children: Vec<Uuid> = session.agent_sections.iter()
            .filter(|section| section.parent_id == Some(id) && !ids.contains(&section.id))
            .map(|section| section.id)
            .collect();
        ids.extend(children);
        index += 1;
    }
    ids
}

/// Names from the top of the list down to the section, such as `Reviews / Backend`.
pub fn section_path(session: &AppSession, section_id: Uuid) -> Option<String> {
    let mut names = Vec::new();
    let mut current = Some(section_id);
    while let Some(id) = current {
        let section = session.agent_sections.iter().find(|section| section.id == id)?;
        names.push(section.name.as_str());
        if names.len() > MAX_SECTIONS {
            break;
        }
        current = section.parent_id;
    }
    names.reverse();
    Some(names.join(" / "))
}

/// Sections in display order, each followed by its sub-sections, with their depth.
pub fn ordered_sections(session: &AppSession) -> Vec<(&AgentSection, usize)> {
    fn visit<'a>(session: &'a AppSession, parent: Option<Uuid>, depth: usize, out: &mut Vec<(&'a AgentSection, usize)>) {
        for section in session.agent_sections.iter().filter(|section| section.parent_id == parent) {
            if out.iter().any(|(listed, _)| listed.id == section.id) {
                continue;
            }
            out.push((section, depth));
            visit(session, Some(section.id), depth + 1, out);
        }
    }
    let mut out = Vec::with_capacity(session.agent_sections.len());
    visit(session, None, 0, &mut out);
    out
}

/// Repairs sections saved by other builds or edited by hand: unknown parents, cycles and
/// nesting deeper than allowed move up, and sessions of unknown sections leave them.
pub fn normalize_sections(session: &mut AppSession) {
    let ids: Vec<Uuid> = session.agent_sections.iter().map(|section| section.id).collect();
    for index in 0..session.agent_sections.len() {
        let id = session.agent_sections[index].id;
        if session.agent_sections[index].parent_id.is_some_and(|parent| parent == id || !ids.contains(&parent)) {
            session.agent_sections[index].parent_id = None;
        }
    }
    // A cycle never reaches the top: cut it at the first section found in it.
    for index in 0..session.agent_sections.len() {
        let id = session.agent_sections[index].id;
        let mut seen = vec![id];
        let mut current = session.agent_sections[index].parent_id;
        while let Some(parent) = current {
            if seen.contains(&parent) {
                session.agent_sections[index].parent_id = None;
                break;
            }
            seen.push(parent);
            current = parent_of(session, parent);
        }
    }
    for index in 0..session.agent_sections.len() {
        let id = session.agent_sections[index].id;
        if section_depth(session, id) >= MAX_SECTION_DEPTH {
            session.agent_sections[index].parent_id = None;
        }
    }
    for terminal in &mut session.terminals {
        if terminal.section_id.is_some_and(|id| !ids.contains(&id)) {
            terminal.section_id = None;
        }
    }
}

/// Moves a section inside `parent` (or to the top of the list), before its sibling `before`
/// or after the last one. A section never moves inside itself or its own sub-sections.
pub fn place_section(session: &mut AppSession, section_id: Uuid, parent: Option<Uuid>, before: Option<Uuid>) -> Result<()> {
    let Some(index) = session.agent_sections.iter().position(|section| section.id == section_id) else {
        bail!("Unknown section {section_id}");
    };
    if let Some(parent) = parent {
        if !session.agent_sections.iter().any(|section| section.id == parent) {
            bail!("Unknown section {parent}");
        }
        if section_and_descendants(session, section_id).contains(&parent) {
            bail!("A section cannot go inside itself or one of its sub-sections");
        }
        if section_depth(session, parent) + 1 + subtree_height(session, section_id) >= MAX_SECTION_DEPTH {
            bail!("Sections can be nested at most {MAX_SECTION_DEPTH} levels deep");
        }
    }
    let mut section = session.agent_sections.remove(index);
    section.parent_id = parent;
    // Siblings keep the order they have in the list; the section goes before `before`, or
    // after the last of its new siblings.
    let target = before
        .filter(|id| *id != section_id)
        .and_then(|id| session.agent_sections.iter().position(|other| other.id == id && other.parent_id == parent))
        .or_else(|| session.agent_sections.iter().rposition(|other| other.parent_id == parent).map(|index| index + 1))
        .unwrap_or(session.agent_sections.len());
    session.agent_sections.insert(target, section);
    Ok(())
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

/// Removes the section. Its sessions and sub-sections move to the section that contained
/// it, or to the top of the Agents list; nothing leaves the list.
pub fn delete_section(session: &mut AppSession, section_id: Uuid) {
    let parent = parent_of(session, section_id);
    session.agent_sections.retain(|section| section.id != section_id);
    for section in &mut session.agent_sections {
        if section.parent_id == Some(section_id) {
            section.parent_id = parent;
        }
    }
    for terminal in &mut session.terminals {
        if terminal.section_id == Some(section_id) {
            terminal.section_id = parent;
        }
    }
}

pub fn set_section_collapsed(session: &mut AppSession, section_id: Uuid, collapsed: bool) {
    if let Some(section) = session.agent_sections.iter_mut().find(|section| section.id == section_id) {
        section.collapsed = collapsed;
    }
}

pub fn set_all_sections_collapsed(session: &mut AppSession, collapsed: bool) {
    for section in &mut session.agent_sections {
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

/// A section by ID, or by name ignoring case.
pub fn resolve_section(session: &AppSession, reference: &str) -> Option<Uuid> {
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
        // Outer None: parent not given; Some(None): move to the top of the list.
        let parent = match change.parent.as_ref().map(|parent| parent.as_deref().map(str::trim).filter(|value| !value.is_empty())) {
            None => None,
            Some(None) => Some(None),
            Some(Some(reference)) => match resolve_section(&next, reference) {
                Some(id) => Some(Some(id)),
                None => bail!("Unknown parent section \"{reference}\""),
            },
        };
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
            (None, Some(name)) => create_section(&mut next, name, parent.flatten())?,
            (None, None) => bail!("Each section change needs an id or a name"),
        };
        if let Some(parent) = parent
            && parent_of(&next, id) != parent
        {
            place_section(&mut next, id, parent, None)?;
        }
        if let Some(description) = &change.description {
            set_section_description(&mut next, id, description.as_deref())?;
        }
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
                    None => create_section(&mut next, reference, None)?,
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
            listed_in_agents: false,
            last_active_at: None,
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
        let section = create_section(&mut state, "Release", None).unwrap();
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
        let section = create_section(&mut state, "Old", None).unwrap();
        set_session_section(&mut state, id, Some(section)).unwrap();
        organize(&mut state, &request(serde_json::json!({ "deleteSections": ["old"] })), None).unwrap();
        assert!(state.agent_sections.is_empty());
        assert_eq!(state.terminals.len(), 1);
        assert_eq!(state.terminals[0].section_id, None);
    }

    #[test]
    fn deleting_a_sub_section_moves_its_content_to_its_parent() {
        let agent = terminal(AgentKind::Claude);
        let id = agent.id;
        let mut state = session(vec![agent]);
        let reviews = create_section(&mut state, "Reviews", None).unwrap();
        let backend = create_section(&mut state, "Backend", Some(reviews)).unwrap();
        let mobile = create_section(&mut state, "Mobile", Some(backend)).unwrap();
        set_session_section(&mut state, id, Some(backend)).unwrap();
        delete_section(&mut state, backend);
        assert_eq!(state.terminals[0].section_id, Some(reviews));
        assert_eq!(state.agent_sections.iter().find(|section| section.id == mobile).unwrap().parent_id, Some(reviews));
    }

    #[test]
    fn sub_sections_nest_up_to_the_depth_limit_and_list_depth_first() {
        let mut state = session(Vec::new());
        let a = create_section(&mut state, "A", None).unwrap();
        let b = create_section(&mut state, "B", None).unwrap();
        let a1 = create_section(&mut state, "A1", Some(a)).unwrap();
        let a1x = create_section(&mut state, "A1x", Some(a1)).unwrap();
        assert!(create_section(&mut state, "Too deep", Some(a1x)).is_err());
        let order: Vec<_> = ordered_sections(&state).into_iter().map(|(section, depth)| (section.id, depth)).collect();
        assert_eq!(order, vec![(a, 0), (a1, 1), (a1x, 2), (b, 0)]);
        assert_eq!(section_path(&state, a1x).as_deref(), Some("A / A1 / A1x"));
        assert_eq!(section_and_descendants(&state, a), vec![a, a1, a1x]);
    }

    #[test]
    fn placing_a_section_reorders_siblings_and_never_creates_a_cycle() {
        let mut state = session(Vec::new());
        let a = create_section(&mut state, "A", None).unwrap();
        let b = create_section(&mut state, "B", None).unwrap();
        let c = create_section(&mut state, "C", None).unwrap();
        place_section(&mut state, c, None, Some(a)).unwrap();
        let top: Vec<_> = ordered_sections(&state).into_iter().map(|(section, _)| section.id).collect();
        assert_eq!(top, vec![c, a, b]);
        place_section(&mut state, b, Some(a), None).unwrap();
        assert!(place_section(&mut state, a, Some(b), None).is_err(), "a section cannot go inside its own sub-section");
        assert!(place_section(&mut state, a, Some(a), None).is_err());
        let b1 = create_section(&mut state, "B1", Some(b)).unwrap();
        // c > a > b > b1 would be four levels.
        assert!(place_section(&mut state, a, Some(c), None).is_err());
        place_section(&mut state, b1, None, None).unwrap();
        let order: Vec<_> = ordered_sections(&state).into_iter().map(|(section, depth)| (section.id, depth)).collect();
        assert_eq!(order, vec![(c, 0), (a, 0), (b, 1), (b1, 0)]);
    }

    #[test]
    fn organize_sets_descriptions_and_parents() {
        let mut state = session(Vec::new());
        organize(&mut state, &request(serde_json::json!({ "sections": [
            { "name": "Reviews", "description": "  Code reviews of   pull requests " },
            { "name": "Backend", "parent": "reviews", "description": "Backend PRs" },
        ] })), None).unwrap();
        let reviews = resolve_section(&state, "Reviews").unwrap();
        let backend = resolve_section(&state, "backend").unwrap();
        let section = |state: &AppSession, id: Uuid| state.agent_sections.iter().find(|section| section.id == id).unwrap().clone();
        assert_eq!(section(&state, reviews).description.as_deref(), Some("Code reviews of pull requests"));
        assert_eq!(section(&state, backend).parent_id, Some(reviews));
        organize(&mut state, &request(serde_json::json!({ "sections": [
            { "id": backend.to_string(), "parent": null, "description": null },
        ] })), None).unwrap();
        assert_eq!(section(&state, backend).parent_id, None);
        assert_eq!(section(&state, backend).description, None);
        assert!(organize(&mut state, &request(serde_json::json!({ "sections": [{ "name": "X", "parent": "missing" }] })), None).is_err());
    }

    #[test]
    fn normalize_repairs_unknown_parents_and_cycles() {
        let mut state = session(Vec::new());
        let a = create_section(&mut state, "A", None).unwrap();
        let b = create_section(&mut state, "B", Some(a)).unwrap();
        state.agent_sections[0].parent_id = Some(b);
        let c = create_section(&mut state, "C", None).unwrap();
        state.agent_sections[2].parent_id = Some(Uuid::new_v4());
        normalize_sections(&mut state);
        assert_eq!(ordered_sections(&state).len(), 3, "every section is reachable again");
        assert_eq!(state.agent_sections.iter().find(|section| section.id == c).unwrap().parent_id, None);
    }

    #[test]
    fn ages_read_in_the_largest_unit() {
        assert_eq!(describe_age(chrono::Duration::days(1)), "1 day");
        assert_eq!(describe_age(chrono::Duration::hours(30)), "1 day");
        assert_eq!(describe_age(chrono::Duration::hours(5)), "5 hours");
        assert_eq!(describe_age(chrono::Duration::minutes(-3)), "0 minutes");
    }

    #[test]
    fn section_order_lists_given_sections_first() {
        let mut state = session(Vec::new());
        let a = create_section(&mut state, "A", None).unwrap();
        let b = create_section(&mut state, "B", None).unwrap();
        let c = create_section(&mut state, "C", None).unwrap();
        organize(&mut state, &request(serde_json::json!({ "sectionOrder": ["c", a.to_string()] })), None).unwrap();
        let ids: Vec<_> = state.agent_sections.iter().map(|section| section.id).collect();
        assert_eq!(ids, vec![c, a, b]);
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
