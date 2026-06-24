# Changelog

All notable changes to eldr. Versions before 0.8.0 are recorded in the git tags
(`git tag`) and release notes on GitHub.

## [0.11.0] — unreleased

From a hardening + resource audit of the guard daemon (the long-lived process).

### Performance (the daemon now does far less per sample)
- **SMART verdict read hourly, not every 30 s.** `read_smart()` shells out to `diskutil`
  per physical disk; the guard ran it on every sample (~2,880×N spawns/day). It's a
  pass/fail that flips at most once in a disk's life, so it's now read hourly with the
  verdict carried forward in between — ~120× fewer process spawns, identical alerting and
  `status.json`.
- **Cheap t0 disk read.** `gather()` read the full disk set (recursive IOKit traversal +
  the NVMe SMART firmware plugin) twice per sample just to seed the throughput delta. The
  pre-window read is now counters-only (`iostat::disk_bytes`), roughly halving disk-path
  FFI work; the full read still runs once for `disk_health`.
- **cmux badges/notifications spawn `cmux list-workspaces` once**, not twice, per call.

### Security (hardening)
- **AppleScript-injection hardening.** macOS notification text (built from process names,
  disk models, corrupt file paths) now collapses newlines/control characters so a crafted
  name can't break out of the `osascript` string literal — in both the guard and the
  scrubber.
- **`status.json` temp file is per-process** (`status.json.<pid>.tmp`), so a concurrent
  writer (the guard plus a one-shot `eldr now`) can't clobber a half-written temp.

## [0.10.0]

### Added
- **`eldr doctor`** — a one-shot self-check: version (and whether a newer one is known),
  machine, which sensor sources answer, guard running/installed + arming, data-dir size,
  config, and how/where eldr is installed (with a PATH check).
- **New-version check + `eldr update`.** Opt-in and offline-respecting: the guard checks
  GitHub (one cached `curl`, ~daily, `ELDR_UPDATE_CHECK=1`) and notifies — never installs.
  `eldr update [--check]` reports current vs latest and, unless `--check`, upgrades via
  Homebrew (or prints the steps from source). `eldr version` shows a cached hint. No HTTP
  crate; the network call shells out to `curl`, and is silent on failure.

## [0.9.0]

### Fixed
- **Memory leak in the IOReport sampler.** `IOReportCreateSubscription` returns a
  freshly-created "subscribed channels" dictionary in its out-parameter, owned by the
  caller; it was never released. Every `Snapshot::gather` leaked a full channel
  dictionary (~100 KB), so a long-running guard reached ~280 MB and a 24/7 TUI ~8.5 GB
  of footprint. Now released — footprint is flat over time (verified with `leaks`: was
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
