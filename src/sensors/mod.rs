//! Turn raw FFI into a unified [`snapshot::Snapshot`].
//!
//! - [`soc`]      Apple Silicon identity + IOReport power/freq (the heart, M1).
//! - [`host`]     RAM/swap, per-core load, disk, net, uptime, processes.
//! - [`snapshot`] the unified data contract every consumer reads.

pub mod host;
pub mod snapshot;
pub mod soc;
