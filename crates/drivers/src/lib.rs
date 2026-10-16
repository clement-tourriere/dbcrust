//! Driver registry: the one place that knows which engine is served by which
//! client, and which of those connectors this build contains.
//!
//! Everything above (`Database`, the broker gateway) asks
//! [`create_database_client`] for a `Box<dyn DatabaseClient>` and never names a
//! driver type. Each row of [`connectors!`] below carries the cargo feature that
//! provides its client; an engine whose feature is off fails with
//! [`DatabaseError::ConnectorNotBuilt`], which names that feature.
//!
//! # Adding a connector
//!
//! A new engine is a `dbcrust-driver-*` crate, an optional dependency plus a
//! same-named feature in this crate's manifest, one row in [`connectors!`], a
//! root-crate feature forwarding to it, and a decision whether the root's
//! `bundled` preset includes it (see "Cargo features" in `CLAUDE.md`).

pub use dbcrust_core::database::{ConnectionInfo, DatabaseClient, DatabaseError, DatabaseType};

/// Declares the registry from one table: for each connector its cargo feature,
/// the engines its client serves and the client type, one row per driver crate.
///
/// The `match`es below have no wildcard, so a [`DatabaseType`] variant missing
/// from the table is a compile error.
macro_rules! connectors {
    ($( $feature:literal => $($variant:ident)|+ => $client:path; )*) => {
        /// Create the client for `connection_info.database_type`.
        ///
        /// Fails with [`DatabaseError::ConnectorNotBuilt`] when the engine's
        /// connector is not part of this build (see [`is_compiled`]), and
        /// otherwise with whatever the client's constructor reports
        /// (connection, authentication, …).
        pub async fn create_database_client(
            connection_info: ConnectionInfo,
        ) -> Result<Box<dyn DatabaseClient>, DatabaseError> {
            match connection_info.database_type.clone() {
                $(
                    #[cfg(feature = $feature)]
                    $(DatabaseType::$variant)|+ => {
                        let client = <$client>::new(connection_info).await?;
                        Ok(Box::new(client))
                    }
                )*
                // Every engine of a build with all features on has an arm above
                #[allow(unreachable_patterns)]
                engine => Err(DatabaseError::ConnectorNotBuilt {
                    feature: required_feature(&engine),
                    engine,
                }),
            }
        }

        /// The cargo feature that provides `engine`'s connector.
        pub fn required_feature(engine: &DatabaseType) -> &'static str {
            match engine {
                $( $(DatabaseType::$variant)|+ => $feature, )*
            }
        }

        /// Whether `engine`'s connector is part of this build.
        pub fn is_compiled(engine: &DatabaseType) -> bool {
            match engine {
                $( $(DatabaseType::$variant)|+ => cfg!(feature = $feature), )*
            }
        }
    };
}

// feature => engine(s) => client type. File formats are all served by
// DataFusion (DuckDB files included until DuckDB gets a connector of its own).
connectors! {
    "files" => Parquet | CSV | JSON | DuckDB => dbcrust_driver_files::DataFusionClient;
    "mongodb" => MongoDB => dbcrust_driver_mongodb::MongoDBClient;
    "elasticsearch" => Elasticsearch => dbcrust_driver_elasticsearch::ElasticsearchClient;
    "clickhouse" => ClickHouse => dbcrust_driver_clickhouse::ClickHouseClient;
    "mysql" => MySQL => dbcrust_driver_mysql::MySqlClient;
    "postgres" => PostgreSQL => dbcrust_driver_postgres::PostgreSQLClient;
    "sqlite" => SQLite => dbcrust_driver_sqlite::SqliteClient;
    "white-dragon" => WhiteDragon => dbcrust_driver_white_dragon::WhiteDragonClient;
}

/// The engines whose connector is part of this build, in `DatabaseType` order.
pub fn compiled_engines() -> Vec<DatabaseType> {
    DatabaseType::supported_types()
        .into_iter()
        .filter(is_compiled)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cargo features of this crate that turn a connector on, as the build
    /// sees them.
    fn enabled_features() -> Vec<&'static str> {
        let mut features = Vec::new();
        macro_rules! probe {
            ($($feature:literal),*) => {
                $( if cfg!(feature = $feature) { features.push($feature); } )*
            };
        }
        probe!(
            "postgres",
            "mysql",
            "sqlite",
            "mongodb",
            "clickhouse",
            "elasticsearch",
            "white-dragon",
            "files"
        );
        features
    }

    #[test]
    fn engines_name_the_feature_that_provides_them() {
        assert_eq!(required_feature(&DatabaseType::PostgreSQL), "postgres");
        assert_eq!(required_feature(&DatabaseType::WhiteDragon), "white-dragon");
        for engine in [
            DatabaseType::Parquet,
            DatabaseType::CSV,
            DatabaseType::JSON,
            DatabaseType::DuckDB,
        ] {
            assert_eq!(required_feature(&engine), "files");
        }
    }

    #[test]
    fn every_engine_maps_to_a_feature_of_this_crate() {
        let known = [
            "postgres",
            "mysql",
            "sqlite",
            "mongodb",
            "clickhouse",
            "elasticsearch",
            "white-dragon",
            "files",
        ];
        for engine in DatabaseType::supported_types() {
            assert!(
                known.contains(&required_feature(&engine)),
                "{engine:?} maps to an unknown feature"
            );
        }
    }

    #[test]
    fn is_compiled_matches_the_enabled_features() {
        let enabled = enabled_features();
        for engine in DatabaseType::supported_types() {
            assert_eq!(
                is_compiled(&engine),
                enabled.contains(&required_feature(&engine)),
                "{engine:?}"
            );
        }
    }

    #[test]
    fn compiled_engines_agree_with_is_compiled() {
        let compiled = compiled_engines();
        for engine in DatabaseType::supported_types() {
            assert_eq!(compiled.contains(&engine), is_compiled(&engine));
        }
    }

    #[cfg(all(
        feature = "postgres",
        feature = "mysql",
        feature = "sqlite",
        feature = "mongodb",
        feature = "clickhouse",
        feature = "elasticsearch",
        feature = "white-dragon",
        feature = "files"
    ))]
    #[test]
    fn a_full_build_has_every_engine() {
        assert_eq!(compiled_engines(), DatabaseType::supported_types());
    }

    #[cfg(not(feature = "mongodb"))]
    #[tokio::test]
    async fn a_disabled_engine_reports_the_feature_that_provides_it() {
        let info = ConnectionInfo {
            database_type: DatabaseType::MongoDB,
            ..Default::default()
        };
        let Err(error) = create_database_client(info).await else {
            panic!("MongoDB must not connect in a build without the feature");
        };
        assert!(
            matches!(
                error,
                DatabaseError::ConnectorNotBuilt {
                    engine: DatabaseType::MongoDB,
                    feature: "mongodb"
                }
            ),
            "{error:?}"
        );
        assert_eq!(
            error.to_string(),
            "MongoDB support is not compiled into this build (enable the `mongodb` cargo feature, or install a full build)"
        );
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn the_factory_builds_a_sqlite_client() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("factory.db");
        std::fs::File::create(&path).unwrap(); // an empty file is an empty database
        let info = ConnectionInfo {
            database_type: DatabaseType::SQLite,
            file_path: Some(path.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let client = create_database_client(info).await.expect("sqlite client");
        assert_eq!(
            client.get_connection_info().database_type,
            DatabaseType::SQLite
        );
    }
}
