# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.3.0 (2025-07-19)

## [0.2.0] - 2024-01-19

### Added
- Python/PyPI release support via maturin
- `uvx` and `uv tool install` support for easy installation
- Comprehensive test coverage for backslash commands
- SSH tunnel optimization for faster connections
- Vault URL parsing improvements

### Fixed
- Python bindings CLI compatibility
- SSH tunnel connection delays
- Version mismatch in CI/CD pipeline
- Compilation warnings

### Changed
- Simplified CLI argument structure
- Improved SSH tunnel establishment process
- Updated README with modern installation instructions

## [0.1.0] - 2024-01-01

### Added
- Initial release
- Multi-database support (PostgreSQL, MySQL, SQLite)
- Smart autocompletion
- SSH tunneling
- HashiCorp Vault integration
- Docker container support
- Python API

[Unreleased]: https://github.com/dbcrust/dbcrust/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/dbcrust/dbcrust/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/dbcrust/dbcrust/releases/tag/v0.1.0