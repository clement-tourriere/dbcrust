//! The DynamoDB input language: PartiQL `SELECT`, or one typed command per line.
//!
//! DynamoDB has two read surfaces and DBCrust speaks both. A line is either a
//! command (`tables`, `describe`, `scan`, `get`, `query`, `help`: the verb is
//! the first word) or a PartiQL `SELECT … FROM "Table" WHERE …`. This module is
//! the single place that knows what is accepted: the DynamoDB driver executes a
//! parsed [`DynamoInput`], the `--read-only` guard accepts exactly what parses,
//! and completion lists [`Verb`]s.
//!
//! **Read-only by construction**: [`DynamoCommand`] has no variant that writes,
//! and the only PartiQL statement [`parse`] lets through is a single `SELECT`
//! (a lexer sees through comments, quoted identifiers and string literals, so
//! `INSERT`, `UPDATE`, `DELETE`, `EXISTS`, a second statement after a `;` or a
//! write hidden after a comment are all refused). Writes are not offered at all
//! in this release; the IAM policy of the credentials remains the server-side
//! limit.
//!
//! Grammar (verbs are case-insensitive; arguments are shell-like words, with
//! `'single'` and `"double"` quotes; `TABLE` may be left out when the
//! connection names a default table):
//!
//! ```text
//! tables [-n|--limit N]
//! describe [TABLE]
//! scan [TABLE] [--index INDEX] [-n|--limit N]
//! get [TABLE] KEY=VALUE [SORTKEY=VALUE]
//! query [TABLE] KEY=VALUE [SORTKEY=VALUE | SORTKEY^=PREFIX] [--index INDEX] [-n|--limit N] [--desc]
//! help
//! SELECT … FROM "Table"[."Index"] [WHERE …]
//! ```
//!
//! Key values are text: the driver converts them to the key attribute's type
//! (string, number or binary) after a `DescribeTable`.

use std::fmt;
use strum::{EnumIter, EnumString, IntoEnumIterator, IntoStaticStr};

/// The DynamoDB command words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, EnumString, IntoStaticStr)]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum Verb {
    Tables,
    Describe,
    Scan,
    Get,
    Query,
    Help,
}

impl Verb {
    pub fn name(self) -> &'static str {
        self.into()
    }

    pub fn usage(self) -> &'static str {
        match self {
            Verb::Tables => "tables [-n|--limit N]",
            Verb::Describe => "describe [TABLE]",
            Verb::Scan => "scan [TABLE] [--index INDEX] [-n|--limit N]",
            Verb::Get => "get [TABLE] KEY=VALUE [SORTKEY=VALUE]",
            Verb::Query => {
                "query [TABLE] KEY=VALUE [SORTKEY=VALUE | SORTKEY^=PREFIX] [--index INDEX] [-n|--limit N] [--desc]"
            }
            Verb::Help => "help",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Verb::Tables => "list the tables of the account and region (ListTables)",
            Verb::Describe => {
                "show a table's keys, indexes, billing mode, item count and size (DescribeTable)"
            }
            Verb::Scan => "read items in storage order, bounded (Scan: reads every item it passes)",
            Verb::Get => "read one item by its full primary key (GetItem)",
            Verb::Query => {
                "read the items of one partition, optionally by sort key (Query: the cheap read)"
            }
            Verb::Help => "list the commands and the PartiQL form",
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

/// PartiQL words the completion offers next to the commands.
pub const PARTIQL_KEYWORDS: [&str; 6] = ["SELECT", "FROM", "WHERE", "AND", "OR", "IN"];

/// How a key attribute is compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOp {
    /// `name=value`
    Eq,
    /// `name^=prefix` (`begins_with`, sort keys of strings and binaries)
    BeginsWith,
}

/// `name=value` (or `name^=prefix`) on a key attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCondition {
    pub name: String,
    pub op: KeyOp,
    pub value: String,
}

/// A parsed typed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DynamoCommand {
    Help,
    Tables {
        limit: Option<usize>,
    },
    Describe {
        table: Option<String>,
    },
    Scan {
        table: Option<String>,
        index: Option<String>,
        limit: Option<usize>,
    },
    /// One or two `=` conditions (partition key, then the sort key).
    Get {
        table: Option<String>,
        keys: Vec<KeyCondition>,
    },
    /// The partition key (`=`) and optionally a sort-key condition.
    Query {
        table: Option<String>,
        index: Option<String>,
        keys: Vec<KeyCondition>,
        limit: Option<usize>,
        descending: bool,
    },
}

/// What a line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DynamoInput {
    Command(DynamoCommand),
    /// A single PartiQL `SELECT`, without its trailing `;`.
    PartiQl(String),
}

/// Why a line is not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamoCommandError(pub String);

impl fmt::Display for DynamoCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DynamoCommandError {}

fn error<T>(message: impl Into<String>) -> Result<T, DynamoCommandError> {
    Err(DynamoCommandError(message.into()))
}

/// A script: one command per line; a PartiQL statement may span lines and ends
/// at its `;`. Blank lines and `#`/`--` comment lines between statements are
/// skipped.
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
        if pending.is_empty() {
            if line.is_empty() || line.starts_with('#') || line.starts_with("--") {
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

/// A token of a PartiQL statement; comments are dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// A bare word (keyword, name, number).
    Word(String),
    /// A `"quoted identifier"`, unescaped.
    Ident(String),
    /// A `'string literal'`, unescaped.
    Str(String),
    /// Any other character.
    Punct(char),
}

/// Split a PartiQL statement into tokens, skipping `--` and `/* */` comments;
/// an unterminated string, identifier or comment is an error.
pub fn lex_partiql(sql: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {}
            '-' if chars.peek() == Some(&'-') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut closed = false;
                while let Some(next) = chars.next() {
                    if next == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    return Err("unterminated /* comment".to_string());
                }
            }
            quote @ ('\'' | '"') => {
                let mut text = String::new();
                let mut closed = false;
                while let Some(next) = chars.next() {
                    if next == quote {
                        if chars.peek() == Some(&quote) {
                            chars.next();
                            text.push(quote);
                        } else {
                            closed = true;
                            break;
                        }
                    } else {
                        text.push(next);
                    }
                }
                if !closed {
                    return Err(if quote == '\'' {
                        "unterminated string literal".to_string()
                    } else {
                        "unterminated quoted identifier".to_string()
                    });
                }
                tokens.push(if quote == '\'' {
                    Token::Str(text)
                } else {
                    Token::Ident(text)
                });
            }
            c if c.is_alphanumeric() || c == '_' || c == '@' => {
                let mut word = String::from(c);
                while let Some(&next) = chars.peek() {
                    if next.is_alphanumeric() || matches!(next, '_' | '$' | '.') {
                        word.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token::Word(word));
            }
            other => tokens.push(Token::Punct(other)),
        }
    }
    Ok(tokens)
}

/// Whether `token` is the (case-insensitive) keyword `word`.
fn is_keyword(token: &Token, word: &str) -> bool {
    matches!(token, Token::Word(text) if text.eq_ignore_ascii_case(word))
}

/// The statement without its trailing `;`, when it is one PartiQL `SELECT`.
fn check_partiql_select(sql: &str) -> Result<String, DynamoCommandError> {
    let tokens = lex_partiql(sql).map_err(DynamoCommandError)?;
    let Some(first) = tokens.first() else {
        return error("empty statement; type `help` for the commands");
    };
    if !is_keyword(first, "select") {
        let shown = match first {
            Token::Word(word) => word.clone(),
            Token::Ident(text) => format!("\"{text}\""),
            Token::Str(_) => "a string".to_string(),
            Token::Punct(c) => c.to_string(),
        };
        return error(format!(
            "unknown DynamoDB command or statement '{shown}': this client only reads \
             (commands: {}; or a PartiQL SELECT)",
            command_names().join(", ")
        ));
    }
    if let Some(index) = tokens.iter().position(|token| *token == Token::Punct(';')) {
        if index + 1 != tokens.len() {
            return error("one statement at a time: nothing may follow the `;`");
        }
    }
    let trimmed = sql.trim().trim_end_matches(';').trim_end();
    Ok(trimmed.to_string())
}

/// The table (and index) a PartiQL `SELECT` reads: the name after its `FROM`.
pub fn partiql_target(sql: &str) -> Option<(String, Option<String>)> {
    let tokens = lex_partiql(sql).ok()?;
    let from = tokens.iter().position(|token| is_keyword(token, "from"))?;
    let name = |token: &Token| match token {
        Token::Ident(text) => Some(text.clone()),
        Token::Word(text) => Some(text.clone()),
        _ => None,
    };
    let table = name(tokens.get(from + 1)?)?;
    // `"Orders"."by_customer"`: a quoted table, a dot, a quoted index. A bare
    // `Orders.by_customer` is one word to the lexer (table names may hold dots)
    let index = match (tokens.get(from + 2), tokens.get(from + 3)) {
        (Some(Token::Punct('.')), Some(token)) => name(token),
        _ => None,
    };
    Some((table, index))
}

fn parse_limit(text: &str) -> Result<usize, DynamoCommandError> {
    match text.parse::<usize>() {
        Ok(0) => error("the limit must be at least 1"),
        Ok(n) => Ok(n),
        Err(_) => error(format!("the limit must be a whole number, got '{text}'")),
    }
}

fn unknown_option<T>(verb: Verb, flag: &str) -> Result<T, DynamoCommandError> {
    error(format!(
        "unknown option '{flag}' for {}; usage: {}",
        verb.name(),
        verb.usage()
    ))
}

/// `name=value` / `name^=value`, or `None` when the word is not a condition.
fn key_condition(word: &str) -> Option<KeyCondition> {
    let (name, value) = word.split_once('=')?;
    let (name, op) = match name.strip_suffix('^') {
        Some(name) => (name, KeyOp::BeginsWith),
        None => (name, KeyOp::Eq),
    };
    (!name.is_empty() && !name.starts_with('-')).then(|| KeyCondition {
        name: name.to_string(),
        op,
        value: value.to_string(),
    })
}

/// The shared tail of `scan`/`get`/`query`: table, key conditions, `--index`,
/// `--limit`, `--desc`.
struct Tail {
    table: Option<String>,
    keys: Vec<KeyCondition>,
    index: Option<String>,
    limit: Option<usize>,
    descending: bool,
}

fn parse_tail(verb: Verb, args: &[String]) -> Result<Tail, DynamoCommandError> {
    let mut tail = Tail {
        table: None,
        keys: Vec::new(),
        index: None,
        limit: None,
        descending: false,
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.to_ascii_lowercase().as_str() {
            "-n" | "--limit" if verb != Verb::Get => {
                let Some(value) = iter.next() else {
                    return error(format!("{arg} needs a number"));
                };
                tail.limit = Some(parse_limit(value)?);
            }
            "--index" | "-i" if verb != Verb::Get => {
                let Some(value) = iter.next() else {
                    return error(format!("{arg} needs an index name"));
                };
                tail.index = Some(value.clone());
            }
            "--desc" | "--descending" if verb == Verb::Query => tail.descending = true,
            _ if arg.starts_with('-') && arg.len() > 1 => return unknown_option(verb, arg),
            _ => {
                if let Some(condition) = key_condition(arg) {
                    if verb == Verb::Scan {
                        return error(format!("scan takes no key; usage: {}", verb.usage()));
                    }
                    tail.keys.push(condition);
                } else if tail.table.replace(arg.clone()).is_some() {
                    return error(format!("usage: {}", verb.usage()));
                }
            }
        }
    }
    Ok(tail)
}

/// Parse one line: a typed command, or a single PartiQL `SELECT`.
pub fn parse(line: &str) -> Result<DynamoInput, DynamoCommandError> {
    let trimmed = line.trim();
    let first = trimmed
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_end_matches(';');
    let Ok(verb) = first.parse::<Verb>() else {
        return check_partiql_select(trimmed).map(DynamoInput::PartiQl);
    };
    let words = crate::s3_command::split_args(trimmed.trim_end_matches(';'))
        .map_err(|e| DynamoCommandError(e.to_string()))?;
    let args = &words[1..];
    match verb {
        Verb::Help => {
            if let Some(extra) = args.first() {
                return error(format!("help takes no arguments, got '{extra}'"));
            }
            Ok(DynamoInput::Command(DynamoCommand::Help))
        }
        Verb::Tables => {
            let mut limit = None;
            let mut iter = args.iter();
            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "-n" | "--limit" => {
                        let Some(value) = iter.next() else {
                            return error(format!("{arg} needs a number"));
                        };
                        limit = Some(parse_limit(value)?);
                    }
                    flag if flag.starts_with('-') => return unknown_option(verb, flag),
                    _ => return error(format!("usage: {}", verb.usage())),
                }
            }
            Ok(DynamoInput::Command(DynamoCommand::Tables { limit }))
        }
        Verb::Describe => match args {
            [] => Ok(DynamoInput::Command(DynamoCommand::Describe {
                table: None,
            })),
            [table] if !table.starts_with('-') => {
                Ok(DynamoInput::Command(DynamoCommand::Describe {
                    table: Some(table.clone()),
                }))
            }
            [flag] => unknown_option(verb, flag),
            _ => error(format!("usage: {}", verb.usage())),
        },
        Verb::Scan => {
            let tail = parse_tail(verb, args)?;
            Ok(DynamoInput::Command(DynamoCommand::Scan {
                table: tail.table,
                index: tail.index,
                limit: tail.limit,
            }))
        }
        Verb::Get => {
            let tail = parse_tail(verb, args)?;
            if !(1..=2).contains(&tail.keys.len()) {
                return error(format!(
                    "give the full primary key as NAME=VALUE (the partition key, then the sort key if the table has one); usage: {}",
                    verb.usage()
                ));
            }
            if tail.keys.iter().any(|key| key.op != KeyOp::Eq) {
                return error("get reads one item: use NAME=VALUE (`query` takes a prefix)");
            }
            Ok(DynamoInput::Command(DynamoCommand::Get {
                table: tail.table,
                keys: tail.keys,
            }))
        }
        Verb::Query => {
            let tail = parse_tail(verb, args)?;
            if !(1..=2).contains(&tail.keys.len()) {
                return error(format!(
                    "give the partition key as NAME=VALUE, and optionally a sort key condition; usage: {}",
                    verb.usage()
                ));
            }
            if tail.keys.iter().all(|key| key.op != KeyOp::Eq) {
                return error("a query needs the partition key as NAME=VALUE");
            }
            Ok(DynamoInput::Command(DynamoCommand::Query {
                table: tail.table,
                index: tail.index,
                keys: tail.keys,
                limit: tail.limit,
                descending: tail.descending,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn key(name: &str, value: &str) -> KeyCondition {
        KeyCondition {
            name: name.to_string(),
            op: KeyOp::Eq,
            value: value.to_string(),
        }
    }

    fn command(input: DynamoInput) -> DynamoCommand {
        match input {
            DynamoInput::Command(command) => command,
            other => panic!("expected a command, got {other:?}"),
        }
    }

    #[test]
    fn verbs_come_from_the_enum() {
        assert_eq!(
            command_names(),
            ["tables", "describe", "scan", "get", "query", "help"]
        );
        for verb in Verb::iter() {
            assert!(!verb.usage().is_empty() && !verb.description().is_empty());
            assert!(verb.usage().starts_with(verb.name()));
            assert_eq!(verb.name().parse::<Verb>().unwrap(), verb);
        }
    }

    #[rstest]
    #[case("tables", DynamoCommand::Tables { limit: None })]
    #[case("TABLES -n 5", DynamoCommand::Tables { limit: Some(5) })]
    #[case("describe", DynamoCommand::Describe { table: None })]
    #[case("describe Orders", DynamoCommand::Describe { table: Some("Orders".into()) })]
    #[case("describe Orders;", DynamoCommand::Describe { table: Some("Orders".into()) })]
    #[case("scan", DynamoCommand::Scan { table: None, index: None, limit: None })]
    #[case("scan Orders --limit 20", DynamoCommand::Scan { table: Some("Orders".into()), index: None, limit: Some(20) })]
    #[case("scan Orders --index by_customer -n 3", DynamoCommand::Scan { table: Some("Orders".into()), index: Some("by_customer".into()), limit: Some(3) })]
    #[case("scan --limit 7", DynamoCommand::Scan { table: None, index: None, limit: Some(7) })]
    #[case("get Orders id=42", DynamoCommand::Get { table: Some("Orders".into()), keys: vec![key("id", "42")] })]
    #[case("get Orders id=42 ts=2026-01-01", DynamoCommand::Get { table: Some("Orders".into()), keys: vec![key("id", "42"), key("ts", "2026-01-01")] })]
    #[case("get id='a b'", DynamoCommand::Get { table: None, keys: vec![key("id", "a b")] })]
    #[case("get Orders id=a=b", DynamoCommand::Get { table: Some("Orders".into()), keys: vec![key("id", "a=b")] })]
    #[case("query Orders customer=c1", DynamoCommand::Query { table: Some("Orders".into()), index: None, keys: vec![key("customer", "c1")], limit: None, descending: false })]
    #[case("query Orders customer=c1 --index by_date --limit 10 --desc", DynamoCommand::Query { table: Some("Orders".into()), index: Some("by_date".into()), keys: vec![key("customer", "c1")], limit: Some(10), descending: true })]
    #[case("query Orders pk=a sk^=2026-", DynamoCommand::Query { table: Some("Orders".into()), index: None, keys: vec![key("pk", "a"), KeyCondition { name: "sk".into(), op: KeyOp::BeginsWith, value: "2026-".into() }], limit: None, descending: false })]
    #[case("help", DynamoCommand::Help)]
    fn commands_parse(#[case] line: &str, #[case] expected: DynamoCommand) {
        assert_eq!(command(parse(line).unwrap()), expected, "{line}");
    }

    #[rstest]
    #[case("select * from \"Orders\"")]
    #[case("SELECT * FROM \"Orders\" WHERE id = '1';")]
    #[case("  select id from Orders where a = 'x;y' ")]
    #[case("-- the orders\nSELECT * FROM \"Orders\"")]
    #[case("/* hint */ SELECT * FROM \"Orders\"")]
    #[case("SELECT * FROM \"Orders\" WHERE note = 'it''s DELETE'")]
    #[case("SELECT * FROM \"drop;table\"")]
    #[case("SELECT * FROM \"Orders\" -- ; DELETE FROM x")]
    fn partiql_selects_are_accepted(#[case] sql: &str) {
        let DynamoInput::PartiQl(statement) = parse(sql).unwrap() else {
            panic!("{sql}");
        };
        assert!(!statement.ends_with(';'), "{statement}");
    }

    #[test]
    fn the_trailing_semicolon_is_dropped_from_the_statement() {
        assert_eq!(
            parse("SELECT * FROM t1;").unwrap(),
            DynamoInput::PartiQl("SELECT * FROM t1".into())
        );
    }

    #[rstest]
    #[case::insert("INSERT INTO \"Orders\" VALUE {'id': '1'}")]
    #[case::update("UPDATE \"Orders\" SET a = 1 WHERE id = '1'")]
    #[case::delete("DELETE FROM \"Orders\" WHERE id = '1'")]
    #[case::exists("EXISTS(SELECT * FROM \"Orders\" WHERE id = '1')")]
    #[case::two_statements("SELECT * FROM \"A\"; DELETE FROM \"A\" WHERE id = '1'")]
    #[case::two_selects("SELECT * FROM \"A\"; SELECT * FROM \"B\"")]
    #[case::hidden_after_comment("/* SELECT */ DELETE FROM \"A\" WHERE id = '1'")]
    #[case::paren("(SELECT * FROM \"A\")")]
    #[case::put("put Orders id=1")]
    #[case::rm("rm Orders")]
    #[case::drop("DROP TABLE Orders")]
    #[case::create("CREATE TABLE x")]
    #[case::empty("")]
    #[case::semicolon_only(";")]
    #[case::unterminated_string("SELECT * FROM \"A\" WHERE a = 'x")]
    #[case::unterminated_ident("SELECT * FROM \"A")]
    #[case::unterminated_comment("SELECT * FROM A /* x")]
    #[case::number("42")]
    fn writes_and_non_commands_are_refused(#[case] line: &str) {
        assert!(parse(line).is_err(), "{line}");
    }

    #[rstest]
    #[case("tables extra")]
    #[case("tables -n 0")]
    #[case("tables -n x")]
    #[case("tables --unknown")]
    #[case("describe a b")]
    #[case("describe --x")]
    #[case("scan a b")]
    #[case("scan Orders id=1")]
    #[case("scan Orders --limit")]
    #[case("scan Orders --index")]
    #[case("get Orders")]
    #[case("get Orders a=1 b=2 c=3")]
    #[case("get Orders a^=1")]
    #[case("get Orders a=1 --limit 3")]
    #[case("query Orders")]
    #[case("query Orders sk^=a")]
    #[case("query Orders a=1 b=2 c=3")]
    #[case("query Orders a=1 --bogus")]
    #[case("help me")]
    #[case("get 'unterminated")]
    fn malformed_commands_are_refused(#[case] line: &str) {
        let error = parse(line).unwrap_err();
        assert!(!error.to_string().is_empty(), "{line}");
    }

    #[rstest]
    #[case("SELECT * FROM \"Orders\"", Some(("Orders", None)))]
    #[case("select a from \"Orders\" where x=1", Some(("Orders", None)))]
    #[case("SELECT * FROM \"Orders\".\"by_customer\" WHERE c='1'", Some(("Orders", Some("by_customer"))))]
    #[case("SELECT * FROM Orders", Some(("Orders", None)))]
    #[case("SELECT * FROM \"my.table\"", Some(("my.table", None)))]
    #[case("SELECT 1", None)]
    fn the_target_of_a_select(#[case] sql: &str, #[case] expected: Option<(&str, Option<&str>)>) {
        assert_eq!(
            partiql_target(sql),
            expected.map(|(table, index)| (table.to_string(), index.map(str::to_string)))
        );
    }

    #[test]
    fn scripts_split_by_command_and_statement() {
        let script = "# warm up\ntables\nSELECT *\n  FROM \"Orders\"\n  WHERE id = '1';\n\ndescribe Orders\nSELECT * FROM \"B\"";
        assert_eq!(
            split_script(script),
            [
                "tables",
                "SELECT *\nFROM \"Orders\"\nWHERE id = '1'",
                "describe Orders",
                "SELECT * FROM \"B\""
            ]
        );
        assert!(split_script("\n  \n-- nothing\n").is_empty());
    }
}
