//! `dbcrust broker install-service` / `uninstall-service`: run the broker as
//! a system daemon under its own OS user.
//!
//! Everything that decides *what* to do is a pure function so it can be
//! snapshot-tested without root:
//!
//! * [`plan_install`] / [`plan_uninstall`] turn options plus [`HostFacts`]
//!   into an ordered list of [`Step`]s;
//! * [`launchd_plist`], [`systemd_unit`] and [`broker_toml`] render the files;
//! * [`render_plan`] prints a plan (the `--dry-run` output).
//!
//! Executing a plan goes through the [`Executor`] trait: [`SystemExecutor`]
//! really runs `dscl`/`launchctl`/`useradd`/`systemctl` and touches the file
//! system; the tests use a recording fake.
//!
//! Layout (macOS / Linux):
//!
//! | what | macOS | Linux |
//! |---|---|---|
//! | service user | `_dbcrust` (hidden) | `dbcrust` (system) |
//! | client group | `dbcrust-agents` | `dbcrust-agents` |
//! | binary (root-owned) | `/usr/local/libexec/dbcrust-broker` | same |
//! | state (0700, service user) | `/Library/Application Support/dbcrust-broker` | `/var/lib/dbcrust-broker` |
//! | socket | `/var/run/dbcrust/broker.sock` | `/run/dbcrust/broker.sock` |
//! | definition | `/Library/LaunchDaemons/dev.dbcrust.broker.plist` | `/etc/systemd/system/dbcrust-broker.service` |
//!
//! macOS cannot start the daemon as `_dbcrust` directly: `/var/run` is wiped
//! at boot and only root can recreate `/var/run/dbcrust`. The launchd job
//! therefore starts as root and the broker (`serve --drop-to _dbcrust`) binds
//! the socket, then drops to the service user and proves the drop.

use super::BrokerError;
use super::paths::{BrokerSettings, effective_uid};
use super::privileges::{
    Credentials, INSTALLED_BINARY, binary_trust_violations, group_id, user_ids, user_in_group,
};
use super::server::{chown, clear_stale_socket};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const AGENTS_GROUP: &str = "dbcrust-agents";
pub const LAUNCHD_LABEL: &str = "dev.dbcrust.broker";
pub const LAUNCHD_PLIST: &str = "/Library/LaunchDaemons/dev.dbcrust.broker.plist";
pub const LAUNCHD_LOG: &str = "/Library/Logs/dbcrust-broker.log";
pub const SYSTEMD_UNIT_NAME: &str = "dbcrust-broker.service";
pub const SYSTEMD_UNIT: &str = "/etc/systemd/system/dbcrust-broker.service";
const LIBEXEC_DIR: &str = "/usr/local/libexec";
/// macOS system-account id range the installer allocates from.
const MAC_ID_RANGE: std::ops::RangeInclusive<u32> = 200..=400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    MacOs,
    Linux,
}

impl Platform {
    pub fn current() -> Option<Self> {
        if cfg!(target_os = "macos") {
            Some(Platform::MacOs)
        } else if cfg!(target_os = "linux") {
            Some(Platform::Linux)
        } else {
            None
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "macos" | "darwin" | "mac" => Some(Platform::MacOs),
            "linux" => Some(Platform::Linux),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Platform::MacOs => "macOS (launchd)",
            Platform::Linux => "Linux (systemd)",
        }
    }

    pub fn service_user(self) -> &'static str {
        match self {
            Platform::MacOs => "_dbcrust",
            Platform::Linux => "dbcrust",
        }
    }

    /// The service user's primary group (same name as the user).
    pub fn service_group(self) -> &'static str {
        self.service_user()
    }

    pub fn state_dir(self) -> &'static str {
        match self {
            Platform::MacOs => "/Library/Application Support/dbcrust-broker",
            Platform::Linux => "/var/lib/dbcrust-broker",
        }
    }

    pub fn socket(self) -> &'static str {
        match self {
            Platform::MacOs => "/var/run/dbcrust/broker.sock",
            Platform::Linux => "/run/dbcrust/broker.sock",
        }
    }

    pub fn definition_path(self) -> &'static str {
        match self {
            Platform::MacOs => LAUNCHD_PLIST,
            Platform::Linux => SYSTEMD_UNIT,
        }
    }

    /// root's primary group name.
    fn root_group(self) -> &'static str {
        match self {
            Platform::MacOs => "wheel",
            Platform::Linux => "root",
        }
    }
}

// ---- generated files -----------------------------------------------------------

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The launchd daemon definition.
///
/// No `UserName` key: the job starts as root so the broker can create the
/// socket directory under `/var/run`, then drops to `service_user` itself
/// (`--drop-to`). The environment is minimal on purpose.
pub fn launchd_plist(binary: &str, service_user: &str, log_path: &str) -> String {
    let arguments = [
        binary,
        "broker",
        "serve",
        "--mode",
        "service",
        "--drop-to",
        service_user,
    ]
    .iter()
    .map(|a| format!("        <string>{}</string>\n", xml_escape(a)))
    .collect::<String>();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
{arguments}    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/usr/bin:/bin</string>
    </dict>
</dict>
</plist>
"#,
        label = LAUNCHD_LABEL,
        log = xml_escape(log_path),
    )
}

/// The systemd unit. The service user starts the process directly;
/// `RuntimeDirectory` creates `/run/dbcrust` and the broker hands it (and the
/// socket) to `dbcrust-agents` at bind time.
pub fn systemd_unit(binary: &str, user: &str, socket: &str) -> String {
    format!(
        "[Unit]
Description=DBCrust credential broker for AI agents
Documentation=https://clement-tourriere.github.io/dbcrust/user-guide/agent-connections/
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User={user}
Group={user}
SupplementaryGroups={AGENTS_GROUP}
ExecStart={binary} broker serve --mode service --socket {socket}
StateDirectory=dbcrust-broker
StateDirectoryMode=0700
RuntimeDirectory=dbcrust
RuntimeDirectoryMode=0750
UMask=0077
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectControlGroups=yes
RestrictSUIDSGID=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
"
    )
}

/// `broker.toml` written on first install: the socket's group.
pub fn broker_toml() -> String {
    format!(
        "# DBCrust broker settings (service mode). Unknown keys are an error.\n\
         # Group allowed to connect to the socket.\n\
         socket_group = \"{AGENTS_GROUP}\"\n"
    )
}

// ---- plan model ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cmd {
    pub program: String,
    pub args: Vec<String>,
}

impl Cmd {
    pub fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
        }
    }

    /// Shell-style rendering for dry-run output and error messages.
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .map(shell_quote)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-=:@+,%".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    pub user: String,
    pub group: String,
}

impl Owner {
    fn new(user: &str, group: &str) -> Self {
        Self {
            user: user.to_string(),
            group: group.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Printed in dry-run output only.
    Note(String),
    Run {
        description: String,
        command: Cmd,
        /// A probe whose success means "already done".
        skip_if: Option<Cmd>,
        /// A failure is reported but does not stop the plan.
        tolerate_failure: bool,
        /// Extra attempts, one second apart (launchd unload races).
        retries: u8,
    },
    MakeDir {
        path: PathBuf,
        mode: u32,
        owner: Owner,
        /// Leave an existing directory's owner and mode alone.
        create_only: bool,
    },
    WriteFile {
        path: PathBuf,
        contents: String,
        mode: u32,
        owner: Owner,
        only_if_missing: bool,
    },
    InstallBinary {
        from: PathBuf,
        to: PathBuf,
        owner: Owner,
    },
    /// The trusted-binary rule applied to the installed copy.
    VerifyTrusted {
        path: PathBuf,
    },
    RemoveFile {
        path: PathBuf,
    },
    RemoveDirAll {
        path: PathBuf,
    },
}

impl Step {
    fn run(description: &str, command: Cmd) -> Self {
        Step::Run {
            description: description.to_string(),
            command,
            skip_if: None,
            tolerate_failure: false,
            retries: 0,
        }
    }

    fn run_tolerant(description: &str, command: Cmd) -> Self {
        Step::Run {
            description: description.to_string(),
            command,
            skip_if: None,
            tolerate_failure: true,
            retries: 0,
        }
    }
}

/// What the plan builder needs to know about the machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostFacts {
    /// gid of the service user's primary group, if it exists.
    pub service_group_gid: Option<u32>,
    pub service_user_exists: bool,
    pub agents_group_exists: bool,
    /// Ids in use (macOS allocation).
    pub used_ids: BTreeSet<u32>,
}

impl HostFacts {
    /// Read the real machine. On macOS this lists `dscl` ids (read-only).
    pub fn probe(platform: Platform) -> Self {
        let used_ids = if platform == Platform::MacOs {
            let mut ids = BTreeSet::new();
            for (class, attribute) in [("/Users", "UniqueID"), ("/Groups", "PrimaryGroupID")] {
                if let Some(text) = capture("dscl", &[".", "-list", class, attribute]) {
                    ids.extend(parse_dscl_ids(&text));
                }
            }
            ids
        } else {
            BTreeSet::new()
        };
        Self {
            service_group_gid: group_id(platform.service_group()),
            service_user_exists: user_ids(platform.service_user()).is_some(),
            agents_group_exists: group_id(AGENTS_GROUP).is_some(),
            used_ids,
        }
    }
}

/// `dscl . -list /Users UniqueID` lines are `name   id`.
pub fn parse_dscl_ids(output: &str) -> Vec<u32> {
    output
        .lines()
        .filter_map(|line| line.split_whitespace().last()?.parse::<u32>().ok())
        .collect()
}

/// `count` ids from the macOS system range that nothing uses, highest
/// first (low ids are where Apple's own daemons grow).
pub fn pick_free_ids(used: &BTreeSet<u32>, count: usize) -> Option<Vec<u32>> {
    let free: Vec<u32> = MAC_ID_RANGE
        .rev()
        .filter(|id| !used.contains(id))
        .take(count)
        .collect();
    (free.len() == count).then_some(free)
}

/// Account names go into command arguments: keep them boring.
pub fn validate_account_name(name: &str) -> Result<(), BrokerError> {
    let ok = !name.is_empty()
        && name.len() <= 32
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if ok {
        Ok(())
    } else {
        Err(BrokerError::Invalid(format!(
            "{name:?} is not a plain account name (letters, digits, `_`, `-`, `.`)"
        )))
    }
}

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub platform: Platform,
    /// The binary being installed (this process's executable).
    pub source_binary: PathBuf,
    /// A user to put into `dbcrust-agents`.
    pub add_user: Option<String>,
}

fn dscl_create(record: &str, key: &str, value: &str) -> Step {
    Step::run(
        &format!("set {key} of {record}"),
        Cmd::new("dscl", &[".", "-create", record, key, value]),
    )
}

fn dscl_record(record: &str) -> Step {
    Step::run(
        &format!("create {record}"),
        Cmd::new("dscl", &[".", "-create", record]),
    )
}

/// Everything `install-service` does, in order.
pub fn plan_install(opts: &InstallOptions, facts: &HostFacts) -> Result<Vec<Step>, BrokerError> {
    let platform = opts.platform;
    if let Some(user) = &opts.add_user {
        validate_account_name(user)?;
    }
    let service_user = platform.service_user();
    let service_group = platform.service_group();
    let service_owner = Owner::new(service_user, service_group);
    let root_owner = Owner::new("root", platform.root_group());
    let mut steps = Vec::new();

    // 1. identities
    match platform {
        Platform::MacOs => {
            let needs_service_group = facts.service_group_gid.is_none();
            let needs_agents_group = !facts.agents_group_exists;
            // A user whose group already exists still needs a uid of its own
            let needs_user_id = !facts.service_user_exists && !needs_service_group;
            let wanted = usize::from(needs_service_group)
                + usize::from(needs_agents_group)
                + usize::from(needs_user_id);
            let mut ids = pick_free_ids(&facts.used_ids, wanted)
                .ok_or_else(|| {
                    BrokerError::State(format!(
                        "no free id in {}–{} for the service account; free one or create the \
                         `{service_user}` user and `{AGENTS_GROUP}` group by hand",
                        MAC_ID_RANGE.start(),
                        MAC_ID_RANGE.end()
                    ))
                })?
                .into_iter();
            let service_gid = match facts.service_group_gid {
                Some(gid) => {
                    steps.push(Step::Note(format!(
                        "group {service_group} already exists (gid {gid})"
                    )));
                    gid
                }
                None => {
                    let gid = ids.next().expect("counted above");
                    let record = format!("/Groups/{service_group}");
                    steps.push(dscl_record(&record));
                    steps.push(dscl_create(&record, "PrimaryGroupID", &gid.to_string()));
                    steps.push(dscl_create(
                        &record,
                        "RealName",
                        "DBCrust credential broker",
                    ));
                    steps.push(dscl_create(&record, "Password", "*"));
                    gid
                }
            };
            if facts.agents_group_exists {
                steps.push(Step::Note(format!("group {AGENTS_GROUP} already exists")));
            } else {
                let gid = ids.next().expect("counted above");
                let record = format!("/Groups/{AGENTS_GROUP}");
                steps.push(dscl_record(&record));
                steps.push(dscl_create(&record, "PrimaryGroupID", &gid.to_string()));
                steps.push(dscl_create(
                    &record,
                    "RealName",
                    "Users allowed to use the DBCrust broker",
                ));
                steps.push(dscl_create(&record, "Password", "*"));
            }
            if facts.service_user_exists {
                steps.push(Step::Note(format!("user {service_user} already exists")));
            } else {
                // Same number for uid and gid when the group was just created
                let uid = if needs_service_group {
                    service_gid
                } else {
                    ids.next().expect("counted above")
                };
                let record = format!("/Users/{service_user}");
                steps.push(dscl_record(&record));
                steps.push(dscl_create(&record, "UniqueID", &uid.to_string()));
                steps.push(dscl_create(
                    &record,
                    "PrimaryGroupID",
                    &service_gid.to_string(),
                ));
                steps.push(dscl_create(&record, "UserShell", "/usr/bin/false"));
                steps.push(dscl_create(&record, "NFSHomeDirectory", "/var/empty"));
                steps.push(dscl_create(
                    &record,
                    "RealName",
                    "DBCrust credential broker",
                ));
                steps.push(dscl_create(&record, "IsHidden", "1"));
                steps.push(dscl_create(&record, "Password", "*"));
            }
            if let Some(user) = &opts.add_user {
                steps.push(Step::Run {
                    description: format!("add {user} to {AGENTS_GROUP}"),
                    command: Cmd::new(
                        "dseditgroup",
                        &["-o", "edit", "-a", user, "-t", "user", AGENTS_GROUP],
                    ),
                    skip_if: Some(Cmd::new(
                        "dseditgroup",
                        &["-o", "checkmember", "-m", user, AGENTS_GROUP],
                    )),
                    tolerate_failure: false,
                    retries: 0,
                });
            }
        }
        Platform::Linux => {
            if facts.agents_group_exists {
                steps.push(Step::Note(format!("group {AGENTS_GROUP} already exists")));
            } else {
                steps.push(Step::run(
                    &format!("create group {AGENTS_GROUP}"),
                    Cmd::new("groupadd", &["--system", AGENTS_GROUP]),
                ));
            }
            if facts.service_user_exists {
                steps.push(Step::Note(format!("user {service_user} already exists")));
            } else {
                let group_arg = if facts.service_group_gid.is_some() {
                    ["-g", service_group]
                } else {
                    ["--user-group", ""]
                };
                let mut args = vec![
                    "--system",
                    "--no-create-home",
                    "--shell",
                    "/usr/sbin/nologin",
                    "--comment",
                    "DBCrust credential broker",
                ];
                args.extend(group_arg.iter().copied().filter(|a| !a.is_empty()));
                args.push(service_user);
                steps.push(Step::run(
                    &format!("create system user {service_user}"),
                    Cmd::new("useradd", &args),
                ));
            }
            if let Some(user) = &opts.add_user {
                steps.push(Step::run(
                    &format!("add {user} to {AGENTS_GROUP}"),
                    Cmd::new("usermod", &["-aG", AGENTS_GROUP, user]),
                ));
            }
        }
    }

    // 2. the root-owned binary
    let binary = PathBuf::from(INSTALLED_BINARY);
    steps.push(Step::MakeDir {
        path: PathBuf::from(LIBEXEC_DIR),
        mode: 0o755,
        owner: root_owner.clone(),
        create_only: true,
    });
    steps.push(Step::InstallBinary {
        from: opts.source_binary.clone(),
        to: binary.clone(),
        owner: root_owner.clone(),
    });
    steps.push(Step::VerifyTrusted { path: binary });

    // 3. state: only created / given settings when missing; the store is
    // never touched
    let state_dir = PathBuf::from(platform.state_dir());
    steps.push(Step::MakeDir {
        path: state_dir.clone(),
        mode: 0o700,
        owner: service_owner.clone(),
        create_only: false,
    });
    steps.push(Step::WriteFile {
        path: state_dir.join(super::paths::BROKER_CONFIG_FILE),
        contents: broker_toml(),
        mode: 0o600,
        owner: service_owner,
        only_if_missing: true,
    });

    // 4. the daemon
    match platform {
        Platform::MacOs => {
            steps.push(Step::WriteFile {
                path: PathBuf::from(LAUNCHD_PLIST),
                contents: launchd_plist(INSTALLED_BINARY, service_user, LAUNCHD_LOG),
                mode: 0o644,
                owner: root_owner,
                only_if_missing: false,
            });
            let target = format!("system/{LAUNCHD_LABEL}");
            steps.push(Step::run_tolerant(
                "unload a previous version of the daemon (fails harmlessly when none)",
                Cmd::new("launchctl", &["bootout", &target]),
            ));
            steps.push(Step::run_tolerant(
                "make sure the daemon is not disabled",
                Cmd::new("launchctl", &["enable", &target]),
            ));
            steps.push(Step::Run {
                description: "load and start the daemon".to_string(),
                command: Cmd::new("launchctl", &["bootstrap", "system", LAUNCHD_PLIST]),
                skip_if: None,
                tolerate_failure: false,
                retries: 3,
            });
        }
        Platform::Linux => {
            steps.push(Step::WriteFile {
                path: PathBuf::from(SYSTEMD_UNIT),
                contents: systemd_unit(INSTALLED_BINARY, service_user, platform.socket()),
                mode: 0o644,
                owner: root_owner,
                only_if_missing: false,
            });
            steps.push(Step::run(
                "reload systemd",
                Cmd::new("systemctl", &["daemon-reload"]),
            ));
            steps.push(Step::run(
                "start the daemon at boot",
                Cmd::new("systemctl", &["enable", "dbcrust-broker"]),
            ));
            steps.push(Step::run(
                "start (or restart onto the new binary) the daemon",
                Cmd::new("systemctl", &["restart", "dbcrust-broker"]),
            ));
        }
    }
    Ok(steps)
}

/// Everything `uninstall-service` does, in order. The store is only removed
/// with `purge`.
pub fn plan_uninstall(platform: Platform, purge: bool) -> Vec<Step> {
    let mut steps = Vec::new();
    match platform {
        Platform::MacOs => {
            steps.push(Step::run_tolerant(
                "stop and unload the daemon (fails harmlessly when not loaded)",
                Cmd::new(
                    "launchctl",
                    &["bootout", &format!("system/{LAUNCHD_LABEL}")],
                ),
            ));
            steps.push(Step::RemoveFile {
                path: PathBuf::from(LAUNCHD_PLIST),
            });
            steps.push(Step::RemoveFile {
                path: PathBuf::from(LAUNCHD_LOG),
            });
            steps.push(Step::RemoveDirAll {
                path: PathBuf::from("/var/run/dbcrust"),
            });
        }
        Platform::Linux => {
            steps.push(Step::run_tolerant(
                "stop and disable the daemon (fails harmlessly when not installed)",
                Cmd::new("systemctl", &["disable", "--now", "dbcrust-broker"]),
            ));
            steps.push(Step::RemoveFile {
                path: PathBuf::from(SYSTEMD_UNIT),
            });
            steps.push(Step::run_tolerant(
                "reload systemd",
                Cmd::new("systemctl", &["daemon-reload"]),
            ));
        }
    }
    steps.push(Step::RemoveFile {
        path: PathBuf::from(INSTALLED_BINARY),
    });
    if purge {
        steps.push(Step::Note(
            "--purge: the state directory holds the stored database credentials".to_string(),
        ));
        steps.push(Step::RemoveDirAll {
            path: PathBuf::from(platform.state_dir()),
        });
        let service_user = platform.service_user();
        match platform {
            Platform::MacOs => {
                for record in [
                    format!("/Users/{service_user}"),
                    format!("/Groups/{service_user}"),
                    format!("/Groups/{AGENTS_GROUP}"),
                ] {
                    steps.push(Step::run_tolerant(
                        &format!("delete {record}"),
                        Cmd::new("dscl", &[".", "-delete", &record]),
                    ));
                }
            }
            Platform::Linux => {
                steps.push(Step::run_tolerant(
                    &format!("delete user {service_user}"),
                    Cmd::new("userdel", &[service_user]),
                ));
                steps.push(Step::run_tolerant(
                    &format!("delete group {AGENTS_GROUP}"),
                    Cmd::new("groupdel", &[AGENTS_GROUP]),
                ));
            }
        }
    } else {
        steps.push(Step::Note(format!(
            "kept: {} (stored credentials), the {} user and the {AGENTS_GROUP} group; \
             use --purge to remove them",
            platform.state_dir(),
            platform.service_user()
        )));
    }
    steps
}

// ---- rendering -----------------------------------------------------------------

fn octal(mode: u32) -> String {
    format!("{mode:04o}")
}

/// The dry-run listing of a plan.
pub fn render_plan(title: &str, steps: &[Step]) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {title}\n"));
    for step in steps {
        match step {
            Step::Note(text) => out.push_str(&format!("# {text}\n")),
            Step::Run {
                description,
                command,
                skip_if,
                tolerate_failure,
                retries,
            } => {
                out.push_str(&format!("# {description}\n"));
                if let Some(probe) = skip_if {
                    out.push_str(&format!(
                        "#   skipped when `{}` succeeds\n",
                        probe.display()
                    ));
                }
                if *tolerate_failure {
                    out.push_str("#   (a failure here is ignored)\n");
                }
                if *retries > 0 {
                    out.push_str(&format!("#   (retried up to {retries} times)\n"));
                }
                out.push_str(&format!("$ {}\n", command.display()));
            }
            Step::MakeDir {
                path,
                mode,
                owner,
                create_only,
            } => {
                out.push_str(&format!(
                    "mkdir {}   # {}:{} mode {}{}\n",
                    shell_quote(&path.display().to_string()),
                    owner.user,
                    owner.group,
                    octal(*mode),
                    if *create_only {
                        ", only if missing"
                    } else {
                        ", fixed if different"
                    }
                ));
            }
            Step::WriteFile {
                path,
                contents,
                mode,
                owner,
                only_if_missing,
            } => {
                out.push_str(&format!(
                    "write {}   # {}:{} mode {}{}\n",
                    shell_quote(&path.display().to_string()),
                    owner.user,
                    owner.group,
                    octal(*mode),
                    if *only_if_missing {
                        ", only if missing"
                    } else {
                        ""
                    }
                ));
                for line in contents.lines() {
                    out.push_str(&format!("  | {line}\n"));
                }
            }
            Step::InstallBinary { from, to, owner } => {
                out.push_str(&format!(
                    "install {} -> {}   # {}:{} mode 0755, replaced atomically\n",
                    shell_quote(&from.display().to_string()),
                    shell_quote(&to.display().to_string()),
                    owner.user,
                    owner.group
                ));
            }
            Step::VerifyTrusted { path } => {
                out.push_str(&format!(
                    "verify {}   # it and every parent directory: owned by root, not group/world-writable\n",
                    path.display()
                ));
            }
            Step::RemoveFile { path } => {
                out.push_str(&format!(
                    "rm -f {}\n",
                    shell_quote(&path.display().to_string())
                ));
            }
            Step::RemoveDirAll { path } => {
                out.push_str(&format!(
                    "rm -rf {}\n",
                    shell_quote(&path.display().to_string())
                ));
            }
        }
    }
    out
}

// ---- execution -----------------------------------------------------------------

/// The effects a plan can have; the real one touches the system.
pub trait Executor {
    /// Run a command; `Ok(code)`, with `-1` when killed by a signal.
    fn run(&mut self, command: &Cmd) -> std::io::Result<i32>;
    fn make_dir(
        &mut self,
        path: &Path,
        mode: u32,
        owner: &Owner,
        create_only: bool,
    ) -> Result<(), BrokerError>;
    fn write_file(
        &mut self,
        path: &Path,
        contents: &str,
        mode: u32,
        owner: &Owner,
        only_if_missing: bool,
    ) -> Result<(), BrokerError>;
    fn install_binary(&mut self, from: &Path, to: &Path, owner: &Owner) -> Result<(), BrokerError>;
    fn verify_trusted(&mut self, path: &Path) -> Result<(), BrokerError>;
    fn remove_file(&mut self, path: &Path) -> Result<(), BrokerError>;
    fn remove_dir_all(&mut self, path: &Path) -> Result<(), BrokerError>;
    /// Progress line.
    fn say(&mut self, line: &str);
    fn sleep_one_second(&mut self) {}
}

/// Run `steps` in order; the first hard failure stops the plan.
pub fn execute_plan(steps: &[Step], exec: &mut dyn Executor) -> Result<(), BrokerError> {
    for step in steps {
        match step {
            Step::Note(text) => exec.say(&format!("note: {text}")),
            Step::Run {
                description,
                command,
                skip_if,
                tolerate_failure,
                retries,
            } => {
                if let Some(probe) = skip_if
                    && matches!(exec.run(probe), Ok(0))
                {
                    exec.say(&format!("{description}: already done"));
                    continue;
                }
                exec.say(description);
                let mut attempt = 0u8;
                let code = loop {
                    let code = exec.run(command).map_err(|e| {
                        BrokerError::State(format!("could not run `{}`: {e}", command.display()))
                    })?;
                    if code == 0 || attempt >= *retries {
                        break code;
                    }
                    attempt += 1;
                    exec.sleep_one_second();
                };
                if code != 0 {
                    if *tolerate_failure {
                        exec.say(&format!(
                            "  `{}` exited with {code} (ignored)",
                            command.display()
                        ));
                    } else {
                        return Err(BrokerError::State(format!(
                            "`{}` failed with exit code {code}",
                            command.display()
                        )));
                    }
                }
            }
            Step::MakeDir {
                path,
                mode,
                owner,
                create_only,
            } => {
                exec.say(&format!("directory {}", path.display()));
                exec.make_dir(path, *mode, owner, *create_only)?;
            }
            Step::WriteFile {
                path,
                contents,
                mode,
                owner,
                only_if_missing,
            } => {
                exec.say(&format!("file {}", path.display()));
                exec.write_file(path, contents, *mode, owner, *only_if_missing)?;
            }
            Step::InstallBinary { from, to, owner } => {
                exec.say(&format!("binary {}", to.display()));
                exec.install_binary(from, to, owner)?;
            }
            Step::VerifyTrusted { path } => exec.verify_trusted(path)?,
            Step::RemoveFile { path } => {
                exec.say(&format!("remove {}", path.display()));
                exec.remove_file(path)?;
            }
            Step::RemoveDirAll { path } => {
                exec.say(&format!("remove {}", path.display()));
                exec.remove_dir_all(path)?;
            }
        }
    }
    Ok(())
}

/// The real thing.
pub struct SystemExecutor;

fn resolve_owner(owner: &Owner) -> Result<(u32, u32), BrokerError> {
    let uid = if owner.user == "root" {
        0
    } else {
        user_ids(&owner.user)
            .map(|(uid, _)| uid)
            .ok_or_else(|| BrokerError::State(format!("no such user: {}", owner.user)))?
    };
    let gid = if owner.group == "root" {
        0
    } else {
        group_id(&owner.group)
            .ok_or_else(|| BrokerError::State(format!("no such group: {}", owner.group)))?
    };
    Ok((uid, gid))
}

fn io_state(path: &Path, e: std::io::Error) -> BrokerError {
    BrokerError::State(format!("{}: {e}", path.display()))
}

/// Refuse to follow a symlink at `path` (we run as root).
fn refuse_symlink(path: &Path) -> Result<(), BrokerError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(BrokerError::State(format!(
            "{} is a symbolic link; refusing to follow it as root",
            path.display()
        ))),
        _ => Ok(()),
    }
}

/// Atomically place `contents` (a closure writing the bytes) at `path`.
fn place_file(
    path: &Path,
    mode: u32,
    owner: &Owner,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> Result<(), BrokerError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    refuse_symlink(path)?;
    let (uid, gid) = resolve_owner(owner)?;
    let parent = path
        .parent()
        .ok_or_else(|| BrokerError::State(format!("{} has no parent", path.display())))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = parent.join(format!(".{name}.dbcrust-new"));
    let _ = std::fs::remove_file(&temp);
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temp)?;
        write(&mut file)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        chown(&temp, Some(uid), Some(gid))?;
        // umask may have trimmed the creation mode
        std::fs::set_permissions(&temp, std::os::unix::fs::PermissionsExt::from_mode(mode))?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(|e| io_state(path, e))
}

impl Executor for SystemExecutor {
    fn run(&mut self, command: &Cmd) -> std::io::Result<i32> {
        let status = std::process::Command::new(&command.program)
            .args(&command.args)
            .stdin(std::process::Stdio::null())
            .status()?;
        Ok(status.code().unwrap_or(-1))
    }

    fn make_dir(
        &mut self,
        path: &Path,
        mode: u32,
        owner: &Owner,
        create_only: bool,
    ) -> Result<(), BrokerError> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        refuse_symlink(path)?;
        let existed = path.exists();
        if !existed {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o755)
                .create(path)
                .map_err(|e| io_state(path, e))?;
        }
        if existed && create_only {
            return Ok(());
        }
        let (uid, gid) = resolve_owner(owner)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .map_err(|e| io_state(path, e))?;
        chown(path, Some(uid), Some(gid)).map_err(|e| io_state(path, e))
    }

    fn write_file(
        &mut self,
        path: &Path,
        contents: &str,
        mode: u32,
        owner: &Owner,
        only_if_missing: bool,
    ) -> Result<(), BrokerError> {
        if only_if_missing && std::fs::symlink_metadata(path).is_ok() {
            return Ok(());
        }
        place_file(path, mode, owner, |file| {
            std::io::Write::write_all(file, contents.as_bytes())
        })
    }

    fn install_binary(&mut self, from: &Path, to: &Path, owner: &Owner) -> Result<(), BrokerError> {
        if let (Ok(a), Ok(b)) = (std::fs::canonicalize(from), std::fs::canonicalize(to))
            && a == b
        {
            // Re-running from the installed copy: nothing to replace
            return Ok(());
        }
        let mut source = std::fs::File::open(from).map_err(|e| io_state(from, e))?;
        place_file(to, 0o755, owner, |file| {
            std::io::copy(&mut source, file).map(|_| ())
        })
    }

    fn verify_trusted(&mut self, path: &Path) -> Result<(), BrokerError> {
        let violations = binary_trust_violations(path)?;
        if violations.is_empty() {
            Ok(())
        } else {
            Err(BrokerError::State(format!(
                "the installed binary {} would not be trusted: {}. Fix those permissions (a user-writable /usr/local, as some Homebrew setups have, is the usual cause) and re-run",
                path.display(),
                violations.join("; ")
            )))
        }
    }

    fn remove_file(&mut self, path: &Path) -> Result<(), BrokerError> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_state(path, e)),
        }
    }

    fn remove_dir_all(&mut self, path: &Path) -> Result<(), BrokerError> {
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(io_state(path, e)),
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(BrokerError::State(format!(
                    "{} is not a plain directory; not removing it",
                    path.display()
                )));
            }
            Ok(_) => {}
        }
        std::fs::remove_dir_all(path).map_err(|e| io_state(path, e))
    }

    fn say(&mut self, line: &str) {
        println!("  {line}");
    }

    fn sleep_one_second(&mut self) {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

// ---- command front end ---------------------------------------------------------

/// Questions the install/uninstall commands may ask the person at the
/// keyboard, and the one network-free probe they need. The core never
/// prompts: the CLI front end (`broker/cli.rs`) supplies the implementation.
pub trait ServicePrompts {
    /// Ask a yes/no question. `None` when it could not be asked or was
    /// cancelled (no terminal, Ctrl-C).
    fn confirm(&mut self, question: &str, default: bool) -> Option<bool>;
}

/// Arguments of `install-service`, already resolved by the CLI layer.
#[derive(Debug, Clone)]
pub struct InstallRequest {
    pub dry_run: bool,
    pub add_user: Option<String>,
    pub no_add_user: bool,
    pub target_os: Option<String>,
}

fn resolve_platform(target_os: &Option<String>, dry_run: bool) -> Result<Platform, BrokerError> {
    match target_os {
        Some(name) => {
            if !dry_run {
                return Err(BrokerError::Invalid(
                    "--target-os only renders a plan: it needs --dry-run".to_string(),
                ));
            }
            Platform::parse(name).ok_or_else(|| {
                BrokerError::Invalid(format!("unknown platform {name:?}; use macos or linux"))
            })
        }
        None => Platform::current().ok_or_else(|| {
            BrokerError::Invalid(
                "install-service supports macOS (launchd) and Linux (systemd) only".to_string(),
            )
        }),
    }
}

fn require_root(what: &str) -> Result<(), BrokerError> {
    if effective_uid() == 0 {
        Ok(())
    } else {
        Err(BrokerError::Forbidden(format!(
            "{what} needs root: run it with sudo, or add --dry-run to see what it would do"
        )))
    }
}

/// The user who ran `sudo`, or (dry-run only) the current user.
fn invoking_user(dry_run: bool) -> Option<String> {
    let from_env = |name: &str| {
        std::env::var(name)
            .ok()
            .filter(|v| !v.is_empty() && v != "root")
    };
    from_env("SUDO_USER").or_else(|| {
        if dry_run {
            from_env("USER").or_else(|| from_env("LOGNAME"))
        } else {
            None
        }
    })
}

/// Which user (if any) joins `dbcrust-agents`.
fn choose_user(
    request: &InstallRequest,
    interactive: bool,
    prompts: &mut dyn ServicePrompts,
) -> Result<Option<String>, BrokerError> {
    if request.no_add_user {
        return Ok(None);
    }
    if let Some(name) = &request.add_user {
        validate_account_name(name)?;
        return Ok(Some(name.clone()));
    }
    let Some(user) = invoking_user(request.dry_run) else {
        return Ok(None);
    };
    if request.dry_run {
        // Show the step the real run would offer
        return Ok(Some(user));
    }
    if !interactive {
        println!(
            "note: not adding {user} to {AGENTS_GROUP} (no terminal to ask); use --add-user {user}"
        );
        return Ok(None);
    }
    let yes = prompts
        .confirm(
            &format!("Add {user} to the {AGENTS_GROUP} group (members can talk to the broker)?"),
            true,
        )
        .ok_or_else(|| BrokerError::Interactive("install cancelled".to_string()))?;
    Ok(yes.then_some(user))
}

pub async fn install(
    request: &InstallRequest,
    interactive: bool,
    prompts: &mut dyn ServicePrompts,
) -> Result<i32, BrokerError> {
    let platform = resolve_platform(&request.target_os, request.dry_run)?;
    if !request.dry_run {
        require_root("install-service")?;
    }
    let source_binary = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|e| BrokerError::State(format!("cannot locate the running binary: {e}")))?;
    let add_user = choose_user(request, interactive, prompts)?;
    let facts = if Some(platform) == Platform::current() {
        HostFacts::probe(platform)
    } else {
        HostFacts::default()
    };
    let options = InstallOptions {
        platform,
        source_binary,
        add_user,
    };
    let steps = plan_install(&options, &facts)?;
    let title = format!("dbcrust broker install-service — {}", platform.label());

    if request.dry_run {
        print!("{}", render_plan(&format!("{title} (dry run)"), &steps));
        println!("# nothing was changed");
        return Ok(0);
    }

    println!("{title}");
    execute_plan(&steps, &mut SystemExecutor)?;

    // Did it come up?
    let socket = PathBuf::from(platform.socket());
    let mut version = None;
    for _ in 0..10 {
        version = super::server::probe_socket(&socket);
        if version.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    match version {
        Some(v) => println!("Broker {v} is running on {}.", socket.display()),
        None => println!(
            "The daemon was installed but nothing answers on {} yet; check `dbcrust broker status --mode service` and {}.",
            socket.display(),
            match platform {
                Platform::MacOs => LAUNCHD_LOG.to_string(),
                Platform::Linux => "journalctl -u dbcrust-broker".to_string(),
            }
        ),
    }
    println!();
    println!("Next:");
    println!(
        "  sudo {INSTALLED_BINARY} broker enroll <alias> <url-without-password> --mode service"
    );
    if let Some(user) = &options.add_user {
        println!(
            "  {user} was added to {AGENTS_GROUP}: log out and back in (or open a new login session) before agents can connect."
        );
    } else {
        println!(
            "  Give a user access with: {}",
            match platform {
                Platform::MacOs =>
                    format!("sudo dseditgroup -o edit -a USER -t user {AGENTS_GROUP}"),
                Platform::Linux => format!("sudo usermod -aG {AGENTS_GROUP} USER"),
            }
        );
    }
    Ok(0)
}

#[derive(Debug, Clone)]
pub struct UninstallRequest {
    pub purge: bool,
    pub dry_run: bool,
    pub target_os: Option<String>,
}

pub async fn uninstall(
    request: &UninstallRequest,
    prompts: &mut dyn ServicePrompts,
) -> Result<i32, BrokerError> {
    let platform = resolve_platform(&request.target_os, request.dry_run)?;
    if !request.dry_run {
        require_root("uninstall-service")?;
    }
    let steps = plan_uninstall(platform, request.purge);
    let title = format!("dbcrust broker uninstall-service — {}", platform.label());
    if request.dry_run {
        print!("{}", render_plan(&format!("{title} (dry run)"), &steps));
        println!("# nothing was changed");
        return Ok(0);
    }
    if request.purge {
        println!(
            "--purge deletes {} — every credential the broker stores — and the {} user and {AGENTS_GROUP} group.",
            platform.state_dir(),
            platform.service_user()
        );
        let sure = prompts
            .confirm("Delete the stored credentials permanently?", false)
            .ok_or_else(|| BrokerError::Interactive("uninstall cancelled".to_string()))?;
        if !sure {
            println!("Nothing changed.");
            return Ok(0);
        }
    }
    println!("{title}");
    execute_plan(&steps, &mut SystemExecutor)?;
    println!("Done.");
    Ok(0)
}

// ---- status --------------------------------------------------------------------

fn capture(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn current_user_name() -> Option<String> {
    // SAFETY: getpwuid result is read immediately.
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if entry.is_null() {
            None
        } else {
            Some(
                std::ffi::CStr::from_ptr((*entry).pw_name)
                    .to_string_lossy()
                    .into_owned(),
            )
        }
    }
}

/// What `broker status --mode service` adds: the daemon, the trusted binary,
/// the socket and whether the invoking user can reach it. No secrets.
#[derive(Debug, Clone, Serialize)]
pub struct ServiceStatus {
    pub platform: Platform,
    pub definition_path: String,
    pub definition_present: bool,
    /// `running`, `loaded`, `active`, `inactive`, … or `unknown` when the
    /// service manager cannot be asked (not root on macOS).
    pub manager_state: String,
    pub installed_binary: String,
    pub installed_binary_present: bool,
    /// `None` when the binary is absent.
    pub installed_binary_trusted: Option<bool>,
    pub installed_binary_violations: Vec<String>,
    pub socket: String,
    pub socket_mode: Option<String>,
    pub socket_owner: Option<String>,
    pub socket_group: Option<String>,
    pub user: Option<String>,
    /// Listed in the group's membership in the directory.
    pub user_in_agents_group: Option<bool>,
    /// The group is in *this process's* credentials (a new login is needed
    /// after being added).
    pub session_in_agents_group: Option<bool>,
}

pub fn service_status(platform: Platform) -> ServiceStatus {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    let definition_path = platform.definition_path();
    let definition_present = Path::new(definition_path).exists();
    let manager_state = match platform {
        Platform::MacOs => capture("launchctl", &["print", &format!("system/{LAUNCHD_LABEL}")])
            .map(|text| {
                text.lines()
                    .find_map(|l| l.trim().strip_prefix("state = "))
                    .map_or("loaded".to_string(), |s| s.trim().to_string())
            })
            .unwrap_or_else(|| {
                if effective_uid() == 0 {
                    "not loaded".to_string()
                } else {
                    "unknown (launchctl needs root)".to_string()
                }
            }),
        Platform::Linux => capture("systemctl", &["is-active", "dbcrust-broker"])
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "inactive or unknown".to_string()),
    };

    let binary = Path::new(INSTALLED_BINARY);
    let present = binary.exists();
    let violations = if present {
        binary_trust_violations(binary).unwrap_or_else(|e| vec![e.to_string()])
    } else {
        Vec::new()
    };

    let socket = platform.socket();
    let (socket_mode, socket_owner, socket_group) = match std::fs::metadata(socket) {
        Ok(meta) if meta.file_type().is_socket() => {
            let gid = meta.gid();
            (
                Some(format!("{:04o}", meta.permissions().mode() & 0o7777)),
                Some(format!("uid {}", meta.uid())),
                Some(format!("gid {gid}")),
            )
        }
        _ => (None, None, None),
    };

    let user = std::env::var("SUDO_USER")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(current_user_name);
    let agents_gid = group_id(AGENTS_GROUP);
    let user_in = match (&user, agents_gid) {
        (Some(user), Some(_)) => Some(user_in_group(user, AGENTS_GROUP)),
        _ => None,
    };
    let session = agents_gid.map(|gid| {
        let creds = Credentials::current();
        creds.groups.contains(&gid) || creds.gid == gid
    });

    ServiceStatus {
        platform,
        definition_path: definition_path.to_string(),
        definition_present,
        manager_state,
        installed_binary: INSTALLED_BINARY.to_string(),
        installed_binary_present: present,
        installed_binary_trusted: present.then_some(violations.is_empty()),
        installed_binary_violations: violations,
        socket: socket.to_string(),
        socket_mode,
        socket_owner,
        socket_group,
        user,
        user_in_agents_group: user_in,
        session_in_agents_group: session,
    }
}

impl ServiceStatus {
    pub fn print(&self) {
        println!("Service");
        println!(
            "  definition: {} ({})",
            self.definition_path,
            if self.definition_present {
                "present"
            } else {
                "missing — run `sudo dbcrust broker install-service`"
            }
        );
        println!("  daemon:     {}", self.manager_state);
        match (self.installed_binary_present, self.installed_binary_trusted) {
            (false, _) => println!("  binary:     {} (missing)", self.installed_binary),
            (true, Some(true)) => println!("  binary:     {} (trusted)", self.installed_binary),
            (true, _) => {
                println!("  binary:     {} (NOT trusted)", self.installed_binary);
                for v in &self.installed_binary_violations {
                    println!("              {v}");
                }
            }
        }
        match (&self.socket_mode, &self.socket_owner, &self.socket_group) {
            (Some(mode), Some(owner), Some(group)) => {
                println!(
                    "  socket:     {} mode {mode}, {owner}, {group}",
                    self.socket
                )
            }
            _ => println!("  socket:     {} (absent)", self.socket),
        }
        if let (Some(user), Some(member)) = (&self.user, self.user_in_agents_group) {
            println!(
                "  access:     {user} {} {AGENTS_GROUP}{}",
                if member { "is in" } else { "is NOT in" },
                match (member, self.session_in_agents_group) {
                    (true, Some(false)) =>
                        " (log out and back in: this session does not have it yet)",
                    (false, _) => " — ask an administrator to add you",
                    _ => "",
                }
            );
        }
    }
}

// ---- --drop-to socket ----------------------------------------------------------

/// Bind the service socket while still root, then hand it to the service
/// user, so the caller can drop privileges and keep serving.
///
/// * the socket directory is created `root:<group> 0750` (`0700` and a
///   warning when the group does not exist) and must be owned by
///   `dir_owner` (root in production) if it already exists;
/// * the socket is created under `umask 077`, then given to
///   `service_uid:<group>` with mode `0660` (`0600` without the group).
pub fn bind_socket_for_drop(
    path: &Path,
    service_uid: u32,
    settings: &BrokerSettings,
    dir_owner: u32,
) -> Result<tokio::net::UnixListener, BrokerError> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| BrokerError::State(format!("{} has no directory", path.display())))?;
    let group = settings.socket_group();
    let gid = group_id(group);

    match std::fs::symlink_metadata(parent) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(BrokerError::State(format!(
                    "{} is not a plain directory",
                    parent.display()
                )));
            }
            if meta.uid() != dir_owner {
                return Err(BrokerError::State(format!(
                    "{} is owned by uid {}, expected {dir_owner}; refusing to put the socket there",
                    parent.display(),
                    meta.uid()
                )));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
                .map_err(|e| io_state(parent, e))?;
        }
        Err(e) => return Err(io_state(parent, e)),
    }
    let dir_mode = if gid.is_some() { 0o750 } else { 0o700 };
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(dir_mode))
        .map_err(|e| io_state(parent, e))?;
    if let Some(gid) = gid {
        chown(parent, Some(dir_owner), Some(gid)).map_err(|e| io_state(parent, e))?;
    } else {
        tracing::warn!(
            "group {group:?} does not exist; the socket directory and socket stay private to the service user (no agent can connect until the group exists)"
        );
    }

    // SAFETY: umask cannot fail.
    unsafe { libc::umask(0o077) };
    clear_stale_socket(path)?;
    let listener = tokio::net::UnixListener::bind(path)
        .map_err(|e| BrokerError::State(format!("cannot bind {}: {e}", path.display())))?;
    chown(path, Some(service_uid), gid).map_err(|e| io_state(path, e))?;
    let mode = if gid.is_some() { 0o660 } else { 0o600 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| io_state(path, e))?;
    Ok(listener)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn opts(platform: Platform, user: Option<&str>) -> InstallOptions {
        InstallOptions {
            platform,
            source_binary: PathBuf::from("/tmp/build/dbcrust"),
            add_user: user.map(str::to_string),
        }
    }

    fn fresh_mac_facts() -> HostFacts {
        HostFacts {
            used_ids: [200, 201, 202, 399, 400].into_iter().collect(),
            ..HostFacts::default()
        }
    }

    // ---- generated files ----

    #[test]
    fn launchd_plist_snapshot() {
        let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>dev.dbcrust.broker</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/libexec/dbcrust-broker</string>
        <string>broker</string>
        <string>serve</string>
        <string>--mode</string>
        <string>service</string>
        <string>--drop-to</string>
        <string>_dbcrust</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/Library/Logs/dbcrust-broker.log</string>
    <key>StandardErrorPath</key>
    <string>/Library/Logs/dbcrust-broker.log</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/usr/bin:/bin</string>
    </dict>
</dict>
</plist>
"#;
        assert_eq!(
            launchd_plist(INSTALLED_BINARY, "_dbcrust", LAUNCHD_LOG),
            expected
        );
    }

    #[test]
    fn launchd_plist_has_no_username_key() {
        // The job starts as root on purpose (it creates /var/run/dbcrust)
        let plist = launchd_plist(INSTALLED_BINARY, "_dbcrust", LAUNCHD_LOG);
        assert!(!plist.contains("UserName"));
        assert!(plist.contains("--drop-to"));
    }

    #[test]
    fn plist_values_are_xml_escaped() {
        let plist = launchd_plist("/p/a&b<c>", "u", "/l");
        assert!(plist.contains("/p/a&amp;b&lt;c&gt;"));
    }

    #[test]
    fn systemd_unit_snapshot() {
        let expected = "[Unit]
Description=DBCrust credential broker for AI agents
Documentation=https://clement-tourriere.github.io/dbcrust/user-guide/agent-connections/
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=dbcrust
Group=dbcrust
SupplementaryGroups=dbcrust-agents
ExecStart=/usr/local/libexec/dbcrust-broker broker serve --mode service --socket /run/dbcrust/broker.sock
StateDirectory=dbcrust-broker
StateDirectoryMode=0700
RuntimeDirectory=dbcrust
RuntimeDirectoryMode=0750
UMask=0077
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectControlGroups=yes
RestrictSUIDSGID=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
";
        assert_eq!(
            systemd_unit(INSTALLED_BINARY, "dbcrust", "/run/dbcrust/broker.sock"),
            expected
        );
    }

    #[test]
    fn broker_toml_is_accepted_by_the_settings_parser() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broker.toml");
        std::fs::write(&path, broker_toml()).unwrap();
        let settings = BrokerSettings::load(&path).unwrap();
        assert_eq!(settings.socket_group(), AGENTS_GROUP);
    }

    // ---- helpers ----

    #[test]
    fn dscl_listing_is_parsed() {
        let text = "_amavisd                83\n_dbcrust   399\nroot  0\nweird notanumber\n";
        assert_eq!(parse_dscl_ids(text), vec![83, 399, 0]);
    }

    #[test]
    fn free_ids_come_from_the_top_of_the_range() {
        let used: BTreeSet<u32> = [400, 399, 250].into_iter().collect();
        assert_eq!(pick_free_ids(&used, 2), Some(vec![398, 397]));
        let full: BTreeSet<u32> = (200..=400).collect();
        assert_eq!(pick_free_ids(&full, 1), None);
    }

    #[rstest]
    #[case("alice", true)]
    #[case("a.b-c_d", true)]
    #[case("-rf", false)]
    #[case("", false)]
    #[case("a b", false)]
    #[case("a;b", false)]
    #[case("$(id)", false)]
    fn account_names_are_validated(#[case] name: &str, #[case] ok: bool) {
        assert_eq!(validate_account_name(name).is_ok(), ok, "{name}");
    }

    // ---- plans ----

    #[test]
    fn macos_install_plan_snapshot() {
        let steps =
            plan_install(&opts(Platform::MacOs, Some("alice")), &fresh_mac_facts()).unwrap();
        let rendered = render_plan("install (dry run)", &steps);
        let expected = r#"# install (dry run)
# create /Groups/_dbcrust
$ dscl . -create /Groups/_dbcrust
# set PrimaryGroupID of /Groups/_dbcrust
$ dscl . -create /Groups/_dbcrust PrimaryGroupID 398
# set RealName of /Groups/_dbcrust
$ dscl . -create /Groups/_dbcrust RealName 'DBCrust credential broker'
# set Password of /Groups/_dbcrust
$ dscl . -create /Groups/_dbcrust Password '*'
# create /Groups/dbcrust-agents
$ dscl . -create /Groups/dbcrust-agents
# set PrimaryGroupID of /Groups/dbcrust-agents
$ dscl . -create /Groups/dbcrust-agents PrimaryGroupID 397
# set RealName of /Groups/dbcrust-agents
$ dscl . -create /Groups/dbcrust-agents RealName 'Users allowed to use the DBCrust broker'
# set Password of /Groups/dbcrust-agents
$ dscl . -create /Groups/dbcrust-agents Password '*'
# create /Users/_dbcrust
$ dscl . -create /Users/_dbcrust
# set UniqueID of /Users/_dbcrust
$ dscl . -create /Users/_dbcrust UniqueID 398
# set PrimaryGroupID of /Users/_dbcrust
$ dscl . -create /Users/_dbcrust PrimaryGroupID 398
# set UserShell of /Users/_dbcrust
$ dscl . -create /Users/_dbcrust UserShell /usr/bin/false
# set NFSHomeDirectory of /Users/_dbcrust
$ dscl . -create /Users/_dbcrust NFSHomeDirectory /var/empty
# set RealName of /Users/_dbcrust
$ dscl . -create /Users/_dbcrust RealName 'DBCrust credential broker'
# set IsHidden of /Users/_dbcrust
$ dscl . -create /Users/_dbcrust IsHidden 1
# set Password of /Users/_dbcrust
$ dscl . -create /Users/_dbcrust Password '*'
# add alice to dbcrust-agents
#   skipped when `dseditgroup -o checkmember -m alice dbcrust-agents` succeeds
$ dseditgroup -o edit -a alice -t user dbcrust-agents
mkdir /usr/local/libexec   # root:wheel mode 0755, only if missing
install /tmp/build/dbcrust -> /usr/local/libexec/dbcrust-broker   # root:wheel mode 0755, replaced atomically
verify /usr/local/libexec/dbcrust-broker   # it and every parent directory: owned by root, not group/world-writable
mkdir '/Library/Application Support/dbcrust-broker'   # _dbcrust:_dbcrust mode 0700, fixed if different
write '/Library/Application Support/dbcrust-broker/broker.toml'   # _dbcrust:_dbcrust mode 0600, only if missing
  | # DBCrust broker settings (service mode). Unknown keys are an error.
  | # Group allowed to connect to the socket.
  | socket_group = "dbcrust-agents"
write /Library/LaunchDaemons/dev.dbcrust.broker.plist   # root:wheel mode 0644
  | <?xml version="1.0" encoding="UTF-8"?>
  | <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
  | <plist version="1.0">
  | <dict>
  |     <key>Label</key>
  |     <string>dev.dbcrust.broker</string>
  |     <key>ProgramArguments</key>
  |     <array>
  |         <string>/usr/local/libexec/dbcrust-broker</string>
  |         <string>broker</string>
  |         <string>serve</string>
  |         <string>--mode</string>
  |         <string>service</string>
  |         <string>--drop-to</string>
  |         <string>_dbcrust</string>
  |     </array>
  |     <key>RunAtLoad</key>
  |     <true/>
  |     <key>KeepAlive</key>
  |     <true/>
  |     <key>StandardOutPath</key>
  |     <string>/Library/Logs/dbcrust-broker.log</string>
  |     <key>StandardErrorPath</key>
  |     <string>/Library/Logs/dbcrust-broker.log</string>
  |     <key>EnvironmentVariables</key>
  |     <dict>
  |         <key>PATH</key>
  |         <string>/usr/bin:/bin</string>
  |     </dict>
  | </dict>
  | </plist>
# unload a previous version of the daemon (fails harmlessly when none)
#   (a failure here is ignored)
$ launchctl bootout system/dev.dbcrust.broker
# make sure the daemon is not disabled
#   (a failure here is ignored)
$ launchctl enable system/dev.dbcrust.broker
# load and start the daemon
#   (retried up to 3 times)
$ launchctl bootstrap system /Library/LaunchDaemons/dev.dbcrust.broker.plist
"#;
        assert_eq!(rendered, expected);
    }

    #[test]
    fn linux_install_plan_snapshot_commands() {
        let steps =
            plan_install(&opts(Platform::Linux, Some("alice")), &HostFacts::default()).unwrap();
        let rendered = render_plan("install", &steps);
        for line in [
            "$ groupadd --system dbcrust-agents",
            "$ useradd --system --no-create-home --shell /usr/sbin/nologin --comment 'DBCrust credential broker' --user-group dbcrust",
            "$ usermod -aG dbcrust-agents alice",
            "mkdir /usr/local/libexec   # root:root mode 0755, only if missing",
            "install /tmp/build/dbcrust -> /usr/local/libexec/dbcrust-broker   # root:root mode 0755, replaced atomically",
            "mkdir /var/lib/dbcrust-broker   # dbcrust:dbcrust mode 0700, fixed if different",
            "write /var/lib/dbcrust-broker/broker.toml   # dbcrust:dbcrust mode 0600, only if missing",
            "write /etc/systemd/system/dbcrust-broker.service   # root:root mode 0644",
            "$ systemctl daemon-reload",
            "$ systemctl enable dbcrust-broker",
            "$ systemctl restart dbcrust-broker",
        ] {
            assert!(rendered.contains(line), "missing {line:?} in:\n{rendered}");
        }
        assert!(rendered.contains("  | MemoryDenyWriteExecute=yes"));
        assert!(!rendered.contains("dscl"));
    }

    #[test]
    fn existing_identities_are_not_recreated() {
        let facts = HostFacts {
            service_group_gid: Some(398),
            service_user_exists: true,
            agents_group_exists: true,
            used_ids: BTreeSet::new(),
        };
        let steps = plan_install(&opts(Platform::MacOs, None), &facts).unwrap();
        let rendered = render_plan("install", &steps);
        assert!(!rendered.contains("dscl"), "{rendered}");
        assert!(rendered.contains("# user _dbcrust already exists"));
        assert!(!rendered.contains("dseditgroup"));

        let linux = plan_install(
            &opts(Platform::Linux, None),
            &HostFacts {
                service_group_gid: Some(990),
                service_user_exists: false,
                agents_group_exists: true,
                used_ids: BTreeSet::new(),
            },
        )
        .unwrap();
        let rendered = render_plan("install", &linux);
        assert!(!rendered.contains("groupadd"));
        // user missing but its group exists: reuse the group
        assert!(rendered.contains("-g dbcrust dbcrust"), "{rendered}");
        assert!(!rendered.contains("--user-group"));
    }

    #[test]
    fn id_exhaustion_is_an_error() {
        let facts = HostFacts {
            used_ids: (200..=400).collect(),
            ..HostFacts::default()
        };
        assert!(plan_install(&opts(Platform::MacOs, None), &facts).is_err());
    }

    #[test]
    fn hostile_user_names_never_reach_a_command() {
        assert!(plan_install(&opts(Platform::Linux, Some("-o")), &HostFacts::default()).is_err());
    }

    #[test]
    fn install_never_removes_or_rewrites_the_store() {
        for platform in [Platform::MacOs, Platform::Linux] {
            let steps = plan_install(&opts(platform, None), &fresh_mac_facts()).unwrap();
            for step in &steps {
                match step {
                    Step::RemoveFile { .. } | Step::RemoveDirAll { .. } => {
                        panic!("install plan removes something: {step:?}")
                    }
                    Step::WriteFile {
                        path,
                        only_if_missing,
                        ..
                    } if path.starts_with(platform.state_dir()) => {
                        assert!(only_if_missing, "{path:?} must only be created");
                        assert!(path.ends_with("broker.toml"));
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn uninstall_keeps_the_store_unless_purged() {
        for platform in [Platform::MacOs, Platform::Linux] {
            let kept = render_plan("u", &plan_uninstall(platform, false));
            assert!(!kept.contains(&format!("rm -rf {}", shell_quote(platform.state_dir()))));
            assert!(kept.contains("--purge"));
            let purged = render_plan("u", &plan_uninstall(platform, true));
            assert!(purged.contains(&format!("rm -rf {}", shell_quote(platform.state_dir()))));
        }
        let mac = render_plan("u", &plan_uninstall(Platform::MacOs, true));
        assert!(mac.contains("$ launchctl bootout system/dev.dbcrust.broker"));
        assert!(mac.contains("$ dscl . -delete /Users/_dbcrust"));
        assert!(mac.contains("rm -f /usr/local/libexec/dbcrust-broker"));
        let linux = render_plan("u", &plan_uninstall(Platform::Linux, true));
        assert!(linux.contains("$ systemctl disable --now dbcrust-broker"));
        assert!(linux.contains("$ userdel dbcrust"));
        assert!(linux.contains("$ groupdel dbcrust-agents"));
    }

    #[test]
    fn socket_locations_match_the_client() {
        assert_eq!(Platform::MacOs.socket(), "/var/run/dbcrust/broker.sock");
        assert_eq!(Platform::Linux.socket(), "/run/dbcrust/broker.sock");
    }

    // ---- executor ----

    #[derive(Default)]
    struct Fake {
        log: Vec<String>,
        /// exit codes by rendered command (default 0)
        codes: std::collections::HashMap<String, Vec<i32>>,
    }

    impl Fake {
        fn code_for(&mut self, command: &Cmd) -> i32 {
            match self.codes.get_mut(&command.display()) {
                Some(queue) if !queue.is_empty() => queue.remove(0),
                _ => 0,
            }
        }
    }

    impl Executor for Fake {
        fn run(&mut self, command: &Cmd) -> std::io::Result<i32> {
            self.log.push(format!("run {}", command.display()));
            Ok(self.code_for(command))
        }
        fn make_dir(&mut self, path: &Path, _: u32, _: &Owner, _: bool) -> Result<(), BrokerError> {
            self.log.push(format!("mkdir {}", path.display()));
            Ok(())
        }
        fn write_file(
            &mut self,
            path: &Path,
            _: &str,
            _: u32,
            _: &Owner,
            _: bool,
        ) -> Result<(), BrokerError> {
            self.log.push(format!("write {}", path.display()));
            Ok(())
        }
        fn install_binary(&mut self, _: &Path, to: &Path, _: &Owner) -> Result<(), BrokerError> {
            self.log.push(format!("install {}", to.display()));
            Ok(())
        }
        fn verify_trusted(&mut self, path: &Path) -> Result<(), BrokerError> {
            self.log.push(format!("verify {}", path.display()));
            Ok(())
        }
        fn remove_file(&mut self, path: &Path) -> Result<(), BrokerError> {
            self.log.push(format!("rm {}", path.display()));
            Ok(())
        }
        fn remove_dir_all(&mut self, path: &Path) -> Result<(), BrokerError> {
            self.log.push(format!("rm -rf {}", path.display()));
            Ok(())
        }
        fn say(&mut self, _: &str) {}
    }

    #[test]
    fn execution_runs_every_step_in_order() {
        let steps =
            plan_install(&opts(Platform::Linux, Some("alice")), &HostFacts::default()).unwrap();
        let mut fake = Fake::default();
        execute_plan(&steps, &mut fake).unwrap();
        let log = fake.log.join("\n");
        let pos = |needle: &str| {
            log.find(needle)
                .unwrap_or_else(|| panic!("{needle} in {log}"))
        };
        assert!(pos("run groupadd") < pos("run useradd"));
        assert!(pos("run usermod") < pos("install /usr/local/libexec/dbcrust-broker"));
        assert!(pos("verify /usr/local/libexec/dbcrust-broker") < pos("write /etc/systemd/system"));
        assert!(pos("write /etc/systemd/system") < pos("run systemctl daemon-reload"));
        assert!(pos("run systemctl enable") < pos("run systemctl restart"));
    }

    #[test]
    fn a_failing_step_stops_the_plan() {
        let steps = plan_install(&opts(Platform::Linux, None), &HostFacts::default()).unwrap();
        let mut fake = Fake::default();
        fake.codes
            .insert("groupadd --system dbcrust-agents".to_string(), vec![9]);
        let err = execute_plan(&steps, &mut fake).unwrap_err();
        assert!(err.to_string().contains("groupadd"));
        assert_eq!(fake.log.len(), 1, "{:?}", fake.log);
    }

    #[test]
    fn tolerated_failures_and_retries_and_probes() {
        let steps = plan_install(
            &opts(Platform::MacOs, Some("bob")),
            &HostFacts {
                service_group_gid: Some(300),
                service_user_exists: true,
                agents_group_exists: true,
                used_ids: BTreeSet::new(),
            },
        )
        .unwrap();
        let mut fake = Fake::default();
        // bootout fails (nothing loaded), bootstrap fails twice then works,
        // bob is already a member
        fake.codes.insert(
            "launchctl bootout system/dev.dbcrust.broker".into(),
            vec![3],
        );
        fake.codes.insert(
            "launchctl bootstrap system /Library/LaunchDaemons/dev.dbcrust.broker.plist".into(),
            vec![5, 5, 0],
        );
        fake.codes.insert(
            "dseditgroup -o checkmember -m bob dbcrust-agents".into(),
            vec![0],
        );
        execute_plan(&steps, &mut fake).unwrap();
        let bootstraps = fake
            .log
            .iter()
            .filter(|l| l.contains("launchctl bootstrap"))
            .count();
        assert_eq!(bootstraps, 3);
        assert!(
            !fake.log.iter().any(|l| l.contains("-o edit")),
            "{:?}",
            fake.log
        );
    }

    #[test]
    fn exhausted_retries_fail() {
        let steps = plan_install(
            &opts(Platform::MacOs, None),
            &HostFacts {
                service_group_gid: Some(300),
                service_user_exists: true,
                agents_group_exists: true,
                used_ids: BTreeSet::new(),
            },
        )
        .unwrap();
        let mut fake = Fake::default();
        fake.codes.insert(
            "launchctl bootstrap system /Library/LaunchDaemons/dev.dbcrust.broker.plist".into(),
            vec![5, 5, 5, 5, 5],
        );
        assert!(execute_plan(&steps, &mut fake).is_err());
    }

    #[test]
    fn target_os_needs_dry_run() {
        assert!(resolve_platform(&Some("linux".into()), false).is_err());
        assert_eq!(
            resolve_platform(&Some("linux".into()), true).unwrap(),
            Platform::Linux
        );
        assert!(resolve_platform(&Some("plan9".into()), true).is_err());
    }

    // ---- real file system pieces ----

    fn current_owner() -> Owner {
        // SAFETY: plain getters; names resolved through the system databases
        let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
        let user = unsafe {
            std::ffi::CStr::from_ptr((*libc::getpwuid(uid)).pw_name)
                .to_string_lossy()
                .into_owned()
        };
        let group = unsafe {
            std::ffi::CStr::from_ptr((*libc::getgrgid(gid)).gr_name)
                .to_string_lossy()
                .into_owned()
        };
        Owner { user, group }
    }

    #[test]
    fn system_executor_writes_atomically_and_respects_only_if_missing() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broker.toml");
        let mut exec = SystemExecutor;
        let owner = current_owner();
        exec.write_file(&path, "first", 0o600, &owner, true)
            .unwrap();
        exec.write_file(&path, "second", 0o600, &owner, true)
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
        exec.write_file(&path, "third", 0o640, &owner, false)
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "third");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
        // no temp file left behind
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn system_executor_refuses_symlink_targets() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::write(&real, "x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut exec = SystemExecutor;
        assert!(
            exec.write_file(&link, "evil", 0o600, &current_owner(), false)
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "x");
        assert!(exec.remove_dir_all(&link).is_err());
    }

    #[test]
    fn install_binary_replaces_atomically_and_skips_itself() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dbcrust-broker");
        std::fs::write(&src, b"v2").unwrap();
        std::fs::write(&dst, b"v1").unwrap();
        let mut exec = SystemExecutor;
        exec.install_binary(&src, &dst, &current_owner()).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"v2");
        assert_eq!(
            std::fs::metadata(&dst).unwrap().permissions().mode() & 0o777,
            0o755
        );
        // from == to: untouched
        exec.install_binary(&dst, &dst, &current_owner()).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"v2");
    }

    #[test]
    fn make_dir_create_only_keeps_an_existing_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("libexec");
        std::fs::create_dir(&existing).unwrap();
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut exec = SystemExecutor;
        exec.make_dir(&existing, 0o755, &current_owner(), true)
            .unwrap();
        assert_eq!(
            std::fs::metadata(&existing).unwrap().permissions().mode() & 0o777,
            0o700
        );
        exec.make_dir(&existing, 0o755, &current_owner(), false)
            .unwrap();
        assert_eq!(
            std::fs::metadata(&existing).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[tokio::test]
    async fn drop_socket_is_created_private_and_handed_over() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run").join("broker.sock");
        let settings = BrokerSettings {
            socket_group: Some("dbcrust-no-such-group-xyz".to_string()),
            ..BrokerSettings::default()
        };
        let own = effective_uid();
        let _listener = bind_socket_for_drop(&path, own, &settings, own).unwrap();
        let socket = std::fs::metadata(&path).unwrap();
        assert_eq!(socket.permissions().mode() & 0o777, 0o600);
        assert_eq!(socket.uid(), own);
        let parent = std::fs::metadata(path.parent().unwrap()).unwrap();
        assert_eq!(parent.permissions().mode() & 0o777, 0o700);
    }

    #[tokio::test]
    async fn drop_socket_refuses_a_directory_with_the_wrong_owner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broker.sock");
        let own = effective_uid();
        // the directory belongs to us, but "root" (uid 0 / other) is expected
        let err = bind_socket_for_drop(&path, own, &BrokerSettings::default(), own + 1)
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("expected"), "{err}");
        assert!(!path.exists());
    }
}
