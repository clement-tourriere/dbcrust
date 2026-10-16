//! The shared DBCrust settings (`config_editor::schema()`), each on exactly
//! one Settings page, rendered from its [`FieldKind`].

use super::*;
use dbcrust_gui_core::dbcrust::config_editor::{ConfigSection, FieldKind, FieldSpec};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use storage::{Edit, read_value, write_value};

/// The page that shows `spec`. Every field lands on exactly one page; the
/// pages that hold schema fields render all of theirs (see the tests).
pub(super) fn page_for(spec: &FieldSpec) -> SettingsPage {
    match spec.path {
        "default_limit" | "query_timeout_seconds" | "metadata_timeout_seconds" => {
            SettingsPage::Queries
        }
        "read_only_default" | "max_recent_connections" => SettingsPage::Connections,
        _ => match spec.section {
            ConfigSection::Vault => SettingsPage::Vault,
            ConfigSection::VectorDisplay | ConfigSection::ComplexDisplay => {
                SettingsPage::ResultsDisplay
            }
            ConfigSection::Ai => SettingsPage::Ai,
            ConfigSection::Logging => SettingsPage::Logging,
            // Display, Pager, Features (completion, EXPLAIN mode), History
            // and the rest are read by the command line only
            ConfigSection::Display
            | ConfigSection::Pager
            | ConfigSection::Features
            | ConfigSection::Timeouts
            | ConfigSection::History
            | ConfigSection::Secrets
            | ConfigSection::SshTunnelPatterns => SettingsPage::Terminal,
        },
    }
}

fn specs(page: SettingsPage) -> impl Iterator<Item = &'static FieldSpec> {
    config_editor::schema()
        .iter()
        .filter(move |spec| page_for(spec) == page)
}

/// The fields of `page`, in schema order.
pub(super) fn schema_items(page: SettingsPage, defaults: &Config) -> Vec<SettingItem> {
    specs(page).map(|spec| item(spec, defaults)).collect()
}

/// One field by path, for pages that arrange their fields themselves.
pub(super) fn schema_item(path: &str, defaults: &Config) -> Option<SettingItem> {
    config_editor::find_spec(path).map(|spec| item(spec, defaults))
}

/// The fields of `page`, one group per schema section.
pub(super) fn schema_groups(page: SettingsPage, defaults: &Config) -> Vec<SettingGroup> {
    use strum::IntoEnumIterator;
    ConfigSection::iter()
        .filter_map(|section| {
            let items: Vec<SettingItem> = specs(page)
                .filter(|spec| spec.section == section)
                .map(|spec| item(spec, defaults))
                .collect();
            (!items.is_empty())
                .then(|| SettingGroup::new().title(group_title(section)).items(items))
        })
        .collect()
}

fn group_title(section: ConfigSection) -> &'static str {
    match section {
        ConfigSection::Display => "Display",
        ConfigSection::Vault => "Vault",
        ConfigSection::ComplexDisplay => "JSON, arrays and other complex values",
        ConfigSection::VectorDisplay => "Vectors",
        section => section.label(),
    }
}

/// Curated title and description; the schema's label and help otherwise.
fn copy(spec: &FieldSpec) -> (&'static str, &'static str) {
    match spec.path {
        "default_limit" => (
            "Default row limit",
            "Rows fetched by a SELECT without its own LIMIT. 0 turns the limit off.",
        ),
        "query_timeout_seconds" => (
            "Query timeout",
            "Seconds before a running statement is stopped.",
        ),
        "metadata_timeout_seconds" => (
            "Schema timeout",
            "Seconds allowed to load tables, columns and completions.",
        ),
        "autocomplete_enabled" => (
            "SQL completion",
            "Suggest keywords, tables and columns while typing.",
        ),
        "read_only_default" => (
            "Open connections read-only",
            "Block statements that change data on new connections. Turn it off per \
             connection when you need to write.",
        ),
        "max_recent_connections" => (
            "Recent connections",
            "How many recent connections to remember.",
        ),
        "vault_credential_cache_enabled" => (
            "Cache Vault credentials",
            "Reuse short-lived credentials until they expire.",
        ),
        "vault_credential_process" => (
            "Credential process",
            "Command that prints a Vault token or credentials when a vault:// connection \
             opens.",
        ),
        "test_named_query_before_saving" => (
            "Test saved queries before saving",
            "\\ns runs a query in a rolled-back transaction before saving it.",
        ),
        _ => (spec.label, spec.help),
    }
}

/// Step of a float field: fine for ratios and temperatures.
fn float_step(min: f64, max: f64) -> f64 {
    if max - min <= 2.0 { 0.05 } else { 1.0 }
}

/// "auto_select" → "Auto select"
fn option_label(value: &str) -> String {
    let text = value.replace('_', " ");
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => text,
    }
}

fn item(spec: &'static FieldSpec, defaults: &Config) -> SettingItem {
    #[cfg(test)]
    tests::record(spec.path);
    let (title, help) = copy(spec);
    let description = if config_editor::takes_effect_next_session(spec.path) {
        format!("{help} Takes effect next session.")
    } else {
        help.to_string()
    };
    let default = (spec.get)(defaults);
    let item = match spec.kind {
        FieldKind::Bool => SettingItem::new(
            title,
            SettingField::switch(
                move |cx| read_value(cx, spec) == "true",
                move |value, cx| write_value(cx, spec, value.to_string()),
            )
            .default_value(default == "true"),
        ),
        FieldKind::UInt { min, max } => {
            let default = default.parse::<f64>().unwrap_or(min as f64);
            SettingItem::new(
                title,
                SettingField::number_input(
                    NumberFieldOptions {
                        min: min as f64,
                        max: max as f64,
                        step: 1.,
                    },
                    move |cx| read_value(cx, spec).parse().unwrap_or(default),
                    move |value, cx| {
                        let value = value.round().clamp(min as f64, max as f64) as u64;
                        write_value(cx, spec, value.to_string())
                    },
                )
                .default_value(default),
            )
        }
        FieldKind::Float { min, max } => {
            let default = default.parse::<f64>().unwrap_or(min);
            SettingItem::new(
                title,
                SettingField::number_input(
                    NumberFieldOptions {
                        min,
                        max,
                        step: float_step(min, max),
                    },
                    move |cx| read_value(cx, spec).parse().unwrap_or(default),
                    move |value, cx| write_value(cx, spec, value.clamp(min, max).to_string()),
                )
                .default_value(default),
            )
        }
        FieldKind::Enum(options) => SettingItem::new(
            title,
            SettingField::dropdown(
                options
                    .iter()
                    .map(|value| ((*value).into(), option_label(value).into()))
                    .collect(),
                move |cx| read_value(cx, spec).into(),
                move |value, cx| write_value(cx, spec, value.to_string()),
            )
            .default_value(default),
        ),
        FieldKind::Text { .. } | FieldKind::OptionalText => {
            SettingItem::new(title, text_field(spec, default)).layout(Axis::Vertical)
        }
    };
    item.description(description)
        .keywords([spec.path, spec.label])
}

/// A text input bound to a schema field. Unlike the stock string field it
/// keeps what is typed (the schema trims values; re-syncing the box from
/// the trimmed value would eat a space typed between two words), applies
/// every valid value at once, and reports an invalid one (e.g. an empty
/// pager command) when the box loses focus or on Enter, then shows the
/// value in effect again. Sensitive fields are masked.
fn text_field(spec: &'static FieldSpec, default: String) -> SettingField<SharedString> {
    let reset_to = default.clone();
    SettingField::render(move |options, window, cx| {
        let value: SharedString = read_value(cx, spec).into();
        let state = window.use_keyed_state(
            SharedString::from(format!("schema-text-{}", spec.path)),
            cx,
            |window, cx| TextFieldState::new(spec, value.clone(), window, cx),
        );
        // Changed elsewhere (reset, reload from disk): show the new value
        state.update(cx, |state, cx| {
            if state.synced != value {
                state.synced = value.clone();
                state.error = None;
                state
                    .input
                    .update(cx, |input, cx| input.set_value(value.clone(), window, cx));
            }
        });
        Input::new(&state.read(cx).input)
            .with_size(options.size())
            .disabled(options.is_disabled())
            .when(spec.sensitive, |input| input.mask_toggle())
            .w_full()
    })
    .on_reset(
        move |cx| read_value(cx, spec) != default,
        move |_, cx| write_value(cx, spec, reset_to.clone()),
    )
}

struct TextFieldState {
    input: Entity<InputState>,
    /// The field's value as last shown or applied.
    synced: SharedString,
    /// Why the text in the box is not applied.
    error: Option<String>,
    _subscription: Subscription,
}

impl TextFieldState {
    fn new(
        spec: &'static FieldSpec,
        value: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(spec.sensitive)
                .default_value(value.clone())
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            move |state: &mut Self, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let text = input.read(cx).value().to_string();
                    match storage::enqueue(Edit::Value(spec.path, text), cx) {
                        Ok(()) => {
                            state.synced = read_value(cx, spec).into();
                            state.error = None;
                        }
                        Err(error) => state.error = Some(error),
                    }
                }
                InputEvent::Blur | InputEvent::PressEnter { .. } => {
                    if let Some(error) = state.error.take() {
                        notify(
                            cx,
                            Notification::error(error)
                                .title(format!("Couldn’t change {}", spec.label)),
                        );
                        let value = state.synced.clone();
                        input.update(cx, |input, cx| input.set_value(value, window, cx));
                    }
                }
                InputEvent::Focus => {}
            },
        );
        Self {
            input,
            synced: value,
            error: None,
            _subscription: subscription,
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::prelude::v1::test;

    thread_local! {
        /// Schema fields built into Settings items on this thread.
        static RENDERED: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    pub(in crate::settings) fn record(path: &'static str) {
        RENDERED.with(|rendered| rendered.borrow_mut().push(path));
    }

    /// Run `build` and return the schema fields it turned into items.
    pub(in crate::settings) fn rendered_by(build: impl FnOnce()) -> Vec<&'static str> {
        RENDERED.with(|rendered| rendered.borrow_mut().clear());
        build();
        RENDERED.with(|rendered| rendered.take())
    }

    #[rstest::rstest]
    #[case("auto_select", "Auto select")]
    #[case("trace", "Trace")]
    #[case("", "")]
    fn enum_options_read_as_words(#[case] value: &str, #[case] label: &str) {
        assert_eq!(option_label(value), label);
    }

    #[test]
    fn floats_step_finely_through_small_ranges() {
        assert_eq!(float_step(0.0, 1.0), 0.05);
        assert_eq!(float_step(0.0, 2.0), 0.05);
        assert_eq!(float_step(0.0, 100.0), 1.0);
    }

    #[test]
    fn cli_only_fields_live_on_the_terminal_page() {
        for path in [
            "autocomplete_enabled",
            "explain_mode_default",
            "pager_command",
            "show_banner",
            "history.per_session_enabled",
        ] {
            let spec = config_editor::find_spec(path).expect(path);
            assert_eq!(page_for(spec), SettingsPage::Terminal, "{path}");
        }
    }
}
