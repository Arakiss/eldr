//! Background operation: the guard loop, launchd integration, cmux fan-out, the
//! armed watchdog (M5), and the bench harness (M6).
//!
//! - [`guard`]   the sample loop -> status.json + passive alerting.
//! - [`cmux`]    subprocess wrapper for cmux badges/notifications.
//! - [`launchd`] guard-install / guard-uninstall.

pub mod bench;
pub mod cmux;
pub mod guard;
pub mod launchd;
pub mod maint;
pub mod notify;
pub mod scrub;
pub mod watchdog;
