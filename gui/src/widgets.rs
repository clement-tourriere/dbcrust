//! Small shared presentation helpers (lead-owned: add view-specific helpers
//! to your own module instead of here).

use dbcrust_gui_core::DatabaseType;
use dbcrust_gui_core::launcher::{EnvColor, EnvTag};
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::*;

/// Short monogram for a database engine (used in badges).
pub fn db_monogram(db_type: &DatabaseType) -> &'static str {
    match db_type {
        DatabaseType::PostgreSQL => "PG",
        DatabaseType::MySQL => "My",
        DatabaseType::SQLite => "SL",
        DatabaseType::ClickHouse => "CH",
        DatabaseType::MongoDB => "Mo",
        DatabaseType::Elasticsearch => "ES",
        DatabaseType::WhiteDragon => "WD",
        DatabaseType::Redis => "Rd",
        DatabaseType::SqlServer => "SQ",
        DatabaseType::Redshift => "RS",
        DatabaseType::LibSql => "Tu",
        DatabaseType::InfluxDB => "In",
        DatabaseType::Parquet => "Pq",
        DatabaseType::CSV => "CSV",
        DatabaseType::JSON => "JS",
        DatabaseType::DuckDB => "Dk",
    }
}

/// Brand-ish hue (0–360) per engine, used for the monogram badge.
pub fn db_hue(db_type: &DatabaseType) -> f32 {
    match db_type {
        DatabaseType::PostgreSQL => 212.,
        DatabaseType::MySQL => 196.,
        DatabaseType::SQLite => 205.,
        DatabaseType::ClickHouse => 50.,
        DatabaseType::MongoDB => 125.,
        DatabaseType::Elasticsearch => 175.,
        DatabaseType::WhiteDragon => 270.,
        DatabaseType::Redis => 5.,
        DatabaseType::SqlServer => 230.,
        DatabaseType::Redshift => 15.,
        DatabaseType::LibSql => 175.,
        DatabaseType::InfluxDB => 300.,
        DatabaseType::Parquet => 25.,
        DatabaseType::CSV => 150.,
        DatabaseType::JSON => 40.,
        DatabaseType::DuckDB => 55.,
    }
}

/// A compact rounded badge with the engine monogram.
pub fn db_badge(db_type: &DatabaseType, cx: &App) -> Div {
    db_badge_sized(db_type, 18., 10., cx)
}

/// [`db_badge`] for dense strips (tabs): 14 px high, 9 px monogram.
pub fn db_badge_mini(db_type: &DatabaseType, cx: &App) -> Div {
    db_badge_sized(db_type, 14., 9., cx)
        .min_w(px(18.))
        .px(px(3.))
}

fn db_badge_sized(db_type: &DatabaseType, height: f32, text: f32, cx: &App) -> Div {
    let hue = db_hue(db_type) / 360.;
    let dark = cx.theme().is_dark();
    let (bg, fg) = if dark {
        (hsla(hue, 0.55, 0.22, 1.), hsla(hue, 0.85, 0.78, 1.))
    } else {
        (hsla(hue, 0.70, 0.92, 1.), hsla(hue, 0.75, 0.32, 1.))
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .min_w(px(22.))
        .h(px(height))
        .px(px(4.))
        .rounded(px(4.))
        .bg(bg)
        .text_color(fg)
        .text_size(px(text))
        .font_weight(FontWeight::BOLD)
        .child(db_monogram(db_type))
}

/// Identity color of a live connection (its `accent` slot), readable in
/// both light and dark themes.
pub fn connection_accent(slot: usize, cx: &App) -> Hsla {
    const HUES: [f32; 8] = [210., 145., 30., 280., 350., 180., 55., 320.];
    let hue = HUES[slot % HUES.len()] / 360.;
    if cx.theme().is_dark() {
        hsla(hue, 0.70, 0.62, 1.)
    } else {
        hsla(hue, 0.65, 0.45, 1.)
    }
}

/// A larger rounded badge for lists and headers: `size` px square with the
/// engine monogram.
pub fn engine_badge(db_type: &DatabaseType, size: f32, cx: &App) -> Div {
    monogram_badge(db_monogram(db_type), db_hue(db_type), size, cx)
}

/// [`engine_badge`] for sources that are not an engine (Docker, Vault).
pub fn source_badge(monogram: &'static str, hue: f32, size: f32, cx: &App) -> Div {
    monogram_badge(monogram, hue, size, cx)
}

fn monogram_badge(monogram: &'static str, hue: f32, size: f32, cx: &App) -> Div {
    let hue = hue / 360.;
    let (bg, fg) = if cx.theme().is_dark() {
        (hsla(hue, 0.50, 0.20, 1.), hsla(hue, 0.80, 0.76, 1.))
    } else {
        (hsla(hue, 0.65, 0.92, 1.), hsla(hue, 0.75, 0.30, 1.))
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px((size * 0.25).round()))
        .bg(bg)
        .text_color(fg)
        .text_size(px((size * 0.34).round().max(9.)))
        .font_weight(FontWeight::BOLD)
        .child(monogram)
}

/// An environment tag (`Production`, red): always label plus colour.
pub fn env_tag(tag: &EnvTag, cx: &App) -> Div {
    let hue = match tag.color {
        EnvColor::Green => 145.,
        EnvColor::Blue => 215.,
        EnvColor::Amber => 38.,
        EnvColor::Red => 2.,
        EnvColor::Purple => 275.,
        EnvColor::Gray => 220.,
    };
    let saturation = if tag.color == EnvColor::Gray {
        0.08
    } else {
        0.65
    };
    let hue = hue / 360.;
    let dark = cx.theme().is_dark();
    let (bg, fg) = if dark {
        (
            hsla(hue, saturation, 0.22, 1.),
            hsla(hue, saturation + 0.1, 0.74, 1.),
        )
    } else {
        (
            hsla(hue, saturation + 0.1, 0.93, 1.),
            hsla(hue, saturation, 0.30, 1.),
        )
    };
    h_flex()
        .flex_none()
        .items_center()
        .gap(px(5.))
        .h(px(18.))
        .px(px(6.))
        .rounded(px(9.))
        .bg(bg)
        .text_color(fg)
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .child(div().size(px(6.)).rounded_full().bg(fg))
        .child(tag.label.clone())
}

/// The swatch of an environment colour (no text: the form pairs it with the
/// colour's name).
pub fn env_swatch(color: EnvColor, cx: &App) -> Hsla {
    let hue = match color {
        EnvColor::Green => 145.,
        EnvColor::Blue => 215.,
        EnvColor::Amber => 38.,
        EnvColor::Red => 2.,
        EnvColor::Purple => 275.,
        EnvColor::Gray => 220.,
    } / 360.;
    let saturation = if color == EnvColor::Gray { 0.08 } else { 0.68 };
    if cx.theme().is_dark() {
        hsla(hue, saturation, 0.62, 1.)
    } else {
        hsla(hue, saturation, 0.48, 1.)
    }
}
