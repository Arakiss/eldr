//! Housekeeping: keep eldr's data dir from growing without bound. The append-only logs
//! (alerts/actions/processes/guard/scrub) are forensic — useful for a while, prunable
//! after — so they're capped to their most recent lines. The scrub manifest is *state*,
//! not a log, and is never touched here. A size check lets the guard warn if the dir
//! gets large (usually that means a manifest over a huge, many-file volume).

use crate::config;
use crate::sensors::snapshot::SCHEMA_VERSION;
use std::fs;
use std::path::Path;

/// The append-only logs eldr writes; each is capped independently.
const LOGS: &[&str] = &[
    "alerts.log",
    "actions.log",
    "processes.log",
    "guard.log",
    "scrub.log",
];

/// Keep at most this many recent lines per log (history.csv is already self-capping).
const LOG_CAP_LINES: usize = 2000;

/// Default warn threshold for the whole data dir, overridable via `ELDR_DATA_WARN_MB`.
const DEFAULT_WARN_MB: i64 = 500;

/// `eldr prune` — cap the logs now and report what was freed and the dir's current size.
pub fn prune(json: bool) -> i32 {
    let freed = rotate_logs();
    let size = dir_size(&config::data_dir());
    if json {
        println!(
            "{{\"schema_version\":\"{SCHEMA_VERSION}\",\"action\":\"prune\",\"freed_bytes\":{freed},\"data_dir_bytes\":{size}}}"
        );
    } else {
        println!(
            "pruned logs: freed {} · data dir now {}",
            human(freed),
            human(size),
        );
    }
    0
}

/// Cap every append-only log to its last [`LOG_CAP_LINES`] lines. Returns bytes freed.
pub fn rotate_logs() -> u64 {
    let dir = config::data_dir();
    let mut freed = 0u64;
    for name in LOGS {
        let path = dir.join(name);
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(capped) = cap_lines(&content, LOG_CAP_LINES) {
            let before = content.len() as u64;
            // Write via a temp file + rename so a reader never sees a half-written log.
            let tmp = dir.join(format!("{name}.tmp"));
            if fs::write(&tmp, &capped).is_ok() && fs::rename(&tmp, &path).is_ok() {
                freed += before.saturating_sub(capped.len() as u64);
            }
        }
    }
    freed
}

/// The data dir's total size if it exceeds the configured threshold, else `None`.
pub fn over_threshold() -> Option<u64> {
    let mb = config::Config::load()
        .int("ELDR_DATA_WARN_MB", DEFAULT_WARN_MB)
        .max(0) as u64;
    let limit = mb.saturating_mul(1024 * 1024);
    let size = dir_size(&config::data_dir());
    if limit > 0 && size > limit {
        Some(size)
    } else {
        None
    }
}

/// Keep only the last `max` lines of `content`. `None` when it's already within the cap
/// (so the caller can skip rewriting an unchanged file).
fn cap_lines(content: &str, max: usize) -> Option<String> {
    let total = content.lines().count();
    if total <= max {
        return None;
    }
    let kept: Vec<&str> = content.lines().skip(total - max).collect();
    Some(format!("{}\n", kept.join("\n")))
}

/// Total bytes of every file under `dir` (recursive, symlinks not followed).
pub fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else {
            continue;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            match fs::symlink_metadata(&p) {
                Ok(m) if m.file_type().is_dir() => stack.push(p),
                Ok(m) if m.file_type().is_file() => total += m.len(),
                _ => {}
            }
        }
    }
    total
}

fn human(bytes: u64) -> String {
    crate::ui::style::human_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_keeps_the_most_recent_lines() {
        let content: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let capped = cap_lines(&content, 10).expect("100 > 10 must trim");
        let lines: Vec<&str> = capped.lines().collect();
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[0], "line 90"); // oldest kept
        assert_eq!(lines[9], "line 99"); // newest kept
    }

    #[test]
    fn cap_is_a_noop_within_the_limit() {
        let content = "a\nb\nc\n";
        assert!(cap_lines(content, 10).is_none());
        assert!(cap_lines(content, 3).is_none());
    }
}
