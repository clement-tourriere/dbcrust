//! Every copy of the app writes the clipboard through [`write`]. A copy whose
//! text is rendered in the background (a whole result set, a column) takes a
//! [`Ticket`] when it starts and lands only if nothing was copied since: the
//! clipboard always ends with the copy the user asked for last, whatever
//! finishes first.
//!
//! "Nothing was copied since" is known without ever reading the clipboard
//! (a read can be large, and slow on some platforms): every copy that may
//! happen meanwhile counts.
//! - The app's own copies go through [`write`] (or report themselves with
//!   [`written_elsewhere`], like GPUI Kit's `Clipboard` button).
//! - GPUI Kit's text inputs, the SQL editor and selectable text copy on the
//!   `Copy` and `Cut` actions: [`track_copy_actions`] counts them at the root
//!   of the window, before the focused widget handles them, however they are
//!   dispatched (shortcut, context menu, touch menu); [`observe_copy_keys`]
//!   counts the shortcuts in every window, layers (dialogs, sheets) included.
//! - Other apps: copying there means leaving the window first, and a window
//!   that loses focus counts as a copy ([`track_window_activation`]).

use gpui_kit::component::input::{Copy, Cut};
use gpui_kit::*;

/// Counts the copies asked for.
#[derive(Default)]
struct Generation(u64);

impl Global for Generation {}

fn next(cx: &mut App) -> u64 {
    let generation = cx.default_global::<Generation>();
    generation.0 += 1;
    generation.0
}

/// Put `text` on the clipboard now. A background copy started earlier will
/// not replace it.
pub fn write(text: String, cx: &mut App) {
    next(cx);
    cx.write_to_clipboard(ClipboardItem::new_string(text));
}

/// The clipboard is written (or may be) by something else than [`write`]: a
/// copy started before must not replace it.
pub fn written_elsewhere(cx: &mut App) {
    next(cx);
}

/// A copy asked for now whose text is not ready yet.
#[must_use = "a ticket writes nothing until `Ticket::write`"]
pub struct Ticket(u64);

/// Start a copy that will write later: it supersedes every copy started
/// before it, and is superseded by any copy made after it.
pub fn reserve(cx: &mut App) -> Ticket {
    Ticket(next(cx))
}

impl Ticket {
    /// Nothing was copied since this copy was asked for.
    pub fn is_current(&self, cx: &App) -> bool {
        cx.try_global::<Generation>()
            .is_some_and(|generation| generation.0 == self.0)
    }

    /// Put `text` on the clipboard unless something was copied meanwhile;
    /// true when written.
    pub fn write(self, text: String, cx: &mut App) -> bool {
        if !self.is_current(cx) {
            return false;
        }
        write(text, cx);
        true
    }
}

/// Count the `Copy` and `Cut` actions dispatched inside `root` (the root
/// element of a window's content) as copies, in the capture phase: before the
/// widget that handles them writes the clipboard, and whatever dispatched
/// them (a shortcut, a context menu, a touch edit menu). They go on to the
/// widget unchanged.
pub fn track_copy_actions<E: InteractiveElement>(root: E) -> E {
    root.capture_action(|_: &Copy, _, cx| written_elsewhere(cx))
        .capture_action(|_: &Cut, _, cx| written_elsewhere(cx))
}

/// Count the copy and cut shortcuts as copies in every window, including
/// the layers above a window's content (dialogs, sheets), which
/// [`track_copy_actions`] does not reach: a keystroke that resolved to
/// `Copy` or `Cut`, or a copy or cut shortcut that resolved to no action at
/// all (a widget may copy on the keys directly). A shortcut bound to another
/// action is not counted: that is the app's own command (the results grid's
/// "Copy selection" is on the same keys), and its copies count themselves
/// through [`write`] and [`reserve`]; counting the keystroke after it would
/// cancel the copy it just started. Call once at startup.
pub fn observe_copy_keys(cx: &mut App) {
    cx.observe_keystrokes(|event, _, cx| {
        let copies = match &event.action {
            Some(action) => action.as_any().is::<Copy>() || action.as_any().is::<Cut>(),
            None => is_copy_shortcut(&event.keystroke),
        };
        if copies {
            written_elsewhere(cx);
        }
    })
    .detach();
}

fn is_copy_shortcut(keystroke: &Keystroke) -> bool {
    keystroke.modifiers.secondary()
        && !keystroke.modifiers.alt
        && matches!(keystroke.key.as_str(), "c" | "x")
}

/// Count `window` losing focus as a copy: copying in another app means
/// leaving it first, so a background copy started before must not replace
/// what was copied there.
pub fn track_window_activation<V: 'static>(
    window: &mut Window,
    cx: &mut Context<V>,
) -> Subscription {
    cx.observe_window_activation(window, |_, window, cx| {
        if !window.is_window_active() {
            written_elsewhere(cx);
        }
    })
}

#[cfg(test)]
mod tests {
    // Explicit imports: a glob of `gpui_kit` would shadow `#[test]`
    use super::{reserve, write};
    use crate::RestoredState;
    use dbcrust_gui_core::ConfigStore;
    use dbcrust_gui_core::dbcrust_app::config::Config;
    use dbcrust_gui_core::gui_state::GuiState;
    use gpui_kit::{AppContext as _, TestAppContext, VisualTestContext, WindowOptions};

    fn clipboard(cx: &mut TestAppContext) -> Option<String> {
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
    }

    #[gpui_kit::test]
    fn the_last_copy_asked_for_wins(cx: &mut TestAppContext) {
        // A background copy, then an immediate one: the background one is dropped
        let bulk = cx.update(reserve);
        cx.update(|cx| write("cell".into(), cx));
        assert!(!cx.update(|cx| bulk.write("bulk".into(), cx)));
        assert_eq!(clipboard(cx).as_deref(), Some("cell"));

        // Two background copies finishing out of order: only the newer lands
        let first = cx.update(reserve);
        let second = cx.update(reserve);
        assert!(cx.update(|cx| second.write("second".into(), cx)));
        assert!(!cx.update(|cx| first.write("first".into(), cx)));
        assert_eq!(clipboard(cx).as_deref(), Some("second"));

        // Nothing newer: it lands
        let alone = cx.update(reserve);
        assert!(cx.update(|cx| alone.write("alone".into(), cx)));
        assert_eq!(clipboard(cx).as_deref(), Some("alone"));
    }

    #[gpui_kit::test]
    fn leaving_the_window_counts_as_a_copy(cx: &mut TestAppContext) {
        crate::test_support::isolate();
        let (window, _) = cx.update(|cx| {
            crate::init(ConfigStore::from_config(Config::default()), cx);
            cx.set_global(RestoredState(GuiState::default()));
            crate::open_main_window(WindowOptions::default(), cx).expect("open the main window")
        });
        cx.update_window(window, |_, window, _| window.activate_window())
            .expect("activate");
        cx.run_until_parked();

        // Copying in another app means leaving the window first
        let bulk = cx.update(reserve);
        VisualTestContext::from_window(window, cx).deactivate_window();
        assert!(!cx.update(|cx| bulk.is_current(cx)));
        assert!(!cx.update(|cx| bulk.write("bulk".into(), cx)));

        // A copy started after coming back is not affected
        cx.update_window(window, |_, window, _| window.activate_window())
            .expect("activate");
        cx.run_until_parked();
        let later = cx.update(reserve);
        assert!(cx.update(|cx| later.write("later".into(), cx)));
        assert_eq!(clipboard(cx).as_deref(), Some("later"));
    }
}
