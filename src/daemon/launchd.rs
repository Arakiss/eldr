//! launchd integration: run the guard 24/7 (start at login, restart on crash).
//! Ported from the bash prototype's `guard-install`/`guard-uninstall`.

use crate::config;
use crate::daemon::guard;
use std::process::Command;

const LABEL: &str = "com.petruarakiss.eldr.guard";

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

fn render_plist(exe: &str, interval: u32, dir: &str, log: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string><string>guard</string><string>--interval</string><string>{interval}</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key><string>/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin:{home}/.local/bin:/Applications/cmux.app/Contents/Resources/bin</string>
    <key>ELDR_DIR</key><string>{dir}</string>
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

/// The executable launchd should run. Prefer the app-bundle binary so the guard is
/// attributed to `Eldr.app` in Login Items (its icon + name); fall back to our own path.
fn guard_exe() -> Option<String> {
    let app =
        std::path::PathBuf::from(config::home()).join("Applications/Eldr.app/Contents/MacOS/eldr");
    if app.exists() {
        return Some(app.to_string_lossy().into_owned());
    }
    std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Install + start the guard LaunchAgent.
pub fn install() -> i32 {
    let exe = match guard_exe() {
        Some(p) => p,
        None => {
            eprintln!("eldr: cannot resolve guard executable");
            return 1;
        }
    };
    let via_bundle = exe.contains("Eldr.app");
    let dir = config::ensure_data_dir();
    let log = dir.join("guard.log");
    let plist = plist_path();
    if let Some(parent) = plist.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(
        &plist,
        render_plist(&exe, 30, &dir.to_string_lossy(), &log.to_string_lossy()),
    ) {
        eprintln!("eldr: cannot write plist: {e}");
        return 1;
    }

    let uid = unsafe { getuid() };
    let domain = format!("gui/{uid}");
    // Replace any prior instance.
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{domain}/{LABEL}")])
        .status();
    guard::stop();
    let ok = Command::new("launchctl")
        .args(["bootstrap", &domain, &plist.to_string_lossy()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        // Fall back to the legacy load path.
        let _ = Command::new("launchctl")
            .args(["load", "-w", &plist.to_string_lossy()])
            .status();
    }

    println!("eldr guard installed as LaunchAgent ({LABEL}) — starts at login, restarts on crash.");
    if via_bundle {
        println!("  running via Eldr.app — shows the eldr icon in Login Items.");
    }
    println!(
        "  stop for real: eldr guard-uninstall   ·   log: {}",
        log.display()
    );
    0
}

/// Stop + remove the guard LaunchAgent.
pub fn uninstall() -> i32 {
    let uid = unsafe { getuid() };
    let plist = plist_path();
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}/{LABEL}")])
        .status();
    let _ = Command::new("launchctl")
        .args(["unload", &plist.to_string_lossy()])
        .status();
    let _ = std::fs::remove_file(&plist);
    guard::stop();
    println!("eldr guard LaunchAgent uninstalled");
    0
}
