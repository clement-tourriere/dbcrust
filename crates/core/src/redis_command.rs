//! `redis-cli` style command lines: `SET "a b" 'c d'` → `["SET", "a b", "c d"]`.
//!
//! Redis has no query language; DBCrust's "query" for a Redis connection is one
//! command line, split exactly like `redis-cli` does (`sdssplitargs`), so a line
//! that works there works here:
//!
//! * whitespace separates arguments;
//! * `"double quotes"` honour `\n \r \t \b \a \\ \" \xHH` (any other `\c` is
//!   `c`), so binary keys can be written;
//! * `'single quotes'` only honour `\'` (every other backslash is literal);
//! * a closing quote must be followed by whitespace or the end of the line;
//! * an unquoted `;` that ends the line is the statement terminator DBCrust's
//!   REPL and script splitter append, not part of the last argument.
//!
//! Arguments are bytes: Redis keys and values are binary-safe. The tokenizer
//! is pure so the driver (which executes the command) and the read-only guard
//! (which classifies it) agree on what a line means.

use std::fmt;

/// Why a command line could not be split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandLineError {
    /// A quote was opened and never closed.
    UnbalancedQuotes,
    /// A closing quote was directly followed by a non-space (`"a"b`).
    TextAfterClosingQuote,
    /// Nothing but whitespace (or a bare `;`).
    Empty,
}

impl fmt::Display for CommandLineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnbalancedQuotes => write!(f, "unbalanced quotes in the command line"),
            Self::TextAfterClosingQuote => {
                write!(
                    f,
                    "a closing quote must be followed by a space or the end of the line"
                )
            }
            Self::Empty => write!(f, "empty Redis command"),
        }
    }
}

impl std::error::Error for CommandLineError {}

/// Split `line` into the command name and its arguments (all as bytes).
///
/// Fails with [`CommandLineError::Empty`] when there is no command at all, so
/// a caller can rely on `result[0]` being the command name.
pub fn split_command_line(line: &str) -> Result<Vec<Vec<u8>>, CommandLineError> {
    let bytes = line.as_bytes();
    let mut args: Vec<Vec<u8>> = Vec::new();
    let mut i = 0;
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let mut current: Vec<u8> = Vec::new();
        match bytes[i] {
            b'"' => {
                i += 1;
                loop {
                    let Some(&c) = bytes.get(i) else {
                        return Err(CommandLineError::UnbalancedQuotes);
                    };
                    if c == b'\\' && bytes.get(i + 1).is_some() {
                        i += 1;
                        let escaped = bytes[i];
                        if escaped == b'x' {
                            let hex = bytes.get(i + 1..i + 3).and_then(|pair| {
                                std::str::from_utf8(pair)
                                    .ok()
                                    .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                            });
                            if let Some(byte) = hex {
                                current.push(byte);
                                i += 2;
                            } else {
                                current.push(b'x');
                            }
                        } else {
                            current.push(match escaped {
                                b'n' => b'\n',
                                b'r' => b'\r',
                                b't' => b'\t',
                                b'b' => 0x08,
                                b'a' => 0x07,
                                other => other,
                            });
                        }
                    } else if c == b'"' {
                        if !closing_quote_ok(bytes, i) {
                            return Err(CommandLineError::TextAfterClosingQuote);
                        }
                        i += 1;
                        break;
                    } else {
                        current.push(c);
                    }
                    i += 1;
                }
            }
            b'\'' => {
                i += 1;
                loop {
                    let Some(&c) = bytes.get(i) else {
                        return Err(CommandLineError::UnbalancedQuotes);
                    };
                    if c == b'\\' && bytes.get(i + 1) == Some(&b'\'') {
                        i += 1;
                        current.push(b'\'');
                    } else if c == b'\'' {
                        if !closing_quote_ok(bytes, i) {
                            return Err(CommandLineError::TextAfterClosingQuote);
                        }
                        i += 1;
                        break;
                    } else {
                        current.push(c);
                    }
                    i += 1;
                }
            }
            _ => {
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    current.push(bytes[i]);
                    i += 1;
                }
            }
        }
        args.push(current);
    }

    // The statement terminator: `GET k;` and `GET k ;` both mean `GET k`
    if let Some(last) = args.last_mut() {
        let terminated_unquoted = !line_ends_in_quote(line);
        if terminated_unquoted && last.as_slice() == b";" {
            args.pop();
        } else if terminated_unquoted && last.ends_with(b";") && last.len() > 1 {
            last.pop();
        }
    }
    if args.is_empty() {
        return Err(CommandLineError::Empty);
    }
    Ok(args)
}

/// A closing quote at `at` must be followed by whitespace, the end of the line,
/// or the statement terminator (`GET "a b";`).
fn closing_quote_ok(bytes: &[u8], at: usize) -> bool {
    match bytes.get(at + 1) {
        None => true,
        Some(next) if next.is_ascii_whitespace() => true,
        Some(b';') => bytes[at + 2..].iter().all(u8::is_ascii_whitespace),
        Some(_) => false,
    }
}

/// Whether the last argument of `line` was a quoted one (its `;` suffix, if
/// any, is then data, not a terminator).
fn line_ends_in_quote(line: &str) -> bool {
    matches!(line.trim_end().as_bytes().last(), Some(b'"' | b'\''))
}

/// The commands most Redis servers have, for completion where the server's
/// own list (`COMMAND LIST`, Redis 7+) is unavailable or not worth a round trip.
pub const COMMON_COMMANDS: &[&str] = &[
    "APPEND",
    "BITCOUNT",
    "DBSIZE",
    "DECR",
    "DECRBY",
    "DEL",
    "DUMP",
    "ECHO",
    "EXISTS",
    "EXPIRE",
    "EXPIREAT",
    "EXPIRETIME",
    "FLUSHDB",
    "GET",
    "GETDEL",
    "GETEX",
    "GETRANGE",
    "GETSET",
    "HDEL",
    "HEXISTS",
    "HGET",
    "HGETALL",
    "HINCRBY",
    "HKEYS",
    "HLEN",
    "HMGET",
    "HMSET",
    "HSCAN",
    "HSET",
    "HSETNX",
    "HVALS",
    "INCR",
    "INCRBY",
    "INCRBYFLOAT",
    "INFO",
    "LINDEX",
    "LLEN",
    "LPOP",
    "LPUSH",
    "LRANGE",
    "LREM",
    "LSET",
    "LTRIM",
    "MEMORY",
    "MGET",
    "MSET",
    "OBJECT",
    "PERSIST",
    "PEXPIRE",
    "PING",
    "PTTL",
    "RENAME",
    "RPOP",
    "RPUSH",
    "SADD",
    "SCAN",
    "SCARD",
    "SET",
    "SETEX",
    "SETNX",
    "SISMEMBER",
    "SMEMBERS",
    "SPOP",
    "SREM",
    "SSCAN",
    "STRLEN",
    "TTL",
    "TYPE",
    "UNLINK",
    "XADD",
    "XINFO",
    "XLEN",
    "XRANGE",
    "XREVRANGE",
    "ZADD",
    "ZCARD",
    "ZCOUNT",
    "ZRANGE",
    "ZRANGEBYSCORE",
    "ZREM",
    "ZSCAN",
    "ZSCORE",
];

/// `value` as one `redis-cli` argument: bare when it is plain, otherwise
/// double-quoted with `\` and `"` escaped (what [`split_command_line`] reads back).
pub fn quote_argument(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | '\\' | ';'));
    if plain {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// Split a script (`-f file`, stdin, a pasted block) into commands: one per
/// line, as `redis-cli` reads them. Blank lines and `#` comment lines are
/// skipped; a command cannot span lines (use `\n` inside a quoted string).
pub fn split_script(script: &str) -> Vec<String> {
    script
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// The upper-cased command name of a split line (`GET` for `get k`).
pub fn command_name(args: &[Vec<u8>]) -> String {
    args.first()
        .map(|name| String::from_utf8_lossy(name).to_ascii_uppercase())
        .unwrap_or_default()
}

/// The upper-cased first argument (a subcommand such as `INFO` of `MEMORY`).
pub fn subcommand_name(args: &[Vec<u8>]) -> Option<String> {
    args.get(1)
        .map(|name| String::from_utf8_lossy(name).to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn split(line: &str) -> Vec<String> {
        split_command_line(line)
            .unwrap()
            .into_iter()
            .map(|arg| String::from_utf8_lossy(&arg).into_owned())
            .collect()
    }

    #[rstest]
    #[case("GET key", &["GET", "key"])]
    #[case("  GET   key  ", &["GET", "key"])]
    #[case("SET \"a b\" 'c d'", &["SET", "a b", "c d"])]
    #[case(r#"SET k "line\nbreak\ttab""#, &["SET", "k", "line\nbreak\ttab"])]
    #[case(r#"SET k "say \"hi\"""#, &["SET", "k", "say \"hi\""])]
    #[case(r#"SET k "back\\slash""#, &["SET", "k", "back\\slash"])]
    #[case(r#"SET k "\x41\x42""#, &["SET", "k", "AB"])]
    #[case(r#"SET k "\xZZ""#, &["SET", "k", "xZZ"])]
    #[case(r#"SET k 'it\'s'"#, &["SET", "k", "it's"])]
    #[case(r#"SET k 'no\nescape'"#, &["SET", "k", "no\\nescape"])]
    #[case(r#"SET k """#, &["SET", "k", ""])]
    #[case("SET k ''", &["SET", "k", ""])]
    #[case(r#"SET k a\nb"#, &["SET", "k", "a\\nb"])]
    #[case("GET key;", &["GET", "key"])]
    #[case(r#"GET "a b";"#, &["GET", "a b"])]
    #[case("GET key ;", &["GET", "key"])]
    #[case(r#"SET k "a;""#, &["SET", "k", "a;"])]
    #[case("SCAN 0 MATCH user:* COUNT 100", &["SCAN", "0", "MATCH", "user:*", "COUNT", "100"])]
    #[case("SET k ünïcödé", &["SET", "k", "ünïcödé"])]
    fn splits_like_redis_cli(#[case] line: &str, #[case] expected: &[&str]) {
        assert_eq!(split(line), expected);
    }

    #[rstest]
    #[case("", CommandLineError::Empty)]
    #[case("   ", CommandLineError::Empty)]
    #[case(";", CommandLineError::Empty)]
    #[case("SET k \"open", CommandLineError::UnbalancedQuotes)]
    #[case("SET k 'open", CommandLineError::UnbalancedQuotes)]
    #[case("SET k \"a\"b", CommandLineError::TextAfterClosingQuote)]
    #[case("SET k 'a'b", CommandLineError::TextAfterClosingQuote)]
    fn rejects_malformed_lines(#[case] line: &str, #[case] expected: CommandLineError) {
        assert_eq!(split_command_line(line), Err(expected));
    }

    #[test]
    fn hex_escapes_produce_raw_bytes() {
        let args = split_command_line(r#"SET k "\xff\x00""#).unwrap();
        assert_eq!(args[2], vec![0xff, 0x00]);
    }

    #[rstest]
    #[case("user:1", "user:1")]
    #[case("a b", "\"a b\"")]
    #[case("say \"hi\"", "\"say \\\"hi\\\"\"")]
    #[case("back\\slash", "\"back\\\\slash\"")]
    #[case("", "\"\"")]
    #[case("a;b", "\"a;b\"")]
    fn quoted_arguments_read_back_as_the_original(#[case] value: &str, #[case] quoted: &str) {
        assert_eq!(quote_argument(value), quoted);
        let line = format!("GET {quoted}");
        let args = split_command_line(&line).unwrap();
        assert_eq!(args[1], value.as_bytes());
    }

    #[test]
    fn scripts_are_one_command_per_line() {
        let script = "# warm up\nSET a 1\n\n  GET a  \r\nHSET h f \"v;w\"\n";
        assert_eq!(
            split_script(script),
            ["SET a 1", "GET a", "HSET h f \"v;w\""]
        );
        assert!(split_script("\n# only a comment\n").is_empty());
    }

    #[test]
    fn command_and_subcommand_are_upper_cased() {
        let args = split_command_line("memory usage k").unwrap();
        assert_eq!(command_name(&args), "MEMORY");
        assert_eq!(subcommand_name(&args).as_deref(), Some("USAGE"));
    }
}
