//! The S3 command language: one typed command per line.
//!
//! An object store has no query language, so a DBCrust "query" for an S3
//! connection is one of a handful of commands (`ls`, `head`, `get`, `buckets`,
//! `cd`, `pwd`, `help`). This module is the single place that knows them: the
//! S3 driver executes a parsed [`S3Command`], the `--read-only` guard accepts
//! exactly what parses, and completion lists [`Verb`]s.
//!
//! **Read-only by construction**: [`S3Command`] has no variant that writes. No
//! put/delete/copy/multipart/ACL/presign command exists, so there is nothing
//! for a guard to reject beyond "not a command"; the IAM policy remains the
//! server-side limit.
//!
//! Grammar (verbs are case-insensitive; arguments are shell-like words, with
//! `'single'` and `"double"` quotes and `\` escapes inside double quotes):
//!
//! ```text
//! ls [PREFIX] [-r|--recursive] [-n|--limit N]
//! head KEY
//! get KEY [bytes=A-B | bytes=A- | bytes=-N | -b|--bytes N] [--base64]
//! buckets
//! cd [PREFIX|..|/]
//! pwd
//! help
//! ```

use std::fmt;
use strum::{EnumIter, EnumString, IntoEnumIterator, IntoStaticStr};

/// The S3 command words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, EnumString, IntoStaticStr)]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum Verb {
    Ls,
    Head,
    Get,
    Buckets,
    Cd,
    Pwd,
    Help,
}

impl Verb {
    pub fn name(self) -> &'static str {
        self.into()
    }

    pub fn usage(self) -> &'static str {
        match self {
            Verb::Ls => "ls [PREFIX] [-r|--recursive] [-n|--limit N]",
            Verb::Head => "head KEY",
            Verb::Get => "get KEY [bytes=A-B | bytes=A- | bytes=-N | --bytes N] [--base64]",
            Verb::Buckets => "buckets",
            Verb::Cd => "cd [PREFIX|..|/]",
            Verb::Pwd => "pwd",
            Verb::Help => "help",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Verb::Ls => "list the objects and prefixes under PREFIX (default: the current prefix)",
            Verb::Head => "show an object's metadata (HeadObject)",
            Verb::Get => "show the first bytes of an object (ranged GetObject, capped)",
            Verb::Buckets => "list the account's buckets",
            Verb::Cd => "change the current prefix used by `ls` and `\\dt`",
            Verb::Pwd => "show the current bucket and prefix",
            Verb::Help => "list the commands",
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

/// Which bytes of an object `get` asks for. Every form is bounded before it is
/// sent: the driver never issues a GetObject without a `Range`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteRange {
    /// No range given: the first default-sized chunk.
    Default,
    /// `--bytes N`: the first N bytes.
    First(u64),
    /// `bytes=A-B` (inclusive) or `bytes=A-` (up to the cap).
    From { start: u64, end: Option<u64> },
    /// `bytes=-N`: the last N bytes.
    Suffix(u64),
}

/// A parsed S3 command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum S3Command {
    Help,
    Pwd,
    /// `cd` with no argument goes to the bucket root.
    Cd {
        prefix: Option<String>,
    },
    Buckets,
    Ls {
        prefix: Option<String>,
        recursive: bool,
        limit: Option<usize>,
    },
    Head {
        key: String,
    },
    Get {
        key: String,
        range: ByteRange,
        base64: bool,
    },
}

/// Why a line is not an S3 command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3CommandError(pub String);

impl fmt::Display for S3CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for S3CommandError {}

fn error<T>(message: impl Into<String>) -> Result<T, S3CommandError> {
    Err(S3CommandError(message.into()))
}

/// Split a line into words. Quotes group (`get "my file.txt"`); inside double
/// quotes `\"` and `\\` escape; outside quotes a backslash is literal (keys
/// often contain them on Windows-built buckets).
pub fn split_args(line: &str) -> Result<Vec<String>, S3CommandError> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => current.push(c),
                        None => return error("unterminated single quote"),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') if matches!(chars.peek(), Some('"' | '\\')) => {
                            current.push(chars.next().unwrap_or('\\'));
                        }
                        Some(c) => current.push(c),
                        None => return error("unterminated double quote"),
                    }
                }
            }
            c => {
                in_word = true;
                current.push(c);
            }
        }
    }
    if in_word {
        words.push(current);
    }
    Ok(words)
}

/// `arg` as one word of a command line: bare when it is plain, quoted when it
/// holds whitespace or quotes (single quotes, or double with escapes when the
/// word itself contains a single quote).
pub fn quote_argument(arg: &str) -> String {
    let plain = !arg.is_empty()
        && !arg
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '\'' | '"'));
    if plain {
        arg.to_string()
    } else if !arg.contains('\'') {
        format!("'{arg}'")
    } else {
        format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// A script: one command per line; blank lines and `#` comments are skipped.
/// (A key containing `;` must survive, so `;` is not a separator.)
pub fn split_script(script: &str) -> Vec<String> {
    script
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.trim_end_matches(';').trim_end().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

fn parse_u64(text: &str, what: &str) -> Result<u64, S3CommandError> {
    text.parse::<u64>()
        .or_else(|_| error(format!("{what} must be a whole number, got '{text}'")))
}

/// `A-B`, `A-` or `-N` (the part after `bytes=`).
fn parse_range(spec: &str) -> Result<ByteRange, S3CommandError> {
    let Some((start, end)) = spec.split_once('-') else {
        return error(format!(
            "bad range '{spec}': use bytes=START-END, bytes=START- or bytes=-LAST"
        ));
    };
    match (start.is_empty(), end.is_empty()) {
        (true, true) => error("bad range 'bytes=-': give a number of bytes"),
        (true, false) => {
            let last = parse_u64(end, "the suffix length")?;
            if last == 0 {
                return error("a suffix range must be at least 1 byte");
            }
            Ok(ByteRange::Suffix(last))
        }
        (false, true) => Ok(ByteRange::From {
            start: parse_u64(start, "the range start")?,
            end: None,
        }),
        (false, false) => {
            let start = parse_u64(start, "the range start")?;
            let end = parse_u64(end, "the range end")?;
            if end < start {
                return error(format!(
                    "bad range: the end {end} is before the start {start}"
                ));
            }
            Ok(ByteRange::From {
                start,
                end: Some(end),
            })
        }
    }
}

fn unknown_option<T>(verb: Verb, flag: &str) -> Result<T, S3CommandError> {
    error(format!(
        "unknown option '{flag}' for {}; usage: {}",
        verb.name(),
        verb.usage()
    ))
}

/// Parse one command line.
pub fn parse(line: &str) -> Result<S3Command, S3CommandError> {
    let words = split_args(line)?;
    let Some((first, args)) = words.split_first() else {
        return error("empty command; type `help` for the commands");
    };
    let Ok(verb) = first.parse::<Verb>() else {
        return error(format!(
            "unknown S3 command '{first}' (commands: {}); this client only reads",
            command_names().join(", ")
        ));
    };
    match verb {
        Verb::Help | Verb::Pwd | Verb::Buckets => {
            if let Some(extra) = args.first() {
                return error(format!("{} takes no arguments, got '{extra}'", verb.name()));
            }
            Ok(match verb {
                Verb::Help => S3Command::Help,
                Verb::Pwd => S3Command::Pwd,
                _ => S3Command::Buckets,
            })
        }
        Verb::Cd => match args {
            [] => Ok(S3Command::Cd { prefix: None }),
            [prefix] => Ok(S3Command::Cd {
                prefix: Some(prefix.clone()),
            }),
            _ => error(format!("usage: {}", verb.usage())),
        },
        Verb::Head => match args {
            [key] if !key.starts_with('-') || key.len() == 1 => {
                Ok(S3Command::Head { key: key.clone() })
            }
            [flag] => unknown_option(verb, flag),
            _ => error(format!("usage: {}", verb.usage())),
        },
        Verb::Ls => {
            let mut prefix = None;
            let mut recursive = false;
            let mut limit = None;
            let mut iter = args.iter();
            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "-r" | "--recursive" => recursive = true,
                    "-n" | "--limit" => {
                        let Some(value) = iter.next() else {
                            return error(format!("{arg} needs a number"));
                        };
                        let n = parse_u64(value, "the limit")?;
                        if n == 0 {
                            return error("the limit must be at least 1");
                        }
                        limit = Some(usize::try_from(n).unwrap_or(usize::MAX));
                    }
                    flag if flag.starts_with('-') && flag.len() > 1 => {
                        return unknown_option(verb, flag);
                    }
                    _ if prefix.is_some() => {
                        return error(format!("usage: {}", verb.usage()));
                    }
                    other => prefix = Some(other.to_string()),
                }
            }
            Ok(S3Command::Ls {
                prefix,
                recursive,
                limit,
            })
        }
        Verb::Get => {
            let mut key = None;
            let mut range = None;
            let mut base64 = false;
            let mut iter = args.iter();
            while let Some(arg) = iter.next() {
                let lower = arg.to_ascii_lowercase();
                if let Some(spec) = lower.strip_prefix("bytes=") {
                    if range.replace(parse_range(spec)?).is_some() {
                        return error("give the range once");
                    }
                } else if matches!(lower.as_str(), "-b" | "--bytes") {
                    let Some(value) = iter.next() else {
                        return error(format!("{arg} needs a number of bytes"));
                    };
                    let n = parse_u64(value, "the byte count")?;
                    if n == 0 {
                        return error("the byte count must be at least 1");
                    }
                    if range.replace(ByteRange::First(n)).is_some() {
                        return error("give the range once");
                    }
                } else if lower == "--base64" {
                    base64 = true;
                } else if arg.starts_with('-') && arg.len() > 1 {
                    return unknown_option(verb, arg);
                } else if key.replace(arg.clone()).is_some() {
                    return error(format!("usage: {}", verb.usage()));
                }
            }
            let Some(key) = key else {
                return error(format!("usage: {}", verb.usage()));
            };
            Ok(S3Command::Get {
                key,
                range: range.unwrap_or(ByteRange::Default),
                base64,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[test]
    fn verbs_come_from_the_enum() {
        assert_eq!(
            command_names(),
            ["ls", "head", "get", "buckets", "cd", "pwd", "help"]
        );
        for verb in Verb::iter() {
            assert!(!verb.usage().is_empty() && !verb.description().is_empty());
            assert!(verb.usage().starts_with(verb.name()));
            assert_eq!(verb.name().parse::<Verb>().unwrap(), verb);
        }
    }

    #[rstest]
    #[case("ls", S3Command::Ls { prefix: None, recursive: false, limit: None })]
    #[case("LS logs/", S3Command::Ls { prefix: Some("logs/".into()), recursive: false, limit: None })]
    #[case("ls -r logs/ -n 50", S3Command::Ls { prefix: Some("logs/".into()), recursive: true, limit: Some(50) })]
    #[case("ls --limit 5 --recursive", S3Command::Ls { prefix: None, recursive: true, limit: Some(5) })]
    #[case("ls 'my folder/'", S3Command::Ls { prefix: Some("my folder/".into()), recursive: false, limit: None })]
    #[case("head a/b.txt", S3Command::Head { key: "a/b.txt".into() })]
    #[case("head \"with space.txt\"", S3Command::Head { key: "with space.txt".into() })]
    #[case("buckets", S3Command::Buckets)]
    #[case("pwd", S3Command::Pwd)]
    #[case("help", S3Command::Help)]
    #[case("cd", S3Command::Cd { prefix: None })]
    #[case("cd ..", S3Command::Cd { prefix: Some("..".into()) })]
    #[case("cd logs/2026/", S3Command::Cd { prefix: Some("logs/2026/".into()) })]
    #[case("get k", S3Command::Get { key: "k".into(), range: ByteRange::Default, base64: false })]
    #[case("get k bytes=0-1023", S3Command::Get { key: "k".into(), range: ByteRange::From { start: 0, end: Some(1023) }, base64: false })]
    #[case("get k BYTES=10-", S3Command::Get { key: "k".into(), range: ByteRange::From { start: 10, end: None }, base64: false })]
    #[case("get k bytes=-64", S3Command::Get { key: "k".into(), range: ByteRange::Suffix(64), base64: false })]
    #[case("get bytes=5-9 k --base64", S3Command::Get { key: "k".into(), range: ByteRange::From { start: 5, end: Some(9) }, base64: true })]
    #[case("get k --bytes 200", S3Command::Get { key: "k".into(), range: ByteRange::First(200), base64: false })]
    #[case("get k -b 7", S3Command::Get { key: "k".into(), range: ByteRange::First(7), base64: false })]
    fn commands_parse(#[case] line: &str, #[case] expected: S3Command) {
        assert_eq!(parse(line).unwrap(), expected, "{line}");
    }

    #[rstest]
    #[case("")]
    #[case("put k")]
    #[case("rm k")]
    #[case("delete k")]
    #[case("cp a b")]
    #[case("SELECT * FROM t")]
    #[case("ls a b")]
    #[case("ls -x")]
    #[case("ls -n")]
    #[case("ls -n 0")]
    #[case("ls -n many")]
    #[case("head")]
    #[case("head a b")]
    #[case("head -v")]
    #[case("get")]
    #[case("get a b")]
    #[case("get k bytes=5-2")]
    #[case("get k bytes=-")]
    #[case("get k bytes=-0")]
    #[case("get k bytes=x-3")]
    #[case("get k bytes=1-2 --bytes 3")]
    #[case("get k --bytes 0")]
    #[case("get k --bytes")]
    #[case("get k --presign")]
    #[case("buckets all")]
    #[case("pwd now")]
    #[case("cd a b")]
    #[case("ls 'unterminated")]
    fn bad_lines_are_refused(#[case] line: &str) {
        assert!(parse(line).is_err(), "{line:?} must not parse");
    }

    #[test]
    fn no_command_writes() {
        // Every verb is a read: the type has no write variant to reach
        for verb in command_names() {
            for write in [
                "put", "rm", "delete", "cp", "mv", "sync", "presign", "mb", "rb",
            ] {
                assert_ne!(verb, write);
            }
        }
        let message = parse("rm important.txt").unwrap_err().to_string();
        assert!(message.contains("unknown S3 command 'rm'"), "{message}");
        assert!(message.contains("only reads"), "{message}");
    }

    #[test]
    fn words_keep_quotes_and_escapes() {
        assert_eq!(
            split_args(r#"get "a \"b\" c" 'd e' f\g"#).unwrap(),
            ["get", r#"a "b" c"#, "d e", r"f\g"]
        );
        assert_eq!(split_args("  a   b  ").unwrap(), ["a", "b"]);
        assert_eq!(split_args("x''y").unwrap(), ["xy"]);
        assert_eq!(split_args("'' z").unwrap(), ["", "z"]);
        assert!(split_args("\"open").is_err());
    }

    #[rstest]
    #[case("logs/a.txt", "logs/a.txt")]
    #[case("my file.txt", "'my file.txt'")]
    #[case("it's.txt", "\"it's.txt\"")]
    #[case("say \"hi\" it's", "\"say \\\"hi\\\" it's\"")]
    #[case("", "''")]
    fn arguments_are_quoted_to_survive_the_tokenizer(#[case] key: &str, #[case] quoted: &str) {
        assert_eq!(quote_argument(key), quoted);
        assert_eq!(split_args(&quote_argument(key)).unwrap(), [key]);
    }

    #[test]
    fn scripts_are_one_command_per_line() {
        let script = "# list\nls logs/;\n\n  head 'a;b.txt'  \ncd ..;\n";
        assert_eq!(
            split_script(script),
            ["ls logs/", "head 'a;b.txt'", "cd .."]
        );
    }
}
