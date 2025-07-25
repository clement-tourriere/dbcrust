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
cargo run --release -- postgres://user@host/database

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
- **Strum crate**: Essential for automatic enum iteration and synchronization - never remove this dependency

## Architecture Overview

### Core Modules

- **`src/main.rs`**: Application entry point with Tokio runtime and CLI orchestration
- **`src/lib.rs`**: Public API and Python bindings (`PyDatabase`, `PyConfig`)
- **`src/commands.rs`**: Type-safe command system using enum-based architecture with strum automation
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

### Display & User Interface Features

- **Interactive Column Selection**: Visual checkbox interface for selecting columns from large result sets
- **Intelligent Auto-Triggering**: Configurable threshold-based column selection activation
- **Session Persistence**: Column selections saved per table structure throughout the session
- **Ctrl-C Support**: Proper interrupt handling that returns to REPL without exiting

## Connection Methods

The client supports multiple connection approaches:
1. Connection URLs: `postgres://user:pass@host:port/db?sslmode=require`
2. Individual parameters: `-H host -p port -U user -d database`
3. Vault URLs: `vault://role@mount/database`
4. SSH tunnel patterns in config for automatic tunnel usage
5. Session URLs: `session://saved_session_name`
6. Recent URLs: `recent://` (interactive selection)
7. Docker URLs: `docker://container_name/database`

## Unified CLI Architecture

DBCrust implements a **single source of truth** architecture where the Rust and Python CLIs share identical functionality through PyO3 integration. This eliminates code duplication and ensures perfect feature parity.

### Architecture Principles

1. **Zero Duplication**: Python CLI calls Rust main logic directly, no separate implementations
2. **Shared Command System**: Both CLIs use the same enum-based command system for identical behavior
3. **Complete Feature Parity**: All connection types, commands, and features work identically
4. **Single Codebase**: New features automatically available in both CLIs

### Implementation Structure

```rust
// Main entry points (src/main.rs)
pub async fn async_main() -> Result<(), Box<dyn StdError>>           // Rust CLI entry
pub async fn async_main_with_args(args: Args) -> Result<(), Box<dyn StdError>>  // Shared logic

// Core functions exposed for Python (src/main.rs) 
pub async fn handle_database_connection(args: &Args) -> Result<(Database, Option<DockerConnectionInfo>), Box<dyn StdError>>
pub async fn run_interactive_mode(database: Database, args: &Args, config: &mut Config) -> Result<(), Box<dyn StdError>>

// PyO3 interface (src/lib.rs)
#[pyfunction]
pub fn run_cli_loop(args: Vec<String>) -> PyResult<i32>              // Python CLI entry
pub async fn run_main_cli_workflow(args: Args) -> Result<i32, Box<dyn StdError>>  // Unified workflow

// Type-safe command handling (src/commands.rs)
impl CommandExecutor for Command {
    async fn execute(&self, ...) -> Result<CommandResult, Box<dyn Error>>  // Shared by both CLIs
}
```

### Connection URL Handling

Both CLIs support identical connection URL patterns:

```bash
# Standard database URLs
dbcrust postgres://user@host:5432/db
dbcrust mysql://user@host:3306/db
dbcrust sqlite:///path/to/file.db

# Advanced connection types
dbcrust session://production_db          # Saved session
dbcrust recent://                         # Interactive recent selection
dbcrust docker://my-container/db         # Docker container
dbcrust vault://role@mount/database      # HashiCorp Vault

# All work identically in Python
python -m dbcrust session://production_db
python -m dbcrust recent://
```

### Command Line Feature Parity

All command-line flags and options work identically:

```bash
# Both CLIs support identical flags
dbcrust --debug --no-banner --ssh-tunnel user@host postgres://db
python -m dbcrust --debug --no-banner --ssh-tunnel user@host postgres://db

# Command mode works identically
dbcrust postgres://db -c "\\dt"        # List tables
python -m dbcrust postgres://db -c "\\dt"  # Identical behavior
```

### Type-Safe Command Integration

The enum-based command system provides 40+ commands shared between CLIs:

```rust
// Adding new commands (benefits both CLIs automatically)
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    ListTables,
    ListDatabases,
    ListSessions,
    NewCommand { args: String },  // Auto-available in both CLIs
    // ... 40+ commands
}

impl CommandExecutor for Command {
    async fn execute(&self, ...) -> Result<CommandResult, Box<dyn Error>> {
        match self {
            Command::ListTables => self.handle_list_tables(...).await,
            Command::ListDatabases => self.handle_list_databases(...).await,
            Command::ListSessions => self.handle_session_list(...).await,
            Command::NewCommand { args } => self.handle_new_command(args, ...).await,
        }
    }
}
```

### Development Workflow for Unified Features

When adding new features that affect the CLI:

1. **Implement in Rust**: Add core functionality to appropriate module
2. **Update Command enum**: Add new command variants to the enum
3. **Update CommandParser**: Add parsing logic for new commands
4. **Automatic Python Support**: Feature is automatically available in Python CLI
5. **Test Both CLIs**: Use `tests/python_cli_parity.rs` to verify identical behavior

### Testing the Unified Architecture

Comprehensive test coverage ensures feature parity:

```rust
// Feature parity testing (tests/python_cli_parity.rs)
#[rstest]
#[case("postgres://localhost/test")]
#[case("session://test_session")]
#[case("vault://role@mount/db")]
fn test_python_cli_connection_url_support(#[case] connection_url: &str) {
    // Verify Python CLI supports all connection URL types
}

// Command system testing (tests/command_system_tests.rs)
#[test]
fn test_command_enum_completeness() {
    let all_commands = Command::all_variants();
    assert!(all_commands.len() >= 40, "Should have 40+ commands");
    
    // Test that all commands can be parsed
    for cmd in all_commands {
        let parsed = CommandParser::parse(&cmd.to_string());
        assert!(parsed.is_ok(), "Command should parse correctly: {:?}", cmd);
    }
}
```

### Benefits of Unified Architecture

1. **Eliminated Code Duplication**: Single implementation for all features
2. **Guaranteed Feature Parity**: Impossible for CLIs to diverge
3. **Reduced Maintenance**: New features automatically work in both CLIs
4. **Consistent User Experience**: Identical behavior regardless of entry point
5. **Simplified Testing**: Test once, verify both CLIs work

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

#### Dedicated Storage Files Pattern
**ARCHITECTURAL DECISION**: For user data that represents separate concerns (sessions, connection history, credentials, etc.), use dedicated storage files rather than mixing everything in the main `config.toml`.

**Pattern to follow:**
- **Main config**: `~/.config/dbcrust/config.toml` - application settings and preferences only
- **Recent connections**: `~/.config/dbcrust/recent_connections.toml` - connection history
- **Saved sessions**: `~/.config/dbcrust/saved_sessions.toml` - named connection configurations  
- **Vault credentials**: `~/.config/dbcrust/vault_credentials.enc` - encrypted cached credentials
- **Future data types**: Follow this pattern with dedicated files

**Benefits:**
- **Separation of concerns**: Settings vs. user data are separate
- **Security**: Sensitive data can have different encryption/permissions
- **Performance**: Avoid loading large user data when only reading settings
- **Maintainability**: Easier to backup, migrate, or clear specific data types
- **Conflict prevention**: Reduces risk of concurrent write conflicts

**Implementation pattern:**
```rust
// In config.rs - storage structs are separate from main config
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct NewFeatureStorage {
    pub data: HashMap<String, NewFeatureData>,
}

// Main config only holds transient/settings data, not user data
pub struct Config {
    pub app_settings: bool,
    // User data stored separately, loaded on demand
    #[serde(skip)]
    new_feature_storage: NewFeatureStorage,
}
```

### Type-Safe Command System with Automatic Synchronization
- **Enum-Based Commands**: All backslash commands (`\dt`, `\l`, etc.) managed by `Command` enum in `src/commands.rs`
- **Strum-Powered Automation**: Uses `strum` derive macros (`EnumIter`, `Display`) for automatic code generation
- **Zero Synchronization Issues**: Command shortcuts, descriptions, categories automatically derived from enums
- **CommandShortcut Pattern**: Separate enum for shortcuts with automatic iteration via `CommandShortcut::iter()`
- **Perfect Feature Parity**: Rust and Python CLIs use identical command implementation
- **Automatic Autocomplete**: `get_command_names()` automatically includes ALL commands via enum iteration
- **Automatic Help Generation**: `get_commands_by_category()` automatically groups commands by category
- **Single Source of Truth**: Add new commands to `Command` enum and `CommandShortcut` enum - everything else is automatic

#### Critical Pattern: Always Use Enum/Traits for Lists
**NEVER use hardcoded Vec/arrays for command lists, categories, or mappings.** Always use:
```rust
// ✅ CORRECT - Automatic synchronization via strum
#[derive(Debug, Clone, PartialEq, EnumIter)]
pub enum CommandShortcut { Q, H, L, Dt, /* ... */ }

impl CommandShortcut {
    pub fn command(&self) -> &'static str { /* mapping */ }
    pub fn description(&self) -> &'static str { /* mapping */ }
    pub fn category(&self) -> CommandCategory { /* mapping */ }
}

pub fn get_command_names() -> Vec<&'static str> {
    CommandShortcut::iter().map(|s| s.command()).collect()
}

// ❌ WRONG - Hardcoded lists cause synchronization issues
pub fn get_command_names() -> Vec<&'static str> {
    vec!["\\q", "\\h", "\\l"] // Will miss new commands!
}
```

This pattern ensures "thanks to the enum/traits, synchronization issues will not happen anymore."

### Unified Python CLI Architecture
- **Single Codebase**: Python CLI calls Rust main logic directly via PyO3
- **Complete Feature Parity**: Python CLI supports all connection types (session://, vault://, docker://, recent://)
- **Shared Command System**: Both CLIs use `Command` enum and `CliCore` for 100% identical behavior
- **Main CLI Wrapper**: `run_command()` provides complete CLI functionality to Python
- **Zero Duplication**: No separate Python command implementations - all logic shared with Rust
- **Connection URL Support**: Full support for all URL types including SSH tunnels and Vault integration
- **Compile with `python` feature flag for unified CLI bindings**

### PyO3 Integration Patterns
- Use `Arc<TokioMutex<Database>>` for thread-safe async access
- PyO3 methods handle Tokio runtime management automatically
- Python client in `python/dbcrust/client.py` provides high-level interface
- CLI entry point: `python/dbcrust/__main__.py` delegates to Rust via `run_command()`

### Testing Strategy
- Use `rstest` for parameterized tests (as specified in Cursor rules)
- Unit tests in individual modules
- Integration tests for CLI workflows
- Do not use `cargo run` for testing (per Cursor rules)
- Test database operations with mock or test database
- **Test Isolation**: All tests use isolated temporary directories for config files
  - `Config::get_config_directory()` automatically detects test mode and returns temp directories
  - Tests never pollute real user config files (`~/.config/dbcrust/`)
  - Each test run uses a unique process-based directory: `/tmp/dbcrust_test_{pid}/`

### Code Style
- Modular design with single responsibility per module
- Use `thiserror` for custom error types
- Async/await throughout with proper error propagation
- Follow Rust naming conventions and use clippy for linting
- **MANDATORY**: ALL user interactions MUST use the `inquire` crate - NEVER use manual stdin/stdout prompting or `println!` for interactive input
  - **Example**: Column selection uses `inquire::MultiSelect` for interactive column choosing with proper Ctrl-C handling

### Feature Development Patterns

When implementing serious/new features, follow this systematic approach:

#### 1. Planning & Design Phase
- **Requirements Gathering**: Clearly define user needs and edge cases
- **Architecture Review**: Identify which modules need modification/creation
- **Data Structure Design**: Plan configuration storage, serialization needs
- **API Design**: Design clean interfaces between components
- **Backward Compatibility**: Ensure config changes use `#[serde(default)]`

#### 2. Implementation Phase
- **Core Data Structures**: Implement structs with proper serde annotations
- **Configuration Integration**: Add new fields to Config with defaults
- **Business Logic**: Implement core functionality in dedicated modules
- **CLI Integration**: Add command-line arguments and help text
- **Interactive Commands**: Add backslash commands following psql conventions
- **User Interactions**: ALL user prompts MUST use `inquire` crate (Select, Confirm, Text, etc.) - NEVER manual stdin/stdout
- **Error Handling**: Use `thiserror` for custom error types with user-friendly messages

#### 3. Testing Strategy (Critical)
- **Unit Tests**: Test individual functions and data structures
- **Integration Tests**: Test complete workflows in `tests/` directory
- **Edge Case Testing**: Test boundary conditions, invalid inputs, error cases
- **Persistence Testing**: Test configuration serialization/deserialization
- **Concurrency Testing**: Test thread safety for shared state
- **Test Coverage**: Aim for comprehensive coverage of new functionality

#### 4. Documentation & Examples
- **Code Documentation**: Add rustdoc comments for public APIs
- **User Documentation**: Update CLAUDE.md with usage examples
- **User Guide Documentation**: Update `docs/` directory (especially `docs/reference/backslash-commands.md` for new commands)
- **Configuration Documentation**: Update `docs/configuration.md` for new config options
- **Configuration Examples**: Show TOML configuration snippets
- **CLI Examples**: Demonstrate command-line usage patterns

#### 5. Validation Checklist
- [ ] All tests pass: `cargo test`
- [ ] Code compiles without warnings: `cargo build`
- [ ] Linting passes: `cargo clippy`
- [ ] Backward compatibility maintained
- [ ] Documentation updated
- [ ] Integration with existing features works
- [ ] Error messages are user-friendly
- [ ] Performance impact is acceptable

#### Example Feature Implementation Pattern

```rust
// 1. Define data structures with proper serialization
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct NewFeature {
    pub enabled: bool,
    pub config_value: String,
    pub optional_field: Option<String>,
}

impl Default for NewFeature {
    fn default() -> Self {
        Self {
            enabled: false,
            config_value: "default".to_string(),
            optional_field: None,
        }
    }
}

// 2. Add to main Config struct
pub struct Config {
    // ... existing fields
    pub new_feature: NewFeature,
}

// 3. Implement core functionality
impl NewFeature {
    pub fn process(&self) -> Result<String, MyError> {
        if !self.enabled {
            return Err(MyError::FeatureDisabled);
        }
        // ... implementation
        Ok(result)
    }
}

// 4. Add CLI integration
#[derive(Parser)]
pub struct Cli {
    // ... existing fields
    #[arg(long, help = "Enable new feature")]
    pub enable_new_feature: bool,
}

// 5. Add backslash command
"\nf" => {
    // Handle \nf command for new feature
    match args.trim() {
        "" => println!("New feature status: {}", config.new_feature.enabled),
        "on" => config.new_feature.enabled = true,
        "off" => config.new_feature.enabled = false,
        _ => eprintln!("Usage: \\nf [on|off]"),
    }
}

// 6. Comprehensive testing
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_feature_default() {
        let feature = NewFeature::default();
        assert!(!feature.enabled);
        assert_eq!(feature.config_value, "default");
    }

    #[test]
    fn test_new_feature_process_disabled() {
        let feature = NewFeature::default();
        let result = feature.process();
        assert!(matches!(result, Err(MyError::FeatureDisabled)));
    }

    #[test]
    fn test_config_serialization_with_new_feature() {
        let mut config = Config::default();
        config.new_feature.enabled = true;
        
        let serialized = toml::to_string(&config).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        
        assert_eq!(config.new_feature.enabled, deserialized.new_feature.enabled);
    }
}
```

This pattern ensures features are:
- Well-tested and reliable
- Properly integrated with the configuration system
- Backward compatible
- Well-documented
- Follow project conventions

## SSH Tunnel Configuration

Configure automatic SSH tunnels in `config.toml`:
```toml
[ssh_tunnel_patterns]
"^db\\.internal\\..*\\.com$" = "user@jumphost.example.com:2222"
```

## Vault Integration

Vault connections use the format:
- `vault://role@mount/database`
- Components are optional and will prompt interactively
- Configure with environment variables or CLI args

## Column Selection & Display Management

DBCrust provides intelligent column selection for queries that return many columns, helping users focus on relevant data with an interactive interface.

### Automatic Column Selection

The system automatically triggers column selection when query results exceed a configurable threshold:

**Configuration:**
```toml
# In ~/.config/dbcrust/config.toml
column_selection_threshold = 10  # Default: 10 columns
```

**Auto-Trigger Behavior:**
- When a query returns more columns than the threshold, column selection interface appears
- User can interactively select which columns to display
- Selected columns are remembered for the same table structure (session persistence)

### Manual Column Selection Mode

Users can manually enable column selection mode for all queries:

**Commands:**
```bash
# Toggle column selection mode on/off
\cs

# Set column selection threshold (saves to config)
\csthreshold <number>

# Clear all saved column selections
\clrcs

# Reset all view settings including column selections
\resetview
```

### Interactive Column Selection Interface

The column selection uses `inquire::MultiSelect` for a user-friendly experience:

**Features:**
- **Visual Interface**: Checkbox-style selection with arrow key navigation
- **Multi-Selection**: Use Space bar to select/deselect columns
- **Keyboard Controls**:
  - Arrow keys: Navigate between columns
  - Space: Toggle column selection
  - Enter: Confirm selection and display results
  - **Ctrl-C: Abort query and return to REPL** (doesn't exit the application)
- **Help Message**: Clear instructions displayed during selection
- **Empty Selection**: Selecting no columns displays all columns

### Session Persistence

Column selections are automatically saved and reused:

**Behavior:**
- Selections are saved per table based on column structure (headers)
- Subsequent queries on the same table automatically use saved selections
- Persistence lasts throughout the session until cleared or reset
- Each unique column structure gets its own saved selection

**Key Generation:**
- Uses column header names joined with ':' as the key
- Same query structure = same saved selection
- Different column order or names = separate selection

### Configuration Integration

Column selection settings are stored in the main configuration:

```toml
# Default column selection mode (false = only auto-trigger)
column_selection_mode_default = false

# Threshold for auto-triggering column selection
column_selection_threshold = 10
```

### Use Cases

**Large Table Analysis:**
```sql
-- Query returns 20+ columns, auto-triggers column selection
SELECT * FROM users_detailed;
-- Interactive interface appears, user selects relevant columns
-- Next time: automatically uses saved selection
```

**Manual Mode for Focused Work:**
```bash
# Enable column selection for all queries
\cs
-- Now even small result sets will prompt for column selection

# Disable when done
\cs
```

**Configuration for Team Workflows:**
```bash
# Set higher threshold for experienced users
\csthreshold 25

# Set lower threshold for analysis work
\csthreshold 5
```

## Session Management & Connection History

DBCrust includes comprehensive session management and connection history tracking as separate but complementary features.

### Saved Sessions

Named sessions allow you to save connection parameters with memorable names for quick reconnection.

**Features:**
- **Named Sessions**: Save connection parameters with memorable names
- **Password Security**: No password storage - integrates with credential stores
- **Database Support**: PostgreSQL (.pgpass), MySQL (.my.cnf), SQLite (no auth)

**Session Commands:**
```bash
# List saved sessions only
\s

# Connect to a saved session by name
\s session_name

# Save current connection as a session
\ss session_name

# Delete a saved session
\sd session_name
```

**Command Line Session Access:**
```bash
# Connect using saved session
dbcrust session://production_db

# Run query against saved session
dbcrust session://staging_db -c "SELECT version()"
```

### Connection History

Connection history automatically tracks recent database connections with full URLs (except passwords) for easy reconnection.

**Features:**
- **Automatic Tracking**: All successful/failed connections are recorded
- **Full URL Storage**: Complete connection details including containers (docker://user@container/db)
- **Interactive Selection**: Use `recent://` for guided reconnection
- **Display Names**: Human-readable connection descriptions

**History Commands:**
```bash
# List recent connections
\r

# Clear connection history
\rc
```

**Interactive Recent Connection Access:**
```bash
# Interactive selection from recent connections
dbcrust recent://
```

This will display a numbered list of recent connections and prompt you to select one.

### Configuration Structure

Sessions and connection history are stored separately in `~/.config/dbcrust/config.toml`:

```toml
# Saved sessions (named connections)
[saved_sessions.production]
host = "prod.example.com"
port = 5432
user = "app_user"
dbname = "myapp_prod"
database_type = "PostgreSQL"
created_at = "2024-01-15T10:30:00Z"

# Connection history (automatic tracking)
[[recent_connections]]
connection_url = "postgres://user@localhost:5432/testdb"
display_name = "user@localhost:5432/testdb"
timestamp = "2024-01-15T14:22:33Z"
database_type = "PostgreSQL"
success = true

[[recent_connections]]
connection_url = "docker://user@my-postgres-container/myapp"
display_name = "docker://user@my-postgres-container/myapp"
timestamp = "2024-01-15T14:20:15Z"
database_type = "PostgreSQL"
success = true
```

### Password Management Integration

DBCrust automatically looks up passwords from database-specific credential stores:

- **PostgreSQL**: Uses `.pgpass` file (format: `host:port:database:user:password`)
- **MySQL**: Uses `.my.cnf` file `[client]` section
- **SQLite**: No authentication required

### Session URL Reconstruction

When connecting to sessions, DBCrust reconstructs full connection URLs:

1. Retrieves connection parameters from saved session
2. Looks up password from appropriate credential store
3. Builds complete connection URL with password (if found)
4. Falls back to password prompt if credential not found

### Development Pattern Example

```rust
// Configuration structure with separated concerns
pub struct Config {
    pub saved_sessions: HashMap<String, SavedSession>,      // Named sessions
    pub recent_connections: Vec<RecentConnection>,          // Connection history
    // ... other fields
}

// RecentConnection structure (automatic tracking)
pub struct RecentConnection {
    pub connection_url: String,     // Full URL without password
    pub display_name: String,       // Human-readable description  
    pub timestamp: DateTime<Utc>,   // When connection occurred
    pub database_type: DatabaseType,// PostgreSQL, MySQL, SQLite
    pub success: bool,              // Connection success/failure
}

// Track connections automatically (with auto-generated display name)
config.add_recent_connection_auto_display(
    sanitized_url,
    database_type,
    true // success
)?;

// Save named session
config.save_session_with_db_type(
    "session_name",
    DatabaseType::PostgreSQL,
    None, // file_path for SQLite
    custom_params
)?;

// Recent connection history management
let recent = config.get_recent_connections();
config.clear_recent_connections()?;
```

## Important Notes

- Project uses Rust 2024 edition
- SQLx for async PostgreSQL operations
- Reedline for modern CLI experience
- Conditional compilation for Python bindings
- Configuration stored in `~/.config/dbcrust/config.toml`
- Debug logs accessible via `--show-debug-logs` flag