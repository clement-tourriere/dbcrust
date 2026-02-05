//! UI rendering for the TUI explain visualizer

use super::app::ExplainTuiApp;
use super::plan_tree::PlanNode;
use crate::performance_analyzer::PerformanceLevel;
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

/// Color scheme for performance levels
fn performance_color(level: PerformanceLevel) -> Color {
    match level {
        PerformanceLevel::Excellent => Color::Green,
        PerformanceLevel::Good => Color::LightGreen,
        PerformanceLevel::Warning => Color::Yellow,
        PerformanceLevel::Poor => Color::LightRed,
        PerformanceLevel::Critical => Color::Red,
    }
}

/// Performance level indicator character
fn performance_indicator(level: PerformanceLevel) -> &'static str {
    match level {
        PerformanceLevel::Excellent => "[+]",
        PerformanceLevel::Good => "[+]",
        PerformanceLevel::Warning => "[!]",
        PerformanceLevel::Poor => "[!]",
        PerformanceLevel::Critical => "[X]",
    }
}

/// Grade color based on score
fn grade_color(score: u8) -> Color {
    match score {
        90..=100 => Color::Green,
        80..=89 => Color::LightGreen,
        70..=79 => Color::Yellow,
        60..=69 => Color::LightYellow,
        50..=59 => Color::LightRed,
        _ => Color::Red,
    }
}

/// Render the entire TUI
pub fn render(frame: &mut Frame, app: &ExplainTuiApp) {
    let area = frame.area();

    // Main layout: header, content, footer
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
    render_footer(frame, main_chunks[2]);

    // Render help overlay if shown
    if app.show_help {
        render_help_overlay(frame, area);
    }
}

/// Render the header with plan statistics
fn render_header(frame: &mut Frame, app: &ExplainTuiApp, area: Rect) {
    let stats = &app.statistics;

    let score_color = grade_color(stats.performance_score);

    let title = Line::from(vec![
        Span::styled(
            " Query Plan Visualizer ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("| Score: "),
        Span::styled(
            format!("{}/100", stats.performance_score),
            Style::default()
                .fg(score_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | Grade: "),
        Span::styled(
            stats.grade(),
            Style::default()
                .fg(score_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | "),
        if let Some(time) = stats.total_time {
            Span::styled(
                format!("{:.2}ms", time),
                Style::default().fg(if time > 100.0 {
                    Color::Red
                } else if time > 10.0 {
                    Color::Yellow
                } else {
                    Color::Green
                }),
            )
        } else {
            Span::styled(
                format!("cost: {:.0}", stats.total_cost),
                Style::default().fg(Color::White),
            )
        },
        Span::raw(" | "),
        Span::styled(
            format!("{} nodes", stats.total_nodes),
            Style::default().fg(Color::White),
        ),
        if stats.total_warnings > 0 {
            Span::styled(
                format!(" | {} warnings", stats.total_warnings),
                Style::default().fg(Color::Yellow),
            )
        } else {
            Span::raw("")
        },
    ]);

    let header = Paragraph::new(title)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .style(Style::default().fg(Color::White));

    frame.render_widget(header, area);
}

/// Render the main content area (tree + details)
fn render_content(frame: &mut Frame, app: &ExplainTuiApp, area: Rect) {
    // Split into tree view and details panel
    let content_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(55), // Tree view
            Constraint::Percentage(45), // Details panel
        ])
        .split(area);

    render_tree(frame, app, content_chunks[0]);
    render_details(frame, app, content_chunks[1]);
}

/// Render the plan tree
fn render_tree(frame: &mut Frame, app: &ExplainTuiApp, area: Rect) {
    let mut items = Vec::new();
    let mut selected_index = 0;

    build_tree_items(
        &app.plan_root,
        app,
        &[],
        0,
        &mut items,
        &app.selected_path,
        &mut selected_index,
    );

    let items_len = items.len();

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Plan Tree ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if app.focus == 0 {
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

    // Create a stateful list with selection
    let mut list_state = ratatui::widgets::ListState::default();
    list_state.select(Some(selected_index));

    frame.render_stateful_widget(list, area, &mut list_state);

    // Add scrollbar if needed
    if items_len > area.height as usize - 2 {
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

/// Recursively build tree items for the list
fn build_tree_items<'a>(
    node: &'a PlanNode,
    app: &ExplainTuiApp,
    current_path: &[usize],
    depth: usize,
    items: &mut Vec<ListItem<'a>>,
    selected_path: &[usize],
    current_index: &mut usize,
) {
    let is_selected = current_path == selected_path;
    let is_expanded = app.is_expanded(&node.id);
    let has_children = !node.children.is_empty();

    // Build the tree prefix (indentation and branch characters)
    let indent = "  ".repeat(depth);
    let branch = if has_children {
        if is_expanded { "[-] " } else { "[+] " }
    } else {
        "    "
    };

    // Performance indicator
    let perf_indicator = performance_indicator(node.performance_level);
    let perf_color = performance_color(node.performance_level);

    // Warning indicator
    let warn_indicator = if node.has_warnings() { " !" } else { "" };

    // Build the line spans
    let mut spans = vec![
        Span::styled(indent, Style::default().fg(Color::DarkGray)),
        Span::styled(
            branch,
            Style::default().fg(if has_children {
                Color::Cyan
            } else {
                Color::DarkGray
            }),
        ),
        Span::styled(perf_indicator, Style::default().fg(perf_color)),
        Span::raw(" "),
        Span::styled(
            node.display_label(),
            Style::default()
                .fg(Color::White)
                .add_modifier(if is_selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
    ];

    // Add cost/time info
    let cost_info = node.cost_summary();
    spans.push(Span::styled(
        format!(" ({})", cost_info),
        Style::default().fg(Color::DarkGray),
    ));

    // Add warning indicator
    if !warn_indicator.is_empty() {
        spans.push(Span::styled(
            warn_indicator,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }

    items.push(ListItem::new(Line::from(spans)));

    if is_selected {
        *current_index = items.len() - 1;
    }

    // Recursively add children if expanded
    if is_expanded {
        for (i, child) in node.children.iter().enumerate() {
            let mut child_path = current_path.to_vec();
            child_path.push(i);
            build_tree_items(
                child,
                app,
                &child_path,
                depth + 1,
                items,
                selected_path,
                current_index,
            );
        }
    }
}

/// Render the details panel for the selected node
fn render_details(frame: &mut Frame, app: &ExplainTuiApp, area: Rect) {
    let node = app.selected_node();
    let mut lines: Vec<Line> = Vec::new();

    // Node type header
    lines.push(Line::from(vec![
        Span::styled(
            &node.node_type,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            performance_indicator(node.performance_level),
            Style::default().fg(performance_color(node.performance_level)),
        ),
    ]));
    lines.push(Line::from(""));

    // Table/Relation
    if let Some(ref relation) = node.relation_name {
        let display = if let Some(ref schema) = node.schema {
            format!("{}.{}", schema, relation)
        } else {
            relation.clone()
        };
        lines.push(Line::from(vec![
            Span::styled("Table: ", Style::default().fg(Color::Yellow)),
            Span::styled(display, Style::default().fg(Color::White)),
        ]));
    }

    // Index
    if let Some(ref index) = node.index_name {
        lines.push(Line::from(vec![
            Span::styled("Index: ", Style::default().fg(Color::Yellow)),
            Span::styled(index, Style::default().fg(Color::Green)),
        ]));
    }

    lines.push(Line::from(""));

    // Cost information
    lines.push(Line::from(vec![
        Span::styled("Cost: ", Style::default().fg(Color::Yellow)),
        Span::styled(
            format!("{:.2}..{:.2}", node.startup_cost, node.total_cost),
            Style::default().fg(Color::White),
        ),
    ]));

    // Timing (if available)
    if let Some(time) = node.actual_time_ms {
        let time_color = if time > 100.0 {
            Color::Red
        } else if time > 10.0 {
            Color::Yellow
        } else {
            Color::Green
        };
        lines.push(Line::from(vec![
            Span::styled("Time: ", Style::default().fg(Color::Yellow)),
            Span::styled(format!("{:.3} ms", time), Style::default().fg(time_color)),
        ]));

        if let Some(startup_time) = node.actual_startup_time_ms {
            lines.push(Line::from(vec![
                Span::styled("  Startup: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:.3} ms", startup_time),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }

    // Row estimates vs actual
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Rows (est): ", Style::default().fg(Color::Yellow)),
        Span::styled(
            format!("{}", node.plan_rows),
            Style::default().fg(Color::White),
        ),
    ]));

    if let Some(actual) = node.actual_rows {
        let ratio = if node.plan_rows > 0 {
            actual as f64 / node.plan_rows as f64
        } else {
            1.0
        };
        let ratio_color = if (0.5..=2.0).contains(&ratio) {
            Color::Green
        } else if (0.1..=10.0).contains(&ratio) {
            Color::Yellow
        } else {
            Color::Red
        };

        lines.push(Line::from(vec![
            Span::styled("Rows (actual): ", Style::default().fg(Color::Yellow)),
            Span::styled(format!("{}", actual), Style::default().fg(Color::White)),
            Span::styled(
                format!(" ({:.1}x)", ratio),
                Style::default().fg(ratio_color),
            ),
        ]));
    }

    if let Some(loops) = node.actual_loops {
        if loops > 1 {
            lines.push(Line::from(vec![
                Span::styled("Loops: ", Style::default().fg(Color::Yellow)),
                Span::styled(format!("{}", loops), Style::default().fg(Color::White)),
            ]));
        }
    }

    // Filter conditions
    if node.filter.is_some()
        || node.index_cond.is_some()
        || node.recheck_cond.is_some()
        || node.hash_cond.is_some()
        || node.join_filter.is_some()
    {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Conditions:",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::UNDERLINED),
        )));

        if let Some(ref filter) = node.filter {
            lines.push(Line::from(vec![
                Span::styled("  Filter: ", Style::default().fg(Color::Yellow)),
                Span::styled(truncate_str(filter, 40), Style::default().fg(Color::White)),
            ]));
        }

        if let Some(ref cond) = node.index_cond {
            lines.push(Line::from(vec![
                Span::styled("  Index Cond: ", Style::default().fg(Color::Yellow)),
                Span::styled(truncate_str(cond, 36), Style::default().fg(Color::White)),
            ]));
        }

        if let Some(ref cond) = node.hash_cond {
            lines.push(Line::from(vec![
                Span::styled("  Hash Cond: ", Style::default().fg(Color::Yellow)),
                Span::styled(truncate_str(cond, 37), Style::default().fg(Color::White)),
            ]));
        }

        if let Some(ref cond) = node.join_filter {
            lines.push(Line::from(vec![
                Span::styled("  Join Filter: ", Style::default().fg(Color::Yellow)),
                Span::styled(truncate_str(cond, 35), Style::default().fg(Color::White)),
            ]));
        }

        if let Some(ref cond) = node.recheck_cond {
            lines.push(Line::from(vec![
                Span::styled("  Recheck: ", Style::default().fg(Color::Yellow)),
                Span::styled(truncate_str(cond, 39), Style::default().fg(Color::White)),
            ]));
        }
    }

    // Sort information
    if !node.sort_key.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Sort Key: ", Style::default().fg(Color::Yellow)),
            Span::styled(node.sort_key.join(", "), Style::default().fg(Color::White)),
        ]));

        if let Some(ref method) = node.sort_method {
            let method_color = if method.contains("external") {
                Color::Red
            } else {
                Color::Green
            };
            lines.push(Line::from(vec![
                Span::styled("  Method: ", Style::default().fg(Color::DarkGray)),
                Span::styled(method, Style::default().fg(method_color)),
            ]));
        }
    }

    // Buffer usage
    if node.shared_hit_blocks.is_some() || node.shared_read_blocks.is_some() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Buffer Usage:",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::UNDERLINED),
        )));

        if let Some(hits) = node.shared_hit_blocks {
            lines.push(Line::from(vec![
                Span::styled("  Cache Hits: ", Style::default().fg(Color::Yellow)),
                Span::styled(format!("{}", hits), Style::default().fg(Color::Green)),
            ]));
        }

        if let Some(reads) = node.shared_read_blocks {
            let read_color = if reads > 100 {
                Color::Yellow
            } else {
                Color::White
            };
            lines.push(Line::from(vec![
                Span::styled("  Disk Reads: ", Style::default().fg(Color::Yellow)),
                Span::styled(format!("{}", reads), Style::default().fg(read_color)),
            ]));
        }
    }

    // Warnings
    if !node.warnings.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Warnings:",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        for warning in &node.warnings {
            lines.push(Line::from(vec![
                Span::styled("  ! ", Style::default().fg(Color::Red)),
                Span::styled(warning, Style::default().fg(Color::Yellow)),
            ]));
        }
    }

    // Recommendations
    if !node.recommendations.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Recommendations:",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        for rec in &node.recommendations {
            lines.push(Line::from(vec![
                Span::styled("  > ", Style::default().fg(Color::Cyan)),
                Span::styled(rec, Style::default().fg(Color::White)),
            ]));
        }
    }

    // Children count
    if !node.children.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Children: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{} node(s)", node.children.len()),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }

    let details = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Node Details ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if app.focus == 1 {
                    Color::Cyan
                } else {
                    Color::DarkGray
                }))
                .padding(Padding::horizontal(1)),
        )
        .wrap(Wrap { trim: true })
        .scroll((app.details_scroll, 0));

    frame.render_widget(details, area);
}

/// Render the footer with keybindings
fn render_footer(frame: &mut Frame, area: Rect) {
    let help_text = Line::from(vec![
        Span::styled(" [", Style::default().fg(Color::DarkGray)),
        Span::styled("j/k", Style::default().fg(Color::Cyan)),
        Span::styled("] Navigate  [", Style::default().fg(Color::DarkGray)),
        Span::styled("Enter", Style::default().fg(Color::Cyan)),
        Span::styled("] Toggle  [", Style::default().fg(Color::DarkGray)),
        Span::styled("e", Style::default().fg(Color::Cyan)),
        Span::styled("] Expand All  [", Style::default().fg(Color::DarkGray)),
        Span::styled("c", Style::default().fg(Color::Cyan)),
        Span::styled("] Collapse  [", Style::default().fg(Color::DarkGray)),
        Span::styled("Tab", Style::default().fg(Color::Cyan)),
        Span::styled("] Focus  [", Style::default().fg(Color::DarkGray)),
        Span::styled("?", Style::default().fg(Color::Cyan)),
        Span::styled("] Help  [", Style::default().fg(Color::DarkGray)),
        Span::styled("q", Style::default().fg(Color::Cyan)),
        Span::styled("] Quit ", Style::default().fg(Color::DarkGray)),
    ]);

    let footer = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::TOP));

    frame.render_widget(footer, area);
}

/// Render help overlay
fn render_help_overlay(frame: &mut Frame, area: Rect) {
    // Calculate centered popup area
    let popup_width = 60.min(area.width.saturating_sub(4));
    let popup_height = 18.min(area.height.saturating_sub(4));
    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = (area.height.saturating_sub(popup_height)) / 2;

    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    // Clear the area behind the popup
    frame.render_widget(Clear, popup_area);

    let help_lines = vec![
        Line::from(Span::styled(
            "Keyboard Shortcuts",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Navigation:",
            Style::default().fg(Color::Yellow),
        )]),
        Line::from("    j / Down    - Next node"),
        Line::from("    k / Up      - Previous node"),
        Line::from("    h / Left    - Collapse / Go to parent"),
        Line::from("    l / Right   - Expand / Go to first child"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Tree Control:",
            Style::default().fg(Color::Yellow),
        )]),
        Line::from("    Enter       - Toggle expand/collapse"),
        Line::from("    e           - Expand all nodes"),
        Line::from("    c           - Collapse all nodes"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Other:",
            Style::default().fg(Color::Yellow),
        )]),
        Line::from("    Tab         - Switch focus (tree/details)"),
        Line::from("    q / Esc     - Quit visualizer"),
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

/// Truncate a string to a maximum length, adding ellipsis if needed
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}
