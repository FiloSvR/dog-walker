//! Error types. Each module owns its enum; only `cli` turns them into exit codes.

use std::io;
use std::path::PathBuf;

/// Why a config file could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("no config file: pass --config, or set XDG_CONFIG_HOME or HOME")]
    NoConfigPath,
    #[error("cannot read config {}: {source}", path.display())]
    Read { path: PathBuf, source: io::Error },
    #[error("invalid config {}:\n{source}", path.display())]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("config {} defines no profiles", path.display())]
    NoProfiles { path: PathBuf },
    #[error("profile `{profile}`: {reason}")]
    Profile { profile: String, reason: String },
    #[error("exactly one profile must be main, but none is: set `main = true` on one")]
    NoMain,
    #[error("exactly one profile must be main, but these are: {names}")]
    ManyMain { names: String },
}

/// Everything that can make a command fail.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("cannot write output: {0}")]
    Output(#[from] io::Error),
}
