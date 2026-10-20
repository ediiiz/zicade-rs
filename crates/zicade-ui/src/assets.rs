//! The asset source: the components' default icons plus the few the app adds.
//! Embedding only these (not the whole icon catalog) keeps the exe small.

use std::borrow::Cow;

use gpui_kit::assets::icon_assets;
use gpui_kit::{AssetSource, Result, SharedString};

icon_assets!(
    AppIcons,
    [
        LayoutDashboard,
        ArrowLeftRight,
        ScrollText,
        Settings,
        ArrowDown,
        ArrowUp,
        Search,
    ]
);

/// Default component icons first, then the app's own.
pub(crate) struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match AppIcons.load(path) {
            Ok(Some(bytes)) => Ok(Some(bytes)),
            _ => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = gpui_kit::assets::Assets.list(path)?;
        all.extend(AppIcons.list(path).unwrap_or_default());
        Ok(all)
    }
}
