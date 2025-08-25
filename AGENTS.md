# AGENTS.md - Development Guide for DBCrust

This file provides guidance for agentic coding tools working with the DBCrust codebase.

## Build/Lint/Test Commands

### Core Build Commands
```bash
# Build the project (development)
cargo build

# Build optimized release version
cargo build --release

# Build Python package (development)
maturin develop

# Build Python wheel
maturin build --release

# Install Python package
pip install -e ./python
```

### Testing Commands
```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run specific test
cargo test test_name

# Run tests for specific module
cargo test --lib module_name

# Run integration tests only
cargo test --test "*"

# Run Python interface tests
python -m pytest python/tests/
```

### Linting and Formatting
```bash
# Format code
cargo fmt

# Lint with clippy (critical issues only)
cargo clippy --lib --bins -- -D "clippy::correctness" -D "clippy::suspicious" -D "clippy::perf" -W "clippy::style" -W "clippy::complexity"

# Pre-commit hooks
pre-commit run --all-files
```

## Code Style Guidelines

### General Principles
- **Modular Design**: Single responsibility per module
- **Async/Await**: Use throughout for database operations
- **Error Handling**: Use `thiserror` for custom error types with user-friendly messages
- **User Interaction**: MANDATORY - Use `inquire` crate for ALL user prompts (Select, Confirm, Text, MultiSelect)
- **NEVER** use manual stdin/stdout prompting or `println!` for interactive input

### Naming Conventions
- **Structs/Enums**: PascalCase (e.g., `Database`, `Command`, `Config`)
- **Functions/Methods**: snake_case (e.g., `connect_to_database`, `parse_command`)
- **Variables**: snake_case (e.g., `connection_url`, `config_path`)
- **Constants**: SCREAMING_SNAKE_CASE (e.g., `DEFAULT_TIMEOUT`)
- **Modules**: snake_case (e.g., `database`, `commands`, `config`)

### Imports and Dependencies
- Group imports: std, external crates, local modules
- Use explicit imports, avoid glob imports (`use::*`)
- Keep dependency features minimal and explicit
- **CRITICAL**: Never remove the `strum` crate dependency (essential for enum iteration)

### Type Safety and Enums
- **Enum-Based Commands**: Use `Command` enum with strum for automatic iteration
- **NEVER** use hardcoded Vec/arrays for command lists - always derive from enums
- Use `#[derive(Debug, Clone, PartialEq, EnumIter)]` for command enums
- Implement consistent patterns: `command()`, `description()`, `category()` methods

### Configuration Management
- Use `serde` with `#[serde(default)]` for backward compatibility
- **Dedicated Storage Pattern**: Separate user data from app settings
  - Main config: `~/.config/dbcrust/config.toml` (settings only)
  - Sessions: `~/.config/dbcrust/saved_sessions.toml`
  - Recent connections: `~/.config/dbcrust/recent_connections.toml`
- Update `save_with_documentation()` method when adding new config fields

### Testing Patterns
- Use `rstest` for parameterized tests
- Test isolation: Use temporary directories (`/tmp/dbcrust_test_{pid}/`)
- Unit tests in individual modules, integration tests in `tests/` directory
- Test database operations with mock/test databases
- **NEVER** use `cargo run` for testing

### Async Patterns
- Use `Arc<TokioMutex<T>>` for thread-safe shared state
- Proper error propagation with `?` operator
- Handle PostgreSQL-specific types in formatting layer
- Graceful error handling with user-friendly messages

### Security and Best Practices
- Never expose or log secrets/passwords
- Use proper encryption for sensitive data storage
- Validate all user inputs
- Follow principle of least privilege
- Use `tracing` for structured logging with appropriate levels

### Documentation
- Add rustdoc comments for all public APIs
- Update CLAUDE.md for new features and usage examples
- Update `docs/` directory for user-facing changes
- Include configuration examples and CLI usage patterns

## Development Workflow

### Feature Implementation Pattern
1. **Planning**: Define requirements, identify modules, design data structures
2. **Implementation**: Core logic → CLI integration → backslash commands
3. **Testing**: Unit tests → integration tests → edge cases
4. **Documentation**: Code docs → user docs → examples
5. **Validation**: `cargo test` → `cargo build` → `cargo clippy`

### Critical Patterns to Follow
- **Enum/Traits for Lists**: Never use hardcoded arrays for commands/categories
- **Inquire for Interaction**: All user prompts must use `inquire` crate
- **Dedicated Storage**: Separate concerns with dedicated files
- **Test Isolation**: Use temporary directories, never pollute real config
- **Backward Compatibility**: Use `#[serde(default)]` for config changes

### Validation Checklist
- [ ] All tests pass: `cargo test`
- [ ] Code compiles: `cargo build`
- [ ] Linting passes: `cargo clippy`
- [ ] Documentation updated
- [ ] Error messages user-friendly
- [ ] Performance impact acceptable
- [ ] Backward compatibility maintained
