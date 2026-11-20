//! Display-size decoding for fetched images.
//!
//! The desktop shell keeps decoded pixels in memory, so the worker decodes a
//! static JPEG, PNG or WebP at the requested display size instead of handing the
//! shell a full-resolution RGBA buffer. Everything else keeps the encoded file
//! bytes: animated GIF/APNG/animated WebP stay with the shell's animation
//! decoder, and a missing target, an already-small source, an unfriendly format
//! or a corrupt payload all fall back to the encoded path, so an image never
//! fails just because a resize was not possible.
//!
//! Format, dimensions and animation are sniffed from the bytes alone (no
//! network, no filename extension). The sniffers are bounded header/block walks:
//! truncated or garbage input ends the walk instead of panicking, and the caller
//! then gets the encoded fallback.

use std::io::Cursor;

use image::imageops::FilterType;
use image::{ImageFormat, ImageReader};

use crate::request::ImagePayload;

/// Longest source edge the worker decodes for a resize.
///
/// Twitter/X media tops out at a few thousand pixels; a larger edge means the
/// header is lying or the payload is unusual, and the encoded fallback gives the
/// shell its existing full-resolution path instead of risking a huge allocation.
const MAX_DECODE_EDGE: u32 = 16_384;

/// One decode may allocate at most this much (output image included).
const MAX_DECODE_ALLOC_BYTES: u64 = 256 * 1024 * 1024;

/// Frames beyond this are not counted in an animated fallback estimate.
///
/// The estimate is a budgeting hint, not a frame count: a long animation must
/// not be able to claim the whole budget.
const MAX_ESTIMATE_FRAMES: u64 = 32;

/// Upper bound for any encoded-payload decoded-footprint estimate (256 MiB).
const MAX_ESTIMATED_DECODED_BYTES: u64 = 256 * 1024 * 1024;

/// How one fetched image body should be presented to the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DecodeOutcome {
    /// Decoded and resized pixels, straight-alpha RGBA8, row-major.
    Rgba {
        /// Pixel width after resizing; the long edge equals the target.
        width: u32,
        /// Pixel height after resizing.
        height: u32,
        /// Exactly `width * height * 4` bytes.
        bytes: Vec<u8>,
    },
    /// Keep the encoded file bytes; the caller's own decoder renders them.
    Encoded {
        /// Conservative estimate of the decoded RGBA footprint in bytes.
        estimated_decoded_bytes: u64,
    },
}

/// Decide how to present one fetched image body.
///
/// Static JPEG, PNG and WebP sources whose long edge is larger than `target` are
/// decoded and resized with a triangle filter, preserving the aspect ratio.
/// Every other case keeps the encoded bytes: animated images, unsupported
/// formats, missing or non-shrinking targets, and any sniff or decode failure.
pub(crate) fn decode_for_display(bytes: &[u8], target: Option<u32>) -> DecodeOutcome {
    let Some(facts) = sniff(bytes) else {
        // Unknown or unparseable payload: keep it encoded, no budget estimate.
        return DecodeOutcome::Encoded {
            estimated_decoded_bytes: 0,
        };
    };
    if facts.animated || !facts.format.is_resizable() {
        return DecodeOutcome::Encoded {
            estimated_decoded_bytes: facts.estimate(),
        };
    }
    let Some(target) = target.filter(|target| *target > 0) else {
        return DecodeOutcome::Encoded {
            estimated_decoded_bytes: facts.estimate(),
        };
    };
    if facts.width.max(facts.height) <= target {
        return DecodeOutcome::Encoded {
            estimated_decoded_bytes: facts.estimate(),
        };
    }
    let Some((width, height)) = scaled_dimensions(facts.width, facts.height, target) else {
        return DecodeOutcome::Encoded {
            estimated_decoded_bytes: facts.estimate(),
        };
    };
    match decode_resized(bytes, facts.format, width, height) {
        Some(bytes) => DecodeOutcome::Rgba {
            width,
            height,
            bytes,
        },
        None => DecodeOutcome::Encoded {
            estimated_decoded_bytes: facts.estimate(),
        },
    }
}

/// Build the [`ImagePayload`] the worker sends for one fetched image.
pub(crate) fn payload_for_display(bytes: Vec<u8>, target: Option<u32>) -> ImagePayload {
    let outcome = decode_for_display(&bytes, target);
    match outcome {
        DecodeOutcome::Rgba {
            width,
            height,
            bytes,
        } => ImagePayload::Rgba {
            width,
            height,
            bytes,
        },
        DecodeOutcome::Encoded {
            estimated_decoded_bytes,
        } => ImagePayload::Encoded {
            bytes,
            estimated_decoded_bytes,
        },
    }
}

/// Header facts parsed without decoding pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeaderFacts {
    format: Format,
    width: u32,
    height: u32,
    animated: bool,
    frames: u64,
}

impl HeaderFacts {
    /// Conservative decoded-footprint estimate for the encoded fallback.
    fn estimate(&self) -> u64 {
        if self.animated {
            animated_estimate(self.width, self.height, self.frames)
        } else {
            static_estimate(self.width, self.height)
        }
    }
}

/// Image container formats this crate can sniff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Jpeg,
    Png,
    Gif,
    WebP,
}

impl Format {
    /// Whether a static image of this format may be decoded and resized.
    fn is_resizable(self) -> bool {
        matches!(self, Self::Jpeg | Self::Png | Self::WebP)
    }

    /// The matching decoder format for the `image` crate.
    fn image_format(self) -> ImageFormat {
        match self {
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Png => ImageFormat::Png,
            Self::Gif => ImageFormat::Gif,
            Self::WebP => ImageFormat::WebP,
        }
    }
}

/// Sniff format, dimensions and animation from the payload bytes.
fn sniff(bytes: &[u8]) -> Option<HeaderFacts> {
    let format = detect_format(bytes)?;
    let (width, height) = source_dimensions(bytes, format)?;
    if width == 0 || height == 0 {
        return None;
    }
    let (animated, frames) = animation_state(bytes, format);
    Some(HeaderFacts {
        format,
        width,
        height,
        animated,
        frames,
    })
}

/// Detect the container format from its magic bytes.
fn detect_format(bytes: &[u8]) -> Option<Format> {
    const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.starts_with(&PNG_SIGNATURE) {
        return Some(Format::Png);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(Format::Gif);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Format::Jpeg);
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(Format::WebP);
    }
    None
}

/// Read the source pixel dimensions from the container header.
fn source_dimensions(bytes: &[u8], format: Format) -> Option<(u32, u32)> {
    match format {
        Format::Jpeg => jpeg_dimensions(bytes),
        Format::Png => png_dimensions(bytes),
        Format::Gif => gif_dimensions(bytes),
        Format::WebP => webp_dimensions(bytes),
    }
}

/// Read the PNG `IHDR` width and height.
fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    // Layout at offset 8: length (4), "IHDR" (4), width (4), height (4).
    let chunk = bytes.get(8..24)?;
    if &chunk[4..8] != b"IHDR" {
        return None;
    }
    Some((
        u32::from_be_bytes(chunk[8..12].try_into().ok()?),
        u32::from_be_bytes(chunk[12..16].try_into().ok()?),
    ))
}

/// Read the GIF logical screen width and height.
fn gif_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let dimensions = bytes.get(6..10)?;
    Some((
        u32::from(u16::from_le_bytes(dimensions[0..2].try_into().ok()?)),
        u32::from(u16::from_le_bytes(dimensions[2..4].try_into().ok()?)),
    ))
}

/// Read the JPEG frame dimensions from the first start-of-frame segment.
fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    // Skip the SOI marker (FF D8).
    let mut offset = 2usize;
    while offset + 2 <= bytes.len() {
        if bytes[offset] != 0xFF {
            return None;
        }
        // A segment may be preceded by any number of 0xFF fill bytes.
        let mut marker_offset = offset + 1;
        while marker_offset < bytes.len() && bytes[marker_offset] == 0xFF {
            marker_offset += 1;
        }
        let marker = *bytes.get(marker_offset)?;
        offset = marker_offset + 1;
        match marker {
            // Standalone markers carry no length field.
            0x01 | 0xD0..=0xD7 => continue,
            // End of image or start of scan before any SOF: no dimensions.
            0xD9 | 0xDA => return None,
            _ => {}
        }
        let length = usize::from(u16::from_be_bytes([
            *bytes.get(offset)?,
            *bytes.get(offset + 1)?,
        ]));
        if length < 2 {
            return None;
        }
        let segment_end = offset.checked_add(length)?;
        if segment_end > bytes.len() {
            // Truncated segment: the walk cannot continue safely.
            return None;
        }
        // SOF0..SOF15 except DHT (C4), JPG (C8) and DAC (CC) hold the frame size.
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let data = &bytes[offset + 2..segment_end];
            if data.len() < 5 {
                return None;
            }
            // `data[0]` is precision, then height and width as big-endian u16.
            let height = u32::from(u16::from_be_bytes([data[1], data[2]]));
            let width = u32::from(u16::from_be_bytes([data[3], data[4]]));
            if width == 0 || height == 0 {
                return None;
            }
            return Some((width, height));
        }
        offset = segment_end;
    }
    None
}

/// Read the WebP canvas or first-frame dimensions from the RIFF chunks.
fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let chunk_type = bytes.get(offset..offset + 4)?;
        let size = u32::from_le_bytes(bytes.get(offset + 4..offset + 8)?.try_into().ok()?);
        let size = usize::try_from(size).ok()?;
        let data_start = offset + 8;
        let data_end = data_start.checked_add(size)?;
        // Truncated chunk: stop rather than trust a partial header.
        if data_end > bytes.len() {
            return None;
        }
        let data = &bytes[data_start..data_end];
        match chunk_type {
            b"VP8X" => {
                // Flags (1), reserved (3), canvas width-1 (3), height-1 (3).
                if data.len() < 10 {
                    return None;
                }
                return Some((
                    u24_le(&data[4..7])?.saturating_add(1),
                    u24_le(&data[7..10])?.saturating_add(1),
                ));
            }
            b"VP8 " => {
                // Frame tag (3), start code 9D 01 2A (3), width and height u16.
                if data.len() < 10 || data.get(3..6)? != [0x9D, 0x01, 0x2A] {
                    return None;
                }
                let width =
                    u32::from(u16::from_le_bytes(data.get(6..8)?.try_into().ok()?) & 0x3FFF);
                let height =
                    u32::from(u16::from_le_bytes(data.get(8..10)?.try_into().ok()?) & 0x3FFF);
                return Some((width, height));
            }
            b"VP8L" => {
                // Signature 0x2F, then 14-bit width-1 and height-1 in a u32.
                if data.len() < 5 || data[0] != 0x2F {
                    return None;
                }
                let header = u32::from_le_bytes(data.get(1..5)?.try_into().ok()?);
                let width = (1 + header) & 0x3FFF;
                let height = (1 + (header >> 14)) & 0x3FFF;
                return Some((width, height));
            }
            _ => {}
        }
        let padded = size.checked_add(size & 1)?;
        offset = data_start.checked_add(padded)?;
    }
    None
}

/// Detect animation and estimate the frame count from the container header.
fn animation_state(bytes: &[u8], format: Format) -> (bool, u64) {
    match format {
        Format::Jpeg => (false, 1),
        Format::Png => png_animation_state(bytes),
        Format::Gif => gif_animation_state(bytes),
        Format::WebP => webp_animation_state(bytes),
    }
}

/// APNG carries `acTL`/`fcTL` chunks before the first `IDAT`.
fn png_animation_state(bytes: &[u8]) -> (bool, u64) {
    let mut offset = 8usize;
    let mut animated = false;
    let mut frames = 0u64;
    while offset + 8 <= bytes.len() {
        let Some(length) = bytes
            .get(offset..offset + 4)
            .and_then(|raw| raw.try_into().ok())
            .map(u32::from_be_bytes)
        else {
            break;
        };
        let Ok(length) = usize::try_from(length) else {
            break;
        };
        let Some(chunk_type) = bytes.get(offset + 4..offset + 8) else {
            break;
        };
        let data_start = offset + 8;
        let Some(data_end) = data_start.checked_add(length) else {
            break;
        };
        // Truncated chunk: the rest of the header cannot be trusted.
        if data_end > bytes.len() {
            break;
        }
        match chunk_type {
            b"acTL" => {
                animated = true;
                if length >= 4 {
                    if let Some(data) = bytes.get(data_start..data_start + 4) {
                        if let Ok(number) = <[u8; 4]>::try_from(data) {
                            frames = u64::from(u32::from_be_bytes(number));
                        }
                    }
                }
                // `acTL` holds the authoritative frame count; normal APNGs put
                // it before any frame control chunk.
                break;
            }
            b"fcTL" => {
                animated = true;
                frames = frames.saturating_add(1);
            }
            b"IDAT" | b"IEND" => break,
            _ => {}
        }
        let Some(next) = data_end.checked_add(4) else {
            break;
        };
        offset = next;
    }
    (animated, frames.max(1))
}

/// GIF animations have more than one image descriptor before the trailer.
///
/// The walk follows the block structure (extensions, image descriptors, LZW
/// data sub-blocks) without decoding pixels, so a `0x2C` byte inside image data
/// is never mistaken for a frame.
fn gif_animation_state(bytes: &[u8]) -> (bool, u64) {
    let Some(mut offset) = gif_blocks_start(bytes) else {
        return (false, 1);
    };
    let mut frames = 0u64;
    while let Some(&introducer) = bytes.get(offset) {
        match introducer {
            0x3B => break, // trailer
            0x21 => {
                // Extension: label byte, then length-prefixed sub-blocks.
                let Some(_label) = bytes.get(offset + 1) else {
                    break;
                };
                match skip_gif_sub_blocks(bytes, offset + 2) {
                    Some(next) => offset = next,
                    None => break,
                }
            }
            0x2C => {
                frames = frames.saturating_add(1);
                let descriptor = offset + 1;
                let Some(packed) = bytes.get(descriptor + 8).copied() else {
                    break;
                };
                let mut next = descriptor + 9;
                if packed & 0x80 != 0 {
                    let color_bits = u32::from(packed & 0x07) + 1;
                    let Some(after_table) = next.checked_add(3usize << color_bits) else {
                        break;
                    };
                    next = after_table;
                }
                // LZW minimum code size byte, then the image's data sub-blocks.
                let Some(after_min_code_size) = next.checked_add(1) else {
                    break;
                };
                match skip_gif_sub_blocks(bytes, after_min_code_size) {
                    Some(after) => offset = after,
                    None => break,
                }
            }
            _ => break, // malformed block introducer
        }
    }
    (frames > 1, frames.max(1))
}

/// Offset of the first GIF block, after the header and optional global colour
/// table (13 bytes of header, then `3 * 2^(N+1)` colour bytes).
fn gif_blocks_start(bytes: &[u8]) -> Option<usize> {
    let packed = *bytes.get(10)?;
    let mut offset = 13usize;
    if packed & 0x80 != 0 {
        let color_bits = u32::from(packed & 0x07) + 1;
        offset = offset.checked_add(3usize << color_bits)?;
    }
    Some(offset)
}

/// Skip one GIF sub-block chain; returns the offset just past the terminator.
fn skip_gif_sub_blocks(bytes: &[u8], mut offset: usize) -> Option<usize> {
    loop {
        let size = usize::from(*bytes.get(offset)?);
        offset = offset.checked_add(1)?;
        if size == 0 {
            return Some(offset);
        }
        offset = offset.checked_add(size)?;
        if offset > bytes.len() {
            return None;
        }
    }
}

/// Animated WebP announces itself with the VP8X animation flag or ANIM/ANMF.
fn webp_animation_state(bytes: &[u8]) -> (bool, u64) {
    let mut offset = 12usize;
    let mut animated = false;
    let mut frames = 0u64;
    while offset + 8 <= bytes.len() {
        let Some(chunk_type) = bytes.get(offset..offset + 4) else {
            break;
        };
        let Some(size) = bytes
            .get(offset + 4..offset + 8)
            .and_then(|raw| raw.try_into().ok())
            .map(u32::from_le_bytes)
        else {
            break;
        };
        let Ok(size) = usize::try_from(size) else {
            break;
        };
        let data_start = offset + 8;
        let Some(data_end) = data_start.checked_add(size) else {
            break;
        };
        if data_end > bytes.len() {
            break;
        }
        match chunk_type {
            b"VP8X" => {
                if let Some(flags) = bytes.get(data_start) {
                    // Flag bit 1 is the animation flag.
                    animated |= flags & 0x02 != 0;
                }
            }
            b"ANIM" => animated = true,
            b"ANMF" => {
                animated = true;
                frames = frames.saturating_add(1);
            }
            _ => {}
        }
        let Some(next) = data_end.checked_add(size & 1) else {
            break;
        };
        offset = next;
    }
    (animated, frames.max(1))
}

/// Scale `width x height` so the long edge becomes `target`, preserving aspect.
fn scaled_dimensions(width: u32, height: u32, target: u32) -> Option<(u32, u32)> {
    let long_edge = width.max(height);
    if long_edge <= target {
        return None;
    }
    let target = u64::from(target.max(1));
    let long_edge = u64::from(long_edge);
    let scaled = |edge: u32| {
        // Rounded integer scaling keeps the aspect ratio and never reaches zero.
        let edge = (u64::from(edge) * target + long_edge / 2) / long_edge;
        u32::try_from(edge.max(1)).ok()
    };
    Some((scaled(width)?, scaled(height)?))
}

/// Decode one static image and resize it with a triangle filter.
///
/// Returns `None` on any decode error: the caller falls back to the encoded
/// bytes, so a resize that is unavailable never fails the image.
fn decode_resized(bytes: &[u8], format: Format, width: u32, height: u32) -> Option<Vec<u8>> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format.image_format());
    reader.limits(decode_limits());
    let decoded = reader.decode().ok()?;
    let rgba = decoded.to_rgba8();
    let resized = image::imageops::resize(&rgba, width, height, FilterType::Triangle);
    Some(resized.into_raw())
}

/// Decode limits that keep one worker resize inside a predictable allocation.
fn decode_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_EDGE);
    limits.max_image_height = Some(MAX_DECODE_EDGE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC_BYTES);
    limits
}

/// Conservative decoded footprint of one static frame.
fn static_estimate(width: u32, height: u32) -> u64 {
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(4)
        .min(MAX_ESTIMATED_DECODED_BYTES)
}

/// Conservative decoded footprint of an animation: `w * h * 4 * 4/3 * frames`.
fn animated_estimate(width: u32, height: u32, frames: u64) -> u64 {
    let frames = frames.clamp(1, MAX_ESTIMATE_FRAMES);
    let frame_bytes = u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(4);
    frame_bytes
        .saturating_mul(4)
        .saturating_mul(frames)
        .saturating_div(3)
        .min(MAX_ESTIMATED_DECODED_BYTES)
}

/// Read a 24-bit little-endian integer from exactly three bytes.
fn u24_le(bytes: &[u8]) -> Option<u32> {
    let bytes: [u8; 3] = bytes.try_into().ok()?;
    Some(u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        animated_gif_bytes, animated_webp_bytes, apng_bytes, jpeg_bytes, png_bytes, webp_bytes,
    };

    fn rgba(outcome: DecodeOutcome) -> (u32, u32, Vec<u8>) {
        match outcome {
            DecodeOutcome::Rgba {
                width,
                height,
                bytes,
            } => (width, height, bytes),
            other => panic!("expected decoded rgba, got {other:?}"),
        }
    }

    fn estimate(outcome: DecodeOutcome) -> u64 {
        match outcome {
            DecodeOutcome::Encoded {
                estimated_decoded_bytes,
            } => estimated_decoded_bytes,
            other => panic!("expected encoded fallback, got {other:?}"),
        }
    }

    #[test]
    fn static_png_is_resized_to_the_target_long_edge() {
        let source = png_bytes(8, 4, [10, 20, 30, 128]);
        let (width, height, bytes) = rgba(decode_for_display(&source, Some(4)));
        assert_eq!((width, height), (4, 2));
        assert_eq!(bytes.len(), 4 * 2 * 4);
        // Resizing a solid colour keeps the straight-alpha value as-is.
        assert_eq!((bytes[0], bytes[1], bytes[2], bytes[3]), (10, 20, 30, 128));
    }

    #[test]
    fn static_jpeg_is_resized_and_has_no_alpha() {
        let source = jpeg_bytes(16, 8);
        let (width, height, bytes) = rgba(decode_for_display(&source, Some(8)));
        assert_eq!((width, height), (8, 4));
        assert_eq!(bytes.len(), 8 * 4 * 4);
        assert!(bytes.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255));
    }

    #[test]
    fn static_webp_is_resized() {
        let source = webp_bytes(20, 10);
        let (width, height, bytes) = rgba(decode_for_display(&source, Some(10)));
        assert_eq!((width, height), (10, 5));
        assert_eq!(bytes.len(), 10 * 5 * 4);
    }

    #[test]
    fn portrait_sources_keep_their_aspect_ratio() {
        let source = png_bytes(4, 8, [0, 0, 0, 255]);
        let (width, height, _) = rgba(decode_for_display(&source, Some(2)));
        assert_eq!((width, height), (1, 2));
    }

    #[test]
    fn missing_target_keeps_the_encoded_bytes() {
        let source = png_bytes(8, 4, [0, 0, 0, 255]);
        assert_eq!(estimate(decode_for_display(&source, None)), 8 * 4 * 4);
    }

    #[test]
    fn sources_at_or_below_the_target_stay_encoded() {
        let source = png_bytes(8, 4, [0, 0, 0, 255]);
        let static_bytes = 8u64 * 4 * 4;
        assert_eq!(estimate(decode_for_display(&source, Some(8))), static_bytes);
        assert_eq!(
            estimate(decode_for_display(&source, Some(64))),
            static_bytes
        );
        // A zero target is a caller bug: never resize to nothing.
        assert_eq!(estimate(decode_for_display(&source, Some(0))), static_bytes);
    }

    #[test]
    fn animated_gif_stays_encoded_with_a_frame_aware_estimate() {
        let source = animated_gif_bytes(8, 4, 2);
        let estimate = estimate(decode_for_display(&source, Some(2)));
        let one_frame = 8u64 * 4 * 4;
        assert!(
            estimate > one_frame,
            "two frames must cost more than one: {estimate}"
        );
        assert!(
            estimate <= one_frame * 4 * MAX_ESTIMATE_FRAMES / 3,
            "estimate must stay clamped: {estimate}"
        );
    }

    #[test]
    fn animated_webp_stays_encoded_with_a_frame_aware_estimate() {
        let source = animated_webp_bytes(8, 6, 2);
        let estimate = estimate(decode_for_display(&source, Some(2)));
        assert!(estimate > 8u64 * 6 * 4, "estimate was {estimate}");
    }

    #[test]
    fn apng_stays_encoded_with_a_frame_aware_estimate() {
        let source = apng_bytes(8, 4, 3);
        let estimate = estimate(decode_for_display(&source, Some(2)));
        assert!(estimate > 8u64 * 4 * 4, "estimate was {estimate}");
    }

    #[test]
    fn single_frame_gif_stays_encoded() {
        let source = animated_gif_bytes(8, 4, 1);
        assert_eq!(estimate(decode_for_display(&source, Some(2))), 8 * 4 * 4);
    }

    #[test]
    fn corrupt_and_truncated_bodies_never_panic() {
        let png = png_bytes(8, 8, [0, 0, 0, 255]);
        let jpeg = jpeg_bytes(8, 8);
        let gif = animated_gif_bytes(8, 4, 2);
        let webp = animated_webp_bytes(8, 4, 2);
        let bodies: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"not an image at all".to_vec(),
            png[..4].to_vec(),
            png[..20].to_vec(),
            png[..png.len() - 4].to_vec(),
            jpeg[..2].to_vec(),
            gif[..8].to_vec(),
            gif[..14].to_vec(),
            webp[..12].to_vec(),
            b"RIFF\x0c\x00\x00\x00WEBPVP8X\xff\xff".to_vec(),
            vec![0xFF; 64],
        ];
        for body in bodies {
            // Any outcome is acceptable as long as sniffing and decoding do not
            // panic and a failure still yields the encoded fallback.
            let outcome = decode_for_display(&body, Some(2));
            assert!(
                matches!(
                    outcome,
                    DecodeOutcome::Encoded { .. } | DecodeOutcome::Rgba { .. }
                ),
                "unexpected outcome {outcome:?}"
            );
        }
        // Corrupt bodies are encoded fallbacks without a usable estimate.
        assert_eq!(estimate(decode_for_display(b"garbage", Some(2))), 0);
        assert_eq!(estimate(decode_for_display(&png[..20], Some(2))), 0);
    }

    #[test]
    fn payload_for_display_builds_both_variants() {
        let source = png_bytes(8, 4, [0, 0, 0, 255]);
        let decoded = payload_for_display(source.clone(), Some(4));
        match &decoded {
            ImagePayload::Rgba {
                width,
                height,
                bytes,
            } => {
                assert_eq!((*width, *height), (4, 2));
                assert_eq!(bytes.len(), 32);
            }
            other => panic!("expected rgba payload, got {other:?}"),
        }
        assert_eq!(decoded.estimated_decoded_bytes(), 32);

        let encoded = payload_for_display(source, None);
        match &encoded {
            ImagePayload::Encoded {
                bytes,
                estimated_decoded_bytes,
            } => {
                assert!(!bytes.is_empty());
                assert_eq!(*estimated_decoded_bytes, 8 * 4 * 4);
            }
            other => panic!("expected encoded payload, got {other:?}"),
        }
    }
}
