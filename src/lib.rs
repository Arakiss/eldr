//! Eldr — a global hardware monitor and protective watchdog for Apple Silicon.
//!
//! Zero external crates: every OS interface is hand-written `extern "C"` FFI over
//! the system frameworks (CoreFoundation, IOKit, IOReport, IOHIDFamily) and libc /
//! mach / sysctl symbols. No `sysinfo`, `ratatui`, `clap`, `serde`, `libc`.
//!
// Eldr is an FFI-heavy crate: many thin wrappers take raw CoreFoundation pointers
// and dereference them under the framework's ownership contract. Marking each such
// wrapper `unsafe` would not add safety, only noise.
#![allow(clippy::not_unsafe_ptr_arg_deref)]
// The Snapshot has ~40 fields filled progressively; default-then-assign is clearer
// than a 40-line struct literal.
#![allow(clippy::field_reassign_with_default)]

//! Module map:
//! - [`ffi`]     hand-written FFI, isolated per OS subsystem.
//! - [`sensors`] turn raw FFI into a unified [`sensors::snapshot::Snapshot`].
//! - [`ui`]      text (`now`/`status`/`check`) and the owned TUI engine.
//! - [`config`]  `~/.config/eldr/config.toml` read as simple KEY=value.

// Every reading comes from hand-written FFI over macOS-private frameworks
// (IOReport / IOKit / IOHID / AppleSMC) that exist only on Apple Silicon. Fail with a
// clear message rather than a wall of missing-symbol linker errors on any other target.
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
compile_error!(
    "eldr only supports Apple Silicon macOS (aarch64-apple-darwin). Its readings come \
     from hand-written FFI over macOS-private frameworks that exist only there."
);

pub mod config;
pub mod crypto;
pub mod daemon;
pub mod ffi;
pub mod sensors;
pub mod ui;
pub mod watch;
