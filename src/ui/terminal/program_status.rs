//! OSC 7501 Program Status Protocol, revision 0.4. No message/title text leaves
//! the PTY parser: only bounded lifecycle state and known provider identity.
use super::{AgentKind, AgentTerminalSignal, AgentTerminalSignalKind};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use std::collections::HashMap;

pub(super) const SUPPORT_REPLY: &[u8] =
    b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";
const MAX_RECORDS: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Working,
    Blocked,
    Done,
    Error,
}
struct Record {
    state: State,
    app: Option<String>,
    updated: u64,
}

#[derive(Default)]
pub(super) struct ProgramStatus {
    records: HashMap<String, Record>,
    revision: u64,
    last_signal: Option<AgentTerminalSignal>,
}

pub(super) enum Event {
    Reply,
    Status {
        signal: Option<AgentTerminalSignal>,
        active: bool,
    },
}

impl ProgramStatus {
    pub fn active(&self) -> bool {
        self.records
            .values()
            .any(|r| matches!(r.state, State::Idle | State::Working | State::Blocked))
    }

    pub fn receive(&mut self, sequence: &str) -> Option<Event> {
        let body = sequence.strip_prefix("7501;")?;
        if body == "?" {
            return Some(Event::Reply);
        }
        if sequence.len() + 4 > 4096 {
            return None;
        }
        let mut fields = HashMap::new();
        for pair in body.split(':') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            if key.len() > 16 {
                return None;
            }
            if key.is_empty() || !key.bytes().all(|b| b.is_ascii_lowercase()) {
                continue;
            }
            // Free text is validated even though it is never shown or stored.
            if matches!(key, "msg" | "title") && !valid_text(value, key == "title") {
                return None;
            }
            if !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.,+/=-".contains(&b))
            {
                continue;
            }
            fields.insert(key, value);
        }
        let id = fields.get("id").copied().unwrap_or("");
        if fields.contains_key("id") && !valid_id(id) {
            return None;
        }
        let state = *fields.get("state")?;
        if state == "clear" {
            self.records.retain(|key, _| {
                !(id.is_empty() || key == id || key.starts_with(&format!("{id}/")))
            });
        } else {
            let state = match state {
                "idle" => State::Idle,
                "working" => State::Working,
                "blocked" => State::Blocked,
                "done" => State::Done,
                "error" => State::Error,
                _ => return None,
            };
            let app = fields
                .get("app")
                .filter(|app| valid_segment(app))
                .map(|app| app.to_string());
            if !self.records.contains_key(id) && self.records.len() == MAX_RECORDS {
                let oldest = self
                    .records
                    .iter()
                    .min_by_key(|(_, r)| r.updated)
                    .map(|(id, _)| id.clone())?;
                self.records.remove(&oldest);
            }
            self.revision = self.revision.wrapping_add(1);
            self.records.insert(
                id.to_string(),
                Record {
                    state,
                    app,
                    updated: self.revision,
                },
            );
        }
        Some(self.event())
    }

    pub fn shell_prompt(&mut self) -> Event {
        self.records
            .retain(|_, r| matches!(r.state, State::Done | State::Error));
        self.event()
    }

    pub fn reset(&mut self) -> Event {
        self.records.clear();
        self.event()
    }

    fn event(&mut self) -> Event {
        // Keep the terminal working while any record is working: close_idle
        // must not terminate a working parent because one child needs input.
        // Completed workers also must not mark a running parent as done.
        let selected = self.records.iter().max_by_key(|(_, r)| {
            (
                match r.state {
                    State::Blocked => 3,
                    State::Working => 4,
                    State::Done | State::Error => 2,
                    State::Idle => 1,
                },
                r.updated,
            )
        });
        let signal = selected
            .map(|(id, record)| AgentTerminalSignal {
                agent: self.agent(id),
                kind: match record.state {
                    State::Idle => AgentTerminalSignalKind::Started,
                    State::Working => AgentTerminalSignalKind::Working,
                    State::Blocked | State::Done | State::Error => {
                        AgentTerminalSignalKind::Attention
                    }
                },
            })
            .unwrap_or(AgentTerminalSignal {
                agent: None,
                kind: AgentTerminalSignalKind::Started,
            });
        let changed = self.last_signal != Some(signal);
        self.last_signal = Some(signal);
        Event::Status {
            signal: changed.then_some(signal),
            active: self.active(),
        }
    }

    fn agent(&self, mut id: &str) -> Option<AgentKind> {
        loop {
            if let Some(app) = self.records.get(id).and_then(|r| r.app.as_deref()) {
                return match app {
                    "claude" | "claude-code" => Some(AgentKind::Claude),
                    "codex" => Some(AgentKind::Codex),
                    "gemini" => Some(AgentKind::Gemini),
                    "opencode" => Some(AgentKind::OpenCode),
                    _ => None,
                };
            }
            if id.is_empty() {
                return None;
            }
            id = id.rsplit_once('/').map(|(parent, _)| parent).unwrap_or("");
        }
    }
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
}
fn valid_id(value: &str) -> bool {
    value.len() <= 128 && value.split('/').count() <= 8 && value.split('/').all(valid_segment)
}
fn valid_text(value: &str, title: bool) -> bool {
    let (encoded_limit, decoded_limit) = if title { (256, 192) } else { (2732, 2048) };
    if value.len() > encoded_limit {
        return false;
    }
    let decoded = STANDARD
        .decode(value)
        .or_else(|_| STANDARD_NO_PAD.decode(value));
    decoded
        .ok()
        .filter(|bytes| bytes.len() <= decoded_limit)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .is_some_and(|text| !text.chars().any(char::is_control))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn signal(event: Option<Event>) -> AgentTerminalSignal {
        match event {
            Some(Event::Status {
                signal: Some(signal),
                ..
            }) => signal,
            _ => panic!("Expected state update"),
        }
    }
    #[test]
    fn current_claude_reports_and_detection_are_supported() {
        let mut status = ProgramStatus::default();
        assert!(matches!(status.receive("7501;?"), Some(Event::Reply)));
        assert_eq!(
            signal(status.receive("7501;state=working:app=claude-code")).kind,
            AgentTerminalSignalKind::Working
        );
        let done = signal(status.receive("7501;state=done:app=claude-code"));
        assert_eq!(done.agent, Some(AgentKind::Claude));
        assert_eq!(done.kind, AgentTerminalSignalKind::Attention);
        assert!(matches!(
            status.receive("7501;state=done:app=claude-code"),
            Some(Event::Status { signal: None, .. })
        ));
    }
    #[test]
    fn worker_status_inherits_identity_and_clear_is_hierarchical() {
        let mut status = ProgramStatus::default();
        status.receive("7501;state=working:app=codex");
        assert!(matches!(
            status.receive("7501;state=done:id=worker"),
            Some(Event::Status { signal: None, .. })
        ));
        assert!(matches!(
            status.receive("7501;state=blocked:id=worker/child:kind=permission"),
            Some(Event::Status { signal: None, .. })
        ));
        let blocked = signal(status.receive("7501;state=idle:app=codex"));
        assert_eq!(blocked.agent, Some(AgentKind::Codex));
        assert_eq!(blocked.kind, AgentTerminalSignalKind::Attention);
        assert_eq!(
            signal(status.receive("7501;state=clear:id=worker")).kind,
            AgentTerminalSignalKind::Started
        );
        status.receive("7501;state=clear");
        assert!(!status.active());
    }
    #[test]
    fn untrusted_text_ids_and_limits_cannot_update_state() {
        let mut status = ProgramStatus::default();
        for body in [
            "state=working:msg=Z28K",
            "state=working:title=***",
            "state=working:id=../",
            "state=working:id=",
            "state=new-state",
            "state=working:msg=77+9/w==",
        ] {
            assert!(status.receive(&format!("7501;{body}")).is_none(), "{body}");
        }
        assert!(
            status
                .receive(&format!("7501;state=working:msg={}", "YQ".repeat(2000)))
                .is_none()
        );
        assert!(!status.active());
        let value = signal(
            status.receive("7501;state=idle:state=working:future=ok:app=claude-code:msg=b2s"),
        );
        assert_eq!(value.kind, AgentTerminalSignalKind::Working);
    }
    #[test]
    fn prompt_and_reset_clear_transient_states_and_records_are_bounded() {
        let mut status = ProgramStatus::default();
        for n in 0..300 {
            status.receive(&format!("7501;state=working:id=worker{n}"));
        }
        assert_eq!(status.records.len(), MAX_RECORDS);
        status.receive("7501;state=done:app=claude-code");
        status.shell_prompt();
        assert_eq!(status.records.len(), 1);
        status.reset();
        assert!(!status.active());
    }
}
