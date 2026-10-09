//! Requests shared by the MCP and the desktop's acknowledged terminal launcher.
use crate::model::{AgentKind, ProjectTask};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::PathBuf};
use uuid::Uuid;

pub const MAX_TASK_AGENTS: usize = 8;
pub const MAX_TASK_PROMPT_CHARS: usize = 16_000;
pub const MAX_MODEL_CHARS: usize = 100;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskAgentRequest {
    pub task_id: Uuid,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub agent: Option<AgentKind>,
    /// Passed to the provider CLI as `--model`; omitted uses its configured default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartTaskAgentsRequest {
    pub tasks: Vec<TaskAgentRequest>,
    pub agent: Option<AgentKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub source_agent: Option<AgentKind>,
    pub source_terminal_id: Option<Uuid>,
    pub source_config_dir: Option<PathBuf>,
}

impl StartTaskAgentsRequest {
    pub fn validate(&self) -> Result<()> {
        if self.tasks.is_empty() || self.tasks.len() > MAX_TASK_AGENTS {
            bail!("Provide between 1 and {MAX_TASK_AGENTS} tasks");
        }
        let mut ids = HashSet::new();
        for task in &self.tasks {
            if !ids.insert(task.task_id) {
                bail!("A task can only appear once in a launch request");
            }
            if let Some(prompt) = &task.prompt {
                if prompt.trim().is_empty()
                    || prompt.contains('\0')
                    || prompt.chars().count() > MAX_TASK_PROMPT_CHARS
                {
                    bail!(
                        "Task prompts must be non-empty, contain no NUL, and be at most {MAX_TASK_PROMPT_CHARS} characters"
                    );
                }
            }
            if task.agent == Some(AgentKind::Shell) {
                bail!("Select a coding agent, not shell");
            }
            if let Some(model) = &task.model {
                validate_model(model)?;
            }
        }
        if self.agent == Some(AgentKind::Shell) {
            bail!("Select a coding agent, not shell");
        }
        if let Some(model) = &self.model {
            validate_model(model)?;
        }
        Ok(())
    }
}

/// A model is one CLI argument value, never another option.
fn validate_model(model: &str) -> Result<()> {
    let valid = !model.is_empty()
        && model.chars().count() <= MAX_MODEL_CHARS
        && !model.starts_with('-')
        && model.chars().all(|character| character.is_ascii_alphanumeric() || "-._:/@[]".contains(character));
    if !valid {
        bail!(
            "Models must be a provider model name such as claude-opus-5-5: at most {MAX_MODEL_CHARS} letters, digits or -._:/@[] characters, not starting with -"
        );
    }
    Ok(())
}

pub fn detect_agent(name: &str) -> Option<AgentKind> {
    let name = name.to_ascii_lowercase();
    if name.contains("codex") {
        Some(AgentKind::Codex)
    } else if name.contains("antigravity") || name == "agy" {
        Some(AgentKind::Antigravity)
    } else if name.contains("claude") {
        Some(AgentKind::Claude)
    } else if name.contains("gemini") {
        Some(AgentKind::Gemini)
    } else if name.contains("opencode") || name.contains("open-code") {
        Some(AgentKind::OpenCode)
    } else {
        None
    }
}

pub fn implementation_prompt(task: &ProjectTask, brief: Option<&str>) -> Result<String> {
    let brief = brief.unwrap_or(task.description.as_deref().filter(|value| value.chars().count() <= MAX_TASK_PROMPT_CHARS).unwrap_or(&task.title));
    if brief.contains('\0') || brief.chars().count() > MAX_TASK_PROMPT_CHARS {
        bail!("The task brief is too large or contains NUL; pass a shorter prompt");
    }
    Ok(format!(
        "Implement the existing Blackholes task \"{}\" (task id {}, project id {}).\n\
         You are its assigned terminal agent. The task and its worktrees already exist. \
         Do not create this task again or delegate it to another agent.\n\
         Read .blackholes-task-details.md, any legacy .blackholes-note.md, and AGENTS.md/CLAUDE.md in this workspace, then the \
         attached repositories' instructions. Use get_current_context and get_task to \
         confirm the task objective, acceptance criteria and links. Use update_task to keep those fields current. Work only in its attached worktrees. Preserve all user constraints, \
         including restrictions on tests, servers, and remote writes.\n\n\
         Implementation brief:\n{}\n\n\
         Show progress and the final result in this terminal. Do not send a completion \
         notification unless the user explicitly asks for one.",
        task.title, task.id, task.workspace_id, brief,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(model: Option<&str>, batch_model: Option<&str>) -> StartTaskAgentsRequest {
        StartTaskAgentsRequest {
            tasks: vec![TaskAgentRequest {
                task_id: Uuid::new_v4(), prompt: None, agent: None, model: model.map(Into::into),
            }],
            agent: None, model: batch_model.map(Into::into),
            source_agent: None, source_terminal_id: None, source_config_dir: None,
        }
    }

    #[test]
    fn models_are_single_safe_cli_values() {
        for model in ["claude-opus-5-5", "opus", "claude-opus-5-5[1m]", "anthropic/claude-opus-5-5", "gpt-5.1-codex"] {
            assert!(request(Some(model), None).validate().is_ok(), "{model}");
            assert!(request(None, Some(model)).validate().is_ok(), "{model}");
        }
        for model in ["", "--dangerously-skip-permissions", "opus 5", "opus;rm", "opus\n", &"m".repeat(MAX_MODEL_CHARS + 1)] {
            assert!(request(Some(model), None).validate().is_err(), "{model:?}");
            assert!(request(None, Some(model)).validate().is_err(), "{model:?}");
        }
    }

    #[test]
    fn omitted_model_is_not_sent() {
        let value = serde_json::to_value(&request(None, None)).unwrap();
        assert!(value.get("model").is_none());
        assert!(value["tasks"][0].get("model").is_none());
    }
}
