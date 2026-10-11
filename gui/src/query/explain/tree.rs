//! The operation tree: connector lines, cost bars, row estimates.

use super::{ExplainView, kind_icon, severity_color};
use dbcrust_gui_core::explain::{PlanNodeModel, PlanTree, format_cost, format_count, format_ms};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, h_flex, scroll::ScrollableElement as _, tooltip::Tooltip, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::ops::Range;
use std::rc::Rc;

const ROW_HEIGHT: f32 = 26.;
const LEVEL_WIDTH: f32 = 16.;
const SHARE_BAR: f32 = 64.;
const SHARE_WIDTH: f32 = 110.;
const ROWS_WIDTH: f32 = 128.;
const TIME_WIDTH: f32 = 76.;

impl ExplainView {
    /// Node indices that are not hidden under a folded ancestor.
    fn visible_nodes(&self, tree: &PlanTree) -> Vec<usize> {
        let mut visible = Vec::with_capacity(tree.nodes.len());
        let mut hidden_below: Option<usize> = None;
        for node in &tree.nodes {
            if let Some(depth) = hidden_below {
                if node.depth > depth {
                    continue;
                }
                hidden_below = None;
            }
            visible.push(node.index);
            if self.collapsed.contains(&node.index) && !node.children.is_empty() {
                hidden_below = Some(node.depth);
            }
        }
        visible
    }

    /// Select `index`, unfolding the nodes above it and scrolling it into
    /// view (an observation can point at a node hidden in a folded subtree).
    pub(super) fn select_node(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.model.clone();
        let Some(tree) = model.tree() else { return };
        if index >= tree.nodes.len() {
            return;
        }
        let mut ancestor = tree.nodes[index].parent;
        while let Some(parent) = ancestor {
            self.collapsed.remove(&parent);
            ancestor = tree.nodes[parent].parent;
        }
        self.selected = Some(index);
        if let Some(position) = self
            .visible_nodes(tree)
            .iter()
            .position(|&visible| visible == index)
        {
            self.scroll
                .scroll_to_item(position, ScrollStrategy::Nearest);
        }
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    pub(super) fn render_tree_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let model = self.model.clone();
        let Some(tree) = model.tree() else {
            return div().into_any_element();
        };
        let visible: Rc<Vec<usize>> = Rc::new(self.visible_nodes(tree));
        let theme = cx.theme();
        let (border, head, head_fg) = (theme.border, theme.table_head, theme.table_head_foreground);
        let show_share = tree.has_costs || tree.has_actuals;
        let time_label = if tree.has_actuals { "Time" } else { "Cost" };

        let list = uniform_list(
            "plan-nodes",
            visible.len(),
            cx.processor({
                let visible = visible.clone();
                move |this, range: Range<usize>, _, cx| {
                    range
                        .filter_map(|ix| visible.get(ix).copied())
                        .map(|index| this.render_node(index, cx))
                        .collect::<Vec<_>>()
                }
            }),
        )
        .track_scroll(&self.scroll)
        .size_full();

        h_flex()
            .size_full()
            .items_stretch()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .flex_none()
                            .h(px(24.))
                            .px_2()
                            .bg(head)
                            .text_color(head_fg)
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .border_b_1()
                            .border_color(border)
                            .child(div().flex_1().min_w_0().pl(px(8.)).child("Operation"))
                            .when(show_share, |this| {
                                this.child(div().flex_none().w(px(SHARE_WIDTH)).child(
                                    if tree.has_actuals {
                                        "Share of time"
                                    } else {
                                        "Share of cost"
                                    },
                                ))
                                .child(div().flex_none().w(px(ROWS_WIDTH)).text_right().child(
                                    if tree.has_actuals {
                                        "Rows (est.)"
                                    } else {
                                        "Rows"
                                    },
                                ))
                                .child(
                                    div()
                                        .flex_none()
                                        .w(px(TIME_WIDTH))
                                        .text_right()
                                        .child(time_label),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("plan-tree-body")
                            .flex_1()
                            .min_h_0()
                            .relative()
                            .child(list)
                            .vertical_scrollbar(&self.scroll),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(340.))
                    .h_full()
                    .border_l_1()
                    .border_color(border)
                    .child(self.render_details(cx)),
            )
            .into_any_element()
    }

    fn render_node(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let model = self.model.clone();
        let Some(tree) = model.tree() else {
            return div().into_any_element();
        };
        let Some(node) = tree.nodes.get(index) else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let (muted, hover, active, line, primary, warning, danger, track, mono) = (
            theme.muted_foreground,
            theme.list_hover,
            theme.list_active,
            theme.muted_foreground.opacity(0.35),
            theme.primary,
            theme.warning,
            theme.danger,
            theme.muted,
            theme.mono_font_family.clone(),
        );
        let selected = self.selected == Some(index);
        let collapsed = self.collapsed.contains(&index);
        let show_share = tree.has_costs || tree.has_actuals;
        let tint = severity_color(node.severity, cx);
        let has_children = !node.children.is_empty();

        // connector columns: vertical guides of ancestors, then this node's elbow
        let connectors = (0..node.depth).map(|column| {
            let is_elbow = column + 1 == node.depth;
            let guide = !is_elbow && node.guides.get(column).copied().unwrap_or(false);
            let cell = div().relative().flex_none().w(px(LEVEL_WIDTH)).h_full();
            if is_elbow {
                cell.child(
                    div()
                        .absolute()
                        .left(px(7.))
                        .top_0()
                        .w(px(1.))
                        .h(if node.is_last {
                            px(ROW_HEIGHT / 2.)
                        } else {
                            px(ROW_HEIGHT)
                        })
                        .bg(line),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(7.))
                        .top(px(ROW_HEIGHT / 2.))
                        .w(px(LEVEL_WIDTH - 7.))
                        .h(px(1.))
                        .bg(line),
                )
            } else if guide {
                cell.child(
                    div()
                        .absolute()
                        .left(px(7.))
                        .top_0()
                        .bottom_0()
                        .w(px(1.))
                        .bg(line),
                )
            } else {
                cell
            }
        });

        let chevron = if has_children {
            div()
                .id(SharedString::from(format!("plan-node-{index}/fold")))
                .flex_none()
                .size(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(if collapsed {
                        IconName::ChevronRight
                    } else {
                        IconName::ChevronDown
                    })
                    .size(px(12.))
                    .text_color(muted),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.collapsed.remove(&index) {
                        this.collapsed.insert(index);
                    }
                    cx.notify();
                    cx.stop_propagation();
                }))
                .into_any_element()
        } else {
            div().flex_none().size(px(14.)).into_any_element()
        };

        let operation = h_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .gap_1p5()
            .pr_3()
            .overflow_hidden()
            .child(connectors.fold(h_flex().flex_none().h_full(), |row, cell| row.child(cell)))
            .child(chevron)
            .child(
                Icon::new(kind_icon(node.kind))
                    .size(px(14.))
                    .text_color(tint),
            )
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .child(node.title.clone()),
            )
            .when_some(node.relation.clone(), |this, relation| {
                this.child(
                    div()
                        .flex_none()
                        .font_family(mono.clone())
                        .text_size(px(11.))
                        .child(relation),
                )
            })
            .when_some(node.index_name.clone(), |this, name| {
                this.child(
                    div()
                        .flex_none()
                        .font_family(mono.clone())
                        .text_size(px(11.))
                        .text_color(muted)
                        .child(format!("using {name}")),
                )
            })
            .when_some(node.detail.clone(), |this, detail| {
                this.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(mono.clone())
                        .text_size(px(11.))
                        .text_color(muted)
                        .child(detail),
                )
            });

        let share_color = if node.share >= 0.5 {
            danger
        } else if node.share >= 0.25 {
            warning
        } else {
            primary
        };

        h_flex()
            .id(SharedString::from(format!("plan-node-{index}")))
            .w_full()
            .h(px(ROW_HEIGHT))
            .pl_2()
            .pr_2()
            .text_size(px(12.))
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .when(selected, move |this| this.bg(active))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_node(index, window, cx);
            }))
            .child(operation)
            .when(show_share, |this| {
                this.child(
                    h_flex()
                        .flex_none()
                        .w(px(SHARE_WIDTH))
                        .gap_1p5()
                        .child(
                            div()
                                .flex_none()
                                .w(px(SHARE_BAR))
                                .h(px(6.))
                                .rounded(px(3.))
                                .bg(track)
                                .child(div().h_full().rounded(px(3.)).bg(share_color).w(px(
                                    if node.share > 0.0 {
                                        (node.share as f32 * SHARE_BAR).max(2.)
                                    } else {
                                        0.
                                    },
                                ))),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_family(mono.clone())
                                .text_size(px(11.))
                                .text_color(muted)
                                .child(share_label(node.share)),
                        ),
                )
                .child(self.rows_cell(node, index, mono.clone(), cx))
                .child(
                    div()
                        .flex_none()
                        .w(px(TIME_WIDTH))
                        .text_right()
                        .font_family(mono.clone())
                        .text_size(px(11.))
                        .child(time_or_cost(node, tree)),
                )
            })
            .into_any_element()
    }

    /// `5,000 (est. 100)` with a warning mark when the estimate is far off.
    fn rows_cell(
        &self,
        node: &PlanNodeModel,
        index: usize,
        mono: SharedString,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let (muted, warning) = (theme.muted_foreground, theme.warning);
        let mut cell = h_flex()
            .id(SharedString::from(format!("plan-node-{index}/rows")))
            .flex_none()
            .w(px(ROWS_WIDTH))
            .justify_end()
            .gap_1()
            .font_family(mono)
            .text_size(px(11.));
        match (node.actual_rows, node.plan_rows) {
            (Some(actual), estimated) => {
                if let Some(factor) = node.estimate_factor {
                    cell = cell.child(
                        Icon::new(IconName::TriangleAlert)
                            .size(px(11.))
                            .text_color(warning),
                    );
                    let message = format!("Estimate off by {factor:.0}x");
                    cell = cell
                        .tooltip(move |window, cx| Tooltip::new(message.clone()).build(window, cx));
                }
                let loops = node
                    .actual_loops
                    .filter(|loops| *loops > 1)
                    .map(|loops| format!(" ×{}", format_count(loops)))
                    .unwrap_or_default();
                cell = cell.child(div().child(format!("{}{loops}", format_count(actual))));
                if let Some(estimated) = estimated {
                    cell = cell.child(
                        div()
                            .text_color(if node.estimate_factor.is_some() {
                                warning
                            } else {
                                muted
                            })
                            .child(format!("({})", format_count(estimated))),
                    );
                }
            }
            (None, Some(estimated)) => {
                cell = cell.child(
                    div()
                        .text_color(muted)
                        .child(format!("~{}", format_count(estimated))),
                );
            }
            (None, None) => {}
        }
        cell.into_any_element()
    }

    pub(super) fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.show_raw || event.keystroke.modifiers.modified() {
            return;
        }
        let model = self.model.clone();
        let Some(tree) = model.tree() else { return };
        let visible = self.visible_nodes(tree);
        let position = self
            .selected
            .and_then(|selected| visible.iter().position(|&i| i == selected));
        let go = |this: &mut Self, target: Option<usize>, cx: &mut Context<Self>| {
            if let Some(target) = target {
                this.selected = Some(target);
                if let Some(position) = visible.iter().position(|&i| i == target) {
                    this.scroll
                        .scroll_to_item(position, ScrollStrategy::Nearest);
                }
                cx.notify();
            }
        };
        match event.keystroke.key.as_str() {
            "down" => {
                let next = match position {
                    Some(p) => visible.get(p + 1).copied(),
                    None => visible.first().copied(),
                };
                go(self, next, cx);
            }
            "up" => {
                let previous = match position {
                    Some(p) if p > 0 => visible.get(p - 1).copied(),
                    Some(_) => None,
                    None => visible.last().copied(),
                };
                go(self, previous, cx);
            }
            "home" => go(self, visible.first().copied(), cx),
            "end" => go(self, visible.last().copied(), cx),
            "left" => {
                if let Some(node) = self.selected.and_then(|i| tree.nodes.get(i)) {
                    if !node.children.is_empty() && !self.collapsed.contains(&node.index) {
                        self.collapsed.insert(node.index);
                        cx.notify();
                    } else {
                        go(self, node.parent, cx);
                    }
                }
            }
            "right" => {
                if let Some(node) = self.selected.and_then(|i| tree.nodes.get(i)) {
                    if self.collapsed.remove(&node.index) {
                        cx.notify();
                    } else {
                        go(self, node.children.first().copied(), cx);
                    }
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }
}

fn share_label(share: f64) -> String {
    if share <= 0.0 {
        "0%".to_string()
    } else if share < 0.01 {
        "<1%".to_string()
    } else {
        format!("{:.0}%", share * 100.0)
    }
}

fn time_or_cost(node: &PlanNodeModel, tree: &PlanTree) -> String {
    if tree.has_actuals {
        node.exclusive_time_ms.map(format_ms).unwrap_or_default()
    } else {
        node.total_cost.map(format_cost).unwrap_or_default()
    }
}
