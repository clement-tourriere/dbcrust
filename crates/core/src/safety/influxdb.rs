//! Read-only classification of InfluxDB queries: InfluxQL (v1, and v2/v3's
//! compatibility endpoints), Flux (v2) and SQL (v3).
//!
//! Default-deny and lexer-based (string literals, regexes, comments and quoted
//! identifiers never count as code). Like the other textual guards it is a
//! best-effort layer. The server does not back it up on its own: InfluxDB 1.x
//! only *warns* about a write sent as `GET /query` ("deprecated use of … in a
//! read only context") and still runs it, so the hard limit is a user with
//! `READ` privileges only (v1) or a read-only API token (v2/v3).

use crate::database::InfluxLanguage;

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Classify `query` without knowing the connection's language: Flux when it
/// looks like Flux (a pipeline, an import, `from(`…), otherwise InfluxQL / SQL.
/// Used by the generic `--read-only` guard; drivers that know their language
/// call [`check_influxdb_as`].
pub(super) fn check_influxdb(query: &str) -> Result<(), String> {
    if looks_like_flux(query) {
        check_flux(query)
    } else {
        check_influxql_or_sql(query, None)
    }
}

/// Classify `query` as `language`.
pub(super) fn check_influxdb_as(query: &str, language: InfluxLanguage) -> Result<(), String> {
    match language {
        InfluxLanguage::Flux => check_flux(query),
        InfluxLanguage::InfluxQl => check_influxql_or_sql(query, Some(InfluxLanguage::InfluxQl)),
        InfluxLanguage::Sql => check_influxql_or_sql(query, Some(InfluxLanguage::Sql)),
    }
}

/// Whether `query` is Flux rather than InfluxQL / SQL.
fn looks_like_flux(query: &str) -> bool {
    let mut rest = query.trim_start();
    // Leading `//` comments
    while let Some(after) = rest.strip_prefix("//") {
        rest = after
            .split_once('\n')
            .map_or("", |(_, tail)| tail)
            .trim_start();
    }
    let head: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    let next = rest[head.len()..].trim_start().chars().next();
    // `name = …` is a Flux assignment (InfluxQL/SQL never start that way)
    let assignment =
        !head.is_empty() && next == Some('=') && !rest[head.len()..].trim_start().starts_with("==");
    matches!(head.as_str(), "import" | "option" | "package")
        || (matches!(head.as_str(), "from" | "buckets") && next == Some('('))
        || assignment
        || query.contains("|>")
}

// ---------------------------------------------------------------------------
// InfluxQL / SQL
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum QlTok {
    /// Unquoted word, ASCII-uppercased.
    Word(String),
    /// Quoted identifier, string or regex literal (contents irrelevant).
    Quoted,
    Number,
    Punct(char),
}

/// Statement keywords refused wherever they appear unquoted: InfluxQL and the
/// SQL of v3 reserve them, so a legitimate read never contains one. `INTO` is
/// what turns an InfluxQL `SELECT` into a write.
const QL_DENIED_WORDS: &[&str] = &[
    "INTO", "DROP", "DELETE", "CREATE", "ALTER", "GRANT", "REVOKE", "KILL", "INSERT", "UPDATE",
    "TRUNCATE", "SET", "COPY", "ATTACH", "DETACH", "VACUUM",
];

fn lex_ql(query: &str) -> Result<Vec<QlTok>, String> {
    if query.contains('\0') {
        return Err("the query contains a NUL byte".to_string());
    }
    let chars: Vec<char> = query.chars().collect();
    let mut tokens: Vec<QlTok> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '-' if chars.get(i + 1) == Some(&'-') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                loop {
                    match chars.get(i) {
                        None => return Err("unterminated comment".to_string()),
                        Some('*') if chars.get(i + 1) == Some(&'/') => {
                            i += 2;
                            break;
                        }
                        Some(_) => i += 1,
                    }
                }
            }
            '\'' | '"' => {
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return Err("unterminated string or quoted identifier".to_string()),
                        Some('\\') => i += 2,
                        // SQL doubles the quote to escape it
                        Some(q) if *q == c && chars.get(i + 1) == Some(&c) => i += 2,
                        Some(q) if *q == c => {
                            i += 1;
                            break;
                        }
                        Some(_) => i += 1,
                    }
                }
                tokens.push(QlTok::Quoted);
            }
            // A regex literal follows FROM, a comma or a match operator;
            // anywhere else `/` divides
            '/' if matches!(
                tokens.last(),
                Some(QlTok::Word(w)) if w == "FROM" || w == "MEASUREMENT" || w == "KEY"
            ) || matches!(tokens.last(), Some(QlTok::Punct(',' | '~')))
                || matches!(tokens.last(), Some(QlTok::Punct('(' | '=' | '!'))) =>
            {
                i += 1;
                loop {
                    match chars.get(i) {
                        None | Some('\n') => {
                            return Err("unterminated regular expression".to_string());
                        }
                        Some('\\') => i += 2,
                        Some('/') => {
                            i += 1;
                            break;
                        }
                        Some(_) => i += 1,
                    }
                }
                tokens.push(QlTok::Quoted);
            }
            c if c.is_ascii_digit() => {
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_')
                {
                    i += 1;
                }
                tokens.push(QlTok::Number);
            }
            c if c.is_alphabetic() || c == '_' || !c.is_ascii() => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_alphanumeric() || chars[i] == '_' || !chars[i].is_ascii())
                {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                tokens.push(QlTok::Word(word.to_ascii_uppercase()));
            }
            other => {
                tokens.push(QlTok::Punct(other));
                i += 1;
            }
        }
    }
    Ok(tokens)
}

/// InfluxQL (`language` = `InfluxQl`), v3 SQL (`Sql`) or either (`None`).
fn check_influxql_or_sql(query: &str, language: Option<InfluxLanguage>) -> Result<(), String> {
    let tokens = lex_ql(query)?;
    let mut statements = tokens
        .split(|t| *t == QlTok::Punct(';'))
        .filter(|statement| !statement.is_empty());
    let mut any = false;
    for statement in statements.by_ref() {
        any = true;
        check_ql_statement(statement, language)?;
    }
    if !any {
        return Err("the statement is empty".to_string());
    }
    Ok(())
}

fn check_ql_statement(statement: &[QlTok], language: Option<InfluxLanguage>) -> Result<(), String> {
    // Leading parentheses: `(SELECT …) UNION …`
    let mut body = statement;
    while body.first() == Some(&QlTok::Punct('(')) {
        body = &body[1..];
    }
    let sql_allowed = language != Some(InfluxLanguage::InfluxQl);
    let first = match body.first() {
        Some(QlTok::Word(word)) => word.as_str(),
        _ => return Err(only_reads(language)),
    };
    match first {
        "SELECT" | "SHOW" => {}
        "EXPLAIN" => {
            // EXPLAIN [ANALYZE] [VERBOSE] <read>: the explained statement is
            // checked like any other
            let mut inner = &body[1..];
            while matches!(inner.first(), Some(QlTok::Word(w)) if w == "ANALYZE" || w == "VERBOSE")
            {
                inner = &inner[1..];
            }
            return check_ql_statement(inner, language);
        }
        "WITH" | "DESCRIBE" if sql_allowed => {
            if first == "WITH"
                && !body
                    .iter()
                    .any(|t| matches!(t, QlTok::Word(w) if w == "SELECT"))
            {
                return Err("WITH must be followed by a SELECT".to_string());
            }
        }
        _ => return Err(only_reads(language)),
    }
    for token in statement {
        if let QlTok::Word(word) = token
            && QL_DENIED_WORDS.contains(&word.as_str())
        {
            return Err(match word.as_str() {
                "INTO" => "SELECT … INTO writes data and is not allowed".to_string(),
                _ => format!("{word} is not allowed in read-only mode"),
            });
        }
    }
    Ok(())
}

fn only_reads(language: Option<InfluxLanguage>) -> String {
    match language {
        Some(InfluxLanguage::InfluxQl) => {
            "only SELECT and SHOW statements (and EXPLAIN of one) are allowed on InfluxDB"
        }
        Some(InfluxLanguage::Sql) => {
            "only SELECT, WITH … SELECT, SHOW, DESCRIBE and EXPLAIN statements are allowed on InfluxDB"
        }
        // The generic guard does not know the connection's language
        _ => {
            "only SELECT and SHOW statements (v3 SQL also WITH … SELECT, DESCRIBE and EXPLAIN) are allowed on InfluxDB"
        }
    }
    .to_string()
}

// ---------------------------------------------------------------------------
// Flux
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum FluxTok {
    Ident(String),
    /// A string literal; `interpolated` when it contains `${…}`.
    Str {
        value: String,
        interpolated: bool,
    },
    Number,
    /// `=~` or `!~`
    Match,
    Punct(char),
}

/// Packages a read-only query may import. Everything else is refused —
/// `http`, `experimental/http`, `sql`, `socket`, `kafka`, `mqtt`, `csv`
/// (`csv.from(url:)`), `influxdata/influxdb/secrets`, `…/tasks`, `…/monitor`,
/// the notification endpoints, `experimental/prometheus` and the like reach
/// outside the database or write to it.
const FLUX_ALLOWED_IMPORTS: &[&str] = &[
    "array",
    "date",
    "dict",
    "experimental",
    "experimental/aggregate",
    "experimental/array",
    "experimental/geo",
    "influxdata/influxdb",
    "influxdata/influxdb/schema",
    "influxdata/influxdb/v1",
    "interpolate",
    "json",
    "math",
    "regexp",
    "strings",
    "timezone",
    "types",
    "universe",
];

/// Functions that write, wherever they come from (`to`, `experimental.to`,
/// `influxdb.to`, `sql.to`, `mqtt.to`, `kafka.to`…): refused when called.
const FLUX_WRITE_FUNCTIONS: &[&str] = &["to", "wideTo"];

/// Named arguments of a `from(…)` call that point it at another server.
const FLUX_REMOTE_ARGUMENTS: &[&str] = &["host", "token", "url"];

fn lex_flux(query: &str) -> Result<Vec<FluxTok>, String> {
    if query.contains('\0') {
        return Err("the query contains a NUL byte".to_string());
    }
    let chars: Vec<char> = query.chars().collect();
    let mut tokens: Vec<FluxTok> = Vec::new();
    let mut i = 0;
    let mut braces = 0usize;
    // Brace depths at which a string interpolation `${` was opened
    let mut interpolations: Vec<usize> = Vec::new();
    // `true` when the next thing to read is the continuation of a string
    // (right after an interpolation closed)
    let mut in_string = false;

    macro_rules! read_string {
        ($interpolated:expr) => {{
            let mut value = String::new();
            let mut interpolated: bool = $interpolated;
            loop {
                match chars.get(i) {
                    None => return Err("unterminated string".to_string()),
                    Some('\\') => {
                        if let Some(escaped) = chars.get(i + 1) {
                            value.push(*escaped);
                        }
                        i += 2;
                    }
                    Some('"') => {
                        i += 1;
                        in_string = false;
                        break;
                    }
                    Some('$') if chars.get(i + 1) == Some(&'{') => {
                        i += 2;
                        braces += 1;
                        interpolations.push(braces);
                        interpolated = true;
                        in_string = false;
                        break;
                    }
                    Some(other) => {
                        value.push(*other);
                        i += 1;
                    }
                }
            }
            tokens.push(FluxTok::Str {
                value,
                interpolated,
            });
        }};
    }

    while i < chars.len() {
        if in_string {
            read_string!(true);
            continue;
        }
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '"' => {
                i += 1;
                read_string!(false);
            }
            '=' | '!' if chars.get(i + 1) == Some(&'~') => {
                tokens.push(FluxTok::Match);
                i += 2;
            }
            // A regex literal follows a match operator or opens an argument;
            // after a value `/` divides
            '/' if matches!(
                tokens.last(),
                None | Some(FluxTok::Match | FluxTok::Punct('(' | ',' | ':' | '[' | '{' | '='))
            ) =>
            {
                i += 1;
                loop {
                    match chars.get(i) {
                        None | Some('\n') => {
                            return Err("unterminated regular expression".to_string());
                        }
                        Some('\\') => i += 2,
                        Some('/') => {
                            i += 1;
                            break;
                        }
                        Some(_) => i += 1,
                    }
                }
                tokens.push(FluxTok::Number);
            }
            c if c.is_ascii_digit() => {
                // A date-time literal (2021-01-01T00:00:00Z) is one token;
                // otherwise digits plus duration units, so `10to(` is a
                // number followed by the identifier `to`
                let date_time = chars[i..].iter().take(5).collect::<String>().len() == 5
                    && chars[i..i + 4].iter().all(char::is_ascii_digit)
                    && chars[i + 4] == '-';
                if date_time {
                    while i < chars.len()
                        && (chars[i].is_ascii_alphanumeric()
                            || matches!(chars[i], '-' | ':' | '.' | '+'))
                    {
                        i += 1;
                    }
                } else {
                    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                        i += 1;
                    }
                    // Duration units, longest first, possibly chained (1h30m)
                    loop {
                        let rest: String = chars[i..].iter().take(2).collect();
                        let unit = ["mo", "ms", "us", "µs", "ns"]
                            .iter()
                            .find(|unit| rest.starts_with(**unit))
                            .map(|unit| unit.chars().count())
                            .or_else(|| {
                                matches!(chars.get(i), Some('y' | 'w' | 'd' | 'h' | 'm' | 's'))
                                    .then_some(1)
                            });
                        match unit {
                            Some(width) => {
                                i += width;
                                while i < chars.len() && chars[i].is_ascii_digit() {
                                    i += 1;
                                }
                            }
                            None => break,
                        }
                    }
                }
                tokens.push(FluxTok::Number);
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                tokens.push(FluxTok::Ident(chars[start..i].iter().collect()));
            }
            '{' => {
                braces += 1;
                tokens.push(FluxTok::Punct('{'));
                i += 1;
            }
            '}' => {
                i += 1;
                if interpolations.last() == Some(&braces) {
                    // The interpolated expression ends: back into the string
                    interpolations.pop();
                    braces -= 1;
                    in_string = true;
                } else {
                    braces = braces.saturating_sub(1);
                    tokens.push(FluxTok::Punct('}'));
                }
            }
            other => {
                tokens.push(FluxTok::Punct(other));
                i += 1;
            }
        }
    }
    if in_string || !interpolations.is_empty() {
        return Err("unterminated string interpolation".to_string());
    }
    Ok(tokens)
}

/// Flux (InfluxDB v2): imports are allow-listed, writers are refused, and
/// `from(host:, token:)` (a read of another server with credentials given
/// inline) is refused. An unbounded `from(bucket:)` is already rejected by
/// the server, which demands a `range()`.
pub(super) fn check_flux(query: &str) -> Result<(), String> {
    let tokens = lex_flux(query)?;
    if tokens.is_empty() {
        return Err("the query is empty".to_string());
    }
    // Innermost open call: (callee identifier, preceded by a dot)
    let mut calls: Vec<Option<String>> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        let next = tokens.get(i + 1);
        match token {
            FluxTok::Ident(word) => {
                if word == "import" {
                    // import [alias] "path"
                    let path = match (next, tokens.get(i + 2)) {
                        (
                            Some(FluxTok::Str {
                                value,
                                interpolated,
                            }),
                            _,
                        ) => Some((value, interpolated)),
                        (
                            Some(FluxTok::Ident(_)),
                            Some(FluxTok::Str {
                                value,
                                interpolated,
                            }),
                        ) => Some((value, interpolated)),
                        _ => None,
                    };
                    match path {
                        Some((value, false)) if FLUX_ALLOWED_IMPORTS.contains(&value.as_str()) => {}
                        Some((value, false)) => {
                            return Err(format!(
                                "importing \"{value}\" is not allowed in read-only mode (it can write or reach outside the database)"
                            ));
                        }
                        _ => return Err("an import must name a literal package".to_string()),
                    }
                } else if FLUX_WRITE_FUNCTIONS.contains(&word.as_str())
                    && next == Some(&FluxTok::Punct('('))
                {
                    return Err(format!("{word}() writes data and is not allowed"));
                } else if word == "builtin" {
                    return Err("builtin declarations are not allowed".to_string());
                } else if word == "option"
                    && matches!(next, Some(FluxTok::Ident(name)) if name == "task")
                {
                    return Err("option task is not allowed".to_string());
                } else if FLUX_REMOTE_ARGUMENTS.contains(&word.as_str())
                    && next == Some(&FluxTok::Punct(':'))
                    && calls
                        .last()
                        .is_some_and(|callee| callee.as_deref() == Some("from"))
                {
                    return Err(format!(
                        "from({word}: …) reads another server and is not allowed"
                    ));
                }
            }
            FluxTok::Punct('(') => {
                let callee = match i.checked_sub(1).and_then(|p| tokens.get(p)) {
                    Some(FluxTok::Ident(name)) => Some(name.clone()),
                    _ => None,
                };
                calls.push(callee);
            }
            FluxTok::Punct(')') => {
                calls.pop();
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("SELECT * FROM cpu")]
    #[case("select mean(\"usage\") from cpu where time > now() - 1h group by time(5m), host")]
    #[case("SELECT * FROM /^cpu.*/ WHERE host =~ /web-\\d+/ LIMIT 10")]
    #[case("SELECT last(*) FROM \"my measurement\"")]
    #[case("SELECT * FROM (SELECT value FROM cpu) ORDER BY time DESC")]
    #[case("SHOW DATABASES")]
    #[case("SHOW MEASUREMENTS WITH MEASUREMENT =~ /cpu/")]
    #[case("SHOW TAG VALUES FROM cpu WITH KEY = \"host\"")]
    #[case("SHOW FIELD KEYS FROM cpu; SHOW TAG KEYS FROM cpu")]
    #[case("EXPLAIN SELECT * FROM cpu")]
    #[case("EXPLAIN ANALYZE SELECT * FROM cpu")]
    #[case("SELECT 'DROP TABLE x' FROM cpu")]
    #[case("SELECT \"delete\" FROM cpu -- DROP")]
    #[case("SELECT /* INTO */ 1 FROM cpu;")]
    #[case("SELECT * FROM cpu WHERE host = 'a;DROP'")]
    fn influxql_reads_are_allowed(#[case] query: &str) {
        assert_eq!(check_influxdb_as(query, InfluxLanguage::InfluxQl), Ok(()));
        assert_eq!(check_influxdb(query), Ok(()), "generic guard");
    }

    #[rstest]
    #[case("DROP DATABASE telemetry", "only SELECT and SHOW")]
    #[case("DROP MEASUREMENT cpu", "only SELECT and SHOW")]
    #[case("DELETE FROM cpu WHERE time < now()", "only SELECT and SHOW")]
    #[case("CREATE DATABASE x", "only SELECT and SHOW")]
    #[case("ALTER RETENTION POLICY rp ON db DURATION 1h", "only SELECT and SHOW")]
    #[case("GRANT ALL ON db TO u", "only SELECT and SHOW")]
    #[case("REVOKE ALL PRIVILEGES FROM u", "only SELECT and SHOW")]
    #[case("KILL QUERY 3", "only SELECT and SHOW")]
    #[case("SELECT * INTO copy FROM cpu", "INTO")]
    #[case("SELECT * INTO db.rp.copy FROM cpu GROUP BY *", "INTO")]
    #[case("SELECT * FROM cpu; DROP DATABASE x", "only SELECT and SHOW")]
    #[case("SELECT * FROM cpu; SELECT * INTO c FROM cpu", "INTO")]
    #[case("SHOW DATABASES; DELETE FROM cpu", "only SELECT and SHOW")]
    #[case("EXPLAIN DROP DATABASE x", "only SELECT and SHOW")]
    #[case("", "empty")]
    #[case(";;", "empty")]
    #[case("-- nothing", "empty")]
    #[case("SELECT 'unterminated FROM cpu", "unterminated")]
    #[case("SELECT * FROM cpu /* never closed", "unterminated")]
    #[case("WITH x AS (SELECT 1) SELECT * FROM x", "only SELECT and SHOW")]
    #[case("SELECT * FROM cpu\0; DROP DATABASE x", "NUL")]
    fn influxql_writes_and_junk_are_refused(#[case] query: &str, #[case] reason: &str) {
        let error = check_influxdb_as(query, InfluxLanguage::InfluxQl).unwrap_err();
        assert!(error.contains(reason), "{query:?}: {error}");
        assert!(
            check_influxdb(query).is_err() || query.starts_with("WITH"),
            "{query:?}"
        );
    }

    #[rstest]
    #[case("SELECT * FROM cpu WHERE time > now() - interval '1 hour'")]
    #[case("WITH recent AS (SELECT * FROM cpu) SELECT count(*) FROM recent")]
    #[case("SHOW TABLES")]
    #[case("DESCRIBE cpu")]
    #[case("EXPLAIN SELECT * FROM cpu")]
    #[case("SELECT replace(host, 'a', 'b') FROM cpu")]
    fn sql_reads_are_allowed(#[case] query: &str) {
        assert_eq!(check_influxdb_as(query, InfluxLanguage::Sql), Ok(()));
    }

    #[rstest]
    #[case("INSERT INTO cpu VALUES (1)")]
    #[case("CREATE TABLE x (a int)")]
    #[case("COPY cpu TO 'x.csv'")]
    #[case("SELECT 1; DROP TABLE cpu")]
    #[case("WITH x AS (DELETE FROM cpu) SELECT 1")]
    #[case("SET datafusion.execution.batch_size = 1")]
    fn sql_writes_are_refused(#[case] query: &str) {
        assert!(
            check_influxdb_as(query, InfluxLanguage::Sql).is_err(),
            "{query:?}"
        );
    }

    #[rstest]
    #[case("from(bucket: \"b\") |> range(start: -1h)")]
    #[case(
        "from(bucket:\"b\")|>range(start:-1h)|>filter(fn:(r)=>r._measurement==\"cpu\" and r.host =~ /web-.*/)"
    )]
    #[case("buckets()")]
    #[case("import \"influxdata/influxdb/schema\"\nschema.measurements(bucket: \"b\")")]
    #[case(
        "import \"influxdata/influxdb/schema\"\nschema.measurementFieldKeys(bucket: \"b\", measurement: \"cpu\")"
    )]
    #[case(
        "import \"date\"\nimport \"strings\"\nfrom(bucket: \"b\") |> range(start: date.truncate(t: -1d, unit: 1h))"
    )]
    #[case(
        "import \"experimental\"\nfrom(bucket: \"b\") |> range(start: experimental.subDuration(d: 1h, from: now()))"
    )]
    #[case("import \"array\"\narray.from(rows: [{a: 1}])")]
    #[case("from(bucket: \"b\") |> range(start: 2021-01-01T00:00:00Z, stop: 2021-01-02T00:00:00Z)")]
    #[case("from(bucket: \"b\") |> range(start: -1h30m) |> filter(fn: (r) => r.to == \"x\")")]
    #[case("// to(bucket: \"x\")\nfrom(bucket: \"b\") |> range(start: -1h)")]
    #[case("from(bucket: \"b\") |> range(start: -1h) |> filter(fn: (r) => r._value > 10 / 2)")]
    #[case(
        "from(bucket: \"b\") |> range(start: -1h) |> map(fn: (r) => ({r with host: \"to(\", msg: \"${r.host}\"}))"
    )]
    #[case("from(bucket: \"to\") |> range(start: -1h) |> keep(columns: [\"host\", \"to\"])")]
    #[case("option now = () => 2021-01-02T00:00:00Z\nfrom(bucket: \"b\") |> range(start: -1h)")]
    #[case("x = 1\nfrom(bucket: \"b\") |> range(start: -x)")]
    fn flux_reads_are_allowed(#[case] query: &str) {
        assert_eq!(check_flux(query), Ok(()), "{query:?}");
        assert_eq!(check_influxdb(query), Ok(()), "generic guard: {query:?}");
    }

    #[rstest]
    #[case(
        "from(bucket: \"b\") |> range(start: -1h) |> to(bucket: \"copy\")",
        "to()"
    )]
    #[case(
        "from(bucket: \"b\") |> range(start: -1h) |> to (bucket: \"copy\", org: \"o\")",
        "to()"
    )]
    #[case(
        "import \"experimental\"\nfrom(bucket: \"b\") |> range(start: -1h) |> experimental.to(bucket: \"c\")",
        "to()"
    )]
    #[case(
        "import \"influxdata/influxdb\"\nfrom(bucket: \"b\") |> range(start: -1h) |> influxdb.wideTo(bucket: \"c\")",
        "wideTo()"
    )]
    #[case(
        "import \"influxdata/influxdb\"\nfrom(bucket: \"b\") |> range(start: -1h) |> influxdb.to(bucket: \"c\")",
        "to()"
    )]
    #[case("import \"http\"\nhttp.post(url: \"https://evil\")", "\"http\"")]
    #[case("import h \"http\"\nh.post(url: \"https://evil\")", "\"http\"")]
    #[case("import \"experimental/http\"\n1", "experimental/http")]
    #[case(
        "import \"sql\"\nsql.from(driverName: \"postgres\", dataSourceName: \"x\", query: \"y\")",
        "\"sql\""
    )]
    #[case(
        "import \"influxdata/influxdb/secrets\"\nsecrets.get(key: \"k\")",
        "secrets"
    )]
    #[case("import \"influxdata/influxdb/tasks\"\n1", "tasks")]
    #[case("import \"influxdata/influxdb/monitor\"\n1", "monitor")]
    #[case("import \"socket\"\n1", "\"socket\"")]
    #[case("import \"csv\"\ncsv.from(url: \"http://evil\")", "\"csv\"")]
    #[case(
        "import \"influxdata/influxdb/sample\"\nsample.data(set: \"airSensor\")",
        "sample"
    )]
    #[case("import \"kafka\"\n1", "\"kafka\"")]
    #[case("import \"slack\"\n1", "\"slack\"")]
    #[case(
        "import \"influxdata/influxdb\"\ninfluxdb.from(bucket: \"b\", host: \"https://other\", token: \"t\") |> range(start: -1h)",
        "from(host"
    )]
    #[case(
        "import \"influxdata/influxdb\"\ninfluxdb.from(bucket: \"b\", token: \"t\")",
        "from(token"
    )]
    #[case("import \"ht\" + \"tp\"\n1", "\"ht\"")]
    #[case("import \"${p}\"\n1", "literal")]
    #[case("import\n1", "literal")]
    #[case(
        "option task = {name: \"x\", every: 1h}\nfrom(bucket: \"b\") |> range(start: -1h)",
        "option task"
    )]
    #[case("builtin to : (<-tables: stream[A]) => stream[A]", "builtin")]
    #[case(
        "from(bucket: \"b\") |> range(start: -1h) |> map(fn: (r) => ({r with x: \"${to(bucket: \"c\")}\"}))",
        "to()"
    )]
    #[case("", "empty")]
    #[case("// only a comment", "empty")]
    #[case("from(bucket: \"b\") |> range(start: \"unterminated", "unterminated")]
    #[case(
        "from(bucket: \"b\") |> filter(fn: (r) => r.x =~ /unterminated",
        "regular expression"
    )]
    #[case("from(bucket: \"${x\") |> range(start: -1h)", "unterminated")]
    #[case("from(bucket: \"b\")\0 |> to(bucket: \"c\")", "NUL")]
    fn flux_writes_and_reach_outs_are_refused(#[case] query: &str, #[case] reason: &str) {
        let error = check_flux(query).unwrap_err();
        assert!(error.contains(reason), "{query:?}: {error}");
        assert!(check_influxdb(query).is_err(), "generic guard: {query:?}");
    }

    #[test]
    fn a_number_glued_to_to_still_counts_as_a_call() {
        // `10to(` is not a number: the identifier `to` is read on its own
        assert!(
            check_flux(
                "from(bucket: \"b\") |> range(start: -1h) |> map(fn: (r) => 10to(bucket: \"c\"))"
            )
            .is_err()
        );
    }

    #[test]
    fn the_language_decides_the_rules() {
        // A Flux program is not InfluxQL
        assert!(
            check_influxdb_as(
                "from(bucket: \"b\") |> range(start: -1h)",
                InfluxLanguage::InfluxQl
            )
            .is_err()
        );
        // InfluxQL has no WITH / DESCRIBE; v3 SQL does
        assert!(check_influxdb_as("DESCRIBE cpu", InfluxLanguage::InfluxQl).is_err());
        assert!(check_influxdb_as("DESCRIBE cpu", InfluxLanguage::Sql).is_ok());
    }

    #[rstest]
    #[case("from(bucket: \"b\")", true)]
    #[case("  // c\n from (bucket: \"b\")", true)]
    #[case("buckets()", true)]
    #[case("import \"array\"", true)]
    #[case("x = 1", true)]
    #[case("SELECT 1 |> 2", true)]
    #[case("SELECT * FROM cpu", false)]
    #[case("SHOW DATABASES", false)]
    #[case("x == 1", false)]
    #[case("", false)]
    fn flux_detection(#[case] query: &str, #[case] expected: bool) {
        assert_eq!(looks_like_flux(query), expected, "{query:?}");
    }
}
