//! Thin subprocess wrapper around the `cmux` CLI (an external tool the user runs, not
//! a crate dependency — same lane as git/stress-ng in the bash prototype). Used by the
//! guard to fan thermal badges and notifications into every cmux workspace. All calls
//! are passive (status/notify); the guard never sends a prompt an agent would execute.

use std::process::{Command, Stdio};

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

fn run(args: &[&str]) {
    let _ = Command::new("cmux")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Set the thermal status badge in a workspace.
pub fn set_status(workspace: &str, key: &str, text: &str, icon: &str, color: &str) {
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
    ]);
}

/// Clear the thermal status badge in a workspace.
pub fn clear_status(workspace: &str, key: &str) {
    run(&["clear-status", key, "--workspace", workspace]);
}

/// Post a notification into a workspace.
pub fn notify(workspace: &str, title: &str, subtitle: &str, body: &str) {
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
    ]);
}

/// Badge every workspace with the current thermal level (passive).
pub fn badge_all(level: &str, text: &str, color: &str) {
    if !available() {
        return;
    }
    for ws in workspaces() {
        set_status(&ws, "thermal", &format!("{level} {text}"), "thermometer", color);
    }
}

/// Clear the thermal badge everywhere.
pub fn clear_all() {
    if !available() {
        return;
    }
    for ws in workspaces() {
        clear_status(&ws, "thermal");
    }
}

/// Notify every workspace of an alert (passive).
pub fn notify_all(title: &str, subtitle: &str, body: &str) {
    if !available() {
        return;
    }
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
