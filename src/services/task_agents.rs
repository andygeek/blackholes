//! Requests shared by the MCP and the desktop's acknowledged terminal launcher.
use crate::model::{AgentKind, ProjectTask};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::PathBuf};
use uuid::Uuid;

pub const MAX_TASK_AGENTS: usize = 8;
pub const MAX_TASK_PROMPT_CHARS: usize = 16_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskAgentRequest {
    pub task_id: Uuid,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub agent: Option<AgentKind>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartTaskAgentsRequest {
    pub tasks: Vec<TaskAgentRequest>,
    pub agent: Option<AgentKind>,
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
        }
        if self.agent == Some(AgentKind::Shell) {
            bail!("Select a coding agent, not shell");
        }
        Ok(())
    }
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
