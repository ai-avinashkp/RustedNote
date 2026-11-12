//! Theme palettes. Colors are stored as raw `0xRRGGBB` values so this module
//! stays free of any UI-framework types; the view layer converts to
//! `gpui::rgb(...)` at render time.

use crate::prefs::ThemeChoice;

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub background: u32,
    pub surface: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub selection: u32,
    pub tab_active: u32,
    pub tab_inactive: u32,
    pub border: u32,
    pub gutter: u32,
}

impl Palette {
    pub fn for_choice(choice: ThemeChoice) -> Palette {
        // `System` should be resolved to Light/Dark before reaching here; fall
        // back to Light if it wasn't.
        match choice.resolve(false) {
            ThemeChoice::System | ThemeChoice::Light => Palette {
                background: 0xffffff,
                surface: 0xf3f3f3,
                text: 0x1f1f1f,
                muted: 0x6b6b6b,
                accent: 0x2563eb,
                selection: 0xbfdbfe,
                tab_active: 0xffffff,
                tab_inactive: 0xe5e5e5,
                border: 0xd4d4d4,
                gutter: 0x9a9a9a,
            },
            // Warm, low-blue "paper" tone — reduces eye strain in long sessions.
            ThemeChoice::SepiaWarm => Palette {
                background: 0xf4ecd8,
                surface: 0xece0c8,
                text: 0x40361f,
                muted: 0x8a7a5c,
                accent: 0xb5651d,
                selection: 0xe6d3a3,
                tab_active: 0xf4ecd8,
                tab_inactive: 0xe0d2b4,
                border: 0xcdbc9c,
                gutter: 0xa8946e,
            },
            // Soft dark: dark-gray base instead of pure black to avoid halation.
            ThemeChoice::DarkSoft => Palette {
                background: 0x1e1e2e,
                surface: 0x28283a,
                text: 0xcdd6f4,
                muted: 0x9399b2,
                accent: 0x89b4fa,
                selection: 0x45475a,
                tab_active: 0x28283a,
                tab_inactive: 0x181825,
                border: 0x313244,
                gutter: 0x6c7086,
            },
            ThemeChoice::HighContrast => Palette {
                background: 0x000000,
                surface: 0x0a0a0a,
                text: 0xffffff,
                muted: 0xd0d0d0,
                accent: 0xffff00,
                selection: 0x264f78,
                tab_active: 0x000000,
                tab_inactive: 0x1a1a1a,
                border: 0xffffff,
                gutter: 0xc0c0c0,
            },
        }
    }
}
