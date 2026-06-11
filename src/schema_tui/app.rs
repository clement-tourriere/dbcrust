//! Application state and event handling for the schema TUI viewer

use super::schema_data::{Relationship, SchemaData, TableListEntry};
use crate::db::{Database, TableDetails};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Which panel currently has focus
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PanelFocus {
    TableList,
    Details,
    Relationships,
}

/// The main application state for the schema TUI viewer
pub struct SchemaTuiApp {
    pub schema_data: SchemaData,
    pub should_quit: bool,
    pub show_help: bool,

    // Database handle for lazy-loading details
    database: Arc<Mutex<Database>>,
    pub(super) details_cache: HashMap<(String, String), TableDetails>,

    // Left panel: table list
    pub table_list: Vec<TableListEntry>,
    pub all_table_list: Vec<TableListEntry>,
    pub selected_table_idx: usize,
    pub table_list_scroll: usize,
    pub search_query: String,
    pub search_active: bool,

    // Center panel: table details
    pub center_scroll: u16,

    // Right panel: relationships
    pub selected_relationship_idx: usize,
    pub right_scroll: u16,

    // Focus & toggles
    pub focus: PanelFocus,
    pub show_indexes: bool,
    pub show_constraints: bool,

    // Navigation history for FK following
    pub nav_history: Vec<usize>,
}

impl SchemaTuiApp {
    pub fn new(schema_data: SchemaData, database: Arc<Mutex<Database>>) -> Self {
        let all_table_list = build_table_list(&schema_data);
        let table_list = all_table_list.clone();

        Self {
            schema_data,
            should_quit: false,
            show_help: false,
            database,
            details_cache: HashMap::new(),
            table_list: table_list.clone(),
            all_table_list: table_list,
            selected_table_idx: 0,
            table_list_scroll: 0,
            search_query: String::new(),
            search_active: false,
            center_scroll: 0,
            selected_relationship_idx: 0,
            right_scroll: 0,
            focus: PanelFocus::TableList,
            show_indexes: true,
            show_constraints: true,
            nav_history: Vec::new(),
        }
    }

    /// Get cached table details, loading on-demand if not yet cached.
    /// Returns None if loading fails or no table is selected.
    pub fn get_or_load_details(&mut self, schema: &str, table: &str) -> Option<&TableDetails> {
        let key = (schema.to_string(), table.to_string());
        if !self.details_cache.contains_key(&key) {
            let details = load_table_details(&self.database, schema, table);
            if let Some(details) = details {
                self.details_cache.insert(key.clone(), details);
            }
        }
        self.details_cache.get(&key)
    }

    /// Get the currently selected table's (schema, name) if any
    pub fn selected_table_key(&self) -> Option<(String, String)> {
        self.table_list
            .get(self.selected_table_idx)
            .and_then(|entry| entry.table_key())
    }

    /// Get relationships for the currently selected table
    pub fn selected_table_relationships(&self) -> (Vec<&Relationship>, Vec<&Relationship>) {
        let (schema, table) = match self.selected_table_key() {
            Some(k) => k,
            None => return (Vec::new(), Vec::new()),
        };

        let outgoing: Vec<&Relationship> = self
            .schema_data
            .relationships
            .iter()
            .filter(|r| r.source_schema == schema && r.source_table == table)
            .collect();

        let incoming: Vec<&Relationship> = self
            .schema_data
            .relationships
            .iter()
            .filter(|r| r.target_schema == schema && r.target_table == table)
            .collect();

        (outgoing, incoming)
    }

    /// Get flattened list of all relationships for the right panel
    pub fn all_relationships_flat(&self) -> Vec<RelationshipEntry> {
        let (outgoing, incoming) = self.selected_table_relationships();
        let mut entries = Vec::new();

        if !outgoing.is_empty() {
            entries.push(RelationshipEntry::Header("Outgoing FK".to_string()));
            for r in outgoing {
                entries.push(RelationshipEntry::Outgoing(r.clone()));
            }
        }

        if !incoming.is_empty() {
            if !entries.is_empty() {
                entries.push(RelationshipEntry::Separator);
            }
            entries.push(RelationshipEntry::Header("Incoming FK".to_string()));
            for r in incoming {
                entries.push(RelationshipEntry::Incoming(r.clone()));
            }
        }

        entries
    }

    /// Navigate to a table by following an FK relationship
    pub fn follow_relationship(&mut self) {
        let entries = self.all_relationships_flat();
        let entry = match entries.get(self.selected_relationship_idx) {
            Some(e) => e,
            None => return,
        };

        let (target_schema, target_table) = match entry {
            RelationshipEntry::Outgoing(r) => (r.target_schema.clone(), r.target_table.clone()),
            RelationshipEntry::Incoming(r) => (r.source_schema.clone(), r.source_table.clone()),
            _ => return,
        };

        // Find the target table in the table list
        if let Some(idx) = self.table_list.iter().position(|e| {
            matches!(e, TableListEntry::Table { name, schema, .. }
                if *name == target_table && *schema == target_schema)
        }) {
            self.nav_history.push(self.selected_table_idx);
            self.selected_table_idx = idx;
            self.center_scroll = 0;
            self.selected_relationship_idx = 0;
            self.right_scroll = 0;
            self.ensure_selected_visible();
        }
    }

    /// Go back in navigation history
    pub fn navigate_back(&mut self) -> bool {
        if let Some(prev_idx) = self.nav_history.pop() {
            self.selected_table_idx = prev_idx;
            self.center_scroll = 0;
            self.selected_relationship_idx = 0;
            self.right_scroll = 0;
            self.ensure_selected_visible();
            true
        } else {
            false
        }
    }

    /// Apply search filter to the table list
    pub fn apply_search_filter(&mut self) {
        if self.search_query.is_empty() {
            self.table_list = self.all_table_list.clone();
        } else {
            let query_lower = self.search_query.to_lowercase();
            let mut filtered = Vec::new();
            let mut current_schema: Option<&TableListEntry> = None;
            let mut schema_has_tables = false;

            for entry in &self.all_table_list {
                match entry {
                    TableListEntry::SchemaHeader { .. } => {
                        if schema_has_tables {
                            // Previous schema had matching tables, keep it
                        }
                        current_schema = Some(entry);
                        schema_has_tables = false;
                    }
                    TableListEntry::Table { name, .. } => {
                        if name.to_lowercase().contains(&query_lower) {
                            if !schema_has_tables {
                                if let Some(header) = current_schema {
                                    filtered.push(header.clone());
                                }
                                schema_has_tables = true;
                            }
                            filtered.push(entry.clone());
                        }
                    }
                }
            }

            self.table_list = filtered;
        }

        // Reset selection
        self.selected_table_idx = self
            .table_list
            .iter()
            .position(|e| e.is_table())
            .unwrap_or(0);
        self.table_list_scroll = 0;
    }

    /// Move selection down in the focused panel
    pub fn move_down(&mut self) {
        match self.focus {
            PanelFocus::TableList => {
                let len = self.table_list.len();
                if len == 0 {
                    return;
                }
                let mut next = self.selected_table_idx + 1;
                // Skip schema headers
                while next < len && !self.table_list[next].is_table() {
                    next += 1;
                }
                if next < len {
                    self.selected_table_idx = next;
                    self.center_scroll = 0;
                    self.selected_relationship_idx = 0;
                    self.right_scroll = 0;
                    self.ensure_selected_visible();
                }
            }
            PanelFocus::Details => {
                self.center_scroll = self.center_scroll.saturating_add(1);
            }
            PanelFocus::Relationships => {
                let entries = self.all_relationships_flat();
                if entries.is_empty() {
                    return;
                }
                let mut next = self.selected_relationship_idx + 1;
                while next < entries.len() && !entries[next].is_selectable() {
                    next += 1;
                }
                if next < entries.len() {
                    self.selected_relationship_idx = next;
                }
            }
        }
    }

    /// Move selection up in the focused panel
    pub fn move_up(&mut self) {
        match self.focus {
            PanelFocus::TableList => {
                if self.selected_table_idx == 0 {
                    return;
                }
                let mut prev = self.selected_table_idx - 1;
                while prev > 0 && !self.table_list[prev].is_table() {
                    prev -= 1;
                }
                if self.table_list.get(prev).is_some_and(|e| e.is_table()) {
                    self.selected_table_idx = prev;
                    self.center_scroll = 0;
                    self.selected_relationship_idx = 0;
                    self.right_scroll = 0;
                    self.ensure_selected_visible();
                }
            }
            PanelFocus::Details => {
                self.center_scroll = self.center_scroll.saturating_sub(1);
            }
            PanelFocus::Relationships => {
                if self.selected_relationship_idx == 0 {
                    return;
                }
                let entries = self.all_relationships_flat();
                let mut prev = self.selected_relationship_idx - 1;
                while prev > 0 && !entries[prev].is_selectable() {
                    prev -= 1;
                }
                if entries.get(prev).is_some_and(|e| e.is_selectable()) {
                    self.selected_relationship_idx = prev;
                }
            }
        }
    }

    /// Jump to first item in focused panel
    pub fn jump_first(&mut self) {
        match self.focus {
            PanelFocus::TableList => {
                self.selected_table_idx = self
                    .table_list
                    .iter()
                    .position(|e| e.is_table())
                    .unwrap_or(0);
                self.center_scroll = 0;
                self.selected_relationship_idx = 0;
                self.ensure_selected_visible();
            }
            PanelFocus::Details => {
                self.center_scroll = 0;
            }
            PanelFocus::Relationships => {
                let entries = self.all_relationships_flat();
                self.selected_relationship_idx =
                    entries.iter().position(|e| e.is_selectable()).unwrap_or(0);
            }
        }
    }

    /// Jump to last item in focused panel
    pub fn jump_last(&mut self) {
        match self.focus {
            PanelFocus::TableList => {
                let len = self.table_list.len();
                if len > 0 {
                    let mut idx = len - 1;
                    while idx > 0 && !self.table_list[idx].is_table() {
                        idx -= 1;
                    }
                    self.selected_table_idx = idx;
                    self.center_scroll = 0;
                    self.selected_relationship_idx = 0;
                    self.ensure_selected_visible();
                }
            }
            PanelFocus::Details => {
                self.center_scroll = u16::MAX / 2; // Will be clamped by rendering
            }
            PanelFocus::Relationships => {
                let entries = self.all_relationships_flat();
                if let Some(idx) = entries.iter().rposition(|e| e.is_selectable()) {
                    self.selected_relationship_idx = idx;
                }
            }
        }
    }

    /// Scroll by a page amount
    pub fn page_scroll(&mut self, delta: i16) {
        match self.focus {
            PanelFocus::TableList => {
                let amount = 10usize;
                if delta > 0 {
                    for _ in 0..amount {
                        self.move_down();
                    }
                } else {
                    for _ in 0..amount {
                        self.move_up();
                    }
                }
            }
            PanelFocus::Details => {
                if delta > 0 {
                    self.center_scroll = self.center_scroll.saturating_add(delta as u16);
                } else {
                    self.center_scroll = self.center_scroll.saturating_sub((-delta) as u16);
                }
            }
            PanelFocus::Relationships => {
                let amount = 5usize;
                if delta > 0 {
                    for _ in 0..amount {
                        self.move_down();
                    }
                } else {
                    for _ in 0..amount {
                        self.move_up();
                    }
                }
            }
        }
    }

    /// Cycle panel focus forward
    pub fn next_focus(&mut self) {
        self.focus = match self.focus {
            PanelFocus::TableList => PanelFocus::Details,
            PanelFocus::Details => PanelFocus::Relationships,
            PanelFocus::Relationships => PanelFocus::TableList,
        };
    }

    /// Cycle panel focus backward
    pub fn prev_focus(&mut self) {
        self.focus = match self.focus {
            PanelFocus::TableList => PanelFocus::Relationships,
            PanelFocus::Details => PanelFocus::TableList,
            PanelFocus::Relationships => PanelFocus::Details,
        };
    }

    /// Ensure the selected table index is visible (adjust scroll)
    fn ensure_selected_visible(&mut self) {
        if self.selected_table_idx < self.table_list_scroll {
            self.table_list_scroll = self.selected_table_idx;
        }
    }

    /// Get total table count
    pub fn table_count(&self) -> usize {
        self.all_table_list.iter().filter(|e| e.is_table()).count()
    }

    /// Get schema count
    pub fn schema_count(&self) -> usize {
        self.schema_data.schemas.len()
    }

    /// Handle keyboard events
    pub fn handle_key_event(&mut self, key: event::KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }

        // Ctrl-C always quits: it previously fell through to the plain 'c'
        // arm (toggling constraint display) in normal mode and typed a
        // literal 'c' in search mode
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }

        // Help overlay: any key closes it
        if self.show_help {
            self.show_help = false;
            return;
        }

        // Search mode: handle text input
        if self.search_active {
            match key.code {
                KeyCode::Esc => {
                    self.search_active = false;
                    self.search_query.clear();
                    self.apply_search_filter();
                }
                KeyCode::Enter => {
                    self.search_active = false;
                }
                KeyCode::Backspace => {
                    self.search_query.pop();
                    self.apply_search_filter();
                }
                KeyCode::Char(c) => {
                    self.search_query.push(c);
                    self.apply_search_filter();
                }
                _ => {}
            }
            return;
        }

        // Normal mode keybindings
        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
            }
            KeyCode::Esc => {
                // Esc: close search -> pop nav history -> quit
                if !self.search_query.is_empty() {
                    self.search_query.clear();
                    self.apply_search_filter();
                } else if !self.navigate_back() {
                    self.should_quit = true;
                }
            }

            // Navigation
            KeyCode::Down | KeyCode::Char('j') => self.move_down(),
            KeyCode::Up | KeyCode::Char('k') => self.move_up(),
            KeyCode::Char('g') => self.jump_first(),
            KeyCode::Char('G') => self.jump_last(),

            // Panel focus
            KeyCode::Tab => self.next_focus(),
            KeyCode::BackTab => self.prev_focus(),

            // Search
            KeyCode::Char('/') => {
                self.search_active = true;
            }

            // Enter: follow FK in relationships panel, or select table in table list
            KeyCode::Enter => {
                if self.focus == PanelFocus::Relationships {
                    self.follow_relationship();
                }
            }

            // Toggle visibility
            KeyCode::Char('i') => {
                self.show_indexes = !self.show_indexes;
            }
            KeyCode::Char('c') => {
                self.show_constraints = !self.show_constraints;
            }

            // Scrolling with Ctrl
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.page_scroll(5);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.page_scroll(-5);
            }

            // Page scroll
            KeyCode::PageDown => self.page_scroll(10),
            KeyCode::PageUp => self.page_scroll(-10),

            // Help
            KeyCode::Char('?') => {
                self.show_help = !self.show_help;
            }

            _ => {}
        }
    }

    /// Poll for events and handle them
    pub fn poll_events(&mut self, timeout: Duration) -> std::io::Result<bool> {
        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
        {
            self.handle_key_event(key);
            return Ok(true);
        }
        Ok(false)
    }
}

/// Relationship entry for the right panel display
#[derive(Debug, Clone)]
pub enum RelationshipEntry {
    Header(String),
    Separator,
    Outgoing(Relationship),
    Incoming(Relationship),
}

impl RelationshipEntry {
    pub fn is_selectable(&self) -> bool {
        matches!(
            self,
            RelationshipEntry::Outgoing(_) | RelationshipEntry::Incoming(_)
        )
    }
}

/// Load table details from the database (sync wrapper for async metadata query).
/// Uses `block_in_place` + `block_on` which is safe with the multi-threaded tokio runtime.
#[allow(clippy::await_holding_lock)]
fn load_table_details(
    database: &Arc<Mutex<Database>>,
    schema: &str,
    table: &str,
) -> Option<TableDetails> {
    let db = database.clone();
    let schema_owned = schema.to_string();
    let table_owned = table.to_string();

    tokio::task::block_in_place(|| {
        let handle = tokio::runtime::Handle::current();
        handle.block_on(async {
            let db_guard = db.lock().unwrap();
            let client = db_guard.get_database_client()?;
            let metadata = client.get_metadata_provider();
            metadata
                .get_table_details(&table_owned, Some(&schema_owned))
                .await
                .ok()
        })
    })
}

/// Build the flattened table list from schema data
fn build_table_list(schema_data: &SchemaData) -> Vec<TableListEntry> {
    let mut list = Vec::new();

    for schema in &schema_data.schemas {
        list.push(TableListEntry::SchemaHeader {
            name: schema.name.clone(),
        });

        for table in &schema.tables {
            list.push(TableListEntry::Table {
                name: table.name.clone(),
                schema: table.schema.clone(),
                outgoing_fk: table.outgoing_fk_count,
                incoming_fk: table.incoming_fk_count,
            });
        }
    }

    list
}
