//! The side panel, in two clearly separate parts: the **selected node**
//! (its numbers and properties) and the **plan observations** (what the
//! whole plan says is worth a look, one entry per node and issue; clicking
//! one selects its node in the tree).

use super::{ExplainView, kind_icon, severity_color};
use dbcrust_gui_core::explain::{
    PlanNodeModel, PlanObservation, PlanTree, Severity, format_cost, format_count, format_ms,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, h_flex, scroll::ScrollableElement as _, tooltip::Tooltip, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

impl ExplainView {
    pub(super) fn render_details(&self, cx: &mut Context<Self>) -> AnyElement {
        let model = self.model.clone();
        let Some(tree) = model.tree() else {
            return div().into_any_element();
        };
        let selected = self.selected.and_then(|index| tree.nodes.get(index));
        let border = cx.theme().border;
        v_flex()
            .id("plan-details")
            .size_full()
            .child(
                v_flex()
                    .id("plan-selected-node")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p_3()
                    .gap_3()
                    .child(section_label("Selected node", None, cx))
                    .child(match selected {
                        Some(node) => self.node_details(node, tree, cx),
                        None => overview(tree, cx),
                    }),
            )
            .child(
                v_flex()
                    .id("plan-observations")
                    .flex_none()
                    .max_h(relative(0.5))
                    .overflow_y_scrollbar()
                    .border_t_1()
                    .border_color(border)
                    .p_3()
                    .gap_2()
                    .child(self.observation_list(tree, cx)),
            )
            .into_any_element()
    }

    fn node_details(
        &self,
        node: &PlanNodeModel,
        tree: &PlanTree,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (muted, border, mono) = (
            theme.muted_foreground,
            theme.border,
            theme.mono_font_family.clone(),
        );
        let tint = severity_color(node.severity, cx);

        let mut metrics: Vec<(&'static str, String)> = Vec::new();
        if tree.has_costs || tree.has_actuals {
            metrics.push((
                "Share",
                if node.share > 0.0 {
                    format!("{:.1}%", node.share * 100.0)
                } else {
                    "0%".to_string()
                },
            ));
        }
        if let (Some(startup), Some(total)) = (node.startup_cost, node.total_cost) {
            metrics.push((
                "Cost",
                format!("{} → {}", format_cost(startup), format_cost(total)),
            ));
        } else if let Some(total) = node.total_cost {
            metrics.push(("Cost", format_cost(total)));
        }
        if let Some(exclusive) = node.exclusive_cost
            && node.total_cost.is_some()
        {
            metrics.push(("Own cost", format_cost(exclusive)));
        }
        match (node.plan_rows, node.actual_rows) {
            (Some(estimated), Some(actual)) => {
                metrics.push((
                    "Rows",
                    format!(
                        "{} (est. {})",
                        format_count(actual),
                        format_count(estimated)
                    ),
                ));
            }
            (Some(estimated), None) => {
                metrics.push(("Rows", format!("~{}", format_count(estimated))))
            }
            _ => {}
        }
        if let Some(time) = node.actual_time_ms {
            metrics.push(("Time / loop", format_ms(time)));
        }
        if let Some(exclusive) = node
            .exclusive_time_ms
            .filter(|_| node.actual_time_ms.is_some())
        {
            metrics.push(("Own time", format_ms(exclusive)));
        }
        if let Some(loops) = node.actual_loops {
            metrics.push(("Loops", format_count(loops)));
        }

        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Icon::new(kind_icon(node.kind))
                                    .size(px(16.))
                                    .text_color(tint),
                            )
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(node.title.clone()),
                            ),
                    )
                    .when(
                        node.relation.is_some() || node.index_name.is_some(),
                        |this| {
                            this.child(
                                div()
                                    .font_family(mono.clone())
                                    .text_size(px(12.))
                                    .text_color(muted)
                                    .child(match (&node.relation, &node.index_name) {
                                        (Some(relation), Some(index)) => {
                                            format!("{relation} using {index}")
                                        }
                                        (Some(relation), None) => relation.clone(),
                                        (None, Some(index)) => format!("index {index}"),
                                        (None, None) => String::new(),
                                    }),
                            )
                        },
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .children(metrics.into_iter().map(|(label, value)| {
                        v_flex()
                            .min_w(px(88.))
                            .px_2()
                            .py_1()
                            .rounded(px(4.))
                            .border_1()
                            .border_color(border)
                            .child(div().text_size(px(10.)).text_color(muted).child(label))
                            .child(
                                div()
                                    .font_family(mono.clone())
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(value),
                            )
                    })),
            )
            .children(node.properties.iter().map(|group| {
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .pb_1()
                            .border_b_1()
                            .border_color(border)
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted)
                            .child(group.title),
                    )
                    .children(group.items.iter().map(|item| {
                        v_flex()
                            .py_0p5()
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(muted)
                                    .child(item.key.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .when(item.code, |this| this.font_family(mono.clone()))
                                    .child(item.value.clone()),
                            )
                    }))
            }))
            .into_any_element()
    }

    /// The plan-wide findings, grouped under the node they are about.
    fn observation_list(&self, tree: &PlanTree, cx: &mut Context<Self>) -> AnyElement {
        let observations = tree.observations();
        let theme = cx.theme();
        let (muted, success) = (theme.muted_foreground, theme.success);
        let heuristic = self.model.summary.grade.clone();

        let header = h_flex()
            .items_center()
            .gap_2()
            .child(section_label(
                "Plan observations",
                Some(observations.len()),
                cx,
            ))
            .when_some(heuristic, |this, grade| {
                this.child(div().flex_1()).child(
                    div()
                        .id("plan-heuristic")
                        .flex_none()
                        .text_size(px(10.))
                        .text_color(muted)
                        .child(format!("Heuristic score {}", grade.letter))
                        .tooltip(|window, cx| {
                            Tooltip::new(
                                "A rough score from the operations in the plan. It is not a measurement.",
                            )
                            .build(window, cx)
                        }),
                )
            });

        if observations.is_empty() {
            return v_flex()
                .gap_2()
                .child(header)
                .child(
                    h_flex()
                        .gap_1p5()
                        .text_size(px(12.))
                        .text_color(success)
                        .child(Icon::new(IconName::CircleCheck).size(px(14.)))
                        .child("Nothing in this plan stands out."),
                )
                .into_any_element();
        }

        v_flex()
            .gap_1p5()
            .child(header)
            .children(
                observations
                    .chunk_by(|a, b| a.node == b.node)
                    .map(|group| self.observation_group(group, cx)),
            )
            .into_any_element()
    }

    /// All observations of one node, as one clickable card.
    fn observation_group(&self, group: &[PlanObservation], cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, hover, active, info, border, mono) = (
            theme.muted_foreground,
            theme.list_hover,
            theme.list_active,
            theme.info,
            theme.border,
            theme.mono_font_family.clone(),
        );
        let node = group[0].node;
        let selected = self.selected == Some(node);
        v_flex()
            .id(SharedString::from(format!("plan-observation-{node}")))
            .gap_1()
            .p_1p5()
            .rounded(px(5.))
            .border_1()
            .border_color(if selected {
                active
            } else {
                border.opacity(0.6)
            })
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .when(selected, move |this| this.bg(active))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_node(node, window, cx);
            }))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(mono)
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(group[0].label.clone()),
            )
            .children(group.iter().map(|observation| {
                let (icon, tint) = match observation.severity {
                    Severity::Neutral | Severity::Good => (IconName::Lightbulb, info),
                    severity => (IconName::TriangleAlert, severity_color(severity, cx)),
                };
                v_flex()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .items_start()
                            .gap_1p5()
                            .child(Icon::new(icon).size(px(12.)).text_color(tint).mt(px(2.)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .child(observation.issue.clone()),
                            ),
                    )
                    .children(observation.advice.iter().map(|advice| {
                        div()
                            .pl(px(18.))
                            .text_size(px(11.))
                            .text_color(muted)
                            .child(advice.clone())
                    }))
            }))
            .into_any_element()
    }
}

/// A panel section title: `Plan observations  3`.
fn section_label(title: &'static str, count: Option<usize>, cx: &App) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .gap_1p5()
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(muted)
        .child(title)
        .when_some(count, |this, count| {
            this.child(
                div()
                    .font_weight(FontWeight::NORMAL)
                    .child(count.to_string()),
            )
        })
}

/// What the panel says when nothing is selected.
fn overview(tree: &PlanTree, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    let hottest = tree.hottest().and_then(|index| tree.nodes.get(index));
    v_flex()
        .gap_1()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Select an operation"),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(muted)
                .child("Click a row to see its costs, row counts, conditions and buffers."),
        )
        .when_some(hottest, |this, node| {
            this.child(div().text_size(px(12.)).text_color(muted).child(format!(
                "Most of the work happens in {} ({:.0}%).",
                node.label(),
                node.share * 100.0
            )))
        })
        .into_any_element()
}
