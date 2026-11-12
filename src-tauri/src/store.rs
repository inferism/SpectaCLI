//! Versioned JSON files in the config dir (`projects.json` for now).
//!
//! The data is kept as raw JSON so fields this version doesn't know about
//! survive a load/save round trip. The frontend owns the shape.
//!
//! Safety net, in order of use:
//! - every save is atomic (`*.tmp`, fsync, rename) and keeps the previous
//!   file as `*.bak`;
//! - an older file is copied to `*.v{N}.bak` before it is migrated;
//! - a file that does not parse is moved to `*.corrupt-<ts>` and the `.bak`
//!   is used instead, with a notice for the user;
//! - a file from a newer version is never touched.

use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const SCHEMA_VERSION: u64 = 1;

/// Turns a file of version N into version N+1. The runner sets
/// `schemaVersion`; a migration only reshapes the data. It must keep fields
/// it does not know, so data written by newer versions survives.
pub type Migration = fn(Value) -> Result<Value, String>;

/// `MIGRATIONS[i]` migrates version `i + 1` to `i + 2`. Append only.
const MIGRATIONS: &[Migration] = &[];
const _: () = assert!(MIGRATIONS.len() as u64 + 1 == SCHEMA_VERSION, "one migration per schema bump");

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Loaded {
    pub data: Value,
    /// Set when the file had to be recovered; show it to the user.
    pub notice: Option<String>,
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

enum Bad {
    /// From a newer app version: leave it alone.
    Newer(String),
    /// Not usable: set it aside.
    Corrupt,
}

fn parse(bytes: &[u8], current: u64) -> Result<(Value, u64), Bad> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| Bad::Corrupt)?;
    if !v.is_object() {
        return Err(Bad::Corrupt);
    }
    match v.get("schemaVersion").and_then(Value::as_u64) {
        Some(n @ 1..) if n <= current => Ok((v, n)),
        Some(n) if n > current => {
            Err(Bad::Newer(format!("schemaVersion {n} is newer than this app supports ({current})")))
        }
        _ => Err(Bad::Corrupt),
    }
}

fn migrate(mut v: Value, from: u64, migrations: &[Migration]) -> Result<Value, String> {
    for n in from..=migrations.len() as u64 {
        v = migrations[n as usize - 1](v).map_err(|e| format!("migrate v{n} to v{}: {e}", n + 1))?;
        match v.as_object_mut() {
            Some(o) => o.insert("schemaVersion".into(), (n + 1).into()),
            None => return Err(format!("migrate v{n} to v{}: result is not an object", n + 1)),
        };
    }
    Ok(v)
}

/// `Ok(None)` when there is no file yet.
pub fn load(path: &Path) -> Result<Option<Loaded>, String> {
    load_with(path, MIGRATIONS)
}

fn load_with(path: &Path, migrations: &[Migration]) -> Result<Option<Loaded>, String> {
    let current = migrations.len() as u64 + 1;
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    match parse(&bytes, current) {
        Ok((v, n)) => {
            if n < current {
                // Kept for a downgrade, or in case the migration turns out wrong.
                let keep = sibling(path, &format!(".v{n}.bak"));
                fs::copy(path, &keep).map_err(|e| format!("back up {} before migrating: {e}", keep.display()))?;
            }
            // A failed migration is an error, so the caller never overwrites the file.
            Ok(Some(Loaded { data: migrate(v, n, migrations)?, notice: None }))
        }
        Err(Bad::Newer(e)) => Err(e),
        Err(Bad::Corrupt) => {
            // Settle the recovery before touching anything: if the backup is
            // from a newer version or fails to migrate, both files stay as
            // they are and the caller never saves over them.
            let backup = match fs::read(sibling(path, ".bak")).map(|b| parse(&b, current)) {
                Ok(Ok((v, n))) => Some(migrate(v, n, migrations)?),
                Ok(Err(Bad::Newer(e))) => return Err(format!("{} is unreadable and its backup: {e}", file_name(path))),
                Ok(Err(Bad::Corrupt)) | Err(_) => None,
            };
            let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let aside = sibling(path, &format!(".corrupt-{ts}"));
            fs::rename(path, &aside).map_err(|e| format!("set aside corrupt file: {e}"))?;
            let (data, outcome) = match backup {
                Some(v) => (v, "the last good copy was restored"),
                None => (serde_json::json!({ "schemaVersion": current }), "no backup was found, so it starts empty"),
            };
            let notice = format!("{} could not be read. It was moved to {} and {outcome}.", file_name(path), file_name(&aside));
            Ok(Some(Loaded { data, notice: Some(notice) }))
        }
    }
}

/// Antivirus and indexers briefly hold files open, which makes a rename fail.
fn rename_retrying(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut last = None;
    for delay in [0, 20, 50, 100, 250] {
        std::thread::sleep(Duration::from_millis(delay));
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            // 5 access denied, 32 sharing violation.
            Err(e) if matches!(e.raw_os_error(), Some(5 | 32)) => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.expect("at least one attempt"))
}

/// Atomic replace: write `*.tmp`, fsync, keep the old file as `*.bak`, rename.
pub fn save(path: &Path, data: &Value) -> Result<(), String> {
    match data.get("schemaVersion").and_then(Value::as_u64) {
        Some(SCHEMA_VERSION) => {}
        other => return Err(format!("refusing to save schemaVersion {other:?}")),
    }
    let bytes = serde_json::to_vec_pretty(data).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let tmp = sibling(path, ".tmp");
    let mut f = fs::File::create(&tmp).map_err(|e| format!("create {}: {e}", tmp.display()))?;
    f.write_all(&bytes).and_then(|_| f.sync_all()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    drop(f);
    if path.exists() {
        // Copied then renamed, so a crash never leaves a half-written .bak.
        let bak_tmp = sibling(path, ".bak.tmp");
        if fs::copy(path, &bak_tmp).is_ok() {
            let _ = rename_retrying(&bak_tmp, &sibling(path, ".bak"));
        }
    }
    rename_retrying(&tmp, path).map_err(|e| format!("replace {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_file() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spectacli-store-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&dir).unwrap();
        dir.join("projects.json")
    }

    fn data(l: Option<Loaded>) -> Value {
        l.expect("a file").data
    }

    fn names(p: &Path) -> Vec<String> {
        fs::read_dir(p.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn missing_file_is_none() {
        assert_eq!(load(&temp_file()).unwrap(), None);
    }

    #[test]
    fn round_trip_keeps_unknown_fields() {
        let p = temp_file();
        let v = json!({ "schemaVersion": 1, "projects": [], "fromTheFuture": { "x": 1 } });
        save(&p, &v).unwrap();
        save(&p, &v).unwrap();
        assert_eq!(load(&p).unwrap(), Some(Loaded { data: v, notice: None }));
        assert!(!sibling(&p, ".tmp").exists());
        assert!(!sibling(&p, ".bak.tmp").exists());
        assert!(sibling(&p, ".bak").exists());
    }

    #[test]
    fn corrupt_file_is_set_aside_and_backup_used() {
        let p = temp_file();
        let first = json!({ "schemaVersion": 1, "n": 1 });
        save(&p, &first).unwrap();
        save(&p, &json!({ "schemaVersion": 1, "n": 2 })).unwrap(); // .bak now holds n=1
        fs::write(&p, b"{ not json").unwrap();
        let l = load(&p).unwrap().unwrap();
        assert_eq!(l.data, first);
        assert!(l.notice.unwrap().contains("last good copy"));
        assert!(!p.exists());
        assert!(names(&p).iter().any(|n| n.starts_with("projects.json.corrupt-")));
    }

    #[test]
    fn corrupt_file_without_backup_starts_empty_with_notice() {
        let p = temp_file();
        fs::write(&p, b"[1, 2]").unwrap();
        let l = load(&p).unwrap().unwrap();
        assert_eq!(l.data, json!({ "schemaVersion": 1 }));
        assert!(l.notice.unwrap().contains("starts empty"));
    }

    #[test]
    fn missing_or_zero_version_is_corrupt() {
        for body in [&br#"{ "projects": [] }"#[..], br#"{ "schemaVersion": 0 }"#] {
            let p = temp_file();
            fs::write(&p, body).unwrap();
            assert!(load(&p).unwrap().unwrap().notice.is_some());
        }
    }

    #[test]
    fn newer_version_is_an_error_and_left_alone() {
        let p = temp_file();
        fs::write(&p, br#"{ "schemaVersion": 99 }"#).unwrap();
        assert!(load(&p).is_err());
        assert!(p.exists());
    }

    #[test]
    fn save_rejects_wrong_version() {
        assert!(save(&temp_file(), &json!({ "projects": [] })).is_err());
    }

    fn rename_a_to_b(mut v: Value) -> Result<Value, String> {
        let o = v.as_object_mut().ok_or("not an object")?;
        if let Some(a) = o.remove("a") {
            o.insert("b".into(), a);
        }
        Ok(v)
    }

    fn add_c(mut v: Value) -> Result<Value, String> {
        v["c"] = json!(true);
        Ok(v)
    }

    fn fail(_: Value) -> Result<Value, String> {
        Err("nope".into())
    }

    #[test]
    fn migrations_run_in_order_and_keep_a_versioned_backup() {
        let p = temp_file();
        let original = br#"{ "schemaVersion": 1, "a": 5, "unknown": "kept" }"#;
        fs::write(&p, original).unwrap();
        let v = data(load_with(&p, &[rename_a_to_b, add_c]).unwrap());
        assert_eq!(v, json!({ "schemaVersion": 3, "b": 5, "c": true, "unknown": "kept" }));
        assert_eq!(fs::read(sibling(&p, ".v1.bak")).unwrap(), original);
    }

    #[test]
    fn migration_starts_from_the_files_version() {
        let p = temp_file();
        fs::write(&p, br#"{ "schemaVersion": 2, "a": 5 }"#).unwrap();
        // Only v2 -> v3 runs, so `a` is not renamed.
        let v = data(load_with(&p, &[rename_a_to_b, add_c]).unwrap());
        assert_eq!(v, json!({ "schemaVersion": 3, "a": 5, "c": true }));
        assert!(sibling(&p, ".v2.bak").exists());
    }

    #[test]
    fn current_version_is_not_migrated_or_backed_up() {
        let p = temp_file();
        fs::write(&p, br#"{ "schemaVersion": 2, "a": 5 }"#).unwrap();
        assert_eq!(data(load_with(&p, &[rename_a_to_b]).unwrap()), json!({ "schemaVersion": 2, "a": 5 }));
        assert!(!sibling(&p, ".v2.bak").exists());
    }

    #[test]
    fn failed_migration_is_an_error_and_leaves_the_file() {
        let p = temp_file();
        let original = br#"{ "schemaVersion": 1 }"#;
        fs::write(&p, original).unwrap();
        let e = load_with(&p, &[fail]).unwrap_err();
        assert!(e.contains("v1 to v2"), "{e}");
        assert_eq!(fs::read(&p).unwrap(), original);
    }

    #[test]
    fn older_backup_is_migrated_when_recovering() {
        let p = temp_file();
        fs::write(sibling(&p, ".bak"), br#"{ "schemaVersion": 1, "a": 1 }"#).unwrap();
        fs::write(&p, b"garbage").unwrap();
        let l = load_with(&p, &[rename_a_to_b]).unwrap().unwrap();
        assert_eq!(l.data, json!({ "schemaVersion": 2, "b": 1 }));
        assert!(l.notice.is_some());
    }

    #[test]
    fn unusable_backup_leaves_both_files_in_place() {
        // A backup from a newer version, and one whose migration fails.
        for (bak, migrations) in [
            (&br#"{ "schemaVersion": 9 }"#[..], &[][..] as &[Migration]),
            (br#"{ "schemaVersion": 1 }"#, &[fail as Migration][..]),
        ] {
            let p = temp_file();
            fs::write(sibling(&p, ".bak"), bak).unwrap();
            fs::write(&p, b"garbage").unwrap();
            assert!(load_with(&p, migrations).is_err());
            assert_eq!(fs::read(&p).unwrap(), b"garbage");
            assert_eq!(fs::read(sibling(&p, ".bak")).unwrap(), bak);
        }
    }
}
