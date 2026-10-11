//! The inspector's tabs: columns, indexes, constraints, references, data
//! preview and DDL.

use super::model::{ColumnLine, Described};
use super::{CountState, DataState, Section, TableInspector};
use crate::app_state::AppState;
use crate::explorer::copy_text;
use crate::theme::{self, Surfaces};
use dbcrust_gui_core::schema::{self, ColumnOutline};
use dbcrust_gui_core::{ConnectionId, DatabaseType, TableEntry};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::Editor,
    scroll::ScrollableElement as _,
    spinner::Spinner,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::ops::Range;
use std::sync::Arc;

const POSITION_WIDTH: f32 = 36.;
const NAME_WIDTH: f32 = 220.;
const TYPE_WIDTH: f32 = 150.;
const NULL_WIDTH: f32 = 64.;
const KEYS_WIDTH: f32 = 110.;
const DEFAULT_WIDTH: f32 = 190.;
const ROW_HEIGHT: f32 = 28.;
const HEAD_HEIGHT: f32 = 26.;
/// Up to this many columns the tab is one scrolling page with the summary
/// cards below the list; longer lists are virtualized and fill the tab.
const COMPACT_COLUMNS: usize = 60;
/// Lines each summary card shows before pointing at its tab.
const CARD_LINES: usize = 4;

impl TableInspector {
    pub(super) fn render_section(
        &mut self,
        described: &Arc<Described>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.section {
            Section::Columns => self.render_columns(described, cx),
            Section::Indexes => self.render_indexes(described, cx),
            Section::Constraints => self.render_constraints(described, cx),
            Section::ReferencedBy => self.render_references(described, cx),
            Section::Data => self.render_data(cx),
            Section::Ddl => self.render_ddl(cx),
        }
    }

    // ---- columns --------------------------------------------------------

    fn render_columns(&mut self, described: &Arc<Described>, cx: &mut Context<Self>) -> AnyElement {
        let noun = match self.db_type {
            DatabaseType::MongoDB | DatabaseType::Elasticsearch => "fields",
            _ => "columns",
        };
        if described.columns.is_empty() {
            return empty_state(
                IconName::Inbox,
                &format!("No {noun} reported"),
                "The engine returned no description for this object.",
                cx,
            );
        }
        let count = described.columns.len();
        let head = header_row(
            &[
                ("#", Some(POSITION_WIDTH)),
                ("Name", Some(NAME_WIDTH)),
                ("Type", Some(TYPE_WIDTH)),
                ("Nullable", Some(NULL_WIDTH)),
                ("Keys", Some(KEYS_WIDTH)),
                ("Default", Some(DEFAULT_WIDTH)),
                ("Details", None),
            ],
            cx,
        );

        // A short list leaves most of the tab empty: show the table's other
        // facts (indexes, constraints, references) as cards below it.
        if count <= COMPACT_COLUMNS {
            let lines: Vec<AnyElement> = described
                .columns
                .iter()
                .map(|line| column_line(self, line, cx))
                .collect();
            let cards = self.render_overview_cards(described, cx);
            return v_flex()
                .size_full()
                .child(head)
                .child(
                    v_flex()
                        .id("inspector-columns-page")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scrollbar()
                        .children(lines)
                        .child(cards),
                )
                .into_any_element();
        }

        let described = described.clone();
        let list = uniform_list(
            "inspector-columns",
            count,
            cx.processor(move |this, range: Range<usize>, _, cx| {
                range
                    .map(|ix| column_line(this, &described.columns[ix], cx))
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.columns_scroll)
        .size_full();

        v_flex()
            .size_full()
            .child(head)
            .child(
                div()
                    .id("inspector-columns-body")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(list)
                    .vertical_scrollbar(&self.columns_scroll),
            )
            .into_any_element()
    }

    // ---- overview cards ---------------------------------------------------

    /// Indexes, constraints and references as compact cards.
    fn render_overview_cards(
        &mut self,
        described: &Arc<Described>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sections = Section::available(&self.db_type);
        let surfaces = theme::surfaces(cx);
        let mut cards: Vec<AnyElement> = Vec::new();
        if sections.contains(&Section::Indexes) {
            cards.push(self.indexes_card(described, &surfaces, cx));
        }
        if sections.contains(&Section::Constraints) {
            cards.push(self.constraints_card(described, &surfaces, cx));
        }
        if sections.contains(&Section::ReferencedBy)
            && (!described.references.is_empty() || schema::reports_references(&self.db_type))
        {
            cards.push(self.references_card(described, &surfaces, cx));
        }
        if sections.contains(&Section::Data) {
            cards.push(self.data_card(&surfaces, cx));
        }
        if cards.is_empty() {
            return div().into_any_element();
        }
        h_flex()
            .flex_none()
            .flex_wrap()
            .items_start()
            .gap_3()
            .p_4()
            .children(cards)
            .into_any_element()
    }

    /// The frame of a summary card: title, count, a link to the full tab.
    #[allow(clippy::too_many_arguments)]
    fn overview_card(
        &self,
        id: &'static str,
        icon: IconName,
        title: &'static str,
        count: usize,
        section: Section,
        body: Vec<AnyElement>,
        surfaces: &Surfaces,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // index lines (name + columns) need the room
        let min_width = if section == Section::Indexes {
            380.
        } else {
            260.
        };
        let muted = cx.theme().muted_foreground;
        let more = count.saturating_sub(CARD_LINES);
        v_flex()
            .id(id)
            .flex_1()
            .min_w(px(min_width))
            .max_w(px(560.))
            .p_3()
            .gap_2()
            .rounded(px(8.))
            .border_1()
            .border_color(surfaces.raised_border)
            .bg(surfaces.raised)
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(Icon::new(icon).size(px(14.)).text_color(muted))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.))
                            .text_color(muted)
                            .child(count.to_string()),
                    )
                    .when(count > 0, |this| {
                        this.child(
                            Button::new(SharedString::from(format!("{id}/all")))
                                .ghost()
                                .xsmall()
                                .label("View all")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.select_section(section, window, cx)
                                })),
                        )
                    }),
            )
            .children(body)
            .when(more > 0, |this| {
                this.child(
                    div()
                        .text_size(px(11.))
                        .text_color(muted)
                        .child(format!("+{more} more")),
                )
            })
            .into_any_element()
    }

    /// The preview entry point, and the row count once it was asked for.
    fn data_card(&mut self, surfaces: &Surfaces, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, mono) = (theme.muted_foreground, theme.mono_font_family.clone());
        let counted = match &self.count {
            CountState::Done { rows, .. } => Some(*rows),
            _ => None,
        };
        let closed = AppState::global(cx)
            .read(cx)
            .connection(self.connection)
            .is_none();
        v_flex()
            .id("inspector-card-data")
            .flex_none()
            .w(px(300.))
            .p_3()
            .gap_2()
            .rounded(px(8.))
            .border_1()
            .border_color(surfaces.raised_border)
            .bg(surfaces.raised)
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(Icon::new(IconName::Table).size(px(14.)).text_color(muted))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Data"),
                    ),
            )
            .child(match counted {
                Some(rows) => h_flex()
                    .h(px(22.))
                    .gap_1()
                    .items_baseline()
                    .child(
                        div()
                            .font_family(mono)
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(schema::group_digits(rows)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(muted)
                            .child(if rows == 1 { "row" } else { "rows" }),
                    )
                    .into_any_element(),
                None => card_note("Row count not requested", muted),
            })
            .child(
                h_flex().child(
                    Button::new("inspector-card-preview")
                        .outline()
                        .xsmall()
                        .icon(IconName::Play)
                        .label("Preview first rows")
                        .disabled(closed)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.select_section(Section::Data, window, cx)
                        })),
                ),
            )
            .into_any_element()
    }

    fn indexes_card(
        &mut self,
        described: &Arc<Described>,
        surfaces: &Surfaces,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (muted, warning, info, mono) = (
            theme.muted_foreground,
            theme.warning,
            theme.info,
            theme.mono_font_family.clone(),
        );
        let body: Vec<AnyElement> = if described.indexes.is_empty() {
            vec![card_note("No indexes", muted)]
        } else {
            described
                .indexes
                .iter()
                .take(CARD_LINES)
                .map(|index| {
                    let (kind, color) = if index.primary {
                        ("PK".to_string(), warning)
                    } else if index.unique {
                        ("UNIQUE".to_string(), info)
                    } else {
                        (index.method.to_uppercase(), muted)
                    };
                    h_flex()
                        .h(px(22.))
                        .gap_2()
                        .items_center()
                        .child(chip(kind, color, cx))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_family(mono.clone())
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(index.name.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(mono.clone())
                                .text_size(px(11.))
                                .text_color(muted)
                                .child(format!("({})", index.columns.join(", "))),
                        )
                        .into_any_element()
                })
                .collect()
        };
        self.overview_card(
            "inspector-card-indexes",
            IconName::ListTree,
            "Indexes",
            described.indexes.len(),
            Section::Indexes,
            body,
            surfaces,
            cx,
        )
    }

    fn constraints_card(
        &mut self,
        described: &Arc<Described>,
        surfaces: &Surfaces,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (muted, info, mono) = (
            theme.muted_foreground,
            theme.info,
            theme.mono_font_family.clone(),
        );
        let connection = self.connection;
        let db_type = self.db_type.clone();
        let checks = &described.details.check_constraints;
        let total = described.foreign_keys.len() + checks.len();

        let mut body: Vec<AnyElement> = Vec::new();
        for fk in described.foreign_keys.iter().take(CARD_LINES) {
            let id: SharedString = format!("card-fk:{}", fk.name).into();
            let (from, target, entry) = match &fk.parsed {
                Some(parsed) => (
                    parsed.columns.join(", "),
                    format!("{}({})", parsed.target, parsed.target_columns.join(", ")),
                    self.find_table(&db_type, &parsed.target, cx),
                ),
                None => (fk.name.clone(), fk.definition.clone(), None),
            };
            body.push(
                h_flex()
                    .h(px(22.))
                    .gap_1p5()
                    .items_center()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(mono.clone())
                            .text_size(px(12.))
                            .child(from),
                    )
                    .child(arrow(muted))
                    .child(fk_link(id, target, entry, connection, info, mono.clone()))
                    .into_any_element(),
            );
        }
        for check in checks.iter().take(CARD_LINES.saturating_sub(body.len())) {
            body.push(
                h_flex()
                    .h(px(22.))
                    .gap_2()
                    .items_center()
                    .child(chip("CHECK", muted, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(mono.clone())
                            .text_size(px(11.))
                            .text_color(muted)
                            .child(check.definition.trim().to_string()),
                    )
                    .into_any_element(),
            );
        }
        if body.is_empty() {
            body.push(card_note("No foreign keys or checks", muted));
        }
        self.overview_card(
            "inspector-card-constraints",
            IconName::Link,
            "Constraints",
            total,
            Section::Constraints,
            body,
            surfaces,
            cx,
        )
    }

    fn references_card(
        &mut self,
        described: &Arc<Described>,
        surfaces: &Surfaces,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (muted, info, mono) = (
            theme.muted_foreground,
            theme.info,
            theme.mono_font_family.clone(),
        );
        let connection = self.connection;
        let db_type = self.db_type.clone();
        let body: Vec<AnyElement> = if described.references.is_empty() {
            vec![card_note("No other table points here", muted)]
        } else {
            described
                .references
                .iter()
                .take(CARD_LINES)
                .map(|reference| {
                    let id: SharedString =
                        format!("card-ref:{}:{}", reference.table, reference.constraint).into();
                    let entry = self.find_table_in(&db_type, reference, cx);
                    let via = reference
                        .parsed
                        .as_ref()
                        .map(|parsed| format!("via {}", parsed.columns.join(", ")))
                        .unwrap_or_default();
                    h_flex()
                        .h(px(22.))
                        .gap_2()
                        .items_center()
                        .child(fk_link(
                            id,
                            reference.table.clone(),
                            entry,
                            connection,
                            info,
                            mono.clone(),
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(mono.clone())
                                .text_size(px(11.))
                                .text_color(muted)
                                .child(via),
                        )
                        .into_any_element()
                })
                .collect()
        };
        self.overview_card(
            "inspector-card-references",
            IconName::ArrowUpRight,
            "Referenced by",
            described.references.len(),
            Section::ReferencedBy,
            body,
            surfaces,
            cx,
        )
    }

    // ---- indexes --------------------------------------------------------

    fn render_indexes(&mut self, described: &Arc<Described>, cx: &mut Context<Self>) -> AnyElement {
        if described.indexes.is_empty() {
            return empty_state(
                IconName::Inbox,
                "No indexes",
                "Queries filtering or joining on this table read all of its rows.",
                cx,
            );
        }
        let theme = cx.theme();
        let (muted, border, warning, info, mono) = (
            theme.muted_foreground,
            theme.border,
            theme.warning,
            theme.info,
            theme.mono_font_family.clone(),
        );
        let rows = described.indexes.iter().map(|index| {
            let id: SharedString = format!("index:{}", index.name).into();
            let (kind, color) = if index.primary {
                ("PRIMARY KEY".to_string(), warning)
            } else if index.unique {
                ("UNIQUE".to_string(), info)
            } else {
                (index.method.to_uppercase(), muted)
            };
            let copy_text_value = index
                .statement
                .clone()
                .unwrap_or_else(|| format!("({})", index.columns.join(", ")));
            let tooltip = index.statement.clone();
            h_flex()
                .id(id.clone())
                .min_h(px(ROW_HEIGHT))
                .px_4()
                .py_1()
                .gap_0()
                .text_size(px(12.))
                .border_b_1()
                .border_color(border.opacity(0.5))
                .hover(|style| style.bg(muted.opacity(0.06)))
                .child(
                    body_cell(Some(NAME_WIDTH + 40.))
                        .font_family(mono.clone())
                        .font_weight(FontWeight::MEDIUM)
                        .child(index.name.clone()),
                )
                .child(body_cell(Some(TYPE_WIDTH)).child(h_flex().child(chip(kind, color, cx))))
                .child(
                    body_cell(Some(NAME_WIDTH + 40.))
                        .font_family(mono.clone())
                        .child(index.columns.join(", ")),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_2()
                        .truncate()
                        .font_family(mono.clone())
                        .text_size(px(12.))
                        .text_color(muted)
                        .child(match (&index.statement, &index.predicate) {
                            (Some(statement), _) => statement.clone(),
                            (None, Some(predicate)) => format!("WHERE {predicate}"),
                            (None, None) => String::new(),
                        })
                        .id(SharedString::from(format!("{id}/definition")))
                        .when_some(tooltip, |this, text| {
                            this.tooltip(move |window, cx| {
                                Tooltip::new(text.clone()).build(window, cx)
                            })
                        }),
                )
                .child(
                    Button::new(SharedString::from(format!("{id}/copy")))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Copy)
                        .tooltip("Copy definition")
                        .on_click(move |_, window, cx| {
                            copy_text(copy_text_value.clone(), "index definition", window, cx)
                        }),
                )
        });
        v_flex()
            .size_full()
            .child(header_row(
                &[
                    ("Name", Some(NAME_WIDTH + 40.)),
                    ("Type", Some(TYPE_WIDTH)),
                    ("Columns", Some(NAME_WIDTH + 40.)),
                    ("Definition", None),
                ],
                cx,
            ))
            .child(
                v_flex()
                    .id("inspector-indexes")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .children(rows),
            )
            .into_any_element()
    }

    // ---- constraints ----------------------------------------------------

    fn render_constraints(
        &mut self,
        described: &Arc<Described>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let details = &described.details;
        if described.foreign_keys.is_empty() && details.check_constraints.is_empty() {
            return empty_state(
                IconName::Inbox,
                "No foreign keys or check constraints",
                "Primary keys and unique indexes are listed under Indexes.",
                cx,
            );
        }
        let theme = cx.theme();
        let (muted, border, mono) = (
            theme.muted_foreground,
            theme.border,
            theme.mono_font_family.clone(),
        );
        let connection = self.connection;
        let db_type = self.db_type.clone();

        let foreign_keys = described.foreign_keys.iter().map(|fk| {
            let id: SharedString = format!("fk:{}", fk.name).into();
            let (mapping, target_entry) = match &fk.parsed {
                Some(parsed) => (
                    parsed.columns.join(", "),
                    self.find_table(&db_type, &parsed.target, cx),
                ),
                None => (String::new(), None),
            };
            let target_label = fk
                .parsed
                .as_ref()
                .map(|parsed| format!("{}({})", parsed.target, parsed.target_columns.join(", ")))
                .unwrap_or_else(|| fk.definition.clone());
            let actions = fk
                .parsed
                .as_ref()
                .map(|parsed| parsed.actions.clone())
                .unwrap_or_default();
            h_flex()
                .id(id.clone())
                .min_h(px(ROW_HEIGHT))
                .px_4()
                .py_1()
                .text_size(px(12.))
                .border_b_1()
                .border_color(border.opacity(0.5))
                .child(
                    body_cell(Some(NAME_WIDTH + 40.))
                        .font_family(mono.clone())
                        .font_weight(FontWeight::MEDIUM)
                        .child(fk.name.clone()),
                )
                .child(
                    body_cell(Some(TYPE_WIDTH + 40.))
                        .font_family(mono.clone())
                        .child(mapping),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .px_2()
                        .gap_2()
                        .child(arrow(muted))
                        .child(table_link(
                            SharedString::from(format!("{id}/target")),
                            target_label,
                            target_entry,
                            connection,
                            mono.clone(),
                            cx,
                        ))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(muted)
                                .truncate()
                                .child(actions),
                        ),
                )
        });

        let checks = details.check_constraints.iter().map(|check| {
            let id: SharedString = format!("check:{}", check.name).into();
            let expression = check.definition.trim().to_string();
            h_flex()
                .id(id.clone())
                .min_h(px(ROW_HEIGHT))
                .px_4()
                .py_1()
                .text_size(px(12.))
                .border_b_1()
                .border_color(border.opacity(0.5))
                .child(
                    body_cell(Some(NAME_WIDTH + 40.))
                        .font_family(mono.clone())
                        .font_weight(FontWeight::MEDIUM)
                        .child(check.name.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_2()
                        .font_family(mono.clone())
                        .text_size(px(12.))
                        .text_color(muted)
                        .truncate()
                        .child(expression.clone())
                        .id(SharedString::from(format!("{id}/expression")))
                        .tooltip(move |window, cx| {
                            Tooltip::new(expression.clone()).build(window, cx)
                        }),
                )
        });

        v_flex()
            .id("inspector-constraints")
            .size_full()
            .overflow_y_scrollbar()
            .when(!described.foreign_keys.is_empty(), |this| {
                this.child(group_title(
                    "Foreign keys",
                    described.foreign_keys.len(),
                    cx,
                ))
                .children(foreign_keys)
            })
            .when(!details.check_constraints.is_empty(), |this| {
                this.child(group_title(
                    "Check constraints",
                    details.check_constraints.len(),
                    cx,
                ))
                .children(checks)
            })
            .into_any_element()
    }

    // ---- referenced by --------------------------------------------------

    fn render_references(
        &mut self,
        described: &Arc<Described>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if described.references.is_empty() {
            let hint = if schema::reports_references(&self.db_type) {
                "No other table has a foreign key pointing here."
            } else {
                "DBCrust does not look up incoming foreign keys for this engine."
            };
            return empty_state(IconName::Inbox, "No references found", hint, cx);
        }
        let theme = cx.theme();
        let (muted, border, mono) = (
            theme.muted_foreground,
            theme.border,
            theme.mono_font_family.clone(),
        );
        let connection = self.connection;
        let db_type = self.db_type.clone();
        let this_name = self.table.name.clone();

        let rows = described.references.iter().map(|reference| {
            let id: SharedString =
                format!("ref:{}:{}", reference.table, reference.constraint).into();
            let entry = self.find_table_in(&db_type, reference, cx);
            let mapping = reference
                .parsed
                .as_ref()
                .map(|parsed| {
                    format!(
                        "{}.{} → {}.{}",
                        reference.name,
                        parsed.columns.join(", "),
                        this_name,
                        parsed.target_columns.join(", ")
                    )
                })
                .unwrap_or_else(|| reference.definition.clone());
            h_flex()
                .id(id.clone())
                .min_h(px(ROW_HEIGHT))
                .px_4()
                .py_1()
                .text_size(px(12.))
                .border_b_1()
                .border_color(border.opacity(0.5))
                .child(
                    h_flex()
                        .flex_none()
                        .w(px(NAME_WIDTH + 40.))
                        .px_2()
                        .child(table_link(
                            SharedString::from(format!("{id}/table")),
                            reference.table.clone(),
                            entry,
                            connection,
                            mono.clone(),
                            cx,
                        )),
                )
                .child(
                    body_cell(Some(TYPE_WIDTH + 40.))
                        .font_family(mono.clone())
                        .text_color(muted)
                        .child(reference.constraint.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_2()
                        .truncate()
                        .font_family(mono.clone())
                        .text_size(px(12.))
                        .child(mapping),
                )
        });

        v_flex()
            .size_full()
            .child(header_row(
                &[
                    ("Table", Some(NAME_WIDTH + 40.)),
                    ("Constraint", Some(TYPE_WIDTH + 40.)),
                    ("Columns", None),
                ],
                cx,
            ))
            .child(
                v_flex()
                    .id("inspector-references")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .children(rows),
            )
            .into_any_element()
    }

    // ---- data -----------------------------------------------------------

    fn render_data(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border, danger) = (theme.muted_foreground, theme.border, theme.danger);
        let status: SharedString = match &self.data {
            DataState::Idle => "".into(),
            DataState::Loading => "Loading…".into(),
            DataState::Loaded { rows } => if *rows >= schema::PREVIEW_ROWS {
                format!("First {rows} rows")
            } else {
                format!("All {rows} row{}", if *rows == 1 { "" } else { "s" })
            }
            .into(),
            DataState::Failed => "Preview failed".into(),
        };
        let table_name = self.table.name.clone();
        let sql = schema::select_star_sql(&self.db_type, &self.table, schema::PREVIEW_ROWS);
        let first_load = matches!(self.data, DataState::Loading) && !self.data_has_content;

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .h(px(36.))
                    .px_4()
                    .gap_2()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(if self.data == DataState::Failed {
                                danger
                            } else {
                                muted
                            })
                            .child(status),
                    )
                    .child(
                        Button::new("inspector-data-reload")
                            .ghost()
                            .small()
                            .icon(IconName::RefreshCw)
                            .label("Reload")
                            .loading(matches!(self.data, DataState::Loading))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.load_data(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("inspector-data-open")
                            .outline()
                            .small()
                            .icon(IconName::SquareTerminal)
                            .label("Open in query tab")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_query(table_name.clone(), sql.clone(), cx)
                            })),
                    ),
            )
            .child(if first_load {
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(Spinner::new().color(muted))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(muted)
                            .child(format!("Reading the first {} rows…", schema::PREVIEW_ROWS)),
                    )
                    .into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.data_view.clone())
                    .into_any_element()
            })
            .into_any_element()
    }

    // ---- ddl ------------------------------------------------------------

    fn render_ddl(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let Some(editor) = self.ddl_editor.clone() else {
            return empty_state(
                IconName::FileText,
                "No DDL for this object",
                "This engine does not describe its objects with a CREATE statement.",
                cx,
            );
        };
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .h(px(36.))
                    .px_4()
                    .gap_2()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(muted)
                            .child("Rebuilt from the catalog; storage options are not included."),
                    )
                    .child(
                        Button::new("inspector-ddl-copy")
                            .ghost()
                            .small()
                            .icon(IconName::Copy)
                            .label("Copy")
                            .on_click(cx.listener(|this, _, window, cx| this.copy_ddl(window, cx))),
                    ),
            )
            .child(
                div().flex_1().min_h_0().child(
                    Editor::new(&editor)
                        .readonly(true)
                        .bordered(false)
                        .size_full(),
                ),
            )
            .into_any_element()
    }

    // ---- helpers --------------------------------------------------------

    /// The table list entry a foreign key target names, if it exists.
    pub(super) fn find_table(
        &self,
        db_type: &DatabaseType,
        target: &str,
        cx: &App,
    ) -> Option<TableEntry> {
        let state = AppState::global(cx);
        let state = state.read(cx);
        let tables = state.connection(self.connection)?.schema.tables()?;
        schema::find_table(db_type, tables, target, self.table.schema.as_deref()).cloned()
    }

    fn find_table_in(
        &self,
        db_type: &DatabaseType,
        reference: &super::model::ReferenceLine,
        cx: &App,
    ) -> Option<TableEntry> {
        let state = AppState::global(cx);
        let state = state.read(cx);
        let tables = state.connection(self.connection)?.schema.tables()?;
        let qualified = if reference.schema.is_empty() {
            reference.name.clone()
        } else {
            format!("\"{}\".\"{}\"", reference.schema, reference.name)
        };
        schema::find_table(db_type, tables, &qualified, None).cloned()
    }
}

// ---- row and cell builders ----------------------------------------------

fn header_row(cells: &[(&'static str, Option<f32>)], cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    h_flex()
        .flex_none()
        .h(px(HEAD_HEIGHT))
        .px_4()
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.table_head)
        .text_color(theme.table_head_foreground)
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .children(cells.iter().map(|(label, width)| match width {
            Some(width) => div().flex_none().w(px(*width)).px_2().child(*label),
            None => div().flex_1().min_w_0().px_2().child(*label),
        }))
}

/// A fixed-width (or flexible) body cell that clips instead of wrapping.
fn body_cell(width: Option<f32>) -> Div {
    match width {
        Some(width) => div().flex_none().w(px(width)).px_2().truncate(),
        None => div().flex_1().min_w_0().px_2().truncate(),
    }
}

fn group_title(title: &'static str, count: usize, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .flex_none()
        .h(px(HEAD_HEIGHT))
        .px_4()
        .gap_2()
        .bg(theme.table_head)
        .border_b_1()
        .border_color(theme.border)
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.table_head_foreground)
        .child(title)
        .child(
            div()
                .font_weight(FontWeight::NORMAL)
                .text_color(theme.muted_foreground)
                .child(count.to_string()),
        )
}

fn arrow(color: Hsla) -> impl IntoElement {
    Icon::new(IconName::ArrowRight)
        .size(px(12.))
        .text_color(color)
}

/// A tinted label (PK, UNIQUE, enum value, …).
fn chip(text: impl Into<SharedString>, color: Hsla, cx: &App) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .px(px(5.))
        .h(px(16.))
        .rounded(px(3.))
        .bg(color.opacity(0.14))
        .text_color(theme::tint_text(color, cx))
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// A muted one-liner for a card with nothing to list.
fn card_note(text: &'static str, color: Hsla) -> AnyElement {
    div()
        .h(px(22.))
        .flex()
        .items_center()
        .text_size(px(12.))
        .text_color(color)
        .child(text)
        .into_any_element()
}

/// A reference to another table: when it resolves, a click opens (or
/// focuses) that table's inspector.
fn fk_link(
    id: SharedString,
    label: String,
    entry: Option<TableEntry>,
    connection: ConnectionId,
    color: Hsla,
    mono: SharedString,
) -> AnyElement {
    let text = div()
        .flex_none()
        .px(px(3.))
        .rounded(px(3.))
        .font_family(mono)
        .text_size(px(12.))
        .text_color(color)
        .child(label);
    match entry {
        Some(entry) => {
            let tooltip = format!("Open {}", entry.qualified_name());
            text.id(id)
                .cursor_pointer()
                .hover(move |style| style.bg(color.opacity(0.14)))
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                .on_click(move |_, _, cx| {
                    let entry = entry.clone();
                    AppState::global(cx)
                        .update(cx, |state, cx| state.open_table(connection, entry, cx));
                })
                .into_any_element()
        }
        None => text.into_any_element(),
    }
}

/// A table name that opens the table's inspector when it resolves.
fn table_link(
    id: SharedString,
    label: String,
    entry: Option<TableEntry>,
    connection: dbcrust_gui_core::ConnectionId,
    mono: SharedString,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    match entry {
        Some(entry) => Button::new(id)
            .link()
            .small()
            .label(label)
            .font_family(mono)
            .tooltip(format!("Open {}", entry.qualified_name()))
            .on_click(move |_, _, cx| {
                let entry = entry.clone();
                AppState::global(cx)
                    .update(cx, |state, cx| state.open_table(connection, entry, cx));
            })
            .into_any_element(),
        None => div()
            .min_w_0()
            .truncate()
            .font_family(mono)
            .text_size(px(12.))
            .text_color(theme.foreground)
            .child(label)
            .into_any_element(),
    }
}

fn empty_state(icon: IconName, title: &str, hint: &str, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_2()
        .p_6()
        .child(Icon::new(icon).size(px(24.)).text_color(muted))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.to_string()),
        )
        .child(
            div()
                .max_w(px(420.))
                .text_center()
                .text_size(px(12.))
                .text_color(muted)
                .child(hint.to_string()),
        )
        .into_any_element()
}

// ---- columns tab rows -----------------------------------------------------

fn column_line(this: &TableInspector, line: &ColumnLine, cx: &App) -> AnyElement {
    match line {
        ColumnLine::Column(column) => column_row(this, column, cx),
        ColumnLine::Nested { text } => {
            let theme = cx.theme();
            h_flex()
                .w_full()
                .h(px(24.))
                .pl(px(16. + POSITION_WIDTH + 8. + 16.))
                .pr_4()
                .font_family(theme.mono_font_family.clone())
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child(format!("└ {text}"))
                .into_any_element()
        }
    }
}

fn column_row(this: &TableInspector, column: &ColumnOutline, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (fg, muted, border, warning, info, success, mono) = (
        theme.foreground,
        theme.muted_foreground,
        theme.border,
        theme.warning,
        theme.info,
        theme.success,
        theme.mono_font_family.clone(),
    );
    let id: SharedString = format!("column:{}", column.name).into();

    let mut keys = h_flex().gap_1();
    if column.flags.primary_key {
        keys = keys.child(chip("PK", warning, cx));
    }
    if column.flags.foreign_key {
        let reference = column.references.clone();
        keys = keys.child(
            chip("FK", info, cx)
                .id(SharedString::from(format!("{id}/fk")))
                .when_some(reference, |this, reference| {
                    this.tooltip(move |window, cx| {
                        Tooltip::new(format!("References {reference}")).build(window, cx)
                    })
                }),
        );
    }
    if column.flags.unique && !column.flags.primary_key {
        keys = keys.child(chip("UQ", success, cx));
    }

    let mut details = h_flex().gap_1().overflow_hidden();
    if let Some(reference) = &column.references {
        // a foreign key leads to the table it points at
        let target = column
            .reference_table
            .as_deref()
            .and_then(|table| this.find_table(&this.db_type, table, cx));
        details = details.child(fk_link(
            SharedString::from(format!("{id}/reference")),
            format!("→ {reference}"),
            target,
            this.connection,
            info,
            mono.clone(),
        ));
    }
    let shown = 6;
    for value in column.enum_values.iter().take(shown) {
        details = details.child(pill(value.clone(), cx));
    }
    if column.enum_values.len() > shown {
        let rest = column.enum_values[shown..].join(", ");
        details = details.child(
            div()
                .id(SharedString::from(format!("{id}/more")))
                .flex_none()
                .text_size(px(11.))
                .text_color(muted)
                .child(format!("+{}", column.enum_values.len() - shown))
                .tooltip(move |window, cx| Tooltip::new(rest.clone()).build(window, cx)),
        );
    }
    for capability in &column.capabilities {
        details = details.child(pill(capability.clone(), cx));
    }
    if let Some(collation) = &column.collation {
        details = details.child(
            div()
                .flex_none()
                .text_size(px(11.))
                .text_color(muted)
                .child(format!("collation {collation}")),
        );
    }

    h_flex()
        .id(id)
        .w_full()
        .h(px(ROW_HEIGHT))
        .px_4()
        .border_b_1()
        .border_color(border.opacity(0.5))
        .text_size(px(12.))
        .hover(|style| style.bg(muted.opacity(0.06)))
        .child(
            body_cell(Some(POSITION_WIDTH))
                .font_family(mono.clone())
                .text_color(muted)
                .child(column.position.to_string()),
        )
        .child(
            body_cell(Some(NAME_WIDTH))
                .font_family(mono.clone())
                .font_weight(FontWeight::MEDIUM)
                .child(column.name.clone()),
        )
        .child(
            body_cell(Some(TYPE_WIDTH))
                .font_family(mono.clone())
                .text_color(muted)
                .child(column.data_type.clone()),
        )
        .child(
            body_cell(Some(NULL_WIDTH))
                .text_color(if column.nullable { muted } else { fg })
                .child(if column.nullable { "yes" } else { "no" }),
        )
        .child(body_cell(Some(KEYS_WIDTH)).child(keys))
        .child(match &column.default_value {
            Some(default) => body_cell(Some(DEFAULT_WIDTH))
                .font_family(mono.clone())
                .child(default.clone()),
            None => body_cell(Some(DEFAULT_WIDTH)).text_color(muted).child("—"),
        })
        .child(div().flex_1().min_w_0().px_2().child(details))
        .into_any_element()
}

/// A neutral monospace pill (enum value, capability).
fn pill(text: String, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex_none()
        .px(px(5.))
        .h(px(16.))
        .flex()
        .items_center()
        .rounded(px(3.))
        .bg(theme.muted)
        .text_color(theme.foreground)
        .font_family(theme.mono_font_family.clone())
        .text_size(px(11.))
        .child(text)
}
