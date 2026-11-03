//! What a task cost its harness, read from the harness's own log (D18).
//!
//! The numbers are the harness's report, not a measurement dog-walker makes: a harness that
//! prints nothing readable has no usage, and the handover then says nothing rather than zero.
//! The log is the harness's output and may hold anything, so a line that does not parse is
//! skipped and a number that does not fit is dropped.

use serde_json::Value;

/// The usage of one task. `total` counts every token the harness reports for the run: input,
/// cached input and output, summed over all its turns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Usage {
    pub total: u64,
    /// How many model calls the run took, when the harness says.
    pub turns: Option<u64>,
    /// What the run cost in US dollars, when the harness says.
    pub cost_usd: Option<f64>,
}

impl Usage {
    /// The front matter lines of this usage, each ending in a newline.
    pub fn front_matter(&self) -> String {
        let turns = self.turns.map(|turns| format!("turns = {turns}\n"));
        let cost = self
            .cost_usd
            .filter(|cost| cost.is_finite() && *cost >= 0.0)
            .map(|cost| format!("cost_usd = {cost:.6}\n"));
        [Some(format!("tokens = {}\n", self.total)), turns, cost]
            .into_iter()
            .flatten()
            .collect()
    }

    /// A short form for a log line: `40317 tokens, 3 turns`.
    pub fn summary(&self) -> String {
        let turns = self.turns.map(|turns| format!("{turns} turns"));
        [Some(format!("{} tokens", self.total)), turns]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Claude Code with `--output-format json`: one object with `"type":"result"`, whose `usage`
/// holds the input (fresh, cache written, cache read) and output tokens of the whole run.
pub fn claude(log: &str) -> Option<Usage> {
    let result = json_lines(log).find(|line| text(line, &["type"]) == Some("result"))?;
    let total = [
        "input_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
        "output_tokens",
    ]
    .iter()
    .try_fold(0u64, |sum, key| {
        sum.checked_add(whole(&result, &["usage", key]).unwrap_or(0))
    })?;
    Some(Usage {
        total,
        turns: whole(&result, &["num_turns"]),
        cost_usd: real(&result, &["total_cost_usd"]),
    })
}

/// Pi with `--mode json`: one `turn_end` event per model call, each with the call's usage.
pub fn pi(log: &str) -> Option<Usage> {
    let mut total = 0u64;
    let mut turns = 0u64;
    let mut cost = 0.0f64;
    let mut priced = false;
    for event in json_lines(log).filter(|line| text(line, &["type"]) == Some("turn_end")) {
        let Some(tokens) = whole(&event, &["message", "usage", "totalTokens"]) else {
            continue;
        };
        total = total.checked_add(tokens)?;
        turns += 1;
        if let Some(call) = real(&event, &["message", "usage", "cost", "total"]) {
            cost += call;
            priced = true;
        }
    }
    (turns > 0).then_some(Usage {
        total,
        turns: Some(turns),
        cost_usd: priced.then_some(cost),
    })
}

/// Codex ends its output with `tokens used`, a line, and the total with thousands separators.
pub fn codex(log: &str) -> Option<Usage> {
    let mut lines = log.lines().map(str::trim);
    let mut found = None;
    while let Some(line) = lines.next() {
        if line == "tokens used" {
            let number = lines.next().unwrap_or("").replace(',', "");
            if let Ok(total) = number.parse::<u64>() {
                found = Some(total);
            }
        }
    }
    found.map(|total| Usage {
        total,
        turns: None,
        cost_usd: None,
    })
}

/// The value at `path` in `value`, without ever panicking on a shape that is not expected.
fn at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(value, |value, key| value.get(*key))
}

fn text<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    at(value, path)?.as_str()
}

fn whole(value: &Value, path: &[&str]) -> Option<u64> {
    at(value, path)?.as_u64()
}

fn real(value: &Value, path: &[&str]) -> Option<f64> {
    at(value, path)?.as_f64()
}

/// The lines of `log` that are a JSON object, in order.
fn json_lines(log: &str) -> impl Iterator<Item = Value> + '_ {
    log.lines()
        .map(str::trim)
        .filter(|line| line.starts_with('{'))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_sums_every_kind_of_token_and_reads_turns_and_cost() {
        let log = "{\"type\":\"result\",\"num_turns\":3,\"total_cost_usd\":0.5,\"usage\":{\"input_tokens\":2,\"cache_creation_input_tokens\":10,\"cache_read_input_tokens\":100,\"output_tokens\":4}}\n";
        let usage = claude(log).unwrap();
        assert_eq!(usage.total, 116);
        assert_eq!(usage.turns, Some(3));
        assert_eq!(usage.cost_usd, Some(0.5));
    }

    #[test]
    fn claude_without_a_result_object_has_no_usage() {
        assert_eq!(claude("just text\n{\"type\":\"system\"}\n"), None);
        assert_eq!(claude(""), None);
    }

    #[test]
    fn pi_sums_the_turns() {
        let log = "{\"type\":\"session\"}\n\
            {\"type\":\"turn_end\",\"message\":{\"usage\":{\"totalTokens\":1000,\"cost\":{\"total\":0.25}}}}\n\
            not json\n\
            {\"type\":\"turn_end\",\"message\":{\"usage\":{\"totalTokens\":500,\"cost\":{\"total\":0.5}}}}\n";
        let usage = pi(log).unwrap();
        assert_eq!(usage.total, 1500);
        assert_eq!(usage.turns, Some(2));
        assert_eq!(usage.cost_usd, Some(0.75));
    }

    #[test]
    fn pi_with_no_turn_has_no_usage() {
        assert_eq!(pi("{\"type\":\"agent_start\"}\n"), None);
    }

    #[test]
    fn codex_reads_the_last_total_without_its_separators() {
        let log = "tokens used\n1,234\nmore\ntokens used\n40,317\nfinal answer\n";
        let usage = codex(log).unwrap();
        assert_eq!(usage.total, 40317);
        assert_eq!(usage.turns, None);
    }

    #[test]
    fn codex_ignores_a_total_that_is_not_a_number() {
        assert_eq!(codex("tokens used\nlots\n"), None);
        assert_eq!(codex("nothing here\n"), None);
    }

    #[test]
    fn the_front_matter_is_toml_a_reader_can_parse() {
        let usage = Usage {
            total: 7,
            turns: Some(2),
            cost_usd: Some(0.25),
        };
        let parsed: toml::Table = toml::from_str(&usage.front_matter()).unwrap();
        assert_eq!(parsed["tokens"].as_integer(), Some(7));
        assert_eq!(parsed["turns"].as_integer(), Some(2));
        assert!(parsed["cost_usd"].as_float().is_some());
        let bare = Usage {
            total: 7,
            turns: None,
            cost_usd: Some(f64::NAN),
        };
        assert_eq!(bare.front_matter(), "tokens = 7\n");
    }
}
