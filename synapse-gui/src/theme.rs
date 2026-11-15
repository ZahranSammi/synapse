use std::fs;
use std::path::{Path, PathBuf};

use gpui::{Rgba, rgb};

/// Supported UI color themes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Dark,
    Light,
}

impl ThemeMode {
    pub fn toggle(self) -> Self {
        match self {
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::Dark => "Dark",
            ThemeMode::Light => "Light",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ThemeMode::Dark => "dark",
            ThemeMode::Light => "light",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "light" => ThemeMode::Light,
            _ => ThemeMode::Dark,
        }
    }
}

/// Color tokens used across all GPUI views for dark and light modes.
#[derive(Debug, Clone, Copy)]
pub struct ThemePalette {
    pub bg_app: Rgba,
    pub bg_sidebar: Rgba,
    pub bg_surface: Rgba,
    pub bg_surface_alt: Rgba,
    pub bg_hover: Rgba,
    pub bg_input: Rgba,
    pub border: Rgba,
    pub border_subtle: Rgba,
    pub border_focus: Rgba,
    pub text_primary: Rgba,
    pub text_secondary: Rgba,
    pub text_muted: Rgba,
    pub accent: Rgba,
    pub accent_hover: Rgba,
    pub accent_fg: Rgba,
    pub accent_soft: Rgba,
    pub success: Rgba,
    pub success_bg: Rgba,
    pub warning: Rgba,
    pub warning_bg: Rgba,
    pub danger: Rgba,
    pub danger_bg: Rgba,
}

impl ThemePalette {
    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self::dark(),
            ThemeMode::Light => Self::light(),
        }
    }

    pub fn dark() -> Self {
        Self {
            bg_app: rgb(0x0d1117),
            bg_sidebar: rgb(0x131822),
            bg_surface: rgb(0x171d28),
            bg_surface_alt: rgb(0x1e2533),
            bg_hover: rgb(0x273042),
            bg_input: rgb(0x0f141c),
            border: rgb(0x2c3547),
            border_subtle: rgb(0x1f2735),
            border_focus: rgb(0x3b82f6),
            text_primary: rgb(0xf3f4f6),
            text_secondary: rgb(0xa3b1c6),
            text_muted: rgb(0x6e7b91),
            accent: rgb(0x3b82f6),
            accent_hover: rgb(0x2563eb),
            accent_fg: rgb(0xffffff),
            accent_soft: rgb(0x1b3459),
            success: rgb(0x34d399),
            success_bg: rgb(0x0c3b2e),
            warning: rgb(0xfbbf24),
            warning_bg: rgb(0x422a08),
            danger: rgb(0xf87171),
            danger_bg: rgb(0x471316),
        }
    }

    pub fn light() -> Self {
        Self {
            bg_app: rgb(0xf4f6fa),
            bg_sidebar: rgb(0xe9eef5),
            bg_surface: rgb(0xffffff),
            bg_surface_alt: rgb(0xf1f5f9),
            bg_hover: rgb(0xe2e8f0),
            bg_input: rgb(0xffffff),
            border: rgb(0xcbd5e1),
            border_subtle: rgb(0xe2e8f0),
            border_focus: rgb(0x2563eb),
            text_primary: rgb(0x0f172a),
            text_secondary: rgb(0x475569),
            text_muted: rgb(0x64748b),
            accent: rgb(0x2563eb),
            accent_hover: rgb(0x1d4ed8),
            accent_fg: rgb(0xffffff),
            accent_soft: rgb(0xdbeafe),
            success: rgb(0x047857),
            success_bg: rgb(0xd1fae5),
            warning: rgb(0xb45309),
            warning_bg: rgb(0xfef3c7),
            danger: rgb(0xdc2626),
            danger_bg: rgb(0xfee2e2),
        }
    }
}

pub fn theme_config_path() -> Option<PathBuf> {
    synapse::platform::paths::synapse_dir()
        .ok()
        .map(|d| d.join("gui-theme"))
}

pub fn load_theme_preference_at(path: &Path) -> ThemeMode {
    fs::read_to_string(path)
        .map(|s| ThemeMode::parse(&s))
        .unwrap_or(ThemeMode::Dark)
}

pub fn save_theme_preference_at(path: &Path, mode: ThemeMode) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{}\n", mode.as_str()))
}

pub fn load_theme_preference() -> ThemeMode {
    theme_config_path()
        .map(|p| load_theme_preference_at(&p))
        .unwrap_or(ThemeMode::Dark)
}

pub fn save_theme_preference(mode: ThemeMode) {
    if let Some(path) = theme_config_path() {
        let _ = save_theme_preference_at(&path, mode);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_mode_toggle_and_palettes() {
        assert_eq!(ThemeMode::Dark.toggle(), ThemeMode::Light);
        assert_eq!(ThemeMode::Light.toggle(), ThemeMode::Dark);

        let dark = ThemePalette::for_mode(ThemeMode::Dark);
        let light = ThemePalette::for_mode(ThemeMode::Light);
        assert_ne!(dark.bg_app, light.bg_app);
        assert_ne!(dark.text_primary, light.text_primary);
        assert_ne!(dark.bg_surface, light.bg_surface);
    }

    #[test]
    fn test_theme_preference_roundtrip() {
        let temp = std::env::temp_dir().join(format!("synapse_gui_theme_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let file = temp.join("gui-theme");

        assert_eq!(load_theme_preference_at(&file), ThemeMode::Dark);
        save_theme_preference_at(&file, ThemeMode::Light).unwrap();
        assert_eq!(load_theme_preference_at(&file), ThemeMode::Light);
        save_theme_preference_at(&file, ThemeMode::Dark).unwrap();
        assert_eq!(load_theme_preference_at(&file), ThemeMode::Dark);

        let _ = fs::remove_dir_all(&temp);
    }
}
