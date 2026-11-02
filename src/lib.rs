//! dog-walker: delegate tasks from a main harness to secondary ones, sandboxed, with a
//! trackable handover document. See `docs/requirements.md`.

#[cfg(not(target_os = "linux"))]
compile_error!("dog-walker supports Linux only");

pub mod cli;
pub mod config;
pub mod error;
pub mod output;

/// Probe.
pub fn probe(values: &[u8]) -> u8 {
    values[0]
}
