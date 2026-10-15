//! Settings → SSH tunnels: the `ssh_tunnel_patterns` of `config.toml`
//! (host regex → `[user[:password]@]host[:port]`), and a host tester.
//!
//! Targets in backticks run a command when connecting. Nothing here ever
//! runs one: validation accepts them as they are, and the list and the
//! tester say "runs a command when connecting" instead of a target.

use super::storage::{Edit, cache, enqueue};
use super::*;
use dbcrust_gui_core::dbcrust::password_sanitizer::sanitize_ssh_tunnel_string;
use gpui_kit::component::{
    button::ButtonVariant,
    input::{Input, InputEvent, InputState},
};

pub(super) const DUPLICATE_PATTERN: &str =
    "A tunnel for this host pattern already exists. Edit it instead.";

/// How a target is shown: passwords masked, commands not shown at all.
fn display_target(target: &str) -> String {
    if target.contains('`') {
        "runs a command when connecting".into()
    } else {
        sanitize_ssh_tunnel_string(target)
    }
}

/// `regex` errors span several lines (the pattern, a caret, the reason):
/// keep the reason.
fn pattern_error(error: String) -> String {
    match error.lines().rev().find(|line| !line.trim().is_empty()) {
        Some(reason) if error.contains('\n') => format!(
            "Not a valid regular expression: {}",
            reason.trim().trim_start_matches("error: ")
        ),
        _ => {
            let mut chars = error.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

/// The patterns, sorted, with the add / edit form.
pub(super) struct TunnelsView {
    pattern: Entity<InputState>,
    target: Entity<InputState>,
    /// The form is open; `Some(pattern)` edits that pattern.
    form: Option<Option<String>>,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl TunnelsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pattern = cx.new(|cx| InputState::new(window, cx).placeholder(r"^db\.internal\..*$"));
        let target =
            cx.new(|cx| InputState::new(window, cx).placeholder("deploy@bastion.example.com:22"));
        let mut subscriptions = Vec::new();
        for input in [&pattern, &target] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        this.error = None;
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.save(window, cx),
                    _ => {}
                },
            ));
        }
        subscriptions.push(cx.observe_global::<SettingsCache>(|_, cx| cx.notify()));
        Self {
            pattern,
            target,
            form: None,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn open_form(&mut self, pattern: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let target = pattern
            .as_ref()
            .and_then(|pattern| cache(cx).config.ssh_tunnel_patterns.get(pattern))
            .cloned()
            .unwrap_or_default();
        // A stored password stays hidden until revealed
        let masked = !target.contains('`') && sanitize_ssh_tunnel_string(&target) != target;
        self.pattern.update(cx, |input, cx| {
            input.set_value(pattern.clone().unwrap_or_default(), window, cx);
            input.focus(window, cx);
        });
        self.target.update(cx, |input, cx| {
            input.set_masked(masked, window, cx);
            input.set_value(target, window, cx);
        });
        self.form = Some(pattern);
        self.error = None;
        cx.notify();
    }

    fn close_form(&mut self, cx: &mut Context<Self>) {
        self.form = None;
        self.error = None;
        cx.notify();
    }

    /// Live validation of the form: (pattern error, target error), each
    /// shown once its box has text.
    fn validation(&self, cx: &App) -> (Option<String>, Option<String>) {
        let pattern = self.pattern.read(cx).value();
        let target = self.target.read(cx).value();
        let config = &cache(cx).config;
        let editing = self.form.clone().flatten();
        let pattern_error = if pattern.trim().is_empty() {
            None
        } else {
            config_editor::validate_tunnel_pattern(&pattern)
                .err()
                .map(pattern_error)
                .or_else(|| {
                    let pattern = pattern.trim();
                    (editing.as_deref() != Some(pattern)
                        && config.ssh_tunnel_patterns.contains_key(pattern))
                    .then(|| DUPLICATE_PATTERN.to_string())
                })
        };
        let target_error = if target.trim().is_empty() {
            None
        } else {
            config_editor::validate_tunnel_target(config, &target)
                .err()
                .map(|_| "Use [user[:password]@]host[:port], e.g. deploy@bastion:22.".to_string())
        };
        (pattern_error, target_error)
    }

    fn can_save(&self, cx: &App) -> bool {
        let filled = !self.pattern.read(cx).value().trim().is_empty()
            && !self.target.read(cx).value().trim().is_empty();
        filled && self.validation(cx) == (None, None)
    }

    fn save(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(old) = self.form.clone() else {
            return;
        };
        if !self.can_save(cx) {
            cx.notify();
            return;
        }
        let edit = Edit::Tunnel {
            old,
            pattern: self.pattern.read(cx).value().to_string(),
            target: self.target.read(cx).value().to_string(),
        };
        match enqueue(edit, cx) {
            Ok(()) => self.close_form(cx),
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }

    fn confirm_delete(&self, pattern: String, window: &mut Window, cx: &mut Context<Self>) {
        window.open_alert_dialog(cx, move |alert, _, _| {
            let pattern = pattern.clone();
            alert
                .confirm()
                .title(format!("Delete the tunnel for “{pattern}”?"))
                .description("Hosts matching this pattern will connect directly.")
                .ok_text("Delete tunnel")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, _, cx| {
                    if let Err(error) = enqueue(Edit::DeleteTunnel(pattern.clone()), cx) {
                        notify(cx, Notification::error(error));
                    }
                    true
                })
        });
    }

    fn render_form(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (pattern_error, target_error) = self.validation(cx);
        let target = self.target.read(cx).value();
        let runs_command = target.contains('`');
        let has_password = !runs_command && sanitize_ssh_tunnel_string(&target) != target.as_ref();
        let can_save = self.can_save(cx);
        let editing = self.form.clone().flatten();
        let theme = cx.theme();
        let hint = |text: &'static str, color: Hsla| div().text_xs().text_color(color).child(text);
        let field = |label: &'static str| {
            div()
                .text_xs()
                .font_medium()
                .text_color(theme.muted_foreground)
                .child(label)
        };
        v_flex()
            .id("tunnel-form")
            .gap_3()
            .p_3()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .child(div().text_sm().font_medium().child(match &editing {
                Some(_) => "Edit tunnel",
                None => "New tunnel",
            }))
            .child(
                v_flex()
                    .gap_1()
                    .child(field("Host pattern (regular expression)"))
                    .child(
                        Input::new(&self.pattern)
                            .small()
                            .font_family(theme.mono_font_family.clone()),
                    )
                    .children(
                        pattern_error
                            .clone()
                            .map(|error| div().text_xs().text_color(theme.danger).child(error)),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(field("SSH target"))
                    .child(
                        Input::new(&self.target)
                            .small()
                            .font_family(theme.mono_font_family.clone())
                            .mask_toggle(),
                    )
                    .map(|this| match (&target_error, runs_command, has_password) {
                        (Some(error), _, _) => this.child(
                            div()
                                .text_xs()
                                .text_color(theme.danger)
                                .child(error.clone()),
                        ),
                        (None, true, _) => this.child(hint(
                            "The command in backticks runs each time a matching host connects. \
                             It is never run here.",
                            theme.muted_foreground,
                        )),
                        (None, false, true) => this.child(hint(
                            "This password is saved in plain text in config.toml. Prefer an SSH \
                             key or ssh-agent.",
                            theme.warning,
                        )),
                        (None, false, false) => this.child(hint(
                            "[user@]host[:port] of the SSH server to go through. Backticks run \
                             a command at connect time.",
                            theme.muted_foreground,
                        )),
                    }),
            )
            .children(
                self.error
                    .clone()
                    .map(|error| div().text_xs().text_color(theme.danger).child(error)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("save-tunnel")
                            .primary()
                            .small()
                            .label(if editing.is_some() {
                                "Save tunnel"
                            } else {
                                "Add tunnel"
                            })
                            .disabled(!can_save)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .child(
                        Button::new("cancel-tunnel")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.close_form(cx))),
                    ),
            )
    }

    fn render_row(&self, pattern: &str, target: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let edit = pattern.to_string();
        let delete = pattern.to_string();
        h_flex()
            .id(SharedString::from(format!("tunnel-{pattern}")))
            .h(px(36.))
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .font_family(theme.mono_font_family.clone())
            .text_sm()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(pattern.to_string()),
            )
            .child(
                Icon::new(IconName::ArrowRight)
                    .xsmall()
                    .text_color(theme.muted_foreground),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.muted_foreground)
                    // Not a target: say so in words, not in code
                    .when(target.contains('`'), |text| {
                        text.font_family(theme.font_family.clone()).italic()
                    })
                    .child(display_target(target)),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(
                        Button::new(SharedString::from(format!("edit-tunnel-{pattern}")))
                            .ghost()
                            .xsmall()
                            .label("Edit")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_form(Some(edit.clone()), window, cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("delete-tunnel-{pattern}")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Trash)
                            .tooltip("Delete tunnel…")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm_delete(delete.clone(), window, cx)
                            })),
                    ),
            )
    }
}

impl Render for TunnelsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let config = cache(cx).config.clone();
        let mut patterns: Vec<(&String, &String)> = config.ssh_tunnel_patterns.iter().collect();
        patterns.sort();
        let editing = self.form.clone();
        let mut list = v_flex().w_full();
        for (pattern, target) in patterns.iter().copied() {
            // The pattern being edited is replaced by the form
            if editing.as_ref() == Some(&Some(pattern.clone())) {
                list = list.child(div().py_2().child(self.render_form(cx)));
            } else {
                list = list.child(self.render_row(pattern, target, cx));
            }
        }
        let empty = patterns.is_empty() && editing.is_none();
        v_flex()
            .id("ssh-tunnels")
            .w_full()
            .gap_3()
            .when(empty, |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "No tunnels yet. Add one to reach databases through a bastion host.",
                        ),
                )
            })
            .child(list)
            .when(editing == Some(None), |this| {
                this.child(self.render_form(cx))
            })
            .when(editing.is_none(), |this| {
                this.child(
                    h_flex().child(
                        Button::new("add-tunnel")
                            .outline()
                            .small()
                            .icon(IconName::Plus)
                            .label("Add tunnel")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_form(None, window, cx)),
                            ),
                    ),
                )
            })
    }
}

/// "Which tunnel would this host use?", answered without connecting.
pub(super) struct TunnelTester {
    host: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl TunnelTester {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let host = cx.new(|cx| InputState::new(window, cx).placeholder("db.internal.example.com"));
        let subscriptions = vec![
            cx.subscribe(&host, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.observe_global::<SettingsCache>(|_, cx| cx.notify()),
        ];
        Self {
            host,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_host(&mut self, host: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.host.update(cx, |input, cx| {
            input.set_value(host.to_string(), window, cx)
        });
        cx.notify();
    }

    fn matches(&self, cx: &App) -> Vec<(String, String)> {
        let host = self.host.read(cx).value();
        let host = host.trim();
        if host.is_empty() {
            return Vec::new();
        }
        config_editor::matching_tunnel_patterns(&cache(cx).config, host)
    }
}

impl Render for TunnelTester {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let host = self.host.read(cx).value().trim().to_string();
        let matches = self.matches(cx);
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let line = |text: String, color: Hsla| div().text_xs().text_color(color).child(text);
        v_flex()
            .id("tunnel-tester")
            .w_full()
            .gap_2()
            .child(
                Input::new(&self.host)
                    .small()
                    .prefix(IconName::Search)
                    .cleanable(true),
            )
            .map(|this| match matches.len() {
                _ if host.is_empty() => this.child(line(
                    "Type a database host to see which tunnel it would use.".into(),
                    muted,
                )),
                0 => this.child(line(
                    format!("No pattern matches “{host}”: it connects directly."),
                    muted,
                )),
                1 => this.child(line(format!("“{host}” connects through:"), muted)),
                n => this.child(line(
                    format!(
                        "{n} patterns match “{host}”. Only one is used and which one is not \
                         predictable: make the patterns not overlap."
                    ),
                    theme.warning,
                )),
            })
            .children(matches.into_iter().map(|(pattern, target)| {
                h_flex()
                    .gap_2()
                    .items_center()
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .child(div().min_w_0().truncate().child(pattern))
                    .child(Icon::new(IconName::ArrowRight).xsmall().text_color(muted))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(muted)
                            .child(display_target(&target)),
                    )
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::tests::{open, settle};
    use gpui_kit::test::TestWindowExt as _;
    use std::prelude::v1::test;

    #[gpui_kit::test]
    fn the_form_validates_saves_and_previews_a_host(cx: &mut TestAppContext) {
        let store = ConfigStore::from_config(Config::default_settings());
        let (window, _, settings) = open(cx, store.clone(), SettingsPage::SshTunnels);
        let (tunnels, tester) = cx.update(|cx| {
            let settings = settings.read(cx);
            (settings.tunnels.clone(), settings.tunnel_tester.clone())
        });
        cx.update_window(window, |_, window, cx| {
            tunnels.update(cx, |this, cx| {
                this.open_form(None, window, cx);
                this.pattern
                    .update(cx, |input, cx| input.set_value("[", window, cx));
                this.target.update(cx, |input, cx| {
                    input.set_value("deploy:secret@bastion:22", window, cx)
                });
                let (pattern_error, target_error) = this.validation(cx);
                assert!(
                    pattern_error
                        .unwrap()
                        .starts_with("Not a valid regular expression")
                );
                assert_eq!(target_error, None);
                this.save(window, cx);
                assert!(this.form.is_some(), "an invalid form stays open");
                this.pattern.update(cx, |input, cx| {
                    input.set_value(r"^db\.internal$", window, cx)
                });
                assert!(this.can_save(cx));
                this.save(window, cx);
                assert!(this.form.is_none());
            });
            tester.update(cx, |this, cx| {
                this.host
                    .update(cx, |input, cx| input.set_value("db.internal", window, cx));
                let matches = this.matches(cx);
                assert_eq!(matches.len(), 1);
                assert!(!display_target(&matches[0].1).contains("secret"));
            });
            window.render_frame(cx);
        })
        .unwrap();
        settle(cx, |_| {
            store
                .try_read(|config| config.ssh_tunnel_patterns.contains_key(r"^db\.internal$"))
                .unwrap_or(false)
        });
        // Editing the pattern string replaces the key in one change
        cx.update_window(window, |_, window, cx| {
            tunnels.update(cx, |this, cx| {
                this.open_form(Some(r"^db\.internal$".into()), window, cx);
                this.pattern
                    .update(cx, |input, cx| input.set_value(r"^db\.", window, cx));
                this.save(window, cx);
            })
        })
        .unwrap();
        settle(cx, |_| {
            store
                .try_read(|config| {
                    config.ssh_tunnel_patterns.len() == 1
                        && config.ssh_tunnel_patterns.contains_key(r"^db\.")
                })
                .unwrap_or(false)
        });
    }

    #[gpui_kit::test]
    fn duplicates_and_empty_targets_are_refused(cx: &mut TestAppContext) {
        let mut config = Config::default_settings();
        config
            .ssh_tunnel_patterns
            .insert("^prod".into(), "deploy@bastion".into());
        let (window, _, settings) = open(
            cx,
            ConfigStore::from_config(config),
            SettingsPage::SshTunnels,
        );
        let tunnels = cx.update(|cx| settings.read(cx).tunnels.clone());
        cx.update_window(window, |_, window, cx| {
            tunnels.update(cx, |this, cx| {
                this.open_form(None, window, cx);
                this.pattern
                    .update(cx, |input, cx| input.set_value("^prod", window, cx));
                this.target
                    .update(cx, |input, cx| input.set_value("deploy@other", window, cx));
                let (pattern_error, _) = this.validation(cx);
                assert_eq!(pattern_error.as_deref(), Some(DUPLICATE_PATTERN));
                assert!(!this.can_save(cx));
                // An empty target cannot be saved
                this.pattern
                    .update(cx, |input, cx| input.set_value("^stage", window, cx));
                this.target
                    .update(cx, |input, cx| input.set_value("   ", window, cx));
                assert!(!this.can_save(cx));
                // A command target is accepted as is, never run
                this.target.update(cx, |input, cx| {
                    input.set_value("deploy@`get-bastion`", window, cx)
                });
                assert!(this.can_save(cx));
            });
            window.render_frame(cx);
        })
        .unwrap();
    }

    #[test]
    fn command_targets_are_never_shown_and_passwords_are_masked() {
        assert_eq!(
            display_target("`token-from-secret-store`"),
            "runs a command when connecting"
        );
        assert!(!display_target("user:secret@bastion").contains("secret"));
    }

    #[test]
    fn regex_errors_keep_the_reason() {
        let error = config_editor::validate_tunnel_pattern("[").unwrap_err();
        assert_eq!(
            pattern_error(error),
            "Not a valid regular expression: unclosed character class"
        );
        assert_eq!(
            pattern_error("pattern cannot be empty".into()),
            "Pattern cannot be empty"
        );
    }
}
