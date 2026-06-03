# Eldr

A global hardware monitor and protective watchdog for Apple Silicon Macs. No sudo,
no external crates — every OS interface is hand-written FFI over the system
frameworks. *Eldr* is Old Norse for "fire".

```
  eldr  Apple M4 Pro (Mac16,11)  8P+4E · 16 GPU   OK (live)
  CPU   P 4512 · E 1991 MHz    44% load ·  43% busy   ▃▃▂▂▄▃▃▃▆▆▆▆
  GPU    338 MHz     4% busy
  Pwr   CPU 13.5 · GPU  0.1 · ANE  0.0 · pkg 13.7 · sys 35.4 W
  Tmp   CPU 88°C · GPU 78°C   fan 1763 rpm (1000–4900)   thermal nominal
  RAM    41.3 / 48.0 GiB  ███████████████░░░  86%
  Dsk   443.6 GiB / 460.4 GiB used   net ↓13 KB/s ↑46 KB/s
  Top   com.apple.Virtualization 6%  cmux 1%  eldr 1%
```

## Why zero crates

The whole binary builds from `std` plus `extern "C"` declarations Eldr writes itself.
There is nothing under `[dependencies]` in `Cargo.toml`, and `Cargo.lock` lists exactly
one package: `eldr`. No `sysinfo`, `ratatui`, `clap`, `serde`, `chrono`, `libc`,
`core-foundation`. The data sources, the JSON emitter, the arg parser, the TUI engine
and the config reader are all hand-rolled.

The readings come from the same no-sudo path Apple's own tools use:

- **IOReport** (private framework) for package/CPU/GPU/ANE power and per-cluster
  frequency residencies.
- **IOHID / SMC** for temperatures (`Tp`/`Te`/`Tg` float sensors, IOHID fallback) and
  fan RPM (`F0Ac`, envelope `F0Mn`/`F0Mx`).
- **mach / sysctl / libproc** for per-core load, RAM/swap, disk, network and the top
  processes.
- **NSProcessInfo** thermal state via the bare Objective-C runtime — the clean throttle
  signal the watchdog gates on.

The IOReport/IOHID/SMC FFI was reimplemented from [macmon](https://github.com/vladkens/macmon)
(MIT) as a reference; Eldr declares its own bindings and does not depend on it.

## Install

```sh
make install          # builds release, copies to ~/.local/bin/eldr
```

Requires a recent Rust toolchain (edition 2024) and an Apple Silicon Mac.

## Commands

```
eldr now                     one-shot snapshot
eldr check                   terse line + exit 0/1/2 (OK/WARN/ALERT) — for agents
eldr status                  panel (live, or the last guard sample)
eldr tui [--interval N]      live dashboard (q or Ctrl-C to quit)

eldr guard [--interval N]    background monitor -> status.json, alerts, interventions
eldr guard-stop              stop a running guard
eldr guard-install           run the guard 24/7 via launchd (start at login, restart on crash)
eldr guard-uninstall         remove the launchd agent
eldr watchdog-test           dry-run: show exactly what an intervention would do

eldr bench <label> [opts]    controlled load -> steady state  (--dur N --interval N --cmd "...")
eldr report <label>          steady-state summary  (--tail N)
eldr compare <a> <b>         iso-load delta + verdict  (--tail N)
```

Agents read `~/.local/share/eldr/status.json` (override the directory with `ELDR_DIR`).
`eldr check` exits `0`/`1`/`2` for OK/WARN/ALERT.

## The watchdog

The guard refreshes `status.json` and, when armed, can take **reversible** action on a
sustained thermal anomaly. The safety model is the point:

- Every action is reversible: Escape to a cmux surface (pauses generation), `SIGSTOP`
  with an automatic `SIGCONT` on recovery, and `git stash create` (a non-destructive
  snapshot of a dirty repo). It never kills, never shuts down, never closes a session.
- A single bad reading cannot fire it: interventions need `ELDR_CONFIRM` consecutive
  critical samples (thermal critical, or a stopped fan).
- A denylist protects this process, running agents, and core system processes from
  being suspended.
- Agents are only ever notified, never sent a prompt they would execute.
- `ELDR_DRYRUN=1` logs intended actions and performs nothing; `eldr watchdog-test`
  previews targeting at any time.

Arming lives in `~/.config/eldr/config.toml` (flat `KEY=value`):

```
ELDR_CMUX=1          # passive badge + notification into cmux workspaces
ELDR_INTERRUPT=0     # Escape to agent surfaces
ELDR_CHECKPOINT=0    # git stash-create dirty agent repos
ELDR_SUSPEND=0       # SIGSTOP the top non-protected CPU hog (auto-SIGCONT)
ELDR_CONFIRM=3       # consecutive critical samples before acting
ELDR_DRYRUN=0        # 1 = log only, perform nothing
```

## Bench discipline

A passive baseline is confounded by ambient drift and unmatched load. To measure
whether (say) a case traps heat, run two matched loads back-to-back the same day in the
same room and compare their steady state:

```sh
eldr bench bare  --dur 1200
eldr bench case  --dur 1200
eldr compare bare case
```

## proto/

`proto/` keeps the original `fanwatch` bash tool that Eldr grew from — the proven
watchdog safety model, the SMC keys, the cmux recipe and the `thermalstate.swift`
helper. It is the prototype and the spec, not part of the build.

## License

MIT — see [LICENSE](LICENSE).
