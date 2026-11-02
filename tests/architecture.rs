//! Architecture invariants that no lint sees. They read the source files, so they are a
//! tripwire for the common forms of a write, not a proof: a write hidden in a macro is not seen.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The file-writing spellings of `WRITES`, which `handover` may use for its own document
/// (`handover.rs`). Everything that reaches stdout or stderr stays in the `output` module.
const FILE_WRITES: [&str; 1] = [".write_all("];

/// Spellings of a write that only the `output` module may use, except for the file writes
/// `handover` is allowed.
const WRITES: [&str; 12] = [
    "print!(",
    "println!(",
    "eprint!(",
    "eprintln!(",
    "write!(",
    "writeln!(",
    ".write_all(",
    "::write_all(",
    ".write_fmt(",
    ".write(",
    "::write(",
    ".flush(",
];

/// Naming the process's own streams: only `main.rs` hands them to `cli::run`.
const STREAMS: [&str; 6] = [
    "stdout()",
    "stderr()",
    "io::stdout",
    "io::stderr",
    "stdout as ",
    "stderr as ",
];

/// Whether `text` uses `pattern`. A pattern that starts with a letter must not continue a
/// longer name, so `use_stderr()` is not `stderr()`.
fn mentions(text: &str, pattern: &str) -> bool {
    let starts_with_letter = pattern.starts_with(|c: char| c.is_alphabetic());
    text.match_indices(pattern).any(|(at, _)| {
        let before = text.get(..at).and_then(|head| head.chars().next_back());
        !(starts_with_letter && before.is_some_and(|c| c.is_alphanumeric() || c == '_'))
    })
}

/// `src/<name>` itself, not a file of that name in a subdirectory.
fn is_top_level(src: &Path, file: &Path, name: &str) -> bool {
    file == src.join(name)
}

/// The spellings in `text` that a file may not use; `may_name_streams` is true for `main.rs`.
fn violations(text: &str, may_name_streams: bool) -> Vec<&'static str> {
    WRITES
        .iter()
        .chain(STREAMS.iter().filter(|_| !may_name_streams))
        .copied()
        .filter(|pattern| mentions(text, pattern))
        .collect()
}

/// Every `.rs` file under `dir`, at any depth.
fn rust_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(rust_files(&path)?);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    Ok(files)
}

// Only the `output` module writes (docs/architecture.md). `main.rs` only hands the real
// stdout and stderr to `cli::run`.
#[test]
fn only_the_output_module_writes() -> io::Result<()> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    for path in rust_files(&src)? {
        if is_top_level(&src, &path, "output.rs") {
            continue;
        }
        checked += 1;
        let mut found = violations(
            &fs::read_to_string(&path)?,
            is_top_level(&src, &path, "main.rs"),
        );
        // The handover writes its own document: a file write, not the process's streams.
        if is_top_level(&src, &path, "handover.rs") {
            found.retain(|pattern| !FILE_WRITES.contains(pattern));
        }
        assert!(
            found.is_empty(),
            "{} writes output with {found:?}",
            path.display()
        );
    }
    assert!(
        checked >= 4,
        "expected to check the other source files, checked {checked}"
    );
    Ok(())
}

// The tripwire itself: each spelling is seen, and only `main.rs` may name the streams.
#[test]
fn the_tripwire_sees_each_spelling() {
    for pattern in WRITES.iter().chain(STREAMS.iter()) {
        let sample = format!("fn f() {{ {pattern} }}");
        assert!(violations(&sample, false).contains(pattern), "{pattern}");
    }
    assert_eq!(
        violations("std::io::stdout().lock()", true),
        Vec::<&str>::new()
    );
    assert!(violations("w.write_all(b)", true).contains(&".write_all("));
    // Names that merely contain a stream's name are not the stream.
    assert_eq!(
        violations("if error.use_stderr() { quiet() }", false),
        Vec::<&str>::new()
    );
}

// Only the top-level `main.rs`, `output.rs` and `handover.rs` are exempt, not files of the same
// name deeper in.
#[test]
fn the_exemptions_are_exact_paths() {
    let src = Path::new("/repo/src");
    assert!(is_top_level(src, Path::new("/repo/src/main.rs"), "main.rs"));
    assert!(is_top_level(
        src,
        Path::new("/repo/src/output.rs"),
        "output.rs"
    ));
    assert!(is_top_level(
        src,
        Path::new("/repo/src/handover.rs"),
        "handover.rs"
    ));
    assert!(!is_top_level(
        src,
        Path::new("/repo/src/sub/handover.rs"),
        "handover.rs"
    ));
    assert!(!is_top_level(
        src,
        Path::new("/repo/src/sub/main.rs"),
        "main.rs"
    ));
    assert!(!is_top_level(
        src,
        Path::new("/repo/src/sub/output.rs"),
        "output.rs"
    ));
}

// The walk reaches files in subdirectories.
#[test]
fn the_walk_reaches_subdirectories() -> io::Result<()> {
    let root = std::env::temp_dir().join(format!("dog-walker-walk-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("a/b"))?;
    fs::write(root.join("top.rs"), "")?;
    fs::write(root.join("a/b/deep.rs"), "")?;
    fs::write(root.join("a/notes.txt"), "")?;
    let mut names: Vec<String> = rust_files(&root)?
        .iter()
        .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
        .collect();
    names.sort();
    fs::remove_dir_all(&root)?;
    assert_eq!(names, ["deep.rs", "top.rs"]);
    Ok(())
}
