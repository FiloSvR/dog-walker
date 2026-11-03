//! Several tasks at once, with limits per harness and model, and a warning when two of them
//! write the same project (R9).
//!
//! Coordination is a directory of small records: `.dog-walker/run/<id>.slot`, one per running
//! task, with the pid of its runner. A record whose pid is gone is stale and is cleaned up; a
//! runner that would exceed the limit for its harness and model waits for one to disappear.
//! The count is advisory (two runners can see the same free slot), which is enough for a
//! warning-grade limit.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::thread;
use std::time::Duration;

use crate::config::Profile;
use crate::error::SlotError;

/// Where the running records live, inside the project.
const RUN_DIR: &str = ".dog-walker/run";

/// How long a waiting runner sleeps between looks at the records.
const POLL: Duration = Duration::from_millis(200);

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
/// `limit` is `None` when the config sets none: the record is still written, since it is what
/// lets another runner warn about an overlap.
pub fn acquire(
    project: &Path,
    profile: &Profile,
    limit: Option<usize>,
    id: &str,
) -> Result<Slot, SlotError> {
    let dir = project.join(RUN_DIR);
    fs::create_dir_all(&dir).map_err(|source| SlotError::Dir {
        path: dir.clone(),
        source,
    })?;
    loop {
        let live = live_records(&dir)?;
        let same: usize = live
            .iter()
            .filter(|record| {
                record.harness == profile.harness.as_str() && record.model == profile.model
            })
            .count();
        if limit.is_none_or(|limit| same < limit) {
            let path = dir.join(format!("{id}.slot"));
            let record = format!(
                "id={id}\npid={}\nharness={}\nmodel={}\n",
                process::id(),
                profile.harness.as_str(),
                profile.model
            );
            write_record(&path, &record)?;
            let others = live
                .iter()
                .filter(|record| record.id != id)
                .map(|record| record.id.clone())
                .collect();
            return Ok(Slot { path, others });
        }
        thread::sleep(POLL);
    }
}

/// A record on disk, read back.
#[derive(Debug)]
struct Record {
    id: String,
    harness: String,
    model: String,
}

/// The records whose runner is still alive; a stale one is removed on the way.
fn live_records(dir: &Path) -> Result<Vec<Record>, SlotError> {
    let entries = fs::read_dir(dir).map_err(|source| SlotError::Dir {
        path: dir.to_owned(),
        source,
    })?;
    let mut records = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "slot") {
            continue;
        }
        match read_record(&path) {
            Ok(Some(record)) => records.push(record),
            Ok(None) => {
                let _ = fs::remove_file(&path);
            }
            // A record that cannot be read is not a reason to fail a run; it is not a slot.
            Err(_) => {}
        }
    }
    Ok(records)
}

/// A record, or `None` when its runner is gone (or the file is not one).
fn read_record(path: &Path) -> Result<Option<Record>, io::Error> {
    let text = fs::read_to_string(path)?;
    let mut id = None;
    let mut pid = None;
    let mut harness = None;
    let mut model = None;
    for line in text.lines() {
        let Some((name, value)) = line.split_once('=') else {
            return Ok(None);
        };
        match name {
            "id" => id = Some(value.to_owned()),
            "pid" => pid = value.parse::<u32>().ok(),
            "harness" => harness = Some(value.to_owned()),
            "model" => model = Some(value.to_owned()),
            _ => {}
        }
    }
    let (Some(id), Some(pid), Some(harness), Some(model)) = (id, pid, harness, model) else {
        return Ok(None);
    };
    if !Path::new(&format!("/proc/{pid}")).exists() {
        return Ok(None);
    }
    Ok(Some(Record { id, harness, model }))
}

/// Writes a record that is not there yet: a runner never overwrites another's slot.
fn write_record(path: &Path, text: &str) -> Result<(), SlotError> {
    let fail = |source| SlotError::Record {
        path: path.to_owned(),
        source,
    };
    let mut file = fs::File::create_new(path).map_err(fail)?;
    file.write_all(text.as_bytes()).map_err(fail)?;
    Ok(())
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
            main: false,
            network: true,
            git_writable: false,
            read_only_dirs: Vec::new(),
            writable_dirs: Vec::new(),
            env: Vec::new(),
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
        let slot = acquire(&project, &profile, None, "0001").unwrap();
        let record = project.join(RUN_DIR).join("0001.slot");
        assert!(record.exists());
        assert_eq!(fs::read_to_string(&record).unwrap().lines().count(), 4);
        assert_eq!(slot.overlaps().len(), 0);
        drop(slot);
        assert!(!record.exists());
        fs::remove_dir_all(&project).unwrap();
    }

    #[test]
    fn a_second_run_sees_the_first() {
        let project = project("second");
        let profile = profile("alpha", Harness::Pi, "local/qwen");
        let first = acquire(&project, &profile, None, "0001").unwrap();
        let second = acquire(&project, &profile, None, "0002").unwrap();
        assert_eq!(second.overlaps(), ["0001"]);
        assert_eq!(first.overlaps().len(), 0);
        fs::remove_dir_all(&project).unwrap();
    }

    #[test]
    fn a_stale_record_is_not_a_slot() {
        let project = project("stale");
        let dir = project.join(RUN_DIR);
        fs::create_dir_all(&dir).unwrap();
        // A pid that cannot be running: pid 0 is the kernel's.
        write_record(
            &dir.join("9999.slot"),
            "id=9999\npid=0\nharness=pi\nmodel=local/qwen\n",
        )
        .unwrap();
        let profile = profile("alpha", Harness::Pi, "local/qwen");
        let slot = acquire(&project, &profile, Some(1), "0001").unwrap();
        assert_eq!(slot.overlaps().len(), 0);
        assert!(!dir.join("9999.slot").exists());
        fs::remove_dir_all(&project).unwrap();
    }
}
