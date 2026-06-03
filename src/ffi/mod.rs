//! Hand-written FFI, isolated per OS subsystem. No `libc`/`core-foundation` crates.
//!
//! - [`mach`]     sysctl, mach host stats (RAM/swap), per-core load, disk, net.
//! - [`cf`]       CoreFoundation opaque types + the entry points Eldr uses.
//! - [`iokit`]    IOKit service matching + registry property reads.
//! - [`ioreport`] the private IOReport telemetry framework (power/freq).
//! - [`iohid`]    IOHID temperature sensors.
//! - [`smc`]      AppleSMC fan telemetry.
//! - [`proc`]     libproc process enumeration + per-process CPU.
//! - [`thermal`]  NSProcessInfo thermal pressure via the objc runtime.

pub mod cf;
pub mod iohid;
pub mod iokit;
pub mod ioreport;
pub mod mach;
pub mod proc;
pub mod smc;
pub mod thermal;
