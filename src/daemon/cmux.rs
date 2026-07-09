//! Thin subprocess wrapper around the `cmux` CLI (an external tool the user runs, not
//! a crate dependency — same lane as git/stress-ng in the bash prototype). Used by the
//! guard to fan thermal badges and notifications into every cmux workspace. All calls
//! are passive (status/notify); the guard never sends a prompt an agent would execute.

use std::collections::{HashMap, HashSet};
use std::process::{Command, Stdio};

const RESOURCE_KEY: &str = "resources";
const MAX_ERR_CHARS: usize = 240;
const WORKSPACE_USAGE_ARGS: [&str; 4] = ["top", "--all", "--format", "tsv"];

/// True if the `cmux` binary is reachable.
pub fn available() -> bool {
    Command::new("cmux")
        .arg("list-workspaces")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Workspace ids like `workspace:3`.
pub fn workspaces() -> Vec<String> {
    let Ok(out) = Command::new("cmux").arg("list-workspaces").output() else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut ids = Vec::new();
    for tok in text.split(|c: char| c.is_whitespace() || c == ',') {
        if let Some(rest) = tok.strip_prefix("workspace:")
            && !rest.is_empty()
            && rest.chars().all(|c| c.is_ascii_digit())
        {
            ids.push(format!("workspace:{rest}"));
        }
    }
    ids
}

fn run(args: &[&str]) -> Result<(), String> {
    let out = Command::new("cmux")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("spawn cmux: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(command_error("cmux", args, &out))
    }
}

/// Set the thermal status badge in a workspace.
pub fn set_status(workspace: &str, key: &str, text: &str, icon: &str, color: &str) -> bool {
    set_status_result(workspace, key, text, icon, color).is_ok()
}

fn set_status_result(
    workspace: &str,
    key: &str,
    text: &str,
    icon: &str,
    color: &str,
) -> Result<(), String> {
    run(&[
        "set-status",
        key,
        text,
        "--workspace",
        workspace,
        "--icon",
        icon,
        "--color",
        color,
    ])
}

/// Clear the thermal status badge in a workspace.
pub fn clear_status(workspace: &str, key: &str) -> bool {
    run(&["clear-status", key, "--workspace", workspace]).is_ok()
}

/// Post a notification into a workspace.
pub fn notify(workspace: &str, title: &str, subtitle: &str, body: &str) -> bool {
    run(&[
        "notify",
        "--title",
        title,
        "--subtitle",
        subtitle,
        "--body",
        body,
        "--workspace",
        workspace,
    ])
    .is_ok()
}

/// Badge every workspace with the current thermal level (passive). One `list-workspaces`
/// spawn: an empty result (cmux absent or no workspaces) short-circuits the loop, so no
/// separate availability probe is needed.
pub fn badge_all(level: &str, text: &str, color: &str) {
    for ws in workspaces() {
        set_status(
            &ws,
            "thermal",
            &format!("{level} {text}"),
            "thermometer",
            color,
        );
    }
}

/// Clear the thermal badge everywhere.
pub fn clear_all() {
    for ws in workspaces() {
        clear_status(&ws, "thermal");
    }
}

/// Clear the resource badge everywhere.
pub fn clear_resources_all() {
    for ws in workspaces() {
        clear_status(&ws, RESOURCE_KEY);
    }
}

#[derive(Clone, Debug, PartialEq)]
struct WorkspaceUsage {
    workspace: String,
    cpu_pct: f64,
    mem_bytes: u64,
    proc_count: u32,
}

/// Read cmux's own per-workspace resource accounting. This uses the already-aggregated
/// `workspace` rows, so Eldr does not need to request or remap every process to a tab itself.
fn workspace_usage() -> Result<Vec<WorkspaceUsage>, String> {
    let out = Command::new("cmux")
        .args(WORKSPACE_USAGE_ARGS)
        .output()
        .map_err(|e| format!("spawn cmux top: {e}"))?;
    if !out.status.success() {
        return Err(command_error("cmux", &WORKSPACE_USAGE_ARGS, &out));
    }
    Ok(parse_workspace_usage(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_workspace_usage(text: &str) -> Vec<WorkspaceUsage> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 5 || cols[3] != "workspace" {
            continue;
        }
        let Ok(cpu_pct) = cols[0].parse::<f64>() else {
            continue;
        };
        let Ok(mem_bytes) = cols[1].parse::<u64>() else {
            continue;
        };
        let Ok(proc_count) = cols[2].parse::<u32>() else {
            continue;
        };
        let workspace = cols[4].trim();
        if !workspace.starts_with("workspace:") {
            continue;
        }
        rows.push(WorkspaceUsage {
            workspace: workspace.to_string(),
            cpu_pct,
            mem_bytes,
            proc_count,
        });
    }
    rows
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResourceSyncReport {
    pub rows: usize,
    pub set: usize,
    pub error: Option<String>,
}

impl ResourceSyncReport {
    pub fn is_problem(&self) -> bool {
        self.error.is_some() || (self.rows == 0 && self.set == 0)
    }

    pub fn summary(&self) -> String {
        match &self.error {
            Some(err) => format!("rows={} set={} error={err}", self.rows, self.set),
            None => format!("rows={} set={}", self.rows, self.set),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResourceBadge {
    text: String,
    color: &'static str,
}

/// The guard owns one cache for its lifetime. It prevents cmux from redrawing a workspace
/// badge when its visible content is unchanged; a periodic forced reconcile still restores
/// badges after an external cmux restart.
#[derive(Default)]
pub struct ResourceBadgeCache {
    entries: HashMap<String, ResourceBadge>,
}

impl ResourceBadgeCache {
    fn should_sync(&self, workspace: &str, badge: &ResourceBadge, force: bool) -> bool {
        force || self.entries.get(workspace) != Some(badge)
    }

    fn mark_synced(&mut self, workspace: String, badge: ResourceBadge) {
        self.entries.insert(workspace, badge);
    }

    fn retain_live(&mut self, live: &HashSet<String>) {
        self.entries.retain(|workspace, _| live.contains(workspace));
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Set a compact CPU/RAM badge on each cmux workspace tab. Repeated, unchanged labels are
/// skipped, while `force` reasserts all live badges after a bounded reconciliation interval.
pub fn sync_resource_badges(cache: &mut ResourceBadgeCache, force: bool) -> ResourceSyncReport {
    let usages = match workspace_usage() {
        Ok(usages) => usages,
        Err(err) => {
            cache.clear();
            return ResourceSyncReport {
                rows: 0,
                set: 0,
                error: Some(err),
            };
        }
    };
    let mut report = ResourceSyncReport {
        rows: usages.len(),
        set: 0,
        error: None,
    };
    let mut live = HashSet::with_capacity(usages.len());
    for usage in usages {
        live.insert(usage.workspace.clone());
        let badge = resource_badge(&usage);
        if !cache.should_sync(&usage.workspace, &badge, force) {
            continue;
        }
        match set_status_result(
            &usage.workspace,
            RESOURCE_KEY,
            &badge.text,
            "speedometer",
            badge.color,
        ) {
            Ok(()) => {
                cache.mark_synced(usage.workspace, badge);
                report.set += 1;
            }
            Err(err) => {
                if report.error.is_none() {
                    report.error = Some(err);
                }
            }
        }
    }
    cache.retain_live(&live);
    if report.error.is_some() {
        cache.clear();
    }
    report
}

fn command_error(cmd: &str, args: &[&str], out: &std::process::Output) -> String {
    let mut detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if detail.is_empty() {
        detail = String::from_utf8_lossy(&out.stdout).trim().to_string();
    }
    if detail.chars().count() > MAX_ERR_CHARS {
        detail = detail.chars().take(MAX_ERR_CHARS).collect();
        detail.push('…');
    }
    if detail.is_empty() {
        format!("{cmd} {} exited {}", args.join(" "), out.status)
    } else {
        format!("{cmd} {} exited {}: {detail}", args.join(" "), out.status)
    }
}

fn resource_text(usage: &WorkspaceUsage) -> String {
    format!(
        "CPU {} · RAM {} · {}",
        fmt_cpu(usage.cpu_pct),
        fmt_mem(usage.mem_bytes),
        fmt_proc_count(usage.proc_count)
    )
}

fn resource_badge(usage: &WorkspaceUsage) -> ResourceBadge {
    ResourceBadge {
        text: resource_text(usage),
        color: resource_color(usage),
    }
}

fn resource_color(usage: &WorkspaceUsage) -> &'static str {
    if usage.cpu_pct >= 300.0 || usage.mem_bytes >= 8 * 1_073_741_824 {
        "#f85149"
    } else if usage.cpu_pct >= 100.0 || usage.mem_bytes >= 4 * 1_073_741_824 {
        "#d29922"
    } else {
        "#7fa8c9"
    }
}

fn fmt_cpu(cpu_pct: f64) -> String {
    if cpu_pct >= 10.0 {
        format!("{cpu_pct:.0}%")
    } else if cpu_pct > 0.0 {
        format!("{cpu_pct:.1}%")
    } else {
        "0%".into()
    }
}

fn fmt_mem(bytes: u64) -> String {
    const GIB: f64 = 1_073_741_824.0;
    const MIB: f64 = 1_048_576.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.1} GB", b / GIB)
    } else if b >= MIB {
        format!("{:.0} MB", b / MIB)
    } else {
        format!("{} KB", bytes / 1024)
    }
}

fn fmt_proc_count(proc_count: u32) -> String {
    format!("{proc_count} proc")
}

/// Notify every workspace of an alert (passive).
pub fn notify_all(title: &str, subtitle: &str, body: &str) {
    for ws in workspaces() {
        notify(&ws, title, subtitle, body);
    }
}

/// Find the cmux surface hosting a given process pid (for sending an Escape). Parses
/// `cmux top --all --processes --format tsv`: column 4 == "process", column 5 == pid,
/// column 6 == the surface ref. Returns only real `surface:`/`:tag:` refs.
pub fn surface_for_pid(pid: i32) -> Option<String> {
    let out = Command::new("cmux")
        .args(["top", "--all", "--processes", "--format", "tsv"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let pid_s = pid.to_string();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() >= 6 && f[3] == "process" && f[4] == pid_s {
            let surf = f[5].trim();
            if surf.starts_with("surface:") || surf.contains(":tag:") {
                return Some(surf.to_string());
            }
        }
    }
    None
}

/// Send a key to a cmux surface (e.g. `Escape` to pause generation — reversible).
pub fn send_key(surface: &str, key: &str) -> bool {
    Command::new("cmux")
        .args(["send-key", "--surface", surface, key])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_usage_uses_compact_cmux_top_rows() {
        assert_eq!(WORKSPACE_USAGE_ARGS, ["top", "--all", "--format", "tsv"]);
    }

    #[test]
    fn parses_compact_workspace_resource_rows_from_cmux_top() {
        let text = "\
87.0\t4607160008\t80\ttotal\ttotal\t\t\n\
2.0\t316904248\t7\tworkspace\tworkspace:7\twindow:1\tProject A\n\
5.9\t597943536\t12\tpane\tpane:25\tworkspace:14\t\n\
0.0\t0\t0\ttag\tworkspace:7:tag:resources\tworkspace:7\tCPU 2%\n\
1.2\t123456\t1\tprocess\t123\tsurface:12\tchild\n\
8.7\t285743392\t9\tworkspace\tworkspace:14\twindow:1\tProject B\n";

        let rows = parse_workspace_usage(text);
        assert_eq!(
            rows,
            vec![
                WorkspaceUsage {
                    workspace: "workspace:7".into(),
                    cpu_pct: 2.0,
                    mem_bytes: 316_904_248,
                    proc_count: 7,
                },
                WorkspaceUsage {
                    workspace: "workspace:14".into(),
                    cpu_pct: 8.7,
                    mem_bytes: 285_743_392,
                    proc_count: 9,
                },
            ]
        );
    }

    #[test]
    fn resource_badge_cache_skips_identical_content_until_reconcile() {
        let usage = WorkspaceUsage {
            workspace: "workspace:7".into(),
            cpu_pct: 8.7,
            mem_bytes: 285_743_392,
            proc_count: 9,
        };
        let badge = resource_badge(&usage);
        let mut cache = ResourceBadgeCache::default();

        assert!(cache.should_sync(&usage.workspace, &badge, false));
        cache.mark_synced(usage.workspace.clone(), badge.clone());
        assert!(!cache.should_sync(&usage.workspace, &badge, false));
        assert!(cache.should_sync(&usage.workspace, &badge, true));
    }

    #[test]
    fn resource_badge_is_compact_and_colored_by_pressure() {
        let calm = WorkspaceUsage {
            workspace: "workspace:1".into(),
            cpu_pct: 8.7,
            mem_bytes: 285_743_392,
            proc_count: 9,
        };
        assert_eq!(resource_text(&calm), "CPU 8.7% · RAM 273 MB · 9 proc");
        assert_eq!(resource_color(&calm), "#7fa8c9");

        let busy = WorkspaceUsage {
            workspace: "workspace:2".into(),
            cpu_pct: 350.0,
            mem_bytes: 9 * 1_073_741_824,
            proc_count: 2,
        };
        assert_eq!(resource_text(&busy), "CPU 350% · RAM 9.0 GB · 2 proc");
        assert_eq!(resource_color(&busy), "#f85149");
    }
}
