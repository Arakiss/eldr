//! launchd integration: run the guard 24/7 (start at login, restart on crash).
//! Ported from the bash prototype's `guard-install`/`guard-uninstall`.

use crate::config;
use crate::daemon::guard;
use std::process::Command;
use std::thread;
use std::time::Duration;

const LABEL: &str = "com.petruarakiss.eldr.guard";
const BUNDLE_ID: &str = "com.petruarakiss.eldr";

unsafe extern "C" {
    fn getuid() -> u32;
}

fn plist_path() -> std::path::PathBuf {
    std::path::PathBuf::from(config::home())
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

/// Whether the guard's LaunchAgent is installed (its plist exists).
pub fn installed() -> bool {
    plist_path().exists()
}

fn render_plist(
    exe: &str,
    interval: u32,
    dir: &str,
    log: &str,
    associate_bundle: bool,
    cmux_socket_path: &str,
) -> String {
    let bundle = if associate_bundle {
        format!(
            r#"  <key>AssociatedBundleIdentifiers</key>
  <array><string>{BUNDLE_ID}</string></array>
"#
        )
    } else {
        String::new()
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
{bundle}  <key>Program</key><string>{exe}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string><string>guard</string><string>--interval</string><string>{interval}</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key><string>/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:{home}/.local/bin:/Applications/cmux.app/Contents/Resources/bin</string>
    <key>ELDR_DIR</key><string>{dir}</string>
    <key>CMUX_SOCKET_PATH</key><string>{cmux_socket_path}</string>
    <key>LC_ALL</key><string>en_US.UTF-8</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
  <key>ProcessType</key><string>Background</string>
</dict>
</plist>
"#,
        home = config::home(),
    )
}

fn cmux_socket_path(uid: u32) -> String {
    let last = std::path::PathBuf::from(config::home())
        .join(".local")
        .join("state")
        .join("cmux")
        .join("last-socket-path");
    if let Ok(text) = std::fs::read_to_string(last) {
        let path = text.trim();
        if !path.is_empty() {
            return path.to_string();
        }
    }
    format!("{}/.local/state/cmux/cmux-{uid}.sock", config::home())
}

struct GuardCandidate {
    exe: String,
    via_bundle: bool,
    associate_bundle: bool,
}

/// The executable launchd should run. Prefer the app-bundle binary so the guard is
/// attributed to `Eldr.app`; fall back to the installed CLI if macOS rejects the bundle
/// executable after a local rebuild.
fn guard_candidates() -> Vec<GuardCandidate> {
    let mut out = Vec::new();
    let app =
        std::path::PathBuf::from(config::home()).join("Applications/Eldr.app/Contents/MacOS/eldr");
    if app.exists() {
        out.push(GuardCandidate {
            exe: app.to_string_lossy().into_owned(),
            via_bundle: true,
            associate_bundle: true,
        });
    }
    let cli = std::path::PathBuf::from(config::home()).join(".local/bin/eldr");
    if cli.exists() {
        out.push(GuardCandidate {
            exe: cli.to_string_lossy().into_owned(),
            via_bundle: false,
            associate_bundle: true,
        });
    }
    if let Ok(exe) = std::env::current_exe() {
        let exe = exe.to_string_lossy().into_owned();
        if !out.iter().any(|c| c.exe == exe) {
            out.push(GuardCandidate {
                exe,
                via_bundle: false,
                associate_bundle: false,
            });
        }
    }
    out
}

/// Install + start the guard LaunchAgent.
pub fn install() -> i32 {
    let candidates = guard_candidates();
    if candidates.is_empty() {
        eprintln!("eldr: cannot resolve guard executable");
        return 1;
    }
    let dir = config::ensure_data_dir();
    let log = dir.join("guard.log");
    let plist = plist_path();
    if let Some(parent) = plist.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let uid = unsafe { getuid() };
    let domain = format!("gui/{uid}");
    let cmux_socket = cmux_socket_path(uid);

    for (idx, candidate) in candidates.iter().enumerate() {
        stop_loaded(&domain, &plist);
        if let Err(e) = std::fs::write(
            &plist,
            render_plist(
                &candidate.exe,
                30,
                &dir.to_string_lossy(),
                &log.to_string_lossy(),
                candidate.associate_bundle,
                &cmux_socket,
            ),
        ) {
            eprintln!("eldr: cannot write plist: {e}");
            return 1;
        }
        if start_loaded(&domain, &plist) {
            println!(
                "eldr guard installed as LaunchAgent ({LABEL}) — starts at login, restarts on crash."
            );
            if candidate.via_bundle {
                println!("  running via Eldr.app — shows the eldr icon in Login Items.");
            } else if candidate.associate_bundle {
                println!("  running via CLI fallback — associated with Eldr.app in Login Items.");
            }
            println!(
                "  stop for real: eldr guard-uninstall   ·   log: {}",
                log.display()
            );
            return 0;
        }
        if idx == 0 && candidate.via_bundle && candidates.len() > 1 {
            eprintln!("eldr: launchd rejected Eldr.app guard executable; trying CLI fallback.");
        }
    }

    eprintln!("eldr: launchd did not start {LABEL}; see {}", log.display());
    1
}

fn stop_loaded(domain: &str, plist: &std::path::Path) {
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{domain}/{LABEL}")])
        .status();
    let _ = Command::new("launchctl")
        .args(["unload", &plist.to_string_lossy()])
        .status();
    guard::stop();
}

fn start_loaded(domain: &str, plist: &std::path::Path) -> bool {
    let bootstrapped = Command::new("launchctl")
        .args(["bootstrap", domain, &plist.to_string_lossy()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !bootstrapped {
        let loaded = Command::new("launchctl")
            .args(["load", "-w", &plist.to_string_lossy()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !loaded {
            return false;
        }
    }
    thread::sleep(Duration::from_millis(500));
    Command::new("launchctl")
        .args(["print", &format!("{domain}/{LABEL}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Stop + remove the guard LaunchAgent.
pub fn uninstall() -> i32 {
    let uid = unsafe { getuid() };
    let plist = plist_path();
    stop_loaded(&format!("gui/{uid}"), &plist);
    let _ = std::fs::remove_file(&plist);
    println!("eldr guard LaunchAgent uninstalled");
    0
}

#[cfg(test)]
mod tests {
    use super::render_plist;

    #[test]
    fn launch_agent_pins_cmux_socket_path() {
        let plist = render_plist(
            "/tmp/eldr",
            30,
            "/tmp/eldr-data",
            "/tmp/eldr-data/guard.log",
            false,
            "/tmp/cmux.sock",
        );
        assert!(plist.contains("<key>CMUX_SOCKET_PATH</key><string>/tmp/cmux.sock</string>"));
    }
}
