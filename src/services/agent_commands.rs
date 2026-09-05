//! Acknowledged local commands. Task briefs exceed macOS's 2 KiB datagram limit.
use crate::paths::AppPaths;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::PermissionsExt, net::{UnixListener, UnixStream}},
    time::Duration,
};

const MAX_FRAME_BYTES: u64 = 128 * 1024;

pub struct AgentCommand {
    pub message: String,
    pub reply: flume::Sender<Value>,
}

fn read_frame(stream: &mut UnixStream) -> Result<String> {
    let mut frame = String::new();
    BufReader::new(stream.take(MAX_FRAME_BYTES + 1)).read_line(&mut frame)?;
    if frame.len() as u64 > MAX_FRAME_BYTES || !frame.ends_with('\n') {
        bail!("Invalid or oversized agent command frame");
    }
    Ok(frame)
}

pub fn send(paths: &AppPaths, message: &str) -> Result<Value> {
    let mut stream = UnixStream::connect(paths.data_dir.join("agent-commands.sock"))
        .context("Cannot reach the running Blackholes app. Reopen the updated app before delegating")?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let frame = serde_json::to_string(message)?;
    if frame.len() as u64 + 1 > MAX_FRAME_BYTES {
        bail!("Agent command is too large");
    }
    stream.write_all(frame.as_bytes())?;
    stream.write_all(b"\n")?;
    let response = read_frame(&mut stream).context(
        "The app did not confirm the handoff. Its state is unknown; check the destination agent before retrying",
    )?;
    let response: Value = serde_json::from_str(&response)?;
    if response.get("accepted").and_then(Value::as_bool) != Some(true) {
        bail!("{}", response.get("error").and_then(Value::as_str)
            .unwrap_or("The app rejected the agent command"));
    }
    Ok(response)
}

pub fn listen(paths: &AppPaths) -> Result<flume::Receiver<AgentCommand>> {
    let path = paths.data_dir.join("agent-commands.sock");
    if path.exists() {
        fs::remove_file(&path).context("Cannot replace the stale agent command socket")?;
    }
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    let (sender, receiver) = flume::bounded::<AgentCommand>(16);
    std::thread::Builder::new().name("blackholes-agent-commands".into()).spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut stream) = connection else { continue; };
            let result = (|| -> Result<Value> {
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                stream.set_write_timeout(Some(Duration::from_secs(5)))?;
                let message: String = serde_json::from_str(&read_frame(&mut stream)?)?;
                let (reply, response) = flume::bounded(1);
                sender.send(AgentCommand { message, reply })?;
                response.recv_timeout(Duration::from_secs(20))
                    .context("The app did not confirm execution; check the destination before retrying")
            })();
            let response = result.unwrap_or_else(|error| json!({
                "accepted": false, "error": error.to_string(),
            }));
            let _ = writeln!(stream, "{response}");
        }
    })?;
    Ok(receiver)
}
