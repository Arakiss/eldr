//! The watchdog: armed, REVERSIBLE interventions fired only on sustained critical
//! thermal anomaly. Ported from the bash prototype with every safety invariant intact.
//!
//! Hard guarantees (do not weaken):
//! - Every action is reversible: Escape (pause generation), SIGSTOP + auto-SIGCONT,
//!   `git stash create` (non-destructive snapshot). NEVER kill, shut down, or close.
//! - A single bad reading can't fire: interventions need CONFIRM consecutive critical
//!   samples (thermal critical OR stopped fan).
//! - Agents are only ever NOTIFIED, never sent a prompt they would execute.
//! - A denylist protects agents, this process, and core system processes from suspend.
//! - dry-run logs intended actions and performs nothing.

use crate::config::{self, Config};
use crate::daemon::cmux;
use crate::ffi::proc;
use crate::sensors::snapshot::Snapshot;
use core::ffi::c_int;
use std::process::Command;

const SIGSTOP: c_int = 17;
const SIGCONT: c_int = 19;

unsafe extern "C" {
    fn getpid() -> i32;
    fn kill(pid: i32, sig: c_int) -> c_int;
}

/// One intended/performed action, for logging and the dry-run report.
#[derive(Clone, Debug)]
pub struct Action {
    pub kind: &'static str,
    pub detail: String,
}

/// Arming configuration (from `~/.config/eldr/config.toml`, env-overridable).
#[derive(Clone, Copy, Debug)]
pub struct Watchdog {
    pub cmux: bool,
    pub interrupt: bool,
    pub checkpoint: bool,
    pub suspend: bool,
    pub confirm: u32,
    pub dryrun: bool,
}

impl Default for Watchdog {
    fn default() -> Self {
        Watchdog {
            cmux: true,
            interrupt: false,
            checkpoint: false,
            suspend: false,
            confirm: 3,
            dryrun: false,
        }
    }
}

impl Watchdog {
    /// Load arming from config. Keys mirror the bash `watchdog.conf` (ELDR_* / the
    /// lower-case forms), so the prototype's config ports unchanged.
    pub fn from_config(cfg: &Config) -> Self {
        let d = Watchdog::default();
        Watchdog {
            cmux: cfg.flag("ELDR_CMUX", d.cmux),
            interrupt: cfg.flag("ELDR_INTERRUPT", d.interrupt),
            checkpoint: cfg.flag("ELDR_CHECKPOINT", d.checkpoint),
            suspend: cfg.flag("ELDR_SUSPEND", d.suspend),
            confirm: cfg.int("ELDR_CONFIRM", d.confirm as i64).max(1) as u32,
            dryrun: cfg.flag("ELDR_DRYRUN", d.dryrun),
        }
    }

    pub fn load() -> Self {
        Watchdog::from_config(&config::Config::load())
    }

    /// Armed interventions for a sustained-critical episode. One-shot per episode (the
    /// guard tracks that). When `force_dry` is set, performs nothing regardless of
    /// config. Returns the actions taken (or that would be taken).
    pub fn intervene(&self, snap: &Snapshot, force_dry: bool) -> Vec<Action> {
        let dry = self.dryrun || force_dry;
        let agents = agent_pids();
        let mut actions = Vec::new();

        let tag = if dry { "[dry] " } else { "" };
        wlog(&format!(
            "## {tag}INTERVENE cpu={:.0}C fan={}rpm thermal={} (agents: {})",
            snap.cpu_temp,
            snap.fan_rpm,
            snap.thermal.as_str(),
            if agents.is_empty() {
                "none".into()
            } else {
                agents.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(" ")
            }
        ));

        if self.checkpoint {
            actions.extend(self.do_checkpoint(&agents, dry));
        }
        if self.interrupt {
            actions.extend(self.do_interrupt(&agents, dry));
        }
        if self.suspend {
            actions.extend(self.do_suspend(snap, &agents, dry));
        }
        actions
    }

    /// `git stash create` a snapshot of every dirty repo an agent is working in.
    /// Non-destructive: the working tree is left untouched; recover with `stash apply`.
    fn do_checkpoint(&self, agents: &[i32], dry: bool) -> Vec<Action> {
        let mut actions = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for &pid in agents {
            let Some(cwd) = agent_cwd(pid) else { continue };
            if !seen.insert(cwd.clone()) {
                continue;
            }
            if !is_git_worktree(&cwd) || !is_dirty(&cwd) {
                continue;
            }
            if dry {
                let a = Action { kind: "checkpoint", detail: format!("would checkpoint {cwd}") };
                wlog(&format!("  {}", a.detail));
                actions.push(a);
            } else if let Some(sha) = git_stash_create(&cwd) {
                git_stash_store(&cwd, &sha);
                let detail = format!(
                    "checkpoint {cwd} -> {} (recover: git -C {cwd} stash apply {sha})",
                    &sha[..sha.len().min(12)]
                );
                wlog(&format!("  {detail}"));
                actions.push(Action { kind: "checkpoint", detail });
            }
        }
        actions
    }

    /// Escape to each agent's cmux surface — pauses generation. Reversible by design.
    fn do_interrupt(&self, agents: &[i32], dry: bool) -> Vec<Action> {
        let mut actions = Vec::new();
        for &pid in agents {
            let Some(surf) = cmux::surface_for_pid(pid) else { continue };
            if dry {
                let a = Action { kind: "interrupt", detail: format!("would Escape {surf} (pid {pid})") };
                wlog(&format!("  {}", a.detail));
                actions.push(a);
            } else if cmux::send_key(&surf, "Escape") {
                let detail = format!("interrupted {surf} (pid {pid})");
                wlog(&format!("  {detail}"));
                actions.push(Action { kind: "interrupt", detail });
            }
        }
        actions
    }

    /// SIGSTOP the top non-protected CPU hog (auto-SIGCONT on recovery).
    fn do_suspend(&self, snap: &Snapshot, agents: &[i32], dry: bool) -> Vec<Action> {
        let mut actions = Vec::new();
        let Some(top) = snap.top_procs.first() else {
            return actions;
        };
        if is_protected(top.pid, &top.name, agents) {
            wlog(&format!("  suspend skipped: top={} protected", top.name));
            return actions;
        }
        if dry {
            let a = Action {
                kind: "suspend",
                detail: format!("would SIGSTOP {} ({} @ {:.0}%)", top.pid, top.name, top.cpu),
            };
            wlog(&format!("  {}", a.detail));
            actions.push(a);
        } else if unsafe { kill(top.pid, SIGSTOP) } == 0 {
            record_suspended(top.pid);
            let detail = format!(
                "SIGSTOP {} ({} @ {:.0}%) — auto-SIGCONT on recovery",
                top.pid, top.name, top.cpu
            );
            wlog(&format!("  {detail}"));
            actions.push(Action { kind: "suspend", detail });
        }
        actions
    }

    /// Recovery: resume everything we paused. Idempotent.
    pub fn unintervene(&self) {
        let path = config::data_dir().join("suspended.pids");
        let Ok(txt) = std::fs::read_to_string(&path) else {
            return;
        };
        for line in txt.lines() {
            if let Ok(pid) = line.trim().parse::<i32>()
                && unsafe { kill(pid, SIGCONT) } == 0
            {
                wlog(&format!("SIGCONT {pid} (resumed)"));
            }
        }
        let _ = std::fs::remove_file(&path);
    }
}

/// `eldr watchdog-test` — show exactly what an intervention WOULD do with the current
/// readings. Forces dry-run and arms all three actions so targeting is fully visible,
/// then prints the real config arming so the operator knows what is actually live.
pub fn test_report() -> i32 {
    let cfg = Watchdog::load();
    let snap = Snapshot::gather(500);

    println!(
        "dry-run readings: cpu={:.0}°C fan={}rpm thermal={}  level={}",
        snap.cpu_temp,
        snap.fan_rpm,
        snap.thermal.as_str(),
        snap.level.as_str()
    );
    println!(
        "config arming: cmux={} interrupt={} checkpoint={} suspend={} confirm={} samples dryrun={}",
        cfg.cmux, cfg.interrupt, cfg.checkpoint, cfg.suspend, cfg.confirm, cfg.dryrun
    );
    println!("--- targeting preview (all actions forced ON, dry — performs nothing) ---");

    let demo = Watchdog {
        cmux: true,
        interrupt: true,
        checkpoint: true,
        suspend: true,
        confirm: cfg.confirm,
        dryrun: true,
    };
    let actions = demo.intervene(&snap, true);
    if actions.is_empty() {
        println!("  (no reversible targets right now: no dirty agent repos, no agent surfaces, top process protected)");
    } else {
        for a in &actions {
            println!("  [{}] {}", a.kind, a.detail);
        }
    }
    0
}

// MARK: targeting + safety

/// Agent pids (claude, codex) via libproc — never our own process.
fn agent_pids() -> Vec<i32> {
    let me = unsafe { getpid() };
    let mut pids = proc::pids_named("claude");
    pids.extend(proc::pids_named("codex"));
    pids.retain(|&p| p != me && p > 1);
    pids
}

/// True if a process must NOT be suspended: us, an agent, low pids, or a core system /
/// terminal / shell process. Conservative by design — when unsure, protect.
pub fn is_protected(pid: i32, name: &str, agents: &[i32]) -> bool {
    if pid <= 1 || pid == unsafe { getpid() } || agents.contains(&pid) {
        return true;
    }
    const DENY: &[&str] = &[
        "kernel_task", "launchd", "WindowServer", "loginwindow", "logind", "Finder", "Dock",
        "SystemUIServer", "coreaudiod", "cmux", "claude", "codex", "node", "eldr", "thermalstate",
        "smctemp", "sh", "bash", "zsh", "fish", "tmux", "caffeinate", "mds", "mds_stores",
        "backupd", "powerd", "hidd", "Ghostty", "Terminal", "iTerm2", "WindowManager",
    ];
    let lname = name.to_ascii_lowercase();
    DENY.iter().any(|d| {
        let dl = d.to_ascii_lowercase();
        lname == dl || lname.contains(&dl)
    })
}

// MARK: helpers (git / cwd / logging)

fn agent_cwd(pid: i32) -> Option<String> {
    // lsof is a system tool (same lane as git/cmux). `-Fn` prints the cwd path on an
    // 'n'-prefixed line.
    let out = Command::new("lsof")
        .args(["-a", "-d", "cwd", "-p", &pid.to_string(), "-Fn"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix('n').map(|s| s.to_string()))
}

fn git(cwd: &str, args: &[&str]) -> Option<std::process::Output> {
    let mut a = vec!["-C", cwd];
    a.extend_from_slice(args);
    Command::new("git").args(&a).output().ok()
}

fn is_git_worktree(cwd: &str) -> bool {
    git(cwd, &["rev-parse", "--is-inside-work-tree"])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn is_dirty(cwd: &str) -> bool {
    git(cwd, &["status", "--porcelain"])
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

fn git_stash_create(cwd: &str) -> Option<String> {
    let out = git(cwd, &["stash", "create"])?;
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

fn git_stash_store(cwd: &str, sha: &str) {
    let ts = crate::sensors::host::timestamp();
    let _ = git(cwd, &["stash", "store", "-m", &format!("eldr {ts}"), sha]);
}

fn record_suspended(pid: i32) {
    use std::io::Write;
    let path = config::ensure_data_dir().join("suspended.pids");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{pid}");
    }
}

fn wlog(msg: &str) {
    use std::io::Write;
    let ts = crate::sensors::host::timestamp();
    config::ensure_data_dir();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config::actions_path())
    {
        let _ = writeln!(f, "{ts}  {msg}");
    }
}
