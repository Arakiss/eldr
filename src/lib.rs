//! Eldr — a global hardware monitor and protective watchdog for Apple Silicon.
//!
//! Zero external crates: every OS interface is hand-written `extern "C"` FFI over
//! the system frameworks (CoreFoundation, IOKit, IOReport, IOHIDFamily) and libc /
//! mach / sysctl symbols. No `sysinfo`, `ratatui`, `clap`, `serde`, `libc`.
//!
// Eldr is an FFI-heavy crate: many thin wrappers take raw CoreFoundation pointers
// and dereference them under the framework's ownership contract. Marking each such
// wrapper `unsafe` would not add safety, only noise — same stance as macmon.
#![allow(clippy::not_unsafe_ptr_arg_deref)]
// The Snapshot has ~40 fields filled progressively; default-then-assign is clearer
// than a 40-line struct literal.
#![allow(clippy::field_reassign_with_default)]

//! Module map:
//! - [`ffi`]     hand-written FFI, isolated per OS subsystem.
//! - [`sensors`] turn raw FFI into a unified [`sensors::snapshot::Snapshot`].
//! - [`ui`]      text (`now`/`status`/`check`) and the owned TUI engine.
//! - [`config`]  `~/.config/eldr/config.toml` read as simple KEY=value.

pub mod config;
pub mod daemon;
pub mod ffi;
pub mod sensors;
pub mod ui;
