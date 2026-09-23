// AYGENT — OMARCHY THEME ADAPTER (Linux port).
//
// Omarchy sets the whole desktop's look by symlinking one theme dir:
//     ~/.config/omarchy/current/theme -> ~/.config/omarchy/themes/<name>/
// Each theme carries per-app colour files (alacritty.toml, waybar.css,
// hyprland.conf, btop.theme, mako.ini, ...). Light themes contain a marker
// file `light.mode`.
//
// AYGENT already themes itself from ~110 CSS custom properties set at runtime
// (ui/src/theme.css + ui/src/lib/theme.ts). So this module does NOT introduce a
// theme system — it READS Omarchy's palette and hands the existing one a
// palette to apply. `omarchy-theme-set <name>` then retints AYGENT live, like
// every other app on the desktop.
//
// PALETTE SOURCE: alacritty.toml. It is TOML (no bespoke parser needed), it is
// present in every stock Omarchy theme, and it carries a full 16-colour set
// plus explicit primary background/foreground. waybar.css is the fallback
// (@define-color lines) for themes that omit it.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A resolved Omarchy palette, in the shape the UI wants (hex strings).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OmarchyTheme {
    /// Theme directory name, e.g. "tokyo-night".
    pub name: String,
    /// true when the theme ships a `light.mode` marker.
    pub light: bool,
    pub bg: String,
    pub surface: String,
    pub text: String,
    pub text_muted: String,
    pub text_faint: String,
    pub accent: String,
    pub ok: String,
    pub danger: String,
    pub warn: String,
}

/// ~/.config (honouring XDG_CONFIG_HOME).
fn config_home() -> Option<PathBuf> {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return Some(PathBuf::from(x));
        }
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
}

/// The live theme dir: ~/.config/omarchy/current/theme (a symlink).
pub fn current_theme_dir() -> Option<PathBuf> {
    let p = config_home()?.join("omarchy").join("current").join("theme");
    // canonicalize so we get the real themes/<name>/ dir (and its name).
    std::fs::canonicalize(&p).ok().or(if p.exists() { Some(p) } else { None })
}

// ── colour helpers ──────────────────────────────────────────────────────────

fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let t = s.trim().trim_matches(|c| c == '"' || c == '\'' || c == '#');
    // Alacritty writes 0xRRGGBB; CSS writes RRGGBB.
    let t = t.strip_prefix("0x").unwrap_or(t);
    if t.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(t, 16).ok()?;
    Some((((n >> 16) & 255) as u8, ((n >> 8) & 255) as u8, (n & 255) as u8))
}

fn hex(c: (u8, u8, u8)) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

/// Blend `a` toward `b` by `t` (0..1).
fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    (f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

// ── alacritty.toml ──────────────────────────────────────────────────────────

/// Pull `key = "value"` pairs out of the [colors.*] tables we care about.
/// Deliberately a small scanner rather than a TOML dependency: we need exactly
/// six keys and the files are machine-generated and flat.
fn parse_alacritty(text: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let mut table = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            table = line.trim_matches(|c| c == '[' || c == ']').to_string();
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let key = format!("{}.{}", table, k.trim());
            out.insert(key, v.trim().trim_matches('"').to_string());
        }
    }
    out
}

/// waybar.css fallback: `@define-color name #rrggbb;`
fn parse_waybar(text: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("@define-color") {
            if let Some((name, val)) = rest.trim().split_once(char::is_whitespace) {
                out.insert(
                    name.trim().to_string(),
                    val.trim().trim_end_matches(';').trim().to_string(),
                );
            }
        }
    }
    out
}

/// Read the current Omarchy theme and map it onto AYGENT's variables.
pub fn read_current() -> Option<OmarchyTheme> {
    let dir = current_theme_dir()?;
    read_from(&dir)
}

pub fn read_from(dir: &Path) -> Option<OmarchyTheme> {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "omarchy".into());
    let light = dir.join("light.mode").exists();

    let (bg, fg, accent, ok, danger, warn) = if let Ok(t) =
        std::fs::read_to_string(dir.join("alacritty.toml"))
    {
        let m = parse_alacritty(&t);
        let g = |k: &str| m.get(k).and_then(|v| parse_hex(v));
        (
            g("colors.primary.background"),
            g("colors.primary.foreground"),
            g("colors.normal.blue"),
            g("colors.normal.green"),
            g("colors.normal.red"),
            g("colors.normal.yellow"),
        )
    } else if let Ok(t) = std::fs::read_to_string(dir.join("waybar.css")) {
        let m = parse_waybar(&t);
        let g = |k: &str| m.get(k).and_then(|v| parse_hex(v));
        (
            g("background").or_else(|| g("bg")),
            g("foreground").or_else(|| g("text")),
            g("accent").or_else(|| g("blue")),
            g("green"),
            g("red"),
            g("yellow"),
        )
    } else {
        return None;
    };

    // Fall back to sane values per mode if a key is missing.
    let bg = bg.unwrap_or(if light { (250, 250, 250) } else { (0, 0, 0) });
    let fg = fg.unwrap_or(if light { (10, 10, 10) } else { (255, 255, 255) });
    let accent = accent.unwrap_or(fg);

    Some(OmarchyTheme {
        name,
        light,
        bg: hex(bg),
        // Lift the surface off the background so cards still separate — the
        // same trick the stock dark theme uses (#000 -> #101012).
        surface: hex(mix(bg, fg, 0.06)),
        text: hex(fg),
        text_muted: hex(mix(fg, bg, 0.35)),
        text_faint: hex(mix(fg, bg, 0.60)),
        accent: hex(accent),
        ok: hex(ok.unwrap_or((52, 209, 126))),
        danger: hex(danger.unwrap_or((255, 107, 107))),
        warn: hex(warn.unwrap_or((224, 165, 63))),
    })
}

/// Command: the current Omarchy theme, or None if Omarchy isn't installed.
#[tauri::command]
pub fn omarchy_theme() -> Option<OmarchyTheme> {
    read_current()
}

/// Command: is this machine running Omarchy at all? (Drives whether the
/// "Follow Omarchy theme" switch is even shown.)
#[tauri::command]
pub fn omarchy_available() -> bool {
    current_theme_dir().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_alacritty_palette() {
        let toml = r##"
[colors.primary]
background = "#1a1b26"
foreground = "#c0caf5"

[colors.normal]
red = "#f7768e"
green = "#9ece6a"
yellow = "#e0af68"
blue = "#7aa2f7"
"##;
        let m = parse_alacritty(toml);
        assert_eq!(m.get("colors.primary.background").unwrap(), "#1a1b26");
        assert_eq!(parse_hex(m.get("colors.normal.blue").unwrap()), Some((0x7a, 0xa2, 0xf7)));
    }

    #[test]
    fn parses_0x_form() {
        assert_eq!(parse_hex("0x1a1b26"), Some((0x1a, 0x1b, 0x26)));
    }

    #[test]
    fn surface_lifts_off_background() {
        // pure black bg + white fg -> surface must be lighter than bg
        let s = mix((0, 0, 0), (255, 255, 255), 0.06);
        assert!(s.0 > 0 && s.0 < 40);
    }

    #[test]
    fn waybar_fallback_parses() {
        let css = "@define-color background #1a1b26;\n@define-color foreground #c0caf5;\n";
        let m = parse_waybar(css);
        assert_eq!(m.get("background").unwrap(), "#1a1b26");
    }
}
