//! Panic hook that appends to `<data>/logs/panic-YYYY-MM-DD.log` (PLAN.md
//! 3.9). One file per UTC day; only the newest `KEEP` files are kept.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const KEEP: usize = 5;
const PREFIX: &str = "panic-";

/// Install the hook. The previous hook (stderr) still runs after logging.
pub fn install(dir: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let entry = format!(
            "[{}] thread '{}' panicked: {}\n{}\n\n",
            timestamp(now_secs()),
            thread.name().unwrap_or("<unnamed>"),
            info,
            std::backtrace::Backtrace::force_capture(),
        );
        // Logging must never panic inside the hook.
        let _ = write_entry(&dir, now_secs(), &entry);
        previous(info);
    }));
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn write_entry(dir: &Path, secs: u64, entry: &str) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{PREFIX}{}.log", date(secs)));
    OpenOptions::new().create(true).append(true).open(path)?.write_all(entry.as_bytes())?;
    prune(dir)
}

/// Delete all but the newest `KEEP` panic logs. Names sort by date.
fn prune(dir: &Path) -> std::io::Result<()> {
    let mut logs: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(PREFIX) && n.ends_with(".log"))
        })
        .collect();
    logs.sort();
    let excess = logs.len().saturating_sub(KEEP);
    for old in &logs[..excess] {
        let _ = fs::remove_file(old);
    }
    Ok(())
}

/// UTC `YYYY-MM-DD` for Unix seconds (Howard Hinnant's civil-from-days).
fn date(secs: u64) -> String {
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn timestamp(secs: u64) -> String {
    let t = secs % 86_400;
    format!("{} {:02}:{:02}:{:02}Z", date(secs), t / 3600, t / 60 % 60, t % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(1_794_527_999), "2026-11-12");
        assert_eq!(timestamp(1_794_527_999), "2026-11-12 23:59:59Z");
    }

    #[test]
    fn appends_per_day_and_keeps_five() {
        let dir = std::env::temp_dir().join(format!("spectacli-panic-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("other.txt"), "keep me").unwrap();
        for day in 0..7u64 {
            write_entry(&dir, day * 86_400, "a\n").unwrap();
        }
        write_entry(&dir, 6 * 86_400 + 5, "b\n").unwrap();

        let mut names: Vec<String> =
            fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        assert_eq!(
            names,
            [
                "other.txt",
                "panic-1970-01-03.log",
                "panic-1970-01-04.log",
                "panic-1970-01-05.log",
                "panic-1970-01-06.log",
                "panic-1970-01-07.log",
            ]
        );
        assert_eq!(fs::read_to_string(dir.join("panic-1970-01-07.log")).unwrap(), "a\nb\n");
        fs::remove_dir_all(&dir).unwrap();
    }
}
