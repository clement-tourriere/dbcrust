//! Settings → Saved queries: every saved query of every scope, searchable,
//! with an editor for the selected one.
//!
//! The list reads the store's published catalogue (never its lock); every
//! write runs on a worker, then `AppState::storage_changed` lets the toolbar
//! menu, the palette and this view re-read it. The selection is kept by
//! storage key across refreshes.

use super::*;
use crate::actions::SaveNamedQuery;
use crate::preferences;
use dbcrust_gui_core::dbcrust::config::{NamedQueriesStorage, NamedQueryScope};
use dbcrust_gui_core::store::SavedQuery;
use dbcrust_gui_core::{DatabaseType, DatabaseTypeExt};
use gpui_kit::component::{
    button::ButtonVariant,
    input::{Editor, EditorState, Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
};
use std::cell::Cell;
use std::sync::Arc;

/// Height of a list row: name line + SQL preview line.
const ROW_HEIGHT: Pixels = px(46.);
/// Width of the list pane.
const LIST_WIDTH: Pixels = px(264.);

/// What the editor holds, compared with what is saved to know if it is dirty.
#[derive(Clone, Debug, Default, PartialEq)]
struct Draft {
    name: String,
    sql: String,
    scope: Option<NamedQueryScope>,
    description: String,
}

impl From<&SavedQuery> for Draft {
    fn from(entry: &SavedQuery) -> Self {
        Self {
            name: entry.name.clone(),
            sql: entry.query.clone(),
            scope: Some(entry.scope.clone()),
            description: entry.description.clone().unwrap_or_default(),
        }
    }
}

/// The list's scope filter.
#[derive(Clone, Debug, PartialEq)]
enum ScopeFilter {
    All,
    Global,
    Engine(DatabaseType),
    Sessions,
}

impl ScopeFilter {
    fn label(&self) -> String {
        match self {
            Self::All => "All scopes".into(),
            Self::Global => "Global".into(),
            Self::Engine(engine) => engine.display_name().into(),
            Self::Sessions => "Sessions".into(),
        }
    }

    fn matches(&self, scope: &NamedQueryScope) -> bool {
        match (self, scope) {
            (Self::All, _) => true,
            (Self::Global, NamedQueryScope::Global) => true,
            (Self::Engine(engine), NamedQueryScope::DatabaseType(scope)) => engine == scope,
            (Self::Sessions, NamedQueryScope::Session(_)) => true,
            _ => false,
        }
    }
}

/// How a scope reads in the manager.
fn scope_text(scope: &NamedQueryScope) -> String {
    match scope {
        NamedQueryScope::Global => "Global".into(),
        NamedQueryScope::DatabaseType(engine) => engine.display_name().into(),
        NamedQueryScope::Session(session) => format!("Session “{session}”"),
    }
}

pub(super) struct SavedQueriesView {
    search: Entity<InputState>,
    name: Entity<InputState>,
    description: Entity<InputState>,
    editor: Entity<EditorState>,
    entries: Arc<Vec<SavedQuery>>,
    /// Lowercased name, SQL and description of each entry, for search.
    haystacks: Vec<String>,
    /// Indexes into `entries` that pass the search and the filter.
    filtered: Vec<usize>,
    filter: ScopeFilter,
    /// Key of the selected query; `None` with `editing` is a new query.
    selected: Option<String>,
    /// Scope of the query being edited.
    scope: NamedQueryScope,
    baseline: Draft,
    editing: bool,
    saving: bool,
    error: Option<String>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl SavedQueriesView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search saved queries"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("monthly_revenue"));
        let description = cx.new(|cx| {
            InputState::new(window, cx).placeholder("What it returns, when to use it (optional)")
        });
        let editor = crate::query::editor::new_detached_editor_state(
            String::new(),
            Rc::new(Cell::new(None)),
            window,
            cx,
        );
        editor.update(cx, |editor, cx| {
            editor.set_placeholder("SELECT … — use $1, $2 for parameters", window, cx)
        });
        let mut subscriptions = vec![cx.subscribe(&search, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.rebuild(cx);
            }
        })];
        // Typing clears a stale error and updates the dirty state
        for input in [&name, &description] {
            subscriptions.push(cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.error = None;
                    cx.notify();
                }
            }));
        }
        subscriptions.push(cx.subscribe(&editor, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.error = None;
                cx.notify();
            }
        }));
        subscriptions.push(cx.subscribe_in(
            &AppState::global(cx),
            window,
            |this, _, event, window, cx| {
                if matches!(event, AppEvent::StorageChanged) {
                    this.refresh(window, cx);
                }
            },
        ));
        subscriptions.push(cx.observe_global::<Preferences>(|_, cx| cx.notify()));
        let mut this = Self {
            search,
            name,
            description,
            editor,
            entries: Arc::default(),
            haystacks: Vec::new(),
            filtered: Vec::new(),
            filter: ScopeFilter::All,
            selected: None,
            scope: NamedQueryScope::Global,
            baseline: Draft::default(),
            editing: false,
            saving: false,
            error: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        this.refresh(window, cx);
        this
    }

    fn draft(&self, cx: &App) -> Draft {
        Draft {
            name: self.name.read(cx).value().to_string(),
            sql: self.editor.read(cx).value().to_string(),
            scope: Some(self.scope.clone()),
            description: self.description.read(cx).value().to_string(),
        }
    }

    fn dirty(&self, cx: &App) -> bool {
        self.editing && self.draft(cx) != self.baseline
    }

    fn selected_entry(&self) -> Option<&SavedQuery> {
        let key = self.selected.as_ref()?;
        self.entries.iter().find(|entry| &entry.key == key)
    }

    /// Re-read the catalogue (cheap: no lock, no I/O) and keep the
    /// selection. A clean editor follows the stored query; one with unsaved
    /// edits is left alone.
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = store(cx).all_named_queries();
        if Arc::ptr_eq(&entries, &self.entries) {
            return;
        }
        self.entries = entries;
        self.haystacks = self
            .entries
            .iter()
            .map(|entry| {
                format!(
                    "{}\n{}\n{}",
                    entry.name,
                    entry.query,
                    entry.description.as_deref().unwrap_or_default()
                )
                .to_lowercase()
            })
            .collect();
        if let ScopeFilter::Engine(engine) = &self.filter
            && !self.entries.iter().any(|entry| {
                matches!(&entry.scope, NamedQueryScope::DatabaseType(scope) if scope == engine)
            })
        {
            self.filter = ScopeFilter::All;
        }
        self.rebuild(cx);
        if self.saving || self.dirty(cx) {
            return;
        }
        match self.selected_entry().cloned() {
            Some(entry) => {
                if Draft::from(&entry) != self.baseline {
                    self.load(Some(entry), window, cx);
                }
            }
            // Deleted elsewhere, or nothing chosen yet: the first match
            None if self.selected.is_some() || !self.editing => {
                let first = self.filtered.first().map(|ix| self.entries[*ix].clone());
                let found = first.is_some();
                self.load(first, window, cx);
                self.editing = found;
            }
            None => {}
        }
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let search = self.search.read(cx).value().trim().to_lowercase();
        self.filtered = self
            .entries
            .iter()
            .enumerate()
            .filter(|(ix, entry)| {
                self.filter.matches(&entry.scope)
                    && (search.is_empty() || self.haystacks[*ix].contains(&search))
            })
            .map(|(ix, _)| ix)
            .collect();
        cx.notify();
    }

    fn set_filter(&mut self, filter: ScopeFilter, cx: &mut Context<Self>) {
        self.filter = filter;
        self.rebuild(cx);
    }

    /// Put `entry` (or an empty new query) in the editor.
    fn load(&mut self, entry: Option<SavedQuery>, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = entry.as_ref().map(|entry| entry.key.clone());
        let draft = match &entry {
            Some(entry) => Draft::from(entry),
            None => Draft {
                scope: Some(self.new_query_scope()),
                ..Draft::default()
            },
        };
        self.name.update(cx, |input, cx| {
            input.set_value(draft.name.clone(), window, cx)
        });
        self.description.update(cx, |input, cx| {
            input.set_value(draft.description.clone(), window, cx)
        });
        self.editor.update(cx, |editor, cx| {
            editor.set_value(draft.sql.clone(), window, cx)
        });
        self.scope = draft.scope.clone().unwrap_or(NamedQueryScope::Global);
        self.baseline = draft;
        self.editing = true;
        self.error = None;
        if let Some(position) = self
            .filtered
            .iter()
            .position(|ix| Some(&self.entries[*ix].key) == self.selected.as_ref())
        {
            self.scroll
                .scroll_to_item(position, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    /// A new query starts in the scope the list is filtered on.
    fn new_query_scope(&self) -> NamedQueryScope {
        match &self.filter {
            ScopeFilter::Engine(engine) => NamedQueryScope::DatabaseType(engine.clone()),
            _ => NamedQueryScope::Global,
        }
    }

    /// Select `key` (`None`: start a new query), asking first when the
    /// editor has unsaved changes.
    fn select(&mut self, key: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || (key.is_some() && self.selected == key && self.editing) {
            return;
        }
        if !self.dirty(cx) {
            self.select_now(key, window, cx);
            return;
        }
        let name = self.draft(cx).name;
        let title = if name.trim().is_empty() {
            "Discard the new saved query?".to_string()
        } else {
            format!("Discard changes to “{}”?", name.trim())
        };
        let view = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            let key = key.clone();
            alert
                .confirm()
                .title(title.clone())
                .description("Your edits to this saved query are not saved.")
                .ok_text("Discard changes")
                .ok_variant(ButtonVariant::Danger)
                .cancel_text("Keep editing")
                .on_ok(move |_, window, cx| {
                    view.update(cx, |this, cx| this.select_now(key.clone(), window, cx));
                    true
                })
        });
    }

    fn select_now(&mut self, key: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        match key {
            Some(key) => {
                let entry = self.entries.iter().find(|entry| entry.key == key).cloned();
                self.load(entry, window, cx);
            }
            None => {
                self.load(None, window, cx);
                self.name.update(cx, |input, cx| input.focus(window, cx));
            }
        }
    }

    /// Move the selection by `delta` rows (↑ / ↓).
    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.filtered.is_empty() {
            return;
        }
        let current = self
            .filtered
            .iter()
            .position(|ix| Some(&self.entries[*ix].key) == self.selected.as_ref());
        let next = match current {
            Some(position) => position
                .saturating_add_signed(delta)
                .min(self.filtered.len() - 1),
            None => 0,
        };
        let key = self.entries[self.filtered[next]].key.clone();
        self.select(Some(key), window, cx);
    }

    /// Why the draft cannot be saved as is, before asking the store.
    fn validate(&self, draft: &Draft) -> Result<(), String> {
        let name = draft.name.trim();
        if name.is_empty() {
            return Err("Enter a name for the saved query.".into());
        }
        if draft.sql.trim().is_empty() {
            return Err("Write SQL for the saved query.".into());
        }
        let target = NamedQueriesStorage::generate_key(name, &self.scope);
        if self.selected.as_ref() != Some(&target)
            && self.entries.iter().any(|entry| entry.key == target)
        {
            return Err(match &self.scope {
                NamedQueryScope::Global => {
                    format!("A global saved query named “{name}” already exists.")
                }
                scope => format!(
                    "A saved query named “{name}” already exists for {}.",
                    scope_text(scope)
                ),
            });
        }
        Ok(())
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || !self.dirty(cx) {
            return;
        }
        let draft = self.draft(cx);
        if let Err(error) = self.validate(&draft) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        self.saving = true;
        self.error = None;
        let old = self.selected.clone();
        let scope = self.scope.clone();
        let store = store(cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    store.update_named_query(
                        old.as_deref(),
                        &draft.name,
                        &draft.sql,
                        scope,
                        Some(draft.description),
                    )
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(key) => {
                        // Load the stored form (trimmed name and description)
                        this.selected = Some(key);
                        this.refresh(window, cx);
                        if let Some(entry) = this.selected_entry().cloned() {
                            this.load(Some(entry), window, cx);
                        }
                        AppState::global(cx).update(cx, |state, cx| state.storage_changed(cx));
                    }
                    Err(error) => {
                        this.error = Some(error.to_string());
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Start a new query with the editor's content, named "… copy". A
    /// session query is copied to the global scope (sessions are read-only
    /// here).
    fn duplicate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let draft = self.draft(cx);
        let scope = match &self.scope {
            NamedQueryScope::Session(_) => NamedQueryScope::Global,
            scope => scope.clone(),
        };
        let original = draft.name.trim().to_string();
        let name = (1..)
            .map(|n| match n {
                1 => format!("{original} copy"),
                n => format!("{original} copy {n}"),
            })
            .find(|name| {
                let key = NamedQueriesStorage::generate_key(name, &scope);
                !self.entries.iter().any(|entry| entry.key == key)
            })
            .unwrap_or_default();
        self.load(None, window, cx);
        self.scope = scope;
        self.name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.description.update(cx, |input, cx| {
            input.set_value(draft.description, window, cx)
        });
        self.editor
            .update(cx, |editor, cx| editor.set_value(draft.sql, window, cx));
        self.name.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        if self.saving {
            return;
        }
        let view = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            let key = entry.key.clone();
            alert
                .confirm()
                .title(format!("Delete saved query “{}”?", entry.name))
                .description(match &entry.scope {
                    NamedQueryScope::Global => {
                        "It is removed for every connection, in the app and the command line."
                            .to_string()
                    }
                    scope => format!(
                        "It is removed from the {} scope, in the app and the command line.",
                        scope_text(scope)
                    ),
                })
                .ok_text("Delete saved query")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    view.update(cx, |this, cx| this.delete(key.clone(), window, cx));
                    true
                })
        });
    }

    fn delete(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        // The neighbour takes the selection
        let position = self
            .filtered
            .iter()
            .position(|ix| self.entries[*ix].key == key);
        let next = position.and_then(|position| {
            self.filtered
                .get(position + 1)
                .or_else(|| position.checked_sub(1).and_then(|p| self.filtered.get(p)))
                .map(|ix| self.entries[*ix].key.clone())
        });
        self.saving = true;
        let store = store(cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { store.delete_named_query(&key) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(_) => {
                        this.refresh(window, cx);
                        // The deleted query's draft goes with it, edits
                        // included: the editor shows the neighbour as stored
                        // (keeping the draft would make Save rewrite the
                        // neighbour with the deleted query's content)
                        let entry = next
                            .and_then(|key| this.entries.iter().find(|entry| entry.key == key))
                            .or_else(|| this.filtered.first().map(|ix| &this.entries[*ix]))
                            .cloned();
                        let found = entry.is_some();
                        this.load(entry, window, cx);
                        this.editing = found;
                        AppState::global(cx).update(cx, |state, cx| state.storage_changed(cx));
                    }
                    Err(error) => this.error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Open the editor's SQL in a query tab that saves back to this query.
    fn open_in_tab(&self, cx: &mut Context<Self>) {
        if let Some(entry) = self.selected_entry().cloned() {
            let sql = self.draft(cx).sql;
            AppState::global(cx).update(cx, |state, cx| state.open_saved_query(entry, sql, cx));
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let in_list =
            self.focus.is_focused(window) || self.search.focus_handle(cx).is_focused(window);
        if !in_list || event.keystroke.modifiers.modified() {
            return;
        }
        match event.keystroke.key.as_str() {
            "up" => self.step(-1, window, cx),
            "down" => self.step(1, window, cx),
            "enter" if self.selected.is_some() => self.open_in_tab(cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn row(&self, position: usize, cx: &Context<Self>) -> AnyElement {
        let entry = &self.entries[self.filtered[position]];
        let key = entry.key.clone();
        let selected = self.selected.as_ref() == Some(&key);
        let theme = cx.theme();
        let preview = preview(&entry.query);
        let scope = match &entry.scope {
            NamedQueryScope::DatabaseType(engine) => {
                crate::widgets::db_badge_mini(engine, cx).into_any_element()
            }
            NamedQueryScope::Global => div()
                .flex_none()
                .text_size(px(10.))
                .text_color(theme.muted_foreground)
                .child("Global")
                .into_any_element(),
            NamedQueryScope::Session(_) => div()
                .flex_none()
                .text_size(px(10.))
                .text_color(theme.muted_foreground)
                .child("Session")
                .into_any_element(),
        };
        v_flex()
            .id(("saved-query-row", position))
            .h(ROW_HEIGHT)
            .w_full()
            .px_2()
            .justify_center()
            .gap_0p5()
            .rounded(theme.radius)
            .overflow_hidden()
            .cursor_pointer()
            .when(selected, |row| row.bg(theme.list_active))
            .when(!selected, |row| row.hover(|row| row.bg(theme.list_hover)))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .min_w_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_medium()
                            .child(entry.name.clone()),
                    )
                    .child(scope),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground)
                    .child(preview),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.focus.focus(window, cx);
                this.select(Some(key.clone()), window, cx);
            }))
            .into_any_element()
    }

    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let mut filters = vec![ScopeFilter::All, ScopeFilter::Global];
        for entry in self.entries.iter() {
            if let NamedQueryScope::DatabaseType(engine) = &entry.scope {
                let filter = ScopeFilter::Engine(engine.clone());
                if !filters.contains(&filter) {
                    filters.push(filter);
                }
            }
        }
        filters[2..].sort_by_key(ScopeFilter::label);
        if self
            .entries
            .iter()
            .any(|entry| matches!(entry.scope, NamedQueryScope::Session(_)))
        {
            filters.push(ScopeFilter::Sessions);
        }
        let current = self.filter.clone();
        let count = if self.filtered.len() == self.entries.len() {
            match self.entries.len() {
                1 => "1 query".to_string(),
                n => format!("{n} queries"),
            }
        } else {
            format!("{} of {}", self.filtered.len(), self.entries.len())
        };
        let theme = cx.theme();
        let empty = if self.entries.is_empty() {
            Some("No saved queries yet. Create one here, or use Save in a query tab.")
        } else if self.filtered.is_empty() {
            Some("No saved query matches. Clear the search or choose All scopes.")
        } else {
            None
        };
        v_flex()
            .w(LIST_WIDTH)
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(theme.border)
            .child(
                v_flex()
                    .gap_2()
                    .p_2()
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                div().flex_1().child(
                                    Input::new(&self.search)
                                        .small()
                                        .prefix(IconName::Search)
                                        .cleanable(true),
                                ),
                            )
                            .child(
                                Button::new("new-saved-query")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Plus)
                                    .tooltip("New saved query")
                                    .disabled(self.saving)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.select(None, window, cx)
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                Button::new("saved-query-scope-filter")
                                    .ghost()
                                    .xsmall()
                                    .label(current.label())
                                    .dropdown_caret(true)
                                    .dropdown_menu(move |menu, _, _| {
                                        filters.iter().fold(menu, |menu, filter| {
                                            let view = view.clone();
                                            let choice = filter.clone();
                                            menu.item(
                                                PopupMenuItem::new(filter.label())
                                                    .checked(*filter == current)
                                                    .on_click(move |_, _, cx| {
                                                        view.update(cx, |this, cx| {
                                                            this.set_filter(choice.clone(), cx)
                                                        })
                                                    }),
                                            )
                                        })
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(count),
                            ),
                    ),
            )
            .child(
                div()
                    .id("saved-query-list")
                    .track_focus(&self.focus)
                    .flex_1()
                    .min_h_0()
                    .px_1()
                    .pb_1()
                    .map(|list| match empty {
                        Some(text) => list.child(
                            div()
                                .p_2()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(text),
                        ),
                        None => list.child(
                            uniform_list(
                                "saved-query-rows",
                                self.filtered.len(),
                                cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                    range.map(|position| this.row(position, cx)).collect()
                                }),
                            )
                            .track_scroll(&self.scroll)
                            .size_full(),
                        ),
                    }),
            )
    }

    fn render_detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        if !self.editing {
            return v_flex()
                .flex_1()
                .h_full()
                .items_center()
                .justify_center()
                .gap_3()
                .child(div().text_sm().text_color(theme.muted_foreground).child(
                    if self.entries.is_empty() {
                        "Saved queries keep SQL you run often, with $1, $2 parameters."
                    } else {
                        "Select a saved query to edit it."
                    },
                ))
                .child(
                    Button::new("start-saved-query")
                        .outline()
                        .small()
                        .icon(IconName::Plus)
                        .label("New saved query")
                        .on_click(cx.listener(|this, _, window, cx| this.select(None, window, cx))),
                )
                .into_any_element();
        }
        let draft = self.draft(cx);
        let dirty = draft != self.baseline;
        let entry = self.selected_entry();
        let is_new = entry.is_none();
        let created = entry
            .and_then(|entry| entry.created_at)
            .map(|created| format!("Created {}", created.format("%b %-d, %Y")));
        let parameters = parameters(&draft.sql);
        let mono = theme.mono_font_family.clone();
        let muted = theme.muted_foreground;
        let label =
            move |text: &'static str| div().text_xs().font_medium().text_color(muted).child(text);
        let title = if is_new {
            "New saved query".to_string()
        } else {
            self.baseline.name.clone()
        };

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                // Header: what is being edited
                h_flex()
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_base()
                            .font_semibold()
                            .child(title),
                    )
                    .when(dirty && !is_new, |header| {
                        header.child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(muted)
                                .child("· Edited"),
                        )
                    })
                    .child(div().flex_1())
                    .children(created.map(|created| {
                        div().flex_none().text_xs().text_color(muted).child(created)
                    })),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .px_4()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_end()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(label("Name"))
                                    .child(Input::new(&self.name).small().disabled(self.saving)),
                            )
                            .child(
                                v_flex()
                                    .w(px(176.))
                                    .flex_none()
                                    .gap_1()
                                    .child(label("Scope"))
                                    .child(self.scope_control(cx)),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(label("Description"))
                            .child(Input::new(&self.description).small().disabled(self.saving)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h(px(120.))
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(label("SQL"))
                                    .child(div().flex_1())
                                    .when(!parameters.is_empty(), |row| {
                                        row.child(
                                            div()
                                                .mr_1()
                                                .text_xs()
                                                .text_color(muted)
                                                .child("Parameters"),
                                        )
                                        .children(
                                            parameters.into_iter().map(|parameter| {
                                                div()
                                                    .px_1()
                                                    .rounded_sm()
                                                    .bg(cx.theme().muted)
                                                    .text_xs()
                                                    .font_family(mono.clone())
                                                    .child(parameter)
                                            }),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .border_1()
                                    .border_color(cx.theme().input)
                                    .rounded(cx.theme().radius)
                                    .overflow_hidden()
                                    .child(
                                        Editor::new(&self.editor)
                                            .bordered(false)
                                            .size_full()
                                            .text_size(preferences::editor_font_size(cx))
                                            .disabled(self.saving),
                                    ),
                            ),
                    ),
            )
            .child(self.render_footer(dirty, is_new, cx))
            .into_any_element()
    }

    /// Save (the one primary action), then what else can be done with it.
    fn render_footer(&self, dirty: bool, is_new: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let danger = cx.theme().danger;
        h_flex()
            .flex_none()
            .px_4()
            .py_3()
            .gap_2()
            .items_center()
            .child(
                Button::new("save-saved-query")
                    .primary()
                    .small()
                    .label(if self.saving {
                        "Saving…"
                    } else if is_new {
                        "Save query"
                    } else {
                        "Save changes"
                    })
                    .tooltip(if cfg!(target_os = "macos") {
                        "Save · ⌘S"
                    } else {
                        "Save · Ctrl+S"
                    })
                    .disabled(!dirty || self.saving)
                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
            )
            .when(!is_new, |footer| {
                footer
                    .child(
                        Button::new("open-saved-query")
                            .outline()
                            .small()
                            .label("Open in new tab")
                            .disabled(self.saving)
                            .on_click(cx.listener(|this, _, _, cx| this.open_in_tab(cx))),
                    )
                    .child(
                        Button::new("duplicate-saved-query")
                            .ghost()
                            .small()
                            .label("Duplicate")
                            .disabled(self.saving)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.duplicate(window, cx)),
                            ),
                    )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .text_color(danger)
                    .children(self.error.clone()),
            )
            .when(!is_new, |footer| {
                footer.child(
                    Button::new("delete-saved-query")
                        .ghost()
                        .small()
                        .icon(IconName::Trash)
                        .label("Delete…")
                        .disabled(self.saving)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.confirm_delete(window, cx)),
                        ),
                )
            })
            .when(is_new, |footer| {
                footer.child(
                    Button::new("cancel-saved-query")
                        .ghost()
                        .small()
                        .label("Cancel")
                        .disabled(self.saving)
                        .on_click(cx.listener(|this, _, window, cx| {
                            let first = this
                                .filtered
                                .first()
                                .map(|ix| this.entries[*ix].key.clone());
                            match first {
                                Some(key) => this.select(Some(key), window, cx),
                                None => {
                                    this.editing = false;
                                    cx.notify();
                                }
                            }
                        })),
                )
            })
    }

    /// Global or an engine; a session scope is shown, not changed.
    fn scope_control(&self, cx: &mut Context<Self>) -> AnyElement {
        if let NamedQueryScope::Session(_) = &self.scope {
            return div()
                .h(px(28.))
                .flex()
                .items_center()
                .min_w_0()
                .truncate()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(scope_text(&self.scope))
                .into_any_element();
        }
        let view = cx.entity();
        let current = self.scope.clone();
        Button::new("saved-query-scope")
            .outline()
            .small()
            .w_full()
            .label(scope_choice(&self.scope))
            .dropdown_caret(true)
            .disabled(self.saving)
            .dropdown_menu(move |menu, _, _| {
                let scopes = std::iter::once(NamedQueryScope::Global).chain(
                    DatabaseType::supported_types()
                        .into_iter()
                        .map(NamedQueryScope::DatabaseType),
                );
                scopes.fold(menu, |menu, scope| {
                    let view = view.clone();
                    let global = scope == NamedQueryScope::Global;
                    let menu = menu.item(
                        PopupMenuItem::new(scope_choice(&scope))
                            .checked(scope == current)
                            .on_click(move |_, _, cx| {
                                view.update(cx, |this, cx| {
                                    this.scope = scope.clone();
                                    this.error = None;
                                    cx.notify();
                                })
                            }),
                    );
                    if global { menu.separator() } else { menu }
                })
            })
            .into_any_element()
    }
}

/// A scope as offered in the Scope menu.
fn scope_choice(scope: &NamedQueryScope) -> String {
    match scope {
        NamedQueryScope::Global => "Global · every connection".into(),
        scope => scope_text(scope),
    }
}

impl Render for SavedQueriesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .id("saved-queries-manager")
            .key_context("SavedQueries")
            .size_full()
            .items_start()
            .on_action(cx.listener(|this, _: &SaveNamedQuery, window, cx| this.save(window, cx)))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(self.render_list(cx))
            .child(self.render_detail(cx))
    }
}

/// One line of SQL for the list: whitespace collapsed, leading `--`
/// comment lines skipped (unless the query is only comments).
fn preview(sql: &str) -> String {
    let code: Vec<&str> = sql
        .lines()
        .skip_while(|line| {
            let line = line.trim();
            line.is_empty() || line.starts_with("--")
        })
        .collect();
    let text = if code.is_empty() {
        sql.to_string()
    } else {
        code.join(" ")
    };
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}

/// The parameters a saved query takes (`src/named_queries.rs`): `$1`, `$2`…
/// in numeric order, then `$*` and `$@`.
fn parameters(sql: &str) -> Vec<String> {
    let mut positional = std::collections::BTreeSet::new();
    let mut star = false;
    let mut at = false;
    let mut chars = sql.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '$' {
            continue;
        }
        match chars.peek() {
            Some('*') => star = true,
            Some('@') => at = true,
            Some('1'..='9') => {
                let mut digits = String::new();
                while let Some(digit) = chars.peek().copied().filter(char::is_ascii_digit) {
                    digits.push(digit);
                    chars.next();
                }
                if let Ok(n) = digits.parse::<u32>() {
                    positional.insert(n);
                }
            }
            _ => {}
        }
    }
    positional
        .into_iter()
        .map(|n| format!("${n}"))
        .chain(star.then(|| "$*".to_string()))
        .chain(at.then(|| "$@".to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::tests::{open, settle};
    use gpui_kit::test::TestWindowExt as _;
    use std::prelude::v1::test;

    fn seeded() -> ConfigStore {
        crate::test_support::isolate();
        let store = ConfigStore::from_config(Config::default_settings());
        for (name, sql, scope) in [
            ("alpha", "SELECT $1", NamedQueryScope::Global),
            (
                "beta",
                "SELECT 2",
                NamedQueryScope::DatabaseType(DatabaseType::PostgreSQL),
            ),
            (
                "gamma",
                "SELECT 3",
                NamedQueryScope::Session("warehouse".into()),
            ),
        ] {
            store
                .update_named_query(None, name, sql, scope, Some(format!("{name} description")))
                .unwrap();
        }
        store
    }

    fn manager(
        cx: &mut TestAppContext,
        store: ConfigStore,
    ) -> (AnyWindowHandle, Entity<SavedQueriesView>) {
        let (window, _, settings) = open(cx, store, SettingsPage::SavedQueries);
        let manager = cx.update(|cx| settings.read(cx).saved_queries.clone());
        (window, manager)
    }

    #[gpui_kit::test]
    fn lists_every_scope_and_selects_the_first_query(cx: &mut TestAppContext) {
        let (_, manager) = manager(cx, seeded());
        cx.update(|cx| {
            let manager = manager.read(cx);
            assert_eq!(manager.entries.len(), 3);
            assert_eq!(manager.filtered.len(), 3);
            assert_eq!(manager.selected.as_deref(), Some("global::alpha"));
            assert!(!manager.dirty(cx));
        });
    }

    #[gpui_kit::test]
    fn renames_and_edits_with_the_keyboard(cx: &mut TestAppContext) {
        let store = seeded();
        let before = store.saved_query("global::alpha").unwrap();
        let (window, manager) = manager(cx, store.clone());
        cx.update_window(window, |_, window, cx| {
            manager.update(cx, |this, cx| {
                this.name
                    .update(cx, |input, cx| input.set_value("renamed", window, cx));
                this.description.update(cx, |input, cx| {
                    input.set_value("Updated description", window, cx)
                });
                this.editor.update(cx, |editor, cx| {
                    editor.set_value("SELECT $2", window, cx);
                    editor.focus(window, cx);
                });
                assert!(this.dirty(cx));
            });
            window.render_frame(cx);
            window.press(
                if cfg!(target_os = "macos") {
                    "cmd-s"
                } else {
                    "ctrl-s"
                },
                cx,
            );
        })
        .unwrap();
        settle(cx, |cx| !manager.read(cx).saving);
        let saved = store.saved_query("global::renamed").expect("renamed query");
        assert!(store.saved_query("global::alpha").is_none());
        assert_eq!(saved.query, "SELECT $2");
        assert_eq!(saved.description.as_deref(), Some("Updated description"));
        assert_eq!(saved.created_at, before.created_at);
        cx.update(|cx| {
            let manager = manager.read(cx);
            assert_eq!(manager.selected.as_deref(), Some("global::renamed"));
            assert!(!manager.dirty(cx));
        });
    }

    #[gpui_kit::test]
    fn moving_to_another_scope_refuses_to_overwrite(cx: &mut TestAppContext) {
        let store = seeded();
        store
            .update_named_query(
                None,
                "alpha",
                "SELECT 'pg'",
                NamedQueryScope::DatabaseType(DatabaseType::PostgreSQL),
                None,
            )
            .unwrap();
        let (window, manager) = manager(cx, store.clone());
        cx.update_window(window, |_, window, cx| {
            manager.update(cx, |this, cx| {
                this.select(Some("global::alpha".into()), window, cx);
                this.scope = NamedQueryScope::DatabaseType(DatabaseType::PostgreSQL);
                this.save(window, cx);
                assert_eq!(
                    this.error.as_deref(),
                    Some("A saved query named “alpha” already exists for PostgreSQL.")
                );
                assert!(!this.saving);
            })
        })
        .unwrap();
        assert_eq!(
            store.saved_query("global::alpha").unwrap().query,
            "SELECT $1"
        );
    }

    #[gpui_kit::test]
    fn searches_descriptions_and_filters_by_scope(cx: &mut TestAppContext) {
        let (window, manager) = manager(cx, seeded());
        cx.update_window(window, |_, window, cx| {
            manager.update(cx, |this, cx| {
                this.search.update(cx, |input, cx| {
                    input.set_value("BETA description", window, cx)
                });
                this.rebuild(cx);
                assert_eq!(this.filtered.len(), 1);
                assert_eq!(this.entries[this.filtered[0]].name, "beta");
                this.search
                    .update(cx, |input, cx| input.set_value("", window, cx));
                this.set_filter(ScopeFilter::Sessions, cx);
                assert_eq!(this.filtered.len(), 1);
                assert_eq!(this.entries[this.filtered[0]].name, "gamma");
                // A new query starts in the engine the list is filtered on
                this.set_filter(ScopeFilter::Engine(DatabaseType::PostgreSQL), cx);
                this.select(None, window, cx);
                assert_eq!(
                    this.scope,
                    NamedQueryScope::DatabaseType(DatabaseType::PostgreSQL)
                );
            })
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn switching_with_unsaved_edits_asks_and_delete_confirms(cx: &mut TestAppContext) {
        let store = seeded();
        let (window, manager) = manager(cx, store.clone());
        let beta = "postgresql::beta".to_string();
        assert!(store.saved_query(&beta).is_some());
        // Unsaved edits: switching asks, Escape keeps the edits
        cx.update_window(window, |_, window, cx| {
            manager.update(cx, |this, cx| {
                this.name
                    .update(cx, |input, cx| input.set_value("unsaved", window, cx));
                this.select(Some(beta.clone()), window, cx);
            });
            window.render_frame(cx);
            window.press("escape", cx);
        })
        .unwrap();
        cx.update(|cx| {
            let manager = manager.read(cx);
            assert_eq!(manager.selected.as_deref(), Some("global::alpha"));
            assert_eq!(manager.name.read(cx).value().as_ref(), "unsaved");
        });
        // Confirming discards them
        cx.update_window(window, |_, window, cx| {
            manager.update(cx, |this, cx| this.select(Some(beta.clone()), window, cx));
            window.render_frame(cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.update(|cx| assert_eq!(manager.read(cx).selected.as_ref(), Some(&beta)));
        // Delete asks first, names the query, then removes it
        cx.update_window(window, |_, window, cx| {
            manager.update(cx, |this, cx| this.confirm_delete(window, cx));
            window.render_frame(cx);
        })
        .unwrap();
        assert!(
            store.saved_query(&beta).is_some(),
            "the dialog alone keeps it"
        );
        cx.update_window(window, |_, window, cx| window.press("enter", cx))
            .unwrap();
        settle(cx, |cx| !manager.read(cx).saving);
        assert!(store.saved_query(&beta).is_none());
        assert!(store.saved_query("global::alpha").is_some());
        // The neighbour is selected
        cx.update(|cx| assert!(manager.read(cx).selected_entry().is_some()));
    }

    #[gpui_kit::test]
    fn deleting_an_edited_query_discards_its_draft(cx: &mut TestAppContext) {
        let store = seeded();
        let (window, manager) = manager(cx, store.clone());
        cx.update_window(window, |_, window, cx| {
            manager.update(cx, |this, cx| {
                assert_eq!(this.selected.as_deref(), Some("global::alpha"));
                this.editor.update(cx, |editor, cx| {
                    editor.set_value("SELECT 'edited alpha'", window, cx)
                });
                assert!(this.dirty(cx));
                this.delete("global::alpha".into(), window, cx);
            })
        })
        .unwrap();
        settle(cx, |cx| !manager.read(cx).saving);
        cx.update(|cx| {
            let manager = manager.read(cx);
            assert_eq!(manager.selected.as_deref(), Some("postgresql::beta"));
            assert!(!manager.dirty(cx), "the deleted query's edits are gone");
            assert_eq!(manager.editor.read(cx).value().as_ref(), "SELECT 2");
            assert_eq!(manager.name.read(cx).value().as_ref(), "beta");
        });
        // Nothing to save, and the neighbour is untouched
        cx.update_window(window, |_, window, cx| {
            manager.update(cx, |this, cx| this.save(window, cx))
        })
        .unwrap();
        settle(cx, |cx| !manager.read(cx).saving);
        assert_eq!(
            store.saved_query("postgresql::beta").unwrap().query,
            "SELECT 2"
        );
        assert!(store.saved_query("global::alpha").is_none());
    }

    #[gpui_kit::test]
    fn duplicate_starts_a_new_unsaved_copy(cx: &mut TestAppContext) {
        let store = seeded();
        let (window, manager) = manager(cx, store.clone());
        cx.update_window(window, |_, window, cx| {
            manager.update(cx, |this, cx| {
                this.duplicate(window, cx);
                assert!(this.selected.is_none());
                assert_eq!(this.name.read(cx).value().as_ref(), "alpha copy");
                assert!(this.dirty(cx));
                this.save(window, cx);
            })
        })
        .unwrap();
        settle(cx, |cx| !manager.read(cx).saving);
        assert_eq!(
            store.saved_query("global::alpha copy").unwrap().query,
            "SELECT $1"
        );
    }

    #[test]
    fn previews_skip_leading_comments() {
        assert_eq!(
            preview("-- Recent\n-- customers\nSELECT *\n  FROM t"),
            "SELECT * FROM t"
        );
        assert_eq!(preview("-- only a note"), "-- only a note");
    }

    #[test]
    fn parameters_are_ordered_and_deduplicated() {
        assert_eq!(
            parameters("SELECT $10, $2, $1, $2, $*, $@, $0, price$"),
            ["$1", "$2", "$10", "$*", "$@"]
        );
        assert!(parameters("SELECT 1").is_empty());
    }
}
