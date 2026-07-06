//! Meeting → PDF renderer.
//!
//! Consumed by the `GET /meetings/:id/export.pdf` endpoint in
//! `api.rs`. Takes a [`PdfInput`] (slimmed-down meeting detail +
//! preloaded screenshot bytes for each moment) and produces a
//! complete PDF as a byte vec ready to write to the wire.
//!
//! Layout is intentionally simple: A4 portrait, single text column
//! with a margin, top-down cursor model. The renderer paginates by
//! checking the cursor position before drawing each block; nothing
//! flows automatically across pages (a long paragraph that doesn't
//! fit gets pushed to the next page wholesale). Built-in Helvetica
//! fonts mean no TTF files to ship.
//!
//! Moment screenshots are interleaved with the transcript at their
//! `t` (ms-since-meeting-start) position, sized to fill most of the
//! body width — bigger than the detail-view UI shows them, per the
//! "make pictures larger on export" ask.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use printpdf::{
    BuiltinFont, Image, ImageTransform, IndirectFontRef, Mm, PdfDocument, PdfDocumentReference,
    PdfLayerReference,
};

// ─── Geometry (A4 portrait, mm) ──────────────────────────────────────────
const PAGE_W: f32 = 210.0;
const PAGE_H: f32 = 297.0;
const MARGIN: f32 = 12.0; // tight margins — denser pages
const BODY_W: f32 = PAGE_W - 2.0 * MARGIN;
const BODY_BOTTOM: f32 = MARGIN; // last legal y before we paginate
/// Indent applied to "block" content (description, mode items) so
/// they read as visually distinct from headings.
const BLOCK_INDENT: f32 = 4.0;

// ─── Type sizes (pt) — biased toward density ─────────────────────────────
const SIZE_TITLE: f32 = 18.0;
const SIZE_BODY: f32 = 9.0;
/// Description body is one notch smaller than transcript body so
/// the description-block feels visually contained, mirroring the
/// UI's gray-tinted description card.
const SIZE_DESC: f32 = 8.5;
const SIZE_SMALL: f32 = 7.5;
const SIZE_MONO: f32 = 8.0;

// Empirical mm-per-character for Helvetica at SIZE_BODY (9pt).
// Used by the greedy word-wrapper. Helvetica is variable-width, so
// this overestimates skinny chars and underestimates wide chars; the
// wrapper biases toward more wrapping rather than overflow. Fine for
// transcript prose, where exact column fit doesn't matter much.
const BODY_CHAR_MM: f32 = 1.75;
const DESC_CHAR_MM: f32 = 1.65;

/// (mode id, section title) pairs, rendered in this order if the
/// meeting has any items for that mode. Transcript is intentionally
/// excluded — it owns its own section at the bottom that
/// interleaves with moments.
const MODE_SECTIONS: &[(&str, &str)] = &[
    ("highlights", "HIGHLIGHTS"),
    ("actions", "ACTIONS"),
    ("open_questions", "OPEN QUESTIONS"),
    ("summary", "SUMMARY"),
    ("chat", "CHAT"),
];

/// Input to the renderer. Decouples the PDF module from `api.rs`'s
/// MeetingDetail so future changes to the wire shape don't ripple
/// into the renderer.
/// All fields are owned (String, not &str) so the value can be
/// moved into `tokio::task::spawn_blocking` without lifetime
/// gymnastics. The renderer doesn't keep any of this data past its
/// return.
pub struct PdfInput {
    pub id: String,
    /// The human-readable title — same `pickMeetingTitle`-style
    /// logic the clients apply, computed by the endpoint.
    pub title: String,
    pub description: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    /// Sorted (key, value) pairs. The endpoint sorts the metadata
    /// HashMap before passing in — clients render in alpha order
    /// too, and PDFs benefit from stable ordering across exports.
    pub metadata: Vec<(String, String)>,
    /// Transcript items in chronological order (by `t`).
    pub transcript: Vec<crate::protocol::Item>,
    /// Persisted per-mode items keyed by mode id (highlights /
    /// actions / open_questions / summary / chat). Rendered as
    /// their own labeled sections before the transcript, matching
    /// what the Mac detail view shows. Missing modes simply
    /// produce no section. Transcript is intentionally NOT in here
    /// (it has its own field above + interleaves with moments).
    pub items_by_mode: std::collections::HashMap<String, Vec<crate::protocol::Item>>,
    /// Moments to interleave with the transcript, with their bytes
    /// already loaded.
    pub moments: Vec<RenderableMoment>,
}

pub struct RenderableMoment {
    pub id: String,
    pub t: i64,
    pub note: Option<String>,
    pub summary: Option<String>,
    /// Encoded screenshot bytes (PNG today; format is sniffed from
    /// the magic bytes at render time, so swapping to JPEG / WebP
    /// later doesn't break this contract). None when the moment was
    /// marked without a screenshot.
    pub screenshot_bytes: Option<Vec<u8>>,
}

/// Render the meeting to PDF bytes. Synchronous because printpdf
/// itself is sync; callers should run it on a blocking thread (e.g.
/// `tokio::task::spawn_blocking`) if invoked from an async context
/// with large transcripts or many moments.
pub fn render(input: &PdfInput) -> Result<Vec<u8>> {
    let (doc, page_idx, layer_idx) = PdfDocument::new(&input.title, Mm(PAGE_W), Mm(PAGE_H), "page");
    let regular = doc
        .add_builtin_font(BuiltinFont::Helvetica)
        .context("add Helvetica")?;
    let bold = doc
        .add_builtin_font(BuiltinFont::HelveticaBold)
        .context("add HelveticaBold")?;
    let italic = doc
        .add_builtin_font(BuiltinFont::HelveticaOblique)
        .context("add HelveticaOblique")?;
    let mono = doc
        .add_builtin_font(BuiltinFont::Courier)
        .context("add Courier")?;
    let mut ctx = RenderCtx {
        doc,
        page_idx,
        layer_idx,
        y: PAGE_H - MARGIN,
        fonts: Fonts {
            regular,
            bold,
            italic,
            mono,
        },
    };

    // ── Header ─────────────────────────────────────────────────────
    ctx.heading(&input.title, SIZE_TITLE, FontStyle::Bold);
    ctx.gap(1.5);

    let timing = format_timing(input.started_at, input.ended_at);
    ctx.text_small(&timing);
    ctx.gap(2.5);

    // ── Description ────────────────────────────────────────────────
    // Smaller font + left-indent gives it a "block" feel without
    // requiring background-fill primitives. Mirrors the UI's
    // gray-tinted description card.
    if let Some(desc) = input
        .description
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        ctx.section_label("DESCRIPTION");
        ctx.indented_paragraph(desc, SIZE_DESC, DESC_CHAR_MM);
        ctx.gap(2.0);
    }

    // ── Metadata ───────────────────────────────────────────────────
    if !input.metadata.is_empty() {
        ctx.section_label("METADATA");
        for (k, v) in &input.metadata {
            ctx.key_value(k, v);
        }
        ctx.gap(2.0);
    }

    // ── Per-mode sections ──────────────────────────────────────────
    // Order matches the agent's emission priority: Highlights (the
    // 5-item Replace digest) → Actions → Open Questions →
    // Summary (single rolling paragraph) → Chat (Q+A pair).
    // Modes with no items produce no section.
    for (mode_id, label) in MODE_SECTIONS {
        if let Some(items) = input.items_by_mode.get(*mode_id) {
            if !items.is_empty() {
                ctx.mode_section(label, mode_id, items);
            }
        }
    }

    // ── Transcript with interleaved moments ────────────────────────
    if input.transcript.is_empty() && input.moments.is_empty() {
        ctx.section_label("TRANSCRIPT");
        ctx.text("(no transcript captured)", SIZE_BODY, FontStyle::Italic);
    } else {
        ctx.section_label("TRANSCRIPT");
        // Merge-sort transcript items + moments by timestamp `t`.
        // Each iteration picks the head of whichever stream's next
        // item comes first.
        let mut ti = 0;
        let mut mi = 0;
        let transcript = input.transcript.as_slice();
        let moments = &input.moments;
        loop {
            match (transcript.get(ti), moments.get(mi)) {
                (Some(item), Some(m)) => {
                    if (item.t as i64) <= m.t {
                        ctx.transcript_item(item);
                        ti += 1;
                    } else {
                        ctx.render_moment(m);
                        mi += 1;
                    }
                }
                (Some(item), None) => {
                    ctx.transcript_item(item);
                    ti += 1;
                }
                (None, Some(m)) => {
                    ctx.render_moment(m);
                    mi += 1;
                }
                (None, None) => break,
            }
        }
    }

    // ── Footer ─────────────────────────────────────────────────────
    ctx.gap(6.0);
    ctx.text_small(&format!("meeting id: {}", input.id));

    let bytes = ctx
        .doc
        .save_to_bytes()
        .context("printpdf save_to_bytes failed")?;
    Ok(bytes)
}

// ─── Internal helpers ────────────────────────────────────────────────────

struct Fonts {
    regular: IndirectFontRef,
    bold: IndirectFontRef,
    italic: IndirectFontRef,
    mono: IndirectFontRef,
}

#[derive(Copy, Clone)]
enum FontStyle {
    Regular,
    Bold,
    Italic,
}

struct RenderCtx {
    doc: PdfDocumentReference,
    page_idx: printpdf::PdfPageIndex,
    layer_idx: printpdf::PdfLayerIndex,
    /// Current cursor y in mm, measured from page bottom. The
    /// origin is bottom-left in printpdf; we draw at `y` and
    /// decrement after each block.
    y: f32,
    fonts: Fonts,
}

impl RenderCtx {
    /// Cloning the `IndirectFontRef` is cheap (it's a reference
    /// into the doc, not the glyph table) and gets us out of the
    /// `&self`-borrow trap: callers can do `self.font_for(...)`
    /// then `self.ensure_room(...)` without overlapping borrows.
    fn font_for(&self, style: FontStyle) -> (IndirectFontRef, f32) {
        match style {
            FontStyle::Regular => (self.fonts.regular.clone(), BODY_CHAR_MM),
            FontStyle::Bold => (self.fonts.bold.clone(), BODY_CHAR_MM),
            FontStyle::Italic => (self.fonts.italic.clone(), BODY_CHAR_MM),
        }
    }

    fn layer(&self) -> PdfLayerReference {
        self.doc.get_page(self.page_idx).get_layer(self.layer_idx)
    }

    /// Reserve `height_mm` below the cursor; start a new page if we
    /// don't have room. Call this BEFORE drawing the block.
    fn ensure_room(&mut self, height_mm: f32) {
        if self.y - height_mm < BODY_BOTTOM {
            let (page_idx, layer_idx) = self.doc.add_page(Mm(PAGE_W), Mm(PAGE_H), "page");
            self.page_idx = page_idx;
            self.layer_idx = layer_idx;
            self.y = PAGE_H - MARGIN;
        }
    }

    /// Advance the cursor by `mm` without drawing.
    fn gap(&mut self, mm: f32) {
        self.y -= mm;
    }

    /// Write a single line at the current cursor and advance y by
    /// the line height. No wrapping — callers should `paragraph()`
    /// for prose.
    fn text(&mut self, line: &str, size_pt: f32, style: FontStyle) {
        let line_h = pt_to_mm(size_pt) * 1.3;
        self.ensure_room(line_h);
        let (font, _) = self.font_for(style);
        // printpdf positions text by baseline; nudge down by the
        // font's cap-height-ish so the visible top of the glyphs
        // matches our cursor.
        let baseline_y = self.y - pt_to_mm(size_pt) * 0.85;
        self.layer()
            .use_text(line, size_pt, Mm(MARGIN), Mm(baseline_y), &font);
        self.y -= line_h;
    }

    /// Smaller text in the regular weight, used for the timing row
    /// + footer.
    fn text_small(&mut self, line: &str) {
        self.text(line, SIZE_SMALL, FontStyle::Regular);
    }

    /// Section header (bold, larger). Adds modest top + bottom
    /// padding so sections breathe.
    fn heading(&mut self, line: &str, size_pt: f32, style: FontStyle) {
        self.gap(1.0);
        self.text(line, size_pt, style);
    }

    /// All-caps mini-header above blocks (DESCRIPTION, METADATA, …).
    /// Matches the visual treatment in the PWA / Mac / mobile detail
    /// views.
    fn section_label(&mut self, label: &str) {
        self.gap(2.5);
        self.text(label, SIZE_SMALL, FontStyle::Bold);
        self.gap(0.5);
    }

    /// "key  →  value" row used in the metadata + LLM usage blocks.
    /// Key is mono-styled to read like a label; value is regular.
    fn key_value(&mut self, key: &str, value: &str) {
        let key_w_mm: f32 = 38.0;
        let line_h = pt_to_mm(SIZE_BODY) * 1.25;
        self.ensure_room(line_h);
        let baseline_y = self.y - pt_to_mm(SIZE_BODY) * 0.85;
        self.layer()
            .use_text(key, SIZE_BODY, Mm(MARGIN), Mm(baseline_y), &self.fonts.mono);
        // Wrap long values that exceed remaining body width.
        let value_x = MARGIN + key_w_mm;
        let value_max_chars = ((BODY_W - key_w_mm) / BODY_CHAR_MM) as usize;
        let lines = wrap_text(value, value_max_chars.max(20));
        let regular = &self.fonts.regular;
        for (i, l) in lines.iter().enumerate() {
            let baseline = baseline_y - (i as f32) * line_h;
            self.layer()
                .use_text(l.as_str(), SIZE_BODY, Mm(value_x), Mm(baseline), regular);
        }
        self.y -= line_h * (lines.len().max(1) as f32);
    }

    /// Greedy-wrap a paragraph and draw each line. Trailing blank
    /// lines in the source are preserved.
    fn paragraph(&mut self, text: &str, style: FontStyle, size_pt: f32) {
        let (font, char_w) = self.font_for(style);
        let max_chars = (BODY_W / char_w) as usize;
        let line_h = pt_to_mm(size_pt) * 1.25;
        for src_line in text.lines() {
            let wrapped = wrap_text(src_line, max_chars.max(20));
            for l in wrapped {
                self.ensure_room(line_h);
                let baseline = self.y - pt_to_mm(size_pt) * 0.85;
                self.layer()
                    .use_text(l.as_str(), size_pt, Mm(MARGIN), Mm(baseline), &font);
                self.y -= line_h;
            }
            if src_line.is_empty() {
                // Preserve intentional paragraph breaks (blank
                // line in source → extra gap on the page).
                self.y -= line_h * 0.5;
            }
        }
    }

    /// Description-style paragraph: indented from the left margin
    /// and rendered at a smaller body size. Mirrors the UI's
    /// gray-tinted DESCRIPTION card visually (without needing fill
    /// primitives — printpdf's rect drawing would be a separate
    /// path; indentation + a smaller font is good enough).
    fn indented_paragraph(&mut self, text: &str, size_pt: f32, char_w: f32) {
        let avail_w = BODY_W - BLOCK_INDENT;
        let max_chars = (avail_w / char_w) as usize;
        let line_h = pt_to_mm(size_pt) * 1.3;
        let font = self.fonts.regular.clone();
        for src_line in text.lines() {
            let wrapped = wrap_text(src_line, max_chars.max(20));
            for l in wrapped {
                self.ensure_room(line_h);
                let baseline = self.y - pt_to_mm(size_pt) * 0.85;
                self.layer().use_text(
                    l.as_str(),
                    size_pt,
                    Mm(MARGIN + BLOCK_INDENT),
                    Mm(baseline),
                    &font,
                );
                self.y -= line_h;
            }
            if src_line.is_empty() {
                self.y -= line_h * 0.5;
            }
        }
    }

    /// Render a per-mode block (HIGHLIGHTS / ACTIONS / OPEN
    /// QUESTIONS / SUMMARY / CHAT). The bullet style varies by mode:
    /// summary is a single prose paragraph, chat alternates user /
    /// assistant Q-and-A, everything else is a "• bullet" list.
    fn mode_section(&mut self, label: &str, mode_id: &str, items: &[crate::protocol::Item]) {
        self.section_label(label);
        match mode_id {
            "summary" => {
                // Summary mode has a single Replace item — render
                // its body straight up as an indented paragraph.
                for item in items {
                    self.indented_paragraph(item.text.trim(), SIZE_BODY, BODY_CHAR_MM);
                }
            }
            "chat" => {
                // Chat items carry `meta.role` = "user" | "assistant".
                // Show user prompts bold, assistant replies regular.
                for item in items {
                    let role = item
                        .meta
                        .as_ref()
                        .and_then(|m| m.get("role").and_then(|v| v.as_str()))
                        .unwrap_or("");
                    let prefix = if role == "user" { "Q:" } else { "A:" };
                    let style = if role == "user" {
                        FontStyle::Bold
                    } else {
                        FontStyle::Regular
                    };
                    self.prefixed_paragraph(prefix, &item.text, style);
                    self.gap(1.0);
                }
            }
            _ => {
                // Highlights / actions / open_questions: bullet list.
                for item in items {
                    self.prefixed_paragraph("•", &item.text, FontStyle::Regular);
                }
            }
        }
        self.gap(1.5);
    }

    /// Bullet- or "Q:" / "A:"-prefixed paragraph. The prefix sits in
    /// a fixed-width gutter, the body wraps to the right and stays
    /// indented on continuation lines.
    fn prefixed_paragraph(&mut self, prefix: &str, text: &str, style: FontStyle) {
        let gutter_w = 5.0;
        let avail_w = BODY_W - BLOCK_INDENT - gutter_w;
        let max_chars = (avail_w / BODY_CHAR_MM) as usize;
        let line_h = pt_to_mm(SIZE_BODY) * 1.3;
        let wrapped = wrap_text(text.trim(), max_chars.max(20));
        if wrapped.is_empty() {
            return;
        }
        self.ensure_room(line_h * (wrapped.len() as f32));
        let baseline_y = self.y - pt_to_mm(SIZE_BODY) * 0.85;
        // Prefix on the first line only.
        let (body_font, _) = self.font_for(style);
        self.layer().use_text(
            prefix,
            SIZE_BODY,
            Mm(MARGIN + BLOCK_INDENT),
            Mm(baseline_y),
            &self.fonts.bold,
        );
        // Body text, possibly multi-line.
        for (i, l) in wrapped.iter().enumerate() {
            let baseline = baseline_y - (i as f32) * line_h;
            self.layer().use_text(
                l.as_str(),
                SIZE_BODY,
                Mm(MARGIN + BLOCK_INDENT + gutter_w),
                Mm(baseline),
                &body_font,
            );
        }
        self.y -= line_h * (wrapped.len() as f32);
    }

    /// One transcript row: `[mm:ss]` timestamp in mono on the left,
    /// body text wrapped on the right.
    fn transcript_item(&mut self, item: &crate::protocol::Item) {
        let ts = format_t(item.t);
        let ts_w_mm = 14.0;
        let line_h = pt_to_mm(SIZE_BODY) * 1.25;
        let body_w = BODY_W - ts_w_mm;
        let max_chars = (body_w / BODY_CHAR_MM) as usize;
        let wrapped = wrap_text(item.text.trim(), max_chars.max(20));
        if wrapped.is_empty() {
            return;
        }
        let block_h = line_h * (wrapped.len() as f32) + 1.0;
        self.ensure_room(block_h);
        let baseline_y = self.y - pt_to_mm(SIZE_BODY) * 0.85;
        // Timestamp on left, mono.
        self.layer().use_text(
            ts.as_str(),
            SIZE_MONO,
            Mm(MARGIN),
            Mm(baseline_y),
            &self.fonts.mono,
        );
        // Body on right, regular.
        for (i, l) in wrapped.iter().enumerate() {
            let baseline = baseline_y - (i as f32) * line_h;
            self.layer().use_text(
                l.as_str(),
                SIZE_BODY,
                Mm(MARGIN + ts_w_mm),
                Mm(baseline),
                &self.fonts.regular,
            );
        }
        self.y -= line_h * (wrapped.len() as f32);
        // Optional speaker tag, dim small italic below the body.
        if let Some(meta) = &item.meta {
            if let Some(speaker) = meta.get("speaker").and_then(|v| v.as_str()) {
                let tag = format!("SPEAKER · {}", speaker);
                let small_h = pt_to_mm(SIZE_SMALL) * 1.4;
                self.ensure_room(small_h);
                let b = self.y - pt_to_mm(SIZE_SMALL) * 0.85;
                self.layer().use_text(
                    tag.as_str(),
                    SIZE_SMALL,
                    Mm(MARGIN + ts_w_mm),
                    Mm(b),
                    &self.fonts.italic,
                );
                self.y -= small_h;
            }
        }
    }

    fn render_moment(&mut self, m: &RenderableMoment) {
        // Visual divider before the moment block — slightly more
        // breathing room than a transcript item gets.
        self.gap(2.5);
        let header = format!(
            "▌ MOMENT at {}{}",
            format_t(m.t as u64),
            m.note
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|n| format!(" — {}", n))
                .unwrap_or_default()
        );
        self.text(&header, SIZE_BODY, FontStyle::Bold);
        // Embed the screenshot, sized to nearly fill body width
        // (the user-asked "make these larger" lives here).
        if let Some(bytes) = &m.screenshot_bytes {
            if let Err(e) = self.embed_screenshot(bytes, BODY_W - 4.0) {
                tracing::warn!(moment_id = %m.id, error = ?e, "embed screenshot failed");
                self.text("(screenshot unavailable)", SIZE_SMALL, FontStyle::Italic);
            }
        } else {
            self.text("(no screenshot)", SIZE_SMALL, FontStyle::Italic);
        }
        // Vision-LLM summary below the image.
        if let Some(summary) = m.summary.as_deref().filter(|s| !s.trim().is_empty()) {
            self.gap(1.0);
            self.paragraph(summary, FontStyle::Italic, SIZE_BODY);
        }
        self.gap(2.0);
    }

    fn embed_screenshot(&mut self, bytes: &[u8], target_width_mm: f32) -> Result<()> {
        // printpdf re-exports its pinned `image` crate as
        // `image_crate`. `load_from_memory` sniffs the format from
        // the magic bytes (PNG / JPEG / WebP / …), then we hand the
        // resulting `DynamicImage` to `ImageXObject::from_dynamic_image`
        // — single code path regardless of source format.
        use printpdf::image_crate;
        use printpdf::image_crate::DynamicImage;
        use printpdf::ImageXObject;
        let dyn_img = image_crate::load_from_memory(bytes)
            .context("load_from_memory: not a recognized image format")?;
        let px_w = image_crate::GenericImageView::width(&dyn_img) as f32;
        let px_h = image_crate::GenericImageView::height(&dyn_img) as f32;
        // Flatten to RGB8 unconditionally. printpdf 0.7's
        // `from_dynamic_image` builds an SMask for any RGBA source,
        // and `From<ImageXObject> for lopdf::Stream` then constructs
        // that mask with `(width, width)` instead of `(width, height)`
        // (printpdf-0.7 src/xobject.rs:267) — so the alpha geometry
        // is wrong and the image renders blank in most readers.
        // Compositing onto white here is the simplest fix; PDF
        // background is always white anyway, so any visible RGBA
        // pixel collapses to its on-white appearance.
        let rgb = dyn_img.to_rgb8();
        let flat = DynamicImage::ImageRgb8(rgb);
        let xobj = ImageXObject::from_dynamic_image(&flat);
        let img = Image::from(xobj);
        // mm = (px / dpi) * 25.4. We want px_w → target_width_mm,
        // so dpi = px_w * 25.4 / target_width_mm.
        let dpi = (px_w * 25.4) / target_width_mm;
        let target_height_mm = (px_h * 25.4) / dpi;
        self.ensure_room(target_height_mm + 2.0);
        // Drop the image at (margin, y - height) — printpdf's image
        // anchors at the bottom-left.
        let translate_x = MARGIN;
        let translate_y = self.y - target_height_mm;
        img.add_to_layer(
            self.layer(),
            ImageTransform {
                translate_x: Some(Mm(translate_x)),
                translate_y: Some(Mm(translate_y)),
                dpi: Some(dpi),
                ..Default::default()
            },
        );
        self.y -= target_height_mm;
        Ok(())
    }
}

// ─── Formatting helpers ──────────────────────────────────────────────────

fn pt_to_mm(pt: f32) -> f32 {
    // 1pt = 1/72 inch, 1 inch = 25.4mm.
    pt * 25.4 / 72.0
}

fn format_t(ms: u64) -> String {
    let total = ms / 1000;
    let m = total / 60;
    let s = total % 60;
    format!("[{:02}:{:02}]", m, s)
}

fn format_timing(start: DateTime<Utc>, end: Option<DateTime<Utc>>) -> String {
    let start_s = start.format("%a, %b %-d %Y %H:%M UTC");
    match end {
        Some(e) => {
            let dur = (e - start).num_seconds().max(0);
            format!(
                "Started {} · Ended {} · Duration {}",
                start_s,
                e.format("%H:%M UTC"),
                format_duration(dur),
            )
        }
        None => format!("Started {} · in progress", start_s),
    }
}

fn format_duration(seconds: i64) -> String {
    if seconds < 60 {
        return format!("{}s", seconds);
    }
    let mins = seconds / 60;
    let rem = seconds % 60;
    if mins < 60 {
        return format!("{}m {}s", mins, rem);
    }
    let hours = mins / 60;
    format!("{}h {}m", hours, mins % 60)
}

#[allow(dead_code)] // Kept for future callers — its test pins
                    // the negative/zero edge cases so we don't have
                    // to redo them next time we need this helper.
fn format_int_with_commas(n: i64) -> String {
    let s = n.abs().to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(b as char);
    }
    if n < 0 {
        format!("-{}", out)
    } else {
        out
    }
}

/// Greedy word-wrap to fit `max_chars`. Splits on whitespace only —
/// long unbroken tokens get pushed onto their own line and may
/// overflow the body width slightly. Good enough for transcript prose.
fn wrap_text(input: &str, max_chars: usize) -> Vec<String> {
    if input.is_empty() {
        return vec![];
    }
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in input.split_whitespace() {
        if cur.is_empty() {
            cur.push_str(word);
        } else if cur.len() + 1 + word.len() <= max_chars {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_short() {
        let r = wrap_text("hello world", 80);
        assert_eq!(r, vec!["hello world"]);
    }

    #[test]
    fn wrap_breaks_on_word_boundary() {
        let r = wrap_text("one two three four five six", 12);
        // Each output line is <= 12 chars.
        for l in &r {
            assert!(l.len() <= 12, "line too long: {l:?}");
        }
        assert_eq!(r.join(" "), "one two three four five six");
    }

    #[test]
    fn wrap_empty_returns_empty() {
        assert!(wrap_text("", 80).is_empty());
    }

    #[test]
    fn format_t_pads() {
        assert_eq!(format_t(0), "[00:00]");
        assert_eq!(format_t(5_000), "[00:05]");
        assert_eq!(format_t(125_000), "[02:05]");
    }

    #[test]
    fn format_int_with_commas_handles_negatives_and_zeros() {
        assert_eq!(format_int_with_commas(0), "0");
        assert_eq!(format_int_with_commas(1_234), "1,234");
        assert_eq!(format_int_with_commas(1_234_567), "1,234,567");
        assert_eq!(format_int_with_commas(-1_234), "-1,234");
    }

    #[test]
    fn render_minimal_meeting_produces_nonempty_pdf() {
        // End-to-end smoke: build a small input, render, assert the
        // bytes start with `%PDF-` (the magic header). Doesn't try
        // to validate layout — just proves the API contract holds.
        let started_at = DateTime::parse_from_rfc3339("2026-05-10T15:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let item = crate::protocol::Item {
            id: "i1".into(),
            text: "we should plan the next milestone".into(),
            detail: None,
            t: 5_000,
            meta: None,
        };
        let input = PdfInput {
            id: "test-meeting-1".to_string(),
            title: "Q2 planning".to_string(),
            description: Some("Working through the next two weeks.".to_string()),
            started_at,
            ended_at: Some(started_at + chrono::Duration::minutes(30)),
            metadata: vec![("project".into(), "helix".into())],
            transcript: vec![item],
            items_by_mode: std::collections::HashMap::new(),
            moments: vec![],
        };
        let bytes = render(&input).expect("render ok");
        assert!(
            bytes.starts_with(b"%PDF-"),
            "expected PDF magic, got {:?}",
            &bytes[..bytes.len().min(8)]
        );
        // Sanity floor: a single-page PDF with text + metadata
        // shouldn't be under 1KB; if it is, something silently
        // dropped content.
        assert!(
            bytes.len() > 1000,
            "PDF suspiciously small: {} bytes",
            bytes.len()
        );
    }
}
