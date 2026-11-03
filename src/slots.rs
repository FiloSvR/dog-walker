//! Several tasks at once, with limits per harness and model, and a warning when two of them
//! write the same project (R9).
//!
//! Coordination is a directory of small records: `.dog-walker/run/<id>.slot`, one per running
//! task, with the pid and start time of its runner. A record whose runner is gone is stale
//! and is cleaned up; a runner that would exceed the limit for its harness and model waits for
//! one to disappear. The count is advisory (two runners can see the same free slot), which is
//! enough for a limit that queues rather than schedules.
//!
//! The records live inside the project, and the sandbox mounts this directory read-only: the
//! runner owns them, and a harness that could write one could block the next run.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process;
use std::thread;
use std::time::Duration;

use rustix::fs::{Mode, OFlags};

use crate::config::Profile;
use crate::error::SlotError;

/// Where the running records live, inside the project.
const RUN_DIR: &str = ".dog-walker/run";

/// How long a waiting runner sleeps between looks at the records.
const POLL: Duration = Duration::from_millis(200);

/// A record is a handful of lines; anything larger is not one.
const MAX_RECORD: u64 = 4 * 1024;

/// A held slot: the record exists while this lives, and is removed when it ends.
#[derive(Debug)]
pub struct Slot {
    path: PathBuf,
    /// The ids of the other runs that were live when this one started.
    others: Vec<String>,
}

impl Slot {
    /// The other tasks that were running when this one took its slot: every task writes the
    /// project, so each of them is an overlapping write scope.
    pub fn overlaps(&self) -> &[String] {
        &self.others
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Takes a slot for `profile`, waiting while the limit for its harness and model is reached.
///
/// `limit` is `None` when the config sets none. `should_stop` is asked between looks: a queued
/// task that is cancelled or interrupted returns `None` without ever starting its harness.
pub fn acquire(
    project: &Path,
    profile: &Profile,
    limit: Option<usize>,
    id: &str,
    should_stop: &dyn Fn() -> bool,
    on_wait: &mut dyn FnMut(),
) -> Result<Option<Slot>, SlotError> {
    let dir = run_dir(project)?;
    let mut waiting = false;
    loop {
        if should_stop() {
            return Ok(None);
        }
        let live = live_records(&dir)?;
        let same = live
            .iter()
            .filter(|record| {
                record.harness == profile.harness.as_str() && record.model == profile.model
            })
            .count();
        if limit.is_none_or(|limit| same < limit) {
            let path = dir.join(format!("{id}.slot"));
            write_record(&path, id, profile)?;
            let others = live
                .iter()
                .filter(|record| record.id != id)
                .map(|record| record.id.clone())
                .collect();
            return Ok(Some(Slot { path, others }));
        }
        if !waiting {
            waiting = true;
            on_wait();
        }
        thread::sleep(POLL);
    }
}

/// Whether the run directory can be used, before a task folder is made: a symlinked one is a
/// configuration problem, not something to discover after a task exists.
pub fn check(project: &Path) -> Result<(), SlotError> {
    run_dir(project).map(|_| ())
}

/// The run directory, created private and never through a link: neither it nor the
/// `.dog-walker` above it may be a link, and both are 0700 like the task folder.
fn run_dir(project: &Path) -> Result<PathBuf, SlotError> {
    let root = project.join(".dog-walker");
    let dir = project.join(RUN_DIR);
    let fail = |path: &Path, source| SlotError::Dir {
        path: path.to_owned(),
        source,
    };
    if fs::symlink_metadata(&root)
        .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_dir())
    {
        return Err(SlotError::UnsafeDir { path: root });
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|source| fail(&dir, source))?;
    let metadata = fs::symlink_metadata(&dir).map_err(|source| fail(&dir, source))?;
    // A link here would put the records, and the unlinking, outside the project.
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(SlotError::UnsafeDir { path: dir });
    }
    Ok(dir)
}

/// A record on disk, read back.
#[derive(Debug)]
struct Record {
    id: String,
    harness: String,
    model: String,
}

/// What a file in the run directory turned out to be.
enum Found {
    Live(Record),
    /// A complete record whose runner is gone: it may be removed.
    Stale,
    /// Not a record (a partial write, a planted file): it is skipped, never removed.
    Other,
}

/// The records whose runner is still alive; a stale one is removed on the way.
fn live_records(dir: &Path) -> Result<Vec<Record>, SlotError> {
    let entries = fs::read_dir(dir).map_err(|source| SlotError::Dir {
        path: dir.to_owned(),
        source,
    })?;
    let mut records = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().ends_with(".slot") {
            continue;
        }
        match read_record(dir, &name) {
            Ok(Found::Live(record)) => records.push(record),
            Ok(Found::Stale) => {
                let _ = fs::remove_file(dir.join(&name));
            }
            // A record that cannot be read is not a reason to fail a run; it is not a slot.
            Ok(Found::Other) | Err(_) => {}
        }
    }
    Ok(records)
}

/// Reads one record without following a link and without blocking on a FIFO, and tells a live
/// record from a stale one and from something that is not a record at all.
fn read_record(dir: &Path, name: &OsString) -> Result<Found, io::Error> {
    let path = dir.join(name);
    let before = fs::symlink_metadata(&path)?;
    if !before.file_type().is_file() {
        return Ok(Found::Other);
    }
    let fd = rustix::fs::open(
        &path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    let file = File::from(fd);
    let opened = file.metadata()?;
    if !opened.is_file() || opened.dev() != before.dev() || opened.ino() != before.ino() {
        return Ok(Found::Other);
    }
    let mut text = String::new();
    file.take(MAX_RECORD + 1).read_to_string(&mut text)?;
    if text.len() as u64 > MAX_RECORD {
        return Ok(Found::Other);
    }
    let mut id = None;
    let mut pid = None;
    let mut start = None;
    let mut harness = None;
    let mut model = None;
    for line in text.lines() {
        let Some((field, value)) = line.split_once('=') else {
            return Ok(Found::Other);
        };
        match field {
            "id" => id = Some(value.to_owned()),
            "pid" => pid = value.parse::<u32>().ok(),
            "start" => start = value.parse::<u64>().ok(),
            "harness" => harness = Some(value.to_owned()),
            "model" => model = Some(value.to_owned()),
            _ => {}
        }
    }
    let (Some(id), Some(pid), Some(start), Some(harness), Some(model)) =
        (id, pid, start, harness, model)
    else {
        return Ok(Found::Other);
    };
    if runner_started(pid) != Some(start) {
        return Ok(Found::Stale);
    }
    Ok(Found::Live(Record { id, harness, model }))
}

/// The start time of `pid` in clock ticks since boot, which tells a live runner from a reused
/// pid. `None` when the process is gone.
fn runner_started(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The second field is the name in parentheses and may hold spaces, so the fields after
    // the last `)` start at the state; the start time is field 22.
    let rest = stat.rsplit_once(')')?.1;
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// Writes a record that is not there yet, through a temporary file, so a reader never sees a
/// partial one and a runner never overwrites another's slot.
fn write_record(path: &Path, id: &str, profile: &Profile) -> Result<(), SlotError> {
    let temp = path.with_extension("slot.tmp");
    let fail = |path: &Path, source| SlotError::Record {
        path: path.to_owned(),
        source,
    };
    let fd = rustix::fs::open(
        &temp,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )
    .map_err(|source| fail(&temp, io::Error::from(source)))?;
    let mut file = File::from(fd);
    let started = runner_started(process::id()).unwrap_or_default();
    let text = format!(
        "id={id}\npid={}\nstart={started}\nharness={}\nmodel={}\n",
        process::id(),
        profile.harness.as_str(),
        profile.model
    );
    let written = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })();
    drop(file);
    if let Err(source) = written {
        let _ = fs::remove_file(&temp);
        return Err(fail(&temp, source));
    }
    fs::rename(&temp, path).map_err(|source| {
        let _ = fs::remove_file(&temp);
        fail(path, source)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::{Harness, ProfileName};

    fn profile(name: &str, harness: Harness, model: &str) -> Profile {
        Profile {
            name: ProfileName::parse(name).unwrap(),
            harness,
            model: model.to_owned(),
            effort: None,
            timeout: None,
            command: Some(vec!["/bin/echo".to_owned()]),
            dsh_profile: None,
            main: false,
            network: true,
            git_writable: false,
            read_only_dirs: Vec::new(),
            writable_dirs: Vec::new(),
            env: Vec::new(),
            secrets: Vec::new(),
        }
    }

    fn project(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("dog-walker-slots-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::canonicalize(root).unwrap()
    }

    #[test]
    fn a_slot_is_a_record_that_goes_away() {
        let project = project("slot");
        let profile = profile("alpha", Harness::Pi, "local/qwen");
        let slot = acquire(&project, &profile, None, "0001", &|| false, &mut || {})
            .unwrap()
            .unwrap();
        let record = project.join(RUN_DIR).join("0001.slot");
        assert!(record.exists());
        assert_eq!(fs::read_to_string(&record).unwrap().lines().count(), 5);
        assert_eq!(slot.overlaps().len(), 0);
        drop(slot);
        assert!(!record.exists());
        fs::remove_dir_all(&project).unwrap();
    }

    #[test]
    fn a_second_run_sees_the_first() {
        let project = project("second");
        let profile = profile("alpha", Harness::Pi, "local/qwen");
        let first = acquire(&project, &profile, None, "0001", &|| false, &mut || {})
            .unwrap()
            .unwrap();
        let second = acquire(&project, &profile, None, "0002", &|| false, &mut || {})
            .unwrap()
            .unwrap();
        assert_eq!(second.overlaps(), ["0001"]);
        assert_eq!(first.overlaps().len(), 0);
        fs::remove_dir_all(&project).unwrap();
    }

    #[test]
    fn a_stale_record_is_not_a_slot() {
        let project = project("stale");
        let dir = project.join(RUN_DIR);
        fs::create_dir_all(&dir).unwrap();
        // A complete record whose runner cannot be running: pid 1, with the wrong start.
        let stale = dir.join("9999.slot");
        let mut record = fs::File::create(&stale).unwrap();
        record
            .write_all(b"id=9999\npid=1\nstart=0\nharness=pi\nmodel=local/qwen\n")
            .unwrap();
        drop(record);
        let profile = profile("alpha", Harness::Pi, "local/qwen");
        let slot = acquire(&project, &profile, Some(1), "0001", &|| false, &mut || {})
            .unwrap()
            .unwrap();
        assert_eq!(slot.overlaps().len(), 0);
        assert!(!stale.exists());
        fs::remove_dir_all(&project).unwrap();
    }

    #[test]
    fn a_waiting_run_stops_when_it_is_asked_to() {
        let project = project("stop");
        let profile = profile("alpha", Harness::Pi, "local/qwen");
        let first = acquire(&project, &profile, Some(1), "0001", &|| false, &mut || {})
            .unwrap()
            .unwrap();
        let stopped = acquire(&project, &profile, Some(1), "0002", &|| true, &mut || {}).unwrap();
        assert!(stopped.is_none());
        drop(first);
        fs::remove_dir_all(&project).unwrap();
    }

    #[test]
    fn a_symlinked_run_directory_is_refused() {
        let project = project("link");
        let outside = project.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(project.join(".dog-walker")).unwrap();
        std::os::unix::fs::symlink(&outside, project.join(RUN_DIR)).unwrap();
        let profile = profile("alpha", Harness::Pi, "local/qwen");
        assert!(matches!(
            acquire(&project, &profile, None, "0001", &|| false, &mut || {}),
            Err(SlotError::UnsafeDir { .. })
        ));
        fs::remove_dir_all(&project).unwrap();
    }
}
