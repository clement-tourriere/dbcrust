# DBCrust patch to `tiberius` 0.13.0

This directory is a vendored copy of [`tiberius`](https://github.com/prisma/tiberius)
(TDS / SQL Server client, MIT OR Apache-2.0, by the Prisma team and contributors)
with **two changes: rustls uses `ring` instead of `aws-lc-rs`, and the crate
builds on 32-bit targets** (DBCrust ships i686 Python wheels). It is a path
dependency of `crates/driver-mssql`; it is excluded from the workspace
(`exclude = ["third_party/tiberius"]`) and from the formatting/lint hooks (`hk.pkl`).

| | |
|---|---|
| Upstream version | `tiberius 0.13.0` (published 2026-09-25, not yanked) |
| Original archive | `https://static.crates.io/crates/tiberius/tiberius-0.13.0.crate` |
| sha256 of the original `.crate` | `e07324791de2bdaed058af4aa433a0b5ec5c0beac407a9797419c19805df2415` (identical to the `cksum` of the 0.13.0 line in `https://index.crates.io/ti/be/tiberius` and to `checksum` in the crates.io API) |
| Upstream commit | `7fd75bb7070fe6ea41374698abf78928bb983bda` (from the archive's `.cargo_vcs_info.json`: `"dirty": true`, `"path_in_vcs": ""`; the file itself is not kept, see below) |
| License files | `LICENSE-MIT.txt`, `LICENSE-APACHE.txt` (unchanged) |

## Why

DBCrust uses **ring as its single rustls crypto provider** (installed once by
`dbcrust_core::tls`) and keeps `aws-lc-rs`/`aws-lc-sys` (a cmake C build) and
OpenSSL out of the dependency tree. Upstream `tiberius 0.13.0`:

- hard-codes `rustls::crypto::aws_lc_rs` as the fallback provider
  (`src/client/tls_stream/rustls_tls_stream.rs`), and
- declares `tokio-rustls = "0.26.4"` with its **default features**, which include
  `aws_lc_rs`, so aws-lc is compiled and linked whatever the caller does.

`tiberius 0.12.3` is ring-based but sits on rustls 0.21 / rustls-webpki 0.101 /
rustls-pemfile 1.0 and trips RUSTSEC-2026-0104, -0098 and -0099 (plus an
unmaintained notice), which the dependency policy forbids.

At runtime a process-installed provider is already honoured
(`CryptoProvider::get_default()`); the patch only removes the compile-time
aws-lc dependency and makes the fallback provider ring.

## The patch (everything that differs from the published crate)

1. `Cargo.toml`: `tokio-rustls` without default features, with `ring`.

```diff
 [dependencies.tokio-rustls]
 version = "0.26.4"
 optional = true
+default-features = false
+features = [
+    "ring",
+    "tls12",
+    "logging",
+]
```

2. `src/client/tls_stream/rustls_tls_stream.rs`: the fallback provider (and the
   two unit tests that build one) come from `rustls::crypto::ring`. The module
   alias keeps the diff to one line; the "aws-lc-rs" wording in a log message and
   test names is now historical.

```diff
--- a/src/client/tls_stream/rustls_tls_stream.rs
+++ b/src/client/tls_stream/rustls_tls_stream.rs
@@ -21,7 +21,7 @@
             danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
             WantsClientCert,
         },
-        crypto::{aws_lc_rs, CryptoProvider},
+        crypto::{ring as aws_lc_rs, CryptoProvider},
         pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer, ServerName, UnixTime},
         ClientConfig, ConfigBuilder, DigitallySignedStruct, Error as RustlsError, RootCertStore,
         SignatureScheme,
```

3. `src/tds/codec/type_info.rs`: the XML column size (the PLP "unknown length"
   marker) is written `usize::MAX - 1` instead of the literal
   `0xfffffffffffffffe_usize`, which overflows `usize` on 32-bit targets and is a
   hard error there (`overflowing_literals`), so upstream 0.13.0 does not compile
   for `i686`. On 64-bit the value is the same; on 32-bit it is `0xfffffffe`.
   The size only feeds `plp::decode`, which treats anything `>= 0xffff` as a PLP
   stream, so XML decoding is unchanged. One line in the decoder and two in its
   `round_trip` test:

```diff
--- a/src/tds/codec/type_info.rs
+++ b/src/tds/codec/type_info.rs
@@ -372,7 +372,7 @@
                 Ok(TypeInfo::Xml {
                     schema,
-                    size: 0xfffffffffffffffe_usize,
+                    size: usize::MAX - 1,
                 })
```

4. Packaging only (no behaviour change): the `[[example]]`, `[[test]]` and
   `[dev-dependencies.*]` tables are removed from `Cargo.toml` because their
   sources are not vendored. The following upstream files are **not** copied:
   `tests/`, `examples/`, `docs/`, `docker/` (it contains a test TLS private key),
   `Cargo.lock`, `Cargo.toml.orig`, CI/Nix/editor config, and `.cargo_vcs_info.json`:
   cargo reserves that name for the packages it generates and refuses to
   package a crate that contains one, which broke `maturin sdist` (the PyPI
   source distribution packages every path dependency); its content is in the
   table above. Only `src/`, the two license files, `README.md`, `CHANGELOG.md`
   and the manifest are kept.

DBCrust enables only: `rustls`, `tds73`, `tds80` (default), `sql-browser-tokio`,
`chrono`, `rust_decimal`. `native-tls`, `winauth`, `sspi-rs` and
`integrated-auth-gssapi` stay off (see `crates/driver-mssql/Cargo.toml`).

## Re-vendoring on a tiberius release

1. Download `tiberius-<new>.crate` from `static.crates.io` into an empty temp
   directory and verify its sha256 against the `cksum` field of
   `https://index.crates.io/ti/be/tiberius`; check the version is not yanked and
   at least 24 hours old (crates.io API, generic `User-Agent`).
2. Extract it, replace `src/`, `Cargo.toml`, the license files, `README.md` and
   `CHANGELOG.md` (record the commit from `.cargo_vcs_info.json` in the table,
   do not copy the file); re-apply the four changes above.
   If upstream added a ring option by then (see below), drop the patch and the
   whole `third_party/tiberius` directory and depend on the registry crate.
3. Update the version and sha256 in this file and the path dependency's version
   in `[workspace.dependencies]`; run
   `python3 .github/scripts/check_cargo_cooldown.py --base HEAD`, then
   `cargo audit` and `cargo tree --locked -i aws-lc-rs` (must report nothing).

## Upstream contribution (ready to send)

**Title:** Add a `rustls-ring` feature (do not hard-code aws-lc-rs)

**Body:**

> Hi, and thanks for tiberius.
>
> With the `rustls` feature enabled, 0.13.0 always compiles and links `aws-lc-rs`
> / `aws-lc-sys`, even when the application installs its own rustls
> `CryptoProvider`:
>
> - `rustls_tls_stream.rs` uses `rustls::crypto::aws_lc_rs::default_provider()` as
>   the fallback when no provider is installed, and
> - `tokio-rustls = "0.26.4"` is declared with default features, which enable its
>   `aws_lc_rs` feature.
>
> For applications that standardise on ring (no cmake / C toolchain, smaller
> binaries, cross-compilation to musl/Windows/ARM), this cannot be turned off
> from the outside. A clean release build of a minimal client goes from 14 s /
> 53 s CPU and +1.24 MB (ring) to 27 s / 102 s CPU and +2.01 MB (aws-lc).
>
> Proposal (no behaviour change for existing users):
>
> ```toml
> tokio-rustls = { version = "0.26.4", optional = true, default-features = false, features = ["tls12", "logging"] }
>
> [features]
> rustls = ["tokio-rustls", "tokio-util", "rustls-native-certs", "rustls-webpki", "rustls-pki-types", "rustls-aws-lc"]  # keep today's default
> rustls-aws-lc = ["tokio-rustls/aws_lc_rs"]
> rustls-ring = ["tokio-rustls/ring"]
> ```
>
> and, in `resolve_crypto_provider`, choose the fallback with
> `#[cfg(feature = "rustls-ring")] rustls::crypto::ring::default_provider()`
> (falling back to aws-lc otherwise). A process-installed default provider is
> already honoured, so nothing else changes. I have been running exactly this
> (a four-line version: `crypto::{ring as aws_lc_rs, ...}` plus the
> `tokio-rustls` feature change) against SQL Server 2022 and can turn it into a
> PR with the feature plumbing and a CI job that builds `--no-default-features
> --features rustls,rustls-ring` and checks `cargo tree -i aws-lc-rs` is empty.
>
> Related: staying on 0.12.3 is not an option for us because it pins rustls 0.21
> (RUSTSEC-2026-0104, -0098, -0099).

A second, independent report (32-bit build):

**Title:** 0.13.0 does not compile for 32-bit targets (`overflowing_literals`)

**Body:**

> `src/tds/codec/type_info.rs` sets the XML column size to
> `0xfffffffffffffffe_usize`, which does not fit a 32-bit `usize`, so
> `cargo check --target i686-unknown-linux-gnu` fails with
> `error: literal out of range for usize`. `usize::MAX - 1` is the same value on
> 64-bit and still `>= 0xffff` (the PLP path of `plp::decode`) on 32-bit; happy
> to send it as a PR.
