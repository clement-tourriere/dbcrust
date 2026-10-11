//! Asset source of the app: the complete Lucide catalog from GPUI Kit
//! (`gpui_kit::assets::AllAssets`) plus the DBCrust mark.
//!
//! GPUI Kit's default `Assets` bundle ships only ~100 icons; views here use
//! many more (`IconName::House`, `Database`, `Table`, `Lock`, …), so every
//! entry point must register [`Assets`] (or [`assets`]) instead.

use gpui_kit::assets::AllAssets;
use gpui_kit::*;
use std::borrow::Cow;
use std::sync::{Arc, OnceLock};

/// Asset path of the DBCrust mark, for `img(BRAND_MARK_PATH)` /
/// `svg().path(..)` when this source is registered.
pub const BRAND_MARK_PATH: &str = "brand/dbcrust-icon.svg";

static BRAND_MARK_SVG: &[u8] = include_bytes!("../assets/brand/dbcrust-icon.svg");

/// All Lucide icons plus the DBCrust mark.
#[derive(Clone, Copy, Debug, Default)]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == BRAND_MARK_PATH {
            return Ok(Some(Cow::Borrowed(BRAND_MARK_SVG)));
        }
        AllAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut entries = AllAssets.list(path)?;
        if BRAND_MARK_PATH.starts_with(path) {
            entries.push(BRAND_MARK_PATH.into());
        }
        Ok(entries)
    }
}

/// The asset source every entry point (`main.rs`, the screenshot harness)
/// registers with the application.
pub fn assets() -> impl AssetSource {
    Assets
}

/// The DBCrust mark as an element, independent of the registered asset
/// source (it is embedded, so it also renders under a plain
/// `gpui_kit::assets::Assets`).
pub fn brand_mark(size: Pixels) -> Img {
    static IMAGE: OnceLock<Arc<Image>> = OnceLock::new();
    let image = IMAGE
        .get_or_init(|| Arc::new(Image::from_bytes(ImageFormat::Svg, BRAND_MARK_SVG.to_vec())))
        .clone();
    img(image).size(size).flex_none()
}

#[cfg(test)]
mod tests {
    // GPUI's `test` macro (glob-imported through `super::*`) would shadow
    // the built-in `#[test]`
    use std::prelude::v1::test;

    use super::*;

    #[test]
    fn serves_the_brand_mark_and_lucide_icons() {
        let brand = Assets
            .load(BRAND_MARK_PATH)
            .expect("load")
            .expect("brand mark");
        assert!(brand.starts_with(b"<svg"));
        // Not part of GPUI Kit's default bundle, but in the full catalog
        let house = Assets.load("icons/house.svg").expect("load");
        assert!(house.is_some());
    }

    #[test]
    fn lists_the_brand_mark() {
        let all = Assets.list("brand").expect("list");
        assert!(all.iter().any(|path| path.as_ref() == BRAND_MARK_PATH));
    }
}
