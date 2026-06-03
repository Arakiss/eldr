//! Hand-written FFI, isolated per OS subsystem. No `libc`/`core-foundation` crates.
//!
//! - [`mach`] sysctl, mach host stats (RAM/swap), per-core load.
//! - more modules (cf, ioreport, iohid, smc, thermal) land in later milestones.

pub mod mach;
