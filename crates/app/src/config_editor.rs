//! Schema-driven configuration management.
//!
//! Single source of truth for every editable scalar in [`Config`]: the
//! `SCHEMA` table powers `\config get`/`\config set` key resolution, the
//! interactive `\config` menu (its `inquire` prompts live in the CLI crate,
//! `dbcrust::config_editor`), the desktop app settings, value validation,
//! and a completeness test that fails when a `Config` field is added without
//! a matching entry (or without being written by `save_with_documentation`).
//!
//! `ssh_tunnel_patterns` is a free-form map and gets its own submenu;
//! `named_queries` (the in-struct map) is frozen legacy and excluded.

use crate::config::{Config, LogLevel};
use dbcrust_core::password_sanitizer::sanitize_ssh_tunnel_string;
use std::collections::BTreeMap;
use strum::{EnumIter, IntoEnumIterator};

/// Sections shown in the interactive menu; every `FieldSpec` belongs to one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter)]
pub enum ConfigSection {
    Display,
    Pager,
    Features,
    Timeouts,
    Vault,
    VectorDisplay,
    ComplexDisplay,
    Ai,
    Logging,
    History,
    Secrets,
    SshTunnelPatterns,
}

impl ConfigSection {
    pub fn label(&self) -> &'static str {
        match self {
            ConfigSection::Display => "Display settings",
            ConfigSection::Pager => "Pager",
            ConfigSection::Features => "Features",
            ConfigSection::Timeouts => "Timeouts",
            ConfigSection::Vault => "Vault integration",
            ConfigSection::VectorDisplay => "Vector display",
            ConfigSection::ComplexDisplay => "Complex data display",
            ConfigSection::Ai => "AI assistant",
            ConfigSection::Logging => "Logging",
            ConfigSection::History => "History",
            ConfigSection::Secrets => "Secret storage",
            ConfigSection::SshTunnelPatterns => "SSH tunnel patterns",
        }
    }

    /// One-line summary of the section's current values, for menu labels.
    pub fn live_summary(&self, config: &Config) -> String {
        let on_off = |b: bool| if b { "on" } else { "off" };
        match self {
            ConfigSection::Display => format!(
                "limit={}, expanded={}",
                config.default_limit,
                on_off(config.expanded_display_default)
            ),
            ConfigSection::Pager => format!(
                "{}, {}",
                if config.pager_enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                config.pager_command
            ),
            ConfigSection::Features => format!(
                "autocomplete={}, explain={}",
                on_off(config.autocomplete_enabled),
                on_off(config.explain_mode_default)
            ),
            ConfigSection::Timeouts => format!(
                "query={}s, metadata={}s",
                config.query_timeout_seconds, config.metadata_timeout_seconds
            ),
            ConfigSection::Vault => format!(
                "cache={}, credential-process={}",
                on_off(config.vault_credential_cache_enabled),
                if config.vault_credential_process.trim().is_empty() {
                    "off"
                } else {
                    "configured"
                }
            ),
            ConfigSection::VectorDisplay => format!("mode={}", config.vector_display.display_mode),
            ConfigSection::ComplexDisplay => {
                format!("mode={}", config.complex_display.display_mode)
            }
            ConfigSection::Ai => format!(
                "{}, model={}",
                if config.ai.enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                config.ai.model
            ),
            ConfigSection::Logging => format!("level={}", config.logging.level),
            ConfigSection::History => {
                format!("per-session={}", on_off(config.history.per_session_enabled))
            }
            ConfigSection::Secrets => format!(
                "backend={}, unlock-cache={}",
                config.secrets.backend.as_str(),
                on_off(config.secrets.file_unlock_cache)
            ),
            ConfigSection::SshTunnelPatterns => {
                let n = config.ssh_tunnel_patterns.len();
                format!("{n} pattern{}", if n == 1 { "" } else { "s" })
            }
        }
    }
}

/// Value type of a config field: drives validation and the prompt widget.
#[derive(Debug, Clone, Copy)]
pub enum FieldKind {
    Bool,
    UInt {
        min: u64,
        max: u64,
    },
    Float {
        min: f64,
        max: f64,
    },
    Text {
        allow_empty: bool,
    },
    /// `Option<String>` field — empty input clears it.
    OptionalText,
    Enum(&'static [&'static str]),
}

impl FieldKind {
    /// Validate raw input and return the canonical value string passed to the setter.
    pub fn validate(&self, input: &str) -> Result<String, String> {
        let trimmed = input.trim();
        match self {
            FieldKind::Bool => match trimmed.to_lowercase().as_str() {
                "true" | "on" | "yes" | "1" => Ok("true".to_string()),
                "false" | "off" | "no" | "0" => Ok("false".to_string()),
                _ => Err(format!(
                    "expected a boolean (true/false, on/off, yes/no, 1/0), got \"{trimmed}\""
                )),
            },
            FieldKind::UInt { min, max } => {
                let n: u64 = trimmed
                    .parse()
                    .map_err(|_| format!("expected an integer, got \"{trimmed}\""))?;
                if n < *min || n > *max {
                    return Err(format!("value must be between {min} and {max}, got {n}"));
                }
                Ok(n.to_string())
            }
            FieldKind::Float { min, max } => {
                let f: f64 = trimmed
                    .parse()
                    .map_err(|_| format!("expected a number, got \"{trimmed}\""))?;
                if !f.is_finite() || f < *min || f > *max {
                    return Err(format!(
                        "value must be between {min} and {max}, got {trimmed}"
                    ));
                }
                Ok(f.to_string())
            }
            FieldKind::Text { allow_empty } => {
                if trimmed.is_empty() && !allow_empty {
                    return Err("value cannot be empty".to_string());
                }
                Ok(input.trim().to_string())
            }
            FieldKind::OptionalText => Ok(trimmed.to_string()),
            FieldKind::Enum(options) => options
                .iter()
                .find(|o| o.eq_ignore_ascii_case(trimmed))
                .map(|o| o.to_string())
                .ok_or_else(|| {
                    format!(
                        "invalid value \"{trimmed}\". Valid options: {}",
                        options.join(", ")
                    )
                }),
        }
    }
}

/// One editable configuration field.
#[derive(Debug)]
pub struct FieldSpec {
    /// Dotted key as it appears in config.toml, e.g. `logging.level`.
    pub path: &'static str,
    /// Short human label used as the prompt message.
    pub label: &'static str,
    /// One-line help shown in the menu.
    pub help: &'static str,
    pub kind: FieldKind,
    pub section: ConfigSection,
    /// Masked as `***` in list output; shown only by explicit `\config get <key>`.
    pub sensitive: bool,
    pub get: fn(&Config) -> String,
    /// Receives the canonical string produced by `FieldKind::validate`.
    pub set: fn(&mut Config, &str) -> Result<(), String>,
}

fn pnum<T: std::str::FromStr>(v: &str) -> Result<T, String>
where
    T::Err: std::fmt::Display,
{
    v.parse::<T>().map_err(|e| format!("invalid number: {e}"))
}

fn pbool(v: &str) -> bool {
    v == "true"
}

fn parse_log_level(v: &str) -> Result<LogLevel, String> {
    match v {
        "trace" => Ok(LogLevel::Trace),
        "debug" => Ok(LogLevel::Debug),
        "info" => Ok(LogLevel::Info),
        "warn" => Ok(LogLevel::Warn),
        "error" => Ok(LogLevel::Error),
        _ => Err(format!("invalid log level: {v}")),
    }
}

fn parse_execution_mode(v: &str) -> Result<dbcrust_core::ai_config::AiExecutionMode, String> {
    use dbcrust_core::ai_config::AiExecutionMode;
    match v {
        "confirm" => Ok(AiExecutionMode::Confirm),
        "auto_select" => Ok(AiExecutionMode::AutoSelect),
        "auto_execute" => Ok(AiExecutionMode::AutoExecute),
        _ => Err(format!("invalid execution mode: {v}")),
    }
}

const LOG_LEVELS: &[&str] = &["trace", "debug", "info", "warn", "error"];
const DISPLAY_MODES: &[&str] = &["full", "truncated", "summary", "viz"];
const AI_EXECUTION_MODES: &[&str] = &["confirm", "auto_select", "auto_execute"];

fn parse_auth_method(v: &str) -> Result<dbcrust_core::ai_config::AiAuthMethod, String> {
    use dbcrust_core::ai_config::AiAuthMethod;
    match v {
        "api_key" => Ok(AiAuthMethod::ApiKey),
        "chatgpt_subscription" => Ok(AiAuthMethod::ChatgptSubscription),
        _ => Err(format!("invalid auth method: {v}")),
    }
}

const AI_AUTH_METHODS: &[&str] = &["api_key", "chatgpt_subscription"];

const SECRETS_BACKENDS: &[&str] = &["auto", "keychain", "file"];

/// Every editable scalar leaf of [`Config`], grouped by section.
///
/// The completeness test in this module compares these paths against the
/// serialized form of `Config` — adding a config field without a `FieldSpec`
/// (or without a `save_with_documentation` entry) fails the build's tests.
static SCHEMA: &[FieldSpec] = &[
    // ---------- Display ----------
    FieldSpec {
        path: "default_limit",
        label: "Default row limit",
        help: "Auto-LIMIT applied to SELECT queries; 0 disables (default: 100)",
        kind: FieldKind::UInt {
            min: 0,
            max: 10_000_000,
        },
        section: ConfigSection::Display,
        sensitive: false,
        get: |c| c.default_limit.to_string(),
        set: |c, v| {
            c.default_limit = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "expanded_display_default",
        label: "Expanded display by default",
        help: "Use expanded (vertical) display mode by default (default: false)",
        kind: FieldKind::Bool,
        section: ConfigSection::Display,
        sensitive: false,
        get: |c| c.expanded_display_default.to_string(),
        set: |c, v| {
            c.expanded_display_default = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "show_banner",
        label: "Show banner on startup",
        help: "Print the DBCrust banner when starting (default: false)",
        kind: FieldKind::Bool,
        section: ConfigSection::Display,
        sensitive: false,
        get: |c| c.show_banner.to_string(),
        set: |c, v| {
            c.show_banner = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "show_server_info",
        label: "Show server info on connect",
        help: "Print server version/details after connecting (default: true)",
        kind: FieldKind::Bool,
        section: ConfigSection::Display,
        sensitive: false,
        get: |c| c.show_server_info.to_string(),
        set: |c, v| {
            c.show_server_info = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "multiline_prompt_indicator",
        label: "Multiline prompt indicator",
        help: "Indicator shown on continuation lines (default: empty)",
        kind: FieldKind::Text { allow_empty: true },
        section: ConfigSection::Display,
        sensitive: false,
        get: |c| c.multiline_prompt_indicator.clone(),
        set: |c, v| {
            c.multiline_prompt_indicator = v.to_string();
            Ok(())
        },
    },
    FieldSpec {
        path: "column_selection_threshold",
        label: "Column selection threshold",
        help: "Column count that triggers interactive column selection; 0 disables (default: 10)",
        kind: FieldKind::UInt {
            min: 0,
            max: 10_000,
        },
        section: ConfigSection::Display,
        sensitive: false,
        get: |c| c.column_selection_threshold.to_string(),
        set: |c, v| {
            c.column_selection_threshold = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "column_selection_default_all",
        label: "Column selection defaults to all",
        help: "Pre-select all columns in the column picker (default: false)",
        kind: FieldKind::Bool,
        section: ConfigSection::Display,
        sensitive: false,
        get: |c| c.column_selection_default_all.to_string(),
        set: |c, v| {
            c.column_selection_default_all = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "test_named_query_before_saving",
        label: "Test named queries before saving",
        help: "Validate named queries with EXPLAIN before saving (default: true)",
        kind: FieldKind::Bool,
        section: ConfigSection::Display,
        sensitive: false,
        get: |c| c.test_named_query_before_saving.to_string(),
        set: |c, v| {
            c.test_named_query_before_saving = pbool(v);
            Ok(())
        },
    },
    // ---------- Pager ----------
    FieldSpec {
        path: "pager_enabled",
        label: "Enable pager",
        help: "Page large outputs through an external pager (default: true)",
        kind: FieldKind::Bool,
        section: ConfigSection::Pager,
        sensitive: false,
        get: |c| c.pager_enabled.to_string(),
        set: |c, v| {
            c.pager_enabled = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "pager_command",
        label: "Pager command",
        help: "External pager command (default: \"less -R\")",
        kind: FieldKind::Text { allow_empty: false },
        section: ConfigSection::Pager,
        sensitive: false,
        get: |c| c.pager_command.clone(),
        set: |c, v| {
            c.pager_command = v.to_string();
            Ok(())
        },
    },
    FieldSpec {
        path: "pager_threshold_lines",
        label: "Pager threshold lines",
        help: "Lines before the pager kicks in; 0 = terminal height (default: 0)",
        kind: FieldKind::UInt {
            min: 0,
            max: 1_000_000,
        },
        section: ConfigSection::Pager,
        sensitive: false,
        get: |c| c.pager_threshold_lines.to_string(),
        set: |c, v| {
            c.pager_threshold_lines = pnum(v)?;
            Ok(())
        },
    },
    // ---------- Features ----------
    FieldSpec {
        path: "autocomplete_enabled",
        label: "SQL autocomplete",
        help: "Enable SQL autocompletion (default: true)",
        kind: FieldKind::Bool,
        section: ConfigSection::Features,
        sensitive: false,
        get: |c| c.autocomplete_enabled.to_string(),
        set: |c, v| {
            c.autocomplete_enabled = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "explain_mode_default",
        label: "EXPLAIN mode by default",
        help: "Start sessions with EXPLAIN mode enabled (default: false)",
        kind: FieldKind::Bool,
        section: ConfigSection::Features,
        sensitive: false,
        get: |c| c.explain_mode_default.to_string(),
        set: |c, v| {
            c.explain_mode_default = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "read_only_default",
        label: "Read-only by default",
        help: "Reject write statements unless --read-only=false is passed (best-effort guard; default: false)",
        kind: FieldKind::Bool,
        section: ConfigSection::Features,
        sensitive: false,
        get: |c| c.read_only_default.to_string(),
        set: |c, v| {
            c.read_only_default = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "max_recent_connections",
        label: "Max recent connections",
        help: "Number of recent connections to remember (default: 10)",
        kind: FieldKind::UInt { min: 1, max: 1_000 },
        section: ConfigSection::Features,
        sensitive: false,
        get: |c| c.max_recent_connections.to_string(),
        set: |c, v| {
            c.max_recent_connections = pnum(v)?;
            Ok(())
        },
    },
    // ---------- Timeouts ----------
    FieldSpec {
        path: "query_timeout_seconds",
        label: "Query timeout (seconds)",
        help: "Query execution timeout (default: 30) — applies immediately",
        kind: FieldKind::UInt {
            min: 1,
            max: 86_400,
        },
        section: ConfigSection::Timeouts,
        sensitive: false,
        get: |c| c.query_timeout_seconds.to_string(),
        set: |c, v| {
            c.query_timeout_seconds = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "metadata_timeout_seconds",
        label: "Metadata timeout (seconds)",
        help: "Timeout for metadata/autocomplete queries (default: 10)",
        kind: FieldKind::UInt {
            min: 1,
            max: 86_400,
        },
        section: ConfigSection::Timeouts,
        sensitive: false,
        get: |c| c.metadata_timeout_seconds.to_string(),
        set: |c, v| {
            c.metadata_timeout_seconds = pnum(v)?;
            Ok(())
        },
    },
    // ---------- Vault ----------
    FieldSpec {
        path: "vault_credential_cache_enabled",
        label: "Vault credential caching",
        help: "Cache HashiCorp Vault credentials between sessions (default: true)",
        kind: FieldKind::Bool,
        section: ConfigSection::Vault,
        sensitive: false,
        get: |c| c.vault_credential_cache_enabled.to_string(),
        set: |c, v| {
            c.vault_credential_cache_enabled = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "vault_cache_renewal_threshold",
        label: "Vault renewal threshold",
        help: "Renew credentials when this fraction of TTL remains (default: 0.25)",
        kind: FieldKind::Float { min: 0.0, max: 1.0 },
        section: ConfigSection::Vault,
        sensitive: false,
        get: |c| c.vault_cache_renewal_threshold.to_string(),
        set: |c, v| {
            c.vault_cache_renewal_threshold = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "vault_cache_min_ttl_seconds",
        label: "Vault minimum TTL (seconds)",
        help: "Don't cache credentials with TTL below this (default: 300)",
        kind: FieldKind::UInt {
            min: 0,
            max: 31_536_000,
        },
        section: ConfigSection::Vault,
        sensitive: false,
        get: |c| c.vault_cache_min_ttl_seconds.to_string(),
        set: |c, v| {
            c.vault_cache_min_ttl_seconds = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "vault_credential_process",
        label: "Vault credential process",
        help: "Command run lazily for vault:// targets; prints a token or JSON credentials",
        kind: FieldKind::Text { allow_empty: true },
        section: ConfigSection::Vault,
        sensitive: false,
        get: |c| c.vault_credential_process.clone(),
        set: |c, v| {
            c.vault_credential_process = v.to_string();
            Ok(())
        },
    },
    // ---------- Vector display ----------
    FieldSpec {
        path: "vector_display.display_mode",
        label: "Vector display mode",
        help: "How pgvector/embedding values are rendered (default: truncated)",
        kind: FieldKind::Enum(DISPLAY_MODES),
        section: ConfigSection::VectorDisplay,
        sensitive: false,
        get: |c| c.vector_display.display_mode.to_string(),
        set: |c, v| {
            c.vector_display.display_mode = v.parse()?;
            Ok(())
        },
    },
    FieldSpec {
        path: "vector_display.truncation_length",
        label: "Vector truncation length",
        help: "Elements shown at start/end when truncated (default: 5)",
        kind: FieldKind::UInt {
            min: 1,
            max: 10_000,
        },
        section: ConfigSection::VectorDisplay,
        sensitive: false,
        get: |c| c.vector_display.truncation_length.to_string(),
        set: |c, v| {
            c.vector_display.truncation_length = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "vector_display.viz_width",
        label: "Vector viz width",
        help: "Width of the ASCII visualization (default: 40)",
        kind: FieldKind::UInt {
            min: 10,
            max: 1_000,
        },
        section: ConfigSection::VectorDisplay,
        sensitive: false,
        get: |c| c.vector_display.viz_width.to_string(),
        set: |c, v| {
            c.vector_display.viz_width = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "vector_display.show_statistics",
        label: "Vector statistics",
        help: "Show summary statistics alongside other modes (default: false)",
        kind: FieldKind::Bool,
        section: ConfigSection::VectorDisplay,
        sensitive: false,
        get: |c| c.vector_display.show_statistics.to_string(),
        set: |c, v| {
            c.vector_display.show_statistics = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "vector_display.dimension_threshold",
        label: "Vector dimension threshold",
        help: "Auto-switch to truncated mode above this dimension count (default: 20)",
        kind: FieldKind::UInt {
            min: 1,
            max: 100_000,
        },
        section: ConfigSection::VectorDisplay,
        sensitive: false,
        get: |c| c.vector_display.dimension_threshold.to_string(),
        set: |c, v| {
            c.vector_display.dimension_threshold = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "vector_display.show_dimensions",
        label: "Vector dimension count",
        help: "Show the dimension count in all modes (default: true)",
        kind: FieldKind::Bool,
        section: ConfigSection::VectorDisplay,
        sensitive: false,
        get: |c| c.vector_display.show_dimensions.to_string(),
        set: |c, v| {
            c.vector_display.show_dimensions = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "vector_display.full_elements_per_row",
        label: "Vector elements per row",
        help: "Elements per row in full-mode matrix layout (default: 8)",
        kind: FieldKind::UInt { min: 1, max: 1_000 },
        section: ConfigSection::VectorDisplay,
        sensitive: false,
        get: |c| c.vector_display.full_elements_per_row.to_string(),
        set: |c, v| {
            c.vector_display.full_elements_per_row = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "vector_display.full_show_row_numbers",
        label: "Vector row numbers",
        help: "Show row numbers in full-mode matrix layout (default: true)",
        kind: FieldKind::Bool,
        section: ConfigSection::VectorDisplay,
        sensitive: false,
        get: |c| c.vector_display.full_show_row_numbers.to_string(),
        set: |c, v| {
            c.vector_display.full_show_row_numbers = pbool(v);
            Ok(())
        },
    },
    // ---------- Complex display ----------
    FieldSpec {
        path: "complex_display.display_mode",
        label: "Complex display mode",
        help: "How arrays/JSON/composites are rendered (default: truncated)",
        kind: FieldKind::Enum(DISPLAY_MODES),
        section: ConfigSection::ComplexDisplay,
        sensitive: false,
        get: |c| c.complex_display.display_mode.to_string(),
        set: |c, v| {
            c.complex_display.display_mode =
                dbcrust_core::complex_display::ComplexDisplayMode::from_str(v)
                    .ok_or_else(|| format!("invalid display mode: {v}"))?;
            Ok(())
        },
    },
    FieldSpec {
        path: "complex_display.truncation_length",
        label: "Complex truncation length",
        help: "Maximum elements shown in truncated mode (default: 5)",
        kind: FieldKind::UInt {
            min: 1,
            max: 10_000,
        },
        section: ConfigSection::ComplexDisplay,
        sensitive: false,
        get: |c| c.complex_display.truncation_length.to_string(),
        set: |c, v| {
            c.complex_display.truncation_length = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "complex_display.viz_width",
        label: "Complex viz width",
        help: "Width for visualization modes (default: 40)",
        kind: FieldKind::UInt {
            min: 10,
            max: 1_000,
        },
        section: ConfigSection::ComplexDisplay,
        sensitive: false,
        get: |c| c.complex_display.viz_width.to_string(),
        set: |c, v| {
            c.complex_display.viz_width = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "complex_display.show_metadata",
        label: "Complex metadata",
        help: "Show metadata/statistics for complex values (default: false)",
        kind: FieldKind::Bool,
        section: ConfigSection::ComplexDisplay,
        sensitive: false,
        get: |c| c.complex_display.show_metadata.to_string(),
        set: |c, v| {
            c.complex_display.show_metadata = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "complex_display.size_threshold",
        label: "Complex size threshold",
        help: "Auto-switch to truncated mode above this element count (default: 20)",
        kind: FieldKind::UInt {
            min: 1,
            max: 100_000,
        },
        section: ConfigSection::ComplexDisplay,
        sensitive: false,
        get: |c| c.complex_display.size_threshold.to_string(),
        set: |c, v| {
            c.complex_display.size_threshold = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "complex_display.show_dimensions",
        label: "Complex size info",
        help: "Show size/dimension information (default: true)",
        kind: FieldKind::Bool,
        section: ConfigSection::ComplexDisplay,
        sensitive: false,
        get: |c| c.complex_display.show_dimensions.to_string(),
        set: |c, v| {
            c.complex_display.show_dimensions = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "complex_display.full_elements_per_row",
        label: "Complex elements per row",
        help: "Elements per row in full mode (default: 8)",
        kind: FieldKind::UInt { min: 1, max: 1_000 },
        section: ConfigSection::ComplexDisplay,
        sensitive: false,
        get: |c| c.complex_display.full_elements_per_row.to_string(),
        set: |c, v| {
            c.complex_display.full_elements_per_row = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "complex_display.max_width",
        label: "Complex max width",
        help: "Maximum display width (default: 80)",
        kind: FieldKind::UInt {
            min: 20,
            max: 10_000,
        },
        section: ConfigSection::ComplexDisplay,
        sensitive: false,
        get: |c| c.complex_display.max_width.to_string(),
        set: |c, v| {
            c.complex_display.max_width = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "complex_display.full_show_numbers",
        label: "Complex row/field numbers",
        help: "Show row/field numbers in full mode (default: true)",
        kind: FieldKind::Bool,
        section: ConfigSection::ComplexDisplay,
        sensitive: false,
        get: |c| c.complex_display.full_show_numbers.to_string(),
        set: |c, v| {
            c.complex_display.full_show_numbers = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "complex_display.json_pretty_print",
        label: "Pretty-print JSON",
        help: "Pretty-print JSON values instead of compact (default: false)",
        kind: FieldKind::Bool,
        section: ConfigSection::ComplexDisplay,
        sensitive: false,
        get: |c| c.complex_display.json_pretty_print.to_string(),
        set: |c, v| {
            c.complex_display.json_pretty_print = pbool(v);
            Ok(())
        },
    },
    // ---------- AI ----------
    FieldSpec {
        path: "ai.enabled",
        label: "AI assistant",
        help: "Enable ?? text-to-SQL and \\ai commands (default: false)",
        kind: FieldKind::Bool,
        section: ConfigSection::Ai,
        sensitive: false,
        get: |c| c.ai.enabled.to_string(),
        set: |c, v| {
            c.ai.enabled = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "ai.provider",
        label: "AI provider",
        help: "Provider key (anthropic, openai, ...), or \"auto\" to infer from the model",
        kind: FieldKind::Text { allow_empty: false },
        section: ConfigSection::Ai,
        sensitive: false,
        get: |c| c.ai.provider.clone(),
        set: |c, v| {
            let p = v.trim().to_lowercase();
            // Without the `ai` feature there is no provider list to check
            // against; the value is kept as written (`[ai]` still parses)
            #[cfg(feature = "ai")]
            if p != "auto" && dbcrust_ai::AdapterKind::from_lower_str(&p).is_none() {
                return Err(format!(
                    "unknown provider: {v} (use \"auto\" or a genai provider key like anthropic, openai, gemini, ollama)"
                ));
            }
            c.ai.provider = p;
            Ok(())
        },
    },
    FieldSpec {
        path: "ai.model",
        label: "AI model",
        help: "Model id; provider inferred from the name, or use provider::model",
        kind: FieldKind::Text { allow_empty: false },
        section: ConfigSection::Ai,
        sensitive: false,
        get: |c| c.ai.model.clone(),
        set: |c, v| {
            c.ai.model = v.to_string();
            Ok(())
        },
    },
    FieldSpec {
        path: "ai.auth_method",
        label: "AI auth method",
        help: "api_key, or chatgpt_subscription to use a ChatGPT plan (default: api_key)",
        kind: FieldKind::Enum(AI_AUTH_METHODS),
        section: ConfigSection::Ai,
        sensitive: false,
        get: |c| c.ai.auth_method.to_string(),
        set: |c, v| {
            c.ai.auth_method = parse_auth_method(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "ai.endpoint",
        label: "AI endpoint URL",
        help: "Custom endpoint (Ollama, gateways); empty uses the provider default",
        kind: FieldKind::OptionalText,
        section: ConfigSection::Ai,
        sensitive: false,
        get: |c| c.ai.endpoint.clone().unwrap_or_default(),
        set: |c, v| {
            c.ai.endpoint = if v.is_empty() {
                None
            } else {
                Some(v.to_string())
            };
            Ok(())
        },
    },
    FieldSpec {
        path: "ai.max_tokens",
        label: "AI max output tokens",
        help: "Maximum output tokens per response (default: 4096)",
        kind: FieldKind::UInt {
            min: 1,
            max: u32::MAX as u64,
        },
        section: ConfigSection::Ai,
        sensitive: false,
        get: |c| c.ai.max_tokens.to_string(),
        set: |c, v| {
            c.ai.max_tokens = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "ai.temperature",
        label: "AI temperature",
        help: "Sampling temperature, 0.0 = deterministic (default: 0.0)",
        kind: FieldKind::Float { min: 0.0, max: 2.0 },
        section: ConfigSection::Ai,
        sensitive: false,
        get: |c| c.ai.temperature.to_string(),
        set: |c, v| {
            c.ai.temperature = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "ai.streaming",
        label: "AI streaming",
        help: "Stream responses as they are generated (default: true)",
        kind: FieldKind::Bool,
        section: ConfigSection::Ai,
        sensitive: false,
        get: |c| c.ai.streaming.to_string(),
        set: |c, v| {
            c.ai.streaming = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "ai.max_schema_tables",
        label: "AI schema table limit",
        help: "Max tables included in schema context (default: 50)",
        kind: FieldKind::UInt {
            min: 1,
            max: 10_000,
        },
        section: ConfigSection::Ai,
        sensitive: false,
        get: |c| c.ai.max_schema_tables.to_string(),
        set: |c, v| {
            c.ai.max_schema_tables = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "ai.show_generated_sql",
        label: "Show generated SQL",
        help: "Display generated SQL before execution (default: true)",
        kind: FieldKind::Bool,
        section: ConfigSection::Ai,
        sensitive: false,
        get: |c| c.ai.show_generated_sql.to_string(),
        set: |c, v| {
            c.ai.show_generated_sql = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "ai.execution_mode",
        label: "AI execution mode",
        help: "What happens after SQL generation (default: confirm)",
        kind: FieldKind::Enum(AI_EXECUTION_MODES),
        section: ConfigSection::Ai,
        sensitive: false,
        get: |c| c.ai.execution_mode.to_string(),
        set: |c, v| {
            c.ai.execution_mode = parse_execution_mode(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "ai.history_length",
        label: "AI history length",
        help: "Conversation exchanges to keep, 0 = stateless (default: 5)",
        kind: FieldKind::UInt { min: 0, max: 1_000 },
        section: ConfigSection::Ai,
        sensitive: false,
        get: |c| c.ai.history_length.to_string(),
        set: |c, v| {
            c.ai.history_length = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "ai.agentic_max_iterations",
        label: "Agentic max iterations",
        help: "Max tool-call turns for ??? agentic mode (default: 8)",
        kind: FieldKind::UInt { min: 1, max: 50 },
        section: ConfigSection::Ai,
        sensitive: false,
        get: |c| c.ai.agentic_max_iterations.to_string(),
        set: |c, v| {
            c.ai.agentic_max_iterations = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "ai.agentic_max_rows_per_tool",
        label: "Agentic max rows per tool",
        help: "Max rows from one agentic tool query fed to the model (default: 50)",
        kind: FieldKind::UInt { min: 1, max: 1_000 },
        section: ConfigSection::Ai,
        sensitive: false,
        get: |c| c.ai.agentic_max_rows_per_tool.to_string(),
        set: |c, v| {
            c.ai.agentic_max_rows_per_tool = pnum(v)?;
            Ok(())
        },
    },
    // ---------- Logging ----------
    FieldSpec {
        path: "logging.level",
        label: "Log level",
        help: "Logging verbosity (default: info)",
        kind: FieldKind::Enum(LOG_LEVELS),
        section: ConfigSection::Logging,
        sensitive: false,
        get: |c| c.logging.level.to_string(),
        set: |c, v| {
            c.logging.level = parse_log_level(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "logging.console_output",
        label: "Console logging",
        help: "Write logs to the console (default: true)",
        kind: FieldKind::Bool,
        section: ConfigSection::Logging,
        sensitive: false,
        get: |c| c.logging.console_output.to_string(),
        set: |c, v| {
            c.logging.console_output = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "logging.file_output",
        label: "File logging",
        help: "Write logs to a file (default: false)",
        kind: FieldKind::Bool,
        section: ConfigSection::Logging,
        sensitive: false,
        get: |c| c.logging.file_output.to_string(),
        set: |c, v| {
            c.logging.file_output = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "logging.file_path",
        label: "Log file path",
        help: "Path of the log file (default: ~/.config/dbcrust/logs/dbcrust.log)",
        kind: FieldKind::Text { allow_empty: false },
        section: ConfigSection::Logging,
        sensitive: false,
        get: |c| c.logging.file_path.clone(),
        set: |c, v| {
            c.logging.file_path = v.to_string();
            Ok(())
        },
    },
    FieldSpec {
        path: "logging.max_file_size_mb",
        label: "Max log file size (MB)",
        help: "Log size before rotation (default: 10)",
        kind: FieldKind::UInt {
            min: 1,
            max: 10_240,
        },
        section: ConfigSection::Logging,
        sensitive: false,
        get: |c| c.logging.max_file_size_mb.to_string(),
        set: |c, v| {
            c.logging.max_file_size_mb = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "logging.max_files",
        label: "Rotated log files to keep",
        help: "Number of rotated log files kept (default: 5)",
        kind: FieldKind::UInt { min: 1, max: 1_000 },
        section: ConfigSection::Logging,
        sensitive: false,
        get: |c| c.logging.max_files.to_string(),
        set: |c, v| {
            c.logging.max_files = pnum(v)?;
            Ok(())
        },
    },
    // ---------- History ----------
    FieldSpec {
        path: "history.per_session_enabled",
        label: "Per-session history",
        help: "Keep a separate history per connection (default: true)",
        kind: FieldKind::Bool,
        section: ConfigSection::History,
        sensitive: false,
        get: |c| c.history.per_session_enabled.to_string(),
        set: |c, v| {
            c.history.per_session_enabled = pbool(v);
            Ok(())
        },
    },
    FieldSpec {
        path: "history.max_history_files",
        label: "Max history files",
        help: "Maximum history files to keep (default: 50)",
        kind: FieldKind::UInt {
            min: 1,
            max: 10_000,
        },
        section: ConfigSection::History,
        sensitive: false,
        get: |c| c.history.max_history_files.to_string(),
        set: |c, v| {
            c.history.max_history_files = pnum(v)?;
            Ok(())
        },
    },
    FieldSpec {
        path: "history.cleanup_after_days",
        label: "History cleanup (days)",
        help: "Delete unused history files after N days (default: 90)",
        kind: FieldKind::UInt {
            min: 1,
            max: 36_500,
        },
        section: ConfigSection::History,
        sensitive: false,
        get: |c| c.history.cleanup_after_days.to_string(),
        set: |c, v| {
            c.history.cleanup_after_days = pnum(v)?;
            Ok(())
        },
    },
    // ---------- Secret storage ----------
    FieldSpec {
        path: "secrets.backend",
        label: "Secret storage backend",
        help: "auto = OS keychain on macOS/Windows, encrypted file on Linux; no silent fallback (default: auto)",
        kind: FieldKind::Enum(SECRETS_BACKENDS),
        section: ConfigSection::Secrets,
        sensitive: false,
        get: |c| c.secrets.backend.as_str().to_string(),
        set: |c, v| {
            c.secrets.backend = dbcrust_secrets::BackendChoice::parse(v)
                .ok_or_else(|| format!("invalid secrets backend: {v}"))?;
            dbcrust_secrets::reset_default_store();
            Ok(())
        },
    },
    FieldSpec {
        path: "secrets.file_unlock_cache",
        label: "Cache file unlock for the login session",
        help: "Linux file backend: remember the unlocked key in the kernel session keyring (default: true)",
        kind: FieldKind::Bool,
        section: ConfigSection::Secrets,
        sensitive: false,
        get: |c| c.secrets.file_unlock_cache.to_string(),
        set: |c, v| {
            c.secrets.file_unlock_cache = pbool(v);
            dbcrust_secrets::reset_default_store();
            Ok(())
        },
    },
];

pub fn schema() -> &'static [FieldSpec] {
    SCHEMA
}

pub fn find_spec(key: &str) -> Option<&'static FieldSpec> {
    SCHEMA.iter().find(|s| s.path == key)
}

// ---------------------------------------------------------------------------
// get / set / summary
// ---------------------------------------------------------------------------

/// Validate and apply a value to the in-memory config without persisting.
/// Returns the matched spec so callers can format feedback.
pub fn apply_value(
    config: &mut Config,
    key: &str,
    value: &str,
) -> Result<&'static FieldSpec, String> {
    let spec = find_spec(key).ok_or_else(|| unknown_key_message(key))?;
    let normalized = spec.kind.validate(strip_surrounding_quotes(value))?;
    (spec.set)(config, &normalized)?;
    Ok(spec)
}

/// Strip one pair of surrounding double quotes so values pasted from
/// config.toml work, and `\config set ai.endpoint ""` can clear a field.
fn strip_surrounding_quotes(value: &str) -> &str {
    let trimmed = value.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(trimmed)
}

/// Apply a value, persist the documented config file, and run side effects.
pub fn set_value(config: &mut Config, key: &str, value: &str) -> Result<String, String> {
    let spec = apply_value(config, key, value)?;
    persist(config)?;
    let note = apply_side_effects(config, spec.path);
    Ok(format!("{} = {}{}", spec.path, (spec.get)(config), note))
}

/// `\config get` — one key (bare value), `ssh_tunnel_patterns` (listing), or all keys.
pub fn get_value(config: &Config, key: Option<&str>) -> Result<String, String> {
    match key {
        None => {
            let mut out = String::new();
            for spec in SCHEMA {
                out.push_str(&format!("{} = {}\n", spec.path, masked_value(spec, config)));
            }
            for (pattern, target) in sorted_tunnel_patterns(config) {
                out.push_str(&format!(
                    "ssh_tunnel_patterns.\"{}\" = \"{}\"\n",
                    pattern,
                    sanitize_ssh_tunnel_string(&target)
                ));
            }
            Ok(out.trim_end().to_string())
        }
        Some("ssh_tunnel_patterns") => {
            let patterns = sorted_tunnel_patterns(config);
            if patterns.is_empty() {
                Ok("No SSH tunnel patterns configured. Use \\config to add one.".to_string())
            } else {
                Ok(patterns
                    .iter()
                    .map(|(p, t)| format!("\"{}\" = \"{}\"", p, sanitize_ssh_tunnel_string(t)))
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
        }
        Some(k) => match find_spec(k) {
            // Explicit single-key get shows the real value, even for sensitive fields.
            Some(spec) => Ok((spec.get)(config)),
            None => Err(unknown_key_message(k)),
        },
    }
}

/// Schema-driven read-only summary (`\config show`, non-TTY fallback).
pub fn render_summary(config: &Config) -> String {
    let config_path = Config::get_config_file_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "~/.config/dbcrust/config.toml".to_string());
    let mut out = format!("Configuration ({config_path})\n");
    out.push_str(
        "Use \\config for the interactive menu, \\config set <key> <value> to change a value.\n",
    );
    for section in ConfigSection::iter() {
        out.push_str(&format!("\n[{}]\n", section.label()));
        if section == ConfigSection::SshTunnelPatterns {
            let patterns = sorted_tunnel_patterns(config);
            if patterns.is_empty() {
                out.push_str("  (none configured)\n");
            }
            for (pattern, target) in patterns {
                out.push_str(&format!(
                    "  {}  ->  {}\n",
                    pattern,
                    sanitize_ssh_tunnel_string(&target)
                ));
            }
            continue;
        }
        for spec in SCHEMA.iter().filter(|s| s.section == section) {
            out.push_str(&format!(
                "  {:<38} = {}\n",
                spec.path,
                masked_value(spec, config)
            ));
        }
    }
    out.trim_end().to_string()
}

/// The field's value as listed: `***` for a set sensitive field.
pub fn masked_value(spec: &FieldSpec, config: &Config) -> String {
    let value = (spec.get)(config);
    if spec.sensitive && !value.is_empty() {
        "***".to_string()
    } else {
        value
    }
}

/// `ssh_tunnel_patterns` as `(pattern, target)` pairs, sorted by pattern.
pub fn sorted_tunnel_patterns(config: &Config) -> Vec<(String, String)> {
    let sorted: BTreeMap<_, _> = config.ssh_tunnel_patterns.iter().collect();
    sorted
        .into_iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Saves `config` with its documentation comments.
pub fn persist(config: &Config) -> Result<(), String> {
    config
        .save_with_documentation()
        .map_err(|e| format!("Failed to save configuration: {e}"))
}

/// Apply runtime side effects for a changed key; returns a note for the user.
fn apply_side_effects(config: &Config, path: &str) -> &'static str {
    reapply_runtime_setting(config, path);
    if takes_effect_next_session(path) {
        " (takes effect next session)"
    } else {
        ""
    }
}

/// Push one changed field into the process-wide runtime state (vector and
/// complex value formatting, the query timeout). Cheap: only the affected
/// section is copied; fields without runtime state are a no-op.
pub fn reapply_runtime_setting(config: &Config, path: &str) {
    if path.starts_with("vector_display.") {
        dbcrust_core::vector_display::set_global_vector_config(config.vector_display.clone());
    } else if path.starts_with("complex_display.") {
        dbcrust_core::complex_display::set_global_complex_config(config.complex_display.clone());
    } else if path == "query_timeout_seconds" {
        dbcrust_core::database::set_query_timeout_seconds(config.query_timeout_seconds);
    }
}

/// Whether a field is consumed when a session starts rather than live.
pub fn takes_effect_next_session(path: &str) -> bool {
    path.starts_with("logging.")
        || path.starts_with("history.")
        || matches!(
            path,
            "autocomplete_enabled" | "show_banner" | "multiline_prompt_indicator"
        )
}

/// Re-sync global runtime state from config (after `\config edit` reload).
pub fn reapply_runtime_settings(config: &Config) {
    dbcrust_core::vector_display::set_global_vector_config(config.vector_display.clone());
    dbcrust_core::complex_display::set_global_complex_config(config.complex_display.clone());
    dbcrust_core::database::set_query_timeout_seconds(config.query_timeout_seconds);
}

// ---------------------------------------------------------------------------
// Unknown-key suggestions
// ---------------------------------------------------------------------------

fn unknown_key_message(key: &str) -> String {
    let suggestions = suggest_keys(key);
    if suggestions.is_empty() {
        format!("Unknown configuration key \"{key}\". Use \\config get to list all keys.")
    } else {
        format!(
            "Unknown configuration key \"{key}\". Did you mean: {}?",
            suggestions.join(", ")
        )
    }
}

fn suggest_keys(key: &str) -> Vec<&'static str> {
    let lower = key.to_lowercase();
    let substring_hits: Vec<&'static str> = SCHEMA
        .iter()
        .map(|s| s.path)
        .filter(|p| p.to_lowercase().contains(&lower))
        .take(3)
        .collect();
    if !substring_hits.is_empty() {
        return substring_hits;
    }
    let mut scored: Vec<(usize, &'static str)> = SCHEMA
        .iter()
        .map(|s| (levenshtein(&lower, &s.path.to_lowercase()), s.path))
        .collect();
    scored.sort();
    scored
        .into_iter()
        .filter(|(d, p)| *d <= p.len().max(key.len()) / 2)
        .take(3)
        .map(|(_, p)| p)
        .collect()
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

// ---------------------------------------------------------------------------
// Tunnel pattern validation (pure, unit-tested; used by the submenu)
// ---------------------------------------------------------------------------

pub fn validate_tunnel_pattern(pattern: &str) -> Result<(), String> {
    if pattern.trim().is_empty() {
        return Err("pattern cannot be empty".to_string());
    }
    regex::Regex::new(pattern.trim())
        .map(|_| ())
        .map_err(|e| format!("invalid regex: {e}"))
}

/// Validate a tunnel target. Targets containing backticks run a command at
/// connect time and are accepted as-is — validation must never execute them.
pub fn validate_tunnel_target(config: &Config, target: &str) -> Result<(), String> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return Err("tunnel target cannot be empty".to_string());
    }
    if trimmed.contains('`') {
        return Ok(());
    }
    if config.parse_ssh_tunnel_string(trimmed).is_some() {
        Ok(())
    } else {
        Err("cannot parse tunnel target (expected [user[:password]@]host[:port])".to_string())
    }
}

/// Safely preview every matching host pattern, sorted for presentation.
/// Targets are returned as data; backtick commands are never evaluated.
pub fn matching_tunnel_patterns(config: &Config, host: &str) -> Vec<(String, String)> {
    let mut matches: Vec<_> = config
        .ssh_tunnel_patterns
        .iter()
        .filter(|(pattern, _)| regex::Regex::new(pattern).is_ok_and(|regex| regex.is_match(host)))
        .map(|(pattern, target)| (pattern.clone(), target.clone()))
        .collect();
    matches.sort_by(|a, b| a.0.cmp(&b.0));
    matches
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::collections::BTreeSet;

    fn leaf_paths(value: &toml::Value, prefix: &str, out: &mut Vec<String>) {
        match value {
            toml::Value::Table(table) => {
                for (key, child) in table {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    leaf_paths(child, &path, out);
                }
            }
            _ => out.push(prefix.to_string()),
        }
    }

    const EXCLUDED_PREFIXES: &[&str] = &["named_queries", "ssh_tunnel_patterns"];

    fn schema_paths() -> BTreeSet<String> {
        schema().iter().map(|s| s.path.to_string()).collect()
    }

    fn filtered_leaves(toml_text: &str) -> BTreeSet<String> {
        let value: toml::Value = toml::from_str(toml_text).expect("generated TOML must parse");
        let mut leaves = Vec::new();
        leaf_paths(&value, "", &mut leaves);
        leaves
            .into_iter()
            .filter(|p| {
                !EXCLUDED_PREFIXES
                    .iter()
                    .any(|ex| p == ex || p.starts_with(&format!("{ex}.")))
            })
            .collect()
    }

    /// A config where optional fields are populated so they serialize.
    fn fully_populated_config() -> Config {
        let mut config = Config::default();
        config.ai.endpoint = Some("http://localhost:11434".to_string());
        config.ssh_tunnel_patterns.insert(
            r"^db\.internal\..*$".to_string(),
            "user@jump:2222".to_string(),
        );
        config
    }

    #[test]
    fn test_config_tunnel_preview_never_executes_commands() {
        let mut config = Config::default_settings();
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("must-not-exist");
        config
            .ssh_tunnel_patterns
            .insert("^db".into(), format!("`touch {}`", marker.display()));
        config
            .ssh_tunnel_patterns
            .insert("internal$".into(), "user:secret@bastion".into());
        let matches = matching_tunnel_patterns(&config, "db.internal");
        assert_eq!(matches.len(), 2);
        for (_, target) in matches {
            validate_tunnel_target(&config, &target).unwrap();
        }
        assert!(!marker.exists());
    }

    #[rstest::rstest]
    #[case("logging.level", true)]
    #[case("history.cleanup_after_days", true)]
    #[case("autocomplete_enabled", true)]
    #[case("show_banner", true)]
    #[case("multiline_prompt_indicator", true)]
    #[case("query_timeout_seconds", false)]
    #[case("ai.temperature", false)]
    fn test_config_next_session_metadata(#[case] path: &str, #[case] expected: bool) {
        assert_eq!(takes_effect_next_session(path), expected);
    }

    #[test]
    fn test_schema_paths_are_unique() {
        let mut seen = BTreeSet::new();
        for spec in schema() {
            assert!(
                seen.insert(spec.path),
                "duplicate schema path: {}",
                spec.path
            );
        }
    }

    #[test]
    fn test_schema_matches_serialized_config() {
        let config = fully_populated_config();
        let toml_text = toml::to_string(&config).expect("config serializes");
        let toml_leaves = filtered_leaves(&toml_text);
        let schema_set = schema_paths();

        let missing_specs: Vec<_> = toml_leaves.difference(&schema_set).collect();
        let stale_specs: Vec<_> = schema_set.difference(&toml_leaves).collect();
        assert!(
            missing_specs.is_empty(),
            "Config fields without a FieldSpec in config_editor::SCHEMA: {missing_specs:?}. \
             Add a FieldSpec (and a save_with_documentation entry) for each."
        );
        assert!(
            stale_specs.is_empty(),
            "FieldSpecs without a matching Config field (renamed/removed?): {stale_specs:?}"
        );
    }

    #[test]
    fn test_documented_save_covers_schema() {
        let config = fully_populated_config();
        let documented = config.render_documented_config();
        let file_leaves = filtered_leaves(&documented);
        let schema_set = schema_paths();

        let missing_from_file: Vec<_> = schema_set.difference(&file_leaves).collect();
        let unknown_in_file: Vec<_> = file_leaves.difference(&schema_set).collect();
        assert!(
            missing_from_file.is_empty(),
            "Fields missing from save_with_documentation output: {missing_from_file:?}"
        );
        assert!(
            unknown_in_file.is_empty(),
            "save_with_documentation writes keys not in the schema (wrong section placement?): {unknown_in_file:?}"
        );

        // The tunnel table itself must round-trip.
        let value: toml::Value = toml::from_str(&documented).unwrap();
        let tunnels = value
            .get("ssh_tunnel_patterns")
            .and_then(|v| v.as_table())
            .expect("[ssh_tunnel_patterns] table present");
        assert_eq!(
            tunnels.get(r"^db\.internal\..*$").and_then(|v| v.as_str()),
            Some("user@jump:2222")
        );
    }

    #[test]
    fn test_vault_fields_are_root_level_in_documented_save() {
        let config = Config::default();
        let documented = config.render_documented_config();
        let value: toml::Value = toml::from_str(&documented).unwrap();
        assert!(
            value.get("vault_credential_cache_enabled").is_some(),
            "vault settings must be root-level keys"
        );
        assert!(
            value
                .get("ai")
                .and_then(|ai| ai.get("vault_credential_cache_enabled"))
                .is_none(),
            "vault settings must not land inside [ai]"
        );
    }

    #[test]
    fn test_vault_credential_process_documented_round_trip() {
        let mut config = Config::default();
        config.vault_credential_process =
            r#"helper --format "json" --script 'printf "token\\n"'"#.to_string();
        let documented = config.render_documented_config();
        let parsed: Config = toml::from_str(&documented).unwrap();
        assert_eq!(
            parsed.vault_credential_process,
            config.vault_credential_process
        );
    }

    #[test]
    fn test_every_enum_option_is_settable() {
        for spec in schema() {
            if let FieldKind::Enum(options) = spec.kind {
                for option in options {
                    let mut config = Config::default();
                    apply_value(&mut config, spec.path, option).unwrap_or_else(|e| {
                        panic!("option \"{option}\" rejected for {}: {e}", spec.path)
                    });
                    assert_eq!(
                        (spec.get)(&config),
                        *option,
                        "round-trip mismatch for {} = {option}",
                        spec.path
                    );
                }
            }
        }
    }

    #[rstest]
    #[case("default_limit", "50", "50")]
    #[case("default_limit", "0", "0")]
    #[case("show_banner", "on", "true")]
    #[case("show_banner", "OFF", "false")]
    #[case("expanded_display_default", "yes", "true")]
    #[case("autocomplete_enabled", "0", "false")]
    #[case("pager_command", "less -RFX", "less -RFX")]
    #[case("vault_cache_renewal_threshold", "0.5", "0.5")]
    #[case(
        "vault_credential_process",
        "credential-helper vault",
        "credential-helper vault"
    )]
    #[case("ai.temperature", "1.5", "1.5")]
    #[case("logging.level", "DEBUG", "debug")]
    #[case("vector_display.display_mode", "viz", "viz")]
    #[case("complex_display.display_mode", "Full", "full")]
    #[case("ai.execution_mode", "auto_select", "auto_select")]
    #[case("ai.endpoint", "http://localhost:11434", "http://localhost:11434")]
    #[case("multiline_prompt_indicator", "", "")]
    fn test_apply_then_get(#[case] key: &str, #[case] input: &str, #[case] expected: &str) {
        let mut config = Config::default();
        apply_value(&mut config, key, input).expect("value should apply");
        assert_eq!(get_value(&config, Some(key)).unwrap(), expected);
    }

    #[test]
    fn test_optional_text_clears_on_empty() {
        let mut config = Config::default();
        config.ai.endpoint = Some("http://x".to_string());
        apply_value(&mut config, "ai.endpoint", "").unwrap();
        assert_eq!(config.ai.endpoint, None);
    }

    #[rstest]
    #[case("default_limit", "abc")]
    #[case("default_limit", "-5")]
    #[case("query_timeout_seconds", "0")]
    #[case("vault_cache_renewal_threshold", "1.5")]
    #[case("show_banner", "banana")]
    #[case("ai.model", "")]
    fn test_invalid_values_rejected(#[case] key: &str, #[case] input: &str) {
        let mut config = Config::default();
        assert!(apply_value(&mut config, key, input).is_err());
    }

    #[test]
    fn test_invalid_enum_lists_options() {
        let mut config = Config::default();
        let err = apply_value(&mut config, "logging.level", "banana").unwrap_err();
        assert!(err.contains("trace, debug, info, warn, error"), "{err}");
    }

    #[test]
    fn test_unknown_key_suggests_closest() {
        let mut config = Config::default();
        let err = apply_value(&mut config, "loging.level", "debug").unwrap_err();
        assert!(err.contains("logging.level"), "{err}");
    }

    #[test]
    fn test_unknown_key_substring_match() {
        let err = unknown_key_message("level");
        assert!(err.contains("logging.level"), "{err}");
    }

    #[test]
    fn test_get_all_lists_schema_and_tunnels_sanitized() {
        let mut config = fully_populated_config();
        config.ssh_tunnel_patterns.insert(
            "^secret\\..*$".to_string(),
            "admin:hunter2@bastion:22".to_string(),
        );
        let output = get_value(&config, None).unwrap();
        assert!(output.contains("default_limit = 100"));
        assert!(output.contains(r#"ssh_tunnel_patterns."^secret\..*$""#));
        assert!(!output.contains("hunter2"), "passwords must be sanitized");
    }

    #[test]
    fn test_get_tunnel_patterns_listing() {
        let config = fully_populated_config();
        let output = get_value(&config, Some("ssh_tunnel_patterns")).unwrap();
        assert!(output.contains(r"^db\.internal\..*$"));
        assert!(output.contains("user@jump:2222"));
    }

    #[test]
    fn test_render_summary_sections_and_sanitization() {
        let mut config = fully_populated_config();
        config.ssh_tunnel_patterns.insert(
            "^p\\..*$".to_string(),
            "admin:hunter2@bastion:22".to_string(),
        );
        let summary = render_summary(&config);
        for section in ConfigSection::iter() {
            assert!(
                summary.contains(&format!("[{}]", section.label())),
                "summary missing section {}",
                section.label()
            );
        }
        assert!(!summary.contains("hunter2"));
    }

    #[rstest]
    #[case(r"^db\..*$", true)]
    #[case("", false)]
    #[case("([unclosed", false)]
    fn test_validate_tunnel_pattern(#[case] pattern: &str, #[case] ok: bool) {
        assert_eq!(validate_tunnel_pattern(pattern).is_ok(), ok);
    }

    #[rstest]
    #[case("user@host:2222", true)]
    #[case("host", true)]
    #[case("user:pass@host", true)]
    #[case("", false)]
    // Backtick targets must be accepted WITHOUT executing the command.
    #[case("user@`this-command-must-not-run`", true)]
    fn test_validate_tunnel_target(#[case] target: &str, #[case] ok: bool) {
        let config = Config::default();
        assert_eq!(validate_tunnel_target(&config, target).is_ok(), ok);
    }

    #[test]
    fn test_levenshtein() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("loging", "logging"), 1);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
    }
}
