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
use std::os::unix::fs::MetadataExt;
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
    /// The device and inode the folder was created with, so a later write can tell that the
    /// path still leads to that folder and not to a link the harness left behind.
    device: u64,
    inode: u64,
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
        let mut id = next_id(&tasks, &ids)?;
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
                    let metadata =
                        fs::symlink_metadata(&directory).map_err(|source| TaskError::Inspect {
                            path: directory.clone(),
                            source,
                        })?;
                    return Ok(Task {
                        id: TaskId(id),
                        directory,
                        device: metadata.dev(),
                        inode: metadata.ino(),
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

    /// Checks that the folder is still the one this task created: not renamed away, not
    /// replaced by a link. The harness writes in the task folder, and a later write must not
    /// follow it anywhere else.
    pub fn verify(&self) -> Result<(), TaskError> {
        let metadata = match fs::symlink_metadata(&self.directory) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Err(TaskError::Moved {
                    path: self.directory.clone(),
                });
            }
            Err(source) => {
                return Err(TaskError::Inspect {
                    path: self.directory.clone(),
                    source,
                });
            }
        };
        if !metadata.is_dir() || metadata.dev() != self.device || metadata.ino() != self.inode {
            return Err(TaskError::Moved {
                path: self.directory.clone(),
            });
        }
        Ok(())
    }

    /// The task with `id` in `project`, if there is one.
    pub fn find(project: &Path, id: TaskId) -> Result<Option<Task>, TaskError> {
        let root = project.join(".dog-walker");
        let tasks = root.join("tasks");
        reject_symlink(&root)?;
        reject_symlink(&tasks)?;
        let entries = match fs::read_dir(&tasks) {
            Ok(entries) => entries,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(TaskError::ReadDir {
                    path: tasks,
                    source,
                });
            }
        };
        for entry in entries {
            let entry = entry.map_err(|source| TaskError::ReadDir {
                path: tasks.clone(),
                source,
            })?;
            if id_of(&entry.file_name()) == Some(id.value()) {
                let directory = entry.path();
                let metadata =
                    fs::symlink_metadata(&directory).map_err(|source| TaskError::Inspect {
                        path: directory.clone(),
                        source,
                    })?;
                if !metadata.is_dir() {
                    continue;
                }
                return Ok(Some(Task {
                    id,
                    directory,
                    device: metadata.dev(),
                    inode: metadata.ino(),
                }));
            }
        }
        Ok(None)
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

/// The next id: one past the highest marker or task folder. Markers outlive the folders, so
/// ids are not reused; folders are counted too, for a tree made before the markers existed or
/// a task folder copied in by hand.
fn next_id(tasks: &Path, ids: &Path) -> Result<u32, TaskError> {
    let next = highest(tasks, 1)?;
    highest(ids, next)
}

/// One past the highest id in `directory`.
fn highest(directory: &Path, mut high: u32) -> Result<u32, TaskError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(high),
        Err(source) => {
            return Err(TaskError::ReadDir {
                path: directory.to_owned(),
                source,
            });
        }
    };
    for entry in entries {
        let entry = entry.map_err(|source| TaskError::ReadDir {
            path: directory.to_owned(),
            source,
        })?;
        if let Some(id) = id_of(&entry.file_name()) {
            // `u32::MAX` is spent, not the next id: it cannot be handed out twice.
            high = high.max(id.checked_add(1).ok_or(TaskError::Exhausted)?);
        }
    }
    Ok(high)
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
        let tasks = root.join(".dog-walker").join("tasks");
        let ids = root.join(".dog-walker").join("ids");
        fs::create_dir_all(&tasks).unwrap();
        fs::create_dir_all(&ids).unwrap();
        for marker in ["0004", "0009", "notes", "0010-not-a-marker"] {
            fs::File::create_new(ids.join(marker)).unwrap();
        }
        assert_eq!(next_id(&tasks, &ids).unwrap(), 11);
        fs::remove_dir_all(&root).unwrap();
    }

    /// A task folder left without a marker (an older tree, a copy) still counts.
    #[test]
    fn a_folder_without_a_marker_still_counts() {
        let root = project("folder-next");
        let tasks = root.join(".dog-walker").join("tasks");
        let ids = root.join(".dog-walker").join("ids");
        fs::create_dir_all(tasks.join("0007-legacy")).unwrap();
        fs::create_dir_all(&ids).unwrap();
        assert_eq!(next_id(&tasks, &ids).unwrap(), 8);
        fs::remove_dir_all(&root).unwrap();
    }

    /// The counter ends at `u32::MAX` rather than handing that id out twice.
    #[test]
    fn the_last_id_is_spent() {
        let root = project("exhausted");
        let tasks = root.join(".dog-walker").join("tasks");
        let ids = root.join(".dog-walker").join("ids");
        fs::create_dir_all(&tasks).unwrap();
        fs::create_dir_all(&ids).unwrap();
        fs::File::create_new(ids.join("4294967295")).unwrap();
        assert!(matches!(
            Task::create(&root, "a brief"),
            Err(TaskError::Exhausted)
        ));
        fs::remove_dir_all(&root).unwrap();
    }

    /// A `.dog-walker` that is a link is refused, not followed.
    #[test]
    fn a_symlinked_tasks_root_is_refused() {
        let root = project("symlinked");
        let elsewhere = root.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join(".dog-walker")).unwrap();
        assert!(matches!(
            Task::create(&root, "a brief"),
            Err(TaskError::Symlink { .. })
        ));
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
        fs::remove_dir_all(&root).unwrap();
    }

    /// A folder moved after it was created is refused: the handover must not follow it.
    #[test]
    fn a_task_folder_that_moved_is_refused() {
        let root = project("moved");
        let task = Task::create(&root, "a brief").unwrap();
        task.verify().unwrap();
        fs::rename(task.directory(), root.join("elsewhere")).unwrap();
        assert!(matches!(task.verify(), Err(TaskError::Moved { .. })));
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
