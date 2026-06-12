//! Self-update support (`dbcrust --update`).
//!
//! dbcrust ships through several channels (uv tool, pipx, pip, cargo,
//! standalone binaries via the install script). The running executable's
//! path identifies the channel; the latest release tag comes from the GitHub
//! API. Package-manager installs are upgraded in place after confirmation;
//! for the other channels the exact command is printed instead of executed
//! (re-running `curl | sh` or a cargo build on the user's behalf is more
//! surprising than helpful).

use std::io::Write;
use std::path::Path;

/// How the running dbcrust was installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallChannel {
    UvTool,
    Pipx,
    Pip,
    Cargo,
    Binary,
}

impl std::fmt::Display for InstallChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            InstallChannel::UvTool => "uv tool",
            InstallChannel::Pipx => "pipx",
            InstallChannel::Pip => "pip",
            InstallChannel::Cargo => "cargo (source build)",
            InstallChannel::Binary => "standalone binary",
        };
        write!(f, "{name}")
    }
}

/// What `--update` should do for a given channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpgradeAction {
    /// A command we can run directly (argv form).
    Run(Vec<String>),
    /// Shell command(s) the user should run themselves.
    Manual(String),
}

/// Classify the install channel from the running executable's path.
///
/// Python-based installs (uv tool, pipx, pip) run dbcrust as a console
/// script, so the process executable is the environment's `python` — the
/// surrounding directory names are what identify the channel.
pub fn detect_install_channel(exe: &Path) -> InstallChannel {
    let parts: Vec<String> = exe
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    let has = |name: &str| parts.iter().any(|p| p == name);
    let has_pair = |a: &str, b: &str| parts.windows(2).any(|w| w[0] == a && w[1] == b);

    if has_pair("uv", "tools") {
        return InstallChannel::UvTool;
    }
    if has("pipx") {
        return InstallChannel::Pipx;
    }

    let file_name = exe
        .file_name()
        .map(|f| f.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if file_name.starts_with("python") || has("site-packages") || has(".venv") || has("venv") {
        return InstallChannel::Pip;
    }
    if has_pair(".cargo", "bin") {
        return InstallChannel::Cargo;
    }
    InstallChannel::Binary
}

/// The upgrade for a channel. `exe` is the running executable — for pip
/// installs that is the environment's Python interpreter, which lets the
/// upgrade target the exact environment dbcrust runs from.
pub fn upgrade_action(channel: &InstallChannel, exe: &Path, repo_url: &str) -> UpgradeAction {
    let argv = |parts: &[&str]| parts.iter().map(|s| s.to_string()).collect();
    match channel {
        InstallChannel::UvTool => UpgradeAction::Run(argv(&["uv", "tool", "upgrade", "dbcrust"])),
        InstallChannel::Pipx => UpgradeAction::Run(argv(&["pipx", "upgrade", "dbcrust"])),
        InstallChannel::Pip => {
            let file_name = exe
                .file_name()
                .map(|f| f.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if file_name.starts_with("python") {
                UpgradeAction::Run(vec![
                    exe.to_string_lossy().to_string(),
                    "-m".to_string(),
                    "pip".to_string(),
                    "install".to_string(),
                    "--upgrade".to_string(),
                    "dbcrust".to_string(),
                ])
            } else {
                UpgradeAction::Manual("pip install --upgrade dbcrust".to_string())
            }
        }
        InstallChannel::Cargo => UpgradeAction::Manual(format!(
            "cargo install --git {repo_url} dbcrust --locked --force\n  (or from a checkout: mise run install)"
        )),
        InstallChannel::Binary => UpgradeAction::Manual(
            "curl -fsSL https://clement-tourriere.github.io/dbcrust/install.sh | sh".to_string(),
        ),
    }
}

/// Numeric prefix of a version string: `v0.28.0-rc1` → `[0, 28, 0]`.
fn version_key(version: &str) -> Vec<u64> {
    version
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['.', '-', '+'])
        .map_while(|part| part.parse::<u64>().ok())
        .collect()
}

/// Lexicographic compare of numeric segments; pre-release suffixes are ignored.
pub fn is_newer(latest: &str, current: &str) -> bool {
    version_key(latest) > version_key(current)
}

fn repo_url() -> &'static str {
    env!("CARGO_PKG_REPOSITORY")
}

fn repo_slug(repo_url: &str) -> &str {
    repo_url
        .trim_start_matches("https://github.com/")
        .trim_end_matches('/')
}

async fn fetch_latest_version(slug: &str) -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{slug}/releases/latest");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(&url)
        .header(
            reqwest::header::USER_AGENT,
            concat!("dbcrust/", env!("CARGO_PKG_VERSION")),
        )
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("GitHub API returned HTTP {}", response.status()));
    }
    #[derive(serde::Deserialize)]
    struct Release {
        tag_name: String,
    }
    let release: Release = response.json().await.map_err(|e| e.to_string())?;
    Ok(release.tag_name.trim_start_matches(['v', 'V']).to_string())
}

/// Entry point for `dbcrust --update`. Returns the process exit code.
pub async fn run_update() -> i32 {
    let current = env!("CARGO_PKG_VERSION");
    let slug = repo_slug(repo_url());

    print!("dbcrust {current} — checking {slug} for updates... ");
    let _ = std::io::stdout().flush();

    match fetch_latest_version(slug).await {
        Ok(latest) if is_newer(&latest, current) => {
            println!("{latest} is available.");
        }
        Ok(latest) => {
            println!("already up to date (latest release: {latest}).");
            return 0;
        }
        Err(e) => {
            println!("check failed: {e}");
            println!("Continuing with upgrade instructions for your install.\n");
        }
    }

    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Could not determine the running executable: {e}");
            return 1;
        }
    };
    let channel = detect_install_channel(&exe);
    println!("Detected install method: {channel}");

    match upgrade_action(&channel, &exe, repo_url()) {
        UpgradeAction::Run(argv) => {
            println!("Upgrade command: {}", argv.join(" "));
            let confirmed = inquire::Confirm::new("Run it now?")
                .with_default(true)
                .prompt()
                .unwrap_or(false);
            if !confirmed {
                println!("Skipped — run the command above when ready.");
                return 0;
            }
            match std::process::Command::new(&argv[0])
                .args(&argv[1..])
                .status()
            {
                Ok(status) if status.success() => {
                    println!(
                        "dbcrust updated — restart your shell or rerun dbcrust to use the new version."
                    );
                    0
                }
                Ok(status) => {
                    eprintln!("Upgrade command exited with {status}.");
                    1
                }
                Err(e) => {
                    eprintln!("Failed to launch '{}': {e}", argv[0]);
                    1
                }
            }
        }
        UpgradeAction::Manual(instructions) => {
            println!("Update with:\n  {instructions}");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(
        "/Users/me/.local/share/uv/tools/dbcrust/bin/python3.12",
        InstallChannel::UvTool
    )]
    #[case("/home/me/.local/pipx/venvs/dbcrust/bin/python", InstallChannel::Pipx)]
    #[case("/usr/bin/python3", InstallChannel::Pip)]
    #[case("/work/proj/.venv/bin/python", InstallChannel::Pip)]
    #[case(
        "/opt/app/venv/lib/site-packages/something/python3",
        InstallChannel::Pip
    )]
    #[case("/home/me/.cargo/bin/dbcrust", InstallChannel::Cargo)]
    #[case("/home/me/.local/bin/dbcrust", InstallChannel::Binary)]
    #[case("/usr/local/bin/dbc", InstallChannel::Binary)]
    fn test_detect_install_channel(#[case] path: &str, #[case] expected: InstallChannel) {
        assert_eq!(detect_install_channel(Path::new(path)), expected);
    }

    #[rstest]
    #[case("0.28.0", "0.27.1", true)]
    #[case("v0.27.2", "0.27.1", true)]
    #[case("0.27.1", "0.27.1", false)]
    #[case("0.27.0", "0.27.1", false)]
    #[case("1.0.0", "0.99.9", true)]
    #[case("0.27.1", "0.27", true)]
    #[case("0.27", "0.27.1", false)]
    #[case("0.28.0-rc1", "0.27.1", true)]
    fn test_is_newer(#[case] latest: &str, #[case] current: &str, #[case] expected: bool) {
        assert_eq!(is_newer(latest, current), expected);
    }

    #[test]
    fn test_pip_upgrade_uses_running_interpreter() {
        let exe = Path::new("/work/proj/.venv/bin/python");
        match upgrade_action(&InstallChannel::Pip, exe, "https://github.com/x/y") {
            UpgradeAction::Run(argv) => {
                assert_eq!(argv[0], "/work/proj/.venv/bin/python");
                assert!(argv.contains(&"--upgrade".to_string()));
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn test_uv_tool_upgrade_command() {
        let exe = Path::new("/Users/me/.local/share/uv/tools/dbcrust/bin/python3.12");
        assert_eq!(
            upgrade_action(&InstallChannel::UvTool, exe, "https://github.com/x/y"),
            UpgradeAction::Run(vec![
                "uv".to_string(),
                "tool".to_string(),
                "upgrade".to_string(),
                "dbcrust".to_string()
            ])
        );
    }

    #[test]
    fn test_repo_slug() {
        assert_eq!(
            repo_slug("https://github.com/clement-tourriere/dbcrust"),
            "clement-tourriere/dbcrust"
        );
        // The actual Cargo.toml repository field must yield a usable slug
        assert!(repo_slug(repo_url()).contains('/'));
    }
}
