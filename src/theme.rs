//! Match caelestia's Material colour scheme when available.

use gtk::gdk::RGBA;

/// Colours the custom-drawn widgets (treemap, disk bar) use.
#[derive(Clone)]
pub struct Palette {
    pub tiles: Vec<RGBA>,
    pub file_tile: RGBA,
    pub text: RGBA,
    pub accent: RGBA,
    pub free: RGBA,
    pub mark: RGBA,
}

const APP_CSS: &str = "
progressbar.sizebar trough, progressbar.sizebar progress { min-height: 6px; border-radius: 3px; }
progressbar.sizebar { min-width: 90px; }
.diskbar-label { font-feature-settings: 'tnum'; }
columnview.data-table cell { padding-top: 5px; padding-bottom: 5px; }
.numeric { font-feature-settings: 'tnum'; }
.breadcrumb button { padding-left: 6px; padding-right: 6px; min-height: 26px; }
";

fn hex(s: &str) -> Option<RGBA> {
    RGBA::parse(format!("#{s}")).ok()
}

/// Pull `"key": "rrggbb"` out of scheme.json without a JSON dependency.
fn field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let at = json.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = json[at..].trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    rest.split('"').next()
}

fn scheme_path() -> std::path::PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(Into::into)
        .unwrap_or_else(|| gtk::glib::home_dir().join(".local/state"));
    state.join("caelestia/scheme.json")
}

/// Install CSS and return the palette. Falls back to libadwaita defaults.
pub fn apply(display: &gtk::gdk::Display) -> Palette {
    let json = std::fs::read_to_string(scheme_path()).unwrap_or_default();
    let c = |k: &str| field(&json, k).filter(|v| v.len() == 6);
    let mut css = String::from(APP_CSS);

    let style = adw::StyleManager::default();
    if let Some(mode) = field(&json, "mode") {
        style.set_color_scheme(if mode == "light" {
            adw::ColorScheme::ForceLight
        } else {
            adw::ColorScheme::ForceDark
        });
    }

    let pairs = [
        ("accent_bg_color", "primary"),
        ("accent_color", "primary"),
        ("accent_fg_color", "onPrimary"),
        ("destructive_bg_color", "error"),
        ("destructive_fg_color", "onError"),
        ("window_bg_color", "surface"),
        ("window_fg_color", "onSurface"),
        ("view_bg_color", "surfaceContainerLow"),
        ("view_fg_color", "onSurface"),
        ("headerbar_bg_color", "surfaceContainer"),
        ("headerbar_fg_color", "onSurface"),
        ("card_bg_color", "surfaceContainer"),
        ("popover_bg_color", "surfaceContainerHigh"),
        ("popover_fg_color", "onSurface"),
        ("dialog_bg_color", "surfaceContainerHigh"),
        ("dialog_fg_color", "onSurface"),
    ];
    for (name, key) in pairs {
        if let Some(v) = c(key) {
            css.push_str(&format!("@define-color {name} #{v};\n"));
        }
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css);
    gtk::style_context_add_provider_for_display(display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    let from_scheme: Vec<RGBA> = ["primary", "tertiary", "secondary", "primaryContainer", "tertiaryContainer", "secondaryContainer"]
        .iter()
        .filter_map(|k| c(k).and_then(hex))
        .collect();
    let dark = style.is_dark();
    let accent = style.accent_color_rgba();
    let tiles = if from_scheme.len() == 6 {
        from_scheme
    } else {
        // Adwaita palette: blue, green, yellow, orange, purple, teal.
        ["3584e4", "33d17a", "f6d32d", "ff7800", "9141ac", "2190a4"].iter().filter_map(|h| hex(h)).collect()
    };
    let text = c("onSurface").and_then(hex).unwrap_or(if dark { RGBA::WHITE } else { RGBA::BLACK });
    Palette {
        file_tile: c("outline").and_then(hex).unwrap_or(RGBA::new(0.5, 0.5, 0.5, 1.0)),
        free: c("surfaceContainerHighest").and_then(hex).unwrap_or(RGBA::new(0.5, 0.5, 0.5, 0.25)),
        mark: c("error").and_then(hex).unwrap_or(RGBA::new(0.88, 0.11, 0.14, 1.0)),
        accent: c("primary").and_then(hex).unwrap_or(accent),
        text,
        tiles,
    }
}

#[cfg(test)]
mod tests {
    use super::field;

    #[test]
    fn reads_scheme_fields() {
        let json = r#"{"name": "dynamic", "mode": "dark", "colours": {"primary": "b5d086", "onPrimary":"213600"}}"#;
        assert_eq!(field(json, "mode"), Some("dark"));
        assert_eq!(field(json, "primary"), Some("b5d086"));
        assert_eq!(field(json, "onPrimary"), Some("213600"));
        assert_eq!(field(json, "missing"), None);
    }
}
