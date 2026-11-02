//! Task folders and ids: `.dog-walker/tasks/<id>-<slug>/` (R10).
//!
//! An id is a zero-padded counter. It is allocated by creating a marker file for it under
//! `.dog-walker/ids/` with `create_new`, and retrying with the next id when it exists: the
//! marker is created atomically, so two tasks started at the same moment cannot take the same
//! id, and it is never removed, so deleting a task folder does not free its id for reuse.

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
        let root = project.join(".dog-walker");
        let tasks = root.join("tasks");
        let ids = root.join("ids");
        // A previous run inside the project could have replaced any of these with a link to
        // somewhere else; the task folder and its log must stay where the handover says.
        reject_symlink(&root)?;
        reject_symlink(&tasks)?;
        reject_symlink(&ids)?;
        private_dirs(&tasks)?;
        private_dirs(&ids)?;
        let slug = slug(brief);
        let mut id = next_id(&ids)?;
        loop {
            // The marker is the reservation and it stays: created exactly once, it makes the
            // id unique among concurrent runs, and it keeps the counter from going back when
            // a task folder is deleted.
            let marker = ids.join(format!("{id:04}"));
            match fs::File::create_new(&marker) {
                Ok(_) => {}
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                    id = bumped(id)?;
                    continue;
                }
                Err(source) => {
                    return Err(TaskError::Create {
                        path: marker,
                        source,
                    });
                }
            }
            let directory = tasks.join(format!("{id:04}-{slug}"));
            match create_private(&directory) {
                Ok(()) => {
                    return Ok(Task {
                        id: TaskId(id),
                        directory,
                    });
                }
                // A folder with this id and slug was left behind: the id is spent, take the next.
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                    id = bumped(id)?;
                }
                Err(source) => {
                    return Err(TaskError::Create {
                        path: directory,
                        source,
                    });
                }
            }
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

/// Refuses a path that is a symbolic link: a link is how a write leaves the project.
fn reject_symlink(path: &Path) -> Result<(), TaskError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(TaskError::Symlink {
            path: path.to_owned(),
        }),
        Ok(_) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(TaskError::Inspect {
            path: path.to_owned(),
            source,
        }),
    }
}

/// `.dog-walker`, `tasks` and `ids` are as private as the task folders in them.
fn private_dirs(path: &Path) -> Result<(), TaskError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|source| TaskError::Create {
            path: path.to_owned(),
            source,
        })
}

fn bumped(id: u32) -> Result<u32, TaskError> {
    id.checked_add(1).ok_or(TaskError::Exhausted)
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

/// The next id: one past the highest marker. Markers outlive the task folders, so ids are
/// never reused.
fn next_id(ids: &Path) -> Result<u32, TaskError> {
    let entries = fs::read_dir(ids).map_err(|source| TaskError::ReadDir {
        path: ids.to_owned(),
        source,
    })?;
    let mut next = 1;
    for entry in entries {
        let entry = entry.map_err(|source| TaskError::ReadDir {
            path: ids.to_owned(),
            source,
        })?;
        if let Some(id) = id_of(&entry.file_name()) {
            // `u32::MAX` is spent, not the next id: it cannot be handed out twice.
            next = next.max(id.checked_add(1).ok_or(TaskError::Exhausted)?);
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
    fn the_next_id_follows_the_highest_marker() {
        let root = project("next");
        let ids = root.join(".dog-walker").join("ids");
        fs::create_dir_all(&ids).unwrap();
        for marker in ["0004", "0009", "notes", "0010-not-a-marker"] {
            fs::File::create_new(ids.join(marker)).unwrap();
        }
        assert_eq!(next_id(&ids).unwrap(), 11);
        fs::remove_dir_all(&root).unwrap();
    }

    /// The id allocation is atomic: many tasks started at once get distinct ids.
    #[test]
    fn concurrent_creations_never_share_an_id() {
        let root = project("concurrent");
        let threads = 16;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(threads));
        let mut handles = Vec::new();
        for number in 0..threads {
            let root = root.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                Task::create(&root, &format!("brief {number}"))
                    .unwrap()
                    .id()
                    .value()
            }));
        }
        let mut ids: Vec<u32> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), threads, "distinct ids: {ids:?}");
        fs::remove_dir_all(&root).unwrap();
    }

    /// Deleting a task folder does not free its id.
    #[test]
    fn an_id_is_not_reused_after_its_folder_goes() {
        let root = project("never-reused");
        let first = Task::create(&root, "first").unwrap();
        let second = Task::create(&root, "second").unwrap();
        fs::remove_dir_all(second.directory()).unwrap();
        let third = Task::create(&root, "third").unwrap();
        assert_eq!(first.id().to_string(), "0001");
        assert_eq!(second.id().to_string(), "0002");
        assert_eq!(third.id().to_string(), "0003");
        fs::remove_dir_all(&root).unwrap();
    }
}
