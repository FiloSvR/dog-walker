//! Task folders and ids: `.dog-walker/tasks/<id>-<slug>/` (R10).
//!
//! An id is a zero-padded counter. It is allocated by creating a directory and retrying when
//! that name exists, so two tasks started at the same moment cannot take the same id: the
//! counter is reserved under the bare id first, then renamed to `<id>-<slug>`. Reserving the
//! bare id is what makes the allocation slug-independent: two different briefs would not
//! collide on `<id>-<slug>`, and both would be handed the same counter.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use crate::error::TaskError;

/// The longest slug, in characters.
const SLUG_LIMIT: usize = 40;

/// A task's counter. Written zero-padded, at least four digits wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskId(u32);

impl TaskId {
    /// Parses the counter of a task folder name, or of the id itself.
    pub fn parse(text: &str) -> Option<TaskId> {
        id_of(OsStr::new(text)).map(TaskId)
    }

    pub fn value(self) -> u32 {
        self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format!("{:04}", self.0))
    }
}

/// A task folder: the id, and where the handover, the result and the log live.
#[derive(Debug)]
pub struct Task {
    id: TaskId,
    directory: PathBuf,
}

impl Task {
    /// Creates the folder for a new task in `project`.
    pub fn create(project: &Path, brief: &str) -> Result<Task, TaskError> {
        let tasks = project.join(".dog-walker").join("tasks");
        fs::create_dir_all(&tasks).map_err(|source| TaskError::Create {
            path: tasks.clone(),
            source,
        })?;
        let slug = slug(brief);
        let mut id = next_id(&tasks)?;
        loop {
            let reserved = tasks.join(id.to_string());
            match create_private(&reserved) {
                Ok(()) => {}
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                    id = id.checked_add(1).ok_or(TaskError::Exhausted)?;
                    continue;
                }
                Err(source) => {
                    return Err(TaskError::Create {
                        path: reserved,
                        source,
                    });
                }
            }
            let directory = tasks.join(format!("{id:04}-{slug}"));
            // The reservation makes the id unique; this only happens when an older task with
            // the same id and slug was left behind, so the id is taken and the next one is due.
            if directory.exists() {
                let _ = fs::remove_dir(&reserved);
                id = id.checked_add(1).ok_or(TaskError::Exhausted)?;
                continue;
            }
            if let Err(source) = fs::rename(&reserved, &directory) {
                let _ = fs::remove_dir(&reserved);
                return Err(TaskError::Create {
                    path: directory,
                    source,
                });
            }
            return Ok(Task {
                id: TaskId(id),
                directory,
            });
        }
    }

    pub fn id(&self) -> TaskId {
        self.id
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn handover_path(&self) -> PathBuf {
        self.directory.join("handover.md")
    }

    pub fn result_path(&self) -> PathBuf {
        self.directory.join("result.md")
    }

    pub fn log_path(&self) -> PathBuf {
        self.directory.join("harness.log")
    }
}

/// A task folder is private: it holds a brief and a log nobody else needs to read.
fn create_private(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new().mode(0o700).create(path)
}

/// The counter of a task folder name: digits, then either the end or a `-`.
///
/// Bytes, not a lossy string: a folder name that is not UTF-8 simply has no id.
fn id_of(name: &OsStr) -> Option<u32> {
    let bytes = name.as_bytes();
    let end = bytes
        .iter()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(bytes.len());
    if end == 0 {
        return None;
    }
    if bytes.get(end).is_some_and(|byte| *byte != b'-') {
        return None;
    }
    std::str::from_utf8(bytes.get(..end)?).ok()?.parse().ok()
}

/// The next id: one past the highest folder in the tasks directory.
fn next_id(tasks: &Path) -> Result<u32, TaskError> {
    let entries = fs::read_dir(tasks).map_err(|source| TaskError::ReadDir {
        path: tasks.to_owned(),
        source,
    })?;
    let mut next = 1;
    for entry in entries {
        let entry = entry.map_err(|source| TaskError::ReadDir {
            path: tasks.to_owned(),
            source,
        })?;
        if let Some(id) = id_of(&entry.file_name()) {
            next = next.max(id.saturating_add(1));
        }
    }
    Ok(next)
}

/// A folder-name-safe summary of the brief: lowercase ASCII, words joined by `-`.
fn slug(brief: &str) -> String {
    let mut slug = String::new();
    for character in brief.chars() {
        if slug.len() >= SLUG_LIMIT {
            break;
        }
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "task".to_owned()
    } else {
        slug.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn project(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("dog-walker-task-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn ids_are_zero_padded_and_parse_back() {
        let id = TaskId::parse("0007").unwrap();
        assert_eq!(id.value(), 7);
        assert_eq!(id.to_string(), "0007");
        assert_eq!(TaskId::parse("10000").unwrap().to_string(), "10000");
        for bad in ["", "-001", "abc", "00x1"] {
            assert!(TaskId::parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn a_slug_is_safe_for_a_file_name() {
        for (brief, expected) in [
            ("Fix the bug.", "fix-the-bug"),
            ("  ", "task"),
            ("# Heading", "heading"),
            ("a/b:c", "a-b-c"),
            ("Ünïcode text", "n-code-text"),
        ] {
            assert_eq!(slug(brief).trim_end_matches('-'), expected, "{brief:?}");
        }
        assert_eq!(
            slug("abcdefghijklmnopqrstuvwxyz0123456789abcdefghij").len(),
            SLUG_LIMIT
        );
    }

    #[test]
    fn folders_are_numbered_in_order_and_are_private() {
        let root = project("order");
        let first = Task::create(&root, "first task").unwrap();
        let second = Task::create(&root, "second task").unwrap();
        assert_eq!(first.id().to_string(), "0001");
        assert_eq!(second.id().to_string(), "0002");
        assert!(first.directory().ends_with("0001-first-task"));
        let mode = fs::metadata(first.directory())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_next_id_follows_the_highest_folder() {
        let root = project("next");
        let tasks = root.join(".dog-walker").join("tasks");
        fs::create_dir_all(&tasks).unwrap();
        fs::create_dir_all(tasks.join("0004-fourth")).unwrap();
        fs::create_dir_all(tasks.join("0009")).unwrap();
        fs::create_dir_all(tasks.join("notes")).unwrap();
        fs::create_dir_all(tasks.join("0010-not-a-task-1")).unwrap();
        assert_eq!(next_id(&tasks).unwrap(), 11);
        fs::remove_dir_all(&root).unwrap();
    }
}
