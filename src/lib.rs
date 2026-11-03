//! dog-walker: delegate tasks from a main harness to secondary ones, sandboxed, with a
//! trackable handover document. See `docs/requirements.md`.

#[cfg(not(target_os = "linux"))]
compile_error!("dog-walker supports Linux only");

pub mod adapter;
pub mod cli;
pub mod config;
pub mod doctor;
pub mod envfile;
pub mod error;
pub mod handover;
pub mod launch;
pub mod output;
pub mod sandbox;
pub mod setup;
pub mod slots;
pub mod status;
pub mod task;
pub mod usage;
