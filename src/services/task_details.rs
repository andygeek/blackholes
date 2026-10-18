//! Shared, patchable task metadata for the desktop and MCP.
use super::{database::Database, tasks::TaskService};
use crate::model::{ProjectTask, Workspace, WorkspaceColor, WorktreeGroupLink, WorktreeGroupLinkKind};
use anyhow::{Context, Result, bail, ensure};
use chrono::Utc;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use std::hash::{Hash, Hasher};
use uuid::Uuid;

pub const DETAILS_FILE_NAME: &str = ".blackholes-task-details.md";

fn nullable<'de, D, T>(deserializer: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskDetailsPatch {
    pub title: Option<String>,
    pub icon: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub acceptance_criteria: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub pull_request_url: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub external_task_url: Option<Option<String>>,
    /// Replaces the entire link collection. An empty array removes all links.
    pub links: Option<Vec<WorktreeGroupLink>>,
    pub color: Option<WorkspaceColor>,
    pub expected_revision: Option<String>,
}

pub fn metadata(task: &ProjectTask) -> Value {
    json!({ "title": task.title, "icon": task.icon, "description": document(task), "summary": summary(task), "links": links(task),
        "acceptanceCriteria": task.acceptance_criteria, "pullRequestUrl": task.pull_request_url,
        "externalTaskUrl": task.external_task_url, "color": task.color })
}

pub fn icon(value: &str) -> Result<String> {
    let value = value.trim();
    if ["layers", "folder", "code-2", "square-terminal", "rocket", "database", "globe", "list-todo", "git-branch"].contains(&value) {
        return Ok(value.to_string());
    }
    if let Some(symbol) = value.strip_prefix("emoji:") {
        // Store text, never user-supplied SVG/HTML or remote image URLs.
        ensure!(!symbol.is_empty() && symbol.chars().count() <= 32
            && !symbol.chars().any(|character| character.is_control() || character.is_whitespace())
            && symbol.chars().any(|character| !character.is_ascii()), "Choose an emoji or a supported icon");
        return Ok(value.to_string());
    }
    bail!("Choose an emoji or a supported icon")
}

/// Present legacy acceptance criteria in the document until its next rewrite.
/// Reading old data never mutates the database or drops user content.
pub fn document(task: &ProjectTask) -> Option<String> {
    let mut content = task.description.clone().unwrap_or_default();
    if let Some(criteria) = task.acceptance_criteria.as_deref().filter(|value| !value.trim().is_empty()) {
        if !content.trim().is_empty() { content.push_str("\n\n"); }
        content.push_str("## Acceptance criteria\n\n");
        content.push_str(criteria);
    }
    (!content.trim().is_empty()).then_some(content)
}

/// The first paragraph is the summary, keeping a single editable source.
pub fn summary(task: &ProjectTask) -> String {
    let content = document(task).unwrap_or_default();
    let paragraph = content.split("\n\n").find(|part| !part.trim().is_empty()).unwrap_or("");
    let paragraph = paragraph.trim().trim_start_matches('#').split_whitespace().collect::<Vec<_>>().join(" ");
    let mut characters = paragraph.chars();
    let mut summary: String = characters.by_ref().take(320).collect();
    if characters.next().is_some() { summary.push('…'); }
    summary
}

pub fn links(task: &ProjectTask) -> Vec<WorktreeGroupLink> {
    let mut links = task.links.clone();
    for (url, kind) in [
        (&task.pull_request_url, WorktreeGroupLinkKind::PullRequest),
        (&task.external_task_url, WorktreeGroupLinkKind::Context),
    ] {
        if let Some(url) = url.as_ref().filter(|url| !url.trim().is_empty()) {
            if !links.iter().any(|entry| entry.url.trim_end_matches('/') == url.trim_end_matches('/')) {
                links.push(WorktreeGroupLink { url: url.clone(), label: None, kind });
            }
        }
    }
    links
}

pub fn validate_links(values: Vec<WorktreeGroupLink>) -> Result<Vec<WorktreeGroupLink>> {
    ensure!(values.len() <= 64, "A worktree group supports at most 64 links");
    let mut links = Vec::<WorktreeGroupLink>::new();
    for value in values {
        let url = link(Some(value.url), "Link")?.context("Link URL cannot be empty")?;
        let label = text(value.label, 200, "Link label")?;
        if !links.iter().any(|entry| entry.url == url) {
            links.push(WorktreeGroupLink { url, label, kind: value.kind });
        }
    }
    Ok(links)
}

pub fn revision(task: &ProjectTask) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    metadata(task).to_string().hash(&mut hash);
    format!("{:016x}", hash.finish())
}

pub(super) fn text(value: Option<String>, limit: usize, field: &str) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    ensure!(
        !value.contains('\0') && value.chars().count() <= limit,
        "{field} is too long or contains NUL"
    );
    let value = value.trim().to_string();
    Ok((!value.is_empty()).then_some(value))
}

pub fn link(value: Option<String>, field: &str) -> Result<Option<String>> {
    let Some(value) = text(value, 4096, field)? else {
        return Ok(None);
    };
    let parsed = url::Url::parse(&value)
        .with_context(|| format!("{field} must be a complete HTTP or HTTPS URL"))?;
    ensure!(
        matches!(parsed.scheme(), "http" | "https")
            && parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && !value.chars().any(char::is_control),
        "{field} must be an HTTP or HTTPS URL without credentials"
    );
    Ok(Some(parsed.to_string()))
}

impl TaskDetailsPatch {
    pub fn apply(&self, task: &mut ProjectTask) -> Result<()> {
        if let Some(expected) = &self.expected_revision {
            ensure!(
                *expected == revision(task),
                "Worktree group changed elsewhere. Reload the latest context before saving."
            );
        }
        if let Some(title) = &self.title {
            task.title =
                text(Some(title.clone()), 500, "Title")?.context("Worktree group title cannot be empty")?;
        }
        if let Some(value) = &self.icon {
            task.icon = icon(value)?;
        }
        if let Some(value) = &self.description {
            task.description = text(value.clone(), 200_100, "Description")?;
        }
        if let Some(value) = &self.acceptance_criteria {
            task.acceptance_criteria = text(value.clone(), 100_000, "Acceptance criteria")?;
        }
        if let Some(value) = &self.pull_request_url {
            task.pull_request_url = link(value.clone(), "Pull request link")?;
        }
        if let Some(value) = &self.external_task_url {
            task.external_task_url = link(value.clone(), "External task link")?;
        }
        if let Some(values) = &self.links {
            task.links = validate_links(values.clone())?;
            task.pull_request_url = None;
            task.external_task_url = None;
        }
        if let Some(color) = self.color {
            task.color = color;
        }
        task.updated_at = Utc::now();
        Ok(())
    }
}

pub fn update(
    database: &Database,
    workspace: &Workspace,
    service: &TaskService,
    task_id: Uuid,
    patch: &TaskDetailsPatch,
) -> Result<ProjectTask> {
    database.edit_task_metadata(task_id, |task| {
        ensure!(task.workspace_id == workspace.id, "Worktree group project changed");
        let previous = task.clone();
        patch.apply(task)?;
        if let Err(error) = service.repair_task_files(workspace, task) {
            let _ = service.repair_task_files(workspace, &previous);
            bail!("Could not write worktree group context: {error:#}");
        }
        Ok(())
    })
}

pub fn markdown(task: &ProjectTask) -> String {
    let mut content = format!(
        "# {}\n\nWorktree Group ID: {}\n\n",
        task.title.replace(['\r', '\n'], " "),
        task.id
    );
    if let Some(document) = document(task) {
        content.push_str(&document);
        content.push_str("\n\n");
    }
    for (heading, kind) in [("Pull requests", WorktreeGroupLinkKind::PullRequest), ("Context links", WorktreeGroupLinkKind::Context)] {
        let entries: Vec<_> = links(task).into_iter().filter(|entry| entry.kind == kind).collect();
        if !entries.is_empty() {
            content.push_str(&format!("## {heading}\n\n"));
            for entry in entries {
                // Plain text avoids treating user-provided labels as Markdown.
                content.push_str(&format!("- {}{}\n", entry.label.map(|label| format!("{}: ", label.replace(['\r', '\n'], " "))).unwrap_or_default(), entry.url));
            }
            content.push('\n');
        }
    }
    content.push_str("This file is generated. Use MCP get_worktree_group / update_worktree_group to maintain the current description and links. Start with a short summary paragraph. Rewrite stale context after meaningful changes and before finishing; preserve requirements, constraints and unresolved work. Never append progress logs, dated entries or a history of actions.\n");
    content
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_group() -> ProjectTask {
        let now = Utc::now();
        ProjectTask {
            id: Uuid::new_v4(), workspace_id: Uuid::new_v4(), title: "Current work".into(),
            description: Some("A concise summary.".into()), acceptance_criteria: Some("Preserve the CSV format.".into()),
            pull_request_url: Some("https://github.com/example/api/pull/1".into()),
            external_task_url: Some("https://app.clickup.com/t/example".into()), links: vec![],
            icon: "layers".into(), color: WorkspaceColor::Slate, sort_order: 0,
            worktree_root_path: std::env::temp_dir().join(format!("blackholes-uncreated-{}", Uuid::new_v4())),
            repositories: vec![], sessions: vec![], created_at: now, updated_at: now,
        }
    }

    #[test]
    fn legacy_fields_are_visible_without_mutating_stored_content() {
        let task = legacy_group();
        assert_eq!(summary(&task), "A concise summary.");
        assert!(document(&task).unwrap().contains("Preserve the CSV format."));
        assert_eq!(links(&task).len(), 2);
        assert_eq!(links(&task)[0].kind, WorktreeGroupLinkKind::PullRequest);
        assert_eq!(task.description.as_deref(), Some("A concise summary."));
        assert!(markdown(&task).contains("update_worktree_group"));
    }

    #[test]
    fn document_and_links_can_replace_and_clear_legacy_fields() {
        let mut task = legacy_group();
        let revision = revision(&task);
        let patch: TaskDetailsPatch = serde_json::from_value(json!({
            "description": "Current summary.\n\nPreserve the CSV format.", "acceptanceCriteria": null,
            "links": [
                {"url":"https://github.com/example/api/pull/2","kind":"pull_request"},
                {"url":"https://github.com/example/web/pull/3","kind":"pull_request"},
                {"url":"https://example.slack.com/archives/CHANNEL/p123","label":"Discussion"}
            ], "expectedRevision": revision
        })).unwrap();
        patch.apply(&mut task).unwrap();
        assert!(task.acceptance_criteria.is_none());
        assert!(task.pull_request_url.is_none() && task.external_task_url.is_none());
        assert_eq!(links(&task).len(), 3);
        let clear: TaskDetailsPatch = serde_json::from_value(json!({"links":[]})).unwrap();
        clear.apply(&mut task).unwrap();
        assert!(links(&task).is_empty());
        assert!(patch.apply(&mut task).is_err(), "stale revisions must fail");
    }

    #[test]
    fn old_description_patches_preserve_legacy_criteria() {
        let mut task = legacy_group();
        let patch: TaskDetailsPatch = serde_json::from_value(json!({"description":"Updated objective"})).unwrap();
        patch.apply(&mut task).unwrap();
        assert_eq!(task.acceptance_criteria.as_deref(), Some("Preserve the CSV format."));
    }

    #[test]
    fn group_icons_are_text_or_bundled_icons_and_part_of_the_revision() {
        let mut task = legacy_group();
        let original = revision(&task);
        for value in ["layers", "rocket", "emoji:😀", "emoji:👩‍💻", "emoji:🇵🇪", "emoji:❤️"] {
            assert!(icon(value).is_ok());
        }
        for value in ["<svg/>", "https://example.com/icon.svg", "emoji:", "emoji:hello", "emoji:😀\nhello", "emoji:😀\0"] {
            assert!(icon(value).is_err());
        }
        let patch: TaskDetailsPatch = serde_json::from_value(json!({"icon":"emoji:🚀","expectedRevision":original})).unwrap();
        patch.apply(&mut task).unwrap();
        assert_eq!(metadata(&task)["icon"], "emoji:🚀");
        assert_eq!(task.title, "Current work");
        assert_ne!(revision(&task), original);
        assert!(patch.apply(&mut task).is_err());
    }

    #[test]
    fn links_reject_unsafe_urls_and_deduplicate_normalized_urls() {
        for url in ["javascript:alert(1)", "file:///tmp/context", "https://user:secret@example.com/", ""] {
            assert!(validate_links(vec![WorktreeGroupLink { url: url.into(), label: None, kind: WorktreeGroupLinkKind::Context }]).is_err());
        }
        let values = serde_json::from_value(json!([{"url":"https://example.com"},{"url":"https://example.com/"}])).unwrap();
        assert_eq!(validate_links(values).unwrap().len(), 1);
        let values = vec![WorktreeGroupLink { url: "https://example.com".into(), label: None, kind: WorktreeGroupLinkKind::Context }; 65];
        assert!(validate_links(values).is_err());
    }

    #[test]
    fn summary_handles_multibyte_characters_without_truncating_the_document() {
        let mut task = legacy_group();
        task.description = Some(format!("{}\n\nCurrent details.", "á".repeat(350)));
        assert_eq!(summary(&task).chars().count(), 321);
        assert!(summary(&task).ends_with('…'));
        assert!(document(&task).unwrap().contains("Current details."));
    }
}
