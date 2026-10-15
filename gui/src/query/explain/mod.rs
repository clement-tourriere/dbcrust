//! EXPLAIN plan view.
//!
//! Contract used by `ResultsView`: `ExplainView::new(raw, window, cx)`.
//!
//! The plan is parsed once into a `dbcrust_gui_core::explain::PlanModel`.
//! Tree plans (PostgreSQL, SQLite, MySQL) get a summary header, an indented
//! operation tree with cost bars and row estimates, and a details panel;
//! text plans (DataFusion, ClickHouse, …) get read-only code blocks. "Raw"
//! shows the engine's own output, "Copy plan" copies it (the JSON document
//! pastes straight into explain.dalibo.com).

mod details;
mod tree;

use dbcrust_gui_core::explain::{
    PlanBody, PlanModel, Severity, format_cost, format_count, timing_stats,
};
use dbcrust_gui_core::{DatabaseType, DatabaseTypeExt, ExplainRaw};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Editor, EditorState},
    notification::Notification,
    scroll::ScrollableElement as _,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::HashSet;
use std::sync::Arc;

pub struct ExplainView {
    focus_handle: FocusHandle,
    raw: Arc<ExplainRaw>,
    model: Arc<PlanModel>,
    selected: Option<usize>,
    /// Nodes whose subtree is folded.
    collapsed: HashSet<usize>,
    show_raw: bool,
    scroll: UniformListScrollHandle,
    /// Read-only text: the raw output, or the whole body of text plans.
    text_editor: Option<Entity<EditorState>>,
    /// One block per plan type (DataFusion).
    section_editors: Vec<(Entity<EditorState>, usize)>,
}

impl ExplainView {
    pub fn new(raw: Arc<ExplainRaw>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = Arc::new(PlanModel::from_raw(&raw));
        let mut view = Self {
            focus_handle: cx.focus_handle(),
            raw,
            model,
            selected: None,
            collapsed: HashSet::new(),
            show_raw: false,
            scroll: UniformListScrollHandle::default(),
            text_editor: None,
            section_editors: Vec::new(),
        };
        match &view.model.body {
            PlanBody::Json(_) | PlanBody::Text(_) => view.ensure_text_editor(window, cx),
            PlanBody::Sections(sections) => {
                view.section_editors = sections
                    .iter()
                    .map(|section| {
                        let text = section.text.clone();
                        let lines = text.lines().count().max(1);
                        (read_only_editor(&text, "text", window, cx), lines)
                    })
                    .collect();
            }
            _ => {}
        }
        // open on the hot spot so the eye lands on what costs most
        if let Some(tree) = view.model.tree() {
            view.selected = tree
                .hottest()
                .or(if tree.nodes.is_empty() { None } else { Some(0) });
        }
        view
    }

    fn ensure_text_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.text_editor.is_some() {
            return;
        }
        let language = if self.model.raw_is_json {
            "json"
        } else {
            "text"
        };
        self.text_editor = Some(read_only_editor(&self.model.raw, language, window, cx));
    }

    fn set_raw(&mut self, raw: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.show_raw = raw;
        if raw {
            self.ensure_text_editor(window, cx);
        }
        cx.notify();
    }

    fn copy_plan(&self, window: &mut Window, cx: &mut App) {
        crate::clipboard::write(self.model.raw.clone(), cx);
        let what = if self.model.raw_is_json {
            "plan JSON"
        } else {
            "plan"
        };
        window.push_notification(Notification::success(format!("Copied {what}")), cx);
    }

    // ---- summary --------------------------------------------------------

    fn render_summary(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, success, warning) = (
            theme.border,
            theme.muted_foreground,
            theme.success,
            theme.warning,
        );
        let summary = &self.model.summary;
        let has_tree = self.model.tree().is_some();

        let mut chips: Vec<AnyElement> = Vec::new();
        if let Some(cost) = summary.total_cost {
            chips.push(stat(
                "cost",
                "Cost",
                format_cost(cost),
                None,
                "Total planner cost, in the planner's arbitrary units",
                cx,
            ));
        }
        match (summary.actual_rows, summary.plan_rows) {
            (Some(actual), Some(estimated)) => chips.push(stat(
                "rows",
                "Rows",
                format!(
                    "{} (est. {})",
                    format_count(actual),
                    format_count(estimated)
                ),
                None,
                "Rows returned by the top node, and what the planner expected",
                cx,
            )),
            (None, Some(estimated)) => chips.push(stat(
                "rows",
                "Rows",
                format!("~{}", format_count(estimated)),
                None,
                "Rows the planner expects the top node to return",
                cx,
            )),
            _ => {}
        }
        if has_tree {
            chips.push(stat(
                "nodes",
                "Nodes",
                summary.node_count.to_string(),
                None,
                "Operations in the plan",
                cx,
            ));
            chips.push(if summary.observation_count > 0 {
                stat(
                    "observations",
                    "Observations",
                    summary.observation_count.to_string(),
                    (summary.warning_count > 0).then_some(warning),
                    "Things worth a look; listed under Plan observations",
                    cx,
                )
            } else {
                stat(
                    "observations",
                    "Observations",
                    "none".to_string(),
                    Some(success),
                    "Nothing stands out in this plan",
                    cx,
                )
            });
        }
        if has_tree && !self.raw.analyzed && matches!(self.raw.db_type, DatabaseType::PostgreSQL) {
            chips.push(stat(
                "estimates",
                "Estimates only",
                "no timings".to_string(),
                Some(muted),
                "Run “Explain analyze” to execute the statement and measure each node",
                cx,
            ));
        }
        if !has_tree {
            chips.push(stat(
                "engine",
                "Engine",
                self.raw.db_type.display_name().to_string(),
                None,
                "The engine that produced this plan",
                cx,
            ));
        }
        for timing in timing_stats(
            self.raw.analyzed,
            self.raw.elapsed.as_secs_f64() * 1000.0,
            summary,
        ) {
            chips.push(stat(
                timing.label,
                timing.label,
                timing.value,
                None,
                timing.tooltip,
                cx,
            ));
        }

        let copy_tooltip = if self.model.raw_is_json {
            "Copy the plan JSON, for explain.dalibo.com"
        } else {
            "Copy the plan text"
        };
        h_flex()
            .flex_none()
            .px_3()
            .py_2()
            .gap_2()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1p5()
                    .flex_wrap()
                    .children(chips),
            )
            .when(has_tree, |this| {
                this.child(
                    h_flex()
                        .flex_none()
                        .gap_0p5()
                        .child(
                            Button::new("plan-view-tree")
                                .ghost()
                                .xsmall()
                                .label("Plan")
                                .selected(!self.show_raw)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.set_raw(false, window, cx)
                                })),
                        )
                        .child(
                            Button::new("plan-view-raw")
                                .ghost()
                                .xsmall()
                                .label("Raw")
                                .selected(self.show_raw)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.set_raw(true, window, cx)
                                })),
                        ),
                )
            })
            .child(
                Button::new("plan-copy")
                    .outline()
                    .xsmall()
                    .icon(IconName::Copy)
                    .label("Copy plan")
                    .tooltip(copy_tooltip)
                    .on_click(cx.listener(|this, _, window, cx| this.copy_plan(window, cx))),
            )
    }

    // ---- bodies ---------------------------------------------------------

    fn render_text_body(&self) -> AnyElement {
        match &self.text_editor {
            Some(editor) => div()
                .size_full()
                .child(
                    Editor::new(editor)
                        .readonly(true)
                        .bordered(false)
                        .size_full(),
                )
                .into_any_element(),
            None => div().size_full().into_any_element(),
        }
    }

    fn render_sections(&self, cx: &mut Context<Self>) -> AnyElement {
        let PlanBody::Sections(sections) = &self.model.body else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let (border, muted, head, mono) = (
            theme.border,
            theme.muted_foreground,
            theme.table_head,
            theme.mono_font_family.clone(),
        );
        v_flex()
            .id("plan-sections")
            .size_full()
            .overflow_y_scrollbar()
            .children(sections.iter().zip(&self.section_editors).map(
                |(section, (editor, lines))| {
                    v_flex()
                        .child(
                            div()
                                .px_3()
                                .h(px(26.))
                                .flex()
                                .items_center()
                                .bg(head)
                                .border_b_1()
                                .border_color(border)
                                .font_family(mono.clone())
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(muted)
                                .child(section.title.clone()),
                        )
                        .child(
                            div()
                                .h(px((*lines).clamp(2, 28) as f32 * 20. + 16.))
                                .border_b_1()
                                .border_color(border)
                                .child(
                                    Editor::new(editor)
                                        .readonly(true)
                                        .bordered(false)
                                        .size_full(),
                                ),
                        )
                },
            ))
            .into_any_element()
    }

    fn render_table_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let PlanBody::Table { header, rows } = &self.model.body else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let (border, head, head_fg, mono) = (
            theme.border,
            theme.table_head,
            theme.table_head_foreground,
            theme.mono_font_family.clone(),
        );
        let cell =
            |text: String, width: f32| div().flex_none().w(px(width)).px_2().truncate().child(text);
        v_flex()
            .id("plan-table")
            .size_full()
            .overflow_y_scrollbar()
            .child(
                h_flex()
                    .h(px(26.))
                    .px_3()
                    .bg(head)
                    .text_color(head_fg)
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .border_b_1()
                    .border_color(border)
                    .children(header.iter().map(|name| cell(name.clone(), 160.))),
            )
            .children(rows.iter().enumerate().map(|(index, row)| {
                h_flex()
                    .id(SharedString::from(format!("plan-row-{index}")))
                    .h(px(26.))
                    .px_3()
                    .font_family(mono.clone())
                    .text_size(px(12.))
                    .border_b_1()
                    .border_color(border.opacity(0.5))
                    .children(row.iter().map(|value| cell(value.clone(), 160.)))
            }))
            .into_any_element()
    }
}

/// A read-only code block.
fn read_only_editor(
    text: &str,
    language: &'static str,
    window: &mut Window,
    cx: &mut Context<ExplainView>,
) -> Entity<EditorState> {
    let text = text.to_string();
    cx.new(|cx| {
        EditorState::new(window, cx)
            .language(language)
            .soft_wrap(false)
            .default_value(text)
    })
}

/// `label value`, as a compact tinted chip with an explanation on hover.
fn stat(
    id: &'static str,
    label: &'static str,
    value: String,
    color: Option<Hsla>,
    tooltip: &'static str,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let (muted, foreground) = (theme.muted_foreground, theme.foreground);
    // a tinted chip stays legible on its own tint in the light theme too
    let color_text = color.map(|color| crate::theme::tint_text(color, cx));
    let tint = color_text.unwrap_or(foreground);
    h_flex()
        .id(SharedString::from(format!("plan-stat-{id}")))
        .flex_none()
        .h(px(22.))
        .px_2()
        .gap_1()
        .rounded(px(4.))
        .bg(color
            .map(|c| c.opacity(0.12))
            .unwrap_or(theme.muted.opacity(0.6)))
        .text_size(px(11.))
        .child(div().text_color(color_text.unwrap_or(muted)).child(label))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tint)
                .child(value),
        )
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .into_any_element()
}

pub(super) fn severity_color(severity: Severity, cx: &App) -> Hsla {
    let theme = cx.theme();
    match severity {
        Severity::Good => theme.success,
        Severity::Neutral => theme.muted_foreground,
        Severity::Warning => theme.warning,
        Severity::Critical => theme.danger,
    }
}

pub(super) fn kind_icon(kind: dbcrust_gui_core::explain::NodeKind) -> IconName {
    use dbcrust_gui_core::explain::NodeKind;
    match kind {
        NodeKind::Scan => IconName::Table,
        NodeKind::IndexScan => IconName::Search,
        NodeKind::Join => IconName::Network,
        NodeKind::Sort => IconName::SortAscending,
        NodeKind::Aggregate => IconName::ChartPie,
        NodeKind::Subquery => IconName::GalleryVerticalEnd,
        NodeKind::Other => IconName::Square,
    }
}

impl Focusable for ExplainView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ExplainView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (background, foreground, muted) =
            (theme.background, theme.foreground, theme.muted_foreground);

        let body = if self.show_raw {
            self.render_text_body()
        } else {
            match &self.model.body {
                PlanBody::Tree(_) => self.render_tree_body(cx),
                PlanBody::Sections(_) => self.render_sections(cx),
                PlanBody::Table { .. } => self.render_table_body(cx),
                PlanBody::Json(_) | PlanBody::Text(_) => self.render_text_body(),
            }
        };

        v_flex()
            .id("explain-view")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .bg(background)
            .text_color(foreground)
            .child(self.render_summary(cx))
            .when_some(self.model.note.clone(), |this, note| {
                this.child(
                    h_flex()
                        .flex_none()
                        .px_3()
                        .py_1()
                        .gap_1p5()
                        .text_size(px(11.))
                        .text_color(muted)
                        .child(Icon::new(IconName::Info).size(px(12.)))
                        .child(note),
                )
            })
            .child(div().flex_1().min_h_0().child(body))
    }
}
