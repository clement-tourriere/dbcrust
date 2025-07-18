# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

DBCrust is a high-performance PostgreSQL interactive client written in Rust with Python bindings. It features advanced CLI capabilities including autocomplete, SSH tunneling, HashiCorp Vault integration, and rich output formatting.

## Build System & Development Commands

### Core Build Commands
```bash
# Build the project (development)
cargo build

# Build optimized release version
cargo build --release

# Run the CLI directly
cargo run --release -- [CONNECTION_OPTIONS]

# Run with connection URL
cargo run --release -- postgresql://user@host/database

# Run specific tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Install from source
cargo install --path .
```

### Python Integration
```bash
# Build Python package
pip install -e ./python

# Build with maturin (development)
maturin develop

# Build Python wheel
maturin build --release
```

### Development Tools
- Uses `mise.toml` for tool management (Node.js, Python tools)
- Pre-commit hooks via `pipx:pre-commit`
- Commitizen for conventional commits

## Architecture Overview

### Core Modules

- **`src/main.rs`**: Application entry point with Tokio runtime and CLI orchestration
- **`src/lib.rs`**: Public API and Python bindings (`PyDatabase`, `PyConfig`)
- **`src/db.rs`**: Database operations layer using SQLx with async PostgreSQL operations
- **`src/cli.rs`**: Command-line argument parsing with Clap, supports multiple connection methods
- **`src/config.rs`**: TOML-based configuration system with session management
- **`src/prompt.rs`**: Interactive CLI using reedline with custom prompts
- **`src/completion.rs`**: SQL autocomplete with table/column metadata caching
- **`src/format.rs`**: Output formatting (table, expanded, psql-compatible modes)

### Advanced Features

- **`src/ssh_tunnel.rs`**: SSH tunnel management for secure database connections
- **`src/vault_client.rs`**: HashiCorp Vault integration for dynamic credentials
- **`src/named_queries.rs`**: Parameterized query storage with `$1`, `$*`, `$@` substitution
- **`src/script.rs`**: External editor integration for multiline SQL editing
- **`src/pgpass.rs`**: `.pgpass` file integration for password management

## Connection Methods

The client supports multiple connection approaches:
1. Connection URLs: `postgresql://user:pass@host:port/db?sslmode=require`
2. Individual parameters: `-H host -p port -U user -d database`
3. Vault URLs: `vaultdb://role@mount/database`
4. SSH tunnel patterns in config for automatic tunnel usage

## Key Development Patterns

### Database Operations
- All database operations are async using SQLx
- Use `Database` struct methods for new database functionality
- Handle PostgreSQL-specific types (JSON, arrays, etc.) in formatting layer
- Implement graceful error handling with user-friendly messages

### Configuration Management
- Configuration uses serde with TOML format
- Layered config: defaults → file → CLI args
- Add new fields with `#[serde(default)]` for backward compatibility
- Store persistent state (sessions, named queries) in config

### CLI Commands
- Backslash commands (`\dt`, `\l`, etc.) handled in main loop
- Add new commands to `BACKSLASH_COMMANDS` in completion.rs
- Update help text in `print_help()` function
- Follow PostgreSQL psql conventions where applicable

### Python Bindings
- Use `Arc<TokioMutex<Database>>` for thread-safe async access
- PyO3 methods should handle runtime management properly
- Python client in `python/dbcrust/client.py` provides high-level interface
- Compile with `python` feature flag for bindings

### Testing Strategy
- Use `rstest` for parameterized tests (as specified in Cursor rules)
- Unit tests in individual modules
- Integration tests for CLI workflows
- Do not use `cargo run` for testing (per Cursor rules)
- Test database operations with mock or test database

### Code Style
- Modular design with single responsibility per module
- Use `thiserror` for custom error types
- Async/await throughout with proper error propagation
- Follow Rust naming conventions and use clippy for linting

## SSH Tunnel Configuration

Configure automatic SSH tunnels in `config.toml`:
```toml
[ssh_tunnel_patterns]
"^db\\.internal\\..*\\.com$" = "user@jumphost.example.com:2222"
```

## Vault Integration

Vault connections use the format:
- `vaultdb://role@mount/database`
- Components are optional and will prompt interactively
- Configure with environment variables or CLI args

## Important Notes

- Project uses Rust 2024 edition
- SQLx for async PostgreSQL operations
- Reedline for modern CLI experience
- Conditional compilation for Python bindings
- Configuration stored in `~/.config/dbcrust/config.toml`
- Debug logs accessible via `--show-debug-logs` flag