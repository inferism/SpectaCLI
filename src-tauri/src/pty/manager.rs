//! Owns every agent process: spawn into a ConPTY + Job Object, route output
//! (see `batcher`), write, resize, kill. Rust owns the truth; panes attach and
//! detach freely without affecting the process.

use super::batcher::{self, Output, Sink};
use super::job::Job;
use super::ring;
use crate::registry::detect::{cmd_quote, LaunchKind, LaunchSpec};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Env var carrying a batch shim's command line. portable-pty quotes every
/// argument MSVC-style, which cmd.exe does not understand, so we run
/// `cmd /d /c %SPECTACLI_CMDLINE%` and let cmd expand it with its own rules.
const SHIM_VAR: &str = "SPECTACLI_CMDLINE";

/// Env vars a parent Claude Code session sets for its own children.
const INHERITED_SESSION_MARKERS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_HOST_SESSION_ID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_PID",
];

/// Device Status Report: "where is the cursor?"
const CURSOR_QUERY: &[u8] = b"\x1b[6n";

pub struct SpawnRequest {
    /// Agent id to use; a new one when `None`. Set it when the id must be
    /// known before launch (it goes into hook arguments).
    pub id: Option<String>,
    pub launch: LaunchSpec,
    /// Appended after the launch spec's own args (session/profile args).
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
}

/// Called once per agent, after its last output was flushed:
/// `(agent_id, exit_code)`.
pub type ExitHook = Arc<dyn Fn(&str, Option<u32>) + Send + Sync>;

struct Agent {
    output: Arc<Output>,
    writer: Mutex<Box<dyn Write + Send>>,
    /// Dropped when the child exits, which closes the ConPTY and lets the
    /// reader see EOF.
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    size: Mutex<(u16, u16)>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    job: Option<Arc<Job>>,
    /// Set once the child has exited.
    exited: (Mutex<bool>, Condvar),
}

impl Agent {
    /// True if the child exited within `timeout`.
    fn wait_exit(&self, timeout: Duration) -> bool {
        let (m, cv) = &self.exited;
        let g = cv.wait_timeout_while(lock(m), timeout, |done| !*done).unwrap_or_else(|e| e.into_inner());
        *g.0
    }
}

/// Sees every chunk of an agent's output as it is read: `(agent_id, bytes)`.
/// Runs on the reader thread, so it must be quick.
pub type OutputObserver = Arc<dyn Fn(&str, &[u8]) + Send + Sync>;

pub struct PtyManager {
    agents: Mutex<HashMap<String, Arc<Agent>>>,
    on_exit: ExitHook,
    observer: Option<OutputObserver>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn build_command(req: &SpawnRequest) -> CommandBuilder {
    let l = &req.launch;
    let mut cmd = CommandBuilder::new(&l.program);
    match l.kind {
        LaunchKind::CmdShim => {
            let shim = l.args.last().map(String::as_str).unwrap_or("");
            let mut line = format!("\"{shim}\"");
            for a in &req.args {
                line.push(' ');
                line.push_str(&cmd_quote(a));
            }
            cmd.args(["/d", "/c", &format!("%{SHIM_VAR}%")]);
            cmd.env(SHIM_VAR, line);
        }
        LaunchKind::Exe | LaunchKind::Node => {
            cmd.args(&l.args);
            cmd.args(&req.args);
        }
    }
    cmd.cwd(&req.cwd);
    // Started from a terminal inside Claude Code, Spectacli inherits markers
    // that tie a process to that session. Claude then treats each agent as
    // its child and, among other things, stops saving transcripts, so the
    // session could never be resumed. Settings (API keys, CLAUDE_CONFIG_DIR) stay.
    for k in INHERITED_SESSION_MARKERS {
        cmd.env_remove(k);
    }
    cmd.env("COLORTERM", "truecolor");
    for (k, v) in &req.env {
        cmd.env(k, v);
    }
    cmd
}

impl PtyManager {
    pub fn new(on_exit: ExitHook) -> Self {
        Self { agents: Mutex::new(HashMap::new()), on_exit, observer: None }
    }

    pub fn with_observer(mut self, observer: OutputObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    fn get(&self, id: &str) -> Result<Arc<Agent>, String> {
        lock(&self.agents).get(id).cloned().ok_or_else(|| format!("no agent `{id}`"))
    }

    /// Start a process in a new pseudoconsole and Job Object. Returns its id.
    pub fn spawn(&self, req: SpawnRequest) -> Result<String, String> {
        let err = |what: &str, e: &dyn std::fmt::Display| format!("{what}: {e}");
        let pair = native_pty_system()
            .openpty(PtySize { rows: req.rows.max(1), cols: req.cols.max(1), pixel_width: 0, pixel_height: 0 })
            .map_err(|e| err("open pty", &e))?;
        let mut child = pair
            .slave
            .spawn_command(build_command(&req))
            .map_err(|e| err(&format!("spawn {}", req.launch.program.display()), &e))?;
        drop(pair.slave);

        // Assigned right after spawn: CLIs take far longer than this to start
        // children of their own (PLAN.md 2.3).
        let job = Job::new().and_then(|j| {
            #[cfg(windows)]
            if let Some(h) = child.as_raw_handle() {
                j.assign(h)?;
            }
            Ok(j)
        });
        let job = match job {
            Ok(j) => Some(Arc::new(j)),
            Err(e) => {
                eprintln!("pty: no job object, tree kill unavailable: {e}");
                None
            }
        };

        let pipes = pair.master.try_clone_reader().and_then(|r| Ok((r, pair.master.take_writer()?)));
        let (mut reader, writer) = match pipes {
            Ok(p) => p,
            Err(e) => {
                let _ = child.kill(); // the job handle drops too, ending the tree
                return Err(err("pty pipes", &e));
            }
        };

        let id = req.id.clone().unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        let output = Output::new(ring::DEFAULT_CAP);
        let agent = Arc::new(Agent {
            output: output.clone(),
            writer: Mutex::new(writer),
            master: Mutex::new(Some(pair.master)),
            size: Mutex::new((req.cols, req.rows)),
            killer: Mutex::new(child.clone_killer()),
            job,
            exited: (Mutex::new(false), Condvar::new()),
        });
        lock(&self.agents).insert(id.clone(), agent.clone());

        // Reader: keep draining even with no pane attached, so the CLI never
        // blocks on a full pipe.
        let out = output.clone();
        let replier = agent.clone();
        let observer = self.observer.clone();
        let observed_id = id.clone();
        let reader_t = std::thread::Builder::new()
            .name(format!("pty-read-{id}"))
            .spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                let mut first = true;
                while let Ok(n @ 1..) = reader.read(&mut buf) {
                    let mut data = &buf[..n];
                    // portable-pty creates the ConPTY with INHERIT_CURSOR, so
                    // conhost first asks for the cursor position and blocks
                    // the child until it gets an answer. Answer it here, so
                    // agents start with no pane attached and a replay never
                    // makes xterm send a second reply into the CLI's input.
                    if std::mem::take(&mut first)
                        && let Some(rest) = data.strip_prefix(CURSOR_QUERY)
                    {
                        let mut w = lock(&replier.writer);
                        let _ = w.write_all(b"\x1b[1;1R").and_then(|_| w.flush());
                        data = rest;
                    }
                    if let Some(o) = &observer {
                        o(&observed_id, data);
                    }
                    out.push(data);
                }
                out.close();
            })
            .map_err(|e| err("reader thread", &e))?;
        let out = output.clone();
        let flusher_t = std::thread::Builder::new()
            .name(format!("pty-flush-{id}"))
            .spawn(move || out.run_flusher(batcher::WINDOW))
            .map_err(|e| err("flusher thread", &e))?;

        let on_exit = self.on_exit.clone();
        let exit_id = id.clone();
        std::thread::Builder::new()
            .name(format!("pty-wait-{id}"))
            .spawn(move || {
                let code = child.wait().ok().map(|s| s.exit_code());
                *lock(&agent.exited.0) = true;
                agent.exited.1.notify_all();
                // The CLI is gone: end whatever it left running, then close
                // the pseudoconsole so the reader drains and hits EOF.
                if let Some(job) = &agent.job {
                    let _ = job.terminate();
                }
                lock(&agent.master).take();
                let _ = reader_t.join();
                let _ = flusher_t.join();
                on_exit(&exit_id, code);
            })
            .map_err(|e| err("wait thread", &e))?;
        Ok(id)
    }

    /// See `Output::attach`.
    pub fn attach(&self, id: &str, sink_id: u64, sink: Sink) -> Result<Vec<u8>, String> {
        Ok(self.get(id)?.output.attach(sink_id, sink))
    }

    /// See `Output::detach_if`.
    pub fn detach(&self, id: &str, sink_id: u64) -> Result<(), String> {
        self.get(id)?.output.detach_if(sink_id);
        Ok(())
    }

    pub fn write(&self, id: &str, data: &[u8]) -> Result<(), String> {
        let agent = self.get(id)?;
        let mut w = lock(&agent.writer);
        w.write_all(data).and_then(|_| w.flush()).map_err(|e| format!("write: {e}"))
    }

    /// No-op when the size is unchanged (ConPTY reflows on every resize).
    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let agent = self.get(id)?;
        let (cols, rows) = (cols.max(1), rows.max(1));
        let mut size = lock(&agent.size);
        if *size == (cols, rows) {
            return Ok(());
        }
        if let Some(m) = lock(&agent.master).as_ref() {
            m.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| format!("resize: {e}"))?;
        }
        *size = (cols, rows);
        Ok(())
    }

    /// Kill the whole tree and forget the agent. The exit hook still fires.
    pub fn kill(&self, id: &str) -> Result<(), String> {
        let agent = lock(&self.agents).remove(id).ok_or_else(|| format!("no agent `{id}`"))?;
        agent.output.detach();
        let by_job = agent.job.as_ref().is_some_and(|j| j.terminate().is_ok());
        if !by_job {
            let _ = lock(&agent.killer).kill();
        }
        Ok(())
    }

    /// Ask the CLI to quit by typing `keys` one at a time (`gap` apart), give
    /// it `grace` to exit, then end the whole tree either way. Blocks; the
    /// exit hook fires as for `kill`.
    pub fn stop_gracefully(&self, id: &str, keys: &[String], gap: Duration, grace: Duration) -> Result<(), String> {
        let agent = self.get(id)?;
        let mut exited = false;
        for k in keys {
            if self.write(id, k.as_bytes()).is_err() {
                break;
            }
            if agent.wait_exit(gap) {
                exited = true;
                break;
            }
        }
        if !exited && !keys.is_empty() {
            agent.wait_exit(grace);
        }
        // Also ends what the CLI left running (MCP servers and the like).
        self.kill(id)
    }

    /// Job Objects of the live agents, for the stats sampler. Holding one
    /// past `kill` is harmless: the tree is already terminated.
    pub fn jobs(&self) -> Vec<(String, Arc<Job>)> {
        lock(&self.agents).iter().filter_map(|(id, a)| Some((id.clone(), a.job.clone()?))).collect()
    }

    #[cfg(test)]
    fn ids(&self) -> Vec<String> {
        lock(&self.agents).keys().cloned().collect()
    }

    #[cfg(test)]
    fn pids(&self, id: &str) -> Vec<u32> {
        self.get(id).ok().and_then(|a| a.job.as_ref().and_then(|j| j.pids().ok())).unwrap_or_default()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn manager() -> (PtyManager, mpsc::Receiver<(String, Option<u32>)>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        (PtyManager::new(Arc::new(move |id, code| drop(lock(&tx).send((id.to_string(), code))))), rx)
    }

    fn req(kind: LaunchKind, program: &str, args: &[&str], extra: &[&str]) -> SpawnRequest {
        SpawnRequest {
            id: None,
            launch: LaunchSpec { kind, program: program.into(), args: args.iter().map(|s| s.to_string()).collect() },
            args: extra.iter().map(|s| s.to_string()).collect(),
            cwd: std::env::temp_dir(),
            env: vec![],
            cols: 80,
            rows: 24,
        }
    }

    /// Attach and collect output until the exit hook fires.
    fn run_to_exit(m: &PtyManager, rx: &mpsc::Receiver<(String, Option<u32>)>, id: &str) -> (String, Option<u32>) {
        let (tx, out) = mpsc::channel();
        let mut text = String::from_utf8_lossy(&m.attach(id, 1, Box::new(move |d| tx.send(d).is_ok())).unwrap()).into_owned();
        let (exited, code) = rx.recv_timeout(Duration::from_secs(10)).expect("agent exited");
        assert_eq!(exited, id);
        for d in out.try_iter() {
            text.push_str(&String::from_utf8_lossy(&d));
        }
        (text, code)
    }

    #[test]
    fn runs_command_and_reports_exit() {
        let (m, rx) = manager();
        let id = m.spawn(req(LaunchKind::Exe, "cmd.exe", &["/d", "/c"], &["echo", "hello-pty"])).unwrap();
        let (text, code) = run_to_exit(&m, &rx, &id);
        assert!(text.contains("hello-pty"), "output: {text:?}");
        assert_eq!(code, Some(0));
    }

    #[test]
    fn cmd_shim_keeps_quoting_with_spaces() {
        let dir = std::env::temp_dir().join(format!("spectacli shim {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shim = dir.join("tool.cmd");
        std::fs::write(&shim, "@echo shim[%1][%2]\r\n").unwrap();
        let (m, rx) = manager();
        let shim_s = shim.to_string_lossy();
        let id = m.spawn(req(LaunchKind::CmdShim, "cmd.exe", &["/d", "/s", "/c", &shim_s], &["a b", "c&d"])).unwrap();
        let (text, _) = run_to_exit(&m, &rx, &id);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(text.contains(r#"shim["a b"]["c&d"]"#), "output: {text:?}");
    }

    #[test]
    fn write_reaches_the_process() {
        let (m, rx) = manager();
        let id = m.spawn(req(LaunchKind::Exe, "cmd.exe", &["/d", "/q", "/k"], &[])).unwrap();
        m.resize(&id, 100, 30).unwrap();
        m.write(&id, b"echo typed-%COLORTERM%\r\nexit 7\r\n").unwrap();
        let (text, code) = run_to_exit(&m, &rx, &id);
        assert!(text.contains("typed-truecolor"), "output: {text:?}");
        assert_eq!(code, Some(7));
    }

    #[test]
    fn kill_ends_the_whole_tree() {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};

        let (m, rx) = manager();
        let id = m.spawn(req(LaunchKind::Exe, "cmd.exe", &["/d", "/c"], &["ping", "-n", "60", "127.0.0.1"])).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let pids = loop {
            let p = m.pids(&id);
            if p.len() >= 2 || Instant::now() > deadline {
                break p;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(pids.len() >= 2, "cmd and its ping child are in the job: {pids:?}");
        // Open handles before killing so the PIDs can't be reused under us.
        let handles: Vec<_> = pids.iter().map(|&p| unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, p) }).collect();
        m.kill(&id).unwrap();
        for h in handles {
            assert!(!h.is_null());
            assert_eq!(unsafe { WaitForSingleObject(h, 5000) }, WAIT_OBJECT_0, "process still alive");
            unsafe { CloseHandle(h) };
        }
        rx.recv_timeout(Duration::from_secs(10)).expect("exit hook fires after kill");
        assert!(m.ids().is_empty());
    }

    #[test]
    fn parent_claude_session_markers_are_not_inherited() {
        // SAFETY: no other test reads or writes this variable.
        unsafe { std::env::set_var("CLAUDE_CODE_CHILD_SESSION", "1") };
        let (m, rx) = manager();
        let id = m
            .spawn(req(LaunchKind::Exe, "cmd.exe", &["/d", "/c"], &["echo", "[%CLAUDE_CODE_CHILD_SESSION%]"]))
            .unwrap();
        let (out, _) = run_to_exit(&m, &rx, &id);
        assert!(out.contains("[%CLAUDE_CODE_CHILD_SESSION%]"), "variable is unset in the agent: {out:?}");
    }

    const GAP: Duration = Duration::from_millis(300);

    #[test]
    fn stop_gracefully_lets_the_cli_quit_itself() {
        let (m, rx) = manager();
        // An interactive cmd prompt that quits when it reads `exit 5`.
        let id = m.spawn(req(LaunchKind::Exe, "cmd.exe", &["/d", "/q"], &[])).unwrap();
        let started = Instant::now();
        m.stop_gracefully(&id, &["exit 5\r".into()], GAP, Duration::from_secs(5)).unwrap();
        let (got, code) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!((got.as_str(), code), (id.as_str(), Some(5)), "cmd ran `exit 5` rather than being killed");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(m.ids().is_empty());
    }

    #[test]
    fn stop_gracefully_kills_a_cli_that_ignores_the_keys() {
        let (m, rx) = manager();
        let id = m.spawn(req(LaunchKind::Exe, "cmd.exe", &["/d", "/c"], &["ping", "-n", "60", "127.0.0.1"])).unwrap();
        let started = Instant::now();
        m.stop_gracefully(&id, &["x".into()], GAP, Duration::from_millis(500)).unwrap();
        let (_, code) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(code, Some(1), "ended by the job");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(m.ids().is_empty());
    }
}
