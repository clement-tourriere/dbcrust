//! The Vault wizard: Address → Database → Role → Connect, in a dialog.
//!
//! Each step renders from its own load state (loading, failed with a retry,
//! ready), stale replies are dropped by a generation counter, and Back always
//! returns to the step the user actually came from (a single role skips the
//! Role step on the way forward and on the way back).

use super::ui::caption;
use crate::app_state::AppState;
use dbcrust_gui_core::url_builder::{self as urls, VaultSpec};
use dbcrust_gui_core::vault::{self, VaultEnvironment};
use dbcrust_gui_core::{ConfigStore, ConnectRequest};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    form::{field, v_form},
    h_flex,
    input::{Input, InputEvent, InputState},
    spinner::Spinner,
    stepper::{Stepper, StepperItem},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::sync::Mutex;

/// The last Vault address used in this session. Persisting it belongs to the
/// desktop state file, owned by `gui_state`.
static LAST_ADDRESS: Mutex<Option<String>> = Mutex::new(None);

pub fn remembered_addr() -> Option<String> {
    LAST_ADDRESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

fn remember_addr(addr: &str) {
    *LAST_ADDRESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Some(addr.to_string()).filter(|addr| !addr.is_empty());
}

/// Open the wizard dialog. `hint` pre-fills the mount and highlights the
/// database / role it names.
pub fn open_wizard(hint: VaultSpec, read_only: bool, window: &mut Window, cx: &mut App) {
    let wizard = cx.new(|cx| VaultWizard::new(hint, read_only, window, cx));
    let content = wizard.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Connect with Vault")
            .width(px(560.))
            .child(content.clone())
    });
    wizard.update(cx, |wizard, cx| wizard.focus(window, cx));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Step {
    Address,
    Database,
    Role,
    Confirm,
}

impl Step {
    const ALL: [(Step, &'static str); 4] = [
        (Step::Address, "Address"),
        (Step::Database, "Database"),
        (Step::Role, "Role"),
        (Step::Confirm, "Connect"),
    ];

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|(step, _)| *step == self)
            .unwrap_or(0)
    }
}

enum Load<T> {
    Idle,
    Loading,
    Ready(T),
    Failed(SharedString),
}

pub struct VaultWizard {
    step: Step,
    addr: Entity<InputState>,
    mount: Entity<InputState>,
    environment: Load<VaultEnvironment>,
    databases: Load<Vec<String>>,
    roles: Load<Vec<String>>,
    database: Option<String>,
    role: Option<String>,
    /// The only role was picked for the user: Back skips the Role step.
    roles_skipped: bool,
    /// Names suggested by a partial `vault://` URL.
    hint: VaultSpec,
    read_only: bool,
    connecting: bool,
    error: Option<SharedString>,
    /// Bumped on every navigation; replies of older loads are ignored.
    generation: u64,
    _subscriptions: Vec<Subscription>,
}

impl VaultWizard {
    fn new(hint: VaultSpec, read_only: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mount_value = if hint.mount.is_empty() {
            "database".to_string()
        } else {
            hint.mount.clone()
        };
        let addr = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("https://vault.example.com:8200");
            match remembered_addr() {
                Some(addr) => state.default_value(addr),
                None => state,
            }
        });
        let mount = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("database")
                .default_value(mount_value)
        });
        let mut subscriptions = Vec::new();
        for input in [&addr, &mount] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) && this.step == Step::Address
                    {
                        this.load_databases(cx);
                    }
                },
            ));
        }
        let mut wizard = Self {
            step: Step::Address,
            addr,
            mount,
            environment: Load::Loading,
            databases: Load::Idle,
            roles: Load::Idle,
            database: None,
            role: None,
            roles_skipped: false,
            hint,
            read_only,
            connecting: false,
            error: None,
            generation: 0,
            _subscriptions: subscriptions,
        };
        wizard.detect_environment(window, cx);
        wizard
    }

    fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.addr.update(cx, |state, cx| state.focus(window, cx));
    }

    fn store(&self, cx: &App) -> ConfigStore {
        AppState::global(cx).read(cx).store().clone()
    }

    fn addr_value(&self, cx: &App) -> Option<String> {
        Some(self.addr.read(cx).value().trim().to_string()).filter(|addr| !addr.is_empty())
    }

    fn mount_value(&self, cx: &App) -> String {
        let mount = self.mount.read(cx).value().trim().to_string();
        if mount.is_empty() {
            "database".to_string()
        } else {
            mount
        }
    }

    /// Look for `VAULT_ADDR` and a token. Blocking (it may run `launchctl`),
    /// so it runs in the background.
    fn detect_environment(&mut self, window: &Window, cx: &mut Context<Self>) {
        let store = self.store(cx);
        cx.spawn_in(window, async move |this, cx| {
            let environment = cx
                .background_spawn(async move { vault::detect_environment(&store) })
                .await;
            this.update_in(cx, |this, window, cx| {
                let typed = this.addr_value(cx).is_some();
                if let (false, Some(detected)) = (typed, environment.addr.clone()) {
                    this.addr
                        .update(cx, |state, cx| state.set_value(detected, window, cx));
                }
                this.environment = Load::Ready(environment);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ── Navigation ────────────────────────────────────────────────────

    fn go_to(&mut self, step: Step, cx: &mut Context<Self>) {
        self.generation += 1;
        self.step = step;
        self.error = None;
        cx.notify();
    }

    /// Back to the step the user came from.
    fn back(&mut self, cx: &mut Context<Self>) {
        let previous = match self.step {
            Step::Address => return,
            Step::Database => Step::Address,
            Step::Role => Step::Database,
            Step::Confirm if self.roles_skipped => Step::Database,
            Step::Confirm => Step::Role,
        };
        self.go_to(previous, cx);
    }

    /// Stepper click: only earlier steps are reachable.
    fn jump(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(&(step, _)) = Step::ALL.get(index)
            && step < self.step
        {
            self.go_to(step, cx);
        }
    }

    fn load_databases(&mut self, cx: &mut Context<Self>) {
        let addr = self.addr_value(cx);
        let mount = self.mount_value(cx);
        remember_addr(addr.as_deref().unwrap_or_default());
        self.generation += 1;
        let generation = self.generation;
        self.step = Step::Database;
        self.databases = Load::Loading;
        self.error = None;
        let listing = vault::list_databases(&self.store(cx), mount, addr);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = listing.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.databases = match result {
                    Ok(databases) if databases.is_empty() => Load::Failed(
                        "No database configs in this mount. Check the mount path and your token's permissions."
                            .into(),
                    ),
                    Ok(databases) => Load::Ready(databases),
                    Err(error) => Load::Failed(error.to_string().into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn choose_database(&mut self, database: String, cx: &mut Context<Self>) {
        self.database = Some(database.clone());
        self.role = None;
        self.roles_skipped = false;
        self.load_roles(database, cx);
    }

    fn reload_roles(&mut self, cx: &mut Context<Self>) {
        if let Some(database) = self.database.clone() {
            self.load_roles(database, cx);
        }
    }

    fn load_roles(&mut self, database: String, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.step = Step::Role;
        self.roles = Load::Loading;
        self.error = None;
        let listing = vault::list_roles(
            &self.store(cx),
            self.mount_value(cx),
            database,
            self.addr_value(cx),
        );
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = listing.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(roles) if roles.is_empty() => {
                        this.roles = Load::Failed(
                            "Your token can't read any role of this database. Ask for access, or pick another database."
                                .into(),
                        );
                    }
                    Ok(roles) if roles.len() == 1 => {
                        // exactly one choice: skip the Role step
                        this.role = roles.first().cloned();
                        this.roles_skipped = true;
                        this.roles = Load::Ready(roles);
                        this.step = Step::Confirm;
                    }
                    Ok(roles) => this.roles = Load::Ready(roles),
                    Err(error) => this.roles = Load::Failed(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn choose_role(&mut self, role: String, cx: &mut Context<Self>) {
        self.role = Some(role);
        self.go_to(Step::Confirm, cx);
    }

    fn spec(&self, cx: &App) -> Option<VaultSpec> {
        Some(VaultSpec {
            role: self.role.clone()?,
            mount: self.mount_value(cx),
            database: self.database.clone()?,
        })
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(spec) = self.spec(cx) else {
            return;
        };
        if self.connecting {
            return;
        }
        let addr = self.addr_value(cx);
        let request = ConnectRequest::url(urls::build_vault_url(&spec))
            .vault_addr(addr)
            .read_only(self.read_only);
        let connecting = AppState::global(cx).update(cx, |state, cx| state.connect(request, cx));
        self.connecting = true;
        self.error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = connecting.await;
            this.update_in(cx, |this, window, cx| {
                this.connecting = false;
                match result {
                    Ok(_) => window.close_dialog(cx),
                    Err(error) => this.error = Some(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

// ── Rendering ─────────────────────────────────────────────────────────

fn loading_row(label: impl Into<SharedString>, cx: &App) -> Div {
    h_flex()
        .justify_center()
        .gap_2()
        .py_6()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(Spinner::new().small())
        .child(label.into())
}

/// A pickable entry of a step's list.
fn choice(id: SharedString, name: &str, hinted: bool, cx: &App) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(px(32.))
        .px_3()
        .gap_2()
        .justify_between()
        .rounded(cx.theme().radius)
        .hover(|this| this.bg(cx.theme().list_hover))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_sm()
                .font_family(cx.theme().mono_font_family.clone())
                .child(name.to_string()),
        )
        .child(
            h_flex()
                .gap_2()
                .text_color(cx.theme().muted_foreground)
                .when(hinted, |this| {
                    this.child(div().text_xs().child("from your URL"))
                })
                .child(Icon::new(IconName::ChevronRight).small()),
        )
}

fn failure(
    id: &'static str,
    message: SharedString,
    retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    v_flex()
        .gap_2()
        .child(Alert::error(id, message).small())
        .child(
            h_flex().child(
                Button::new(SharedString::from(format!("retry-{id}")))
                    .small()
                    .outline()
                    .label("Try again")
                    .on_click(retry),
            ),
        )
}

fn heading(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

impl VaultWizard {
    fn render_stepper(&self, cx: &Context<Self>) -> impl IntoElement {
        Stepper::new("vault-steps")
            .small()
            .selected_index(self.step.index())
            .items(
                Step::ALL
                    .iter()
                    .map(|(_, label)| StepperItem::new().child(*label)),
            )
            .on_click(cx.listener(|this, index: &usize, _, cx| this.jump(*index, cx)))
    }

    fn render_address(&self) -> AnyElement {
        let (description, warning) = match &self.environment {
            Load::Ready(environment) => {
                let description = match (&environment.addr, &environment.source) {
                    (Some(_), Some(source)) => format!("Detected from {source}"),
                    (Some(_), None) => "Detected automatically".to_string(),
                    (None, _) => {
                        "No address detected. Enter it, or set VAULT_ADDR before launching DBCrust."
                            .to_string()
                    }
                };
                let warning = (!environment.token_available).then_some(
                    "No Vault token found. Set VAULT_TOKEN, run `vault login`, or configure \
                     vault_credential_process, then continue.",
                );
                (description, warning)
            }
            Load::Loading => ("Looking for VAULT_ADDR and a token…".to_string(), None),
            _ => (String::new(), None),
        };
        v_flex()
            .gap_3()
            .child(
                v_form()
                    .small()
                    .child(
                        field()
                            .label("Vault address")
                            .description(description)
                            .child(Input::new(&self.addr).small()),
                    )
                    .child(
                        field()
                            .label("Mount")
                            .description("Secrets engine that holds the database configs")
                            .child(Input::new(&self.mount).small()),
                    ),
            )
            .children(warning.map(|text| Alert::warning("vault-token", text).small()))
            .into_any_element()
    }

    fn render_databases(&self, cx: &Context<Self>) -> AnyElement {
        match &self.databases {
            Load::Idle | Load::Loading => loading_row(
                format!("Listing databases in “{}”…", self.mount_value(cx)),
                cx,
            )
            .into_any_element(),
            Load::Failed(message) => failure(
                "databases",
                message.clone(),
                cx.listener(|this, _, _, cx| this.load_databases(cx)),
            )
            .into_any_element(),
            Load::Ready(databases) => v_flex()
                .gap_1()
                .child(heading(
                    format!(
                        "{} in “{}”",
                        plural(databases.len(), "database"),
                        self.mount_value(cx)
                    ),
                    cx,
                ))
                .child(
                    v_flex()
                        .id("vault-databases")
                        .max_h(px(280.))
                        .overflow_y_scroll()
                        .children(databases.iter().map(|name| {
                            let database = name.clone();
                            let hinted = self.hint.database == *name;
                            choice(format!("vault-database-{name}").into(), name, hinted, cx)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.choose_database(database.clone(), cx);
                                }))
                        })),
                )
                .into_any_element(),
        }
    }

    fn render_roles(&self, cx: &Context<Self>) -> AnyElement {
        let database = self.database.clone().unwrap_or_default();
        match &self.roles {
            Load::Idle | Load::Loading => {
                loading_row(format!("Looking up the roles of “{database}”…"), cx).into_any_element()
            }
            Load::Failed(message) => failure(
                "roles",
                message.clone(),
                cx.listener(|this, _, _, cx| this.reload_roles(cx)),
            )
            .into_any_element(),
            Load::Ready(roles) => v_flex()
                .gap_1()
                .child(heading(
                    format!("{} for “{database}”", plural(roles.len(), "role")),
                    cx,
                ))
                .child(
                    v_flex()
                        .id("vault-roles")
                        .max_h(px(280.))
                        .overflow_y_scroll()
                        .children(roles.iter().map(|name| {
                            let role = name.clone();
                            let hinted = self.hint.role == *name;
                            choice(format!("vault-role-{name}").into(), name, hinted, cx).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.choose_role(role.clone(), cx);
                                }),
                            )
                        })),
                )
                .into_any_element(),
        }
    }

    fn render_confirm(&self, cx: &Context<Self>) -> AnyElement {
        let Some(spec) = self.spec(cx) else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let detail = |label: &'static str, value: String| {
            h_flex()
                .gap_3()
                .child(
                    div()
                        .w(px(72.))
                        .flex_none()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_family(theme.mono_font_family.clone())
                        .child(value),
                )
        };
        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_1p5()
                    .p_3()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .child(detail(
                        "Address",
                        self.addr_value(cx)
                            .unwrap_or_else(|| "from VAULT_ADDR".into()),
                    ))
                    .child(detail("Mount", spec.mount.clone()))
                    .child(detail("Database", spec.database.clone()))
                    .child(detail("Role", spec.role.clone())),
            )
            .child(caption(urls::build_vault_url(&spec), cx))
            .child(heading(
                "Vault issues short-lived credentials for this role when you connect.",
                cx,
            ))
            .into_any_element()
    }
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

impl Render for VaultWizard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.step {
            Step::Address => self.render_address(),
            Step::Database => self.render_databases(cx),
            Step::Role => self.render_roles(cx),
            Step::Confirm => self.render_confirm(cx),
        };
        let back = (self.step != Step::Address).then(|| {
            Button::new("vault-back")
                .outline()
                .label("Back")
                .on_click(cx.listener(|this, _, _, cx| this.back(cx)))
        });
        let primary = match self.step {
            Step::Address => Some(
                Button::new("vault-continue")
                    .primary()
                    .label("List databases")
                    .on_click(cx.listener(|this, _, _, cx| this.load_databases(cx))),
            ),
            Step::Confirm => Some(
                Button::new("vault-connect")
                    .primary()
                    .label("Connect")
                    .loading(self.connecting)
                    .on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))),
            ),
            Step::Database | Step::Role => None,
        };
        v_flex()
            .gap_4()
            .pb_1()
            .child(self.render_stepper(cx))
            .child(body)
            .children(
                self.error
                    .clone()
                    .map(|message| Alert::error("vault-error", message).small()),
            )
            .child(
                h_flex()
                    .justify_between()
                    .child(h_flex().children(back))
                    .child(h_flex().children(primary)),
            )
    }
}
