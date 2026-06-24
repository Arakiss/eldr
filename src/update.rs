//! New-version check and `eldr update`. eldr is offline by default; the automatic check
//! (guard notification, `now`/TUI hint) only runs when `ELDR_UPDATE_CHECK=1`. The explicit
//! `eldr update [--check]` command always reaches out. The network call shells out to
//! `curl` (a system tool, like `osascript`/`diskutil`) — no HTTP/TLS crate, keeping the
//! zero-dependency invariant. The result is cached so we never hit GitHub more than once a
//! day, and every failure is silent (offline, no curl, timeout → no check, no noise).

use crate::config;
use crate::sensors::host;
use std::process::Command;

const LATEST_URL: &str = "https://api.github.com/repos/Arakiss/eldr/releases/latest";
const MAX_AGE_SECS: u64 = 86_400; // re-check at most once a day
const CACHE_FILE: &str = "update_check.json";

/// The version this binary was built as (`Cargo.toml`).
pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Extract `tag_name` from the GitHub release JSON, stripping a leading `v`. Pure (no
/// network) so it's unit-testable.
pub fn parse_tag(json: &str) -> Option<String> {
    let key = "\"tag_name\"";
    let after = &json[json.find(key)? + key.len()..];
    let after = after[after.find(':')? + 1..].trim_start();
    // The value must be a JSON string; a `null`/number (draft or malformed release) yields
    // None instead of bleeding into the next field.
    if !after.starts_with('"') {
        return None;
    }
    let start = after.find('"')? + 1;
    let end = after[start..].find('"')? + start;
    let tag = after[start..end].trim().trim_start_matches('v');
    if tag.is_empty() {
        None
    } else {
        Some(tag.to_string())
    }
}

/// `(major, minor, patch)` from a version string; missing/garbage parts read as 0.
fn parts(v: &str) -> (u64, u64, u64) {
    let mut it = v.trim().trim_start_matches('v').split(['.', '-', '+', ' ']);
    let n = |o: Option<&str>| o.and_then(|x| x.parse().ok()).unwrap_or(0);
    (n(it.next()), n(it.next()), n(it.next()))
}

/// True when `latest` is a strictly newer version than `current` (numeric compare). Pure.
pub fn is_newer(latest: &str, current: &str) -> bool {
    parts(latest) > parts(current)
}

fn cache_path() -> std::path::PathBuf {
    config::data_dir().join(CACHE_FILE)
}

fn read_cache() -> Option<(u64, String)> {
    let txt = std::fs::read_to_string(cache_path()).ok()?;
    let ts = parse_tag_named(&txt, "\"ts\"")?.parse().ok()?;
    let latest = parse_tag_named(&txt, "\"latest\"")?;
    Some((ts, latest))
}

/// Tiny extractor for our own cache file (`"ts":N,"latest":"X"`). Handles both the quoted
/// `latest` and the bare-number `ts`.
fn parse_tag_named(json: &str, key: &str) -> Option<String> {
    let after = &json[json.find(key)? + key.len()..];
    let after = after[after.find(':')? + 1..].trim_start();
    if let Some(stripped) = after.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some(stripped[..end].to_string())
    } else {
        let end = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        Some(after[..end].to_string())
    }
}

fn write_cache(ts: u64, latest: &str) {
    let _ = config::ensure_data_dir();
    let body = format!("{{\"ts\":{ts},\"latest\":\"{latest}\"}}");
    let _ = std::fs::write(cache_path(), body);
}

/// Query GitHub for the latest release tag. `None` on any failure (offline, no `curl`,
/// timeout, parse error) — a version check must never disrupt a monitoring tool.
fn fetch() -> Option<String> {
    let out = Command::new("curl")
        .args([
            "-fsS",
            "--max-time",
            "4",
            "-H",
            "Accept: application/vnd.github+json",
            LATEST_URL,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_tag(&String::from_utf8_lossy(&out.stdout))
}

/// The latest version tag, cached for a day. `force` bypasses the cache (for the explicit
/// `eldr update`). `None` if the network check fails and there's no usable cache.
pub fn latest(force: bool) -> Option<String> {
    if !force
        && let Some((ts, latest)) = read_cache()
        && host::unix_time().saturating_sub(ts) < MAX_AGE_SECS
    {
        return Some(latest);
    }
    let latest = fetch()?;
    write_cache(host::unix_time(), &latest);
    Some(latest)
}

/// The newer version if one is available, else `None`. Hits the network (cached). For the
/// opt-in automatic check (guard); callers must gate on `ELDR_UPDATE_CHECK`.
pub fn newer_available(force: bool) -> Option<String> {
    let latest = latest(force)?;
    is_newer(&latest, current()).then_some(latest)
}

/// The newer version from the cache only — never touches the network. For passive hints
/// (`now`/TUI) that should stay offline; shows what a prior check already found.
pub fn cached_newer() -> Option<String> {
    let (_, latest) = read_cache()?;
    is_newer(&latest, current()).then_some(latest)
}

/// Whether this binary lives under a Homebrew prefix (its symlink resolves into a Cellar).
fn via_homebrew() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_string))
        .map(|p| p.contains("/Cellar/") || p.contains("/homebrew/"))
        .unwrap_or(false)
}

/// `eldr update [--check]`: report the current vs latest version and, unless `--check`,
/// update in place via Homebrew (or print the steps when installed from source).
pub fn run(args: &[String]) -> i32 {
    let check_only = args.iter().any(|a| a == "--check");
    let cur = current();
    match latest(true) {
        None => {
            eprintln!("eldr update: couldn't reach GitHub (offline?). You're on {cur}.");
            1
        }
        Some(latest) if is_newer(&latest, cur) => {
            println!("eldr {cur} → a newer version is available: {latest}");
            if check_only {
                println!("run `eldr update` to install it.");
                return 0;
            }
            if via_homebrew() {
                println!("updating via Homebrew…");
                let ok = Command::new("brew")
                    .args(["upgrade", "Arakiss/tap/eldr"])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if ok { 0 } else { 1 }
            } else {
                println!("eldr isn't installed via Homebrew. To update:");
                println!("  • Homebrew:  brew install Arakiss/tap/eldr");
                println!("  • from source:  git pull && make install");
                0
            }
        }
        Some(_) => {
            println!("eldr {cur} is the latest version.");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_tag() {
        let json = r#"{"url":"...","tag_name":"v0.9.0","name":"v0.9.0 — ..."}"#;
        assert_eq!(parse_tag(json), Some("0.9.0".to_string()));
        // No leading v, extra whitespace tolerated.
        assert_eq!(
            parse_tag(r#"{"tag_name": "1.2.3"}"#),
            Some("1.2.3".to_string())
        );
        // Missing key → None.
        assert_eq!(parse_tag(r#"{"name":"x"}"#), None);
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.10.0", "0.9.0")); // not a string compare (0.10 > 0.9)
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.9.1", "0.9.0"));
        assert!(!is_newer("0.9.0", "0.9.0"));
        assert!(!is_newer("0.8.0", "0.9.0"));
        assert!(!is_newer("v0.9.0", "0.9.0")); // tolerates a leading v, equal
    }

    #[test]
    fn reads_back_cache_format() {
        // The shape write_cache emits must round-trip through the reader.
        let body = r#"{"ts":1750000000,"latest":"0.9.0"}"#;
        assert_eq!(
            parse_tag_named(body, "\"ts\""),
            Some("1750000000".to_string())
        );
        assert_eq!(
            parse_tag_named(body, "\"latest\""),
            Some("0.9.0".to_string())
        );
    }
}
