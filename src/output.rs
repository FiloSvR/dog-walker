//! The only module that writes output. Stdout carries results; everything else goes to
//! stderr (R6). Exit codes are defined here so they are documented in one place.

use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

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

/// Writes `text` as it is, with no added newline (clap's help and usage text).
pub fn text(writer: &mut dyn Write, text: &str) -> io::Result<()> {
    writer.write_all(text.as_bytes())
}

/// Writes `path` and a newline, byte for byte: stdout holds the handover path (R6), and a
/// project directory that is not valid UTF-8 must not make the path come out wrong.
pub fn path(writer: &mut dyn Write, path: &Path) -> io::Result<()> {
    writer.write_all(path.as_os_str().as_bytes())?;
    writer.write_all(b"\n")
}
