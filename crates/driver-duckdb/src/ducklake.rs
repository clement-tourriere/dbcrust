//! DuckLake: a lakehouse catalog attached through DuckDB's `ducklake` extension.
//!
//! DuckLake keeps its metadata in an ordinary database (a DuckDB file, SQLite
//! or PostgreSQL) and its data as Parquet files under `DATA_PATH`. DBCrust
//! attaches one when the URL carries `ducklake=<catalog>`:
//!
//! ```text
//! duckdb://:memory:?ducklake=sqlite:/srv/lake/meta.sqlite&data_path=/srv/lake/data/&alias=lake
//! ```
//!
//! which runs `ATTACH 'ducklake:sqlite:/srv/lake/meta.sqlite' AS lake
//! (DATA_PATH '/srv/lake/data/'); USE lake;` once, right after opening. The
//! extension has to be installed (`install_extensions=true` downloads it from
//! DuckDB's extension repository, a network access that is never made without
//! that opt-in) and a hardened (`--read-only`, broker) connection refuses the
//! whole thing: it never installs or loads an extension.

use crate::conn::{option, sql_literal};
use dbcrust_core::database::{ConnectionInfo, DatabaseError};

const DEFAULT_ALIAS: &str = "lake";

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.len() <= 63
}

/// The extensions a catalog spec needs besides `ducklake` itself
/// (`sqlite:…` and `postgres:…` catalogs go through their scanners).
pub(crate) fn catalog_extension(catalog: &str) -> Option<&'static str> {
    let scheme = catalog.split(':').next().unwrap_or_default();
    match scheme.to_ascii_lowercase().as_str() {
        "sqlite" => Some("sqlite"),
        "postgres" | "postgresql" => Some("postgres"),
        _ => None,
    }
}

/// The statements that attach `catalog` as a DuckLake and make it current.
pub(crate) fn attach_statements(
    info: &ConnectionInfo,
    catalog: &str,
) -> Result<Vec<String>, DatabaseError> {
    let invalid = |message: String| Err(DatabaseError::InvalidUrl(message));
    let catalog = catalog.trim();
    if catalog.is_empty() {
        return invalid(
            "ducklake needs the metadata catalog (ducklake=sqlite:/path/meta.sqlite, \
             ducklake=postgres:dbname=lake host=db, or a DuckDB file)"
                .to_string(),
        );
    }
    let lowered = catalog.to_ascii_lowercase();
    if lowered.contains("password=") || lowered.contains("passwd=") {
        return invalid(
            "the DuckLake catalog must not carry a password: use PGPASSWORD or a ~/.pgpass entry"
                .to_string(),
        );
    }
    let alias = option(info, "alias").unwrap_or(DEFAULT_ALIAS);
    if !is_identifier(alias) {
        return invalid(format!(
            "invalid DuckLake alias '{alias}' (letters, digits and _)"
        ));
    }

    let mut attach_options: Vec<String> = Vec::new();
    if let Some(data_path) = option(info, "data_path") {
        attach_options.push(format!("DATA_PATH {}", sql_literal(data_path)));
    }
    let read_only = option(info, "read_only")
        .or_else(|| option(info, "readonly"))
        .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on"));
    if read_only {
        attach_options.push("READ_ONLY".to_string());
    }
    let suffix = if attach_options.is_empty() {
        String::new()
    } else {
        format!(" ({})", attach_options.join(", "))
    };
    Ok(vec![
        format!(
            "ATTACH {} AS {alias}{suffix};",
            sql_literal(&format!("ducklake:{catalog}"))
        ),
        format!("USE {alias};"),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn info(options: &[(&str, &str)]) -> ConnectionInfo {
        ConnectionInfo {
            options: options
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn attaches_a_catalog_with_its_data_path() {
        let statements = attach_statements(
            &info(&[("data_path", "/srv/lake/data/"), ("alias", "sales")]),
            "sqlite:/srv/lake/meta.sqlite",
        )
        .unwrap();
        assert_eq!(
            statements,
            vec![
                "ATTACH 'ducklake:sqlite:/srv/lake/meta.sqlite' AS sales (DATA_PATH '/srv/lake/data/');",
                "USE sales;"
            ]
        );
    }

    #[test]
    fn the_default_alias_is_lake_and_quotes_are_escaped() {
        let statements =
            attach_statements(&info(&[("data_path", "/o'brien/")]), "meta.ducklake").unwrap();
        assert_eq!(
            statements[0],
            "ATTACH 'ducklake:meta.ducklake' AS lake (DATA_PATH '/o''brien/');"
        );
    }

    #[test]
    fn read_only_is_passed_on_to_the_attach() {
        let statements =
            attach_statements(&info(&[("read_only", "true")]), "meta.ducklake").unwrap();
        assert_eq!(
            statements[0],
            "ATTACH 'ducklake:meta.ducklake' AS lake (READ_ONLY);"
        );
    }

    #[rstest]
    #[case("", "needs the metadata catalog")]
    #[case(
        "postgres:dbname=x host=y password=hunter2",
        "must not carry a password"
    )]
    #[case("postgres:dbname=x PASSWD=hunter2", "must not carry a password")]
    fn bad_catalogs_are_refused(#[case] catalog: &str, #[case] expected: &str) {
        let error = attach_statements(&info(&[]), catalog)
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("hunter2"));
    }

    #[rstest]
    #[case("la ke")]
    #[case("1lake")]
    #[case("lake;DROP TABLE x")]
    fn bad_aliases_are_refused(#[case] alias: &str) {
        assert!(attach_statements(&info(&[("alias", alias)]), "meta.ducklake").is_err());
    }

    #[rstest]
    #[case("sqlite:/x.sqlite", Some("sqlite"))]
    #[case("postgres:dbname=x", Some("postgres"))]
    #[case("/x/meta.ducklake", None)]
    fn catalog_extensions(#[case] catalog: &str, #[case] expected: Option<&str>) {
        assert_eq!(catalog_extension(catalog), expected);
    }
}
