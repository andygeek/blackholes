//! Session tracking is observational: it must never delay a provider's turn.
use serde_json::{Value, json};

pub(super) fn upsert(groups: &mut Vec<Value>, marker: &str, command: &str) {
    groups.retain_mut(|group| {
        let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let before = handlers.len();
        handlers.retain(|handler| {
            !handler
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| command.contains(marker))
        });
        // Only remove groups that our migration emptied.
        !(before > handlers.len() && handlers.is_empty())
    });
    groups.push(json!({ "hooks": [{
        "type": "command", "command": command, "async": true, "timeout": 3,
    }] }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_only_our_handler_and_preserves_shared_groups() {
        let custom = json!({"type":"command","command":"user-owned-hook","timeout":30});
        let mut groups = vec![json!({"matcher":"startup|resume", "hooks":[
            {"type":"command","command":"old blackholes-marker", "timeout":3}, custom.clone(),
        ]})];
        upsert(&mut groups, "blackholes-marker", "new blackholes-marker");
        assert_eq!(groups[0]["hooks"], json!([custom]));
        assert_eq!(groups[0]["matcher"], "startup|resume");
        assert_eq!(groups[1]["hooks"][0]["async"], true);
        assert!(groups[1].get("matcher").is_none());
        let once = groups.clone();
        upsert(&mut groups, "blackholes-marker", "new blackholes-marker");
        assert_eq!(once, groups);
    }
}
