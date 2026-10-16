# DBCrust stand-in for `zip` 6.x

`libduckdb-sys` (the C++ engine behind the `duckdb` feature, every release from
1.4.3 through 1.10506.0) declares `zip = "^6"` as a **build dependency**. Every
`zip` release from 5.0.0 through 9.0.2 has been **yanked** on crates.io (only
9.0.3 is live), so a fresh `Cargo.lock` cannot resolve it, and a lock entry for
the yanked release is rejected by `.github/scripts/check_cargo_cooldown.py`
(no exemptions) and flagged by `cargo audit`.

The build script only uses the crate in its **non-bundled** mode
(`DUCKDB_DOWNLOAD_LIB=1`): `zip::ZipArchive::new(file)?.extract(dir)?` unpacks a
prebuilt `libduckdb` it just downloaded. DBCrust always builds DuckDB from
source (`bundled`), so that code is compiled but never runs.

This directory is therefore a ~40-line crate named `zip` at version `6.0.0`,
wired in with `[patch.crates-io] zip = { path = "third_party/zip-stub" }`. It
offers `ZipArchive::new`/`extract` and fails with an explanatory error. It is
excluded from the workspace (`exclude` in the root manifest) and from the
formatting/lint hooks (`hk.pkl` skips `third_party/**`).

Drop it once `duckdb-rs` stops depending on a yanked `zip` (a bump to a live
major, or removing the download mode): delete the `[patch.crates-io]` entry,
this directory and the `exclude` item, and re-run
`python3 .github/scripts/check_cargo_cooldown.py --base HEAD`.
