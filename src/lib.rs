//! Eldr — a global hardware monitor and protective watchdog for Apple Silicon.
//!
//! Zero external crates: every OS interface is hand-written `extern "C"` FFI over
//! the system frameworks (CoreFoundation, IOKit, IOReport, IOHIDFamily) and libc /
//! mach / sysctl symbols. No `sysinfo`, `ratatui`, `clap`, `serde`, `libc`.
//!
//! Module map:
//! - [`ffi`]     hand-written FFI, isolated per OS subsystem.
//! - [`sensors`] turn raw FFI into a unified [`sensors::snapshot::Snapshot`].
//! - [`ui`]      text (`now`/`status`/`check`) and the owned TUI engine.
//! - [`config`]  `~/.config/eldr/config.toml` read as simple KEY=value.

pub mod config;
pub mod ffi;
pub mod sensors;
pub mod ui;
