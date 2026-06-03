//! Proves the watchdog's two destructive-looking-but-reversible primitives are
//! actually reversible and non-destructive, the same property the bash prototype
//! guaranteed: SIGSTOP is undone by SIGCONT (auto-undo via `unintervene`), and
//! `git stash create` snapshots a dirty tree WITHOUT modifying it.

use eldr::daemon::watchdog::Watchdog;
use std::process::Command;

unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}
const SIGSTOP: i32 = 17;
const SIGKILL: i32 = 9;

fn proc_state(pid: i32) -> String {
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn sigstop_is_reversed_by_unintervene() {
    let dir = "/tmp/eldr-revtest-sig";
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::remove_file(format!("{dir}/suspended.pids"));
    // SAFETY: test-only env override read by eldr::config.
    unsafe { std::env::set_var("ELDR_DIR", dir) };

    let mut child = Command::new("sleep")
        .arg("60")
        .spawn()
        .expect("spawn sleep");
    let pid = child.id() as i32;
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert!(!proc_state(pid).contains('T'), "child should start running");

    // Suspend (what `do_suspend` does), record it (what `record_suspended` does).
    assert_eq!(unsafe { kill(pid, SIGSTOP) }, 0);
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        proc_state(pid).contains('T'),
        "child must be STOPPED after SIGSTOP"
    );
    std::fs::write(format!("{dir}/suspended.pids"), format!("{pid}\n")).unwrap();

    // Auto-undo: unintervene must SIGCONT it and clear the ledger.
    Watchdog::default().unintervene();
    std::thread::sleep(std::time::Duration::from_millis(200));
    let st = proc_state(pid);
    assert!(
        !st.contains('T'),
        "child must be RESUMED after unintervene, got {st:?}"
    );
    assert!(
        !std::path::Path::new(&format!("{dir}/suspended.pids")).exists(),
        "suspended.pids must be cleared"
    );

    unsafe { kill(pid, SIGKILL) };
    let _ = child.wait(); // reap, no zombie
}

#[test]
fn git_stash_create_is_nondestructive_and_recoverable() {
    let dir = "/tmp/eldr-revtest-git";
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    let git = |args: &[&str]| {
        Command::new("git")
            .args(["-C", dir])
            .args(args)
            .output()
            .expect("git")
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "t"]);
    std::fs::write(format!("{dir}/f.txt"), "original\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "init"]);

    // Make the tree dirty (an agent's uncommitted work).
    std::fs::write(format!("{dir}/f.txt"), "AGENT WORK IN PROGRESS\n").unwrap();
    assert!(
        !git(&["status", "--porcelain"]).stdout.is_empty(),
        "tree should be dirty"
    );

    // The exact watchdog flow: stash create -> store.
    let sha = String::from_utf8_lossy(&git(&["stash", "create"]).stdout)
        .trim()
        .to_string();
    assert!(
        !sha.is_empty(),
        "stash create should yield a sha for a dirty tree"
    );
    git(&["stash", "store", "-m", "eldr test", &sha]);

    // Non-destructive: the working tree is UNCHANGED (the dirty content remains).
    let content = std::fs::read_to_string(format!("{dir}/f.txt")).unwrap();
    assert_eq!(
        content, "AGENT WORK IN PROGRESS\n",
        "stash create must not touch the tree"
    );
    assert!(
        !git(&["status", "--porcelain"]).stdout.is_empty(),
        "tree must still be dirty"
    );

    // Recoverable: the snapshot is recorded in the stash list...
    let list = String::from_utf8_lossy(&git(&["stash", "list"]).stdout).into_owned();
    assert!(
        list.contains("eldr test"),
        "stash should be recorded, got: {list}"
    );

    // ...and after reverting the tree, applying the snapshot restores the agent work.
    git(&["checkout", "--", "f.txt"]);
    assert_eq!(
        std::fs::read_to_string(format!("{dir}/f.txt")).unwrap(),
        "original\n"
    );
    assert!(
        git(&["stash", "apply", &sha]).status.success(),
        "stash must apply onto clean tree"
    );
    assert_eq!(
        std::fs::read_to_string(format!("{dir}/f.txt")).unwrap(),
        "AGENT WORK IN PROGRESS\n",
        "applying the snapshot must restore the agent's uncommitted work"
    );

    let _ = std::fs::remove_dir_all(dir);
}
