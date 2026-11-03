//! A profile's env file: the `NAME=value` lines `init` writes (D21).
//!
//! dog-walker reads the file as data and never runs it. A line is `NAME=value`, with an optional
//! `export `, where the value is bare or single-quoted (a quote inside is written `'\''`).
//! Comments, blank lines and anything else are ignored.

/// The variables the file sets, in order.
pub fn parse(text: &str) -> Vec<(String, String)> {
    text.lines().filter_map(line).collect()
}

fn line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
    let (name, value) = line.split_once('=')?;
    if name.is_empty()
        || name.starts_with(|c: char| c.is_ascii_digit())
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    Some((name.to_owned(), unquote(value)?))
}

/// A bare word, or a run of single-quoted parts and `\'` as the shell writes them.
fn unquote(value: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => loop {
                match chars.next()? {
                    '\'' => break,
                    c => out.push(c),
                }
            },
            '\\' => out.push(chars.next()?),
            c if c.is_whitespace()
                || matches!(c, '"' | '$' | '`' | ';' | '&' | '|' | '<' | '>') =>
            {
                return None;
            }
            c => out.push(c),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_reads_bare_and_quoted_values_and_skips_the_rest() {
        let text = "# a comment\n\nA=1\nexport B='two words'\nC='it'\\''s'\n# D=secret\nnot a line\n1X=3\nE=$(touch x)\nF='open\n";
        assert_eq!(
            parse(text),
            vec![
                ("A".to_owned(), "1".to_owned()),
                ("B".to_owned(), "two words".to_owned()),
                ("C".to_owned(), "it's".to_owned()),
            ]
        );
    }

    #[test]
    fn what_init_writes_reads_back_the_same() {
        let value = "/home/u/it's here/my dir";
        let quoted = crate::setup::shell_quote(value);
        assert_eq!(
            parse(&format!("P={quoted}\n")),
            vec![("P".to_owned(), value.to_owned())]
        );
    }
}
