//! The slim toolbar above the editor: connection picker, Run/Cancel,
//! Run all, Explain, saved queries, history, file menu, and the execution
//! target of the next Run.

use super::{QueryTab, history};
use crate::actions::{
    CancelQuery, ExplainAnalyze, ExplainStatement, FormatSql, NewConnection, OpenSqlFile, RunAll,
    RunStatement, SaveNamedQuery, SaveSqlFile,
};
use crate::app_state::AppState;
use crate::widgets::{connection_accent, db_badge};
use dbcrust_gui_core::history::{HistoryEntry, HistoryOutcome, format_duration, preview_sql};
use dbcrust_gui_core::{ConnectionId, ConnectionSummary, DatabaseType};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _, DropdownButton},
    h_flex,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    separator::Separator,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

/// How many history rows the menu lists.
const HISTORY_ROWS: usize = 60;
/// Characters of a statement shown in menus.
const PREVIEW_CHARS: usize = 96;

/// How much of the toolbar fits: labels while there is room, icons when it
/// narrows, and the lesser menus folded into "⋯" when it is narrow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Density {
    Full,
    Compact,
    Narrow,
}

impl Density {
    /// Width of the toolbar needed for every label.
    const FULL_FROM: f32 = 980.;
    /// Width below which Saved queries and History move into "⋯".
    const NARROW_BELOW: f32 = 740.;

    /// `width` is what the last frame measured (zero before the first).
    pub(super) fn of(width: Pixels) -> Self {
        let width = width.as_f32();
        if width <= 0. || width >= Self::FULL_FROM {
            Density::Full
        } else if width >= Self::NARROW_BELOW {
            Density::Compact
        } else {
            Density::Narrow
        }
    }
}

impl QueryTab {
    pub(super) fn render_toolbar(&self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let live = self.live(cx);
        let bound = live.is_some();
        let running = self.is_running();
        let cancelling = self.is_cancelling();
        let blank = self.is_blank(cx);
        let can_run = bound && !blank;
        let supports_analyze = live
            .as_ref()
            .is_some_and(|(_, s)| s.supports_explain_analyze());
        let db_type = live.as_ref().map(|(_, s)| s.db_type.clone());
        let view = cx.entity().downgrade();
        let density = Density::of(self.toolbar_width.get());
        let full = density == Density::Full;
        let narrow = density == Density::Narrow;
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let border = theme.border;
        let background = theme.background;

        let run_button = if running {
            Button::new("query-cancel")
                .danger()
                .small()
                .icon(IconName::Square)
                .label(if cancelling {
                    "Cancelling…"
                } else {
                    "Cancel"
                })
                .loading(cancelling)
                .disabled(cancelling)
                .tooltip_with_action(
                    "Cancel the running statement",
                    &CancelQuery,
                    Some("QueryTab"),
                )
                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
        } else {
            // The label says what runs: the statement under the cursor, or
            // the selection
            Button::new("query-run")
                .primary()
                .small()
                .icon(IconName::Play)
                .label(self.hint.run_label())
                .disabled(!can_run)
                .tooltip_with_action(
                    if bound {
                        self.hint.tooltip()
                    } else {
                        "Choose a connection to run queries".to_string()
                    },
                    &RunStatement,
                    Some("QueryTab"),
                )
                .on_click(cx.listener(|this, _, window, cx| this.run_current(window, cx)))
        };
        let run_detail = self.hint.detail().filter(|_| !running && bound);

        let run_all = Button::new("query-run-all")
            .ghost()
            .small()
            .icon(IconName::ListVideo)
            .label(self.hint.run_all_label(narrow))
            .disabled(!can_run || running)
            .tooltip_with_action(
                "Run every statement of the selection (or the editor), stopping at the first error",
                &RunAll,
                Some("QueryTab"),
            )
            .on_click(cx.listener(|this, _, window, cx| this.run_all(window, cx)));

        let explain = DropdownButton::new("query-explain")
            .button(
                Button::new("query-explain-run")
                    .ghost()
                    .small()
                    .icon(IconName::ScanSearch)
                    .when(full, |button| button.label("Explain"))
                    .disabled(!can_run || running)
                    .tooltip_with_action("Show the query plan", &ExplainStatement, Some("QueryTab"))
                    .on_click(cx.listener(|this, _, window, cx| this.explain(false, window, cx))),
            )
            .disabled(!can_run || running)
            .dropdown_menu(move |menu, _, _| {
                menu.min_w(px(250.))
                    .menu_with_icon("Explain", IconName::ScanSearch, Box::new(ExplainStatement))
                    .menu_with_icon_and_disabled(
                        if supports_analyze {
                            "Explain Analyze (runs the statement)"
                        } else {
                            "Explain Analyze (PostgreSQL only)"
                        },
                        IconName::Activity,
                        Box::new(ExplainAnalyze),
                        !supports_analyze,
                    )
            });

        let save = Button::new("query-save")
            .ghost()
            .small()
            .icon(IconName::Save)
            .when(!narrow, |button| button.label("Save"))
            .disabled(blank && self.baseline.is_none())
            .tooltip_with_action(
                match self.target_description() {
                    Some(target) => format!("Save to {target}"),
                    None => "Save this query…".to_string(),
                },
                &SaveNamedQuery,
                Some("QueryTab"),
            )
            .on_click(cx.listener(|this, _, window, cx| this.save_query(window, cx)));

        let saved = Button::new("query-saved")
            .ghost()
            .small()
            .icon(IconName::Bookmark)
            .when(full, |button| {
                button.label("Saved queries").dropdown_caret(true)
            })
            .tooltip("Saved queries")
            .dropdown_menu({
                let view = view.clone();
                let db_type = db_type.clone();
                move |menu, _, cx| saved_queries_menu(menu, &view, db_type.as_ref(), cx)
            });

        let history_button = Button::new("query-history")
            .ghost()
            .small()
            .icon(IconName::Clock)
            .when(full, |button| button.label("History").dropdown_caret(true))
            .tooltip("History")
            .dropdown_menu({
                let view = view.clone();
                move |menu, _, cx| history_menu(menu, &view, cx)
            });

        let more = Button::new("query-files")
            .ghost()
            .small()
            .icon(IconName::Ellipsis)
            .tooltip("More: format, open and save files")
            .dropdown_menu({
                let view = view.clone();
                let db_type = db_type.clone();
                move |menu, window, cx| {
                    let to_file = view.clone();
                    let mut menu = menu
                        .min_w(px(230.))
                        .menu_with_icon("Format SQL", IconName::WandSparkles, Box::new(FormatSql))
                        .separator();
                    if narrow {
                        menu = menu
                            .submenu_with_icon(
                                Some(IconName::Bookmark.into()),
                                "Saved queries",
                                window,
                                cx,
                                {
                                    let view = view.clone();
                                    let db_type = db_type.clone();
                                    move |menu, _, cx| {
                                        saved_queries_menu(menu, &view, db_type.as_ref(), cx)
                                    }
                                },
                            )
                            .submenu_with_icon(
                                Some(IconName::Clock.into()),
                                "History",
                                window,
                                cx,
                                {
                                    let view = view.clone();
                                    move |menu, _, cx| history_menu(menu, &view, cx)
                                },
                            )
                            .separator();
                    }
                    menu.menu_with_icon(
                        "Open SQL file…",
                        IconName::FolderOpen,
                        Box::new(OpenSqlFile),
                    )
                    .menu_with_icon("Save as…", IconName::Save, Box::new(SaveSqlFile))
                    .item(
                        PopupMenuItem::new("Save to a .sql file…")
                            .icon(IconName::FileDown)
                            .on_click(move |_, window, cx| {
                                to_file
                                    .update(cx, |tab, cx| tab.save_sql_file(true, window, cx))
                                    .ok();
                            }),
                    )
                }
            });

        // Measures the toolbar so the next frame knows which labels fit
        let measured = self.toolbar_width.clone();
        let ruler = canvas(
            move |bounds, window, _| {
                let before = Density::of(measured.get());
                measured.set(bounds.size.width);
                if Density::of(bounds.size.width) != before {
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        h_flex()
            .relative()
            .flex_none()
            .w_full()
            .h(px(36.))
            .px_2()
            .gap_1()
            .items_center()
            .border_b_1()
            .border_color(border)
            .bg(background)
            .child(ruler)
            .child(self.connection_picker(live.as_ref().map(|(_, s)| s), view, cx))
            // Which environment this runs against, in words and colour
            .children(live.as_ref().and_then(|(_, s)| environment_chip(s.id, cx)))
            .when(live.as_ref().is_some_and(|(_, s)| s.read_only), |this| {
                this.child(read_only_chip(cx))
            })
            .child(Separator::vertical().h_4().mx_1())
            .child(run_button)
            .when_some(run_detail, |this, detail| {
                this.child(
                    div()
                        .flex_none()
                        .px_0p5()
                        .text_size(px(11.))
                        .text_color(muted)
                        .child(detail),
                )
            })
            .child(run_all)
            .child(explain)
            .child(Separator::vertical().h_4().mx_1())
            .child(save)
            .when(!narrow, |this| this.child(saved).child(history_button))
            .child(more)
            .into_any_element()
    }

    fn connection_picker(
        &self,
        summary: Option<&ConnectionSummary>,
        view: WeakEntity<QueryTab>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = self.connection;
        let (content, tooltip) = match summary {
            Some(summary) => {
                let accent = AppState::global(cx)
                    .read(cx)
                    .connection(summary.id)
                    .map_or(0, |live| live.accent);
                (
                    connection_label(summary, accent, cx).into_any_element(),
                    format!(
                        "This tab runs on “{}” ({}) · {}. Click to run it on another connection.",
                        summary.label,
                        summary.type_name(),
                        summary.target()
                    ),
                )
            }
            None => (
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .text_color(cx.theme().warning)
                    .child(Icon::new(IconName::Unplug).small())
                    .child("Choose a connection")
                    .into_any_element(),
                "This tab is not connected. Choose where it runs.".to_string(),
            ),
        };
        Button::new("query-connection")
            .ghost()
            .small()
            .tooltip(tooltip)
            .child(content)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, cx| connection_menu(menu, &view, current, cx))
            .into_any_element()
    }
}

/// The environment label of a live connection (`Production`, red), as the
/// Connections page set it: always text with its colour.
fn environment_chip(connection: ConnectionId, cx: &App) -> Option<impl IntoElement + use<>> {
    let tag = crate::app_state::AppState::global(cx)
        .read(cx)
        .environment_of(connection)?;
    Some(
        div()
            .id("query-environment")
            .flex_none()
            .child(crate::widgets::env_tag(&tag, cx))
            .tooltip(|window, cx| {
                Tooltip::new("Environment of this connection (set on the Connections page)")
                    .build(window, cx)
            }),
    )
}

/// "Read-only" next to the execution target: statements that change data
/// are blocked.
fn read_only_chip(cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .id("query-read-only")
        .flex_none()
        .h(px(20.))
        .px_1p5()
        .gap_1()
        .items_center()
        .rounded(theme.radius)
        .bg(theme.warning.opacity(0.14))
        .text_color(theme.warning)
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .child(Icon::new(IconName::Lock).size_3())
        .child("Read-only")
        .tooltip(|window, cx| {
            Tooltip::new("Read-only connection: statements that change data are blocked")
                .build(window, cx)
        })
}

/// Accent dot, engine badge and label.
fn connection_label(summary: &ConnectionSummary, accent: usize, cx: &App) -> Div {
    h_flex()
        .gap_1p5()
        .items_center()
        .child(
            div()
                .size_2()
                .flex_none()
                .rounded_full()
                .bg(connection_accent(accent, cx)),
        )
        .child(db_badge(&summary.db_type, cx))
        .child(
            div()
                .max_w(px(220.))
                .truncate()
                .text_color(cx.theme().foreground)
                .child(summary.label.clone()),
        )
}

struct ConnectionRow {
    id: ConnectionId,
    summary: ConnectionSummary,
    accent: usize,
}

fn connection_menu(
    menu: PopupMenu,
    view: &WeakEntity<QueryTab>,
    current: Option<ConnectionId>,
    cx: &App,
) -> PopupMenu {
    let rows: Vec<ConnectionRow> = AppState::global(cx)
        .read(cx)
        .connections()
        .iter()
        .map(|live| ConnectionRow {
            id: live.id(),
            summary: live.summary.clone(),
            accent: live.accent,
        })
        .collect();
    let mut menu = menu.min_w(px(280.)).max_w(px(480.));
    if rows.is_empty() {
        menu = menu.item(PopupMenuItem::label("No open connections"));
    }
    for row in rows {
        let id = row.id;
        let view = view.clone();
        menu = menu.item(
            PopupMenuItem::element(move |_, cx| {
                v_flex()
                    .gap_0p5()
                    .child(connection_label(&row.summary, row.accent, cx).when(
                        row.summary.read_only,
                        |this| {
                            this.child(
                                Icon::new(IconName::Lock)
                                    .xsmall()
                                    .text_color(cx.theme().warning),
                            )
                        },
                    ))
                    .child(
                        div()
                            .pl_4()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(row.summary.target()),
                    )
            })
            .checked(current == Some(id))
            .on_click(move |_, window, cx| {
                view.update(cx, |tab, cx| tab.set_connection(Some(id), window, cx))
                    .ok();
                // Connection commands (disconnect, read-only, …) follow the
                // tab in front
                AppState::global(cx).update(cx, |state, cx| state.set_active(Some(id), cx));
            }),
        );
    }
    menu.separator()
        .menu_with_icon("New connection…", IconName::Plus, Box::new(NewConnection))
}

fn saved_queries_menu(
    menu: PopupMenu,
    view: &WeakEntity<QueryTab>,
    db_type: Option<&DatabaseType>,
    cx: &App,
) -> PopupMenu {
    let queries = AppState::global(cx).read(cx).store().named_queries(db_type);
    let mut menu = menu
        .min_w(px(320.))
        .max_w(px(560.))
        .max_h(px(380.))
        .scrollable(true);
    if queries.is_empty() {
        return menu
            .item(PopupMenuItem::label("No saved queries yet"))
            .item(PopupMenuItem::label(
                "Use Save to keep the current query here",
            ));
    }
    menu = menu.item(PopupMenuItem::label(
        "Saved queries · click to insert or open",
    ));
    for query in queries {
        let view = view.clone();
        let (name, sql, scope) = (
            query.name.clone(),
            query.query.clone(),
            query.scope_label.clone(),
        );
        let preview = preview_sql(&sql, PREVIEW_CHARS);
        menu = menu.item(
            PopupMenuItem::element(move |_, cx| {
                v_flex()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_baseline()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(query.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(query.scope_label.clone()),
                            ),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.))
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(cx.theme().muted_foreground)
                            .child(preview.clone()),
                    )
            })
            .on_click(move |_, window, cx| {
                let (name, sql, scope) = (name.clone(), sql.clone(), scope.clone());
                view.update(cx, |tab, cx| {
                    tab.apply_named_query(name, sql, &scope, window, cx)
                })
                .ok();
            }),
        );
    }
    menu
}

fn history_menu(menu: PopupMenu, view: &WeakEntity<QueryTab>, cx: &App) -> PopupMenu {
    let entries = history::recent(HISTORY_ROWS, cx);
    let mut menu = menu
        .min_w(px(420.))
        .max_w(px(640.))
        .max_h(px(420.))
        .scrollable(true);
    if entries.is_empty() {
        return menu.item(PopupMenuItem::label("Nothing run yet"));
    }
    menu = menu.item(PopupMenuItem::label("Recent statements · click to load"));
    for entry in entries {
        let view = view.clone();
        let sql = entry.sql.clone();
        menu = menu.item(
            PopupMenuItem::element(move |_, cx| history_row(&entry, cx)).on_click(
                move |_, window, cx| {
                    let sql = sql.clone();
                    view.update(cx, |tab, cx| tab.load_text(sql, window, cx))
                        .ok();
                },
            ),
        );
    }
    menu
}

fn history_row(entry: &HistoryEntry, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (icon, color) = match &entry.outcome {
        HistoryOutcome::Ok => (IconName::CircleCheck, theme.success),
        HistoryOutcome::Error(_) => (IconName::CircleX, theme.danger),
        HistoryOutcome::Cancelled => (IconName::Ban, theme.muted_foreground),
    };
    let mut meta = vec![entry.age(), format_duration(entry.elapsed)];
    if let Some(rows) = entry.rows {
        meta.push(if rows == 1 {
            "1 row".to_string()
        } else {
            format!("{rows} rows")
        });
    }
    if let HistoryOutcome::Error(message) = &entry.outcome {
        meta.push(message.lines().next().unwrap_or_default().to_string());
    }
    if !entry.connection_label.is_empty() {
        meta.push(entry.connection_label.clone());
    }
    h_flex()
        .w_full()
        .gap_2()
        .items_start()
        .child(Icon::new(icon).xsmall().text_color(color).mt_0p5())
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .truncate()
                        .text_size(px(12.))
                        .font_family(theme.mono_font_family.clone())
                        .child(entry.preview(PREVIEW_CHARS)),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(10.))
                        .text_color(theme.muted_foreground)
                        .child(meta.join(" · ")),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // GPUI's `test` macro (glob-imported through `super::*`) would shadow
    // the built-in `#[test]`
    use std::prelude::v1::test;

    use super::Density;
    use gpui_kit::px;
    use rstest::rstest;

    #[rstest]
    #[case(0., Density::Full)]
    #[case(1400., Density::Full)]
    #[case(980., Density::Full)]
    #[case(979., Density::Compact)]
    #[case(740., Density::Compact)]
    #[case(739., Density::Narrow)]
    #[case(520., Density::Narrow)]
    fn labels_give_way_as_the_toolbar_narrows(#[case] width: f32, #[case] density: Density) {
        assert_eq!(Density::of(px(width)), density);
    }
}
