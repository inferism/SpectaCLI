//! Find registry agents on PATH, resolve how to launch them, and probe their
//! version to reject impostors. Probe results are cached by target path and
//! mtime in `detect-cache.json`, so repeat detection is instant.

use super::shim::{self, ShimTarget};
use super::AgentDef;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

pub const CACHE_FILE: &str = "detect-cache.json";
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// Extensions CreateProcess can run (directly or via cmd.exe), best first.
const RUNNABLE: [&str; 4] = [".exe", ".com", ".cmd", ".bat"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LaunchKind {
    /// `program` is the CLI itself.
    Exe,
    /// `program` is node.exe and `args[0]` is the script.
    Node,
    /// Unparseable batch shim: `program` is cmd.exe and `args` is
    /// `["/d", "/s", "/c", <shim>]`. With `/s`, cmd strips the outer quotes of
    /// everything after `/c`, so the command line must be built as
    /// `/d /s /c ""<shim>" <args>"` (see `command`), not with normal quoting.
    CmdShim,
}

/// How to start an agent: `program` + `args`, then the session's own args.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchSpec {
    pub kind: LaunchKind,
    pub program: PathBuf,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedAgent {
    pub def: AgentDef,
    /// What was found on PATH (may be a shim).
    pub path: PathBuf,
    pub launch: LaunchSpec,
    /// First line of the version probe output, if the agent has a probe.
    pub version: Option<String>,
}

impl LaunchSpec {
    /// The file whose mtime tells us the CLI changed (the real target, not the
    /// shim, since npm updates rarely rewrite the shim).
    fn target(&self) -> &Path {
        match self.kind {
            LaunchKind::Exe => &self.program,
            LaunchKind::Node | LaunchKind::CmdShim => Path::new(self.args.last().map(String::as_str).unwrap_or("")),
        }
    }

    /// A `Command` running this spec with `extra` arguments appended.
    pub fn command(&self, extra: &[String]) -> Command {
        let mut cmd = Command::new(&self.program);
        match self.kind {
            LaunchKind::CmdShim => {
                let shim = self.args.last().map(String::as_str).unwrap_or("");
                let mut line = format!("/d /s /c \"\"{shim}\"");
                for a in extra {
                    line.push(' ');
                    line.push_str(&cmd_quote(a));
                }
                line.push('"');
                raw_arg(&mut cmd, line);
            }
            _ => {
                cmd.args(&self.args).args(extra);
            }
        }
        cmd
    }
}

#[cfg(windows)]
fn raw_arg(cmd: &mut Command, line: String) {
    use std::os::windows::process::CommandExt;
    cmd.raw_arg(line);
}

#[cfg(not(windows))]
fn raw_arg(cmd: &mut Command, line: String) {
    cmd.arg(line);
}

fn cmd_quote(a: &str) -> String {
    if a.is_empty() || a.contains([' ', '\t', '&', '|', '<', '>', '^', '(', ')']) {
        format!("\"{}\"", a.replace('"', "\"\""))
    } else {
        a.to_string()
    }
}

/// Look `name` up on `path_var`, Windows-style: directories in PATH order,
/// and within a directory `.exe`/`.com` before `.cmd`/`.bat`. Only extensions
/// in `pathext` that CreateProcess can run are tried. A name that already has
/// one of those extensions is looked up as is.
pub fn find_on_path(name: &str, path_var: &OsStr, pathext: &str) -> Option<PathBuf> {
    let lower = name.to_ascii_lowercase();
    let exts: Vec<&str> = if RUNNABLE.iter().any(|e| lower.ends_with(e)) {
        vec![""]
    } else {
        let allowed: Vec<String> = pathext.split(';').map(|e| e.trim().to_ascii_lowercase()).collect();
        RUNNABLE.into_iter().filter(|e| allowed.iter().any(|a| a == e)).collect()
    };
    std::env::split_paths(path_var)
        .filter(|d| !d.as_os_str().is_empty())
        .find_map(|dir| {
            exts.iter()
                .map(|ext| dir.join(format!("{name}{ext}")))
                .find(|p| p.is_file())
        })
}

/// Decide how to launch `found`. `.cmd`/`.bat` shims that call an `.exe` or
/// `node <script>` are launched directly; anything else goes through cmd.exe.
pub fn resolve_launch(found: &Path, path_var: &OsStr, pathext: &str) -> LaunchSpec {
    let ext = found
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if ext != "cmd" && ext != "bat" {
        return LaunchSpec { kind: LaunchKind::Exe, program: found.to_path_buf(), args: vec![] };
    }
    let dir = found.parent().unwrap_or(Path::new("."));
    let text = std::fs::read(found).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    for target in shim::parse_cmd_shim(&text, dir) {
        match target {
            ShimTarget::Exe(exe) if exe.is_file() => {
                return LaunchSpec { kind: LaunchKind::Exe, program: exe, args: vec![] };
            }
            ShimTarget::Node(script) if script.is_file() => {
                let local = dir.join("node.exe");
                let node = if local.is_file() { Some(local) } else { find_on_path("node", path_var, pathext) };
                if let Some(node) = node {
                    let script = script.to_string_lossy().into_owned();
                    return LaunchSpec { kind: LaunchKind::Node, program: node, args: vec![script] };
                }
            }
            _ => {}
        }
    }
    let comspec = std::env::var_os("COMSPEC").map(PathBuf::from).unwrap_or_else(|| "cmd.exe".into());
    LaunchSpec {
        kind: LaunchKind::CmdShim,
        program: comspec,
        args: ["/d", "/s", "/c"].map(String::from).into_iter().chain([found.to_string_lossy().into_owned()]).collect(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    Ok(Option<String>),
    Rejected,
    TimedOut,
}

/// Run `version_args` and check the exit code and `version_regex`. Agents
/// without `version_args` are accepted without running anything.
pub fn probe(launch: &LaunchSpec, def: &AgentDef, timeout: Duration) -> Probe {
    if def.version_args.is_empty() {
        return Probe::Ok(None);
    }
    let mut cmd = launch.command(&def.version_args);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let Ok(mut child) = cmd.spawn() else { return Probe::Rejected };

    // Read the pipes on threads: a grandchild can hold them open after the
    // child exits, so never block on them past the deadline.
    let (tx, rx) = mpsc::channel();
    for pipe in [child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>), child.stderr.take().map(|p| Box::new(p) as _)] {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
        });
    }
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Probe::TimedOut;
            }
        }
    };
    let mut out = String::new();
    for _ in 0..2 {
        let left = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(200));
        match rx.recv_timeout(left) {
            Ok(s) => out.push_str(&s),
            Err(_) => break,
        }
    }
    if !status.success() {
        return Probe::Rejected;
    }
    if let Some(re) = &def.version_regex {
        match regex::Regex::new(re) {
            Ok(re) if re.is_match(&out) => {}
            _ => return Probe::Rejected,
        }
    }
    let version = out.lines().map(str::trim).find(|l| !l.is_empty()).map(|l| l.chars().take(80).collect());
    Probe::Ok(version)
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Cache {
    schema_version: u32,
    #[serde(default)]
    entries: HashMap<String, CacheEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheEntry {
    mtime_ms: u64,
    ok: bool,
    version: Option<String>,
}

fn mtime_ms(p: &Path) -> Option<u64> {
    let t = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64)
}

fn cache_key(launch: &LaunchSpec, def: &AgentDef) -> String {
    format!(
        "{}|{}|{}",
        launch.target().display(),
        def.version_args.join(" "),
        def.version_regex.as_deref().unwrap_or("")
    )
}

/// Serialises detection so concurrent callers do not race on the cache file.
static DETECT_LOCK: Mutex<()> = Mutex::new(());

/// Detect every agent in `defs`, in registry order. Each agent's `commands`
/// are tried in order; the first that is found and passes the probe wins.
pub fn detect_all(defs: &[AgentDef], config_dir: Option<&Path>) -> Vec<DetectedAgent> {
    let _guard = DETECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let cache_path = config_dir.map(|d| d.join(CACHE_FILE));
    let cache: Cache = cache_path
        .as_ref()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let cache = Mutex::new(cache);
    let dirty = std::sync::atomic::AtomicBool::new(false);

    let found: Vec<Option<DetectedAgent>> = std::thread::scope(|s| {
        let handles: Vec<_> = defs
            .iter()
            .map(|def| {
                let (path_var, pathext, cache, dirty) = (&path_var, &pathext, &cache, &dirty);
                s.spawn(move || {
                    def.commands.iter().find_map(|name| {
                        let path = find_on_path(name, path_var, pathext)?;
                        let launch = resolve_launch(&path, path_var, pathext);
                        let key = cache_key(&launch, def);
                        let mtime = mtime_ms(launch.target());
                        let cached = cache.lock().unwrap().entries.get(&key).cloned();
                        let (ok, version) = match cached {
                            Some(e) if Some(e.mtime_ms) == mtime => (e.ok, e.version),
                            _ => {
                                let result = probe(&launch, def, PROBE_TIMEOUT);
                                let timed_out = result == Probe::TimedOut;
                                let (ok, version) = match result {
                                    Probe::Ok(v) => (true, v),
                                    Probe::Rejected | Probe::TimedOut => (false, None),
                                };
                                // A timeout may be a slow cold start, so do not cache it.
                                if let (Some(mtime_ms), false) = (mtime, timed_out) {
                                    let entry = CacheEntry { mtime_ms, ok, version: version.clone() };
                                    cache.lock().unwrap().entries.insert(key, entry);
                                    dirty.store(true, std::sync::atomic::Ordering::Relaxed);
                                }
                                (ok, version)
                            }
                        };
                        ok.then(|| DetectedAgent { def: def.clone(), path, launch, version })
                    })
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().ok().flatten()).collect()
    });

    if let (Some(p), true) = (cache_path, dirty.into_inner()) {
        let mut cache = cache.into_inner().unwrap();
        cache.schema_version = 1;
        if let Err(e) = write_atomic(&p, &serde_json::to_vec_pretty(&cache).unwrap_or_default()) {
            eprintln!("registry: could not write {}: {e}", p.display());
        }
    }
    found.into_iter().flatten().collect()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::ffi::OsString;

    /// A fresh empty directory under the system temp dir.
    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("spectacli-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn path_of(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    const PATHEXT: &str = ".COM;.EXE;.BAT;.CMD;.VBS;.JS";

    #[test]
    fn path_order_wins_and_exe_beats_cmd_in_one_dir() {
        let root = temp_dir("which");
        let (a, b) = (root.join("a"), root.join("b"));
        touch(&a.join("tool.cmd"), "");
        touch(&a.join("tool.exe"), "");
        touch(&b.join("tool.exe"), "");
        touch(&b.join("other.js"), "");
        touch(&b.join("bare"), "");
        let p = path_of(&[&a, &b]);
        assert_eq!(find_on_path("tool", &p, PATHEXT), Some(a.join("tool.exe")));
        assert_eq!(find_on_path("tool.cmd", &p, PATHEXT), Some(a.join("tool.cmd")));
        assert_eq!(find_on_path("other", &p, PATHEXT), None, ".js is not runnable");
        assert_eq!(find_on_path("bare", &p, PATHEXT), None, "extensionless sh shims are skipped");
        assert_eq!(find_on_path("tool", &p, ".CMD"), Some(a.join("tool.cmd")), "PATHEXT limits extensions");
        assert_eq!(find_on_path("missing", &p, PATHEXT), None);
    }

    #[test]
    fn resolves_exe_and_node_shims_and_falls_back_to_cmd() {
        let root = temp_dir("resolve");
        let bin = root.join("npm");
        let exe = bin.join(r"node_modules\x\bin\x.exe");
        touch(&exe, "");
        touch(&bin.join("x.cmd"), "@ECHO off\r\n\"%dp0%\\node_modules\\x\\bin\\x.exe\"   %*\r\n");
        let p = path_of(&[&bin]);
        let spec = resolve_launch(&bin.join("x.cmd"), &p, PATHEXT);
        assert_eq!((spec.kind, spec.program, spec.args.len()), (LaunchKind::Exe, exe, 0));

        let script = bin.join(r"node_modules\y\y.js");
        touch(&script, "");
        touch(&bin.join("node.exe"), "");
        touch(&bin.join("y.cmd"), "\"%_prog%\"  \"%dp0%\\node_modules\\y\\y.js\" %*\r\n");
        let spec = resolve_launch(&bin.join("y.cmd"), &p, PATHEXT);
        assert_eq!(spec.kind, LaunchKind::Node);
        assert_eq!(spec.program, bin.join("node.exe"));
        assert_eq!(spec.args, [script.to_string_lossy()]);

        // Target missing on disk: fall back to cmd.exe.
        touch(&bin.join("z.cmd"), "\"%dp0%\\gone.exe\" %*\r\n");
        let spec = resolve_launch(&bin.join("z.cmd"), &p, PATHEXT);
        assert_eq!(spec.kind, LaunchKind::CmdShim);
        assert_eq!(spec.args.last().unwrap(), &bin.join("z.cmd").to_string_lossy());
    }

    fn def(args: &[&str], regex: Option<&str>) -> AgentDef {
        AgentDef {
            id: "t".into(),
            name: "t".into(),
            commands: vec!["t".into()],
            version_args: args.iter().map(|s| s.to_string()).collect(),
            version_regex: regex.map(String::from),
            new_session_args: vec![],
            resume_args: vec![],
            continue_args: vec![],
            hook: None,
            config_dir_env: None,
            image_paste: None,
            submit: "\r".into(),
        }
    }

    fn batch(name: &str, body: &str) -> LaunchSpec {
        let dir = temp_dir(name);
        let shim = dir.join(format!("{name} shim.cmd")); // space exercises cmd quoting
        touch(&shim, &format!("@echo off\r\n{body}\r\n"));
        resolve_launch(&shim, OsStr::new(""), PATHEXT)
    }

    #[test]
    fn probe_accepts_matching_version_and_rejects_impostors() {
        let real = batch("real", "echo tool 1.2.3 %1");
        assert_eq!(real.kind, LaunchKind::CmdShim);
        assert_eq!(
            probe(&real, &def(&["--version"], Some(r"tool \d+\.\d+")), PROBE_TIMEOUT),
            Probe::Ok(Some("tool 1.2.3 --version".into()))
        );
        assert_eq!(probe(&real, &def(&["--version"], Some("other")), PROBE_TIMEOUT), Probe::Rejected);

        // Like VS Code's copilot shim: prints a message and exits non-zero.
        let fake = batch("fake", "echo Tool 1.0.0 was not found\r\nexit /b 127");
        assert_eq!(probe(&fake, &def(&["--version"], None), PROBE_TIMEOUT), Probe::Rejected);

        assert_eq!(probe(&fake, &def(&[], None), PROBE_TIMEOUT), Probe::Ok(None), "no probe configured");
    }

    #[test]
    fn probe_times_out() {
        let slow = batch("slow", "ping -n 6 127.0.0.1 >nul");
        let start = Instant::now();
        assert_eq!(probe(&slow, &def(&["x"], None), Duration::from_millis(300)), Probe::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
