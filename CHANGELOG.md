## v0.21.2 (2025-09-02)

### Fix

- elasticsearch index autocompletion

## v0.21.1 (2025-09-01)

### Fix

- handle postgres enum

## v0.21.0 (2025-08-27)

### Feat

- allow dbcrust to securely ask and store passwords
- add elasticsearch connection

## v0.20.1 (2025-08-26)

### Fix

- replug docker pipe

## v0.20.0 (2025-08-26)

### Feat

- handle complex fieds for multiple databases

### Fix

- missing postgres extensions support like pgvector or postgis

## v0.19.0 (2025-08-25)

### Feat

- add mongodb connector

## v0.18.0 (2025-08-19)

### Feat

- add clickhouse connection

## v0.17.0 (2025-08-08)

### Feat

- python cursor queries

## v0.16.1 (2025-08-07)

## v0.16.0 (2025-08-07)

### Feat

- better django analyzer and add a django middleware

## v0.15.1 (2025-08-05)

### Fix

- python packagin and remove the unused CLI arguments

## v0.15.0 (2025-08-05)

### Feat

- rework named queries to use their own config file and be scoped

## v0.14.1 (2025-08-05)

### Fix

- small auto completion fixes

## v0.14.0 (2025-08-05)

### Feat

- allow column auto completion thanks to full line context aware completion

## v0.13.2 (2025-08-05)

### Fix

- completion for \ef
- replug the \ecopy

## v0.13.1 (2025-08-05)

### Fix

- remove unused import warning in config

## v0.13.0 (2025-08-05)

### Feat

- better completion system with db engine awareness

### Fix

- fix almost all failing tests related to last architecture changes

## v0.12.3 (2025-08-01)

## v0.12.2 (2025-08-01)

### Fix

- automatic tunnel

## v0.12.1 (2025-08-01)

### Fix

- column selection

## v0.12.0 (2025-07-31)

### Feat

- add Shift+Tab navigation for autocomplete suggestions (#3)

### Fix

- autocomplete table names

## v0.11.5 (2025-07-29)

## v0.11.4 (2025-07-28)

### Fix

- explain on vault connection

## v0.11.3 (2025-07-28)

### Fix

- autocompletion for non-postgres databases

## v0.11.2 (2025-07-28)

## v0.11.1 (2025-07-28)

### Fix

- better and faster autocompletion system

## v0.11.0 (2025-07-28)

### Feat

- huge refactoring of pool connections + cache + stats

## v0.10.5 (2025-07-28)

### Fix

- dbcrust django manage with special chars in db name

## v0.10.4 (2025-07-25)

### Fix

- problematic postgres URL

## v0.10.3 (2025-07-25)

### Fix

- switch dbs with \c

## v0.10.2 (2025-07-24)

### Fix

- remove vault creds file in case of decryption error

## v0.10.1 (2025-07-23)

## v0.10.0 (2025-07-23)

### Feat

- add django queries analyzer

## v0.19.0 (2025-08-25)

### Feat

- MongoDB connector with full SQL-to-MongoDB query translation
- MongoDB Docker container discovery and automatic connection
- Advanced MongoDB filtering support (LIKE, IN, OR, BETWEEN, NULL checks)
- MongoDB database and collection management commands
- MongoDB text search capabilities
- Comprehensive MongoDB documentation suite

### Docs

- Add AGENTS.md for development workflow guidance
- Complete MongoDB user guide with examples and best practices

## v0.18.0 (2025-08-19)

### Feat

- add clickhouse connection

### Docs

- add clickhouse in the documentation

## v0.17.0 (2025-08-08)

### Feat

- python cursor queries

### Docs

- update the documentation again to be more clear

## v0.16.1 (2025-08-07)

### Docs

- huge documentation cleaning and improvement

### Fix

- add clippy and format to codebase

## v0.16.0 (2025-08-07)

### Feat

- better django analyzer and add a django middleware

## v0.15.1 (2025-08-05)

### Fix

- python packagin and remove the unused CLI arguments

## v0.15.0 (2025-08-05)

### Feat

- rework named queries to use their own config file and be scoped

## v0.14.1 (2025-08-05)

### Fix

- small auto completion fixes

## v0.14.0 (2025-08-05)

### Feat

- allow column auto completion thanks to full line context aware completion

## v0.13.2 (2025-08-05)

### Fix

- completion for \ef
- replug the \ecopy

## v0.13.1 (2025-08-05)

### Fix

- remove unused import warning in config

## v0.13.0 (2025-08-05)

### Feat

- better completion system with db engine awareness

### Fix

- fix almost all failing tests related to last architecture changes

## v0.12.3 (2025-08-01)

## v0.12.2 (2025-08-01)

### Fix

- automatic tunnel

## v0.12.1 (2025-08-01)

### Fix

- column selection

## v0.12.0 (2025-07-31)

### Feat

- add Shift+Tab navigation for autocomplete suggestions (#3)

### Fix

- autocomplete table names

## v0.11.5 (2025-07-29)

## v0.11.4 (2025-07-28)

### Fix

- explain on vault connection

## v0.11.3 (2025-07-28)

### Fix

- autocompletion for non-postgres databases

## v0.11.2 (2025-07-28)

## v0.11.1 (2025-07-28)

### Fix

- better and faster autocompletion system

## v0.11.0 (2025-07-28)

### Feat

- huge refactoring of pool connections + cache + stats

## v0.10.5 (2025-07-28)

### Fix

- dbcrust django manage with special chars in db name

## v0.10.4 (2025-07-25)

### Fix

- problematic postgres URL

## v0.10.3 (2025-07-25)

### Fix

- switch dbs with \c

## v0.10.2 (2025-07-24)

### Fix

- remove vault creds file in case of decryption error

## v0.10.1 (2025-07-23)

## v0.10.0 (2025-07-23)

### Feat

- add django queries analyzer

## v0.9.0 (2025-07-23)

### Feat

- store vault credentials

## v0.8.1 (2025-07-23)

### Fix

- replug external editor

## v0.8.0 (2025-07-22)

### Feat

- replug column selection

## v0.7.4 (2025-07-22)

### Fix

- add a run_with_url method to not mix with python CLI args

## v0.7.3 (2025-07-22)

### Fix

- add recent connections for vault

## v0.7.2 (2025-07-22)

### Fix

- completion installation from python

## v0.7.1 (2025-07-22)

### Fix

- shell autocompletion

## v0.7.0 (2025-07-22)

### Feat

- smart autocompletion

### Fix

- better handling of sqlite paths
- completion for named queries
- replug \d table

## v0.6.2 (2025-07-21)

### Fix

- saved sessions
- vault connection
- don't pollute config files while testing
- recent connection with missing password

## v0.6.1 (2025-07-21)

### Fix

- add missing vault:// scheme in python

## v0.6.0 (2025-07-21)

### Feat

- unify Rust and Python CLI

### Fix

- improve doc building time

## v0.5.0 (2025-07-20)

### Feat

- rework recent and session features

## v0.4.1 (2025-07-20)

### Fix

- github address
- documentation
- docs CI
- docs CI
- critical workflow issues

## v0.4.0 (2025-07-19)

## v0.2.5 (2025-07-19)

### Fix

- commitizen versioning
- update Cargo.lock with cz
- python version

## v0.2.4 (2025-07-19)

### Fix

- fix project link + name

## v0.2.3 (2025-07-19)

### Fix

- pyproject

## v0.2.2 (2025-07-19)

### Fix

- remove duplicated pypi-release

## v0.2.1 (2025-07-19)

### Fix

- pypi metadata and release trusted user

## v0.2.0 (2025-07-19)

### Fix

- consolidate release workflows and fix OIDC PyPI publishing

## v0.3.0 (2025-07-19)

### Feat

- dbcrust first commit for public GitHub

### Fix

- maturin build
- publish to pypi
