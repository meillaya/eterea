//! Bounded properties for [`eterea_logic::is_image_mime`] (INV-MED-09).
//!
//! `is_image_mime` is the second pure function in `src/logic`. It guards which
//! remote image bytes the media pipeline is willing to decode, so its allowlist
//! is security-relevant: it must be exactly the image formats eterea can
//! decode, matched case-insensitively, ignoring `;`-parameters.

use eterea_logic::is_image_mime;

/// MIME strings that must be accepted: the decoder allowlist, with the cases
/// and parameters that appear in real servers' `Content-Type` headers.
pub const MUST_ACCEPT: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
    "image/avif",
    "IMAGE/JPEG",
    "Image/Png",
    "image/jpeg; charset=binary",
    "image/png; charset=binary",
    "image/webp; q=1",
    "  image/gif  ",
];

/// MIME strings that must be rejected: other media, markup, scripts, empty and
/// lookalike types.
pub const MUST_REJECT: &[&str] = &[
    "",
    "image/",
    "image/svg+xml",
    "image/jpeg2",
    "image/pngx",
    "x-image/png",
    "application/octet-stream",
    "text/html",
    "text/plain",
    "video/mp4",
    "audio/mpeg",
    "application/json",
];

/// The MIME essence (type/subtype without parameters), lower-cased.
pub fn essence(mime: &str) -> String {
    mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

/// The decoded image formats eterea supports (kept here as the readable
/// allowlist; comparison is against the lower-cased essence).
pub const ALLOWED_ESSENCES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp", "image/avif"];

/// Returns a reason when `mime` violates the allowlist contract. `None` for
/// rejected values (rejection is always allowed).
pub fn contract_violation(mime: &str) -> Option<&'static str> {
    if !is_image_mime(mime) {
        return None;
    }
    if !ALLOWED_ESSENCES.contains(&essence(mime).as_str()) {
        return Some("INV-MED-09: accepted a MIME outside the decoder allowlist");
    }
    None
}
