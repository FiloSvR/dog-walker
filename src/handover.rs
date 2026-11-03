//! The handover document: TOML front matter, the brief dog-walker was given and the result
//! the harness wrote (R11, R12). The whole file is rendered from what dog-walker holds in
//! memory, so a status or brief edited by the harness is overwritten with the real one.
//!
//! The front matter is TOML between `+++` lines (D5). It is rendered by hand: every value that
//! reaches it is restricted (the id is digits, the profile name is `ProfileName`, the status
//! comes from `Status`), and `front_matter_is_toml` parses it back with the same crate readers
//! use.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::ProfileName;
use crate::error::HandoverError;
use crate::status::Status;
use crate::task::TaskId;
use crate::usage::Usage;

pub struct Handover {
    id: TaskId,
    profile: ProfileName,
    status: Status,
    brief: String,
    /// What goes under the Result heading: the harness's result, or why the task failed.
    outcome: Option<String>,
    /// The sandbox policy the run had, as a `## Sandbox` section when it is set.
    sandbox: Option<String>,
    /// What the run cost, when the harness reported it.
    usage: Option<Usage>,
}

impl Handover {
    /// A handover for a task that has just been created and has not started.
    pub fn new(id: &TaskId, profile: &ProfileName, brief: &str) -> Handover {
        Handover {
            id: *id,
            profile: profile.clone(),
            status: Status::Created,
            brief: brief.to_owned(),
            outcome: None,
            sandbox: None,
            usage: None,
        }
    }

    /// Records the policy the harness ran under (R16, R20).
    #[must_use]
    pub fn with_sandbox(mut self, policy: &str) -> Handover {
        self.sandbox = Some(policy.to_owned());
        self
    }

    /// Records what the run cost, as the harness reported it (D18).
    pub fn record_usage(&mut self, usage: Option<Usage>) {
        self.usage = usage;
    }

    pub fn status(&self) -> Status {
        self.status
    }

    /// `created` to `running`.
    pub fn start(&mut self) -> Result<(), crate::error::StatusError> {
        self.status = self.status.start()?;
        Ok(())
    }

    /// `created` to `cancelled`, with `reason` under the Result heading: the task stopped
    /// while it was queued, before its harness started.
    pub fn cancel_before_start(&mut self, reason: &str) -> Result<(), crate::error::StatusError> {
        self.status = self.status.cancel_before_start()?;
        self.outcome = Some(reason.to_owned());
        Ok(())
    }

    /// `running` to `to`, with `outcome` under the Result heading.
    pub fn end(&mut self, to: Status, outcome: &str) -> Result<(), crate::error::StatusError> {
        self.status = self.status.end(to)?;
        self.outcome = Some(outcome.to_owned());
        Ok(())
    }

    /// Writes the handover to `path` through a temporary file and a rename, so a reader never
    /// sees half a document.
    ///
    /// The temporary is opened with `create_new`: the task folder is where the harness writes,
    /// and a file or a symlink planted at the temporary's name would otherwise redirect the
    /// write outside the folder. A name already taken is unlinked once — unlinking removes a
    /// link, never what it points at — and the file is created again.
    pub fn save(&self, path: &Path) -> Result<(), HandoverError> {
        let directory = path.parent().unwrap_or(Path::new("."));
        let mut temporary = PathBuf::from(directory);
        temporary.push(format!(
            ".{}.{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("handover.md"),
            std::process::id()
        ));
        let fail = |source: std::io::Error| HandoverError::Write {
            path: path.to_owned(),
            source,
        };
        let mut file = match create_new(&temporary) {
            Ok(file) => file,
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                fs::remove_file(&temporary).map_err(fail)?;
                create_new(&temporary).map_err(fail)?
            }
            Err(source) => return Err(fail(source)),
        };
        file.write_all(self.render().as_bytes()).map_err(fail)?;
        file.sync_all().map_err(fail)?;
        fs::rename(&temporary, path).map_err(fail)?;
        Ok(())
    }

    /// The document as it is written to disk.
    pub fn render(&self) -> String {
        let usage = self.usage.map(|usage| usage.front_matter());
        let front = format!(
            "+++\nid = \"{}\"\nprofile = \"{}\"\nstatus = \"{}\"\n{}+++\n",
            self.id,
            self.profile,
            self.status,
            usage.unwrap_or_default()
        );
        let result = match &self.outcome {
            Some(outcome) => format!("\n## Result\n\n{}\n", outcome.trim_end()),
            None => String::new(),
        };
        let sandbox = match &self.sandbox {
            Some(policy) => format!("## Sandbox\n\n{}\n\n", policy.trim_end()),
            None => String::new(),
        };
        format!(
            "{front}\n# Task {}\n\n{sandbox}## Brief\n\n{}\n{result}",
            self.id,
            self.brief.trim_end()
        )
    }
}

/// Creates `path` for writing, failing if anything is there already, a symlink included.
fn create_new(path: &Path) -> std::io::Result<fs::File> {
    fs::File::create_new(path)
}

/// The front matter, as a reader parses it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrontMatter {
    pub id: String,
    pub profile: String,
    pub status: String,
    /// Tokens the harness reported for the run; absent when it reported none.
    pub tokens: Option<u64>,
    pub turns: Option<u64>,
    pub cost_usd: Option<f64>,
}

/// Reads the front matter of `text`; `None` when there is none or it is not TOML.
pub fn front_matter(text: &str) -> Option<FrontMatter> {
    let rest = text.strip_prefix("+++\n")?;
    let end = rest.find("\n+++\n")?;
    toml::from_str(rest.get(..end)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handover() -> Handover {
        let id = TaskId::parse("0007").unwrap();
        let profile = ProfileName::parse("alpha").unwrap();
        Handover::new(&id, &profile, "Fix the bug.\nKeep it small.")
    }

    #[test]
    fn a_new_handover_is_created_and_has_no_result() {
        let text = handover().render();
        assert!(
            text.starts_with(
                "+++\nid = \"0007\"\nprofile = \"alpha\"\nstatus = \"created\"\n+++\n"
            )
        );
        assert!(text.contains("## Brief\n\nFix the bug.\nKeep it small.\n"));
        assert!(!text.contains("## Result"));
    }

    #[test]
    fn a_finished_handover_has_its_result() {
        let mut handover = handover();
        handover.start().unwrap();
        handover.end(Status::Done, "Changed one file.").unwrap();
        let text = handover.render();
        assert!(text.contains("status = \"done\""));
        assert!(text.contains("## Result\n\nChanged one file.\n"));
    }

    #[test]
    fn a_sandbox_section_sits_before_the_brief() {
        let handover = handover().with_sandbox("- tool: bwrap\n- network: off");
        let text = handover.render();
        let sandbox = text.find("## Sandbox").unwrap();
        let brief = text.find("## Brief").unwrap();
        assert!(sandbox < brief, "{text}");
        assert!(
            text.contains("## Sandbox\n\n- tool: bwrap\n- network: off\n\n## Brief"),
            "{text}"
        );
    }

    #[test]
    fn front_matter_is_toml() {
        let mut handover = handover();
        handover.start().unwrap();
        let parsed = front_matter(&handover.render()).unwrap();
        assert_eq!(parsed.id, "0007");
        assert_eq!(parsed.profile, "alpha");
        assert_eq!(parsed.status, "running");
    }

    #[test]
    fn the_front_matter_has_the_usage_only_when_it_is_known() {
        let mut handover = handover();
        let bare = front_matter(&handover.render()).unwrap();
        assert_eq!(bare.tokens, None);
        handover.record_usage(Some(Usage {
            total: 40317,
            turns: Some(3),
            cost_usd: Some(0.5),
        }));
        let parsed = front_matter(&handover.render()).unwrap();
        assert_eq!(parsed.tokens, Some(40317));
        assert_eq!(parsed.turns, Some(3));
        assert_eq!(parsed.cost_usd, Some(0.5));
        assert_eq!(parsed.status, "created");
    }

    #[test]
    fn a_document_without_front_matter_has_none() {
        assert!(front_matter("# just a file\n").is_none());
        assert!(front_matter("+++\nnot toml\n+++\n").is_none());
    }

    #[test]
    fn the_handover_is_written_atomically() {
        let root = std::env::temp_dir().join(format!("dog-walker-handover-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("handover.md");
        handover().save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("status = \"created\""));
        assert_eq!(
            fs::read_dir(&root).unwrap().count(),
            1,
            "the temporary file is gone"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
