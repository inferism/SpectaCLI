//! `spectacli-notify <pipe> <agent-id> <event> [-- <command...>] [payload]`
//!
//! With `-- <command...>` (Codex, whose one `notify` slot we take), it also
//! runs the user's own notify command with the payload appended.
//!
//! Run by an agent CLI's hook (PLAN.md feature 4). Sends one message to
//! SpectaCLI's named pipe and exits:
//!
//! ```text
//! <agent-id>\n<event>\n<payload bytes>
//! ```
//!
//! Claude hooks pass their JSON on stdin; Codex appends it as the last
//! argument. The helper always exits 0 and gives up after a short wait, so a
//! closed SpectaCLI never slows down or breaks the CLI that called it.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// Payload cap; the receiver enforces the same limit.
const MAX_PAYLOAD: u64 = 64 * 1024;
/// How long to retry while the pipe is busy with another message.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

fn main() {
    let _ = run();
}

fn run() -> Option<()> {
    let mut args = std::env::args().skip(1);
    let (pipe, agent, event) = (args.next()?, args.next()?, args.next()?);
    // Only ever talk to our own pipes.
    if !pipe.starts_with("spectacli-") || !pipe.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return None;
    }
    // Codex: `[-- <user's notify command...>] <payload>`, payload last.
    let mut rest: Vec<String> = args.collect();
    let mut payload = rest.pop().unwrap_or_default();
    if rest.first().is_some_and(|a| a == "--") && rest.len() > 1 {
        // The user's own notify, which ours replaced: run it with the same
        // payload, as Codex would have. Not awaited.
        let _ = std::process::Command::new(&rest[1])
            .args(&rest[2..])
            .arg(&payload)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    } else if !rest.is_empty() {
        // Not the shape we launch with: forward nothing odd.
        payload.clear();
    }
    let mut payload = payload.into_bytes();
    if event.starts_with("claude-") {
        let _ = std::io::stdin().take(MAX_PAYLOAD).read_to_end(&mut payload);
    }
    payload.truncate(MAX_PAYLOAD as usize);

    let path = format!(r"\\.\pipe\{pipe}");
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let mut pipe = loop {
        match std::fs::OpenOptions::new().write(true).open(&path) {
            Ok(f) => break f,
            // Busy (one message at a time) or briefly between messages.
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return None,
        }
    };
    let mut msg = format!("{agent}\n{event}\n").into_bytes();
    msg.extend_from_slice(&payload);
    pipe.write_all(&msg).ok()
}
