//! Small dialogs opened from the explorer's context menus.

use crate::app_state::AppState;
use dbcrust_gui_core::ConnectionId;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    notification::Notification,
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::*;

/// Pick one of the server's databases and open it as a new connection.
pub(super) fn open_database_picker(
    connection: ConnectionId,
    databases: Vec<String>,
    window: &mut Window,
    cx: &mut App,
) {
    let picker = cx.new(|cx| DatabasePicker::new(connection, databases, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Open database")
            .w(px(420.))
            .child(picker.clone())
    });
}

/// Name the connection and save it as a session.
pub(super) fn open_session_dialog(connection: ConnectionId, window: &mut Window, cx: &mut App) {
    let form = cx.new(|cx| SessionForm::new(connection, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Save as session")
            .w(px(400.))
            .child(form.clone())
    });
}

struct DatabasePicker {
    connection: ConnectionId,
    databases: Vec<String>,
    current: String,
    filter: Entity<InputState>,
    filter_text: SharedString,
    _subscription: Subscription,
}

impl DatabasePicker {
    /// More rows than this are never useful to scroll; the filter narrows.
    const VISIBLE: usize = 200;

    fn new(
        connection: ConnectionId,
        databases: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let current = AppState::global(cx)
            .read(cx)
            .connection(connection)
            .map(|live| live.summary.database.clone())
            .unwrap_or_default();
        let filter = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter databases")
                .clean_on_escape()
        });
        filter.update(cx, |input, cx| input.focus(window, cx));
        let subscription = cx.subscribe(&filter, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.filter_text = input.read(cx).value();
                cx.notify();
            }
        });
        Self {
            connection,
            databases,
            current,
            filter,
            filter_text: SharedString::default(),
            _subscription: subscription,
        }
    }

    fn open(&mut self, database: String, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        // A failure is announced through `AppEvent::ConnectFailed`
        AppState::global(cx)
            .update(cx, |state, cx| {
                state.open_database(self.connection, database, cx)
            })
            .detach();
    }
}

impl Render for DatabasePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let needle = self.filter_text.trim().to_lowercase();
        let matches: Vec<&String> = self
            .databases
            .iter()
            .filter(|name| needle.is_empty() || name.to_lowercase().contains(&needle))
            .collect();
        let hidden = matches.len().saturating_sub(Self::VISIBLE);
        let muted = cx.theme().muted_foreground;

        v_flex()
            .gap_2()
            .child(Input::new(&self.filter).small().cleanable(true))
            .child(
                v_flex()
                    .id("database-list")
                    .max_h(px(320.))
                    .overflow_y_scrollbar()
                    .children(matches.iter().take(Self::VISIBLE).map(|name| {
                        let is_current = **name == self.current;
                        let database = (*name).clone();
                        Button::new(SharedString::from(format!("database:{name}")))
                            .ghost()
                            .small()
                            .w_full()
                            .justify_start()
                            .label(if is_current {
                                format!("{name}  (current)")
                            } else {
                                (*name).clone()
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open(database.clone(), window, cx);
                            }))
                    })),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(muted)
                    .child(if matches.is_empty() {
                        "No database matches".to_string()
                    } else if hidden > 0 {
                        format!("{hidden} more — type to narrow the list")
                    } else {
                        format!("{} databases on this server", self.databases.len())
                    }),
            )
    }
}

struct SessionForm {
    connection: ConnectionId,
    name: Entity<InputState>,
    error: Option<SharedString>,
    saving: bool,
    _subscription: Subscription,
}

impl SessionForm {
    fn new(connection: ConnectionId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let suggestion = AppState::global(cx)
            .read(cx)
            .connection(connection)
            .map(|live| live.summary.label.clone())
            .unwrap_or_default();
        let name = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Session name");
            input.set_value(suggestion, window, cx);
            input
        });
        name.update(cx, |input, cx| input.focus(window, cx));
        let subscription =
            cx.subscribe_in(&name, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.submit(window, cx);
                }
            });
        Self {
            connection,
            name,
            error: None,
            saving: false,
            _subscription: subscription,
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let name = self.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.error = Some("Enter a name for the session".into());
            cx.notify();
            return;
        }
        self.saving = true;
        self.error = None;
        cx.notify();
        let connection = self.connection;
        let saving = AppState::global(cx).update(cx, |state, cx| {
            state.save_session(connection, name.clone(), cx)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = saving.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    window.close_dialog(cx);
                    window.push_notification(
                        Notification::success(format!("Saved session “{name}”")),
                        cx,
                    );
                }
                Err(error) => {
                    this.saving = false;
                    this.error = Some(error.to_string().into());
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

impl Render for SessionForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let danger = cx.theme().danger;
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_2()
            .child(Input::new(&self.name))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(muted)
                    .child("Saved sessions are listed on the home page."),
            )
            .children(
                self.error
                    .clone()
                    .map(|error| div().text_size(px(12.)).text_color(danger).child(error)),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("session-cancel")
                            .label("Cancel")
                            .outline()
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("session-save")
                            .primary()
                            .label("Save session")
                            .loading(self.saving)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
