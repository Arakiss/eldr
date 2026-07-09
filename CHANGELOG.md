# Changelog

All notable changes to eldr. Versions before 0.8.0 are recorded in the git tags
(`git tag`) and release notes on GitHub.

## [0.11.6] - 2026-07-09

### Added
- **Per-workspace cmux resource badges.** The guard can show aggregate CPU, RAM, and process
  count on each workspace before you switch to it.

### Changed
- **Badge refreshes use aggregate workspace rows and skip unchanged labels.** Eldr no longer
  asks cmux for every child process just to update a workspace summary, and it avoids needless
  status writes while periodically reconciling after a cmux restart.
- **The launchd guard targets the active cmux automation socket.** Resource badges can reach
  the live cmux server instead of silently addressing a stale socket.

### Fixed
- **Routine thermal conditions no longer spam every cmux tab.** Thermal badges are reserved for
  serious cooling pressure or a fan fault, and passive thermal notifications are less noisy.

## [0.11.4] - 2026-06-24

From a correctness & robustness audit (15 findings examined, each adversarially verified;
the 11 confirmed, all low-risk, applied here).

### Fixed
- **Tiny terminals no longer scroll the header off.** `clamp_lines(0)` was a no-op, so at
  `rows <= 6` a tab's whole body was emitted; and below the chrome floor there was no
  fallback. Now `clamp_lines(0)` clears the body and `rows < 6` shows a single clipped line.
  The overflow regression test now sweeps the 1..8-row danger zone.
- **Watchdog suspends the first non-protected hog**, not only the #1 process: if the top
  CPU process is protected (a shell, an agent, a terminal), it now falls through to a real
  reversible target instead of giving up.
- **No double `/s/s`** on the Storage tab's total-I/O line.
- **Robustness against odd inputs (no panics):** `host_processor_info` is bounded by the
  integer count the kernel actually returned (no out-of-bounds read on a short reply);
  memory page-count sums are computed in u64 (no u32 overflow on very large RAM);
  filesystem size = blocks × block-size uses `saturating_mul`.
- **Disk throughput** divides the byte delta by the real elapsed window (not the nominal
  duration), matching how network rates are computed.
- **Update check** ignores a non-string `tag_name` (draft/malformed GitHub JSON) instead of
  mis-parsing the next field.
- **SMART carry-forward** drops cached verdicts for disks no longer present, so a reused
  `bsd_name` can't inherit a removed disk's stale verdict.
- The footer bottom-pad loop is O(rows) instead of O(rows²).

## [0.11.3] - 2026-06-24

### Fixed
- **The header (with the version) no longer scrolls off the top.** The panel emitted a
  trailing newline on the last row, which scrolled the whole frame up one line and pushed
  the `eldr vX.Y.Z` header off-screen. The final newline is now dropped so the cursor stays
  on the bottom row; no scroll, header always visible. Regression-tested (every tab emits
  strictly fewer newlines than the terminal has rows).

## [0.11.2]

### Changed
- **The TUI version is shown in the brand (fire) colour** in the header (`eldr vX.Y.Z`),
  so it stands out and is unmistakable at a glance (was dim).

## [0.11.1]

### Added
- **TUI shows the eldr version** in the header (`eldr vX.Y.Z`), so it's easy to tell which
  build is on screen.

### Security / hardening (the deferred audit findings)
- **Data directory is now owner-only (0700)** and the pid file 0600, so another local user
  can't read status, logs (which name processes), the pid, or scrub manifests.
- **`running_pid()` validates identity**: it confirms the pid is actually an `eldr` process
  (via libproc) before treating it as the guard, so a recycled pid can't suppress a restart
  or make `guard-stop` SIGTERM an unrelated process.
- **history.csv written atomically** (temp + rename), so the TUI never reads a torn file.
- **launchd plist `PATH` puts system dirs first** (`/usr/bin:/bin:…` before Homebrew /
  `~/.local/bin`), so the long-lived guard resolves system tools from system locations.

## [0.11.0]

From a hardening + resource audit of the guard daemon (the long-lived process).

### Performance (the daemon now does far less per sample)
- **SMART verdict read hourly, not every 30 s.** `read_smart()` shells out to `diskutil`
  per physical disk; the guard ran it on every sample (~2,880×N spawns/day). It's a
  pass/fail that flips at most once in a disk's life, so it's now read hourly with the
  verdict carried forward in between; about 120× fewer process spawns, identical alerting and
  `status.json`.
- **Cheap t0 disk read.** `gather()` read the full disk set (recursive IOKit traversal +
  the NVMe SMART firmware plugin) twice per sample just to seed the throughput delta. The
  pre-window read is now counters-only (`iostat::disk_bytes`), roughly halving disk-path
  FFI work; the full read still runs once for `disk_health`.
- **cmux badges/notifications spawn `cmux list-workspaces` once**, not twice, per call.

### Security (hardening)
- **AppleScript-injection hardening.** macOS notification text (built from process names,
  disk models, corrupt file paths) now collapses newlines/control characters so a crafted
  name can't break out of the `osascript` string literal; this applies to both the guard and the
  scrubber.
- **`status.json` temp file is per-process** (`status.json.<pid>.tmp`), so a concurrent
  writer (the guard plus a one-shot `eldr now`) can't clobber a half-written temp.

## [0.10.0]

### Added
- **`eldr doctor`**: a one-shot self-check for the version (and whether a newer one is known),
  machine, which sensor sources answer, guard running/installed + arming, data-dir size,
  config, and how/where eldr is installed (with a PATH check).
- **New-version check + `eldr update`.** Opt-in and offline-respecting: the guard checks
  GitHub (one cached `curl`, ~daily, `ELDR_UPDATE_CHECK=1`) and notifies; it never installs.
  `eldr update [--check]` reports current vs latest and, unless `--check`, upgrades via
  Homebrew (or prints the steps from source). `eldr version` shows a cached hint. No HTTP
  crate; the network call shells out to `curl`, and is silent on failure.

## [0.9.0]

### Fixed
- **Memory leak in the IOReport sampler.** `IOReportCreateSubscription` returns a
  freshly-created "subscribed channels" dictionary in its out-parameter, owned by the
  caller; it was never released. Every `Snapshot::gather` leaked a full channel
  dictionary (~100 KB), so a long-running guard reached ~280 MB and a 24/7 TUI ~8.5 GB
  of footprint. Now released, footprint is flat over time (verified with `leaks`: was
  64 440 leaks / 4 MB in 30 s, now 0). This was the monitor quietly becoming the hog.
- **Mounted disk images no longer show as volumes.** Read-only mounts under `/Volumes`
  (an app's `.dmg`, e.g. the "Otty" terminal) were listed as storage; they're now filtered
  out. The boot volume is kept even though it's a sealed read-only system volume.

### Added
- **Network tab.** A dedicated view with tall download/upload braille charts (filling the
  height like the rest of the dashboard), current rates, peaks, and totals since boot.
- **Live disk I/O throughput.** Per-physical-disk read/write bytes/s, measured over the
  sample window like the network rates. Shown on the TUI Storage tab (per disk + total)
  and added to `status.json` (`read_rate`/`write_rate`).
- **Configurable resource-hog thresholds.** `ELDR_HOG_CPU` (percent) and `ELDR_HOG_RAM`
  (fraction of RAM) in `config.toml` tune when the guard notifies; defaults 300% / 0.15.
- **Resource-hog callout in `eldr now`.** The one-shot panel flags a CPU/RAM hog in red,
  matching the TUI and guard.

## [0.8.0]

### Added
- Responsive Banner HUD / dashboard-wall TUI that fills the full width and height, tuned
  for ultra-wide screens; every tab is height-aware and degrades to compact lanes when
  narrow. New std-only graphics module (`ui/chart.rs`): braille area charts, gradient
  bars, column compositor.
- Automatic terminal-size detection via cursor-position report (`ESC[6n`) when the
  winsize ioctl is stale (e.g. inside a multiplexer surface), with `ELDR_COLS`/`ELDR_ROWS`
  override.
- Resource-hog alerts: the guard passively notifies on a process pinning the CPU, holding
  a large share of RAM, or memory under sustained pressure; the Overview flags it in red.

See the GitHub release for the full 0.8.0 notes.
