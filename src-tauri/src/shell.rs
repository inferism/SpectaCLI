//! Which shell runs custom commands (pwsh, else Windows PowerShell), and how
//! to start things inside WSL (PLAN.md feature 8).

use crate::registry::detect::find_on_path;
use crate::registry::{LaunchKind, LaunchSpec};
use std::ffi::OsStr;
use std::path::PathBuf;

/// PowerShell 7 (`pwsh`) when it is on PATH, otherwise Windows PowerShell.
pub fn powershell(path_var: &OsStr, pathext: &str) -> PathBuf {
    if let Some(p) = find_on_path("pwsh", path_var, pathext) {
        return p;
    }
    if let Some(p) = find_on_path("powershell", path_var, pathext) {
        return p;
    }
    // Always present on a Windows install, even with a stripped PATH.
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    PathBuf::from(root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe")
}

/// Run a user-typed command line through PowerShell.
pub fn custom_command(command: &str, path_var: &OsStr, pathext: &str) -> LaunchSpec {
    LaunchSpec {
        kind: LaunchKind::Exe,
        program: powershell(path_var, pathext),
        args: vec!["-NoLogo".into(), "-Command".into(), command.into()],
    }
}

/// `C:\Users\me\x` -> `/mnt/c/Users/me/x`; `\\wsl$\Ubuntu\home\me` (or
/// `\\wsl.localhost\...`) -> `/home/me`. Anything else just gets forward slashes.
pub fn to_wsl_path(p: &str) -> String {
    let lower = p.to_ascii_lowercase();
    for unc in [r"\\wsl$\", r"\\wsl.localhost\"] {
        if lower.starts_with(unc) {
            // Drop the distro name: it is passed with `-d`.
            let rest = &p[unc.len()..];
            return match rest.find(['\\', '/']) {
                Some(i) => rest[i..].replace('\\', "/"),
                None => "/".into(),
            };
        }
    }
    let b = p.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        let drive = (b[0] as char).to_ascii_lowercase();
        let rest = p[2..].replace('\\', "/");
        let rest = rest.trim_end_matches('/');
        return if rest.is_empty() { format!("/mnt/{drive}") } else { format!("/mnt/{drive}/{}", rest.trim_start_matches('/')) };
    }
    p.replace('\\', "/")
}

/// `wsl.exe -d <distro> --cd <wslpath> [-- sh -lc <command>]`: a login shell
/// in `cwd`, or the command run by one.
pub fn wsl_launch(distro: &str, command: Option<&str>, cwd: &str) -> LaunchSpec {
    let mut args: Vec<String> = vec!["-d".into(), distro.into(), "--cd".into(), to_wsl_path(cwd)];
    if let Some(c) = command.map(str::trim).filter(|c| !c.is_empty()) {
        args.extend(["--".into(), "sh".into(), "-lc".into(), c.into()]);
    }
    LaunchSpec { kind: LaunchKind::Exe, program: PathBuf::from("wsl.exe"), args }
}

/// `wsl -l -q` prints UTF-16LE. Returns one name per line, no blanks.
pub fn parse_distros(raw: &[u8]) -> Vec<String> {
    let units: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    String::from_utf16_lossy(&units)
        .lines()
        .map(|l| l.trim_matches(|c: char| c == '\u{feff}' || c == '\0' || c.is_whitespace()))
        // The default distro is marked "(Default)" by the verbose list only, but be safe.
        .map(|l| l.trim_end_matches("(Default)").trim())
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// Installed WSL distributions; empty when WSL is missing.
pub fn list_distros() -> Vec<String> {
    let mut cmd = std::process::Command::new("wsl.exe");
    cmd.args(["-l", "-q"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    match cmd.output() {
        Ok(o) if o.status.success() => parse_distros(&o.stdout),
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_paths_go_under_mnt() {
        assert_eq!(to_wsl_path(r"C:\Users\me\proj"), "/mnt/c/Users/me/proj");
        assert_eq!(to_wsl_path(r"d:\"), "/mnt/d");
        assert_eq!(to_wsl_path(r"C:/a/b/"), "/mnt/c/a/b");
    }

    #[test]
    fn wsl_unc_paths_become_linux_paths() {
        assert_eq!(to_wsl_path(r"\\wsl$\Ubuntu\home\me"), "/home/me");
        assert_eq!(to_wsl_path(r"\\wsl.localhost\Debian\srv"), "/srv");
        assert_eq!(to_wsl_path(r"\\wsl$\Ubuntu"), "/");
    }

    #[test]
    fn launch_args() {
        let shell = wsl_launch("Ubuntu", None, r"C:\p");
        assert_eq!(shell.args, ["-d", "Ubuntu", "--cd", "/mnt/c/p"]);
        let cmd = wsl_launch("Ubuntu", Some(" claude "), r"C:\p");
        assert_eq!(cmd.args, ["-d", "Ubuntu", "--cd", "/mnt/c/p", "--", "sh", "-lc", "claude"]);
    }

    #[test]
    fn distro_list_decodes_utf16() {
        let text = "\u{feff}Ubuntu\r\nDebian\r\n\r\n";
        let raw: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(parse_distros(&raw), ["Ubuntu", "Debian"]);
        assert!(parse_distros(b"").is_empty());
    }

    #[test]
    fn powershell_prefers_pwsh() {
        let dir = std::env::temp_dir().join(format!("spectacli-sh-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = std::env::join_paths([&dir]).unwrap();
        // Neither on this PATH: falls back to the System32 copy.
        assert!(powershell(&path, ".EXE").ends_with("powershell.exe"));
        std::fs::write(dir.join("pwsh.exe"), "").unwrap();
        assert_eq!(powershell(&path, ".EXE"), dir.join("pwsh.exe"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
