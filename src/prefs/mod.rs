//! User preferences: font, theme, tab layout, zoom, and session options.
//! Serialized to `preferences.json` in the config directory.

use serde::{Deserialize, Serialize};

pub const PREFS_FILE: &str = "preferences.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeChoice {
    /// Follow the operating system's light/dark appearance.
    System,
    Light,
    /// Warm, low-blue-light "paper" tone — an eye-comfort option.
    SepiaWarm,
    /// Soft dark (not pure black) to reduce halation/eye strain.
    DarkSoft,
    HighContrast,
}

impl Default for ThemeChoice {
    fn default() -> Self {
        ThemeChoice::System
    }
}

impl ThemeChoice {
    /// Resolve `System` to a concrete choice using the OS appearance;
    /// all other choices pass through unchanged.
    pub fn resolve(self, system_dark: bool) -> ThemeChoice {
        match self {
            ThemeChoice::System => {
                if system_dark {
                    ThemeChoice::DarkSoft
                } else {
                    ThemeChoice::Light
                }
            }
            other => other,
        }
    }

    /// The next mode when cycling the primary System → Light → Dark toggle.
    pub fn cycle_primary(self) -> ThemeChoice {
        match self {
            ThemeChoice::System => ThemeChoice::Light,
            ThemeChoice::Light => ThemeChoice::DarkSoft,
            _ => ThemeChoice::System,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "System",
            ThemeChoice::Light => "Light",
            ThemeChoice::SepiaWarm => "Sepia",
            ThemeChoice::DarkSoft => "Dark",
            ThemeChoice::HighContrast => "Contrast",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TabDir {
    Horizontal,
    Vertical,
}

impl Default for TabDir {
    fn default() -> Self {
        TabDir::Horizontal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl Default for LineEnding {
    fn default() -> Self {
        // Sensible per-platform default.
        if cfg!(windows) {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        }
    }
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub font_family: String,
    pub font_size: f32,
    pub theme: ThemeChoice,
    pub tab_orientation: TabDir,
    pub word_wrap: bool,
    pub show_line_numbers: bool,
    /// Global UI+text zoom multiplier driven by Ctrl +/-/0.
    pub zoom_level: f32,
    pub restore_session: bool,
    pub default_encoding: String,
    pub line_ending: LineEnding,
}

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            font_family: default_ui_font(),
            font_size: 15.0,
            theme: ThemeChoice::default(),
            tab_orientation: TabDir::default(),
            word_wrap: true,
            show_line_numbers: false,
            zoom_level: 1.0,
            restore_session: true,
            default_encoding: "utf-8".to_string(),
            line_ending: LineEnding::default(),
        }
    }
}

impl Preferences {
    pub const MIN_ZOOM: f32 = 0.5;
    pub const MAX_ZOOM: f32 = 3.0;
    const ZOOM_STEP: f32 = 0.1;

    pub fn zoom_in(&mut self) {
        self.zoom_level = (self.zoom_level + Self::ZOOM_STEP).min(Self::MAX_ZOOM);
    }

    pub fn zoom_out(&mut self) {
        self.zoom_level = (self.zoom_level - Self::ZOOM_STEP).max(Self::MIN_ZOOM);
    }

    pub fn zoom_reset(&mut self) {
        self.zoom_level = 1.0;
    }

    /// Effective font size after applying the zoom multiplier.
    pub fn effective_font_size(&self) -> f32 {
        self.font_size * self.zoom_level
    }

    pub const MIN_FONT: f32 = 8.0;
    pub const MAX_FONT: f32 = 48.0;

    pub fn font_bigger(&mut self) {
        self.font_size = (self.font_size + 1.0).min(Self::MAX_FONT);
    }

    pub fn font_smaller(&mut self) {
        self.font_size = (self.font_size - 1.0).max(Self::MIN_FONT);
    }

    pub fn zoom_percent(&self) -> u32 {
        (self.zoom_level * 100.0).round() as u32
    }
}

fn default_ui_font() -> String {
    if cfg!(windows) {
        "Consolas".to_string()
    } else if cfg!(target_os = "macos") {
        "Menlo".to_string()
    } else {
        "monospace".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip() {
        let mut p = Preferences::default();
        p.theme = ThemeChoice::DarkSoft;
        p.tab_orientation = TabDir::Vertical;
        p.zoom_level = 1.3;
        let json = serde_json::to_string(&p).unwrap();
        let back: Preferences = serde_json::from_str(&json).unwrap();
        assert_eq!(back.theme, ThemeChoice::DarkSoft);
        assert_eq!(back.tab_orientation, TabDir::Vertical);
        assert!((back.zoom_level - 1.3).abs() < f32::EPSILON);
    }

    #[test]
    fn zoom_clamps() {
        let mut p = Preferences::default();
        for _ in 0..100 {
            p.zoom_in();
        }
        assert!(p.zoom_level <= Preferences::MAX_ZOOM + f32::EPSILON);
        for _ in 0..100 {
            p.zoom_out();
        }
        assert!(p.zoom_level >= Preferences::MIN_ZOOM - f32::EPSILON);
    }

    #[test]
    fn partial_json_uses_defaults() {
        // Missing fields must fall back to defaults (forward-compat with old files).
        let back: Preferences = serde_json::from_str(r#"{ "font_size": 20.0 }"#).unwrap();
        assert_eq!(back.font_size, 20.0);
        assert_eq!(back.zoom_level, 1.0);
    }
}
