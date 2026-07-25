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
// The font is EMBEDDED in the binary (include_bytes!) so PDF generation never
// depends on the OS having a specific font file — truly zero-setup, identical on
// every machine. DejaVu Sans is open-license (safe to ship). This replaces the
// fragile system-font lookup that failed with "no usable system font found".
const FONT_REGULAR: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../assets/fonts/DejaVuSans-Bold.ttf");
const FONT_ITALIC: &[u8] = include_bytes!("../assets/fonts/DejaVuSans-Oblique.ttf");
const FONT_BOLD_ITALIC: &[u8] = include_bytes!("../assets/fonts/DejaVuSans-BoldOblique.ttf");

pub fn generate(title: &str, content: &str, dest: &std::path::Path) -> Result<(), String> {
    // Build the font family from the embedded bytes — no filesystem, no OS deps.
    let font = genpdf::fonts::FontFamily {
        regular: genpdf::fonts::FontData::new(FONT_REGULAR.to_vec(), None).map_err(|e| format!("font regular: {e}"))?,
        bold: genpdf::fonts::FontData::new(FONT_BOLD.to_vec(), None).map_err(|e| format!("font bold: {e}"))?,
        italic: genpdf::fonts::FontData::new(FONT_ITALIC.to_vec(), None).map_err(|e| format!("font italic: {e}"))?,
        bold_italic: genpdf::fonts::FontData::new(FONT_BOLD_ITALIC.to_vec(), None).map_err(|e| format!("font bold-italic: {e}"))?,
    };

    let mut doc = Document::new(font);
    doc.set_title(title);
    let mut deco = SimplePageDecorator::new();
    deco.set_margins(18);
    doc.set_page_decorator(deco);

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

/// Strip a minimal set of inline markdown (**bold**, *italic*, `code`) since the
/// simple genpdf paragraph is single-style. We drop the markers, keeping text.
fn strip_inline(s: &str) -> String {
    s.replace("**", "").replace('`', "").replace("__", "")
}
