//! Architecture invariants that no lint sees. They read the source files, so they are a
//! tripwire for the common forms of a write, not a proof: a write hidden in a macro is not seen.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Spellings of a write that only the `output` module may use.
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
const STREAMS: [&str; 4] = ["stdout()", "stderr()", "io::stdout", "io::stderr"];

/// Whether `text` uses `pattern`. A pattern that starts with a letter must not continue a
/// longer name, so `use_stderr()` is not `stderr()`.
fn mentions(text: &str, pattern: &str) -> bool {
    let starts_with_letter = pattern.starts_with(|c: char| c.is_alphabetic());
    text.match_indices(pattern).any(|(at, _)| {
        let before = text.get(..at).and_then(|head| head.chars().next_back());
        !(starts_with_letter && before.is_some_and(|c| c.is_alphanumeric() || c == '_'))
    })
}

/// The spellings in `text` that `file` may not use.
fn violations(file: &Path, text: &str) -> Vec<&'static str> {
    let may_name_streams = file.ends_with("main.rs");
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
        if path.ends_with("output.rs") {
            continue;
        }
        checked += 1;
        let found = violations(&path, &fs::read_to_string(&path)?);
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
    let other = Path::new("src/other.rs");
    for pattern in WRITES.iter().chain(STREAMS.iter()) {
        let sample = format!("fn f() {{ {pattern} }}");
        assert!(violations(other, &sample).contains(pattern), "{pattern}");
    }
    let main = Path::new("src/main.rs");
    assert_eq!(
        violations(main, "std::io::stdout().lock()"),
        Vec::<&str>::new()
    );
    assert!(violations(main, "w.write_all(b)").contains(&".write_all("));
    // Names that merely contain a stream's name are not the stream.
    assert_eq!(
        violations(other, "if error.use_stderr() { quiet() }"),
        Vec::<&str>::new()
    );
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
