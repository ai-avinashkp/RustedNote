//! System font enumeration via GPUI's platform text system.

use gpui::{SharedString, TextSystem};

/// Sorted, de-duplicated list of installed font family names.
pub fn system_fonts(text_system: &TextSystem) -> Vec<SharedString> {
    let mut names = text_system.all_font_names();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    // Drop the ".SomeHidden" system faces that clutter the picker.
    names
        .into_iter()
        .filter(|n| !n.starts_with('.'))
        .map(SharedString::from)
        .collect()
}
