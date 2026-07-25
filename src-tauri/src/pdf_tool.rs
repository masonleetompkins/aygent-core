// AYGENT — PDF generation (the first built-in tool).
//
// Pure-Rust (genpdf) → compiled in, zero user setup (no headless Chromium, no
// external binary), matching AYGENT's zero-setup constraint. Renders a light
// markdown subset (headings, bold/italic, bullet lists, paragraphs, code) to a
// paginated PDF. The OUTPUT PATH is resolved THROUGH THE BROKER (jailed) by the
// caller, so a PDF can only ever be written inside the agent folder.

use genpdf::{elements, style, Document, SimplePageDecorator};
use genpdf::Element; // brings the `.styled()` extension method into scope
use serde_json::Value;

// The DEFAULT font is EMBEDDED in the binary (include_bytes!) so PDF generation
// never depends on the OS having a font — truly zero-setup, works on every
// machine. DejaVu Sans is open-license. A user MAY pick a system font in the
// tool config; if that font fails to load we fall back to bundled (never breaks).
const FONT_REGULAR: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../assets/fonts/DejaVuSans-Bold.ttf");
const FONT_ITALIC: &[u8] = include_bytes!("../assets/fonts/DejaVuSans-Oblique.ttf");
const FONT_BOLD_ITALIC: &[u8] = include_bytes!("../assets/fonts/DejaVuSans-BoldOblique.ttf");

/// Render `content` (markdown-ish) to a PDF at the jail-resolved `dest`.
/// `config` = the folder's saved PDF tool config ({} if unset): keys `font`,
/// `text_color`, `heading_color`, `page_size`, `font_size`, `margin`.
pub fn generate(title: &str, content: &str, dest: &std::path::Path, config: &Value) -> Result<(), String> {
    // Config with sensible defaults.
    let font_choice = config.get("font").and_then(|v| v.as_str()).unwrap_or("bundled");
    let text_color = parse_color(config.get("text_color").and_then(|v| v.as_str()).unwrap_or("#111111"));
    let heading_color = parse_color(config.get("heading_color").and_then(|v| v.as_str()).unwrap_or("#111111"));
    let base_size = config.get("font_size").and_then(|v| v.as_u64()).unwrap_or(11) as u8;
    let margin = config.get("margin").and_then(|v| v.as_u64()).unwrap_or(18) as f64;
    let page_a4 = config.get("page_size").and_then(|v| v.as_str()).unwrap_or("Letter") == "A4";

    // Font: bundled (guaranteed) unless the user picked a system font that loads.
    let font = load_font_family(font_choice);

    let mut doc = Document::new(font);
    doc.set_title(title);
    doc.set_font_size(base_size);
    let mut deco = SimplePageDecorator::new();
    deco.set_margins(margin);
    doc.set_page_decorator(deco);
    if page_a4 { doc.set_paper_size(genpdf::PaperSize::A4); }

    let text_style = style::Style::new().with_color(text_color);
    // Build each heading style FRESH (color + bold + size together). Chaining
    // .with_font_size() onto a cloned base style was dropping the color in
    // genpdf 0.2 — which is why heading color didn't show. A helper guarantees
    // color + bold + size are all set in one Style.
    let heading = |size: u8| style::Style::new().bold().with_color(heading_color).with_font_size(size);

    // Render the title as a heading — UNLESS the content already leads with the
    // same title as a markdown H1 (a very common case, and it was producing a
    // DUPLICATE title). We compare the title to the content's first non-empty
    // line stripped of a leading "# "; if they match, we let the content's own
    // heading carry it instead of adding our own.
    let first_line = content.replace('\r', "")
        .lines().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("").to_string();
    let content_leads_with_title = first_line.strip_prefix("# ")
        .map(|h| h.trim().eq_ignore_ascii_case(title.trim()))
        .unwrap_or(false);

    if !title.trim().is_empty() && !content_leads_with_title {
        doc.push(elements::Paragraph::new(title).styled(heading(base_size + 9)));
        doc.push(elements::Break::new(1));
    }

    // Very small markdown subset — enough for reports/notes without a full engine.
    for raw in content.replace('\r', "").split('\n') {
        let line = raw.trim_end();
        if line.trim().is_empty() { doc.push(elements::Break::new(1)); continue; }

        if let Some(rest) = line.strip_prefix("### ") {
            doc.push(elements::Paragraph::new(strip_inline(rest)).styled(heading(base_size + 2)));
        } else if let Some(rest) = line.strip_prefix("## ") {
            doc.push(elements::Paragraph::new(strip_inline(rest)).styled(heading(base_size + 4)));
        } else if let Some(rest) = line.strip_prefix("# ") {
            doc.push(elements::Paragraph::new(strip_inline(rest)).styled(heading(base_size + 7)));
        } else if let Some(rest) = line.trim_start().strip_prefix("- ").or_else(|| line.trim_start().strip_prefix("* ")) {
            doc.push(elements::Paragraph::new(format!("\u{2022} {}", strip_inline(rest))).styled(text_style.clone()));
        } else {
            doc.push(elements::Paragraph::new(strip_inline(line)).styled(text_style.clone()));
        }
    }

    doc.render_to_file(dest).map_err(|e| format!("render pdf: {e}"))
}

/// Build the embedded (bundled) DejaVu family — always succeeds.
fn bundled_family() -> genpdf::fonts::FontFamily<genpdf::fonts::FontData> {
    genpdf::fonts::FontFamily {
        regular: genpdf::fonts::FontData::new(FONT_REGULAR.to_vec(), None).expect("bundled regular"),
        bold: genpdf::fonts::FontData::new(FONT_BOLD.to_vec(), None).expect("bundled bold"),
        italic: genpdf::fonts::FontData::new(FONT_ITALIC.to_vec(), None).expect("bundled italic"),
        bold_italic: genpdf::fonts::FontData::new(FONT_BOLD_ITALIC.to_vec(), None).expect("bundled bold-italic"),
    }
}

/// Load the chosen font family. `choice` is "bundled" or a system font family
/// name. A system font that fails to load falls back to bundled — so a bad
/// choice can NEVER break PDF generation again.
fn load_font_family(choice: &str) -> genpdf::fonts::FontFamily<genpdf::fonts::FontData> {
    if choice.is_empty() || choice.eq_ignore_ascii_case("bundled") {
        return bundled_family();
    }
    // Try to load a system font by family name from the known font dirs.
    for dir in system_font_dirs() {
        if let Ok(fam) = genpdf::fonts::from_files(&dir, choice, None) {
            return fam;
        }
    }
    bundled_family() // safe fallback
}

/// OS font directories to search for user-selectable system fonts.
fn system_font_dirs() -> Vec<String> {
    vec![
        "/System/Library/Fonts".into(),
        "/System/Library/Fonts/Supplemental".into(),
        "/Library/Fonts".into(),
        format!("{}/Library/Fonts", std::env::var("HOME").unwrap_or_default()),
        "/usr/share/fonts".into(),
        "C:\\Windows\\Fonts".into(),
    ]
}

/// Enumerate system fonts the user can pick (family names derived from the
/// -Regular/.ttf files genpdf can actually load). Always returns "bundled" first.
pub fn system_fonts() -> Vec<String> {
    let mut names = vec!["bundled".to_string()];
    for dir in system_font_dirs() {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let fname = e.file_name().to_string_lossy().to_string();
                // genpdf's from_files wants a family name and expects
                // "<family>-Regular.ttf" or "<family>.ttf". Surface families that
                // have a plain "<family>.ttf" or "<family>-Regular.ttf".
                let lower = fname.to_lowercase();
                if !lower.ends_with(".ttf") { continue; }
                let stem = fname.trim_end_matches(".ttf").trim_end_matches(".TTF");
                let family = stem.trim_end_matches("-Regular").trim_end_matches("-regular");
                if !family.is_empty() && !names.iter().any(|n| n.eq_ignore_ascii_case(family)) {
                    names.push(family.to_string());
                }
            }
        }
    }
    // Cap the list so the dropdown isn't a thousand entries.
    names.truncate(60);
    names
}

/// Parse "#rrggbb" into a genpdf Color (defaults to near-black on bad input).
fn parse_color(hex: &str) -> style::Color {
    let h = hex.trim_start_matches('#');
    if h.len() == 6 {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&h[0..2], 16),
            u8::from_str_radix(&h[2..4], 16),
            u8::from_str_radix(&h[4..6], 16),
        ) { return style::Color::Rgb(r, g, b); }
    }
    style::Color::Rgb(17, 17, 17)
}

/// Strip a minimal set of inline markdown (**bold**, *italic*, `code`) since the
/// simple genpdf paragraph is single-style. We drop the markers, keeping text.
fn strip_inline(s: &str) -> String {
    s.replace("**", "").replace('`', "").replace("__", "")
}
