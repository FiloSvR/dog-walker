//! Profiles and the config file that defines them (R1, R2).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::ConfigError;

/// The harnesses dog-walker can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    Claude,
    Codex,
    Pi,
    Dsh,
}

impl Harness {
    pub const ALL: [Harness; 4] = [Harness::Claude, Harness::Codex, Harness::Pi, Harness::Dsh];

    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Pi => "pi",
            Harness::Dsh => "dsh",
        }
    }

    fn parse(text: &str) -> Option<Harness> {
        Harness::ALL
            .into_iter()
            .find(|harness| harness.as_str() == text)
    }
}

impl fmt::Display for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A profile's name: non-empty, and safe to use later in file names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProfileName(String);

impl ProfileName {
    pub fn parse(text: &str) -> Result<Self, String> {
        let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
        if text.is_empty() || !text.chars().all(allowed) || text.starts_with('.') {
            return Err(
                "a profile name uses only letters, digits, `-`, `_` and `.`, and does not start with `.`".to_owned(),
            );
        }
        Ok(Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProfileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One validated profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: ProfileName,
    pub harness: Harness,
    pub model: String,
    pub effort: Option<String>,
    pub timeout_minutes: Option<u32>,
    /// Overrides the adapter's default command: an argv list.
    pub command: Option<Vec<String>>,
    pub main: bool,
}

/// A validated config: at least one profile, exactly one of them main.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The main profile first, then the others by name.
    profiles: Vec<Profile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    profiles: BTreeMap<String, RawProfile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    harness: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    timeout_minutes: Option<u32>,
    command: Option<Vec<String>>,
    #[serde(default)]
    main: bool,
}

impl Config {
    /// Reads and validates the config file at `path`.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        Config::parse(&text, path)
    }

    /// Validates config `text`; `origin` is only used in error messages.
    pub fn parse(text: &str, origin: &Path) -> Result<Config, ConfigError> {
        let raw: RawConfig = toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: origin.to_owned(),
            source,
        })?;
        if raw.profiles.is_empty() {
            return Err(ConfigError::NoProfiles {
                path: origin.to_owned(),
            });
        }
        let mut profiles = Vec::with_capacity(raw.profiles.len());
        for (name, profile) in raw.profiles {
            profiles.push(validate(&name, profile)?);
        }
        let mains: Vec<&Profile> = profiles.iter().filter(|profile| profile.main).collect();
        match mains.len() {
            1 => {}
            0 => return Err(ConfigError::NoMain),
            _ => {
                let names: Vec<String> = mains
                    .iter()
                    .map(|profile| format!("`{}`", profile.name))
                    .collect();
                return Err(ConfigError::ManyMain {
                    names: names.join(", "),
                });
            }
        }
        // `false` sorts before `true`, so negate to put the main profile first;
        // the sort is stable, so the others stay in name order.
        profiles.sort_by_key(|profile| !profile.main);
        Ok(Config { profiles })
    }

    /// The profiles, the main one first.
    pub fn profiles(&self) -> &[Profile] {
        &self.profiles
    }
}

fn validate(name: &str, raw: RawProfile) -> Result<Profile, ConfigError> {
    let fail = |reason: String| ConfigError::Profile {
        profile: name.to_owned(),
        reason,
    };

    let profile_name = ProfileName::parse(name).map_err(fail)?;
    let harness_text = raw
        .harness
        .ok_or_else(|| fail("missing required field `harness`".to_owned()))?;
    let harness = Harness::parse(&harness_text).ok_or_else(|| {
        fail(format!(
            "unknown harness `{harness_text}` (expected claude, codex, pi or dsh)"
        ))
    })?;
    let model = raw
        .model
        .ok_or_else(|| fail("missing required field `model`".to_owned()))?;
    check_text("model", &model).map_err(fail)?;
    if let Some(effort) = &raw.effort {
        check_text("effort", effort).map_err(fail)?;
        if effort == "-" {
            return Err(fail(
                "`effort` cannot be `-`: `profiles` prints that for no effort".to_owned(),
            ));
        }
    }
    if raw.timeout_minutes == Some(0) {
        return Err(fail("`timeout_minutes` must be greater than 0".to_owned()));
    }
    if raw
        .command
        .as_ref()
        .is_some_and(|argv| argv.first().is_none_or(String::is_empty))
    {
        return Err(fail("`command` must start with a program name".to_owned()));
    }
    Ok(Profile {
        name: profile_name,
        harness,
        model,
        effort: raw.effort,
        timeout_minutes: raw.timeout_minutes,
        command: raw.command,
        main: raw.main,
    })
}

/// A text field must hold something printable, so a row of `profiles` can always be split.
fn check_text(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("`{field}` must not be empty"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("`{field}` must not contain control characters"));
    }
    Ok(())
}

/// Where the config file is when `--config` is not given: `$XDG_CONFIG_HOME` if it is an
/// absolute path, otherwise `$HOME/.config`.
pub fn default_path(xdg_config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = match xdg_config_home.map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir,
        _ => PathBuf::from(home.filter(|home| !home.is_empty())?).join(".config"),
    };
    Some(base.join("dog-walker").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config, ConfigError> {
        Config::parse(text, Path::new("test.toml"))
    }

    #[test]
    fn main_profile_comes_first_then_names_in_order() {
        let config = parse(
            "[profiles.b]\nharness='pi'\nmodel='m'\n[profiles.a]\nharness='pi'\nmodel='m'\n\
             [profiles.z]\nharness='claude'\nmodel='m'\nmain=true\n",
        )
        .unwrap();
        let names: Vec<&str> = config.profiles().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["z", "a", "b"]);
    }

    #[test]
    fn optional_fields_are_kept() {
        let config = parse(
            "[profiles.a]\nharness='dsh'\nmodel='m'\neffort='high'\ntimeout_minutes=5\n\
             command=['/bin/x','-y']\nmain=true\n",
        )
        .unwrap();
        let profile = &config.profiles()[0];
        assert_eq!(profile.harness, Harness::Dsh);
        assert_eq!(profile.effort.as_deref(), Some("high"));
        assert_eq!(profile.timeout_minutes, Some(5));
        assert_eq!(
            profile.command.as_deref(),
            Some(&["/bin/x".to_owned(), "-y".to_owned()][..])
        );
    }

    #[test]
    fn zero_timeout_and_empty_command_are_rejected() {
        assert!(
            parse("[profiles.a]\nharness='pi'\nmodel='m'\nmain=true\ntimeout_minutes=0\n").is_err()
        );
        assert!(parse("[profiles.a]\nharness='pi'\nmodel='m'\nmain=true\ncommand=[]\n").is_err());
    }

    #[test]
    fn profile_names_are_restricted() {
        assert!(ProfileName::parse("pi-local_2.1").is_ok());
        for bad in ["", ".hidden", "a b", "a/b", "é"] {
            assert!(ProfileName::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn no_profiles_is_an_error() {
        assert!(matches!(parse(""), Err(ConfigError::NoProfiles { .. })));
    }

    #[test]
    fn default_path_prefers_an_absolute_xdg_dir_then_home() {
        let some = |s: &str| Some(OsString::from(s));
        assert_eq!(
            default_path(some("/x"), some("/h")),
            Some(PathBuf::from("/x/dog-walker/config.toml"))
        );
        assert_eq!(
            default_path(some("relative"), some("/h")),
            Some(PathBuf::from("/h/.config/dog-walker/config.toml"))
        );
        assert_eq!(
            default_path(None, some("/h")),
            Some(PathBuf::from("/h/.config/dog-walker/config.toml"))
        );
        assert_eq!(default_path(None, None), None);
        assert_eq!(default_path(None, some("")), None);
    }
}
