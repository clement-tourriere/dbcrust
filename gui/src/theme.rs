//! Light / dark / system appearance and the bundled color themes.
//!
//! The choice (`ThemeModeSetting` + an optional theme name) lives in a GPUI
//! global ([`Appearance`]), is applied through GPUI Kit's `Theme::update` /
//! `Theme::change`, and is persisted in `gui_state.toml`.
//!
//! A theme name only applies when its mode matches the effective mode:
//! with *System* appearance, picking "Tokyo Night" (dark) gives Tokyo Night
//! whenever the OS is dark and the default light theme otherwise.

use dbcrust_gui_core::gui_state::{GuiState, ThemeModeSetting};
use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Theme, ThemeColor, ThemeMode, ThemeRegistry,
};
use gpui_kit::*;

/// Themes shipped with the app (GPUI Kit's theme collection, Apache-2.0 —
/// see `assets/themes/README.md`).
const BUNDLED_THEMES: [&str; 4] = [
    include_str!("../assets/themes/catppuccin.json"),
    include_str!("../assets/themes/tokyonight.json"),
    include_str!("../assets/themes/gruvbox.json"),
    include_str!("../assets/themes/solarized.json"),
];

/// The user's appearance choice.
#[derive(Clone, Debug, PartialEq)]
pub struct Appearance {
    pub mode: ThemeModeSetting,
    /// A theme from the registry; ignored when its mode differs from the
    /// effective mode.
    pub theme_name: Option<SharedString>,
    /// Base font size in pixels; the interface scales with it.
    pub ui_font_size: f32,
}

impl Global for Appearance {}

/// A selectable theme.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeChoice {
    pub name: SharedString,
    pub mode: ThemeMode,
}

/// Register the bundled themes, restore the saved choice and apply it. Call
/// after `gpui_kit::init`.
pub fn init(state: &GuiState, cx: &mut App) {
    for json in BUNDLED_THEMES {
        if let Err(error) = ThemeRegistry::global_mut(cx).load_themes_from_str(json) {
            tracing::error!("invalid bundled theme: {error}");
        }
    }
    cx.set_global(Appearance {
        mode: state.theme_mode,
        theme_name: state.theme_name.clone().map(SharedString::from),
        ui_font_size: state.ui_font_size,
    });
    apply(cx);
}

pub fn appearance(cx: &App) -> Appearance {
    cx.global::<Appearance>().clone()
}

/// Light or dark, after resolving *System* against the OS appearance.
pub fn effective_mode(setting: ThemeModeSetting, system: WindowAppearance) -> ThemeMode {
    match setting {
        ThemeModeSetting::System => ThemeMode::from(system),
        ThemeModeSetting::Light => ThemeMode::Light,
        ThemeModeSetting::Dark => ThemeMode::Dark,
    }
}

/// Every registered theme: the two defaults first, then the rest by mode
/// (light first) and name.
pub fn available_themes(cx: &App) -> Vec<ThemeChoice> {
    ThemeRegistry::global(cx)
        .sorted_themes()
        .into_iter()
        .map(|config| ThemeChoice {
            name: config.name.clone(),
            mode: config.mode,
        })
        .collect()
}

/// The registered default theme of `mode`.
pub fn default_theme_name(mode: ThemeMode, cx: &App) -> SharedString {
    let registry = ThemeRegistry::global(cx);
    match mode {
        ThemeMode::Light => registry.default_light_theme().name.clone(),
        ThemeMode::Dark => registry.default_dark_theme().name.clone(),
    }
}

pub fn set_mode(mode: ThemeModeSetting, cx: &mut App) {
    cx.update_global::<Appearance, _>(|appearance, _| appearance.mode = mode);
    apply(cx);
    persist(cx);
}

/// Choose a theme (`None`: the default of the current mode). When the theme
/// belongs to the other mode and the mode is explicit, the mode follows so
/// the choice is visible right away.
pub fn set_theme(name: Option<SharedString>, cx: &mut App) {
    let theme_mode = name.as_ref().and_then(|name| {
        ThemeRegistry::global(cx)
            .themes()
            .get(name)
            .map(|config| config.mode)
    });
    let system = cx.window_appearance();
    cx.update_global::<Appearance, _>(|appearance, _| {
        // A default theme is simply "no custom theme"
        appearance.theme_name = name.clone();
        if let Some(theme_mode) = theme_mode
            && appearance.mode != ThemeModeSetting::System
            && effective_mode(appearance.mode, system) != theme_mode
        {
            appearance.mode = if theme_mode.is_dark() {
                ThemeModeSetting::Dark
            } else {
                ThemeModeSetting::Light
            };
        }
    });
    apply(cx);
    persist(cx);
}

/// Scale the whole interface by changing the base font size (in pixels).
pub fn set_ui_font_size(size: f32, cx: &mut App) {
    let size = size.clamp(GuiState::MIN_UI_FONT_SIZE, GuiState::MAX_UI_FONT_SIZE);
    cx.update_global::<Appearance, _>(|appearance, _| appearance.ui_font_size = size);
    apply(cx);
    persist(cx);
}

/// Flip between light and dark, making the choice explicit (⌘⌥T, the title
/// bar button).
pub fn toggle(cx: &mut App) {
    let next = if Theme::global(cx).is_dark() {
        ThemeModeSetting::Light
    } else {
        ThemeModeSetting::Dark
    };
    set_mode(next, cx);
}

/// The OS appearance changed: follow it when the mode is *System*.
pub fn on_system_appearance_changed(cx: &mut App) {
    if cx.global::<Appearance>().mode == ThemeModeSetting::System {
        apply(cx);
    }
}

fn apply(cx: &mut App) {
    let Appearance {
        mode,
        theme_name,
        ui_font_size,
    } = appearance(cx);
    let target = effective_mode(mode, cx.window_appearance());
    let registry = ThemeRegistry::global(cx);
    let light_default = registry.default_light_theme().clone();
    let dark_default = registry.default_dark_theme().clone();
    let chosen = theme_name
        .and_then(|name| registry.themes().get(&name).cloned())
        .filter(|config| config.mode == target);

    Theme::update(cx, |theme| {
        theme.light_theme = match (&chosen, target) {
            (Some(config), ThemeMode::Light) => config.clone(),
            _ => light_default,
        };
        theme.dark_theme = match (&chosen, target) {
            (Some(config), ThemeMode::Dark) => config.clone(),
            _ => dark_default,
        };
    });
    // Reload the colors of the (possibly swapped) theme for this mode
    Theme::change(target, None, cx);
    // `change` reloads the theme file's size (16 by default); ours wins, and
    // the surface hierarchy is laid over whichever theme was loaded
    Theme::update(cx, |theme| {
        theme.font_size = px(ui_font_size);
        refine_surfaces(&mut theme.colors, target.is_dark());
    });
}

// ---------------------------------------------------------------------------
// Surface hierarchy
// ---------------------------------------------------------------------------
//
// GPUI Kit's default themes paint the sidebar, the canvas and group boxes the
// same color, give selections barely more than a hover tint and leave muted
// labels at ~2.6:1. Instead of forking the theme files (and the bundled ones),
// the app lays a small, theme-independent pass over whichever theme is loaded:
//
//   canvas (background)  <  sidebar  <  bars (title / status / tabs)  <  raised
//
// Themes that already separate their surfaces keep their own colors; only
// what falls short of the thresholds below is adjusted.

/// WCAG contrast the muted labels and table headers must reach on the plain
/// surfaces (canvas, sidebar, table headers).
const MIN_LABEL_CONTRAST: f32 = 5.0;
/// ... and on highlighted ones (selected / hovered rows, cards, the palette's
/// selection), where small text must still reach the AA threshold.
const MIN_ROW_CONTRAST: f32 = 4.5;
/// Lightness the sidebar must differ from the canvas by.
const MIN_SIDEBAR_STEP: f32 = 0.02;
/// Lightness step used when the sidebar has to be pushed off the canvas.
const SIDEBAR_STEP: f32 = 0.035;
/// A selected / highlighted row must differ from the surface it sits on by
/// at least this much lightness.
const MIN_SELECTION_STEP: f32 = 0.13;
/// How much of the selection blue tints a strengthened highlight.
const SELECTION_TINT: f32 = 0.3;

/// Surfaces for the app's own views, derived from the active theme (so they
/// follow light / dark and every bundled theme). Prefer these over
/// hand-mixed colors.
#[derive(Clone, Copy, Debug)]
pub struct Surfaces {
    /// Main content area (editor, results, inspector body).
    pub canvas: Hsla,
    /// Side panels (the explorer).
    pub sidebar: Hsla,
    /// Cards and controls that sit on the canvas.
    pub raised: Hsla,
    pub raised_border: Hsla,
    /// A selected row of a side-panel list that has focus / lost it, and a
    /// hovered one. Opaque (painted over the sidebar), so an overlay can mask
    /// text with exactly the row's color.
    pub selected: Hsla,
    pub selected_inactive: Hsla,
    pub hover: Hsla,
    /// The bar marking a selected row.
    pub selected_edge: Hsla,
    /// Section headers and column labels: muted, but readable.
    pub label: Hsla,
}

pub fn surfaces(cx: &App) -> Surfaces {
    Surfaces::of(cx.theme())
}

impl Surfaces {
    pub fn of(theme: &Theme) -> Self {
        Self::from_colors(&theme.colors, theme.is_dark())
    }

    fn from_colors(colors: &ThemeColor, dark: bool) -> Self {
        let canvas = colors.background;
        let sidebar = colors.sidebar;
        Self {
            canvas,
            sidebar,
            raised: shift_lightness(canvas, if dark { 0.05 } else { -0.022 }),
            raised_border: colors.border,
            selected: over(sidebar, colors.accent),
            selected_inactive: over(sidebar, colors.accent.opacity(0.5)),
            hover: over(sidebar, colors.accent.opacity(0.35)),
            selected_edge: colors.blue,
            label: colors.muted_foreground,
        }
    }

    /// The selection and hover surfaces, for text that sits on them.
    fn highlight_targets(&self, colors: &ThemeColor) -> Vec<(Hsla, f32)> {
        let popover = over(colors.background, colors.popover);
        vec![
            (self.selected, MIN_ROW_CONTRAST),
            (self.selected_inactive, MIN_ROW_CONTRAST),
            (self.hover, MIN_ROW_CONTRAST),
            (over(popover, colors.accent), MIN_ROW_CONTRAST),
        ]
    }

    /// Every surface a muted label or small badge can sit on, with the
    /// contrast it must reach there: the plain surfaces, rows that are
    /// selected, hovered or in a card, and the palette's selection.
    fn text_targets(&self, colors: &ThemeColor) -> Vec<(Hsla, f32)> {
        let popover = over(colors.background, colors.popover);
        vec![
            (self.canvas, MIN_LABEL_CONTRAST),
            (self.sidebar, MIN_LABEL_CONTRAST),
            (
                over(colors.background, colors.table_head),
                MIN_LABEL_CONTRAST,
            ),
            (
                over(colors.background, colors.list_head),
                MIN_LABEL_CONTRAST,
            ),
            (self.selected, MIN_ROW_CONTRAST),
            (self.selected_inactive, MIN_ROW_CONTRAST),
            (self.hover, MIN_ROW_CONTRAST),
            (self.raised, MIN_ROW_CONTRAST),
            (over(popover, colors.accent), MIN_ROW_CONTRAST),
            (
                over(colors.background, colors.list_active),
                MIN_ROW_CONTRAST,
            ),
            (over(colors.background, colors.list_hover), MIN_ROW_CONTRAST),
        ]
    }
}

/// Text color for a small label tinted with `color` (PK, FK, UNIQUE, …): the
/// color itself where it reads on its own 14% tint on every surface a row can
/// have (plain, selected, hovered, in a card), darker or lighter where it
/// would not (yellow on pale yellow in the light theme).
pub fn tint_text(color: Hsla, cx: &App) -> Hsla {
    let surfaces = Surfaces::of(cx.theme());
    let row_surfaces = [
        surfaces.canvas,
        surfaces.sidebar,
        surfaces.selected,
        surfaces.selected_inactive,
        surfaces.hover,
        surfaces.raised,
    ];
    // The contrast search is far too slow to run for every chip of every
    // frame, and its answer only changes with the theme
    TINTS.with_borrow_mut(|tints| tints.get(color, row_surfaces))
}

thread_local! {
    static TINTS: std::cell::RefCell<TintCache> = Default::default();
}

/// [`tint_text`] results, for the row surfaces they were computed on.
#[derive(Default)]
struct TintCache {
    surfaces: Option<[Hsla; 6]>,
    tints: Vec<(Hsla, Hsla)>,
}

impl TintCache {
    fn get(&mut self, color: Hsla, surfaces: [Hsla; 6]) -> Hsla {
        if self.surfaces != Some(surfaces) {
            self.surfaces = Some(surfaces);
            self.tints.clear();
        }
        // A handful of colors (PK, FK, NN, info…): a scan beats hashing floats
        if let Some((_, tint)) = self.tints.iter().find(|(key, _)| *key == color) {
            return *tint;
        }
        let targets: Vec<(Hsla, f32)> = surfaces
            .into_iter()
            .map(|surface| (over(surface, color.opacity(0.14)), MIN_ROW_CONTRAST))
            .collect();
        let tint = ensure_contrast(color, &targets);
        self.tints.push((color, tint));
        tint
    }
}

/// Lay the surface hierarchy over `colors` (see the section comment).
pub fn refine_surfaces(colors: &mut ThemeColor, dark: bool) {
    // The sidebar is its own surface, not the canvas again
    if (colors.sidebar.l - colors.background.l).abs() < MIN_SIDEBAR_STEP {
        colors.sidebar = shift_lightness(
            colors.background,
            if dark { SIDEBAR_STEP } else { -SIDEBAR_STEP },
        );
    }
    colors.sidebar_border = colors.border;

    // Group boxes and secondary surfaces read as raised, not as the canvas
    if (colors.group_box.l - colors.background.l).abs() < MIN_SIDEBAR_STEP {
        colors.group_box = shift_lightness(colors.background, if dark { 0.05 } else { -0.022 });
    }

    // A highlighted row is clearly different from the surface under it
    let base = over(colors.background, colors.popover);
    if (colors.accent.l - base.l).abs() < MIN_SELECTION_STEP {
        let direction = if dark { 1. } else { -1. };
        let stronger = shift_lightness(base, direction * MIN_SELECTION_STEP);
        let mut accent = stronger.mix_oklab(colors.selection, 1.0 - SELECTION_TINT);
        // The tint moves the lightness too: make up what is missing
        let missing = MIN_SELECTION_STEP - (accent.l - base.l).abs();
        if missing > 0. {
            accent = shift_lightness(accent, direction * (missing + 0.01));
        }
        colors.accent = accent;
        colors.accent_foreground = colors.foreground;
    }

    // Muted labels, table headers and body text stay readable on every
    // surface they can sit on, selected / hovered rows and cards included
    let targets = Surfaces::from_colors(colors, dark).text_targets(colors);
    colors.muted_foreground = ensure_contrast(colors.muted_foreground, &targets);
    colors.table_head_foreground = ensure_contrast(colors.table_head_foreground, &targets);
    colors.foreground = ensure_contrast(colors.foreground, &targets);
    colors.sidebar_foreground = ensure_contrast(colors.sidebar_foreground, &targets);
    // text on the highlight itself
    let highlights = Surfaces::from_colors(colors, dark).highlight_targets(colors);
    colors.accent_foreground = ensure_contrast(colors.accent_foreground, &highlights);

    // A scrim that is felt without hiding the page
    colors.overlay = hsla(0., 0., 0., if dark { 0.5 } else { 0.12 });
}

fn shift_lightness(color: Hsla, delta: f32) -> Hsla {
    Hsla {
        l: (color.l + delta).clamp(0., 1.),
        ..color
    }
}

/// `top` painted over the opaque `base`.
fn over(base: Hsla, top: Hsla) -> Hsla {
    let (base, top_rgb) = (base.to_rgb(), top.to_rgb());
    let a = top.a.clamp(0., 1.);
    Rgba {
        r: top_rgb.r * a + base.r * (1. - a),
        g: top_rgb.g * a + base.g * (1. - a),
        b: top_rgb.b * a + base.b * (1. - a),
        a: 1.,
    }
    .into()
}

/// WCAG relative luminance of an opaque color.
fn luminance(color: Hsla) -> f32 {
    let rgb = color.to_rgb();
    let linear = |c: f32| {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(rgb.r) + 0.7152 * linear(rgb.g) + 0.0722 * linear(rgb.b)
}

/// WCAG contrast ratio between two opaque colors (1 to 21).
pub fn contrast_ratio(a: Hsla, b: Hsla) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `foreground`, or when it falls short, the same hue moved along the
/// lightness axis (away from the surfaces) until it reaches the contrast
/// asked for on every one of the `(surface, minimum)` targets.
fn ensure_contrast(foreground: Hsla, targets: &[(Hsla, f32)]) -> Hsla {
    // how far the worst target is from its minimum (>= 0 once all are met)
    let shortfall = |color: Hsla| {
        targets
            .iter()
            .map(|(surface, min)| contrast_ratio(color, *surface) - min)
            .fold(f32::INFINITY, f32::min)
    };
    let mut color = Hsla {
        a: 1.,
        ..foreground
    };
    if shortfall(color) >= 0. {
        return foreground;
    }
    let mean_luminance =
        targets.iter().map(|(s, _)| luminance(*s)).sum::<f32>() / targets.len().max(1) as f32;
    let step = if mean_luminance > 0.4 { -0.01 } else { 0.01 };
    for _ in 0..100 {
        let next = shift_lightness(color, step);
        if (next.l - color.l).abs() < f32::EPSILON {
            break;
        }
        color = next;
        if shortfall(color) >= 0. {
            break;
        }
    }
    color
}

fn persist(cx: &mut App) {
    let Appearance {
        mode,
        theme_name,
        ui_font_size,
    } = appearance(cx);
    cx.background_spawn(async move {
        let saved = GuiState::update(|state| {
            state.ui_font_size = ui_font_size;
            state.theme_mode = mode;
            state.theme_name = theme_name.map(|name| name.to_string());
        });
        if let Err(error) = saved {
            tracing::warn!("could not save the theme choice: {error}");
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    // GPUI's `test` macro (glob-imported through `super::*`) would shadow
    // the built-in `#[test]`
    use std::prelude::v1::test;

    use super::*;
    use gpui_kit::component::ThemeSet;

    fn bundled() -> Vec<ThemeSet> {
        BUNDLED_THEMES
            .iter()
            .map(|json| serde_json::from_str::<ThemeSet>(json).expect("valid theme set"))
            .collect()
    }

    #[test]
    fn bundled_themes_parse_and_cover_both_modes() {
        let sets = bundled();
        let themes: Vec<_> = sets.iter().flat_map(|set| set.themes.iter()).collect();
        assert!(themes.len() >= 8);
        assert!(themes.iter().any(|theme| theme.mode == ThemeMode::Light));
        assert!(themes.iter().any(|theme| theme.mode == ThemeMode::Dark));
        assert!(themes.iter().all(|theme| !theme.is_default));
    }

    #[test]
    fn bundled_theme_names_are_unique_and_not_the_defaults() {
        let sets = bundled();
        let mut names = std::collections::HashSet::new();
        for theme in sets.iter().flat_map(|set| set.themes.iter()) {
            assert!(
                names.insert(theme.name.to_string()),
                "duplicate {}",
                theme.name
            );
            assert_ne!(theme.name.as_ref(), "Default Light");
            assert_ne!(theme.name.as_ref(), "Default Dark");
        }
    }

    /// Every text color against every surface it can really sit on: the
    /// canvas, the sidebar, table headers, selected / hovered rows, cards, the
    /// palette's selection.
    fn assert_readable(colors: &ThemeColor, dark: bool, name: &str) {
        let surfaces = Surfaces::from_colors(colors, dark);
        for (surface, min) in surfaces.text_targets(colors) {
            for (label, text) in [
                ("muted labels", colors.muted_foreground),
                ("table headers", colors.table_head_foreground),
                ("text", colors.foreground),
                ("sidebar text", colors.sidebar_foreground),
            ] {
                let ratio = contrast_ratio(text, surface);
                assert!(
                    ratio >= min - 0.05,
                    "{name}: {label} reach only {ratio:.2}:1 (need {min}) on {}",
                    surface.to_hex()
                );
            }
        }
        for (surface, min) in surfaces.highlight_targets(colors) {
            let ratio = contrast_ratio(colors.accent_foreground, surface);
            assert!(
                ratio >= min - 0.05,
                "{name}: text on a highlight reaches only {ratio:.2}:1 on {}",
                surface.to_hex()
            );
        }
    }

    fn assert_hierarchy(colors: &ThemeColor, dark: bool, name: &str) {
        assert_readable(colors, dark, name);
        assert!(
            (colors.sidebar.l - colors.background.l).abs() >= MIN_SIDEBAR_STEP - 0.001,
            "{name}: sidebar and canvas are the same surface"
        );
        let base = over(colors.background, colors.popover);
        assert!(
            (colors.accent.l - base.l).abs() >= MIN_SELECTION_STEP - 0.001,
            "{name}: highlighted rows barely differ from their surface"
        );
        assert!(
            colors.overlay.a >= 0.1 && colors.overlay.a <= 0.6,
            "{name}: scrim"
        );
    }

    #[rstest::rstest]
    #[case(false)]
    #[case(true)]
    fn the_default_themes_get_a_visible_surface_hierarchy(#[case] dark: bool) {
        let mut colors = *if dark {
            ThemeColor::dark()
        } else {
            ThemeColor::light()
        };
        refine_surfaces(&mut colors, dark);
        assert_hierarchy(
            &colors,
            dark,
            if dark {
                "default dark"
            } else {
                "default light"
            },
        );
        // sidebar: lighter than the canvas in dark, darker in light
        assert_eq!(colors.sidebar.l > colors.background.l, dark);
    }

    #[test]
    fn a_dim_muted_label_is_lifted_until_it_reads_on_selected_rows() {
        // a Tokyo-Storm-like label: fine on the canvas for a designer, about
        // 3.5:1 on a selected row
        let mut colors = *ThemeColor::dark();
        colors.muted_foreground = hsla(0.65, 0.2, 0.38, 1.);
        let before = colors.muted_foreground.l;
        refine_surfaces(&mut colors, true);
        assert!(colors.muted_foreground.l > before);
        assert_readable(&colors, true, "dim muted label");
    }

    #[test]
    fn refining_twice_changes_nothing() {
        let mut once = *ThemeColor::dark();
        refine_surfaces(&mut once, true);
        let mut twice = once;
        refine_surfaces(&mut twice, true);
        assert_eq!(once, twice);
    }

    #[test]
    fn a_theme_that_already_separates_its_surfaces_keeps_them() {
        let mut colors = *ThemeColor::dark();
        colors.sidebar = hsla(0.6, 0.3, 0.2, 1.);
        colors.muted_foreground = hsla(0., 0., 0.9, 1.);
        let (sidebar, muted) = (colors.sidebar, colors.muted_foreground);
        refine_surfaces(&mut colors, true);
        assert_eq!(colors.sidebar, sidebar);
        assert_eq!(colors.muted_foreground, muted);
    }

    #[test]
    fn contrast_ratio_matches_wcag() {
        let white = hsla(0., 0., 1., 1.);
        let black = hsla(0., 0., 0., 1.);
        assert!((contrast_ratio(white, black) - 21.0).abs() < 0.05);
        assert!((contrast_ratio(white, white) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn ensure_contrast_moves_away_from_the_surface() {
        let dim = hsla(0., 0., 0.32, 1.);
        let dark_surface = hsla(0., 0., 0.04, 1.);
        let fixed = ensure_contrast(dim, &[(dark_surface, 5.0)]);
        assert!(fixed.l > dim.l);
        assert!(contrast_ratio(fixed, dark_surface) >= 5.0);

        let light_surface = hsla(0., 0., 1., 1.);
        let pale = hsla(0., 0., 0.6, 1.);
        let fixed = ensure_contrast(pale, &[(light_surface, 5.0)]);
        assert!(fixed.l < pale.l);
        assert!(contrast_ratio(fixed, light_surface) >= 5.0);
    }

    #[gpui_kit::test]
    fn every_registered_theme_keeps_the_hierarchy(cx: &mut gpui_kit::TestAppContext) {
        crate::test_support::isolate();
        cx.update(|cx| {
            gpui_kit::init(cx);
            init(&GuiState::default(), cx);
            for choice in available_themes(cx) {
                let mode = if choice.mode.is_dark() {
                    ThemeModeSetting::Dark
                } else {
                    ThemeModeSetting::Light
                };
                cx.update_global::<Appearance, _>(|appearance, _| {
                    appearance.mode = mode;
                    appearance.theme_name = Some(choice.name.clone());
                });
                apply(cx);
                let colors = Theme::global(cx).colors;
                let dark = choice.mode.is_dark();
                assert_hierarchy(&colors, dark, &choice.name);
                // measured, for the record (`cargo test … -- --nocapture`)
                let worst = |text: Hsla| {
                    Surfaces::from_colors(&colors, dark)
                        .text_targets(&colors)
                        .into_iter()
                        .map(|(surface, _)| contrast_ratio(text, surface))
                        .fold(f32::INFINITY, f32::min)
                };
                eprintln!(
                    "contrast {:<18} muted {:.2}  headers {:.2}  text {:.2}",
                    choice.name,
                    worst(colors.muted_foreground),
                    worst(colors.table_head_foreground),
                    worst(colors.foreground)
                );

                // the small tinted badges (PK, FK, kind tags, explain chips)
                // on the surface of every row they appear in
                let surfaces = surfaces(cx);
                for (label, tint) in [
                    ("warning", colors.warning),
                    ("info", colors.info),
                    ("success", colors.success),
                    ("danger", colors.danger),
                    ("muted", colors.muted_foreground),
                ] {
                    let text = tint_text(tint, cx);
                    for surface in [
                        surfaces.canvas,
                        surfaces.sidebar,
                        surfaces.selected,
                        surfaces.selected_inactive,
                        surfaces.hover,
                        surfaces.raised,
                    ] {
                        let ratio = contrast_ratio(text, over(surface, tint.opacity(0.14)));
                        assert!(
                            ratio >= MIN_ROW_CONTRAST - 0.05,
                            "{}: {label} badge text reaches only {ratio:.2}:1 on {}",
                            choice.name,
                            surface.to_hex()
                        );
                    }
                }
            }
        });
    }

    #[test]
    fn badge_tints_are_recomputed_for_new_surfaces() {
        let yellow = hsla(0.14, 0.9, 0.5, 1.);
        let light = [hsla(0., 0., 0.98, 1.); 6];
        let dark = [hsla(0., 0., 0.1, 1.); 6];
        let fresh = |surfaces: [Hsla; 6]| {
            let targets: Vec<(Hsla, f32)> = surfaces
                .into_iter()
                .map(|surface| (over(surface, yellow.opacity(0.14)), MIN_ROW_CONTRAST))
                .collect();
            ensure_contrast(yellow, &targets)
        };
        let mut cache = TintCache::default();
        assert_eq!(cache.get(yellow, light), fresh(light));
        assert_eq!(cache.get(yellow, light), fresh(light));
        assert_eq!(cache.get(yellow, dark), fresh(dark));
        assert_ne!(fresh(light), fresh(dark), "the case must tell them apart");
        assert_eq!(cache.tints.len(), 1);
    }

    #[test]
    fn system_mode_follows_the_os_appearance() {
        assert_eq!(
            effective_mode(ThemeModeSetting::System, WindowAppearance::Dark),
            ThemeMode::Dark
        );
        assert_eq!(
            effective_mode(ThemeModeSetting::System, WindowAppearance::VibrantLight),
            ThemeMode::Light
        );
        assert_eq!(
            effective_mode(ThemeModeSetting::Light, WindowAppearance::Dark),
            ThemeMode::Light
        );
        assert_eq!(
            effective_mode(ThemeModeSetting::Dark, WindowAppearance::Light),
            ThemeMode::Dark
        );
    }
}
