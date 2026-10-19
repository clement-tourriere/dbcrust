//! The interactive `\config` menu, SSH tunnel manager and `$EDITOR`
//! round-trip: the `inquire` front end of the shared configuration schema.
//!
//! The schema itself (`\config get`/`set`, validation, side effects) lives in
//! `dbcrust_app::config_editor` so the desktop app uses the same table; it is
//! re-exported here under its historical path.

pub use dbcrust_app::config_editor::*;

use crate::config::Config;
use crate::password_sanitizer::sanitize_ssh_tunnel_string;
use inquire::{Confirm, InquireError, Select, Text};
use std::io::IsTerminal;
use strum::IntoEnumIterator;

// ---------------------------------------------------------------------------
// Interactive menu
// ---------------------------------------------------------------------------

/// True when stdin and stdout are TTYs and prompts are not disabled
/// (`--no-input`), i.e. inquire prompts can run.
pub fn can_run_interactive() -> bool {
    std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && crate::cli_core::prompts_allowed()
}

/// Navigation outcome of a nested prompt: Esc goes back, Ctrl-C exits the menu.
enum Nav {
    Back,
    Exit,
}

fn map_inquire_err(e: InquireError) -> Result<Nav, String> {
    match e {
        InquireError::OperationCanceled => Ok(Nav::Back),
        InquireError::OperationInterrupted => Ok(Nav::Exit),
        other => Err(format!("Prompt error: {other}")),
    }
}

struct Choice<T> {
    label: String,
    value: T,
}

impl<T> std::fmt::Display for Choice<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label)
    }
}

enum MenuAction {
    Section(ConfigSection),
    OpenEditor,
}

/// Top-level `\config` menu. Esc/Ctrl-C exits.
pub fn run_menu(config: &mut Config) -> Result<String, String> {
    loop {
        let mut items: Vec<Choice<MenuAction>> = ConfigSection::iter()
            .map(|section| Choice {
                label: format!("{:<24} ({})", section.label(), section.live_summary(config)),
                value: MenuAction::Section(section),
            })
            .collect();
        items.push(Choice {
            label: "Open config.toml in $EDITOR".to_string(),
            value: MenuAction::OpenEditor,
        });

        let selection = Select::new("Configuration — select a section:", items)
            .with_page_size(13)
            .with_help_message("↑↓ to move, Enter to open, Esc to exit")
            .prompt();

        match selection {
            Ok(choice) => match choice.value {
                MenuAction::Section(ConfigSection::SshTunnelPatterns) => {
                    if let Nav::Exit = run_tunnel_menu(config)? {
                        break;
                    }
                }
                MenuAction::Section(section) => {
                    if let Nav::Exit = run_section_menu(config, section)? {
                        break;
                    }
                }
                MenuAction::OpenEditor => {
                    let message = edit_in_editor(config)?;
                    println!("{message}");
                }
            },
            // Esc or Ctrl-C at the top level both exit the menu.
            Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => break,
            Err(e) => return Err(format!("Prompt error: {e}")),
        }
    }
    Ok("Configuration menu closed.".to_string())
}

fn run_section_menu(config: &mut Config, section: ConfigSection) -> Result<Nav, String> {
    loop {
        let items: Vec<Choice<&'static FieldSpec>> = schema()
            .iter()
            .filter(|s| s.section == section)
            .map(|spec| Choice {
                label: format!("{:<38} = {}", spec.path, masked_value(spec, config)),
                value: spec,
            })
            .collect();

        let selection = Select::new(&format!("{} — select a setting:", section.label()), items)
            .with_page_size(12)
            .with_help_message("Enter to edit, Esc to go back")
            .prompt();

        match selection {
            Ok(choice) => {
                if let Nav::Exit = edit_field(config, choice.value)? {
                    return Ok(Nav::Exit);
                }
            }
            Err(e) => return map_inquire_err(e),
        }
    }
}

fn edit_field(config: &mut Config, spec: &'static FieldSpec) -> Result<Nav, String> {
    let current = (spec.get)(config);
    println!("  {}", spec.help);

    let new_value = loop {
        let raw = match &spec.kind {
            FieldKind::Bool => Confirm::new(spec.label)
                .with_default(current == "true")
                .prompt()
                .map(|b| b.to_string()),
            FieldKind::Enum(options) => {
                let start = options.iter().position(|o| *o == current).unwrap_or(0);
                Select::new(spec.label, options.to_vec())
                    .with_starting_cursor(start)
                    .prompt()
                    .map(|s| s.to_string())
            }
            _ => Text::new(spec.label).with_initial_value(&current).prompt(),
        };

        match raw {
            Ok(input) => match spec.kind.validate(&input) {
                Ok(normalized) => break Some(normalized),
                Err(e) => {
                    println!("  ✗ {e}");
                    continue;
                }
            },
            Err(e) => match map_inquire_err(e)? {
                Nav::Back => break None,
                Nav::Exit => return Ok(Nav::Exit),
            },
        }
    };

    if let Some(value) = new_value {
        if value != current {
            let message = set_value(config, spec.path, &value)?;
            println!("  ✓ {message}");
        }
    }
    Ok(Nav::Back)
}

// ---------------------------------------------------------------------------
// SSH tunnel pattern submenu
// ---------------------------------------------------------------------------

enum TunnelAction {
    Add,
    Edit,
    Remove,
    Test,
}

fn run_tunnel_menu(config: &mut Config) -> Result<Nav, String> {
    loop {
        let patterns = sorted_tunnel_patterns(config);
        println!();
        if patterns.is_empty() {
            println!("No SSH tunnel patterns configured.");
        } else {
            println!("SSH tunnel patterns (regex  ->  tunnel target):");
            for (i, (pattern, target)) in patterns.iter().enumerate() {
                println!(
                    "  {}. {}  ->  {}",
                    i + 1,
                    pattern,
                    sanitize_ssh_tunnel_string(target)
                );
            }
        }

        let mut actions = vec![Choice {
            label: "Add a pattern".to_string(),
            value: TunnelAction::Add,
        }];
        if !patterns.is_empty() {
            actions.push(Choice {
                label: "Edit a pattern".to_string(),
                value: TunnelAction::Edit,
            });
            actions.push(Choice {
                label: "Remove a pattern".to_string(),
                value: TunnelAction::Remove,
            });
            actions.push(Choice {
                label: "Test a hostname against the patterns".to_string(),
                value: TunnelAction::Test,
            });
        }

        let selection = Select::new("SSH tunnel patterns:", actions)
            .with_help_message("Enter to select, Esc to go back")
            .prompt();

        let nav = match selection {
            Ok(choice) => match choice.value {
                TunnelAction::Add => tunnel_upsert(config, None)?,
                TunnelAction::Edit => match pick_tunnel_pattern(config, "Edit which pattern?")? {
                    Some(existing) => tunnel_upsert(config, Some(existing))?,
                    None => Nav::Back,
                },
                TunnelAction::Remove => tunnel_remove(config)?,
                TunnelAction::Test => tunnel_test(config)?,
            },
            Err(e) => return map_inquire_err(e),
        };
        if let Nav::Exit = nav {
            return Ok(Nav::Exit);
        }
    }
}

fn pick_tunnel_pattern(config: &Config, prompt: &str) -> Result<Option<(String, String)>, String> {
    let items: Vec<Choice<(String, String)>> = sorted_tunnel_patterns(config)
        .into_iter()
        .map(|(pattern, target)| Choice {
            label: format!("{}  ->  {}", pattern, sanitize_ssh_tunnel_string(&target)),
            value: (pattern, target),
        })
        .collect();
    match Select::new(prompt, items).prompt() {
        Ok(choice) => Ok(Some(choice.value)),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(e) => Err(format!("Prompt error: {e}")),
    }
}

/// Add a new pattern, or edit `existing` (pre-filled prompts, rename-aware).
fn tunnel_upsert(config: &mut Config, existing: Option<(String, String)>) -> Result<Nav, String> {
    let (initial_pattern, initial_target) = existing.clone().unwrap_or_default();

    let pattern = loop {
        let prompted = Text::new("Regex pattern matching database hosts:")
            .with_initial_value(&initial_pattern)
            .with_help_message(r"e.g. ^db\.internal\..*\.com$  — Esc to cancel")
            .prompt();
        match prompted {
            Ok(input) => match validate_tunnel_pattern(&input) {
                Ok(()) => break input.trim().to_string(),
                Err(e) => {
                    println!("  ✗ {e}");
                    continue;
                }
            },
            Err(e) => match map_inquire_err(e)? {
                Nav::Back => return Ok(Nav::Back),
                Nav::Exit => return Ok(Nav::Exit),
            },
        }
    };

    let target = loop {
        let prompted = Text::new("Tunnel target ([user[:password]@]host[:port]):")
            .with_initial_value(&initial_target)
            .with_help_message("backticks run a command, e.g. user@`get-bastion.sh`")
            .prompt();
        match prompted {
            Ok(input) => match validate_tunnel_target(config, &input) {
                Ok(()) => break input.trim().to_string(),
                Err(e) => {
                    println!("  ✗ {e}");
                    continue;
                }
            },
            Err(e) => match map_inquire_err(e)? {
                Nav::Back => return Ok(Nav::Back),
                Nav::Exit => return Ok(Nav::Exit),
            },
        }
    };

    let renaming_from = existing
        .as_ref()
        .map(|(old_pattern, _)| old_pattern.clone())
        .filter(|old_pattern| *old_pattern != pattern);
    let overwrites_other = config.ssh_tunnel_patterns.contains_key(&pattern)
        && existing.as_ref().map(|(p, _)| p.as_str()) != Some(pattern.as_str());
    if overwrites_other {
        match Confirm::new(&format!(
            "Pattern \"{pattern}\" already exists — overwrite?"
        ))
        .with_default(false)
        .prompt()
        {
            Ok(true) => {}
            Ok(false) => return Ok(Nav::Back),
            Err(e) => return map_inquire_err(e),
        }
    }

    if let Some(old_pattern) = renaming_from {
        config.ssh_tunnel_patterns.remove(&old_pattern);
    }
    config
        .ssh_tunnel_patterns
        .insert(pattern.clone(), target.clone());
    persist(config)?;
    println!(
        "  ✓ {}  ->  {}",
        pattern,
        sanitize_ssh_tunnel_string(&target)
    );
    Ok(Nav::Back)
}

fn tunnel_remove(config: &mut Config) -> Result<Nav, String> {
    let Some((pattern, _)) = pick_tunnel_pattern(config, "Remove which pattern?")? else {
        return Ok(Nav::Back);
    };
    match Confirm::new(&format!("Remove pattern \"{pattern}\"?"))
        .with_default(false)
        .prompt()
    {
        Ok(true) => {
            config.ssh_tunnel_patterns.remove(&pattern);
            persist(config)?;
            println!("  ✓ removed {pattern}");
            Ok(Nav::Back)
        }
        Ok(false) => Ok(Nav::Back),
        Err(e) => map_inquire_err(e),
    }
}

fn tunnel_test(config: &Config) -> Result<Nav, String> {
    let host = match Text::new("Hostname to test:")
        .with_help_message("e.g. db.internal.prod.example.com")
        .prompt()
    {
        Ok(h) => h.trim().to_string(),
        Err(e) => return map_inquire_err(e),
    };
    if host.is_empty() {
        return Ok(Nav::Back);
    }

    let matches: Vec<(String, String)> = sorted_tunnel_patterns(config)
        .into_iter()
        .filter(|(pattern, _)| {
            regex::Regex::new(pattern)
                .map(|re| re.is_match(&host))
                .unwrap_or(false)
        })
        .collect();

    if matches.is_empty() {
        println!("  No pattern matches \"{host}\" — no tunnel would be opened.");
        return Ok(Nav::Back);
    }
    for (pattern, target) in &matches {
        println!(
            "  ✓ {}  ->  {}",
            pattern,
            sanitize_ssh_tunnel_string(target)
        );
    }
    if matches.len() > 1 {
        println!(
            "  ⚠ {} patterns match — pattern order is not guaranteed, the first match wins at connect time.",
            matches.len()
        );
    }

    let needs_command_execution = matches.iter().any(|(_, target)| target.contains('`'));
    let resolve = if needs_command_execution {
        match Confirm::new("Resolve the target now? (executes the backtick command substitution)")
            .with_default(false)
            .prompt()
        {
            Ok(answer) => answer,
            Err(e) => return map_inquire_err(e),
        }
    } else {
        true
    };

    if resolve {
        match config.get_ssh_tunnel_for_host(&host) {
            Some(tunnel) => println!(
                "  Tunnel: {}@{}:{}{}",
                tunnel.ssh_username.as_deref().unwrap_or("(current user)"),
                tunnel.ssh_host,
                tunnel.ssh_port,
                if tunnel.ssh_password.is_some() {
                    "  (password: ***)"
                } else {
                    ""
                }
            ),
            None => println!("  Failed to resolve the tunnel target (see errors above)."),
        }
    }
    Ok(Nav::Back)
}

// ---------------------------------------------------------------------------
// $EDITOR round-trip
// ---------------------------------------------------------------------------

/// Open config.toml in $EDITOR, then reload it and re-sync runtime settings.
pub fn edit_in_editor(config: &mut Config) -> Result<String, String> {
    let path = Config::get_config_directory()
        .map_err(|e| format!("Cannot determine config directory: {e}"))?
        .join("config.toml");
    if !path.exists() {
        persist(config)?;
    }

    let editor = crate::script::resolve_editor();
    println!("Opening {} in {editor}...", path.display());
    // $EDITOR may carry arguments ("code --wait"); split them off.
    let mut parts = editor.split_whitespace();
    let program = parts
        .next()
        .ok_or_else(|| "EDITOR is set but empty".to_string())?;
    let status = std::process::Command::new(program)
        .args(parts)
        .arg(&path)
        .status()
        .map_err(|e| format!("Failed to launch editor '{editor}': {e}"))?;
    if !status.success() {
        return Err(format!("Editor '{editor}' exited with non-zero status"));
    }

    *config = Config::load();
    reapply_runtime_settings(config);
    Ok(format!(
        "Configuration reloaded from {} ({} SSH tunnel pattern{}).\n\
         Logging, history, autocomplete and banner changes take effect next session.",
        path.display(),
        config.ssh_tunnel_patterns.len(),
        if config.ssh_tunnel_patterns.len() == 1 {
            ""
        } else {
            "s"
        }
    ))
}
