//! The only module that writes output. Stdout carries results; everything else goes to
//! stderr (R6). Exit codes are defined here so they are documented in one place.

use std::io::{self, Write};

/// Process exit codes.
pub mod exit {
    /// The command did what was asked (for `run`: the task ended `done`).
    pub const SUCCESS: u8 = 0;
    /// A task ended in any state but `done` (used by `run`).
    pub const TASK_NOT_DONE: u8 = 1;
    /// Bad usage, a config error, or an I/O failure.
    pub const ERROR: u8 = 2;
}

/// Writes `text` and a newline.
pub fn line(writer: &mut dyn Write, text: &str) -> io::Result<()> {
    writeln!(writer, "{text}")
}
