//! Presentation: text output (`now`/`status`/`check`) and the owned TUI engine.
//!
//! - [`pretty`] one-shot human-readable panels and the terse `check` line.
//! - [`term`]   owned termios raw mode + ANSI primitives.
//! - [`tui`]    the live dashboard.

pub mod pretty;
pub mod style;
pub mod term;
pub mod tui;
