//! The pure half of the tray: what the menu says ([`build_menu`]), which
//! command each entry runs ([`TrayCommand`]), and what closing the window
//! does ([`close_action`]). Nothing here touches the OS or GPUI, so all of it
//! is unit-tested.
//!
//! Menus show names only: never URLs, hosts or credentials.

use std::collections::HashSet;

/// Saved sessions listed under Connect.
pub const MAX_SESSIONS: usize = 15;
/// Recent connections listed under Connect (after removing duplicates).
pub const MAX_RECENTS: usize = 5;
/// Longest label shown in the menu before it is cut with an ellipsis.
const MAX_LABEL_CHARS: usize = 44;

// ----- Closing the window -----

/// What the window's close button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    /// Hide the window; the app keeps running in the tray.
    Hide,
    /// Quit (after the usual confirmations and the final save).
    Quit,
}

/// Closing hides the window only when the user wants it, a tray icon exists
/// to bring it back, and the platform can hide a window without destroying
/// it. Without all three it quits: a hidden process must always have a way
/// back.
pub fn close_action(keep_running: bool, tray_active: bool, window_can_hide: bool) -> CloseAction {
    if keep_running && tray_active && window_can_hide {
        CloseAction::Hide
    } else {
        CloseAction::Quit
    }
}

// ----- Wording -----

/// Platform words used in the first-close dialog and the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wording {
    /// Where the icon lives: "menu bar" / "notification area".
    pub area: &'static str,
    /// How to get the window back.
    pub reopen: &'static str,
    /// The quit shortcut, as written in the UI.
    pub quit_shortcut: &'static str,
}

impl Wording {
    pub fn for_platform(mac: bool, dock_icon_hidden: bool) -> Self {
        match (mac, dock_icon_hidden) {
            (true, false) => Wording {
                area: "menu bar",
                reopen: "Open it from the menu bar or the Dock",
                quit_shortcut: "⌘Q",
            },
            (true, true) => Wording {
                area: "menu bar",
                reopen: "Open it from the menu bar",
                quit_shortcut: "⌘Q",
            },
            (false, _) => Wording {
                area: "notification area",
                reopen: "Open it from its tray icon (it may be under the ^ arrow)",
                quit_shortcut: "Ctrl+Q",
            },
        }
    }

    pub fn current(dock_icon_hidden: bool) -> Self {
        Self::for_platform(cfg!(target_os = "macos"), dock_icon_hidden)
    }

    /// The text of the one-time dialog shown before the first hide.
    pub fn first_close_message(&self) -> String {
        format!(
            "DBCrust will keep running in the {}. Connections and queries stay active. {}; {} quits.",
            self.area, self.reopen, self.quit_shortcut
        )
    }
}

// ----- Commands -----

/// What a tray entry does. The id round-trips through the OS menu item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    /// Show the window.
    Open,
    NewQuery,
    NewConnection,
    /// Connect a saved session, by name.
    ConnectSession(String),
    /// Connect a recent connection, by [`recent_key`].
    ConnectRecent(u64),
    /// Show the window with this live connection (`ConnectionId::as_u64`)
    /// active.
    OpenConnection(u64),
    ConnectionNewQuery(u64),
    Disconnect(u64),
    DisconnectAll,
    Settings,
    Quit,
}

impl TrayCommand {
    /// The OS menu item id.
    pub fn id(&self) -> String {
        match self {
            TrayCommand::Open => "open".into(),
            TrayCommand::NewQuery => "new-query".into(),
            TrayCommand::NewConnection => "new-connection".into(),
            TrayCommand::ConnectSession(name) => format!("session:{name}"),
            TrayCommand::ConnectRecent(key) => format!("recent:{key:016x}"),
            TrayCommand::OpenConnection(id) => format!("conn-open:{id}"),
            TrayCommand::ConnectionNewQuery(id) => format!("conn-query:{id}"),
            TrayCommand::Disconnect(id) => format!("conn-disconnect:{id}"),
            TrayCommand::DisconnectAll => "disconnect-all".into(),
            TrayCommand::Settings => "settings".into(),
            TrayCommand::Quit => "quit".into(),
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        Some(match id {
            "open" => TrayCommand::Open,
            "new-query" => TrayCommand::NewQuery,
            "new-connection" => TrayCommand::NewConnection,
            "disconnect-all" => TrayCommand::DisconnectAll,
            "settings" => TrayCommand::Settings,
            "quit" => TrayCommand::Quit,
            _ => {
                let (kind, value) = id.split_once(':')?;
                match kind {
                    "session" => TrayCommand::ConnectSession(value.to_string()),
                    "recent" => TrayCommand::ConnectRecent(u64::from_str_radix(value, 16).ok()?),
                    "conn-open" => TrayCommand::OpenConnection(value.parse().ok()?),
                    "conn-query" => TrayCommand::ConnectionNewQuery(value.parse().ok()?),
                    "conn-disconnect" => TrayCommand::Disconnect(value.parse().ok()?),
                    _ => return None,
                }
            }
        })
    }
}

/// A stable identity for a recent connection (FNV-1a of its stored URL), so
/// the menu item id carries neither the URL nor an index that could go stale
/// before the click.
pub fn recent_key(url: &str) -> u64 {
    url.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

// ----- The menu -----

/// Everything the menu shows, copied out of the app state. Equal snapshots
/// give identical menus, so the OS menu is rebuilt only when this changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraySnapshot {
    pub connections: Vec<TrayConnection>,
    /// Query tabs currently executing.
    pub running_queries: usize,
    /// A connection is being opened.
    pub connecting: bool,
    /// Saved session names.
    pub sessions: Vec<String>,
    /// Recent connections, newest first.
    pub recents: Vec<TrayRecent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayConnection {
    /// `ConnectionId::as_u64`.
    pub id: u64,
    /// Session name or `database (host)`; never a URL.
    pub label: String,
    /// "PostgreSQL", "SQLite", …
    pub engine: String,
    pub read_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayRecent {
    pub key: u64,
    pub label: String,
}

/// One entry of the tray menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayItem {
    /// Disabled text: the status line and section headings.
    Label(String),
    Action {
        command: TrayCommand,
        label: String,
        enabled: bool,
    },
    Submenu {
        label: String,
        enabled: bool,
        items: Vec<TrayItem>,
    },
    Separator,
}

impl TrayItem {
    fn action(command: TrayCommand, label: impl Into<String>, enabled: bool) -> Self {
        TrayItem::Action {
            command,
            label: label.into(),
            enabled,
        }
    }
}

/// "No connections", "2 connections · 1 query running", "Connecting…".
pub fn status_text(snapshot: &TraySnapshot) -> String {
    let mut parts = vec![match snapshot.connections.len() {
        0 => "No connections".to_string(),
        1 => "1 connection".to_string(),
        n => format!("{n} connections"),
    }];
    match snapshot.running_queries {
        0 => {}
        1 => parts.push("1 query running".to_string()),
        n => parts.push(format!("{n} queries running")),
    }
    if snapshot.connecting {
        parts.push("Connecting…".to_string());
    }
    parts.join(" · ")
}

/// The whole menu for `snapshot`.
pub fn build_menu(snapshot: &TraySnapshot) -> Vec<TrayItem> {
    let connected = !snapshot.connections.is_empty();
    vec![
        TrayItem::Label(status_text(snapshot)),
        TrayItem::Separator,
        TrayItem::action(TrayCommand::Open, "Open DBCrust", true),
        TrayItem::action(TrayCommand::NewQuery, "New query", connected),
        TrayItem::Separator,
        connect_menu(snapshot),
        TrayItem::Submenu {
            label: "Connections".into(),
            enabled: connected,
            items: snapshot.connections.iter().map(connection_menu).collect(),
        },
        TrayItem::action(TrayCommand::DisconnectAll, "Disconnect all", connected),
        TrayItem::Separator,
        TrayItem::action(TrayCommand::Settings, "Settings…", true),
        TrayItem::Separator,
        TrayItem::action(TrayCommand::Quit, "Quit DBCrust", true),
    ]
}

/// Saved sessions (sorted, capped), recents that are neither duplicated nor
/// already saved (capped), then "New connection…".
fn connect_menu(snapshot: &TraySnapshot) -> TrayItem {
    let mut sessions: Vec<&String> = snapshot.sessions.iter().collect();
    sessions.sort_by_key(|name| name.to_lowercase());
    sessions.dedup();
    sessions.truncate(MAX_SESSIONS);

    let saved: HashSet<String> = snapshot
        .sessions
        .iter()
        .map(|name| name.to_lowercase())
        .collect();
    let mut seen_keys = HashSet::new();
    let mut seen_labels = HashSet::new();
    let recents: Vec<&TrayRecent> = snapshot
        .recents
        .iter()
        .filter(|recent| {
            let label = recent.label.to_lowercase();
            !saved.contains(&label) && seen_keys.insert(recent.key) && seen_labels.insert(label)
        })
        .take(MAX_RECENTS)
        .collect();

    let mut items = Vec::new();
    if !sessions.is_empty() {
        items.push(TrayItem::Label("Saved".into()));
        items.extend(sessions.into_iter().map(|name| {
            TrayItem::action(
                TrayCommand::ConnectSession(name.clone()),
                shorten(name),
                true,
            )
        }));
    }
    if !recents.is_empty() {
        if !items.is_empty() {
            items.push(TrayItem::Separator);
        }
        items.push(TrayItem::Label("Recent".into()));
        items.extend(recents.into_iter().map(|recent| {
            TrayItem::action(
                TrayCommand::ConnectRecent(recent.key),
                shorten(&recent.label),
                true,
            )
        }));
    }
    if !items.is_empty() {
        items.push(TrayItem::Separator);
    }
    items.push(TrayItem::action(
        TrayCommand::NewConnection,
        "New connection…",
        true,
    ));
    TrayItem::Submenu {
        label: "Connect".into(),
        enabled: true,
        items,
    }
}

fn connection_menu(connection: &TrayConnection) -> TrayItem {
    let mut title = format!("{} · {}", shorten(&connection.label), connection.engine);
    if connection.read_only {
        title.push_str(" · read-only");
    }
    TrayItem::Submenu {
        label: title,
        enabled: true,
        items: vec![
            TrayItem::action(TrayCommand::OpenConnection(connection.id), "Open", true),
            TrayItem::action(
                TrayCommand::ConnectionNewQuery(connection.id),
                "New query",
                true,
            ),
            TrayItem::action(TrayCommand::Disconnect(connection.id), "Disconnect", true),
        ],
    }
}

/// Cut a long name so the menu stays narrow.
fn shorten(label: &str) -> String {
    let label = label.trim();
    if label.chars().count() <= MAX_LABEL_CHARS {
        return label.to_string();
    }
    let kept: String = label.chars().take(MAX_LABEL_CHARS - 1).collect();
    format!("{}…", kept.trim_end())
}

/// OS menus treat `&` as a mnemonic marker (`&&` shows one).
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub fn escape_mnemonics(label: &str) -> String {
    label.replace('&', "&&")
}

#[cfg(test)]
mod tests {
    // GPUI's `test` macro is not in scope here, but keep the same prelude as
    // the other test modules
    use std::prelude::v1::test;

    use super::*;
    use rstest::rstest;

    fn connection(id: u64, label: &str, engine: &str) -> TrayConnection {
        TrayConnection {
            id,
            label: label.into(),
            engine: engine.into(),
            read_only: false,
        }
    }

    fn recent(label: &str, url: &str) -> TrayRecent {
        TrayRecent {
            key: recent_key(url),
            label: label.into(),
        }
    }

    fn submenu<'a>(menu: &'a [TrayItem], title: &str) -> &'a [TrayItem] {
        menu.iter()
            .find_map(|item| match item {
                TrayItem::Submenu { label, items, .. } if label == title => Some(items.as_slice()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no submenu {title}"))
    }

    fn action_labels(items: &[TrayItem]) -> Vec<&str> {
        items
            .iter()
            .filter_map(|item| match item {
                TrayItem::Action { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect()
    }

    // ----- Closing -----

    #[rstest]
    #[case(true, true, true, CloseAction::Hide)]
    #[case(false, true, true, CloseAction::Quit)] // the user turned it off
    #[case(true, false, true, CloseAction::Quit)] // no tray icon: no way back
    #[case(true, true, false, CloseAction::Quit)] // Linux: windows cannot hide
    #[case(false, false, false, CloseAction::Quit)]
    fn closing_hides_only_with_a_way_back(
        #[case] keep_running: bool,
        #[case] tray_active: bool,
        #[case] can_hide: bool,
        #[case] expected: CloseAction,
    ) {
        assert_eq!(close_action(keep_running, tray_active, can_hide), expected);
    }

    #[rstest]
    #[case(true, false, "menu bar or the Dock", "⌘Q")]
    #[case(true, true, "from the menu bar;", "⌘Q")]
    #[case(false, false, "notification area", "Ctrl+Q")]
    fn first_close_message_names_the_platform(
        #[case] mac: bool,
        #[case] dock_hidden: bool,
        #[case] fragment: &str,
        #[case] quit: &str,
    ) {
        let message = Wording::for_platform(mac, dock_hidden).first_close_message();
        assert!(message.starts_with("DBCrust will keep running in the "));
        assert!(message.contains("Connections and queries stay active."));
        assert!(message.contains(fragment), "{message}");
        assert!(message.ends_with(&format!("{quit} quits.")), "{message}");
        if dock_hidden {
            assert!(!message.contains("Dock"), "{message}");
        }
    }

    // ----- Commands -----

    #[rstest]
    #[case(TrayCommand::Open)]
    #[case(TrayCommand::NewQuery)]
    #[case(TrayCommand::NewConnection)]
    #[case(TrayCommand::ConnectSession("prod: eu/west, a:b".into()))]
    #[case(TrayCommand::ConnectRecent(recent_key("postgres://app@db/shop")))]
    #[case(TrayCommand::OpenConnection(7))]
    #[case(TrayCommand::ConnectionNewQuery(12))]
    #[case(TrayCommand::Disconnect(3))]
    #[case(TrayCommand::DisconnectAll)]
    #[case(TrayCommand::Settings)]
    #[case(TrayCommand::Quit)]
    fn command_ids_round_trip(#[case] command: TrayCommand) {
        assert_eq!(TrayCommand::parse(&command.id()), Some(command));
    }

    #[rstest]
    #[case("")]
    #[case("nope")]
    #[case("conn-query:x")]
    #[case("recent:zz")]
    #[case("session")]
    fn unknown_ids_are_ignored(#[case] id: &str) {
        assert_eq!(TrayCommand::parse(id), None);
    }

    #[test]
    fn recent_ids_do_not_carry_the_url() {
        let url = "postgres://app@db.internal:5432/shop";
        let id = TrayCommand::ConnectRecent(recent_key(url)).id();
        assert!(!id.contains("db.internal") && !id.contains("postgres"));
        assert_eq!(recent_key(url), recent_key(url));
        assert_ne!(
            recent_key(url),
            recent_key("postgres://app@db.internal:5432/other")
        );
    }

    // ----- Status -----

    #[rstest]
    #[case(0, 0, false, "No connections")]
    #[case(1, 0, false, "1 connection")]
    #[case(2, 1, false, "2 connections · 1 query running")]
    #[case(3, 2, false, "3 connections · 2 queries running")]
    #[case(0, 0, true, "No connections · Connecting…")]
    #[case(1, 1, true, "1 connection · 1 query running · Connecting…")]
    fn status_line(
        #[case] connections: usize,
        #[case] running: usize,
        #[case] connecting: bool,
        #[case] expected: &str,
    ) {
        let snapshot = TraySnapshot {
            connections: (0..connections as u64)
                .map(|id| connection(id, "db", "SQLite"))
                .collect(),
            running_queries: running,
            connecting,
            ..TraySnapshot::default()
        };
        assert_eq!(status_text(&snapshot), expected);
    }

    // ----- Menu -----

    #[test]
    fn idle_menu_disables_what_needs_a_connection() {
        let menu = build_menu(&TraySnapshot::default());
        assert_eq!(menu[0], TrayItem::Label("No connections".into()));
        let state = |wanted: &TrayCommand| {
            menu.iter().find_map(|item| match item {
                TrayItem::Action {
                    command, enabled, ..
                } if command == wanted => Some(*enabled),
                _ => None,
            })
        };
        assert_eq!(state(&TrayCommand::Open), Some(true));
        assert_eq!(state(&TrayCommand::NewQuery), Some(false));
        assert_eq!(state(&TrayCommand::DisconnectAll), Some(false));
        assert_eq!(state(&TrayCommand::Settings), Some(true));
        assert_eq!(state(&TrayCommand::Quit), Some(true));
        assert!(matches!(
            menu.iter().find(|item| matches!(item, TrayItem::Submenu { label, .. } if label == "Connections")),
            Some(TrayItem::Submenu { enabled: false, items, .. }) if items.is_empty()
        ));
        // The last entry is always Quit, set apart
        assert!(matches!(
            menu.last(),
            Some(TrayItem::Action {
                command: TrayCommand::Quit,
                ..
            })
        ));
        assert_eq!(menu[menu.len() - 2], TrayItem::Separator);
    }

    #[test]
    fn empty_connect_menu_offers_only_a_new_connection() {
        let menu = build_menu(&TraySnapshot::default());
        let connect = submenu(&menu, "Connect");
        assert_eq!(connect.len(), 1);
        assert_eq!(action_labels(connect), ["New connection…"]);
    }

    #[test]
    fn live_connections_get_open_new_query_and_disconnect() {
        let snapshot = TraySnapshot {
            connections: vec![
                connection(4, "shop", "PostgreSQL"),
                TrayConnection {
                    read_only: true,
                    ..connection(9, "reports", "MySQL")
                },
            ],
            running_queries: 1,
            ..TraySnapshot::default()
        };
        let menu = build_menu(&snapshot);
        assert_eq!(
            menu[0],
            TrayItem::Label("2 connections · 1 query running".into())
        );
        let connections = submenu(&menu, "Connections");
        assert_eq!(connections.len(), 2);
        let shop = submenu(connections, "shop · PostgreSQL");
        assert_eq!(action_labels(shop), ["Open", "New query", "Disconnect"]);
        assert!(matches!(
            &shop[2],
            TrayItem::Action {
                command: TrayCommand::Disconnect(4),
                enabled: true,
                ..
            }
        ));
        // Read-only connections say so
        submenu(connections, "reports · MySQL · read-only");
        // The quick actions are live now
        assert!(menu.iter().any(|item| matches!(
            item,
            TrayItem::Action {
                command: TrayCommand::NewQuery,
                enabled: true,
                ..
            }
        )));
        assert!(menu.iter().any(|item| matches!(
            item,
            TrayItem::Action {
                command: TrayCommand::DisconnectAll,
                enabled: true,
                ..
            }
        )));
    }

    #[test]
    fn saved_sessions_are_sorted_ignoring_case_and_capped() {
        let mut sessions: Vec<String> = (0..20).map(|n| format!("session {n:02}")).collect();
        sessions.push("Alpha".into());
        sessions.push("beta".into());
        sessions.reverse();
        let menu = build_menu(&TraySnapshot {
            sessions,
            ..TraySnapshot::default()
        });
        let connect = submenu(&menu, "Connect");
        let labels = action_labels(connect);
        assert_eq!(labels.len(), MAX_SESSIONS + 1, "sessions + New connection…");
        assert_eq!(&labels[..3], ["Alpha", "beta", "session 00"]);
        assert_eq!(*labels.last().expect("last"), "New connection…");
        assert_eq!(connect[0], TrayItem::Label("Saved".into()));
    }

    #[test]
    fn recents_are_deduplicated_capped_and_skip_saved_sessions() {
        let menu = build_menu(&TraySnapshot {
            sessions: vec!["Shop".into()],
            recents: vec![
                recent("shop", "postgres://a/shop"), // same name as a saved session
                recent("analytics", "postgres://a/analytics"),
                recent("Analytics", "postgres://b/analytics"), // same label
                recent("analytics (copy)", "postgres://a/analytics"), // same url
                recent("logs", "postgres://a/logs"),
                recent("r3", "postgres://a/r3"),
                recent("r4", "postgres://a/r4"),
                recent("r5", "postgres://a/r5"),
                recent("r6", "postgres://a/r6"),
            ],
            ..TraySnapshot::default()
        });
        let connect = submenu(&menu, "Connect");
        let labels = action_labels(connect);
        assert_eq!(
            labels,
            [
                "Shop",
                "analytics",
                "logs",
                "r3",
                "r4",
                "r5",
                "New connection…"
            ]
        );
        // Sections are headed and separated
        assert!(connect.contains(&TrayItem::Label("Recent".into())));
        assert_eq!(
            connect
                .iter()
                .filter(|item| **item == TrayItem::Separator)
                .count(),
            2
        );
    }

    #[test]
    fn long_names_are_shortened_and_never_show_urls() {
        let long = "a very long saved connection name that would make the menu absurdly wide";
        let menu = build_menu(&TraySnapshot {
            sessions: vec![long.into()],
            recents: vec![recent("shop", "postgres://user:secret@db.internal/shop")],
            ..TraySnapshot::default()
        });
        let connect = submenu(&menu, "Connect");
        for label in action_labels(connect) {
            assert!(label.chars().count() <= MAX_LABEL_CHARS, "{label}");
            assert!(!label.contains("secret") && !label.contains("postgres://"));
        }
        assert!(action_labels(connect)[0].ends_with('…'));
        // The command still names the full session
        assert!(connect.iter().any(|item| matches!(
            item,
            TrayItem::Action { command: TrayCommand::ConnectSession(name), .. } if name == long
        )));
    }

    #[test]
    fn equal_snapshots_build_equal_menus() {
        let snapshot = TraySnapshot {
            connections: vec![connection(1, "shop", "SQLite")],
            sessions: vec!["b".into(), "a".into()],
            ..TraySnapshot::default()
        };
        assert_eq!(build_menu(&snapshot), build_menu(&snapshot.clone()));
    }

    #[test]
    fn mnemonic_markers_are_escaped() {
        assert_eq!(escape_mnemonics("R&D"), "R&&D");
        assert_eq!(escape_mnemonics("plain"), "plain");
    }
}
