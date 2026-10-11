//! Connect and test attempts on the page: inline progress with the real
//! phases and elapsed time, results and failures attached to the attempt's
//! target, and the "Connection details" dialog.

use super::ui::mono;
use crate::app_state::{
    AppState, AttemptFailure, AttemptKind, AttemptSpec, AttemptState, ConnectAttempt,
};
use dbcrust_gui_core::docker::{DockerRoute, RouteKind};
use dbcrust_gui_core::launcher::{ConnectPhase, elapsed_label, phase_label};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    notification::Notification,
    spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;

/// The attempt key under which the test of `key` is filed (a test and a
/// connect of the same target run independently).
pub(super) fn test_target(key: &str) -> String {
    format!("{key}#test")
}

/// How the caller lets the user go back to the connection's settings.
pub(super) type EditHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// The panels for everything the attempts of `key` (connect, then test) have
/// to say. Empty when there is nothing to show.
pub(super) fn attempt_panels(key: &str, edit: Option<EditHandler>, cx: &App) -> Vec<AnyElement> {
    let state = AppState::global(cx);
    let state = state.read(cx);
    [key.to_string(), test_target(key)]
        .iter()
        .filter_map(|target| state.attempt_for(target))
        .map(|attempt| render_attempt(attempt, edit.clone(), cx))
        .collect()
}

/// Is an attempt of `key` (either kind) running?
pub(super) fn is_busy(key: &str, cx: &App) -> bool {
    let state = AppState::global(cx);
    let state = state.read(cx);
    [key.to_string(), test_target(key)].iter().any(|target| {
        state
            .attempt_for(target)
            .is_some_and(ConnectAttempt::is_running)
    })
}

/// `Published port · 127.0.0.1:5432`, `Through Docker · perl relay`.
pub(super) fn route_text(route: &DockerRoute) -> String {
    match route.kind {
        RouteKind::ExecTunnel => match &route.helper {
            Some(helper) => format!("{} · {helper} relay", route.kind.label()),
            None => route.kind.label().to_string(),
        },
        kind => format!("{} · {}:{}", kind.label(), route.host, route.port),
    }
}

fn render_attempt(attempt: &ConnectAttempt, edit: Option<EditHandler>, cx: &App) -> AnyElement {
    match &attempt.state {
        AttemptState::Running => render_running(attempt, cx),
        AttemptState::Tested(outcome) => {
            let theme = cx.theme();
            let id = attempt.id;
            let mut detail = vec![outcome.server.clone()];
            if let Some(route) = &outcome.route {
                detail.push(route_text(route));
            }
            detail.push(elapsed_label(outcome.elapsed));
            panel(theme.success, cx)
                .child(
                    Icon::new(IconName::CircleCheck)
                        .size(px(16.))
                        .text_color(theme.success)
                        .flex_none(),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(div().text_sm().font_medium().child("Connection works"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(detail.join(" · ")),
                        ),
                )
                .child(
                    Button::new(SharedString::from(format!("dismiss-{}", attempt.target)))
                        .ghost()
                        .xsmall()
                        .icon(IconName::X)
                        .tooltip("Dismiss")
                        .on_click(move |_, _, cx| {
                            AppState::global(cx)
                                .update(cx, |state, cx| state.dismiss_attempt(id, cx));
                        }),
                )
                .into_any_element()
        }
        AttemptState::Failed(failure) => render_failure(attempt, failure, edit, cx),
    }
}

/// The bordered, tinted box all attempt panels share.
fn panel(accent: Hsla, cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .w_full()
        .items_start()
        .gap_3()
        .p_3()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(accent.opacity(0.45))
        .bg(accent.opacity(0.07))
}

fn render_running(attempt: &ConnectAttempt, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let elapsed = attempt.elapsed();
    let id = attempt.id;
    let verb = match attempt.kind {
        AttemptKind::Connect => "Connecting to",
        AttemptKind::Test => "Testing",
    };
    // The steps reached so far. "Preparing" is not worth a line of its own
    // once something more specific followed.
    let steps: Vec<_> = attempt
        .phases
        .steps(elapsed)
        .into_iter()
        .filter(|step| step.phase != ConnectPhase::Starting)
        .collect();
    v_flex()
        .w_full()
        .gap_2()
        .p_3()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted.opacity(0.35))
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(Spinner::new().small())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_medium()
                        .child(format!("{verb} {}…", attempt.subject)),
                )
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .font_family(theme.mono_font_family.clone())
                        .child(elapsed_label(elapsed)),
                )
                .child(
                    Button::new(SharedString::from(format!("cancel-{}", attempt.target)))
                        .xsmall()
                        .outline()
                        .label("Cancel")
                        .tooltip("Esc")
                        .on_click(move |_, _, cx| {
                            AppState::global(cx)
                                .update(cx, |state, cx| state.cancel_attempt(id, cx));
                        }),
                ),
        )
        .when(!steps.is_empty(), |this| {
            this.child(
                v_flex()
                    .pl(px(28.))
                    .gap(px(3.))
                    .children(steps.into_iter().map(|step| {
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_xs()
                            .text_color(if step.done {
                                theme.muted_foreground
                            } else {
                                theme.foreground
                            })
                            .child(
                                Icon::new(if step.done {
                                    IconName::Check
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(px(12.))
                                .flex_none(),
                            )
                            .child(phase_label(step.phase))
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(elapsed_label(step.took)),
                            )
                    })),
            )
        })
        .into_any_element()
}

fn render_failure(
    attempt: &ConnectAttempt,
    failure: &AttemptFailure,
    edit: Option<EditHandler>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let id = attempt.id;
    let spec = AttemptSpec {
        kind: attempt.kind,
        target: attempt.target.clone(),
        subject: attempt.subject.clone(),
        request: attempt.request.clone(),
        identity: attempt.identity.clone(),
        environment: attempt.environment.clone(),
    };
    // A container no route reaches says so in its own words
    let (title, body) = match &failure.docker {
        Some(docker) => (docker.headline.clone(), docker.explanation.clone()),
        None => (
            match attempt.kind {
                AttemptKind::Test => format!("Test of {} failed", attempt.subject),
                AttemptKind::Connect => format!("Couldn’t connect to {}", attempt.subject),
            },
            failure.message.clone(),
        ),
    };
    let details_failure = failure.clone();
    let details_title = title.clone();
    let has_details = failure.docker.is_some();

    panel(theme.danger, cx)
        .child(
            Icon::new(IconName::CircleAlert)
                .size(px(16.))
                .text_color(theme.danger)
                .flex_none()
                .mt(px(1.)),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1p5()
                .child(div().text_sm().font_semibold().child(title))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.foreground.opacity(0.9))
                        .child(body),
                )
                .child(
                    h_flex()
                        .mt_1()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            Button::new(SharedString::from(format!("retry-{}", attempt.target)))
                                .small()
                                .outline()
                                .icon(IconName::RefreshCw)
                                .label("Retry")
                                .on_click(move |_, _, cx| {
                                    let spec = AttemptSpec {
                                        kind: spec.kind,
                                        target: spec.target.clone(),
                                        subject: spec.subject.clone(),
                                        request: spec.request.clone(),
                                        identity: spec.identity.clone(),
                                        environment: spec.environment.clone(),
                                    };
                                    AppState::global(cx).update(cx, |state, cx| {
                                        state.start_attempt(spec, cx);
                                    });
                                }),
                        )
                        .children(edit.map(|edit| {
                            Button::new(SharedString::from(format!("edit-{}", attempt.target)))
                                .small()
                                .ghost()
                                .label("Edit connection")
                                .on_click(move |_, window, cx| edit(window, cx))
                        }))
                        .when(has_details, |this| {
                            this.child(
                                Button::new(SharedString::from(format!(
                                    "details-{}",
                                    attempt.target
                                )))
                                .small()
                                .ghost()
                                .label("Connection details")
                                .on_click(
                                    move |_, window, cx| {
                                        open_details(
                                            details_title.clone(),
                                            details_failure.clone(),
                                            window,
                                            cx,
                                        );
                                    },
                                ),
                            )
                        }),
                ),
        )
        .child(
            Button::new(SharedString::from(format!("dismiss-{}", attempt.target)))
                .ghost()
                .xsmall()
                .icon(IconName::X)
                .tooltip("Dismiss")
                .on_click(move |_, _, cx| {
                    AppState::global(cx).update(cx, |state, cx| state.dismiss_attempt(id, cx));
                }),
        )
        .into_any_element()
}

/// The routes that were tried, and the line that publishes a container's
/// port on localhost only.
fn open_details(title: String, failure: AttemptFailure, window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let theme = cx.theme();
        let block = |text: String| {
            mono(text, cx)
                .w_full()
                .p_2p5()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .bg(theme.muted.opacity(0.4))
                .whitespace_normal()
        };
        let label = |text: &'static str| {
            div()
                .text_xs()
                .font_semibold()
                .text_color(theme.muted_foreground)
                .child(text)
        };
        let docker = failure.docker.clone();
        let routes: Vec<String> = match &docker {
            Some(docker) if !docker.attempts.is_empty() => docker
                .attempts
                .iter()
                .map(|attempt| {
                    format!(
                        "{} · {}\n{}",
                        attempt.route.label(),
                        attempt.target,
                        attempt.reason
                    )
                })
                .collect(),
            _ => vec!["No route detail was reported for this attempt.".to_string()],
        };
        let body = v_flex()
            .gap_3()
            .child(div().text_sm().font_medium().child(title.clone()))
            .child(
                v_flex()
                    .gap_1p5()
                    .child(label("WHAT WAS TRIED"))
                    .children(routes.into_iter().map(&block)),
            )
            .children(docker.map(|docker| {
                let copied = docker.compose_example.clone();
                v_flex()
                    .gap_1p5()
                    .child(label("PUBLISH THE PORT ON LOCALHOST ONLY"))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_start()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(block(docker.compose_example)),
                            )
                            .child(
                                Button::new("copy-compose-snippet")
                                    .small()
                                    .outline()
                                    .icon(IconName::Copy)
                                    .label("Copy")
                                    .on_click(move |_, window, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copied.clone(),
                                        ));
                                        window.push_notification(
                                            Notification::success("Copied the ports line"),
                                            cx,
                                        );
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "Add it to the database service in compose.yaml. {}",
                                docker.compose_note
                            )),
                    )
            }));
        dialog
            .title("Connection details")
            .width(px(560.))
            .child(body)
    });
}
