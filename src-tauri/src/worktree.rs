//! One git worktree per agent (PLAN.md feature 7). Spectacli only creates and
//! removes them; merging is the user's call. Uses the `git` on PATH.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

/// Prefix of an error meaning "has uncommitted changes"; the UI confirms and retries with `force`.
pub const DIRTY: &str = "dirty:";

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().map_err(|e| format!("could not run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if msg.is_empty() { format!("git {} failed", args.join(" ")) } else { msg })
    }
}

/// Lowercase letters, digits and `-`, so it is a safe folder and branch name.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "agent".into() } else { out }
}

/// Keep `.spectacli/` out of the user's repo through `.git/info/exclude`
/// (local, never committed); their `.gitignore` is not touched.
fn exclude_spectacli(project: &Path) -> Result<(), String> {
    let common = git(project, &["rev-parse", "--git-common-dir"])?;
    let dir = if Path::new(&common).is_absolute() { PathBuf::from(common) } else { project.join(common) };
    let file = dir.join("info").join("exclude");
    let current = std::fs::read_to_string(&file).unwrap_or_default();
    if current.lines().any(|l| matches!(l.trim(), ".spectacli" | ".spectacli/" | "/.spectacli/")) {
        return Ok(());
    }
    std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
    let sep = if current.is_empty() || current.ends_with('\n') { "" } else { "\n" };
    std::fs::write(&file, format!("{current}{sep}.spectacli/\n")).map_err(|e| e.to_string())
}

/// `git worktree add <project>/.spectacli/worktrees/<slug> -b spectacli/<slug>`.
/// `slug` is made unique by the caller.
pub fn create(project: &Path, slug_in: &str) -> Result<Worktree, String> {
    let slug = slug(slug_in);
    git(project, &["rev-parse", "--is-inside-work-tree"]).map_err(|_| "this folder is not a git repository".to_string())?;
    let path = project.join(".spectacli").join("worktrees").join(&slug);
    let branch = format!("spectacli/{slug}");
    // Best effort: a failure here must not block the worktree itself.
    if let Err(e) = exclude_spectacli(project) {
        eprintln!("worktree: could not update .git/info/exclude: {e}");
    }
    git(project, &["worktree", "add", &path.to_string_lossy(), "-b", &branch])?;
    Ok(Worktree { path, branch })
}

/// Number of changed (modified or untracked) files in a worktree.
pub fn changes(path: &Path) -> Result<usize, String> {
    Ok(git(path, &["status", "--porcelain"])?.lines().count())
}

/// Remove a worktree. Refuses (error starting with `DIRTY`) when it has
/// uncommitted changes unless `force`. The branch is kept: it holds the work.
pub fn remove(path: &Path, force: bool) -> Result<(), String> {
    if !force {
        let n = changes(path)?;
        if n > 0 {
            return Err(format!("{DIRTY} {n} changed files"));
        }
    }
    // Run from the main working tree: git cannot remove the tree it is in.
    let main = main_worktree(path)?;
    let p = path.to_string_lossy();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&p);
    git(&main, &args).map(|_| ())
}

/// The first entry of `git worktree list`: the repository's own working tree.
fn main_worktree(any: &Path) -> Result<PathBuf, String> {
    let list = git(any, &["worktree", "list", "--porcelain"])?;
    list.lines()
        .find_map(|l| l.strip_prefix("worktree "))
        .map(PathBuf::from)
        .ok_or_else(|| "could not find the main working tree".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_a_safe_name() {
        assert_eq!(slug("Claude Code 1a2b"), "claude-code-1a2b");
        assert_eq!(slug("  --Weird//Name!! "), "weird-name");
        assert_eq!(slug("!!!"), "agent");
    }

    fn run(dir: &Path, args: &[&str]) {
        assert!(git(dir, args).is_ok(), "git {args:?}");
    }

    #[test]
    fn create_and_remove_in_a_real_repo() {
        let dir = std::env::temp_dir().join(format!("spectacli-wt-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q"]);
        run(&dir, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init"]);

        let wt = create(&dir, "Claude 1").unwrap();
        assert_eq!(wt.branch, "spectacli/claude-1");
        assert!(wt.path.join(".git").exists());
        let exclude = std::fs::read_to_string(dir.join(".git/info/exclude")).unwrap();
        assert!(exclude.contains(".spectacli/"));
        // Creating again does not duplicate the exclude line.
        create(&dir, "second").unwrap();
        let exclude = std::fs::read_to_string(dir.join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.matches(".spectacli/").count(), 1);

        std::fs::write(wt.path.join("x.txt"), "hi").unwrap();
        assert_eq!(changes(&wt.path).unwrap(), 1);
        let err = remove(&wt.path, false).unwrap_err();
        assert!(err.starts_with(DIRTY), "{err}");
        remove(&wt.path, true).unwrap();
        assert!(!wt.path.exists());

        let clean = dir.join(".spectacli/worktrees/second");
        remove(&clean, false).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn not_a_repo_is_a_clear_error() {
        let dir = std::env::temp_dir().join(format!("spectacli-nogit-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = create(&dir, "x").unwrap_err();
        assert_eq!(err, "this folder is not a git repository");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
