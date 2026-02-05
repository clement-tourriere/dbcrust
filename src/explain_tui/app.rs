//! Application state and event handling for the TUI explain visualizer

use super::plan_tree::{PlanNode, PlanStatistics};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::time::Duration;

/// The main application state for the TUI
#[derive(Debug)]
pub struct ExplainTuiApp {
    /// The root of the plan tree
    pub plan_root: PlanNode,
    /// Statistics about the plan
    pub statistics: PlanStatistics,
    /// Currently selected node path (list of indices into children)
    pub selected_path: Vec<usize>,
    /// Set of expanded node IDs
    pub expanded_nodes: std::collections::HashSet<String>,
    /// Whether the help overlay is shown
    pub show_help: bool,
    /// Whether the app should quit
    pub should_quit: bool,
    /// Vertical scroll offset for the details panel
    pub details_scroll: u16,
    /// Current focus: 0 = tree, 1 = details
    pub focus: u8,
}

impl ExplainTuiApp {
    /// Create a new app instance from a plan tree
    pub fn new(plan_root: PlanNode) -> Self {
        let statistics = PlanStatistics::from_plan(&plan_root);

        // Start with root expanded
        let mut expanded_nodes = std::collections::HashSet::new();
        expanded_nodes.insert(plan_root.id.clone());

        // Also expand the first few levels for better initial view
        Self::expand_initial_nodes(&plan_root, &mut expanded_nodes, 0, 2);

        Self {
            plan_root,
            statistics,
            selected_path: vec![],
            expanded_nodes,
            show_help: false,
            should_quit: false,
            details_scroll: 0,
            focus: 0,
        }
    }

    /// Expand nodes up to a certain depth for initial view
    fn expand_initial_nodes(
        node: &PlanNode,
        expanded: &mut std::collections::HashSet<String>,
        current_depth: usize,
        max_depth: usize,
    ) {
        if current_depth > max_depth {
            return;
        }
        expanded.insert(node.id.clone());
        for child in &node.children {
            Self::expand_initial_nodes(child, expanded, current_depth + 1, max_depth);
        }
    }

    /// Get the currently selected node
    pub fn selected_node(&self) -> &PlanNode {
        let mut node = &self.plan_root;
        for &idx in &self.selected_path {
            if idx < node.children.len() {
                node = &node.children[idx];
            }
        }
        node
    }

    /// Check if a node is expanded
    pub fn is_expanded(&self, node_id: &str) -> bool {
        self.expanded_nodes.contains(node_id)
    }

    /// Toggle expansion of the selected node
    pub fn toggle_selected(&mut self) {
        let node = self.selected_node();
        let node_id = node.id.clone();

        if !node.children.is_empty() {
            if self.expanded_nodes.contains(&node_id) {
                self.expanded_nodes.remove(&node_id);
            } else {
                self.expanded_nodes.insert(node_id);
            }
        }
    }

    /// Expand the selected node
    pub fn expand_selected(&mut self) {
        let node = self.selected_node();
        if !node.children.is_empty() {
            self.expanded_nodes.insert(node.id.clone());
        }
    }

    /// Collapse the selected node
    pub fn collapse_selected(&mut self) {
        let node_id = self.selected_node().id.clone();
        self.expanded_nodes.remove(&node_id);
    }

    /// Move selection to the next visible node
    pub fn select_next(&mut self) {
        // Build list of visible nodes in order
        let visible = self.build_visible_list();
        if visible.is_empty() {
            return;
        }

        // Find current position
        let current_idx = visible
            .iter()
            .position(|p| p == &self.selected_path)
            .unwrap_or(0);

        // Move to next
        if current_idx + 1 < visible.len() {
            self.selected_path = visible[current_idx + 1].clone();
            self.details_scroll = 0; // Reset scroll when changing selection
        }
    }

    /// Move selection to the previous visible node
    pub fn select_prev(&mut self) {
        let visible = self.build_visible_list();
        if visible.is_empty() {
            return;
        }

        let current_idx = visible
            .iter()
            .position(|p| p == &self.selected_path)
            .unwrap_or(0);

        if current_idx > 0 {
            self.selected_path = visible[current_idx - 1].clone();
            self.details_scroll = 0;
        }
    }

    /// Jump to first visible node
    pub fn select_first(&mut self) {
        self.selected_path = vec![];
        self.details_scroll = 0;
    }

    /// Jump to last visible node
    pub fn select_last(&mut self) {
        let visible = self.build_visible_list();
        if let Some(last) = visible.last() {
            self.selected_path = last.clone();
            self.details_scroll = 0;
        }
    }

    /// Move to parent node (or collapse if expanded)
    pub fn select_parent_or_collapse(&mut self) {
        let node = self.selected_node();

        // If node is expanded and has children, collapse it
        if !node.children.is_empty() && self.is_expanded(&node.id) {
            self.collapse_selected();
        } else if !self.selected_path.is_empty() {
            // Otherwise go to parent
            self.selected_path.pop();
            self.details_scroll = 0;
        }
    }

    /// Expand and go into first child
    pub fn select_child_or_expand(&mut self) {
        let node = self.selected_node();

        if node.children.is_empty() {
            return;
        }

        if !self.is_expanded(&node.id) {
            // First expand
            self.expand_selected();
        } else {
            // Go to first child
            self.selected_path.push(0);
            self.details_scroll = 0;
        }
    }

    /// Build a list of paths to all visible nodes in tree order
    fn build_visible_list(&self) -> Vec<Vec<usize>> {
        let mut visible = Vec::new();
        self.collect_visible(&self.plan_root, &[], &mut visible);
        visible
    }

    /// Recursively collect visible node paths
    fn collect_visible(
        &self,
        node: &PlanNode,
        current_path: &[usize],
        visible: &mut Vec<Vec<usize>>,
    ) {
        visible.push(current_path.to_vec());

        if self.is_expanded(&node.id) {
            for (i, child) in node.children.iter().enumerate() {
                let mut child_path = current_path.to_vec();
                child_path.push(i);
                self.collect_visible(child, &child_path, visible);
            }
        }
    }

    /// Expand all nodes
    pub fn expand_all(&mut self) {
        self.expand_all_recursive(&self.plan_root.clone());
    }

    fn expand_all_recursive(&mut self, node: &PlanNode) {
        self.expanded_nodes.insert(node.id.clone());
        for child in &node.children {
            self.expand_all_recursive(child);
        }
    }

    /// Collapse all nodes (except root)
    pub fn collapse_all(&mut self) {
        self.expanded_nodes.clear();
        self.expanded_nodes.insert(self.plan_root.id.clone());
        self.selected_path.clear();
    }

    /// Scroll the details panel
    pub fn scroll_details(&mut self, delta: i16) {
        if delta < 0 {
            self.details_scroll = self.details_scroll.saturating_sub((-delta) as u16);
        } else {
            self.details_scroll = self.details_scroll.saturating_add(delta as u16);
        }
    }

    /// Toggle help overlay
    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    /// Handle keyboard events
    pub fn handle_key_event(&mut self, key: event::KeyEvent) {
        // Only handle key press events (not release)
        if key.kind != KeyEventKind::Press {
            return;
        }

        // If help is showing, any key closes it
        if self.show_help {
            self.show_help = false;
            return;
        }

        match key.code {
            // Quit
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
            }

            // Navigation
            KeyCode::Up | KeyCode::Char('k') => {
                self.select_prev();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.select_next();
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.select_parent_or_collapse();
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.select_child_or_expand();
            }
            KeyCode::Char('g') => {
                self.select_first();
            }
            KeyCode::Char('G') => {
                self.select_last();
            }

            // Toggle expand/collapse
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.toggle_selected();
            }

            // Expand/collapse all
            KeyCode::Char('e') => {
                self.expand_all();
            }
            KeyCode::Char('c') => {
                self.collapse_all();
            }

            // Help
            KeyCode::Char('?') => {
                self.toggle_help();
            }

            // Details scroll (with Ctrl modifier)
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_details(5);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_details(-5);
            }

            // Page up/down for details
            KeyCode::PageDown => {
                self.scroll_details(10);
            }
            KeyCode::PageUp => {
                self.scroll_details(-10);
            }

            // Tab to switch focus
            KeyCode::Tab => {
                self.focus = (self.focus + 1) % 2;
            }

            _ => {}
        }
    }

    /// Poll for events and handle them
    /// Returns true if an event was handled
    pub fn poll_events(&mut self, timeout: Duration) -> std::io::Result<bool> {
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                self.handle_key_event(key);
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// Result of running the TUI
#[derive(Debug)]
pub enum TuiResult {
    /// User exited normally
    Quit,
    /// An error occurred
    Error(String),
}
