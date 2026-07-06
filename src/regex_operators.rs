//! Cross-backend translation of PostgreSQL-style regex operators.
//!
//! DBCrust lets users write the PostgreSQL regex operators in SQL against every
//! backend: `col ~ 'pattern'` (match), `~*` (case-insensitive match), `!~` and
//! `!~*` (negations). PostgreSQL and DataFusion understand them natively; the
//! other backends get the operator rewritten to their own regex construct
//! before the query is sent:
//!
//! | Target        | `col ~ 'p'`        | `col ~* 'p'`            |
//! |---------------|--------------------|-------------------------|
//! | MySQL/MariaDB | `col REGEXP 'p'`   | `col REGEXP '(?i)p'`    |
//! | SQLite        | `col REGEXP 'p'`   | `col REGEXP '(?i)p'`    |
//! | ClickHouse    | `match(col, 'p')`  | `match(col, '(?i)p')`   |
//! | Elasticsearch | `col RLIKE 'p'`    | unsupported (error)     |
//!
//! SQLite support relies on the `regexp` function registered by sqlx
//! (`SqliteConnectOptions::with_regexp`, Rust `regex` crate semantics).
//! Elasticsearch SQL's `RLIKE` uses Lucene regex syntax, which has no
//! case-insensitive flag, so `~*`/`!~*` are rejected with a clear error.
//!
//! Queries are parsed with sqlparser's permissive `GenericDialect`. A query
//! that fails to parse is passed through unchanged so backend-specific syntax
//! keeps working; a query with no regex operator is returned verbatim.

use crate::database::DatabaseError;
use datafusion::sql::sqlparser::ast::{
    BinaryOperator, Expr, Function, FunctionArg, FunctionArgExpr, FunctionArgumentList,
    FunctionArguments, Ident, ObjectName, UnaryOperator, Value, ValueWithSpan,
    visit_expressions_mut,
};
use datafusion::sql::sqlparser::dialect::GenericDialect;
use datafusion::sql::sqlparser::parser::Parser;
use std::ops::ControlFlow;
use tracing::debug;

/// Which backend the regex operators are being translated for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegexTarget {
    /// `expr REGEXP 'pattern'` (also MariaDB). Case-sensitivity of `~` follows
    /// the column collation; `~*` forces insensitivity with an inline `(?i)`.
    MySql,
    /// `expr REGEXP 'pattern'` via sqlx's registered `regexp()` function.
    Sqlite,
    /// `match(expr, 'pattern')` (RE2 syntax).
    ClickHouse,
    /// `expr RLIKE 'pattern'` (Lucene syntax) for the Elasticsearch SQL API.
    ElasticsearchSql,
}

/// Rewrite PostgreSQL regex operators (`~`, `~*`, `!~`, `!~*`) in `sql` into
/// the construct understood by `target`. Returns the input unchanged when it
/// contains no regex operator or cannot be parsed as standard SQL.
pub fn translate_regex_operators(sql: &str, target: RegexTarget) -> Result<String, DatabaseError> {
    // Fast path: no tilde anywhere means no regex operator.
    if !sql.contains('~') {
        return Ok(sql.to_string());
    }

    let mut statements = match Parser::parse_sql(&GenericDialect {}, sql) {
        Ok(statements) => statements,
        Err(e) => {
            debug!(
                "[translate_regex_operators] Leaving query unchanged (not parseable as generic SQL): {e}"
            );
            return Ok(sql.to_string());
        }
    };

    let mut replaced = false;
    let flow = visit_expressions_mut(&mut statements, |expr| {
        let Expr::BinaryOp { left, op, right } = expr else {
            return ControlFlow::Continue(());
        };
        let (negated, case_insensitive) = match op {
            BinaryOperator::PGRegexMatch => (false, false),
            BinaryOperator::PGRegexIMatch => (false, true),
            BinaryOperator::PGRegexNotMatch => (true, false),
            BinaryOperator::PGRegexNotIMatch => (true, true),
            _ => return ControlFlow::Continue(()),
        };

        if case_insensitive && target == RegexTarget::ElasticsearchSql {
            return ControlFlow::Break(
                "Elasticsearch SQL does not support case-insensitive regex matching \
                 (Lucene regex has no (?i) flag); use ~ / !~ with explicit character classes"
                    .to_string(),
            );
        }

        let matched = (**left).clone();
        let mut pattern = (**right).clone();
        if case_insensitive {
            pattern = case_insensitive_pattern(pattern, target);
        }

        *expr = match target {
            RegexTarget::MySql | RegexTarget::Sqlite => Expr::RLike {
                negated,
                expr: Box::new(matched),
                pattern: Box::new(pattern),
                regexp: true,
            },
            RegexTarget::ElasticsearchSql => Expr::RLike {
                negated,
                expr: Box::new(matched),
                pattern: Box::new(pattern),
                regexp: false,
            },
            RegexTarget::ClickHouse => {
                let call = function_call("match", vec![matched, pattern]);
                if negated {
                    Expr::UnaryOp {
                        op: UnaryOperator::Not,
                        expr: Box::new(call),
                    }
                } else {
                    call
                }
            }
        };
        replaced = true;
        ControlFlow::Continue(())
    });

    if let ControlFlow::Break(message) = flow {
        return Err(DatabaseError::QueryError(message));
    }

    if !replaced {
        // Tilde was inside a literal/comment or some other operator; keep the
        // user's original formatting.
        return Ok(sql.to_string());
    }

    let translated = statements
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    debug!("[translate_regex_operators] Rewrote regex operators: {sql} -> {translated}");
    Ok(translated)
}

/// Force case-insensitive matching by prefixing the pattern with `(?i)`
/// (understood by MySQL 8 ICU, MariaDB PCRE, Rust regex, and RE2).
fn case_insensitive_pattern(pattern: Expr, target: RegexTarget) -> Expr {
    if let Expr::Value(ValueWithSpan {
        value: Value::SingleQuotedString(s),
        ..
    }) = &pattern
    {
        return string_literal(format!("(?i){s}"));
    }

    // Non-literal pattern: build the prefix at query time.
    match target {
        RegexTarget::Sqlite => Expr::Nested(Box::new(Expr::BinaryOp {
            left: Box::new(string_literal("(?i)".to_string())),
            op: BinaryOperator::StringConcat,
            right: Box::new(pattern),
        })),
        _ => function_call("concat", vec![string_literal("(?i)".to_string()), pattern]),
    }
}

fn string_literal(s: String) -> Expr {
    Expr::Value(Value::SingleQuotedString(s).with_empty_span())
}

fn function_call(name: &str, args: Vec<Expr>) -> Expr {
    Expr::Function(Function {
        name: ObjectName::from(vec![Ident::new(name)]),
        uses_odbc_syntax: false,
        parameters: FunctionArguments::None,
        args: FunctionArguments::List(FunctionArgumentList {
            duplicate_treatment: None,
            args: args
                .into_iter()
                .map(|e| FunctionArg::Unnamed(FunctionArgExpr::Expr(e)))
                .collect(),
            clauses: vec![],
        }),
        filter: None,
        null_treatment: None,
        over: None,
        within_group: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    // MySQL / MariaDB
    #[case(
        RegexTarget::MySql,
        "SELECT * FROM users WHERE name ~ 'AKIA[0-9A-Z]{16}'",
        "SELECT * FROM users WHERE name REGEXP 'AKIA[0-9A-Z]{16}'"
    )]
    #[case(
        RegexTarget::MySql,
        "SELECT * FROM users WHERE name ~* 'akia'",
        "SELECT * FROM users WHERE name REGEXP '(?i)akia'"
    )]
    #[case(
        RegexTarget::MySql,
        "SELECT * FROM users WHERE name !~ 'x' AND id > 3",
        "SELECT * FROM users WHERE name NOT REGEXP 'x' AND id > 3"
    )]
    #[case(
        RegexTarget::MySql,
        "SELECT * FROM `my table` WHERE `a b` ~ 'x'",
        "SELECT * FROM `my table` WHERE `a b` REGEXP 'x'"
    )]
    // SQLite
    #[case(
        RegexTarget::Sqlite,
        "SELECT * FROM logs WHERE line !~* 'error'",
        "SELECT * FROM logs WHERE line NOT REGEXP '(?i)error'"
    )]
    // ClickHouse
    #[case(
        RegexTarget::ClickHouse,
        "SELECT * FROM logs WHERE line ~ 'ghp_[0-9A-Za-z]{36}'",
        "SELECT * FROM logs WHERE match(line, 'ghp_[0-9A-Za-z]{36}')"
    )]
    #[case(
        RegexTarget::ClickHouse,
        "SELECT * FROM logs WHERE line !~* 'error'",
        "SELECT * FROM logs WHERE NOT match(line, '(?i)error')"
    )]
    #[case(
        RegexTarget::ClickHouse,
        "SELECT * FROM logs WHERE line ~ 'x' FORMAT JSONEachRow",
        "SELECT * FROM logs WHERE match(line, 'x') FORMAT JSONEachRow"
    )]
    // Elasticsearch SQL
    #[case(
        RegexTarget::ElasticsearchSql,
        "SELECT * FROM \"logs-2024.01\" WHERE message ~ 'AKIA[0-9A-Z]+'",
        "SELECT * FROM \"logs-2024.01\" WHERE message RLIKE 'AKIA[0-9A-Z]+'"
    )]
    #[case(
        RegexTarget::ElasticsearchSql,
        "SELECT * FROM logs WHERE message !~ 'x'",
        "SELECT * FROM logs WHERE message NOT RLIKE 'x'"
    )]
    fn test_translates_regex_operators(
        #[case] target: RegexTarget,
        #[case] input: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(translate_regex_operators(input, target).unwrap(), expected);
    }

    #[rstest]
    // No tilde at all: fast path.
    #[case("SELECT * FROM users WHERE name = 'joe'")]
    // Tilde inside a string literal is not an operator.
    #[case("SELECT * FROM users WHERE name = 'a ~ b'")]
    // Backend-specific syntax that GenericDialect cannot parse: passthrough.
    #[case("GET _cat/indices WHERE x ~ 'y'")]
    fn test_leaves_query_untouched(#[case] input: &str) {
        for target in [
            RegexTarget::MySql,
            RegexTarget::Sqlite,
            RegexTarget::ClickHouse,
            RegexTarget::ElasticsearchSql,
        ] {
            assert_eq!(
                translate_regex_operators(input, target).unwrap(),
                input,
                "target {target:?} must not rewrite: {input}"
            );
        }
    }

    #[test]
    fn test_case_insensitive_rejected_for_elasticsearch() {
        let err = translate_regex_operators(
            "SELECT * FROM logs WHERE message ~* 'x'",
            RegexTarget::ElasticsearchSql,
        )
        .unwrap_err();
        assert!(err.to_string().contains("case-insensitive"));
    }

    #[test]
    fn test_non_literal_pattern_gets_runtime_prefix() {
        assert_eq!(
            translate_regex_operators("SELECT * FROM t WHERE a ~* b", RegexTarget::Sqlite).unwrap(),
            "SELECT * FROM t WHERE a REGEXP ('(?i)' || b)"
        );
        assert_eq!(
            translate_regex_operators("SELECT * FROM t WHERE a ~* b", RegexTarget::ClickHouse)
                .unwrap(),
            "SELECT * FROM t WHERE match(a, concat('(?i)', b))"
        );
    }

    #[test]
    fn test_multiple_statements_and_operators() {
        assert_eq!(
            translate_regex_operators(
                "SELECT 1 WHERE a ~ 'x'; SELECT 2 WHERE b ~* 'y'",
                RegexTarget::MySql
            )
            .unwrap(),
            "SELECT 1 WHERE a REGEXP 'x'; SELECT 2 WHERE b REGEXP '(?i)y'"
        );
    }

    #[test]
    fn test_regex_operator_in_projection() {
        assert_eq!(
            translate_regex_operators(
                "SELECT name ~ 'x' AS matches FROM t",
                RegexTarget::ClickHouse
            )
            .unwrap(),
            "SELECT match(name, 'x') AS matches FROM t"
        );
    }
}
