//! The command palette (⌘K): commands, connections, tables of the active
//! connection and named queries behind one fuzzy search field.
//!
//! The palette is GPUI Kit's `Command` hosted in a dialog. Filtering is ours
//! (subsequence match with word-start and adjacency bonuses, see
//! [`fuzzy_score`]), so the component's own substring filter is switched off.

use crate::app_state::AppState;
use crate::commands::AppCommand;
use crate::widgets::{connection_accent, db_badge};
use crate::workspace::tabs::{TabBinding, TabChoice};
use dbcrust_gui_core::{
    ConnectRequest, ConnectionId, DatabaseType, TabId, TableEntry, TableKind, store::RecentEntry,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IndexPath, Sizable as _, WindowExt as _,
    command::{Command, CommandGroup, CommandItem, CommandState},
    h_flex,
    kbd::Kbd,
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use strum::IntoEnumIterator;

/// How many rows each group shows, with and without a query.
const LIMIT_COMMANDS: usize = 14;
const LIMIT_CONNECTIONS: usize = 8;
const LIMIT_TABLES: usize = 25;
const LIMIT_QUERIES: usize = 10;
const LIMIT_TABS: usize = 50;
const RECENT_CONNECTIONS: usize = 5;

/// Subsequence match of `query` in `text`, case-insensitive. `None` when some
/// query character cannot be found in order; otherwise higher is better
/// (contiguous runs, word starts and early matches score more).
pub(crate) fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();

    let mut score = 0;
    let mut next = 0; // next query char to place
    let mut previous_match: Option<usize> = None;
    for (ix, &ch) in text.iter().enumerate() {
        if next == query.len() {
            break;
        }
        if ch != query[next] {
            continue;
        }
        let word_start = ix == 0 || !text[ix - 1].is_alphanumeric();
        score += 10;
        if word_start {
            score += 8;
        }
        match previous_match {
            Some(previous) if previous + 1 == ix => score += 6,
            Some(previous) => score -= ((ix - previous - 1) as i32).min(8),
            None => score -= (ix as i32).min(10),
        }
        previous_match = Some(ix);
        next += 1;
    }
    if next < query.len() {
        return None;
    }

    let haystack: String = text.iter().collect();
    let needle: String = query.iter().collect();
    if let Some(at) = haystack.find(&needle) {
        score += 30;
        if at == 0 {
            score += 20;
        }
    }
    Some(score)
}

/// Every whitespace-separated word of `query` must match `label` or the
/// extra `keywords`; a label match counts double.
pub(crate) fn match_score(query: &str, label: &str, keywords: &str) -> Option<i32> {
    let mut total = 0;
    for word in query.split_whitespace() {
        let in_label = fuzzy_score(word, label).map(|score| score * 2);
        let in_keywords = fuzzy_score(word, keywords);
        total += in_label.into_iter().chain(in_keywords).max()?;
    }
    Some(total)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GroupKind {
    Commands,
    Connections,
    Tables,
    Queries,
    Tabs,
}

/// What the palette searches: everything (⌘K), or only the open tabs
/// ("Go to tab…").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    Everything,
    Tabs,
}

/// Switches to a tab; provided by the workspace, which owns them.
pub(crate) type TabPicker = Rc<dyn Fn(TabId, &mut Window, &mut App)>;

impl GroupKind {
    fn limit(self) -> usize {
        match self {
            GroupKind::Commands => LIMIT_COMMANDS,
            GroupKind::Connections => LIMIT_CONNECTIONS,
            GroupKind::Tables => LIMIT_TABLES,
            GroupKind::Queries => LIMIT_QUERIES,
            GroupKind::Tabs => LIMIT_TABS,
        }
    }
}

/// What confirming a row does.
#[derive(Clone)]
enum Target {
    Command(AppCommand),
    Switch(ConnectionId),
    ConnectSession(String),
    ConnectRecent(Box<RecentEntry>),
    OpenTable(ConnectionId, TableEntry),
    OpenQuery {
        connection: ConnectionId,
        name: String,
        sql: String,
    },
    Tab(TabId),
}

#[derive(Clone)]
enum Leading {
    Icon(IconName),
    Connection {
        accent: Option<usize>,
        db_type: DatabaseType,
    },
}

/// The connection a row acts on, shown at the end of the row so it is
/// never a guess which database "Disconnect" or a table belongs to.
#[derive(Clone)]
struct ConnectionTag {
    accent: usize,
    label: SharedString,
}

struct Entry {
    group: GroupKind,
    label: SharedString,
    detail: Option<SharedString>,
    /// Extra search terms.
    keywords: String,
    leading: Leading,
    /// Command whose key binding is shown at the end of the row.
    shortcut: Option<AppCommand>,
    mono: bool,
    disabled: bool,
    /// Set for rows that act on the active connection.
    context: Option<ConnectionTag>,
    target: Target,
}

/// Everything the palette can offer, captured when it opens.
struct Model {
    entries: Vec<Rc<Entry>>,
    table_heading: SharedString,
    scope: Scope,
}

impl Model {
    fn build(tabs: &[TabChoice], scope: Scope, cx: &App) -> Self {
        let state = AppState::global(cx);
        let state = state.read(cx);
        let store = state.store();
        let active = state.active();
        let active_tag = active.map(|live| ConnectionTag {
            accent: live.accent,
            label: live.summary.label.clone().into(),
        });
        let mut entries = Vec::new();

        for choice in tabs {
            entries.push(Rc::new(tab_entry(choice)));
        }
        if scope == Scope::Tabs {
            return Self {
                entries,
                table_heading: "Tables".into(),
                scope,
            };
        }

        for command in AppCommand::iter().filter(|command| command.in_palette()) {
            let needs_connection = needs_connection(command);
            let mut keywords = command.category().label().to_string();
            for keyword in command.keywords() {
                keywords.push(' ');
                keywords.push_str(keyword);
            }
            let unavailable = needs_connection && active.is_none();
            entries.push(Rc::new(Entry {
                group: GroupKind::Commands,
                label: command.label().into(),
                detail: Some(if unavailable {
                    format!("{} · needs a connection", command.category().label()).into()
                } else {
                    command.category().label().into()
                }),
                keywords,
                leading: Leading::Icon(command.icon()),
                shortcut: Some(command),
                mono: false,
                disabled: unavailable,
                context: needs_connection.then(|| active_tag.clone()).flatten(),
                target: Target::Command(command),
            }));
        }

        for live in state.connections() {
            let summary = &live.summary;
            entries.push(Rc::new(Entry {
                group: GroupKind::Connections,
                label: format!("Switch to {}", summary.label).into(),
                detail: Some(summary.target().into()),
                keywords: format!(
                    "{} {} {}",
                    summary.type_name(),
                    summary.database,
                    summary.host
                ),
                leading: Leading::Connection {
                    accent: Some(live.accent),
                    db_type: summary.db_type.clone(),
                },
                shortcut: None,
                mono: false,
                disabled: state.active_id() == Some(live.id()),
                context: None,
                target: Target::Switch(live.id()),
            }));
        }

        let live_sessions: Vec<&str> = state
            .connections()
            .iter()
            .filter_map(|live| live.summary.session_name.as_deref())
            .collect();
        for session in store.sessions() {
            if live_sessions.contains(&session.name.as_str()) {
                continue;
            }
            entries.push(Rc::new(Entry {
                group: GroupKind::Connections,
                label: format!("Connect to {}", session.name).into(),
                detail: Some(session.target.clone().into()),
                keywords: format!("session saved {} {}", session.database, session.user),
                leading: Leading::Connection {
                    accent: None,
                    db_type: session.db_type.clone(),
                },
                shortcut: None,
                mono: false,
                disabled: false,
                context: None,
                target: Target::ConnectSession(session.name.clone()),
            }));
        }
        let live_urls: Vec<&str> = state
            .connections()
            .iter()
            .map(|live| live.summary.display_url.as_str())
            .collect();
        for recent in store
            .recent_connections()
            .into_iter()
            .filter(|recent| !live_urls.contains(&recent.url.as_str()))
            .take(RECENT_CONNECTIONS)
        {
            entries.push(Rc::new(Entry {
                group: GroupKind::Connections,
                label: format!("Connect to {}", recent.display_name).into(),
                detail: Some("recent".into()),
                keywords: format!("recent {}", recent.url),
                leading: Leading::Connection {
                    accent: None,
                    db_type: recent.db_type.clone(),
                },
                shortcut: None,
                mono: false,
                disabled: false,
                context: None,
                target: Target::ConnectRecent(Box::new(recent)),
            }));
        }

        let mut table_heading: SharedString = "Tables".into();
        if let Some(live) = active {
            table_heading = format!("Tables in {}", live.summary.label).into();
            if let Some(tables) = live.schema.tables() {
                for table in tables.iter() {
                    let kind = table.kind.label().to_string();
                    entries.push(Rc::new(Entry {
                        group: GroupKind::Tables,
                        label: table.qualified_name().into(),
                        detail: Some(kind.clone().into()),
                        keywords: kind,
                        leading: Leading::Icon(table_icon(&table.kind)),
                        shortcut: None,
                        mono: true,
                        disabled: false,
                        context: active_tag.clone(),
                        target: Target::OpenTable(live.id(), table.clone()),
                    }));
                }
            }
            for query in store.named_queries(Some(&live.summary.db_type)) {
                entries.push(Rc::new(Entry {
                    group: GroupKind::Queries,
                    label: query.name.clone().into(),
                    detail: Some(query.scope_label.clone().into()),
                    keywords: query.query.clone(),
                    leading: Leading::Icon(IconName::BookmarkCheck),
                    shortcut: None,
                    mono: false,
                    disabled: false,
                    context: active_tag.clone(),
                    target: Target::OpenQuery {
                        connection: live.id(),
                        name: query.name,
                        sql: query.query,
                    },
                }));
            }
        }

        Self {
            entries,
            table_heading,
            scope,
        }
    }

    /// Entries matching `query`, best first within each non-empty group.
    fn visible(&self, query: &str) -> Vec<(GroupKind, Vec<Rc<Entry>>)> {
        let query = query.trim();
        [
            GroupKind::Tabs,
            GroupKind::Commands,
            GroupKind::Connections,
            GroupKind::Tables,
            GroupKind::Queries,
        ]
        .into_iter()
        // Open tabs show up in the general search once you type, not before
        .filter(|group| match (self.scope, group) {
            (Scope::Tabs, group) => *group == GroupKind::Tabs,
            (Scope::Everything, GroupKind::Tabs) => !query.is_empty(),
            (Scope::Everything, _) => true,
        })
        .filter_map(|group| {
            let mut scored: Vec<(i32, Rc<Entry>)> = self
                .entries
                .iter()
                .filter(|entry| entry.group == group)
                .filter_map(|entry| {
                    match_score(query, &entry.label, &entry.keywords)
                        .map(|score| (score, entry.clone()))
                })
                .collect();
            if !query.is_empty() {
                // Stable: equal scores keep declaration order
                scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
            }
            let entries: Vec<Rc<Entry>> = scored
                .into_iter()
                .map(|(_, entry)| entry)
                .take(group.limit())
                .collect();
            (!entries.is_empty()).then_some((group, entries))
        })
        .collect()
    }

    fn heading(&self, group: GroupKind) -> SharedString {
        match group {
            GroupKind::Commands => "Commands".into(),
            GroupKind::Connections => "Connections".into(),
            GroupKind::Tables => self.table_heading.clone(),
            GroupKind::Queries => "Named queries".into(),
            GroupKind::Tabs => "Open tabs".into(),
        }
    }

    fn placeholder(&self) -> &'static str {
        match self.scope {
            Scope::Everything => "Search commands, connections, tables, queries…",
            Scope::Tabs => "Go to tab…",
        }
    }
}

/// `Top customers` · `shop · SQLite`: a tab's row, with the connection it
/// is bound to so two tabs titled alike are told apart.
fn tab_entry(choice: &TabChoice) -> Entry {
    let (detail, keywords, leading) = match &choice.binding {
        TabBinding::Live(connection) => (
            format!("{} · {}", connection.name, connection.engine),
            format!("tab {} {}", connection.name, connection.engine),
            Leading::Connection {
                accent: Some(connection.accent),
                db_type: connection.db_type.clone(),
            },
        ),
        TabBinding::Detached => (
            "No connection".to_string(),
            "tab no connection disconnected".to_string(),
            Leading::Icon(choice.icon),
        ),
        TabBinding::None => (String::new(), "tab".to_string(), Leading::Icon(choice.icon)),
    };
    Entry {
        group: GroupKind::Tabs,
        label: choice.title.clone(),
        detail: (!detail.is_empty()).then(|| detail.into()),
        keywords,
        leading,
        shortcut: None,
        mono: false,
        disabled: false,
        context: None,
        target: Target::Tab(choice.id),
    }
}

/// Commands that act on the active connection (disabled without one, and
/// labelled with it otherwise).
fn needs_connection(command: AppCommand) -> bool {
    matches!(
        command,
        AppCommand::Disconnect
            | AppCommand::RefreshSchema
            | AppCommand::ToggleReadOnly
            | AppCommand::SaveSession
    )
}

fn table_icon(kind: &TableKind) -> IconName {
    match kind {
        TableKind::Table | TableKind::ForeignTable => IconName::Table,
        TableKind::View | TableKind::MaterializedView => IconName::Eye,
        TableKind::Collection => IconName::Layers,
        TableKind::Index => IconName::ListTree,
        TableKind::Other(_) => IconName::Table,
    }
}

impl Entry {
    fn perform(&self, tab_picker: &TabPicker, window: &mut Window, cx: &mut App) {
        match self.target.clone() {
            Target::Tab(id) => tab_picker(id, window, cx),
            Target::Command(command) => window.dispatch_action(command.action(), cx),
            Target::Switch(id) => AppState::global(cx).update(cx, |state, cx| {
                state.set_active(Some(id), cx);
            }),
            Target::ConnectSession(name) => connect(ConnectRequest::session(name), cx),
            Target::ConnectRecent(recent) => connect(ConnectRequest::recent(*recent), cx),
            Target::OpenTable(connection, table) => {
                AppState::global(cx)
                    .update(cx, |state, cx| state.open_table(connection, table, cx));
            }
            Target::OpenQuery {
                connection,
                name,
                sql,
            } => AppState::global(cx).update(cx, |state, cx| {
                state.open_query(connection, Some(name), sql, false, cx);
            }),
        }
    }

    fn render_row(&self, window: &Window, cx: &App) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let leading: AnyElement = match &self.leading {
            Leading::Icon(icon) => Icon::new(*icon)
                .small()
                .text_color(muted)
                .into_any_element(),
            Leading::Connection { accent, db_type } => h_flex()
                .gap_1p5()
                .items_center()
                .child(match accent {
                    Some(slot) => div()
                        .size_2()
                        .flex_none()
                        .rounded_full()
                        .bg(connection_accent(*slot, cx)),
                    // Not connected yet: a hollow dot
                    None => div()
                        .size_2()
                        .flex_none()
                        .rounded_full()
                        .border_1()
                        .border_color(muted),
                })
                .child(db_badge(db_type, cx))
                .into_any_element(),
        };

        let shortcut = self
            .shortcut
            .and_then(|command| Kbd::global_binding_for_action(command.action().as_ref(), window));

        h_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .items_center()
            .child(leading)
            .child(
                div()
                    .flex_none()
                    .max_w(px(320.))
                    .truncate()
                    .when(self.mono, |this| {
                        this.font_family(cx.theme().mono_font_family.clone())
                    })
                    .child(self.label.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(muted)
                    .when_some(self.detail.clone(), |this, detail| this.child(detail)),
            )
            .when_some(self.context.clone(), |this, tag| {
                this.child(connection_chip(&tag, cx))
            })
            .when_some(shortcut, |this, kbd| this.child(kbd))
            .into_any_element()
    }
}

/// `● shop`: the connection a row acts on.
fn connection_chip(tag: &ConnectionTag, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .flex_none()
        .max_w(px(150.))
        .h(px(18.))
        .px_1p5()
        .gap_1()
        .items_center()
        .rounded(px(4.))
        .bg(theme.muted)
        .text_size(px(11.))
        .text_color(theme.muted_foreground)
        .child(
            div()
                .flex_none()
                .size(px(6.))
                .rounded_full()
                .bg(connection_accent(tag.accent, cx)),
        )
        .child(div().min_w_0().truncate().child(tag.label.clone()))
}

fn connect(request: ConnectRequest, cx: &mut App) {
    // Failures surface through `AppEvent::ConnectFailed`
    AppState::global(cx)
        .update(cx, |state, cx| state.connect(request, cx))
        .detach();
}

/// Open the command palette on `window`. `tabs` are the open tabs (in tab
/// bar order), `pick_tab` activates one.
pub(crate) fn open(
    scope: Scope,
    tabs: Vec<TabChoice>,
    pick_tab: TabPicker,
    window: &mut Window,
    cx: &mut App,
) {
    if window.has_active_dialog(cx) {
        return;
    }
    let model = Rc::new(Model::build(&tabs, scope, cx));
    let state = cx.new(|cx| CommandState::new(window, cx));
    let query = Rc::new(RefCell::new(String::new()));
    let focus_on_open = Rc::new(Cell::new(true));

    window.open_dialog(cx, move |dialog, _, _| {
        let model = model.clone();
        let state = state.clone();
        let query = query.clone();
        let focus_on_open = focus_on_open.clone();
        let pick_tab = pick_tab.clone();
        dialog
            .close_button(false)
            .p_0()
            .width(px(600.))
            .margin_top(px(72.))
            .content(move |content, window, cx| {
                if focus_on_open.replace(false) {
                    let state = state.clone();
                    window.defer(cx, move |window, cx| {
                        state.read(cx).focus_handle(cx).focus(window, cx);
                    });
                }

                let groups = model.visible(&query.borrow());
                let command = groups.iter().fold(
                    Command::new(&state)
                        .bordered(false)
                        .filterable(false)
                        .max_h(px(380.))
                        .placeholder(model.placeholder()),
                    |command, (group, entries)| {
                        command.group(CommandGroup::new().label(model.heading(*group)).items(
                            entries.iter().map(|entry| {
                                let row = entry.clone();
                                CommandItem::new()
                                    .label(entry.label.clone())
                                    .disabled(entry.disabled)
                                    .child(move |window, cx| row.render_row(window, cx))
                            }),
                        ))
                    },
                );

                let on_query = query.clone();
                let on_confirm_model = model.clone();
                let on_confirm_query = query.clone();
                let on_confirm_tab = pick_tab.clone();
                content.child(
                    command
                        .on_query(move |text, window, _| {
                            if *on_query.borrow() != text {
                                *on_query.borrow_mut() = text.to_string();
                                window.refresh();
                            }
                        })
                        .on_confirm(move |path: IndexPath, window, cx| {
                            let groups = on_confirm_model.visible(&on_confirm_query.borrow());
                            let entry = groups
                                .get(path.section)
                                .and_then(|(_, entries)| entries.get(path.row))
                                .cloned();
                            if let Some(entry) = entry {
                                // Close first so focus returns to where the user was and
                                // the action reaches the workspace's handlers
                                window.close_dialog(cx);
                                entry.perform(&on_confirm_tab, window, cx);
                            }
                        })
                        .empty(|_, _, cx| {
                            v_flex()
                                .w_full()
                                .items_center()
                                .gap_2()
                                .py_6()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(Icon::new(IconName::Search).size_6())
                                .child("Nothing matches")
                        })
                        .footer(|state, _, cx| {
                            h_flex()
                                .justify_between()
                                .px_3()
                                .py_1p5()
                                .border_t_1()
                                .border_color(cx.theme().border)
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("↑↓ navigate  ↵ select  esc close")
                                .child(format!("{} results", state.matched_count()))
                        }),
                )
            })
    });
}

#[cfg(test)]
mod tests {
    // GPUI's `test` macro (glob-imported through `super::*`) would shadow
    // the built-in `#[test]`
    use std::prelude::v1::test;

    use super::*;

    #[test]
    fn connection_scoped_commands_are_identified() {
        assert!(needs_connection(AppCommand::Disconnect));
        assert!(needs_connection(AppCommand::RefreshSchema));
        assert!(!needs_connection(AppCommand::NewQuery));
    }

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(fuzzy_score("", "Run statement"), Some(0));
        assert_eq!(match_score("  ", "anything", ""), Some(0));
    }

    #[test]
    fn requires_an_ordered_subsequence() {
        assert!(fuzzy_score("rst", "Run statement").is_some());
        assert!(fuzzy_score("RUN", "run statement").is_some());
        assert!(fuzzy_score("rnu", "Run statement").is_none());
        assert!(fuzzy_score("xyz", "Run statement").is_none());
    }

    #[test]
    fn prefers_prefixes_contiguous_runs_and_word_starts() {
        let prefix = fuzzy_score("run", "Run statement").unwrap();
        let scattered = fuzzy_score("run", "Refresh unrelated").unwrap();
        assert!(prefix > scattered);

        let word_starts = fuzzy_score("ra", "Run all").unwrap();
        let buried = fuzzy_score("ra", "Format SQL").unwrap();
        assert!(word_starts > buried);

        let contiguous = fuzzy_score("stat", "Run statement").unwrap();
        // "stat" is only a scattered subsequence here (s, then, tag, it)
        let spread = fuzzy_score("stat", "Save as named query, then tag it").unwrap();
        assert!(contiguous > spread);
    }

    #[test]
    fn every_word_must_match_label_or_keywords() {
        assert!(match_score("run all", "Run all", "execute script").is_some());
        assert!(match_score("exec all", "Run all", "execute script").is_some());
        assert!(match_score("run zzz", "Run all", "execute script").is_none());
    }

    #[test]
    fn label_matches_outrank_keyword_matches() {
        let label = match_score("plan", "Plan", "").unwrap();
        let keyword = match_score("plan", "Explain statement", "plan").unwrap();
        assert!(label > keyword);
    }
}
