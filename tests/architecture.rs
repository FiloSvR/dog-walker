//! Architecture invariants that no lint sees. They read the source files.

use std::fs;
use std::io;
use std::path::Path;

// Only the `output` module writes (docs/architecture.md): nothing else in `src/` uses a
// write macro or `write_all`. `main.rs` only hands the real stdout and stderr to `cli::run`.
#[test]
fn only_the_output_module_writes() -> io::Result<()> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let patterns = [
        "print!(",
        "println!(",
        "eprint!(",
        "eprintln!(",
        "write!(",
        "writeln!(",
        ".write_all(",
        ".write(",
    ];
    let mut checked = 0;
    for entry in fs::read_dir(&src)? {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "rs") || path.ends_with("output.rs") {
            continue;
        }
        checked += 1;
        let text = fs::read_to_string(&path)?;
        for pattern in patterns {
            assert!(
                !text.contains(pattern),
                "{} writes output with `{pattern}`",
                path.display()
            );
        }
    }
    assert!(
        checked >= 4,
        "expected to check the other source files, checked {checked}"
    );
    Ok(())
}
