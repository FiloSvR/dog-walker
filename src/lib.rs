//! dog-walker: delegate tasks from a main harness to secondary ones, sandboxed, with a
//! trackable handover document. See `docs/requirements.md`.

#[cfg(not(target_os = "linux"))]
compile_error!("dog-walker supports Linux only");

pub mod cli;
pub mod config;
pub mod error;
pub mod handover;
pub mod launch;
pub mod output;
pub mod status;
pub mod task;
