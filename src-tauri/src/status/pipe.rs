//! Named pipe `\\.\pipe\<name>` that receives one message per connection from
//! `spectacli-notify`: `<agent-id>\n<event>\n<payload>`. One instance,
//! served one client at a time; a client that finds it busy retries (the
//! helper waits up to 2 s, and a message takes well under a millisecond).

/// Payload cap, matching the helper.
const MAX_PAYLOAD: usize = 64 * 1024;
/// Agent id and event name, with room to spare.
const MAX_HEADER: usize = 256;

/// Split a message into (agent id, event, payload).
fn parse(msg: &[u8]) -> Option<(&str, &str, &[u8])> {
    let mut parts = msg.splitn(3, |&b| b == b'\n');
    let agent = std::str::from_utf8(parts.next()?).ok()?;
    let event = std::str::from_utf8(parts.next()?).ok()?;
    (!agent.is_empty() && !event.is_empty()).then_some((agent, event, parts.next().unwrap_or_default()))
}

#[cfg(windows)]
pub fn serve(name: &str, on_msg: impl Fn(&str, &str, &[u8]) + Send + 'static) -> Result<(), String> {
    use std::io::Read;
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND};
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_WAIT,
    };

    let wide: Vec<u16> = format!(r"\\.\pipe\{name}").encode_utf16().chain([0]).collect();
    // FIRST_PIPE_INSTANCE: fail rather than join a pipe someone else created
    // under our name. The default DACL lets only this user (and admins and
    // SYSTEM) write to it; remote clients are rejected.
    let h = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            0,
            MAX_PAYLOAD as u32,
            0,
            std::ptr::null(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(format!("create pipe {name}: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: a fresh handle that nothing else owns.
    let pipe = std::fs::File::from(unsafe { OwnedHandle::from_raw_handle(h) });

    std::thread::Builder::new()
        .name("status-pipe".into())
        .spawn(move || {
            use std::os::windows::io::AsRawHandle;
            let h = pipe.as_raw_handle();
            let mut buf = Vec::with_capacity(MAX_HEADER + MAX_PAYLOAD);
            loop {
                let connected = unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) } != 0
                    || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
                if connected {
                    buf.clear();
                    // EOF when the helper closes its end. Anything past the
                    // cap is dropped with the connection.
                    let limit = (MAX_HEADER + MAX_PAYLOAD) as u64;
                    if (&pipe).take(limit).read_to_end(&mut buf).is_ok()
                        && let Some((agent, event, payload)) = parse(&buf)
                    {
                        on_msg(agent, event, payload);
                    }
                } else {
                    eprintln!("status: pipe connect: {}", std::io::Error::last_os_error());
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                unsafe { DisconnectNamedPipe(h) };
            }
        })
        .map_err(|e| format!("pipe thread: {e}"))?;
    Ok(())
}

#[cfg(not(windows))]
pub fn serve(_name: &str, _on_msg: impl Fn(&str, &str, &[u8]) + Send + 'static) -> Result<(), String> {
    Err("named pipes are Windows-only".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_messages() {
        assert_eq!(parse(b"id\nclaude-stop\n{\"a\":1}\n"), Some(("id", "claude-stop", &b"{\"a\":1}\n"[..])));
        assert_eq!(parse(b"id\ncodex\n"), Some(("id", "codex", &b""[..])));
        assert_eq!(parse(b"id\ncodex"), Some(("id", "codex", &b""[..])));
        assert_eq!(parse(b"id"), None);
        assert_eq!(parse(b"\nx\n"), None);
    }

    #[cfg(windows)]
    #[test]
    fn receives_from_many_clients() {
        use std::io::Write;
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        let name = format!("spectacli-test-{}", uuid::Uuid::now_v7().simple());
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        serve(&name, move |a, e, p| drop(tx.lock().unwrap().send(format!("{a}|{e}|{}", String::from_utf8_lossy(p)))))
            .unwrap();
        assert!(serve(&name, |_, _, _| {}).is_err(), "a second server can't take the name");

        let path = format!(r"\\.\pipe\{name}");
        let clients: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let deadline = Instant::now() + Duration::from_secs(2);
                    let mut f = loop {
                        match std::fs::OpenOptions::new().write(true).open(&path) {
                            Ok(f) => break f,
                            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                            Err(e) => panic!("connect: {e}"),
                        }
                    };
                    f.write_all(format!("agent{i}\nclaude-stop\n{{}}").as_bytes()).unwrap();
                })
            })
            .collect();
        clients.into_iter().for_each(|c| c.join().unwrap());
        let mut got: Vec<String> = (0..8).map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap()).collect();
        got.sort();
        let want: Vec<String> = (0..8).map(|i| format!("agent{i}|claude-stop|{{}}")).collect();
        assert_eq!(got, want);
    }

    /// The real helper, as Claude (stdin) and Codex (last argument) call it.
    /// Needs `npm run notify` first; skipped when the helper isn't built.
    #[cfg(windows)]
    #[test]
    fn helper_delivers_stdin_and_arg_payloads() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        use std::sync::mpsc;
        use std::time::Duration;

        let helper = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/spectacli-notify.exe");
        if !helper.is_file() {
            eprintln!("skipped: {} not built", helper.display());
            return;
        }
        let name = format!("spectacli-test-{}", uuid::Uuid::now_v7().simple());
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        serve(&name, move |a, e, p| drop(tx.lock().unwrap().send(format!("{a}|{e}|{}", String::from_utf8_lossy(p)))))
            .unwrap();

        let mut claude = Command::new(&helper).args([&name, "id1", "claude-stop"]).stdin(Stdio::piped()).spawn().unwrap();
        claude.stdin.take().unwrap().write_all(br#"{"hook_event_name":"Stop"}"#).unwrap();
        assert!(claude.wait().unwrap().success());
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), r#"id1|claude-stop|{"hook_event_name":"Stop"}"#);

        let codex = Command::new(&helper).args([&name, "id2", "codex", r#"{"type":"agent-turn-complete"}"#]).status();
        assert!(codex.unwrap().success());
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), r#"id2|codex|{"type":"agent-turn-complete"}"#);

        // No server: exits 0 after its short wait instead of failing the CLI.
        let gone = Command::new(&helper).args(["spectacli-nobody", "id", "codex", "{}"]).status();
        assert!(gone.unwrap().success());
    }
}
