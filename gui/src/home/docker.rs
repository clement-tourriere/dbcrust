//! Docker on the Connections page: discovery, and how the list explains a
//! daemon that is loading, empty, or not usable.

use super::HomeView;
use super::ui::{skeleton_row, state_block};
use dbcrust_gui_core::docker::{self, DockerDiscovery};
use dbcrust_gui_core::launcher::{
    DockerListState, DockerNote, DockerProblemKind, classify_docker_problem,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
};
use gpui_kit::*;

impl HomeView {
    /// Look for database containers. Ignored while a lookup is running; the
    /// list keeps showing the previous result until the new one arrives, so
    /// refreshing never makes rows jump or selections disappear.
    pub(super) fn discover_docker(&mut self, cx: &mut Context<Self>) {
        if self.docker_in_flight {
            return;
        }
        self.docker_in_flight = true;
        if !matches!(self.docker, DockerListState::Ready { .. }) {
            self.docker = DockerListState::Loading;
            self.rebuild(cx);
        }
        let snapshot = docker::discover_snapshot();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = snapshot.await;
            this.update(cx, |this, cx| {
                this.docker_in_flight = false;
                // The list and the daemon that produced it travel together:
                // a click connects to that daemon, whatever context is
                // current by then
                let (discovery, daemon) = match result {
                    Ok(snapshot) => (Ok(snapshot.discovery), snapshot.daemon),
                    Err(error) => (Err(error), None),
                };
                let context = daemon
                    .as_ref()
                    .and_then(|daemon| daemon.label().map(str::to_string));
                this.set_docker_daemon(daemon);
                this.docker = match discovery {
                    Ok(DockerDiscovery::Containers(containers)) => DockerListState::Ready {
                        containers,
                        context,
                    },
                    Ok(DockerDiscovery::Unavailable(message)) => DockerListState::Problem {
                        kind: classify_docker_problem(&message),
                        message,
                        context,
                    },
                    Err(error) => {
                        let message = error.to_string();
                        DockerListState::Problem {
                            kind: classify_docker_problem(&message),
                            message,
                            context,
                        }
                    }
                };
                this.rebuild(cx);
            })
            .ok();
        })
        .detach();
    }

    /// The Docker note inside the All list: skeleton rows while loading, a
    /// one-line reason otherwise.
    pub(super) fn render_docker_note(&self, note: &DockerNote, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let line = |icon: IconName, text: SharedString, action: Option<AnyElement>| {
            h_flex()
                .w_full()
                .px_4()
                .min_h(px(40.))
                .gap_2()
                .items_center()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(Icon::new(icon).size(px(14.)).flex_none())
                .child(div().flex_1().min_w_0().child(text))
                .children(action)
                .into_any_element()
        };
        match note {
            DockerNote::Loading => gpui_kit::component::v_flex()
                .children((0..3).map(|ix| skeleton_row(ix, cx)))
                .into_any_element(),
            DockerNote::Empty { context } => line(
                IconName::Container,
                match context {
                    Some(context) => format!("No containers in “{context}”").into(),
                    None => "No database containers found".into(),
                },
                Some(self.refresh_button("docker-note-refresh", "Refresh", cx)),
            ),
            DockerNote::Problem { kind, context } => line(
                IconName::TriangleAlert,
                match context {
                    Some(context) => format!("{} · “{context}”", kind.title()).into(),
                    None => kind.title().into(),
                },
                Some(self.refresh_button("docker-note-retry", "Retry", cx)),
            ),
        }
    }

    /// The Docker filter's full explanation (nothing to list).
    pub(super) fn render_docker_block(&self, note: &DockerNote, cx: &Context<Self>) -> AnyElement {
        match note {
            DockerNote::Loading => gpui_kit::component::v_flex()
                .pt_1()
                .children((0..5).map(|ix| skeleton_row(ix, cx)))
                .into_any_element(),
            DockerNote::Empty { context } => {
                let body = match context {
                    Some(context) => format!(
                        "No database containers found in “{context}”. Containers running \
                         PostgreSQL, MySQL, MongoDB, ClickHouse or Elasticsearch images appear \
                         here."
                    ),
                    None => "No database containers found. Containers running PostgreSQL, MySQL, \
                             MongoDB, ClickHouse or Elasticsearch images appear here."
                        .to_string(),
                };
                state_block(
                    IconName::Container,
                    "No database containers",
                    body,
                    vec![
                        self.refresh_button("docker-block-refresh", "Refresh", cx),
                        Button::new("docker-block-new")
                            .small()
                            .ghost()
                            .label("New connection")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_new_form(window, cx)),
                            )
                            .into_any_element(),
                    ],
                    cx,
                )
                .into_any_element()
            }
            DockerNote::Problem { kind, context } => {
                let message = match &self.docker {
                    DockerListState::Problem { message, .. } if !message.is_empty() => {
                        format!("{}\n\n{message}", kind.advice(context.as_deref()))
                    }
                    _ => kind.advice(context.as_deref()),
                };
                let icon = match kind {
                    DockerProblemKind::PermissionDenied => IconName::Lock,
                    DockerProblemKind::Unsupported => IconName::Unplug,
                    _ => IconName::Container,
                };
                let retry = match context {
                    Some(context) => format!("Retry “{context}”"),
                    None => "Retry".to_string(),
                };
                state_block(
                    icon,
                    kind.title(),
                    message,
                    vec![self.refresh_button("docker-block-retry", retry, cx)],
                    cx,
                )
                .into_any_element()
            }
        }
    }

    fn refresh_button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        cx: &Context<Self>,
    ) -> AnyElement {
        Button::new(id)
            .xsmall()
            .outline()
            .label(label)
            .loading(self.docker_in_flight)
            .on_click(cx.listener(|this, _, _, cx| this.discover_docker(cx)))
            .into_any_element()
    }
}
