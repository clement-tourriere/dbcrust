//! UI rendering for the schema TUI viewer

use super::app::{PanelFocus, RelationshipEntry, SchemaTuiApp};
use super::schema_data::TableListEntry;
use crate::database::DatabaseTypeExt;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, Padding, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
};

/// Render the entire schema TUI
pub fn render(frame: &mut Frame, app: &mut SchemaTuiApp) {
    let area = frame.area();

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(10),   // Content
            Constraint::Length(2), // Footer
        ])
        .split(area);

    render_header(frame, app, main_chunks[0]);
    render_content(frame, app, main_chunks[1]);
    render_footer(frame, app, main_chunks[2]);

    if app.show_help {
        render_help_overlay(frame, area);
    }
}

/// Render the header bar
fn render_header(frame: &mut Frame, app: &SchemaTuiApp, area: Rect) {
    let title = Line::from(vec![
        Span::styled(
            " Schema Viewer ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("| ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            &app.schema_data.database_name,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" ({})", app.schema_data.database_type.display_name()),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{} tables", app.table_count()),
            Style::default().fg(Color::White),
        ),
        Span::styled(", ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{} schemas", app.schema_count()),
            Style::default().fg(Color::White),
        ),
        Span::styled(", ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{} FK relationships", app.schema_data.relationships.len()),
            Style::default().fg(Color::White),
        ),
    ]);

    let header = Paragraph::new(title).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );

    frame.render_widget(header, area);
}

/// Render the three-panel content area
fn render_content(frame: &mut Frame, app: &mut SchemaTuiApp, area: Rect) {
    let content_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30), // Table list
            Constraint::Percentage(45), // Details
            Constraint::Percentage(25), // Relationships
        ])
        .split(area);

    render_table_list(frame, app, content_chunks[0]);
    render_details(frame, app, content_chunks[1]);
    render_relationships(frame, app, content_chunks[2]);
}

/// Render the left panel: table list with search
fn render_table_list(frame: &mut Frame, app: &SchemaTuiApp, area: Rect) {
    let is_focused = app.focus == PanelFocus::TableList;

    // Build title with search indicator
    let title = if app.search_active {
        format!(" / {} ", app.search_query)
    } else if !app.search_query.is_empty() {
        format!(" Tables [/: {}] ", app.search_query)
    } else {
        " Tables ".to_string()
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if is_focused {
            Color::Cyan
        } else {
            Color::DarkGray
        }))
        .padding(Padding::horizontal(1));

    let mut items: Vec<ListItem> = Vec::new();
    let mut selected_index = 0;

    for (i, entry) in app.table_list.iter().enumerate() {
        let is_selected = i == app.selected_table_idx;

        match entry {
            TableListEntry::SchemaHeader { name } => {
                items.push(ListItem::new(Line::from(Span::styled(
                    format!(" {name}"),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ))));
            }
            TableListEntry::Table {
                name,
                outgoing_fk,
                incoming_fk,
                ..
            } => {
                let total_fk = outgoing_fk + incoming_fk;
                let fk_indicator = if total_fk > 0 {
                    format!("  ({total_fk} FK)")
                } else {
                    String::new()
                };

                let mut spans = vec![Span::styled(
                    format!("  {name}"),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(if is_selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                )];

                if !fk_indicator.is_empty() {
                    spans.push(Span::styled(
                        fk_indicator,
                        Style::default().fg(Color::DarkGray),
                    ));
                }

                items.push(ListItem::new(Line::from(spans)));

                if is_selected {
                    selected_index = items.len() - 1;
                }
            }
        }
    }

    let items_len = items.len();

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    let mut list_state = ratatui::widgets::ListState::default();
    list_state.select(Some(selected_index));

    frame.render_stateful_widget(list, area, &mut list_state);

    // Scrollbar
    // saturating_sub: a pane squeezed below 2 rows underflowed here
    if items_len > (area.height as usize).saturating_sub(2) {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("^"))
            .end_symbol(Some("v"));
        let mut scrollbar_state = ScrollbarState::new(items_len).position(selected_index);
        frame.render_stateful_widget(
            scrollbar,
            area.inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

/// Render the center panel: table details
fn render_details(frame: &mut Frame, app: &mut SchemaTuiApp, area: Rect) {
    let is_focused = app.focus == PanelFocus::Details;

    let (schema, table_name) = match app.selected_table_key() {
        Some(k) => k,
        None => {
            let empty = Paragraph::new("Select a table to view details")
                .style(Style::default().fg(Color::DarkGray))
                .block(
                    Block::default()
                        .title(" Details ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::DarkGray)),
                );
            frame.render_widget(empty, area);
            return;
        }
    };

    // Trigger lazy-load into cache
    let has_details = app.get_or_load_details(&schema, &table_name).is_some();

    if !has_details {
        let loading = Paragraph::new("Loading table details...")
            .style(Style::default().fg(Color::DarkGray))
            .block(
                Block::default()
                    .title(format!(" {} ", table_name))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(if is_focused {
                        Color::Cyan
                    } else {
                        Color::DarkGray
                    })),
            );
        frame.render_widget(loading, area);
        return;
    }

    // Now borrow immutably from cache (we know the key exists)
    let cache_key = (schema.clone(), table_name.clone());
    let details = &app.details_cache[&cache_key];

    let center_scroll = app.center_scroll;
    let show_indexes = app.show_indexes;
    let show_constraints = app.show_constraints;

    // Inline the detail rendering (details is borrowed from cache)
    {
        let mut lines: Vec<Line> = Vec::new();

        // Table name header
        lines.push(Line::from(vec![
            Span::styled(
                table_name,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" ({schema})"), Style::default().fg(Color::DarkGray)),
        ]));
        lines.push(Line::from(""));

        // Determine which columns are PKs and FKs
        let pk_columns: Vec<String> = details
            .indexes
            .iter()
            .filter(|idx| idx.is_primary)
            .flat_map(|idx| extract_columns_from_definition(&idx.definition))
            .collect();

        let fk_columns: Vec<String> = details
            .foreign_keys
            .iter()
            .flat_map(|fk| extract_fk_source_columns(&fk.definition))
            .collect();

        // Columns section
        lines.push(Line::from(Span::styled(
            "Columns:",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::UNDERLINED),
        )));

        for col in &details.columns {
            let is_pk = pk_columns.iter().any(|pk| pk == &col.name);
            let is_fk = fk_columns.iter().any(|fk| fk == &col.name);

            let name_style = if is_pk {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if is_fk {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::White)
            };

            let mut spans = vec![
                Span::styled(format!("  {:<20}", col.name), name_style),
                Span::styled(
                    format!("{:<15}", col.data_type),
                    Style::default().fg(Color::Yellow),
                ),
            ];

            // Indicators
            if is_pk {
                spans.push(Span::styled(
                    "PK ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            if is_fk {
                spans.push(Span::styled("FK ", Style::default().fg(Color::Green)));
            }
            if !col.nullable {
                spans.push(Span::styled("NN ", Style::default().fg(Color::White)));
            }

            // Check for unique indexes on this column
            let is_unique = details.indexes.iter().any(|idx| {
                idx.is_unique
                    && !idx.is_primary
                    && extract_columns_from_definition(&idx.definition)
                        .iter()
                        .any(|c| c == &col.name)
            });
            if is_unique {
                spans.push(Span::styled("U ", Style::default().fg(Color::Magenta)));
            }

            if let Some(ref default) = col.default_value {
                spans.push(Span::styled(
                    format!("D={default}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            lines.push(Line::from(spans));
        }

        // Indexes section (toggleable)
        if show_indexes && !details.indexes.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Indexes:",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::UNDERLINED),
            )));

            for idx in &details.indexes {
                let type_indicator = if idx.is_primary {
                    "PRIMARY"
                } else if idx.is_unique {
                    "UNIQUE"
                } else {
                    &idx.index_type
                };

                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {:<24}", idx.name),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(
                        format!("{type_indicator:<10}"),
                        Style::default().fg(if idx.is_primary {
                            Color::Cyan
                        } else if idx.is_unique {
                            Color::Magenta
                        } else {
                            Color::Yellow
                        }),
                    ),
                ]));

                // Show definition on next line if it's long
                if !idx.definition.is_empty() {
                    let def = &idx.definition;
                    if def.len() > 50 {
                        for wrapped in textwrap::wrap(def, (area.width as usize).saturating_sub(8))
                        {
                            lines.push(Line::from(Span::styled(
                                format!("    {wrapped}"),
                                Style::default().fg(Color::DarkGray),
                            )));
                        }
                    } else {
                        lines.push(Line::from(Span::styled(
                            format!("    {def}"),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }

                if let Some(ref pred) = idx.predicate {
                    lines.push(Line::from(Span::styled(
                        format!("    WHERE {pred}"),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
        }

        // Check constraints section (toggleable)
        if show_constraints && !details.check_constraints.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Check Constraints:",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::UNDERLINED),
            )));

            for chk in &details.check_constraints {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {}: ", chk.name),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(&chk.definition, Style::default().fg(Color::DarkGray)),
                ]));
            }
        }

        // Foreign keys section (always show, important for navigation context)
        if !details.foreign_keys.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Foreign Keys:",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::UNDERLINED),
            )));

            for fk in &details.foreign_keys {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {}: ", fk.name),
                        Style::default().fg(Color::Green),
                    ),
                    Span::styled(&fk.definition, Style::default().fg(Color::DarkGray)),
                ]));
            }
        }

        // Referenced by section
        if !details.referenced_by.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Referenced By:",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::UNDERLINED),
            )));

            for ref_by in &details.referenced_by {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {}.{}: ", ref_by.schema, ref_by.table),
                        Style::default().fg(Color::LightBlue),
                    ),
                    Span::styled(&ref_by.definition, Style::default().fg(Color::DarkGray)),
                ]));
            }
        }

        let paragraph = Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Details ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(if is_focused {
                        Color::Cyan
                    } else {
                        Color::DarkGray
                    }))
                    .padding(Padding::horizontal(1)),
            )
            .wrap(Wrap { trim: true })
            .scroll((center_scroll, 0));

        frame.render_widget(paragraph, area);
    }
}

/// Render the right panel: FK relationships
fn render_relationships(frame: &mut Frame, app: &SchemaTuiApp, area: Rect) {
    let is_focused = app.focus == PanelFocus::Relationships;

    let entries = app.all_relationships_flat();

    if entries.is_empty() {
        let msg = if app.schema_data.relationships.is_empty() {
            "No FK relationships\nfound in this database"
        } else {
            "No relationships\nfor this table"
        };

        let empty = Paragraph::new(msg)
            .style(Style::default().fg(Color::DarkGray))
            .block(
                Block::default()
                    .title(" Relationships ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(if is_focused {
                        Color::Cyan
                    } else {
                        Color::DarkGray
                    })),
            );
        frame.render_widget(empty, area);
        return;
    }

    let mut items: Vec<ListItem> = Vec::new();
    let mut selected_visual_idx = 0;

    for (i, entry) in entries.iter().enumerate() {
        match entry {
            RelationshipEntry::Header(title) => {
                items.push(ListItem::new(Line::from(Span::styled(
                    title.as_str(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                ))));
            }
            RelationshipEntry::Separator => {
                items.push(ListItem::new(Line::from("")));
            }
            RelationshipEntry::Outgoing(r) => {
                let cols = format_fk_columns(&r.source_columns, &r.target_columns);
                items.push(ListItem::new(Line::from(vec![
                    Span::styled("-> ", Style::default().fg(Color::Green)),
                    Span::styled(
                        format!("{}.{}", r.target_schema, r.target_table),
                        Style::default().fg(Color::Green),
                    ),
                    Span::styled(format!(" ({cols})"), Style::default().fg(Color::DarkGray)),
                ])));
                if i == app.selected_relationship_idx {
                    selected_visual_idx = items.len() - 1;
                }
            }
            RelationshipEntry::Incoming(r) => {
                let cols = format_fk_columns(&r.source_columns, &r.target_columns);
                items.push(ListItem::new(Line::from(vec![
                    Span::styled("<- ", Style::default().fg(Color::LightBlue)),
                    Span::styled(
                        format!("{}.{}", r.source_schema, r.source_table),
                        Style::default().fg(Color::LightBlue),
                    ),
                    Span::styled(format!(" ({cols})"), Style::default().fg(Color::DarkGray)),
                ])));
                if i == app.selected_relationship_idx {
                    selected_visual_idx = items.len() - 1;
                }
            }
        }
    }

    let items_len = items.len();

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Relationships ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if is_focused {
                    Color::Cyan
                } else {
                    Color::DarkGray
                }))
                .padding(Padding::horizontal(1)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    let mut list_state = ratatui::widgets::ListState::default();
    if is_focused {
        list_state.select(Some(selected_visual_idx));
    }

    frame.render_stateful_widget(list, area, &mut list_state);

    // saturating_sub: a pane squeezed below 2 rows underflowed here
    if items_len > (area.height as usize).saturating_sub(2) {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("^"))
            .end_symbol(Some("v"));
        let mut scrollbar_state = ScrollbarState::new(items_len).position(selected_visual_idx);
        frame.render_stateful_widget(
            scrollbar,
            area.inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

/// Render the footer with keybindings
fn render_footer(frame: &mut Frame, app: &SchemaTuiApp, area: Rect) {
    let help_text = if app.search_active {
        Line::from(vec![
            Span::styled(" Type to filter", Style::default().fg(Color::Yellow)),
            Span::styled("  [", Style::default().fg(Color::DarkGray)),
            Span::styled("Enter", Style::default().fg(Color::Cyan)),
            Span::styled("] Accept  [", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::Cyan)),
            Span::styled("] Cancel ", Style::default().fg(Color::DarkGray)),
        ])
    } else {
        Line::from(vec![
            Span::styled(" [", Style::default().fg(Color::DarkGray)),
            Span::styled("j/k", Style::default().fg(Color::Cyan)),
            Span::styled("] Nav  [", Style::default().fg(Color::DarkGray)),
            Span::styled("Tab", Style::default().fg(Color::Cyan)),
            Span::styled("] Panel  [", Style::default().fg(Color::DarkGray)),
            Span::styled("/", Style::default().fg(Color::Cyan)),
            Span::styled("] Search  [", Style::default().fg(Color::DarkGray)),
            Span::styled("Enter", Style::default().fg(Color::Cyan)),
            Span::styled("] Follow FK  [", Style::default().fg(Color::DarkGray)),
            Span::styled("i", Style::default().fg(Color::Cyan)),
            Span::styled("] Idx  [", Style::default().fg(Color::DarkGray)),
            Span::styled("c", Style::default().fg(Color::Cyan)),
            Span::styled("] Chk  [", Style::default().fg(Color::DarkGray)),
            Span::styled("?", Style::default().fg(Color::Cyan)),
            Span::styled("] Help  [", Style::default().fg(Color::DarkGray)),
            Span::styled("q", Style::default().fg(Color::Cyan)),
            Span::styled("] Quit ", Style::default().fg(Color::DarkGray)),
        ])
    };

    let footer = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::TOP));

    frame.render_widget(footer, area);
}

/// Render help overlay
fn render_help_overlay(frame: &mut Frame, area: Rect) {
    let popup_width = 60.min(area.width.saturating_sub(4));
    let popup_height = 28.min(area.height.saturating_sub(4));
    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = (area.height.saturating_sub(popup_height)) / 2;

    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let help_lines = vec![
        Line::from(Span::styled(
            "Schema Viewer - Help",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Navigation:",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("    j / Down    - Next item"),
        Line::from("    k / Up      - Previous item"),
        Line::from("    g           - Jump to first"),
        Line::from("    G           - Jump to last"),
        Line::from("    Ctrl+d      - Scroll down 5"),
        Line::from("    Ctrl+u      - Scroll up 5"),
        Line::from("    PageDown    - Page down"),
        Line::from("    PageUp      - Page up"),
        Line::from(""),
        Line::from(Span::styled(
            "  Panels:",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("    Tab         - Next panel"),
        Line::from("    Shift+Tab   - Previous panel"),
        Line::from(""),
        Line::from(Span::styled(
            "  Actions:",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("    /           - Search/filter tables"),
        Line::from("    Enter       - Follow FK relationship"),
        Line::from("    Esc         - Back (nav history) / quit"),
        Line::from("    i           - Toggle index visibility"),
        Line::from("    c           - Toggle constraint visibility"),
        Line::from(""),
        Line::from(Span::styled(
            "  General:",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("    ?           - Toggle this help"),
        Line::from("    q           - Quit viewer"),
        Line::from(""),
        Line::from(Span::styled(
            "  Press any key to close this help",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let help = Paragraph::new(help_lines)
        .block(
            Block::default()
                .title(" Help ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .style(Style::default().bg(Color::Black)),
        )
        .style(Style::default().bg(Color::Black));

    frame.render_widget(help, popup_area);
}

/// Extract column names from an index definition like "CREATE INDEX ... ON table (col1, col2)"
fn extract_columns_from_definition(definition: &str) -> Vec<String> {
    if let Some(start) = definition.find('(')
        && let Some(end) = definition[start..].find(')')
    {
        let cols = &definition[start + 1..start + end];
        return cols
            .split(',')
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect();
    }
    Vec::new()
}

/// Extract source columns from FK definition like "FOREIGN KEY (col1) REFERENCES table(col2)"
fn extract_fk_source_columns(definition: &str) -> Vec<String> {
    let lower = definition.to_lowercase();
    if let Some(fk_pos) = lower.find("foreign key") {
        let after_fk = &definition[fk_pos + 11..];
        if let Some(start) = after_fk.find('(')
            && let Some(end) = after_fk[start..].find(')')
        {
            let cols = &after_fk[start + 1..start + end];
            return cols
                .split(',')
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect();
        }
    }
    Vec::new()
}

/// Format FK column mapping for display
fn format_fk_columns(source: &[String], target: &[String]) -> String {
    source
        .iter()
        .zip(target.iter())
        .map(|(s, t)| format!("{s}->{t}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_columns_from_definition() {
        assert_eq!(
            extract_columns_from_definition("CREATE INDEX idx ON users (id, name)"),
            vec!["id", "name"]
        );
        assert_eq!(extract_columns_from_definition("(email)"), vec!["email"]);
        assert!(extract_columns_from_definition("no parens").is_empty());
    }

    #[test]
    fn test_extract_fk_source_columns() {
        assert_eq!(
            extract_fk_source_columns("FOREIGN KEY (user_id) REFERENCES users(id)"),
            vec!["user_id"]
        );
        assert!(extract_fk_source_columns("no fk here").is_empty());
    }

    #[test]
    fn test_format_fk_columns() {
        assert_eq!(
            format_fk_columns(&["user_id".to_string()], &["id".to_string()]),
            "user_id->id"
        );
        assert_eq!(
            format_fk_columns(
                &["a".to_string(), "b".to_string()],
                &["x".to_string(), "y".to_string()]
            ),
            "a->x, b->y"
        );
    }
}
