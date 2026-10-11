//! Dialogs opened by workspace commands.

use super::Workspace;
use crate::app_state::AppState;
use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _,
    dialog::DialogButtonProps,
    input::{Input, InputState},
    notification::Notification,
    v_flex,
};
use gpui_kit::*;

impl Workspace {
    /// "Save connection as session…": a name, then `AppState::save_session`.
    pub(super) fn open_save_session_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = {
            let state = AppState::global(cx);
            let state = state.read(cx);
            state.active().map(|live| {
                let suggestion = live
                    .summary
                    .session_name
                    .clone()
                    .unwrap_or_else(|| live.summary.label.clone());
                (live.id(), suggestion, live.summary.target())
            })
        };
        let Some((connection, suggestion, endpoint)) = target else {
            window.push_notification(Notification::info("Connect to a database first"), cx);
            return;
        };

        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Session name")
                .default_value(suggestion)
        });
        let focus_on_open = std::rc::Rc::new(std::cell::Cell::new(true));
        window.open_dialog(cx, move |dialog, _, _| {
            let input = input.clone();
            let ok_input = input.clone();
            let focus_on_open = focus_on_open.clone();
            let endpoint = endpoint.clone();
            dialog
                .title("Save connection as session")
                .width(px(420.))
                .content(move |content, window, cx| {
                    if focus_on_open.replace(false) {
                        let input = input.clone();
                        window.defer(cx, move |window, cx| {
                            input.update(cx, |input, cx| input.focus(window, cx));
                        });
                    }
                    content.child(
                        v_flex().gap_2().child(Input::new(&input)).child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "Reconnect to {endpoint} from Home, without the password."
                                )),
                        ),
                    )
                })
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Save")
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    let name = ok_input.read(cx).value().trim().to_string();
                    if name.is_empty() {
                        window.push_notification(
                            Notification::warning("Give the session a name."),
                            cx,
                        );
                        return false;
                    }
                    let saving = AppState::global(cx).update(cx, |state, cx| {
                        state.save_session(connection, name.clone(), cx)
                    });
                    window
                        .spawn(cx, async move |cx| {
                            let outcome = saving.await;
                            cx.update(|window, cx| match outcome {
                                Ok(()) => window.push_notification(
                                    Notification::success(format!("Saved session “{name}”")),
                                    cx,
                                ),
                                Err(error) => window.push_notification(
                                    Notification::error(error.to_string())
                                        .title("Couldn’t save the session"),
                                    cx,
                                ),
                            })
                            .ok();
                        })
                        .detach();
                    true
                })
        });
    }
}
