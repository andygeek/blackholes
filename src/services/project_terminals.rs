//! Requests shared by the MCP and the desktop for project terminals outside tasks.
use crate::model::AgentKind;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// The command is typed at the prompt before the shell reads it, so it must fit
// in the terminal's canonical input line (1024 bytes on macOS).
pub const MAX_TERMINAL_COMMAND_BYTES: usize = 1000;
pub const MAX_TERMINAL_TITLE_CHARS: usize = 80;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenProjectTerminalRequest {
    pub project_id: Uuid,
    #[serde(default)]
    pub repository_id: Option<Uuid>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub reveal: bool,
}

impl OpenProjectTerminalRequest {
    pub fn validate(&self) -> Result<()> {
        if let Some(command) = &self.command {
            if command.trim().is_empty() || command.chars().any(char::is_control) || command.len() > MAX_TERMINAL_COMMAND_BYTES {
                bail!("The command must be a single non-empty line without control characters, at most {MAX_TERMINAL_COMMAND_BYTES} bytes");
            }
        }
        if let Some(title) = &self.title {
            if title.trim().is_empty() || title.chars().any(char::is_control) || title.chars().count() > MAX_TERMINAL_TITLE_CHARS {
                bail!("The title must be non-empty, without control characters, and at most {MAX_TERMINAL_TITLE_CHARS} characters");
            }
        }
        Ok(())
    }

    pub fn command(&self) -> Option<&str> {
        self.command.as_deref().map(str::trim)
    }

    /// Explicit title, else the command itself, else the regular terminal label.
    pub fn label(&self) -> String {
        let label = self.title.as_deref().map(str::trim).or(self.command());
        match label {
            Some(label) if label.chars().count() > 40 => format!("{}…", label.chars().take(39).collect::<String>()),
            Some(label) => label.to_owned(),
            None => AgentKind::Shell.label().to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(command: Option<&str>, title: Option<&str>) -> OpenProjectTerminalRequest {
        OpenProjectTerminalRequest {
            project_id: Uuid::nil(),
            repository_id: None,
            command: command.map(str::to_owned),
            title: title.map(str::to_owned),
            reveal: false,
        }
    }

    #[test]
    fn accepts_plain_shells_and_single_line_commands() {
        assert!(request(None, None).validate().is_ok());
        assert!(request(Some("pnpm dev --port 3001"), Some("Web")).validate().is_ok());
    }

    #[test]
    fn rejects_multiline_blank_or_oversized_commands() {
        assert!(request(Some("pnpm dev\nrm -rf ."), None).validate().is_err());
        assert!(request(Some("   "), None).validate().is_err());
        assert!(request(Some(&"x".repeat(MAX_TERMINAL_COMMAND_BYTES + 1)), None).validate().is_err());
        assert!(request(None, Some("")).validate().is_err());
    }

    #[test]
    fn labels_prefer_the_title_then_the_command() {
        assert_eq!(request(Some("pnpm dev"), Some(" Web ")).label(), "Web");
        assert_eq!(request(Some(" pnpm dev "), None).label(), "pnpm dev");
        assert_eq!(request(None, None).label(), "Terminal");
        assert_eq!(request(Some(&"a".repeat(50)), None).label().chars().count(), 40);
    }

    #[test]
    fn rejects_unknown_fields() {
        let value = serde_json::json!({ "projectId": Uuid::nil(), "cmd": "pnpm dev" });
        assert!(serde_json::from_value::<OpenProjectTerminalRequest>(value).is_err());
    }
}
