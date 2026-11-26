//! Conversation of a Claude Code or Codex session, read from the transcript the provider
//! keeps on this computer. Tool calls, tool output, reasoning and context the CLI injects
//! are left out, so another agent can learn what the session did without its noise.
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::SystemTime,
};

pub const MAX_MESSAGES: usize = 40;
pub const DEFAULT_MESSAGES: usize = 6;
pub const DEFAULT_MESSAGE_CHARS: usize = 1500;
pub const MAX_MESSAGE_CHARS: usize = 20_000;
/// Characters kept from the first and last message of a summary.
pub const EXCERPT_CHARS: usize = 300;
/// Files listed per session; the most recently touched come first.
pub const MAX_FILES: usize = 200;
const FIRST_CHUNK_BYTES: u64 = 1024 * 1024;
const MAX_READ_BYTES: u64 = 96 * 1024 * 1024;
/// A summary reads at most this much from the end of a transcript.
const MAX_SCAN_BYTES: u64 = 256 * 1024 * 1024;
/// A search reads at most this much from the end of each transcript.
pub const MAX_SEARCH_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Where the provider records a session's working folder, at the start of the file.
const HEAD_BYTES: u64 = 2 * 1024 * 1024;
/// Older Codex sessions are compressed; reading them would need a zstd decoder.
const COMPRESSED_SUFFIX: &str = ".jsonl.zst";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
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
    /// Byte offset of the transcript line holding the message.
    #[serde(skip)]
    pub offset: u64,
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
    /// Byte offset where the turn's first message starts.
    #[serde(skip)]
    start: u64,
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
    /// Pass back as `before` to read the page of older entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub older_cursor: Option<String>,
}

fn group_turns(messages: Vec<ActivityMessage>) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for message in messages {
        if message.role == "user" {
            let start = message.offset;
            turns.push(Turn { prompt: Some(message), reply: None, earlier_replies: 0, start });
            continue;
        }
        if turns.is_empty() {
            turns.push(Turn { prompt: None, reply: None, earlier_replies: 0, start: message.offset });
        }
        let turn = turns.last_mut().expect("a turn exists");
        if turn.reply.replace(message).is_some() {
            turn.earlier_replies += 1;
        }
    }
    turns
}

fn valid_session_id(session_id: &str) -> bool {
    !session_id.is_empty() && session_id.len() <= 200
        && !session_id.contains(['/', '\\']) && !session_id.contains("..")
}

/// Claude keeps `<home>/projects/<encoded cwd>/<session>.jsonl`; Codex keeps
/// `<home>/sessions/YYYY/MM/DD/rollout-<time>-<session>.jsonl` (or `archived_sessions`),
/// compressed as `.jsonl.zst` once it is old.
pub fn find_transcript(provider: Provider, session_id: &str, homes: &[PathBuf]) -> Option<PathBuf> {
    if !valid_session_id(session_id) {
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
            // A readable copy wins over a compressed one in another home.
            [format!("-{session_id}.jsonl"), format!("-{session_id}{COMPRESSED_SUFFIX}")].iter().find_map(|suffix| {
                homes.iter().find_map(|home| {
                    ["sessions", "archived_sessions"].iter()
                        .find_map(|folder| find_file(&home.join(folder), suffix, 4))
                })
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

pub fn is_compressed(path: &Path) -> bool {
    path.to_string_lossy().ends_with(COMPRESSED_SUFFIX)
}

fn ensure_readable(path: &Path) -> Result<()> {
    if is_compressed(path) {
        bail!("Codex compressed this older transcript ({}); Blackholes cannot read compressed transcripts yet", path.display());
    }
    Ok(())
}

/// A cursor is the byte offset where the oldest returned entry starts.
pub fn parse_cursor(cursor: &str) -> Result<u64> {
    cursor.trim().parse().ok().context("before must be an olderCursor returned by get_agent_session_activity")
}

/// Reads `[start, end)` of the file. When `start` is inside the file, one earlier byte is
/// read too, so the line iterator can tell whether `start` begins a line.
fn read_window(file: &mut File, start: u64, end: u64) -> Result<(Vec<u8>, u64)> {
    let from = start.saturating_sub(1);
    file.seek(SeekFrom::Start(from))?;
    let mut bytes = Vec::with_capacity((end - from) as usize);
    (&mut *file).take(end - from).read_to_end(&mut bytes)?;
    Ok((bytes, from))
}

/// Lines of `bytes` with the file offset where each starts. `base` is the offset of
/// `bytes[0]`; when it is not the start of the file, everything up to the first newline
/// belongs to a line that started earlier and is skipped.
fn lines(bytes: &[u8], base: u64) -> impl Iterator<Item = (u64, &[u8])> {
    let mut position = if base == 0 { 0 } else {
        bytes.iter().position(|byte| *byte == b'\n').map_or(bytes.len(), |index| index + 1)
    };
    std::iter::from_fn(move || {
        while position < bytes.len() {
            let start = position;
            let end = bytes[start..].iter().position(|byte| *byte == b'\n').map_or(bytes.len(), |index| start + index);
            position = end + 1;
            if end > start {
                return Some((base + start as u64, &bytes[start..end]));
            }
        }
        None
    })
}

/// Calls `visit` for each line in `[start, end)`, streaming instead of loading the range.
fn stream_lines(file: &mut File, start: u64, end: u64, mut visit: impl FnMut(u64, &[u8]) -> bool) -> Result<()> {
    let from = start.saturating_sub(1);
    file.seek(SeekFrom::Start(from))?;
    let mut reader = BufReader::with_capacity(1024 * 1024, (&mut *file).take(end - from));
    let mut offset = from;
    let mut line = Vec::new();
    let mut first = start > 0;
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            return Ok(());
        }
        let line_start = offset;
        offset += read as u64;
        if std::mem::take(&mut first) {
            continue;
        }
        let content = line.strip_suffix(b"\n").unwrap_or(&line);
        if !content.is_empty() && !visit(line_start, content) {
            return Ok(());
        }
    }
}

/// Reads the last `limit` turns or messages before `before` (the whole file when `None`),
/// starting from the end so a long session costs only the bytes it needs.
pub fn read_recent(
    provider: Provider, path: &Path, view: View, limit: usize, max_chars: usize, before: Option<u64>,
) -> Result<Activity> {
    ensure_readable(path)?;
    let limit = limit.clamp(1, MAX_MESSAGES);
    let max_chars = max_chars.clamp(100, MAX_MESSAGE_CHARS);
    let mut file = File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
    let file_size = file.metadata()?.len();
    let size = match before {
        Some(before) if before > file_size => bail!("The cursor is past the end of the transcript; it changed since it was read"),
        Some(before) => before,
        None => file_size,
    };
    let mut chunk = FIRST_CHUNK_BYTES.min(size);
    loop {
        let start = size - chunk;
        let (bytes, base) = read_window(&mut file, start, size)?;
        let mut messages = Vec::new();
        let mut provider_title = None;
        for (offset, line) in lines(&bytes, base) {
            let info = line_info(provider, line, offset);
            provider_title = info.title.or(provider_title);
            messages.extend(info.message);
        }
        let whole_file = start == 0;
        let count = match view {
            // The first turn of a chunk may have lost its prompt; one extra keeps it complete.
            View::Turns => messages.iter().filter(|message| message.role == "user").count(),
            View::Messages => messages.len(),
        };
        if count > limit || whole_file || chunk >= MAX_READ_BYTES {
            let more_available = count > limit || (!whole_file && !messages.is_empty());
            let messages: Vec<_> = messages.into_iter().map(|message| truncate(message, max_chars)).collect();
            let (turns, messages, oldest) = match view {
                View::Turns => {
                    let turns = group_turns(messages);
                    let skip = turns.len().saturating_sub(limit);
                    let turns: Vec<_> = turns.into_iter().skip(skip).collect();
                    let oldest = turns.first().map(|turn| turn.start);
                    (turns, Vec::new(), oldest)
                }
                View::Messages => {
                    let skip = messages.len().saturating_sub(limit);
                    let messages: Vec<_> = messages.into_iter().skip(skip).collect();
                    let oldest = messages.first().map(|message| message.offset);
                    (Vec::new(), messages, oldest)
                }
            };
            let older_cursor = oldest.filter(|_| more_available).map(|offset| offset.to_string());
            return Ok(Activity { transcript_path: path.to_path_buf(), provider_title, turns, messages, more_available, older_cursor });
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

/// How a session touched a file, from its edit tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileAction {
    Created,
    Written,
    Edited,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTouch {
    pub path: String,
    pub actions: Vec<FileAction>,
    pub count: usize,
}

/// What one transcript line contributes.
#[derive(Default)]
struct LineInfo {
    message: Option<ActivityMessage>,
    title: Option<String>,
    cwd: Option<String>,
    /// A prompt or a tool result: either answers a pending question.
    answers: bool,
    /// Claude asked through its question tool and waits for the answer.
    asks_user: bool,
    /// Files from the provider's own record of the change (Claude edit tools, Codex file changes).
    files: Vec<(String, FileAction)>,
    /// Files named in Codex patch text, used only when a transcript records no file changes.
    patch_files: Vec<(String, FileAction)>,
}

fn line_info(provider: Provider, line: &[u8], offset: u64) -> LineInfo {
    let Ok(value) = serde_json::from_slice::<Value>(line) else { return LineInfo::default(); };
    match provider {
        Provider::Claude => claude_line(&value, offset),
        Provider::Codex => codex_line(&value, offset),
    }
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

fn claude_line(value: &Value, offset: u64) -> LineInfo {
    let mut info = LineInfo {
        message: claude_message(value, offset),
        cwd: value["cwd"].as_str().map(str::to_string),
        ..LineInfo::default()
    };
    if value["type"] == "ai-title" {
        info.title = value["aiTitle"].as_str().map(str::to_string);
    }
    let blocks = value["message"]["content"].as_array().map(Vec::as_slice).unwrap_or_default();
    let main_thread = value["isSidechain"] != true;
    match value["type"].as_str() {
        Some("user") if main_thread => {
            info.answers = info.message.is_some() || blocks.iter().any(|block| block["type"] == "tool_result");
        }
        Some("assistant") => {
            for block in blocks.iter().filter(|block| block["type"] == "tool_use") {
                let name = block["name"].as_str().unwrap_or_default();
                if name == "AskUserQuestion" && main_thread {
                    info.asks_user = true;
                }
                // Subagents edit files for the session too.
                let action = match name {
                    "Write" => FileAction::Written,
                    "Edit" | "MultiEdit" | "NotebookEdit" => FileAction::Edited,
                    _ => continue,
                };
                if let Some(path) = block["input"]["file_path"].as_str().or(block["input"]["notebook_path"].as_str()) {
                    info.files.push((path.to_string(), action));
                }
            }
        }
        _ => {}
    }
    info
}

fn claude_message(value: &Value, offset: u64) -> Option<ActivityMessage> {
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
        role, text, timestamp: value["timestamp"].as_str().map(str::to_string), truncated: false, automated: false, offset,
    })
}

fn codex_line(value: &Value, offset: u64) -> LineInfo {
    let payload = &value["payload"];
    let mut info = LineInfo { message: codex_message(value, offset), ..LineInfo::default() };
    info.answers = info.message.as_ref().is_some_and(|message| message.role == "user");
    match (value["type"].as_str(), payload["type"].as_str()) {
        (Some("session_meta" | "turn_context"), _) => info.cwd = payload["cwd"].as_str().map(str::to_string),
        (Some("event_msg"), Some("item_completed"))
            if payload["item"]["type"] == "FileChange" && payload["item"]["status"] == "completed" =>
        {
            for (path, change) in payload["item"]["changes"].as_object().into_iter().flatten() {
                let action = match change["type"].as_str() {
                    Some("add") => FileAction::Created,
                    Some("delete") => FileAction::Deleted,
                    _ => FileAction::Edited,
                };
                info.files.push((path.clone(), action));
            }
        }
        (Some("response_item"), Some("custom_tool_call" | "function_call")) => {
            let input = payload["input"].as_str().or(payload["arguments"].as_str()).unwrap_or_default();
            if input.contains("*** Begin Patch") {
                info.patch_files = patch_files(input);
            }
        }
        _ => {}
    }
    info
}

/// Files named by the headers of a Codex patch, also when the patch sits escaped inside code.
fn patch_files(text: &str) -> Vec<(String, FileAction)> {
    let mut files = Vec::new();
    for line in text.split('\n').flat_map(|line| line.split("\\n")) {
        let line = line.trim_start();
        let (action, rest) = if let Some(rest) = line.strip_prefix("*** Add File: ") {
            (FileAction::Created, rest)
        } else if let Some(rest) = line.strip_prefix("*** Update File: ") {
            (FileAction::Edited, rest)
        } else if let Some(rest) = line.strip_prefix("*** Delete File: ") {
            (FileAction::Deleted, rest)
        } else if let Some(rest) = line.strip_prefix("*** Move to: ") {
            (FileAction::Created, rest)
        } else {
            continue;
        };
        let path = rest.split(['"', '`', '\\', '\r']).next().unwrap_or_default().trim();
        if !path.is_empty() {
            files.push((path.to_string(), action));
        }
    }
    files
}

fn codex_message(value: &Value, offset: u64) -> Option<ActivityMessage> {
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
        role, text, timestamp: value["timestamp"].as_str().map(str::to_string), truncated: false, automated, offset,
    })
}

/// True when the text ends with a question, ignoring closing quotes, brackets and emphasis.
fn ends_with_question(text: &str) -> bool {
    text.trim_end_matches(|character: char| character.is_whitespace()
        || matches!(character, '*' | '_' | '`' | ')' | ']' | '"' | '\'' | '»' | '”' | '’'))
        .ends_with(['?', '？'])
}

/// A file log keeps each path once, with its actions in order and the time it was last touched.
#[derive(Default)]
struct FileLog {
    files: HashMap<String, (Vec<FileAction>, usize, usize)>,
}

impl FileLog {
    fn add(&mut self, path: String, action: FileAction, sequence: usize) {
        let entry = self.files.entry(path).or_insert_with(|| (Vec::new(), 0, 0));
        if !entry.0.contains(&action) {
            entry.0.push(action);
        }
        entry.1 += 1;
        entry.2 = sequence;
    }

    fn into_list(self, cwd: Option<&str>) -> (Vec<FileTouch>, bool) {
        let mut files: Vec<_> = self.files.into_iter().collect();
        files.sort_by(|a, b| b.1.2.cmp(&a.1.2));
        let truncated = files.len() > MAX_FILES;
        let list = files.into_iter().take(MAX_FILES).map(|(path, (actions, count, _))| {
            // Codex patches name files relative to the session's folder.
            let path = match cwd {
                Some(cwd) if Path::new(&path).is_relative() => Path::new(cwd).join(&path).to_string_lossy().into_owned(),
                _ => path,
            };
            FileTouch { path, actions, count }
        }).collect();
        (list, truncated)
    }
}

/// The shape of a whole session, without its messages.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_message: Option<ActivityMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message: Option<ActivityMessage>,
    /// User prompts, including automated ones.
    pub turn_count: usize,
    /// Prompts and agent replies.
    pub message_count: usize,
    /// The agent's last message ends with a question nobody answered, or Claude is
    /// waiting on its question tool.
    pub unanswered_question: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_title: Option<String>,
    /// The transcript is longer than what was read: counts and files cover only its end.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<FileTouch>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub files_truncated: bool,
}

/// One message that contains the searched text.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// Turn number, counting prompts from the start of the session. Missing when only the
    /// end of a long transcript was searched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<usize>,
    pub role: &'static str,
    pub excerpt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// Cursor that reads the turns up to and including this one; missing for the last turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

#[derive(Debug)]
pub struct SessionSearch {
    pub summary: SessionSummary,
    pub hits: Vec<SearchHit>,
    pub match_count: usize,
}

/// Case-insensitive text to find in prompts and replies.
pub struct Query {
    text: regex::Regex,
    /// Checks the raw file first so transcripts without the text are never parsed. Only
    /// usable when JSON stores the text unescaped.
    raw: Option<regex::bytes::Regex>,
}

impl Query {
    pub fn new(text: &str) -> Result<Self> {
        let text = text.trim();
        if text.chars().count() < 2 {
            bail!("query needs at least 2 characters");
        }
        let pattern = regex::escape(text);
        let raw = (!text.chars().any(|character| character == '"' || character == '\\' || character.is_control()))
            .then(|| regex::bytes::RegexBuilder::new(&pattern).case_insensitive(true).build())
            .transpose()?;
        Ok(Self { text: regex::RegexBuilder::new(&pattern).case_insensitive(true).build()?, raw })
    }
}

/// Reads a session line by line and keeps what a summary or a search needs.
struct Scanner<'a> {
    query: Option<&'a Query>,
    max_hits: usize,
    partial: bool,
    sequence: usize,
    turn_count: usize,
    message_count: usize,
    first: Option<ActivityMessage>,
    last: Option<ActivityMessage>,
    last_is_question: bool,
    pending_question: bool,
    title: Option<String>,
    cwd: Option<String>,
    files: FileLog,
    patch_files: FileLog,
    hits: Vec<SearchHit>,
    /// Trailing hits whose turn has not ended yet.
    open_hits: usize,
    match_count: usize,
}

impl<'a> Scanner<'a> {
    fn new(query: Option<&'a Query>, max_hits: usize, partial: bool) -> Self {
        Self {
            query, max_hits, partial, sequence: 0, turn_count: 0, message_count: 0, first: None, last: None,
            last_is_question: false, pending_question: false, title: None, cwd: None,
            files: FileLog::default(), patch_files: FileLog::default(), hits: Vec::new(), open_hits: 0, match_count: 0,
        }
    }

    fn feed(&mut self, info: LineInfo) {
        self.sequence += 1;
        if info.title.is_some() {
            self.title = info.title;
        }
        if self.cwd.is_none() {
            self.cwd = info.cwd;
        }
        if info.answers {
            self.pending_question = false;
        }
        if info.asks_user {
            self.pending_question = true;
        }
        for (path, action) in info.files {
            self.files.add(path, action, self.sequence);
        }
        for (path, action) in info.patch_files {
            self.patch_files.add(path, action, self.sequence);
        }
        let Some(message) = info.message else { return; };
        if message.role == "user" {
            self.turn_count += 1;
            let start = self.hits.len() - self.open_hits;
            for hit in &mut self.hits[start..] {
                hit.before = Some(message.offset.to_string());
            }
            self.open_hits = 0;
        }
        self.message_count += 1;
        if let Some(query) = self.query
            && let Some(found) = query.text.find(&message.text)
        {
            self.match_count += 1;
            if self.hits.len() < self.max_hits {
                self.hits.push(SearchHit {
                    turn: (!self.partial && self.turn_count > 0).then_some(self.turn_count),
                    role: message.role,
                    excerpt: excerpt(&message.text, found.start(), found.end()),
                    timestamp: message.timestamp.clone(),
                    before: None,
                });
                self.open_hits += 1;
            }
        }
        if self.first.is_none() {
            self.first = Some(truncate(message.clone(), EXCERPT_CHARS));
        }
        self.last_is_question = message.role == "assistant" && ends_with_question(&message.text);
        self.last = Some(truncate(message, EXCERPT_CHARS));
    }

    fn finish(self, include_files: bool) -> (SessionSummary, Vec<SearchHit>, usize) {
        let (files, files_truncated) = if include_files {
            let log = if self.files.files.is_empty() { self.patch_files } else { self.files };
            let (list, truncated) = log.into_list(self.cwd.as_deref());
            (Some(list), truncated)
        } else {
            (None, false)
        };
        let summary = SessionSummary {
            first_message: self.first,
            last_message: self.last,
            turn_count: self.turn_count,
            message_count: self.message_count,
            unanswered_question: self.pending_question || self.last_is_question,
            cwd: self.cwd,
            provider_title: self.title,
            partial: self.partial,
            files,
            files_truncated,
        };
        (summary, self.hits, self.match_count)
    }
}

/// Up to about 220 characters around the match, on character boundaries.
fn excerpt(text: &str, start: usize, end: usize) -> String {
    const BEFORE: usize = 70;
    const AFTER: usize = 150;
    let from = text[..start].char_indices().rev().nth(BEFORE - 1).map_or(0, |(index, _)| index);
    let to = text[end..].char_indices().nth(AFTER).map_or(text.len(), |(index, _)| end + index);
    let mut excerpt = text[from..to].split_whitespace().collect::<Vec<_>>().join(" ");
    if from > 0 {
        excerpt.insert(0, '…');
    }
    if to < text.len() {
        excerpt.push('…');
    }
    excerpt
}

/// Counts and first/last messages of a whole session, read forward. Transcripts longer
/// than the scan limit are read from their end and marked `partial`.
pub fn summarize(provider: Provider, path: &Path, include_files: bool) -> Result<SessionSummary> {
    ensure_readable(path)?;
    let mut file = File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
    let size = file.metadata()?.len();
    let start = size.saturating_sub(MAX_SCAN_BYTES);
    let mut scanner = Scanner::new(None, 0, start > 0);
    stream_lines(&mut file, start, size, |offset, line| {
        scanner.feed(line_info(provider, line, offset));
        true
    })?;
    let (mut summary, _, _) = scanner.finish(include_files);
    if start > 0 {
        // The real first message is at the start of the file.
        let mut first = None;
        stream_lines(&mut file, 0, HEAD_BYTES.min(size), |offset, line| {
            first = line_info(provider, line, offset).message;
            first.is_none()
        })?;
        summary.first_message = first.map(|message| truncate(message, EXCERPT_CHARS));
        if summary.cwd.is_none() {
            summary.cwd = transcript_cwd(provider, path);
        }
    }
    Ok(summary)
}

/// Searches one transcript. Returns the matches, if any, and the bytes read.
pub fn search_transcript(provider: Provider, path: &Path, query: &Query, max_hits: usize) -> Result<(Option<SessionSearch>, u64)> {
    ensure_readable(path)?;
    let mut file = File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
    let size = file.metadata()?.len();
    let start = size.saturating_sub(MAX_SEARCH_FILE_BYTES);
    let (bytes, base) = read_window(&mut file, start, size)?;
    let read = bytes.len() as u64;
    if query.raw.as_ref().is_some_and(|raw| !raw.is_match(&bytes)) {
        return Ok((None, read));
    }
    let mut scanner = Scanner::new(Some(query), max_hits.max(1), start > 0);
    for (offset, line) in lines(&bytes, base) {
        scanner.feed(line_info(provider, line, offset));
    }
    let (mut summary, hits, match_count) = scanner.finish(false);
    if match_count == 0 {
        return Ok((None, read));
    }
    if summary.cwd.is_none() {
        summary.cwd = transcript_cwd(provider, path);
    }
    Ok((Some(SessionSearch { summary, hits, match_count }), read))
}

/// The working folder the provider recorded at the start of the session.
pub fn transcript_cwd(provider: Provider, path: &Path) -> Option<String> {
    if is_compressed(path) {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let mut cwd = None;
    stream_lines(&mut file, 0, HEAD_BYTES.min(size), |offset, line| {
        cwd = line_info(provider, line, offset).cwd;
        cwd.is_none()
    }).ok()?;
    cwd
}

#[derive(Clone, Debug)]
pub struct TranscriptFile {
    pub provider: Provider,
    pub path: PathBuf,
    pub session_id: String,
    pub modified: SystemTime,
    pub size: u64,
}

impl TranscriptFile {
    pub fn compressed(&self) -> bool {
        is_compressed(&self.path)
    }
}

/// Claude names each project folder after its working directory, with every character
/// other than an ASCII letter or digit replaced by `-`.
pub fn claude_project_folder(path: &Path) -> String {
    path.to_string_lossy().chars().map(|character| if character.is_ascii_alphanumeric() { character } else { '-' }).collect()
}

/// Whether a session's working folder is one of `roots` or inside one.
pub fn in_scope(cwd: &str, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| Path::new(cwd).starts_with(root))
}

/// Transcripts under `homes`, newest first. With `scope`, Claude folders are limited to
/// those named after a root or a folder below one; Codex files carry their folder inside
/// and are filtered by the caller with `transcript_cwd`.
pub fn list_transcripts(provider: Provider, homes: &[PathBuf], scope: Option<&[PathBuf]>) -> Vec<TranscriptFile> {
    let mut files = Vec::new();
    match provider {
        Provider::Claude => {
            let prefixes: Option<Vec<String>> = scope.map(|roots| roots.iter().map(|root| claude_project_folder(root)).collect());
            for home in homes {
                let Ok(folders) = std::fs::read_dir(home.join("projects")) else { continue; };
                for folder in folders.flatten() {
                    let name = folder.file_name().to_string_lossy().into_owned();
                    if let Some(prefixes) = &prefixes
                        && !prefixes.iter().any(|prefix| name == *prefix || name.starts_with(&format!("{prefix}-")))
                    {
                        continue;
                    }
                    let Ok(entries) = std::fs::read_dir(folder.path()) else { continue; };
                    for entry in entries.flatten() {
                        let path = entry.path();
                        let Some(session_id) = path.file_name().and_then(|name| name.to_str())
                            .and_then(|name| name.strip_suffix(".jsonl")).map(str::to_string) else { continue; };
                        push_file(&mut files, provider, path, session_id);
                    }
                }
            }
        }
        Provider::Codex => {
            for home in homes {
                for folder in ["sessions", "archived_sessions"] {
                    collect_codex(&home.join(folder), 4, &mut files);
                }
            }
        }
    }
    files.sort_by(|a, b| b.modified.cmp(&a.modified));
    files
}

fn push_file(files: &mut Vec<TranscriptFile>, provider: Provider, path: PathBuf, session_id: String) {
    let Ok(metadata) = std::fs::metadata(&path) else { return; };
    if !metadata.is_file() || !valid_session_id(&session_id) {
        return;
    }
    files.push(TranscriptFile {
        provider, path, session_id,
        modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        size: metadata.len(),
    });
}

fn collect_codex(directory: &Path, depth: usize, files: &mut Vec<TranscriptFile>) {
    let Ok(entries) = std::fs::read_dir(directory) else { return; };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                collect_codex(&path, depth - 1, files);
            }
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue; };
        let Some(stem) = name.strip_suffix(".jsonl").or_else(|| name.strip_suffix(COMPRESSED_SUFFIX)) else { continue; };
        if !stem.starts_with("rollout-") || stem.len() < 36 {
            continue;
        }
        // `rollout-<time>-<uuid>`: the session ID is the trailing UUID.
        let session_id = stem[stem.len() - 36..].to_string();
        push_file(files, Provider::Codex, path, session_id);
    }
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

    fn user(text: &str) -> Value {
        serde_json::json!({"type":"user","message":{"content":text},"timestamp":format!("t-{text}")})
    }

    fn assistant(text: &str) -> Value {
        serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":text}]}})
    }

    fn tool_use(name: &str, input: Value) -> Value {
        serde_json::json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":name,"input":input}]}})
    }

    fn tool_result() -> Value {
        serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}})
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
        let activity = read_recent(Provider::Claude, &path, View::Messages, 10, 1000, None).unwrap();
        assert_eq!(activity.provider_title.as_deref(), Some("Fix login"));
        let texts: Vec<_> = activity.messages.iter().map(|message| (message.role, message.text.as_str())).collect();
        assert_eq!(texts, vec![("user", "Fix the login bug"), ("assistant", "Fixed it.")]);
        assert!(!activity.more_available);
        assert!(activity.older_cursor.is_none());
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
        let activity = read_recent(Provider::Codex, &path, View::Messages, 2, 100, None).unwrap();
        let texts: Vec<_> = activity.messages.iter().map(|message| message.text.as_str()).collect();
        assert_eq!(texts, vec!["Second", "A long answer that will be cut"]);
        assert!(activity.more_available);
        let short = read_recent(Provider::Codex, &path, View::Messages, 1, 100, None).unwrap();
        assert!(!short.messages[0].truncated);
        let all = read_recent(Provider::Codex, &path, View::Turns, 10, 100, None).unwrap();
        let first = all.turns[0].prompt.as_ref().unwrap();
        assert!(first.automated);
        assert_eq!(first.text, "Check Slack");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn turns_keep_each_prompt_with_its_last_reply() {
        let path = write(&[
            assistant("tail of an older turn"),
            user("Review PR 12"), assistant("Reading the diff"), assistant("Checking tests"), assistant("Two issues found"),
            user("Fix them"), assistant("Fixed both"),
        ]);
        let activity = read_recent(Provider::Claude, &path, View::Turns, 5, 1000, None).unwrap();
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
        let last = read_recent(Provider::Claude, &path, View::Turns, 1, 1000, None).unwrap();
        assert_eq!(last.turns.len(), 1);
        assert!(last.more_available);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn older_cursor_pages_back_through_turns_and_messages() {
        let mut lines = Vec::new();
        for number in 1..=5 {
            lines.push(user(&format!("Prompt {number}")));
            lines.push(tool_use("Bash", serde_json::json!({"command":"ls"})));
            lines.push(tool_result());
            lines.push(assistant(&format!("Reply {number}")));
        }
        let path = write(&lines);
        let prompts = |activity: &Activity| activity.turns.iter()
            .map(|turn| turn.prompt.as_ref().unwrap().text.clone()).collect::<Vec<_>>();
        let page = read_recent(Provider::Claude, &path, View::Turns, 2, 1000, None).unwrap();
        assert_eq!(prompts(&page), vec!["Prompt 4", "Prompt 5"]);
        let cursor = parse_cursor(page.older_cursor.as_deref().unwrap()).unwrap();
        let older = read_recent(Provider::Claude, &path, View::Turns, 2, 1000, Some(cursor)).unwrap();
        assert_eq!(prompts(&older), vec!["Prompt 2", "Prompt 3"]);
        assert_eq!(older.turns[1].reply.as_ref().unwrap().text, "Reply 3");
        let cursor = parse_cursor(older.older_cursor.as_deref().unwrap()).unwrap();
        let oldest = read_recent(Provider::Claude, &path, View::Turns, 2, 1000, Some(cursor)).unwrap();
        assert_eq!(prompts(&oldest), vec!["Prompt 1"]);
        assert!(!oldest.more_available);
        assert!(oldest.older_cursor.is_none());

        let messages = read_recent(Provider::Claude, &path, View::Messages, 3, 1000, None).unwrap();
        let cursor = parse_cursor(messages.older_cursor.as_deref().unwrap()).unwrap();
        let older = read_recent(Provider::Claude, &path, View::Messages, 3, 1000, Some(cursor)).unwrap();
        let texts: Vec<_> = older.messages.iter().map(|message| message.text.as_str()).collect();
        assert_eq!(texts, vec!["Prompt 3", "Reply 3", "Prompt 4"]);
        // Appending to a live session does not move an earlier cursor.
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{}", user("Prompt 6")).unwrap();
        let again = read_recent(Provider::Claude, &path, View::Messages, 3, 1000, Some(cursor)).unwrap();
        assert_eq!(again.messages, older.messages);
        assert!(read_recent(Provider::Claude, &path, View::Messages, 3, 1000, Some(u64::MAX)).is_err());
        assert!(parse_cursor("not a cursor").is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn summary_counts_turns_and_notices_an_unanswered_question() {
        let path = write(&[
            serde_json::json!({"type":"user","cwd":"/work/app","message":{"content":"Start"},"timestamp":"t1"}),
            assistant("Done. Should I also update the docs?**"),
        ]);
        let summary = summarize(Provider::Claude, &path, false).unwrap();
        assert_eq!(summary.turn_count, 1);
        assert_eq!(summary.message_count, 2);
        assert_eq!(summary.first_message.as_ref().unwrap().text, "Start");
        assert_eq!(summary.first_message.as_ref().unwrap().timestamp.as_deref(), Some("t1"));
        assert!(summary.last_message.as_ref().unwrap().text.starts_with("Done."));
        assert!(summary.unanswered_question);
        assert_eq!(summary.cwd.as_deref(), Some("/work/app"));
        assert!(summary.files.is_none());
        std::fs::remove_file(path).unwrap();

        // A prompt after the question answers it.
        let path = write(&[user("Start"), assistant("Which one?"), user("The first"), assistant("Done.")]);
        assert!(!summarize(Provider::Claude, &path, false).unwrap().unanswered_question);
        std::fs::remove_file(path).unwrap();

        // Claude's question tool waits for an answer until its result arrives.
        let ask = tool_use("AskUserQuestion", serde_json::json!({"questions":[{"question":"Which?"}]}));
        let path = write(&[user("Start"), assistant("I need a decision."), ask.clone()]);
        assert!(summarize(Provider::Claude, &path, false).unwrap().unanswered_question);
        std::fs::remove_file(path).unwrap();
        let path = write(&[user("Start"), ask, tool_result(), assistant("Going with the first.")]);
        assert!(!summarize(Provider::Claude, &path, false).unwrap().unanswered_question);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn files_come_from_edit_tools_without_their_contents() {
        let path = write(&[
            user("Edit things"),
            tool_use("Write", serde_json::json!({"file_path":"/work/new.rs","content":"secret contents"})),
            tool_use("Edit", serde_json::json!({"file_path":"/work/old.rs","old_string":"a","new_string":"b"})),
            tool_use("Edit", serde_json::json!({"file_path":"/work/new.rs","old_string":"c","new_string":"d"})),
            tool_use("Read", serde_json::json!({"file_path":"/work/read-only.rs"})),
        ]);
        let summary = summarize(Provider::Claude, &path, true).unwrap();
        let files = summary.files.unwrap();
        assert_eq!(files, vec![
            FileTouch { path: "/work/new.rs".into(), actions: vec![FileAction::Written, FileAction::Edited], count: 2 },
            FileTouch { path: "/work/old.rs".into(), actions: vec![FileAction::Edited], count: 1 },
        ]);
        assert!(!serde_json::to_string(&files).unwrap().contains("secret"));
        std::fs::remove_file(path).unwrap();

        let change = |changes: Value| serde_json::json!({"type":"event_msg","payload":{"type":"item_completed",
            "item":{"type":"FileChange","status":"completed","changes":changes}}});
        let patch = |input: &str| serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call","name":"exec","input":input}});
        let codex = write(&[
            serde_json::json!({"type":"session_meta","payload":{"cwd":"/repo"}}),
            patch("const patch = \"*** Begin Patch\\n*** Update File: src/lib.rs\\n@@\\n*** End Patch\";"),
            change(serde_json::json!({"/repo/src/lib.rs":{"type":"update","content":"x"},"/repo/notes.md":{"type":"add","content":"y"}})),
            change(serde_json::json!({"/repo/old.md":{"type":"delete","content":"z"}})),
        ]);
        let mut files = summarize(Provider::Codex, &codex, true).unwrap().files.unwrap();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let touched: Vec<_> = files.iter().map(|file| (file.path.as_str(), file.actions.clone())).collect();
        assert_eq!(touched, vec![
            ("/repo/notes.md", vec![FileAction::Created]),
            ("/repo/old.md", vec![FileAction::Deleted]),
            ("/repo/src/lib.rs", vec![FileAction::Edited]),
        ]);
        std::fs::remove_file(codex).unwrap();

        // Without file-change records, patch headers name the files, relative to the session folder.
        let older = write(&[
            serde_json::json!({"type":"session_meta","payload":{"cwd":"/repo"}}),
            serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch",
                "input":"*** Begin Patch\n*** Add File: docs/a.md\n+hi\n*** Delete File: b.md\n*** End Patch"}}),
        ]);
        let files = summarize(Provider::Codex, &older, true).unwrap().files.unwrap();
        let mut paths: Vec<_> = files.iter().map(|file| file.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, vec!["/repo/b.md", "/repo/docs/a.md"]);
        std::fs::remove_file(older).unwrap();
    }

    #[test]
    fn search_finds_prompts_and_replies_with_their_turn_and_cursor() {
        let path = write(&[
            serde_json::json!({"type":"user","cwd":"/work","message":{"content":"Set up the staging DATABASE"}}),
            tool_use("Bash", serde_json::json!({"command":"echo database in a tool call"})),
            assistant("Staging is ready."),
            user("Now production"),
            assistant("The production database is ready too."),
            user("Thanks"),
            assistant("You're welcome."),
        ]);
        let query = Query::new("database").unwrap();
        let (found, read) = search_transcript(Provider::Claude, &path, &query, 5).unwrap();
        assert!(read > 0);
        let found = found.unwrap();
        assert_eq!(found.match_count, 2);
        assert_eq!(found.summary.turn_count, 3);
        assert_eq!(found.summary.cwd.as_deref(), Some("/work"));
        let hits: Vec<_> = found.hits.iter().map(|hit| (hit.turn, hit.role)).collect();
        assert_eq!(hits, vec![(Some(1), "user"), (Some(2), "assistant")]);
        assert_eq!(found.hits[0].excerpt, "Set up the staging DATABASE");
        // The cursor reads the turns up to the match.
        let cursor = parse_cursor(found.hits[1].before.as_deref().unwrap()).unwrap();
        let activity = read_recent(Provider::Claude, &path, View::Turns, 1, 1000, Some(cursor)).unwrap();
        assert_eq!(activity.turns[0].prompt.as_ref().unwrap().text, "Now production");
        assert!(search_transcript(Provider::Claude, &path, &Query::new("missing words").unwrap(), 5).unwrap().0.is_none());
        // Text that only appears in tool calls is not a match.
        assert!(search_transcript(Provider::Claude, &path, &Query::new("tool call").unwrap(), 5).unwrap().0.is_none());
        assert!(Query::new(" a ").is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn excerpts_stay_on_character_boundaries() {
        let text = format!("{}café{}", "ñ".repeat(100), "é".repeat(200));
        let start = text.find("café").unwrap();
        let excerpt = excerpt(&text, start, start + "café".len());
        assert!(excerpt.starts_with('…') && excerpt.ends_with('…'));
        assert!(excerpt.contains("café"));
    }

    #[test]
    fn transcripts_are_found_by_session_id_only() {
        let home = std::env::temp_dir().join(format!("blackholes-homes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(home.join("projects/-tmp-project")).unwrap();
        std::fs::create_dir_all(home.join("sessions/2026/11/21")).unwrap();
        std::fs::write(home.join("projects/-tmp-project/abc.jsonl"), "").unwrap();
        std::fs::write(home.join("sessions/2026/11/21/rollout-2026-11-21T10-00-00-def.jsonl"), "").unwrap();
        std::fs::write(home.join("sessions/2026/11/21/rollout-2026-11-21T09-00-00-old.jsonl.zst"), "").unwrap();
        let homes = vec![home.clone()];
        assert!(find_transcript(Provider::Claude, "abc", &homes).is_some());
        assert!(find_transcript(Provider::Codex, "def", &homes).is_some());
        assert!(find_transcript(Provider::Claude, "../abc", &homes).is_none());
        assert!(find_transcript(Provider::Codex, "missing", &homes).is_none());
        // A compressed transcript is found but reported as unreadable.
        let compressed = find_transcript(Provider::Codex, "old", &homes).unwrap();
        let error = read_recent(Provider::Codex, &compressed, View::Turns, 1, 100, None).unwrap_err();
        assert!(error.to_string().contains("compressed"));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn listing_limits_claude_folders_to_the_project_and_reads_codex_ids() {
        assert_eq!(claude_project_folder(Path::new("/Users/me/Work_projects/App.v2")), "-Users-me-Work-projects-App-v2");
        let home = std::env::temp_dir().join(format!("blackholes-list-{}", uuid::Uuid::new_v4()));
        for folder in ["-work-app", "-work-app-api", "-work-application", "-other"] {
            std::fs::create_dir_all(home.join("projects").join(folder)).unwrap();
            std::fs::write(home.join("projects").join(folder).join(format!("{}.jsonl", &folder[1..])), "").unwrap();
        }
        let roots = vec![PathBuf::from("/work/app")];
        let mut ids: Vec<_> = list_transcripts(Provider::Claude, &[home.clone()], Some(&roots))
            .into_iter().map(|file| file.session_id).collect();
        ids.sort();
        assert_eq!(ids, vec!["work-app", "work-app-api"]);
        assert_eq!(list_transcripts(Provider::Claude, &[home.clone()], None).len(), 4);
        assert!(in_scope("/work/app/api", &roots));
        assert!(!in_scope("/work/application", &roots));

        let day = home.join("sessions/2026/11/21");
        std::fs::create_dir_all(&day).unwrap();
        let id = "019b60e0-2aae-75d0-931a-9830d70eb061";
        std::fs::write(day.join(format!("rollout-2026-11-21T10-00-00-{id}.jsonl")),
            "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/work/app\"}}\n").unwrap();
        std::fs::write(day.join(format!("rollout-2026-11-20T10-00-00-{id}.jsonl.zst")), "").unwrap();
        let codex = list_transcripts(Provider::Codex, &[home.clone()], None);
        assert_eq!(codex.len(), 2);
        assert!(codex.iter().all(|file| file.session_id == id));
        assert_eq!(codex.iter().filter(|file| file.compressed()).count(), 1);
        let readable = codex.iter().find(|file| !file.compressed()).unwrap();
        assert_eq!(transcript_cwd(Provider::Codex, &readable.path).as_deref(), Some("/work/app"));
        std::fs::remove_dir_all(home).unwrap();
    }
}
