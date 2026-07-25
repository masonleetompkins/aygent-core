// AYGENT — PDF generation (the first built-in tool).
//
// Pure-Rust (genpdf) → compiled in, zero user setup (no headless Chromium, no
// external binary), matching AYGENT's zero-setup constraint. Renders a light
// markdown subset (headings, bold/italic, bullet lists, paragraphs, code) to a
// paginated PDF. The OUTPUT PATH is resolved THROUGH THE BROKER (jailed) by the
// caller, so a PDF can only ever be written inside the agent folder.

use genpdf::{elements, style, Document, SimplePageDecorator};
use genpdf::Element; // brings the `.styled()` extension method into scope

/// Render `content` (markdown-ish) to a PDF at the already-jail-resolved
/// absolute `dest` path. `title` is the document title / first heading.
pub fn generate(title: &str, content: &str, dest: &std::path::Path) -> Result<(), String> {
    // Font: genpdf needs a TrueType family. We load from the OS font directory,
    // trying a few common families so this works on macOS out of the box (and
    // degrades to whatever's present). No install step for the USER — these are
    // system fonts already on the machine.
    let font = load_font()
        .map_err(|e| format!("load font: {e} (no usable system font found)"))?;

    let mut doc = Document::new(font);
    doc.set_title(title);
    let mut deco = SimplePageDecorator::new();
    deco.set_margins(18);
    doc.set_page_decorator(deco);

    if !title.trim().is_empty() {
        doc.push(elements::Paragraph::new(title).styled(style::Style::new().bold().with_font_size(20)));
        doc.push(elements::Break::new(1));
    }

    // Very small markdown subset — enough for reports/notes without a full engine.
    for raw in content.replace('\r', "").split('\n') {
        let line = raw.trim_end();
        if line.trim().is_empty() { doc.push(elements::Break::new(1)); continue; }

        if let Some(rest) = line.strip_prefix("### ") {
            doc.push(elements::Paragraph::new(rest).styled(style::Style::new().bold().with_font_size(13)));
        } else if let Some(rest) = line.strip_prefix("## ") {
            doc.push(elements::Paragraph::new(rest).styled(style::Style::new().bold().with_font_size(15)));
        } else if let Some(rest) = line.strip_prefix("# ") {
            doc.push(elements::Paragraph::new(rest).styled(style::Style::new().bold().with_font_size(18)));
        } else if let Some(rest) = line.trim_start().strip_prefix("- ").or_else(|| line.trim_start().strip_prefix("* ")) {
            doc.push(elements::Paragraph::new(format!("\u{2022} {}", strip_inline(rest))));
        } else {
            doc.push(elements::Paragraph::new(strip_inline(line)));
        }
    }

    doc.render_to_file(dest).map_err(|e| format!("render pdf: {e}"))
}

/// Load a usable font family from the OS font directories. Tries common
/// families/dirs in order; returns the first that loads. genpdf::fonts::from_files
/// wants a directory + family name (it appends -Regular/-Bold/etc).
fn load_font() -> Result<genpdf::fonts::FontFamily<genpdf::fonts::FontData>, genpdf::error::Error> {
    // (dir, family) candidates — macOS Supplemental has Arial; core has others.
    let candidates: &[(&str, &str)] = &[
        ("/Library/Fonts", "Arial"),
        ("/System/Library/Fonts/Supplemental", "Arial"),
        ("/System/Library/Fonts/Supplemental", "Times New Roman"),
        ("/usr/share/fonts/truetype/dejavu", "DejaVuSans"),
        ("C:\\Windows\\Fonts", "arial"),
    ];
    let mut last_err: Option<genpdf::error::Error> = None;
    for (dir, fam) in candidates {
        match genpdf::fonts::from_files(dir, fam, None) {
            Ok(f) => return Ok(f),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| genpdf::error::Error::new(
        "no font found", genpdf::error::ErrorKind::Internal,
    )))
}

/// Strip a minimal set of inline markdown (**bold**, *italic*, `code`) since the
/// simple genpdf paragraph is single-style. We drop the markers, keeping text.
fn strip_inline(s: &str) -> String {
    s.replace("**", "").replace('`', "").replace("__", "")
}
