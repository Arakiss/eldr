# proto — the fanwatch prototype

Eldr grew out of `fanwatch`, a bash thermal/fan tool with a cmux-integrated protective
watchdog. The bash version hit its ceiling (bash 3.2 fought unset arrays under `set -u`,
traps that didn't exit, multibyte concat), so Eldr is the Rust rewrite — broader (per-core
CPU, RAM, GPU/ANE, power, disk, net, processes, not just fans) and dependency-free.

This directory keeps the prototype as the spec. It is **not** part of the Rust build.

- `fanwatch` — the original bash tool. The proven watchdog safety model (reversible
  actions, CONFIRM gate, denylist, dry-run, auto-undo), the SMC keys (`F0Ac`/`F0Mn`/
  `F0Mx`), the cmux recipe and the launchd plist all originate here.
- `watchdog.conf.example` — the arming config the watchdog reads.
- `thermalstate.swift` — the 3-line helper the bash tool used for `NSProcessInfo`
  thermal state. Eldr reimplements this in Rust via the bare objc runtime.

What carried over into Eldr, and where:

| prototype                         | Eldr                                           |
| --------------------------------- | ---------------------------------------------- |
| `smctemp` fan key `F0Ac`          | `src/ffi/smc.rs` (hand-written AppleSMC FFI)   |
| `thermalstate` Swift helper       | `src/ffi/thermal.rs` (bare `objc_msgSend`)     |
| watchdog reversible interventions | `src/daemon/watchdog.rs`                        |
| cmux badge / notify recipe        | `src/daemon/cmux.rs`                            |
| launchd guard install             | `src/daemon/launchd.rs`                         |
| bench / report / compare          | `src/daemon/bench.rs`                           |
