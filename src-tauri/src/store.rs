//! Versioned JSON files in the config dir (`projects.json` for now).
//!
//! The data is kept as raw JSON so fields this version doesn't know about
//! survive a load/save round trip. The frontend owns the shape.

use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SCHEMA_VERSION: u64 = 1;

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn parse(bytes: &[u8]) -> Result<Value, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    match v.get("schemaVersion").and_then(Value::as_u64) {
        Some(n) if n <= SCHEMA_VERSION => Ok(v),
        Some(n) => Err(format!("schemaVersion {n} is newer than this app supports ({SCHEMA_VERSION})")),
        None => Err("missing schemaVersion".into()),
    }
}

/// `Ok(None)` when there is no file yet. A file that fails to parse is moved
/// to `*.corrupt-<ts>` and the last good copy (`*.bak`) is used instead. A
/// file from a newer version is an error, so the caller never overwrites it.
pub fn load(path: &Path) -> Result<Option<Value>, String> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    match parse(&bytes) {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.contains("newer") => Err(e),
        Err(_) => {
            let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            fs::rename(path, sibling(path, &format!(".corrupt-{ts}"))).map_err(|e| format!("set aside corrupt file: {e}"))?;
            Ok(fs::read(sibling(path, ".bak")).ok().and_then(|b| parse(&b).ok()))
        }
    }
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
        let _ = fs::copy(path, sibling(path, ".bak"));
    }
    fs::rename(&tmp, path).map_err(|e| format!("replace {}: {e}", path.display()))
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

    #[test]
    fn missing_file_is_none() {
        assert_eq!(load(&temp_file()).unwrap(), None);
    }

    #[test]
    fn round_trip_keeps_unknown_fields() {
        let p = temp_file();
        let v = json!({ "schemaVersion": 1, "projects": [], "fromTheFuture": { "x": 1 } });
        save(&p, &v).unwrap();
        assert_eq!(load(&p).unwrap(), Some(v));
        assert!(!sibling(&p, ".tmp").exists());
    }

    #[test]
    fn corrupt_file_is_set_aside_and_backup_used() {
        let p = temp_file();
        let first = json!({ "schemaVersion": 1, "n": 1 });
        save(&p, &first).unwrap();
        save(&p, &json!({ "schemaVersion": 1, "n": 2 })).unwrap(); // .bak now holds n=1
        fs::write(&p, b"{ not json").unwrap();
        assert_eq!(load(&p).unwrap(), Some(first));
        assert!(!p.exists());
        let set_aside = fs::read_dir(p.parent().unwrap())
            .unwrap()
            .any(|e| e.unwrap().file_name().to_string_lossy().starts_with("projects.json.corrupt-"));
        assert!(set_aside);
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
}
