//! The CloudWatch Logs input language: a Logs Insights query over a time
//! window, or one typed command per line.
//!
//! A line is either a **command** (`groups`, `streams`, `tail`, `use`, `help`:
//! the verb is the first word) or a **Logs Insights query**
//! (`fields @timestamp, @message | filter @message like /error/ | sort @timestamp desc | limit 50`)
//! with an optional prefix that sets the window and the log groups of that one
//! query:
//!
//! ```text
//! groups [PREFIX] [-n|--limit N]
//! streams [GROUP] [-n|--limit N]
//! tail [GROUP] [--since DURATION] [--filter PATTERN] [-n|--limit N]
//! use [GROUP[,GROUP…]]            (a trailing * makes a name a prefix)
//! help
//! [since DURATION | between TIME and TIME] [--group GROUP[,GROUP…]] fields … | filter … | stats …
//! ```
//!
//! `DURATION` is `90s`, `30m`, `2h`, `1d`, `1w` or a mix (`1h30m`); `TIME` is
//! `2026-10-16T08:00:00Z` (RFC 3339), `2026-10-16 08:00[:00]` or `2026-10-16`
//! (UTC), or `now`. Every query has a window: there is no way to ask for one
//! without (the driver applies its default, the last hour).
//!
//! **Read-only by construction**: no command and no query word writes. The
//! only server-side effect of a query is the (billable) work of a
//! `StartQuery`, which the driver stops on Ctrl-C and on timeout.
//! [`parse`] is the single judge of what is accepted: the driver runs a parsed
//! [`CloudWatchInput`], the `--read-only` guard accepts exactly what parses,
//! and completion lists [`Verb`]s.

use std::fmt;
use strum::{EnumIter, EnumString, IntoEnumIterator, IntoStaticStr};

/// The CloudWatch Logs command words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, EnumString, IntoStaticStr)]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum Verb {
    Groups,
    Streams,
    Tail,
    Use,
    Help,
}

impl Verb {
    pub fn name(self) -> &'static str {
        self.into()
    }

    pub fn usage(self) -> &'static str {
        match self {
            Verb::Groups => "groups [PREFIX] [-n|--limit N]",
            Verb::Streams => "streams [GROUP] [-n|--limit N]",
            Verb::Tail => "tail [GROUP] [--since DURATION] [--filter PATTERN] [-n|--limit N]",
            Verb::Use => "use [GROUP[,GROUP…]]",
            Verb::Help => "help",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Verb::Groups => {
                "list the log groups, optionally under a name prefix (DescribeLogGroups)"
            }
            Verb::Streams => "list a group's most recently active streams (DescribeLogStreams)",
            Verb::Tail => {
                "show recent events of a group, oldest first, bounded (FilterLogEvents; no follow mode)"
            }
            Verb::Use => {
                "choose the log groups Insights queries run on (a name ending in * is a prefix)"
            }
            Verb::Help => "list the commands and the query form",
        }
    }
}

/// Every verb, in help order (derived from the enum, never hand-listed).
pub fn verbs() -> impl Iterator<Item = Verb> {
    Verb::iter()
}

/// Every command word, in help order.
pub fn command_names() -> Vec<&'static str> {
    verbs().map(Verb::name).collect()
}

/// The words a Logs Insights query may start with (the first command of the
/// pipeline). Anything else is not a query.
pub const INSIGHTS_COMMANDS: [&str; 13] = [
    "fields",
    "filter",
    "filterIndex",
    "stats",
    "parse",
    "sort",
    "limit",
    "display",
    "dedup",
    "pattern",
    "diff",
    "unnest",
    "anomaly",
];

/// An edge of a time window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeEdge {
    /// Seconds since the Unix epoch.
    Epoch(i64),
    Now,
}

/// The time window of one query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeWindow {
    /// The last N seconds.
    Since(u64),
    Between(TimeEdge, TimeEdge),
}

/// A Logs Insights query with what a prefix may set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsightsQuery {
    /// `None`: the connection's default window.
    pub window: Option<TimeWindow>,
    /// Names or `prefix*` patterns given with `--group`; empty: the selected groups.
    pub groups: Vec<String>,
    pub query: String,
}

/// A parsed typed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudWatchCommand {
    Help,
    Groups {
        prefix: Option<String>,
        limit: Option<usize>,
    },
    Streams {
        group: Option<String>,
        limit: Option<usize>,
    },
    Tail {
        group: Option<String>,
        since: Option<u64>,
        filter: Option<String>,
        limit: Option<usize>,
    },
    /// No names: show the current selection.
    Use {
        groups: Vec<String>,
    },
}

/// What a line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudWatchInput {
    Command(CloudWatchCommand),
    Query(InsightsQuery),
}

/// Why a line is not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudWatchCommandError(pub String);

impl fmt::Display for CloudWatchCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CloudWatchCommandError {}

fn error<T>(message: impl Into<String>) -> Result<T, CloudWatchCommandError> {
    Err(CloudWatchCommandError(message.into()))
}

/// `90s`, `30m`, `2h`, `1d`, `1w`, or a mix such as `1h30m`, in seconds.
pub fn parse_duration_secs(text: &str) -> Result<u64, String> {
    let text = text.trim().to_ascii_lowercase();
    let invalid =
        || format!("'{text}' is not a duration (use 90s, 30m, 2h, 1d, 1w or a mix such as 1h30m)");
    if text.is_empty() {
        return Err(invalid());
    }
    let mut total: u64 = 0;
    let mut digits = String::new();
    let mut seen_unit = false;
    for c in text.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        let unit: u64 = match c {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86_400,
            'w' => 604_800,
            _ => return Err(invalid()),
        };
        let n: u64 = digits.parse().map_err(|_| invalid())?;
        digits.clear();
        total = n
            .checked_mul(unit)
            .and_then(|part| total.checked_add(part))
            .ok_or_else(invalid)?;
        seen_unit = true;
    }
    if !digits.is_empty() || !seen_unit || total == 0 {
        return Err(invalid());
    }
    Ok(total)
}

/// RFC 3339, `YYYY-MM-DD HH:MM[:SS]`, `YYYY-MM-DDTHH:MM[:SS]` or `YYYY-MM-DD`
/// (UTC), as seconds since the epoch.
pub fn parse_timestamp(text: &str) -> Result<i64, String> {
    use chrono::{NaiveDate, NaiveDateTime};
    let text = text.trim();
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(text) {
        return Ok(parsed.timestamp());
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(text, format) {
            return Ok(parsed.and_utc().timestamp());
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        if let Some(midnight) = date.and_hms_opt(0, 0, 0) {
            return Ok(midnight.and_utc().timestamp());
        }
    }
    Err(format!(
        "'{text}' is not a time (use 2026-10-16T08:00:00Z, 2026-10-16 08:00 or 2026-10-16, UTC; or now)"
    ))
}

/// The first whitespace-delimited word of `text` and what follows it.
fn next_word(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    if text.is_empty() {
        return None;
    }
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    Some((&text[..end], &text[end..]))
}

fn looks_like_clock(word: &str) -> bool {
    let bytes = word.as_bytes();
    bytes.len() >= 5
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2] == b':'
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
}

fn looks_like_date(word: &str) -> bool {
    word.len() == 10
        && word.as_bytes().get(4) == Some(&b'-')
        && word.as_bytes().get(7) == Some(&b'-')
}

/// One time edge from the front of `text`: `now`, or a timestamp (a date may be
/// followed by its clock time as a second word).
fn take_edge(text: &str) -> Result<(TimeEdge, &str), CloudWatchCommandError> {
    let Some((first, rest)) = next_word(text) else {
        return error("give a time after `between` (2026-10-16T08:00:00Z, 2026-10-16 08:00, now)");
    };
    if first.eq_ignore_ascii_case("now") {
        return Ok((TimeEdge::Now, rest));
    }
    if looks_like_date(first) {
        if let Some((second, after)) = next_word(rest) {
            if looks_like_clock(second) {
                let combined = format!("{first} {second}");
                return parse_timestamp(&combined)
                    .map(|epoch| (TimeEdge::Epoch(epoch), after))
                    .map_err(CloudWatchCommandError);
            }
        }
    }
    parse_timestamp(first)
        .map(|epoch| (TimeEdge::Epoch(epoch), rest))
        .map_err(CloudWatchCommandError)
}

fn split_groups(list: &str) -> Vec<String> {
    list.split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

/// A script: one command per line; a query ends at its `;`, at a blank line or
/// at the end of the script, so it may span lines. `#` and `--` comment lines
/// between statements are skipped.
pub fn split_script(script: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut pending = String::new();
    let flush = |pending: &mut String, statements: &mut Vec<String>| {
        let statement = pending.trim().trim_end_matches(';').trim_end().to_string();
        pending.clear();
        if !statement.is_empty() {
            statements.push(statement);
        }
    };
    for line in script.lines() {
        let line = line.trim();
        if line.is_empty() {
            flush(&mut pending, &mut statements);
            continue;
        }
        if pending.is_empty() {
            if line.starts_with('#') || line.starts_with("--") {
                continue;
            }
            let first = line
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_end_matches(';');
            if first.parse::<Verb>().is_ok() {
                // a command is one line
                pending.push_str(line);
                flush(&mut pending, &mut statements);
                continue;
            }
        }
        if !pending.is_empty() {
            pending.push('\n');
        }
        pending.push_str(line);
        if line.ends_with(';') {
            flush(&mut pending, &mut statements);
        }
    }
    flush(&mut pending, &mut statements);
    statements
}

fn parse_limit(text: &str) -> Result<usize, CloudWatchCommandError> {
    match text.parse::<usize>() {
        Ok(0) => error("the limit must be at least 1"),
        Ok(n) => Ok(n),
        Err(_) => error(format!("the limit must be a whole number, got '{text}'")),
    }
}

fn unknown_option<T>(verb: Verb, flag: &str) -> Result<T, CloudWatchCommandError> {
    error(format!(
        "unknown option '{flag}' for {}; usage: {}",
        verb.name(),
        verb.usage()
    ))
}

fn parse_command(verb: Verb, line: &str) -> Result<CloudWatchCommand, CloudWatchCommandError> {
    let words = crate::s3_command::split_args(line.trim_end_matches(';'))
        .map_err(|e| CloudWatchCommandError(e.to_string()))?;
    let args = &words[1..];
    match verb {
        Verb::Help => match args.first() {
            Some(extra) => error(format!("help takes no arguments, got '{extra}'")),
            None => Ok(CloudWatchCommand::Help),
        },
        Verb::Use => match args {
            [] => Ok(CloudWatchCommand::Use { groups: Vec::new() }),
            [list] => {
                let groups = split_groups(list);
                if groups.is_empty() {
                    return error(format!("usage: {}", verb.usage()));
                }
                Ok(CloudWatchCommand::Use { groups })
            }
            _ => error(format!(
                "give the groups as one comma-separated list; usage: {}",
                verb.usage()
            )),
        },
        Verb::Groups | Verb::Streams | Verb::Tail => {
            let mut positional: Option<String> = None;
            let mut limit = None;
            let mut since = None;
            let mut filter = None;
            let mut iter = args.iter();
            while let Some(arg) = iter.next() {
                match arg.to_ascii_lowercase().as_str() {
                    "-n" | "--limit" => {
                        let Some(value) = iter.next() else {
                            return error(format!("{arg} needs a number"));
                        };
                        limit = Some(parse_limit(value)?);
                    }
                    "--since" if verb == Verb::Tail => {
                        let Some(value) = iter.next() else {
                            return error("--since needs a duration (5m, 2h…)");
                        };
                        since = Some(parse_duration_secs(value).map_err(CloudWatchCommandError)?);
                    }
                    "--filter" if verb == Verb::Tail => {
                        let Some(value) = iter.next() else {
                            return error("--filter needs a CloudWatch filter pattern");
                        };
                        filter = Some(value.clone());
                    }
                    _ if arg.starts_with('-') && arg.len() > 1 => {
                        return unknown_option(verb, arg);
                    }
                    _ => {
                        if positional.replace(arg.clone()).is_some() {
                            return error(format!("usage: {}", verb.usage()));
                        }
                    }
                }
            }
            Ok(match verb {
                Verb::Groups => CloudWatchCommand::Groups {
                    prefix: positional,
                    limit,
                },
                Verb::Streams => CloudWatchCommand::Streams {
                    group: positional,
                    limit,
                },
                _ => CloudWatchCommand::Tail {
                    group: positional,
                    since,
                    filter,
                    limit,
                },
            })
        }
    }
}

fn parse_query(line: &str) -> Result<InsightsQuery, CloudWatchCommandError> {
    let mut rest = line;
    let mut window: Option<TimeWindow> = None;
    let mut groups: Vec<String> = Vec::new();
    while let Some((word, after)) = next_word(rest) {
        match word.to_ascii_lowercase().as_str() {
            "since" => {
                let Some((value, after)) = next_word(after) else {
                    return error("give a duration after `since` (30m, 2h, 1d…)");
                };
                if window.is_some() {
                    return error("give the time window once (since … or between … and …)");
                }
                window = Some(TimeWindow::Since(
                    parse_duration_secs(value).map_err(CloudWatchCommandError)?,
                ));
                rest = after;
            }
            "between" => {
                if window.is_some() {
                    return error("give the time window once (since … or between … and …)");
                }
                let (start, after) = take_edge(after)?;
                let Some((and, after)) = next_word(after) else {
                    return error(
                        "`between` needs `and`: between 2026-10-16 08:00 and 2026-10-16 09:00",
                    );
                };
                if !and.eq_ignore_ascii_case("and") {
                    return error(
                        "`between` needs `and`: between 2026-10-16 08:00 and 2026-10-16 09:00",
                    );
                }
                let (end, after) = take_edge(after)?;
                if let (TimeEdge::Epoch(start), TimeEdge::Epoch(end)) = (start, end) {
                    if end <= start {
                        return error("the window ends before it starts");
                    }
                }
                window = Some(TimeWindow::Between(start, end));
                rest = after;
            }
            "--group" | "--groups" | "-g" => {
                let Some((value, after)) = next_word(after) else {
                    return error("--group needs a log group name (or a comma-separated list)");
                };
                groups.extend(split_groups(value));
                rest = after;
            }
            _ => break,
        }
    }
    let query = rest.trim().trim_end_matches(';').trim_end().to_string();
    let Some(first) = query
        .split(|c: char| c.is_whitespace() || c == '|')
        .find(|word| !word.is_empty())
    else {
        return error(
            "empty query; write a Logs Insights query such as `fields @timestamp, @message | limit 20`, or `help`",
        );
    };
    if !INSIGHTS_COMMANDS
        .iter()
        .any(|command| command.eq_ignore_ascii_case(first))
    {
        return error(format!(
            "unknown CloudWatch Logs command or query '{first}': this client only reads \
             (commands: {}; or a Logs Insights query starting with one of: {})",
            command_names().join(", "),
            INSIGHTS_COMMANDS.join(", ")
        ));
    }
    Ok(InsightsQuery {
        window,
        groups,
        query,
    })
}

/// Parse one line: a typed command, or a Logs Insights query.
pub fn parse(line: &str) -> Result<CloudWatchInput, CloudWatchCommandError> {
    let trimmed = line.trim();
    let first = trimmed
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_end_matches(';');
    match first.parse::<Verb>() {
        Ok(verb) => parse_command(verb, trimmed).map(CloudWatchInput::Command),
        Err(_) => parse_query(trimmed).map(CloudWatchInput::Query),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn command(input: CloudWatchInput) -> CloudWatchCommand {
        match input {
            CloudWatchInput::Command(command) => command,
            other => panic!("expected a command, got {other:?}"),
        }
    }

    fn query(input: CloudWatchInput) -> InsightsQuery {
        match input {
            CloudWatchInput::Query(query) => query,
            other => panic!("expected a query, got {other:?}"),
        }
    }

    #[test]
    fn verbs_come_from_the_enum() {
        assert_eq!(
            command_names(),
            ["groups", "streams", "tail", "use", "help"]
        );
        for verb in Verb::iter() {
            assert!(!verb.usage().is_empty() && !verb.description().is_empty());
            assert!(verb.usage().starts_with(verb.name()));
            assert_eq!(verb.name().parse::<Verb>().unwrap(), verb);
        }
    }

    #[rstest]
    #[case("30m", Ok(1800))]
    #[case("90s", Ok(90))]
    #[case("2h", Ok(7200))]
    #[case("1d", Ok(86_400))]
    #[case("1w", Ok(604_800))]
    #[case("1h30m", Ok(5400))]
    #[case("1D12H", Ok(129_600))]
    #[case("", Err(()))]
    #[case("0m", Err(()))]
    #[case("30", Err(()))]
    #[case("m", Err(()))]
    #[case("1x", Err(()))]
    #[case("-5m", Err(()))]
    #[case("1.5h", Err(()))]
    #[case("99999999999999999999h", Err(()))]
    fn durations(#[case] text: &str, #[case] expected: Result<u64, ()>) {
        assert_eq!(
            parse_duration_secs(text).map_err(|_| ()),
            expected,
            "{text}"
        );
    }

    #[rstest]
    #[case("2026-10-16T08:00:00Z", Some(1_792_137_600))]
    #[case("2026-10-16T10:00:00+02:00", Some(1_792_137_600))]
    #[case("2026-10-16T08:00:00", Some(1_792_137_600))]
    #[case("2026-10-16T08:00", Some(1_792_137_600))]
    #[case("2026-10-16 08:00:00", Some(1_792_137_600))]
    #[case("2026-10-16 08:00", Some(1_792_137_600))]
    #[case("2026-10-16", Some(1_792_108_800))]
    #[case("yesterday", None)]
    #[case("2026-13-01", None)]
    #[case("08:00", None)]
    fn timestamps(#[case] text: &str, #[case] expected: Option<i64>) {
        assert_eq!(parse_timestamp(text).ok(), expected, "{text}");
    }

    #[rstest]
    #[case("groups", CloudWatchCommand::Groups { prefix: None, limit: None })]
    #[case("GROUPS /aws/lambda/ -n 5", CloudWatchCommand::Groups { prefix: Some("/aws/lambda/".into()), limit: Some(5) })]
    #[case("streams /aws/lambda/a", CloudWatchCommand::Streams { group: Some("/aws/lambda/a".into()), limit: None })]
    #[case("streams --limit 3", CloudWatchCommand::Streams { group: None, limit: Some(3) })]
    #[case("tail", CloudWatchCommand::Tail { group: None, since: None, filter: None, limit: None })]
    #[case("tail /app --since 5m --filter ERROR -n 20", CloudWatchCommand::Tail { group: Some("/app".into()), since: Some(300), filter: Some("ERROR".into()), limit: Some(20) })]
    #[case("tail --filter '\"timed out\"'", CloudWatchCommand::Tail { group: None, since: None, filter: Some("\"timed out\"".into()), limit: None })]
    #[case("use", CloudWatchCommand::Use { groups: vec![] })]
    #[case("use /aws/lambda/a,/aws/lambda/b", CloudWatchCommand::Use { groups: vec!["/aws/lambda/a".into(), "/aws/lambda/b".into()] })]
    #[case("use /aws/lambda/*;", CloudWatchCommand::Use { groups: vec!["/aws/lambda/*".into()] })]
    #[case("help", CloudWatchCommand::Help)]
    fn commands_parse(#[case] line: &str, #[case] expected: CloudWatchCommand) {
        assert_eq!(command(parse(line).unwrap()), expected, "{line}");
    }

    #[test]
    fn a_query_without_a_prefix_has_no_window() {
        let parsed = query(parse("fields @timestamp, @message | limit 20").unwrap());
        assert_eq!(parsed.window, None);
        assert!(parsed.groups.is_empty());
        assert_eq!(parsed.query, "fields @timestamp, @message | limit 20");
    }

    #[test]
    fn prefixes_set_the_window_and_the_groups() {
        let parsed = query(parse("since 30m fields @message | limit 5;").unwrap());
        assert_eq!(parsed.window, Some(TimeWindow::Since(1800)));
        assert_eq!(parsed.query, "fields @message | limit 5");

        let parsed = query(
            parse(
                "between 2026-10-16 08:00 and 2026-10-16T09:30:00Z --group /a,/b fields @message",
            )
            .unwrap(),
        );
        assert_eq!(
            parsed.window,
            Some(TimeWindow::Between(
                TimeEdge::Epoch(1_792_137_600),
                TimeEdge::Epoch(1_792_143_000)
            ))
        );
        assert_eq!(parsed.groups, ["/a", "/b"]);
        assert_eq!(parsed.query, "fields @message");
    }

    #[test]
    fn between_to_now_and_a_date_with_its_clock() {
        let parsed = query(parse("between 2026-10-16 and now fields @message").unwrap());
        assert_eq!(
            parsed.window,
            Some(TimeWindow::Between(
                TimeEdge::Epoch(1_792_108_800),
                TimeEdge::Now
            ))
        );
    }

    #[rstest]
    #[case("fields @message")]
    #[case("FIELDS @message")]
    #[case("filter @message like /ERROR/ | stats count(*) by bin(5m)")]
    #[case("stats count(*) by @logStream")]
    #[case("parse @message \"user=* \" as user | display user")]
    #[case("sort @timestamp desc | limit 10")]
    #[case("limit 5")]
    #[case("pattern @message")]
    #[case("filterIndex requestId = 'x' | fields @message")]
    #[case("| fields @message")]
    fn insights_queries_are_accepted(#[case] line: &str) {
        let parsed = parse(line);
        assert!(parsed.is_ok(), "{line}: {parsed:?}");
    }

    #[rstest]
    #[case::empty("")]
    #[case::semicolon(";")]
    #[case::prefix_only("since 30m")]
    #[case::unknown_word("put @message")]
    #[case::sql("SELECT * FROM logs")]
    #[case::delete("delete log group x")]
    #[case::bad_duration("since soon fields @message")]
    #[case::duration_missing("since")]
    #[case::bad_time("between yesterday and today fields @message")]
    #[case::missing_and("between 2026-10-16 2026-10-17 fields @message")]
    #[case::backwards("between 2026-10-17 and 2026-10-16 fields @message")]
    #[case::twice("since 1h since 2h fields @message")]
    #[case::both("since 1h between 2026-10-16 and now fields @message")]
    #[case::group_missing("since 5m --group")]
    fn malformed_lines_are_refused(#[case] line: &str) {
        let error = parse(line).unwrap_err();
        assert!(!error.to_string().is_empty(), "{line}");
    }

    #[rstest]
    #[case("groups a b")]
    #[case("groups -n 0")]
    #[case("groups -n x")]
    #[case("groups --since 5m")]
    #[case("streams --unknown")]
    #[case("tail --since")]
    #[case("tail --since soon")]
    #[case("tail --filter")]
    #[case("tail a b")]
    #[case("use a b")]
    #[case("use ,")]
    #[case("help me")]
    #[case("tail 'unterminated")]
    fn malformed_commands_are_refused(#[case] line: &str) {
        let error = parse(line).unwrap_err();
        assert!(!error.to_string().is_empty(), "{line}");
    }

    #[test]
    fn scripts_split_by_command_blank_line_and_semicolon() {
        let script = "# warm up\ngroups\nfields @message\n| filter @message like /x/\n| limit 5;\n\nuse /a\nfields @timestamp\n\nsince 5m fields @message";
        assert_eq!(
            split_script(script),
            [
                "groups",
                "fields @message\n| filter @message like /x/\n| limit 5",
                "use /a",
                "fields @timestamp",
                "since 5m fields @message"
            ]
        );
        assert!(split_script("\n  \n-- nothing\n").is_empty());
    }
}
