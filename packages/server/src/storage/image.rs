//! Extension → image-kind mapping for stored screenshot/photo assets.
//!
//! Single source of truth for turning a stored asset's file extension
//! into (a) the canonical mime type to serve over HTTP and (b) the
//! `rig_core` `ImageMediaType` to pass into vision LLM calls. Assets
//! land on disk from two producers — the Mac's auto-screenshot (PNG)
//! and the mobile camera capture (JPEG) — and both the moment-
//! screenshot GET handler and the moment-summary worker need to know
//! which one a given `asset_path` is. Keeping the mapping in one
//! place avoids the two copies drifting (see the mobile-moment-photo
//! branch review, fix #1: the worker used to hardcode PNG, which
//! broke vision extraction for JPEG phone photos on strict providers
//! like Anthropic/Bedrock that validate media type against bytes).

use rig_core::completion::message::ImageMediaType;

/// Image kind derived from a stored asset's extension: the canonical
/// mime type (for HTTP responses) plus rig's `ImageMediaType` (for
/// vision LLM calls).
#[derive(Debug, Clone, PartialEq)]
pub struct ImageKind {
    pub mime: &'static str,
    pub media_type: ImageMediaType,
}

const PNG: ImageKind = ImageKind {
    mime: "image/png",
    media_type: ImageMediaType::PNG,
};
const JPEG: ImageKind = ImageKind {
    mime: "image/jpeg",
    media_type: ImageMediaType::JPEG,
};

/// Classify a stored asset by its path's extension (case-insensitive).
/// Legacy rows (and anything unrecognized) fall back to PNG, which is
/// what every pre-JPEG upload actually was.
pub fn image_kind_for_path(asset_path: &str) -> ImageKind {
    match asset_path.rsplit('.').next() {
        Some(e) if e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg") => JPEG,
        _ => PNG,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpg_and_jpeg_extensions_map_to_jpeg() {
        assert_eq!(image_kind_for_path("a/b/c.jpg"), JPEG);
        assert_eq!(image_kind_for_path("a/b/c.jpeg"), JPEG);
        assert_eq!(image_kind_for_path("a/b/c.JPG"), JPEG);
        assert_eq!(image_kind_for_path("a/b/c.JPEG"), JPEG);
    }

    #[test]
    fn png_and_unknown_fall_back_to_png() {
        assert_eq!(image_kind_for_path("a/b/c.png"), PNG);
        assert_eq!(image_kind_for_path("a/b/c"), PNG);
        assert_eq!(image_kind_for_path("a/b/c.gif"), PNG);
        assert_eq!(image_kind_for_path(""), PNG);
    }

    #[test]
    fn mime_and_media_type_are_consistent() {
        let jpg = image_kind_for_path("x.jpg");
        assert_eq!(jpg.mime, "image/jpeg");
        assert_eq!(jpg.media_type, ImageMediaType::JPEG);
        let png = image_kind_for_path("x.png");
        assert_eq!(png.mime, "image/png");
        assert_eq!(png.media_type, ImageMediaType::PNG);
    }
}
