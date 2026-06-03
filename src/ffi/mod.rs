//! Hand-written FFI, isolated per OS subsystem. No `libc`/`core-foundation` crates.
//!
//! - [`mach`]     sysctl, mach host stats (RAM/swap), per-core load.
//! - [`cf`]       CoreFoundation opaque types + the entry points Eldr uses.
//! - [`iokit`]    IOKit service matching + registry property reads.
//! - [`ioreport`] the private IOReport telemetry framework (power/freq).
//! - more modules (iohid, smc, thermal) land in M2.

pub mod cf;
pub mod ioreport;
pub mod iokit;
pub mod mach;
