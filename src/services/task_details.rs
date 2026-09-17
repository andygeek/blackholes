//! Shared, patchable task metadata for the desktop and MCP.
use super::{database::Database, tasks::TaskService};
use crate::model::{ProjectTask, Workspace, WorkspaceColor};
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
    #[serde(default, deserialize_with = "nullable")]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub acceptance_criteria: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub pull_request_url: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub external_task_url: Option<Option<String>>,
    pub color: Option<WorkspaceColor>,
    pub expected_revision: Option<String>,
}

pub fn metadata(task: &ProjectTask) -> Value {
    json!({ "title": task.title, "description": task.description,
        "acceptanceCriteria": task.acceptance_criteria, "pullRequestUrl": task.pull_request_url,
        "externalTaskUrl": task.external_task_url, "color": task.color })
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
                "Task details changed elsewhere. Reload the latest details before saving."
            );
        }
        if let Some(title) = &self.title {
            task.title =
                text(Some(title.clone()), 500, "Title")?.context("Task title cannot be empty")?;
        }
        if let Some(value) = &self.description {
            task.description = text(value.clone(), 100_000, "Description")?;
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
        ensure!(task.workspace_id == workspace.id, "Task project changed");
        let previous = task.clone();
        patch.apply(task)?;
        if let Err(error) = service.repair_task_files(workspace, task) {
            let _ = service.repair_task_files(workspace, &previous);
            bail!("Could not write task context: {error:#}");
        }
        Ok(())
    })
}

pub fn markdown(task: &ProjectTask) -> String {
    let mut content = format!(
        "# {}\n\nTask ID: {}\n\n",
        task.title.replace(['\r', '\n'], " "),
        task.id
    );
    for (heading, value) in [
        ("Objective and description", task.description.as_deref()),
        ("Acceptance criteria", task.acceptance_criteria.as_deref()),
        ("Pull request", task.pull_request_url.as_deref()),
        ("External task", task.external_task_url.as_deref()),
    ] {
        if let Some(value) = value {
            content.push_str(&format!("## {heading}\n\n{value}\n\n"));
        }
    }
    content.push_str("Update these fields through the Blackholes task details or MCP update_task. This file is generated.\n");
    content
}
