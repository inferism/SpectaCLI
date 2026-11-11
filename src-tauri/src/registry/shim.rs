//! Parse npm/pnpm-style `.cmd` shims so the real program can be launched
//! directly, without `cmd.exe` in between (which costs RAM and makes Ctrl+C
//! prompt "Terminate batch job (Y/N)?").

use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShimTarget {
    /// The shim runs a native executable.
    Exe(PathBuf),
    /// The shim runs `node <script>`.
    Node(PathBuf),
}

/// Candidate targets of a `.cmd` shim, in the order they appear. Only lines
/// that forward arguments (`%*`) are considered, and on each line the last
/// quoted `.exe` (other than `node.exe`) or `.js` path is taken. Paths using
/// variables other than `%~dp0` / `%dp0%` are skipped. The caller picks the
/// first candidate that exists.
pub fn parse_cmd_shim(text: &str, shim_dir: &Path) -> Vec<ShimTarget> {
    let dir = format!("{}\\", shim_dir.display());
    text.lines()
        .filter(|line| line.contains("%*"))
        .filter_map(|line| {
            quoted(line)
                .filter_map(|tok| {
                    let expanded = tok.replace("%~dp0", &dir).replace("%dp0%", &dir);
                    if expanded.contains('%') {
                        return None;
                    }
                    let lower = expanded.to_ascii_lowercase();
                    let path = normalize(&expanded);
                    if lower.ends_with(".exe") && !lower.ends_with("node.exe") {
                        Some(ShimTarget::Exe(path))
                    } else if [".js", ".cjs", ".mjs"].iter().any(|e| lower.ends_with(e)) {
                        Some(ShimTarget::Node(path))
                    } else {
                        None
                    }
                })
                .last()
        })
        .collect()
}

/// The contents of each `"..."` pair on a line.
fn quoted(line: &str) -> impl Iterator<Item = &str> {
    line.split('"').skip(1).step_by(2)
}

/// Lexically collapse `.`, `..` and doubled separators.
fn normalize(p: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for c in Path::new(p).components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    const DIR: &str = r"C:\Users\me\AppData\Roaming\npm";

    #[test]
    fn npm_exe_shim() {
        // The shape of claude.cmd from npm.
        let text = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe\"   %*\r\n";
        assert_eq!(
            parse_cmd_shim(text, Path::new(DIR)),
            [ShimTarget::Exe(PathBuf::from(format!(
                r"{DIR}\node_modules\@anthropic-ai\claude-code\bin\claude.exe"
            )))]
        );
    }

    #[test]
    fn npm_node_shim() {
        // The shape of codex.cmd from npm.
        let text = r#"@ECHO off
SETLOCAL
CALL :find_dp0
IF EXIST "%dp0%\node.exe" (
  SET "_prog=%dp0%\node.exe"
) ELSE (
  SET "_prog=node"
  SET PATHEXT=%PATHEXT:;.JS;=;%
)
endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & "%_prog%"  "%dp0%\node_modules\@openai\codex\bin\codex.js" %*
"#;
        assert_eq!(
            parse_cmd_shim(text, Path::new(DIR)),
            [ShimTarget::Node(PathBuf::from(format!(
                r"{DIR}\node_modules\@openai\codex\bin\codex.js"
            )))]
        );
    }

    #[test]
    fn pnpm_style_shim_with_parent_dirs() {
        let text = r#"@IF EXIST "%~dp0\node.exe" (
  "%~dp0\node.exe"  "%~dp0\..\tool\bin\cli.js" %*
) ELSE (
  node  "%~dp0\..\tool\bin\cli.js" %*
)"#;
        let want = ShimTarget::Node(PathBuf::from(r"C:\Users\me\AppData\Roaming\tool\bin\cli.js"));
        assert_eq!(parse_cmd_shim(text, Path::new(DIR)), [want.clone(), want]);
    }

    #[test]
    fn unparseable_shims_give_nothing() {
        let dir = Path::new(DIR);
        assert!(parse_cmd_shim("@echo off\r\npython -m tool %*", dir).is_empty());
        assert!(parse_cmd_shim(r#""%LOCALAPPDATA%\x\tool.exe" %*"#, dir).is_empty());
        assert!(parse_cmd_shim(r#""%~dp0\tool.exe""#, dir).is_empty(), "no %* means no forwarding");
    }
}
