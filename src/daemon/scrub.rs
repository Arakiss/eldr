//! Data-integrity scrubber: fingerprint every file under a path, then re-verify later to
//! catch silent corruption (bit rot) — a flipped bit on disk that nothing reports.
//!
//! SMART won't see it (it predicts device failure, not stray bits) and APFS won't either
//! (it checksums its own metadata, not your file data). The only honest detector is to
//! hash the bytes and compare. The tell of true corruption vs a normal edit: the content
//! changed while size AND modification time stayed identical. Zero crates — the SHA-256
//! is ours.

use crate::config;
use crate::crypto::cc::CcSha256;
use crate::crypto::sha256;
use crate::sensors::snapshot::SCHEMA_VERSION;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// One recorded file fingerprint.
struct Entry {
    hash: String,
    size: u64,
    mtime: u64,
}

/// Outcome of comparing a file against its recorded fingerprint.
#[derive(PartialEq, Eq, Debug)]
enum Verdict {
    Intact,
    Edited,  // content changed, and so did size or mtime — a legitimate write
    Corrupt, // content changed but size AND mtime are identical — silent bit rot
}

/// `eldr scrub <init|verify|status> [path] [--notify] [--json]`.
pub fn run(args: &[String]) -> i32 {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("");
    let json = args.iter().any(|a| a == "--json");
    let path = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .map(|s| s.as_str());
    match sub {
        "init" => match path {
            Some(p) => cmd_init(p, json),
            None => usage(),
        },
        "verify" => match path {
            Some(p) => cmd_verify(p, args.iter().any(|a| a == "--notify"), json),
            None => usage(),
        },
        "status" => cmd_status(path, json),
        _ => usage(),
    }
}

fn usage() -> i32 {
    eprintln!(
        "usage: eldr scrub <command> <path>\n  \
         init <path>            fingerprint a tree into a manifest\n  \
         verify <path> [--notify]  re-hash; report bit rot, edits, new/missing\n  \
         status [path]          manifest summary\n\n\
         --notify on verify raises a macOS notification and logs to alerts.log when\n  \
         corruption is found — for scheduled (launchd/cron) scrubs."
    );
    2
}

fn cmd_init(root: &str, json: bool) -> i32 {
    let rootp = Path::new(root);
    if !rootp.is_dir() {
        emit_error(json, &format!("not a directory: {root}"));
        return 2;
    }
    let mut entries: BTreeMap<String, Entry> = BTreeMap::new();
    let (mut bytes, mut skipped) = (0u64, 0u64);
    for p in walk(rootp) {
        match fingerprint(&p) {
            Some((hash, size, mtime)) => {
                bytes += size;
                entries.insert(
                    p.to_string_lossy().into_owned(),
                    Entry { hash, size, mtime },
                );
            }
            None => skipped += 1,
        }
    }
    if let Err(e) = write_manifest(root, &entries) {
        emit_error(json, &format!("cannot write manifest: {e}"));
        return 1;
    }
    if json {
        println!(
            "{{\"schema_version\":\"{}\",\"action\":\"init\",\"root\":\"{}\",\"files\":{},\"bytes\":{},\"skipped\":{},\"manifest\":\"{}\"}}",
            SCHEMA_VERSION,
            esc(root),
            entries.len(),
            bytes,
            skipped,
            esc(&manifest_path(root).to_string_lossy()),
        );
    } else {
        let skip = if skipped > 0 {
            format!(" · {skipped} unreadable skipped")
        } else {
            String::new()
        };
        println!(
            "indexed {} files · {}{skip}\nmanifest: {}",
            entries.len(),
            human(bytes),
            manifest_path(root).display(),
        );
    }
    0
}

fn cmd_verify(root: &str, notify: bool, json: bool) -> i32 {
    let Some((_, mut manifest)) = read_manifest(root) else {
        emit_error(
            json,
            &format!("no manifest for {root}; run: eldr scrub init {root}"),
        );
        return 2;
    };
    let rootp = Path::new(root);
    let mut seen: HashSet<String> = HashSet::new();
    let (mut intact, mut edited, mut added) = (0u64, 0u64, 0u64);
    let mut corrupt: Vec<String> = Vec::new();

    for p in walk(rootp) {
        let Some((hash, size, mtime)) = fingerprint(&p) else {
            continue;
        };
        let key = p.to_string_lossy().into_owned();
        seen.insert(key.clone());
        match manifest
            .get(&key)
            .map(|e| (e.hash.clone(), e.size, e.mtime))
        {
            None => {
                added += 1;
                manifest.insert(key, Entry { hash, size, mtime });
            }
            Some((ph, ps, pm)) => match classify((&ph, ps, pm), &hash, size, mtime) {
                Verdict::Intact => intact += 1,
                Verdict::Edited => {
                    edited += 1;
                    manifest.insert(key, Entry { hash, size, mtime });
                }
                // Keep the original entry so repeated verifies keep flagging it.
                Verdict::Corrupt => corrupt.push(key),
            },
        }
    }

    let missing: Vec<String> = manifest
        .keys()
        .filter(|k| !seen.contains(*k))
        .cloned()
        .collect();
    for m in &missing {
        manifest.remove(m);
    }
    // Persist the reconciled manifest (edits applied, new added, missing dropped). The
    // corrupt files keep their original recorded hash, so they keep being reported.
    let _ = write_manifest(root, &manifest);

    let code = if corrupt.is_empty() { 0 } else { 2 };
    if json {
        let list = corrupt
            .iter()
            .map(|c| format!("\"{}\"", esc(c)))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"schema_version\":\"{}\",\"action\":\"verify\",\"root\":\"{}\",\"intact\":{intact},\"edited\":{edited},\"added\":{added},\"missing\":{},\"corrupt\":[{list}],\"exit\":{code}}}",
            SCHEMA_VERSION,
            esc(root),
            missing.len(),
        );
    } else {
        println!(
            "{intact} intact · {edited} edited · {added} new · {} missing · {} corrupt",
            missing.len(),
            corrupt.len(),
        );
        for c in &corrupt {
            println!("  CORRUPT  {c}");
        }
    }
    if !corrupt.is_empty() && notify {
        notify_corruption(root, &corrupt);
    }
    code
}

/// Raise a macOS notification and append to alerts.log when a scheduled scrub finds
/// corruption — so a launchd/cron `eldr scrub verify --notify` is actually heard.
fn notify_corruption(root: &str, corrupt: &[String]) {
    let body = format!(
        "{} file(s) corrupted under {root} — restore from backup",
        corrupt.len()
    );
    // Escape for an AppleScript literal; collapse control chars (a corrupt path can't
    // break out of the string and inject AppleScript).
    let esc = |s: &str| -> String {
        s.chars()
            .map(|c| match c {
                '\\' => "\\\\".to_string(),
                '"' => "\\\"".to_string(),
                c if (c as u32) < 0x20 => " ".to_string(),
                c => c.to_string(),
            })
            .collect()
    };
    let script = format!(
        "display notification \"{}\" with title \"eldr · data integrity\" sound name \"Basso\"",
        esc(&body)
    );
    let _ = std::process::Command::new("osascript")
        .args(["-e", &script])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    let ts = crate::sensors::host::timestamp();
    let mut line = format!("{ts} SCRUB {} corrupt under {root}\n", corrupt.len());
    for c in corrupt {
        line.push_str(&format!("  CORRUPT {c}\n"));
    }
    use std::io::Write;
    config::ensure_data_dir();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config::alerts_path())
    {
        let _ = f.write_all(line.as_bytes());
    }
}

fn cmd_status(root: Option<&str>, json: bool) -> i32 {
    // Collect (root, files, bytes, last_scan_ts) for the requested manifest(s).
    let mut summaries: Vec<(String, usize, u64, u64)> = Vec::new();
    let mut missing_one = false;
    match root {
        Some(r) => match read_manifest(r) {
            Some((meta, entries)) => {
                let bytes = entries.values().map(|e| e.size).sum();
                summaries.push((r.to_string(), entries.len(), bytes, meta.ts));
            }
            None => missing_one = true,
        },
        None => {
            if let Ok(rd) = fs::read_dir(config::data_dir().join("scrub")) {
                for ent in rd.flatten() {
                    let p = ent.path();
                    if p.extension().and_then(|e| e.to_str()) != Some("manifest") {
                        continue;
                    }
                    if let Some((meta, entries)) = read_manifest_file(&p) {
                        let bytes = entries.values().map(|e| e.size).sum();
                        summaries.push((meta.root, entries.len(), bytes, meta.ts));
                    }
                }
            }
        }
    }

    if missing_one {
        emit_error(json, &format!("no manifest for {}", root.unwrap_or("")));
        return 2;
    }
    if json {
        let items = summaries
            .iter()
            .map(|(r, files, bytes, ts)| {
                format!(
                    "{{\"root\":\"{}\",\"files\":{files},\"bytes\":{bytes},\"last_scan_ts\":{ts}}}",
                    esc(r)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"schema_version\":\"{}\",\"action\":\"status\",\"manifests\":[{items}]}}",
            SCHEMA_VERSION
        );
        return 0;
    }
    if summaries.is_empty() {
        println!("no scrub manifests yet — run: eldr scrub init <path>");
        return 0;
    }
    for (r, files, bytes, ts) in &summaries {
        println!(
            "{r}: {files} files · {} · last scan {}",
            human(*bytes),
            fmt_age(*ts),
        );
    }
    0
}

/// JSON-escape a string (shared with the snapshot serializer).
fn esc(s: &str) -> String {
    crate::sensors::snapshot::json_escape(s)
}

/// Report an error as JSON (under `--json`) or a plain stderr line otherwise.
fn emit_error(json: bool, msg: &str) {
    if json {
        println!(
            "{{\"schema_version\":\"{}\",\"error\":\"{}\"}}",
            SCHEMA_VERSION,
            esc(msg),
        );
    } else {
        eprintln!("eldr scrub: {msg}");
    }
}

/// The core decision, factored out so it is exhaustively unit-tested without touching the
/// filesystem: identical hash is intact; a differing hash with a changed size or mtime is
/// a legitimate edit; a differing hash with size AND mtime unchanged is silent corruption.
fn classify(prev: (&str, u64, u64), cur_hash: &str, cur_size: u64, cur_mtime: u64) -> Verdict {
    let (ph, ps, pm) = prev;
    if ph == cur_hash {
        Verdict::Intact
    } else if ps != cur_size || pm != cur_mtime {
        Verdict::Edited
    } else {
        Verdict::Corrupt
    }
}

// MARK: filesystem helpers

/// Hash a file and read its size + mtime. `None` if it can't be read.
fn fingerprint(path: &Path) -> Option<(String, u64, u64)> {
    let md = fs::metadata(path).ok()?;
    let mut f = fs::File::open(path).ok()?;
    let mut hasher = CcSha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some((sha256::hex(&hasher.finalize()), md.len(), mtime_secs(&md)))
}

/// Every regular file under `root`, iterative (no recursion depth limit). Symlinks are
/// skipped (no loops, no escaping the tree) and so are macOS's hidden bookkeeping dirs.
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            let Ok(md) = fs::symlink_metadata(&p) else {
                continue;
            };
            let ft = md.file_type();
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                let skip = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(is_skip_dir)
                    .unwrap_or(false);
                if !skip {
                    stack.push(p);
                }
            } else if ft.is_file() {
                out.push(p);
            }
        }
    }
    out
}

/// macOS volume bookkeeping directories that churn constantly and aren't user data.
fn is_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".Spotlight-V100"
            | ".fseventsd"
            | ".Trashes"
            | ".DocumentRevisions-V100"
            | ".TemporaryItems"
            | ".vol"
    )
}

fn mtime_secs(md: &fs::Metadata) -> u64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// MARK: manifest persistence

#[derive(Default)]
struct ManifestMeta {
    root: String,
    ts: u64,
}

fn sanitize(root: &str) -> String {
    let s: String = root
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let s = s.trim_matches('_').to_string();
    if s.is_empty() { "root".into() } else { s }
}

fn manifest_path(root: &str) -> PathBuf {
    config::data_dir()
        .join("scrub")
        .join(format!("{}.manifest", sanitize(root)))
}

fn write_manifest(root: &str, entries: &BTreeMap<String, Entry>) -> std::io::Result<()> {
    let path = manifest_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut body = format!(
        "# eldr-scrub v1 ts={} files={} root={}\n",
        crate::sensors::host::unix_time(),
        entries.len(),
        root,
    );
    for (p, e) in entries {
        body.push_str(&format!("{} {} {} {}\n", e.hash, e.size, e.mtime, p));
    }
    let tmp = path.with_extension("manifest.tmp");
    fs::write(&tmp, body)?;
    fs::rename(&tmp, &path)
}

fn read_manifest(root: &str) -> Option<(ManifestMeta, BTreeMap<String, Entry>)> {
    read_manifest_file(&manifest_path(root))
}

fn read_manifest_file(path: &Path) -> Option<(ManifestMeta, BTreeMap<String, Entry>)> {
    let text = fs::read_to_string(path).ok()?;
    let mut entries = BTreeMap::new();
    let mut meta = ManifestMeta::default();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# eldr-scrub") {
            for tok in rest.split_whitespace() {
                if let Some(v) = tok.strip_prefix("ts=") {
                    meta.ts = v.parse().unwrap_or(0);
                } else if let Some(v) = tok.strip_prefix("root=") {
                    meta.root = v.to_string();
                }
            }
            continue;
        }
        if line.is_empty() {
            continue;
        }
        // "<hash> <size> <mtime> <path>" — path is last and may contain spaces.
        let mut it = line.splitn(4, ' ');
        let (Some(hash), Some(size), Some(mtime), Some(p)) =
            (it.next(), it.next(), it.next(), it.next())
        else {
            continue;
        };
        entries.insert(
            p.to_string(),
            Entry {
                hash: hash.to_string(),
                size: size.parse().unwrap_or(0),
                mtime: mtime.parse().unwrap_or(0),
            },
        );
    }
    Some((meta, entries))
}

// MARK: formatting

fn human(bytes: u64) -> String {
    crate::ui::style::human_bytes(bytes)
}

/// Coarse age of a Unix timestamp ("3h ago", "2d ago", "just now").
fn fmt_age(ts: u64) -> String {
    if ts == 0 {
        return "unknown".into();
    }
    let now = crate::sensors::host::unix_time();
    let secs = now.saturating_sub(ts);
    if secs < 90 {
        "just now".into()
    } else if secs < 5400 {
        format!("{}m ago", secs / 60)
    } else if secs < 172_800 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_distinguishes_edit_from_bit_rot() {
        // Identical hash: intact.
        assert_eq!(classify(("aa", 10, 100), "aa", 10, 100), Verdict::Intact);
        // Content changed and so did mtime: a normal edit.
        assert_eq!(classify(("aa", 10, 100), "bb", 10, 200), Verdict::Edited);
        // Content changed and so did size: a normal edit.
        assert_eq!(classify(("aa", 10, 100), "bb", 12, 100), Verdict::Edited);
        // Content changed but size AND mtime identical: silent corruption.
        assert_eq!(classify(("aa", 10, 100), "bb", 10, 100), Verdict::Corrupt);
    }

    #[test]
    fn sanitize_makes_a_filesystem_safe_key() {
        assert_eq!(sanitize("/Volumes/Vault"), "Volumes_Vault");
        assert_eq!(sanitize("/"), "root");
        assert_eq!(sanitize("/Users/me/My Photos"), "Users_me_My_Photos");
    }
}
