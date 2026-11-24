//! Recent conversation of a Claude Code or Codex session, read from the transcript the
//! provider keeps on this computer. Tool calls, tool output, reasoning and context the CLI
//! injects are left out, so another agent can learn what the session did without its noise.
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

pub const MAX_MESSAGES: usize = 40;
pub const DEFAULT_MESSAGES: usize = 6;
pub const DEFAULT_MESSAGE_CHARS: usize = 1500;
pub const MAX_MESSAGE_CHARS: usize = 20_000;
const FIRST_CHUNK_BYTES: u64 = 1024 * 1024;
const MAX_READ_BYTES: u64 = 96 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Claude,
    Codex,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityMessage {
    pub role: &'static str,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// A prompt sent by an automation (such as a Codex heartbeat), not typed by the user.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub automated: bool,
}

/// What to return: whole turns (a prompt and the agent's last reply to it) or every message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Turns,
    Messages,
}

/// One user prompt and the agent's last reply before the next prompt. Agents write several
/// progress messages per turn; only the last is kept and the others are counted.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<ActivityMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<ActivityMessage>,
    #[serde(skip_serializing_if = "is_zero")]
    pub earlier_replies: usize,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub transcript_path: PathBuf,
    /// Title the provider gave the conversation, when it records one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_title: Option<String>,
    /// Oldest first; the last entry is the most recent turn.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<Turn>,
    /// Oldest first; the last entry is the most recent message.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<ActivityMessage>,
    /// True when older turns or messages exist beyond the ones returned.
    pub more_available: bool,
}

fn group_turns(messages: Vec<ActivityMessage>) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for message in messages {
        if message.role == "user" {
            turns.push(Turn { prompt: Some(message), reply: None, earlier_replies: 0 });
            continue;
        }
        if turns.is_empty() {
            turns.push(Turn { prompt: None, reply: None, earlier_replies: 0 });
        }
        let turn = turns.last_mut().expect("a turn exists");
        if turn.reply.replace(message).is_some() {
            turn.earlier_replies += 1;
        }
    }
    turns
}

/// Claude keeps `<home>/projects/<encoded cwd>/<session>.jsonl`; Codex keeps
/// `<home>/sessions/YYYY/MM/DD/rollout-<time>-<session>.jsonl` (or `archived_sessions`).
pub fn find_transcript(provider: Provider, session_id: &str, homes: &[PathBuf]) -> Option<PathBuf> {
    if session_id.is_empty() || session_id.contains(['/', '\\']) || session_id.contains("..") {
        return None;
    }
    match provider {
        Provider::Claude => {
            let file_name = format!("{session_id}.jsonl");
            homes.iter().find_map(|home| {
                std::fs::read_dir(home.join("projects")).ok()?.flatten()
                    .map(|entry| entry.path().join(&file_name))
                    .find(|path| path.is_file())
            })
        }
        Provider::Codex => {
            let suffix = format!("-{session_id}.jsonl");
            homes.iter().find_map(|home| {
                ["sessions", "archived_sessions"].iter()
                    .find_map(|folder| find_file(&home.join(folder), &suffix, 4))
            })
        }
    }
}

fn find_file(directory: &Path, suffix: &str, depth: usize) -> Option<PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(directory).ok()?.flatten().map(|entry| entry.path()).collect();
    // Dated folders sort by name; recent sessions are found first.
    entries.sort_unstable_by(|a, b| b.cmp(a));
    for path in entries {
        if path.is_dir() {
            if depth > 0 && let Some(found) = find_file(&path, suffix, depth - 1) {
                return Some(found);
            }
        } else if path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with(suffix)) {
            return Some(path);
        }
    }
    None
}

/// Reads the last `limit` turns or messages, starting from the end of the file so a long
/// session costs only the bytes it needs.
pub fn read_recent(provider: Provider, path: &Path, view: View, limit: usize, max_chars: usize) -> Result<Activity> {
    let limit = limit.clamp(1, MAX_MESSAGES);
    let max_chars = max_chars.clamp(100, MAX_MESSAGE_CHARS);
    let mut file = File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
    let size = file.metadata()?.len();
    let mut chunk = FIRST_CHUNK_BYTES.min(size);
    loop {
        let start = size - chunk;
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::with_capacity(chunk as usize);
        (&mut file).take(chunk).read_to_end(&mut bytes)?;
        let text = String::from_utf8_lossy(&bytes);
        // A chunk that starts mid-file begins with a partial line.
        let lines = if start == 0 { text.as_ref() } else { text.split_once('\n').map_or("", |(_, rest)| rest) };
        let (messages, provider_title) = parse(provider, lines);
        let whole_file = start == 0;
        let count = match view {
            // The first turn of a chunk may have lost its prompt; one extra keeps it complete.
            View::Turns => messages.iter().filter(|message| message.role == "user").count(),
            View::Messages => messages.len(),
        };
        if count > limit || whole_file || chunk >= MAX_READ_BYTES {
            let more_available = count > limit || (!whole_file && !messages.is_empty());
            let messages: Vec<_> = messages.into_iter().map(|message| truncate(message, max_chars)).collect();
            let (turns, messages) = match view {
                View::Turns => {
                    let turns = group_turns(messages);
                    let skip = turns.len().saturating_sub(limit);
                    (turns.into_iter().skip(skip).collect(), Vec::new())
                }
                View::Messages => {
                    let skip = messages.len().saturating_sub(limit);
                    (Vec::new(), messages.into_iter().skip(skip).collect())
                }
            };
            return Ok(Activity { transcript_path: path.to_path_buf(), provider_title, turns, messages, more_available });
        }
        chunk = (chunk * 4).min(size).min(MAX_READ_BYTES);
    }
}

fn truncate(mut message: ActivityMessage, max_chars: usize) -> ActivityMessage {
    if message.text.chars().count() > max_chars {
        message.text = message.text.chars().take(max_chars).collect::<String>() + "…";
        message.truncated = true;
    }
    message
}

fn parse(provider: Provider, lines: &str) -> (Vec<ActivityMessage>, Option<String>) {
    let mut messages = Vec::new();
    let mut title = None;
    for line in lines.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else { continue; };
        match provider {
            Provider::Claude => {
                if value["type"] == "ai-title" && let Some(text) = value["aiTitle"].as_str() {
                    title = Some(text.to_string());
                }
                messages.extend(claude_message(&value));
            }
            Provider::Codex => messages.extend(codex_message(&value)),
        }
    }
    (messages, title)
}

fn text_blocks(content: &Value, kinds: &[&str]) -> String {
    match content {
        Value::String(text) => text.trim().to_string(),
        Value::Array(blocks) => blocks.iter()
            .filter(|block| block["type"].as_str().is_some_and(|kind| kinds.contains(&kind)))
            .filter_map(|block| block["text"].as_str())
            .map(str::trim).filter(|text| !text.is_empty())
            .collect::<Vec<_>>().join("\n\n"),
        _ => String::new(),
    }
}

/// Wrappers Claude Code adds around slash commands, shell escapes, hooks and reminders.
const CLAUDE_INJECTED: &[&str] = &[
    "<command-name>", "<command-message>", "<command-args>", "<local-command-stdout>",
    "<local-command-stderr>", "<local-command-caveat>", "<system-reminder>", "<task-notification>",
    "<bash-input>", "<bash-stdout>", "<bash-stderr>", "<user-prompt-submit-hook>",
];

fn claude_message(value: &Value) -> Option<ActivityMessage> {
    let role = match value["type"].as_str()? {
        "user" => "user",
        "assistant" => "assistant",
        _ => return None,
    };
    // Subagent turns and CLI-generated messages are not part of the conversation.
    if value["isSidechain"] == true || value["isMeta"] == true {
        return None;
    }
    let text = text_blocks(&value["message"]["content"], &["text"]);
    if text.is_empty() || (role == "user" && CLAUDE_INJECTED.iter().any(|tag| text.starts_with(tag))) {
        return None;
    }
    Some(ActivityMessage {
        role, text, timestamp: value["timestamp"].as_str().map(str::to_string), truncated: false, automated: false,
    })
}

fn codex_message(value: &Value) -> Option<ActivityMessage> {
    let payload = &value["payload"];
    if value["type"] != "response_item" || payload["type"] != "message" {
        return None;
    }
    let role = match payload["role"].as_str()? {
        "user" => "user",
        "assistant" => "assistant",
        _ => return None,
    };
    let mut text = text_blocks(&payload["content"], &["input_text", "output_text"]);
    let mut automated = false;
    if role == "user" && text.starts_with("<heartbeat>") {
        // An automation run starts a turn; its instructions are the prompt.
        automated = true;
        text = text.split_once("<instructions>")
            .and_then(|(_, rest)| rest.split_once("</instructions>"))
            .map_or_else(String::new, |(instructions, _)| instructions.trim().to_string());
    } else if role == "user" && (text.starts_with('<') || text.starts_with("# AGENTS.md")) {
        // Codex sends environment context and instructions as tagged user messages.
        return None;
    }
    if text.is_empty() {
        return None;
    }
    Some(ActivityMessage {
        role, text, timestamp: value["timestamp"].as_str().map(str::to_string), truncated: false, automated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(lines: &[Value]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("blackholes-activity-{}.jsonl", uuid::Uuid::new_v4()));
        let mut file = File::create(&path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        path
    }

    #[test]
    fn claude_keeps_the_conversation_and_drops_tools_and_injected_text() {
        let path = write(&[
            serde_json::json!({"type":"ai-title","aiTitle":"Fix login"}),
            serde_json::json!({"type":"user","message":{"role":"user","content":"Fix the login bug"},"timestamp":"t1"}),
            serde_json::json!({"type":"assistant","message":{"content":[{"type":"thinking","thinking":"..."},{"type":"tool_use","name":"Bash"}]}}),
            serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}),
            serde_json::json!({"type":"user","isMeta":true,"message":{"content":"meta"}}),
            serde_json::json!({"type":"user","message":{"content":"<system-reminder>hidden</system-reminder>"}}),
            serde_json::json!({"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"subagent"}]}}),
            serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"Fixed it."}]},"timestamp":"t2"}),
        ]);
        let activity = read_recent(Provider::Claude, &path, View::Messages, 10, 1000).unwrap();
        assert_eq!(activity.provider_title.as_deref(), Some("Fix login"));
        let texts: Vec<_> = activity.messages.iter().map(|message| (message.role, message.text.as_str())).collect();
        assert_eq!(texts, vec![("user", "Fix the login bug"), ("assistant", "Fixed it.")]);
        assert!(!activity.more_available);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn codex_skips_environment_context_and_limits_from_the_end() {
        let message = |role: &str, kind: &str, text: &str| serde_json::json!({"type":"response_item","payload":{
            "type":"message","role":role,"content":[{"type":kind,"text":text}]}});
        let path = write(&[
            message("user", "input_text", "<environment_context>cwd</environment_context>"),
            message("user", "input_text", "<heartbeat><automation_id>a</automation_id><instructions> Check Slack </instructions></heartbeat>"),
            message("assistant", "output_text", "Nothing new"),
            message("developer", "input_text", "rules"),
            message("user", "input_text", "First"),
            message("assistant", "output_text", "One"),
            message("user", "input_text", "Second"),
            message("assistant", "output_text", "A long answer that will be cut"),
        ]);
        let activity = read_recent(Provider::Codex, &path, View::Messages, 2, 100).unwrap();
        let texts: Vec<_> = activity.messages.iter().map(|message| message.text.as_str()).collect();
        assert_eq!(texts, vec!["Second", "A long answer that will be cut"]);
        assert!(activity.more_available);
        let short = read_recent(Provider::Codex, &path, View::Messages, 1, 100).unwrap();
        assert!(!short.messages[0].truncated);
        let all = read_recent(Provider::Codex, &path, View::Turns, 10, 100).unwrap();
        let first = all.turns[0].prompt.as_ref().unwrap();
        assert!(first.automated);
        assert_eq!(first.text, "Check Slack");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn turns_keep_each_prompt_with_its_last_reply() {
        let user = |text: &str| serde_json::json!({"type":"user","message":{"content":text}});
        let assistant = |text: &str| serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":text}]}});
        let path = write(&[
            assistant("tail of an older turn"),
            user("Review PR 12"), assistant("Reading the diff"), assistant("Checking tests"), assistant("Two issues found"),
            user("Fix them"), assistant("Fixed both"),
        ]);
        let activity = read_recent(Provider::Claude, &path, View::Turns, 5, 1000).unwrap();
        assert!(activity.messages.is_empty());
        let turns: Vec<_> = activity.turns.iter().map(|turn| (
            turn.prompt.as_ref().map(|message| message.text.as_str()),
            turn.reply.as_ref().map(|message| message.text.as_str()),
            turn.earlier_replies,
        )).collect();
        assert_eq!(turns, vec![
            (None, Some("tail of an older turn"), 0),
            (Some("Review PR 12"), Some("Two issues found"), 2),
            (Some("Fix them"), Some("Fixed both"), 0),
        ]);
        let last = read_recent(Provider::Claude, &path, View::Turns, 1, 1000).unwrap();
        assert_eq!(last.turns.len(), 1);
        assert!(last.more_available);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn transcripts_are_found_by_session_id_only() {
        let home = std::env::temp_dir().join(format!("blackholes-homes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(home.join("projects/-tmp-project")).unwrap();
        std::fs::create_dir_all(home.join("sessions/2026/11/21")).unwrap();
        std::fs::write(home.join("projects/-tmp-project/abc.jsonl"), "").unwrap();
        std::fs::write(home.join("sessions/2026/11/21/rollout-2026-11-21T10-00-00-def.jsonl"), "").unwrap();
        let homes = vec![home.clone()];
        assert!(find_transcript(Provider::Claude, "abc", &homes).is_some());
        assert!(find_transcript(Provider::Codex, "def", &homes).is_some());
        assert!(find_transcript(Provider::Claude, "../abc", &homes).is_none());
        assert!(find_transcript(Provider::Codex, "missing", &homes).is_none());
        std::fs::remove_dir_all(home).unwrap();
    }
}
