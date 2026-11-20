//! Request and event types exchanged with the media manager.

use std::fmt;

/// Caller-assigned identity of a media request.
///
/// Ids are assigned by the caller (the desktop shell) so a response can be
/// matched to the image slot that asked for it. An id may be reused after its
/// request has completed or has been cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct RequestId(pub u64);

impl RequestId {
    /// Wrap a raw id.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the raw id.
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl From<u64> for RequestId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<RequestId> for u64 {
    fn from(value: RequestId) -> Self {
        value.0
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Which part of the UI asked for an image.
///
/// The kind is carried through the queue and back so callers can route the
/// bytes without keeping a second map of id to slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageKind {
    /// Author avatar, loaded by default.
    Avatar,
    /// Post media image.
    Image,
    /// Video poster or other preview still.
    Poster,
}

impl ImageKind {
    /// Stable lowercase label used in cache keys and diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Avatar => "avatar",
            Self::Image => "image",
            Self::Poster => "poster",
        }
    }
}

impl fmt::Display for ImageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One image to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRequest {
    /// Caller-assigned id.
    pub id: RequestId,
    /// HTTPS URL to fetch.
    pub url: String,
    /// Which image slot this is for.
    pub kind: ImageKind,
    /// Longest edge of the display size in device pixels.
    ///
    /// The worker decodes static JPEG/PNG/WebP payloads at this size and
    /// delivers straight-alpha RGBA pixels. [`None`] keeps the original encoded
    /// bytes: the shell's own decoder renders them at full resolution, which is
    /// what the caller wants when it will zoom past the display size.
    pub target: Option<u32>,
}

impl ImageRequest {
    /// Build a request that keeps the original encoded bytes.
    pub fn new(id: impl Into<RequestId>, url: impl Into<String>, kind: ImageKind) -> Self {
        Self {
            id: id.into(),
            url: url.into(),
            kind,
            target: None,
        }
    }

    /// Decode at `target` device pixels on the longest edge (clamped to at
    /// least one pixel) instead of keeping the original bytes.
    pub fn with_target(mut self, target: u32) -> Self {
        self.target = Some(target.max(1));
        self
    }

    /// Set the display target explicitly; [`None`] keeps the original bytes.
    pub fn with_optional_target(mut self, target: Option<u32>) -> Self {
        self.target = target.map(|target| target.max(1));
        self
    }
}

/// Where the bytes of a successful image came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageSource {
    /// Fetched from the network on this request.
    Network,
    /// Served from the opt-in disk cache.
    DiskCache,
}

/// The display-ready payload of one fetched image.
///
/// The worker either decodes the image at the requested display size or leaves
/// it encoded for the shell's own decoder. The bytes are never written to the
/// disk cache in this form: the cache keeps the raw response for the cache key.
#[derive(Clone, PartialEq, Eq)]
pub enum ImagePayload {
    /// Decoded and resized pixels, straight-alpha RGBA8, row-major.
    Rgba {
        /// Pixel width after resizing.
        width: u32,
        /// Pixel height after resizing.
        height: u32,
        /// Exactly `width * height * 4` bytes of RGBA8.
        bytes: Vec<u8>,
    },
    /// The original encoded image bytes (at most [`crate::MAX_RESPONSE_BYTES`]).
    Encoded {
        /// Complete encoded image bytes.
        bytes: Vec<u8>,
        /// Conservative estimate of the decoded RGBA footprint in bytes, for
        /// the caller's memory budget.
        estimated_decoded_bytes: u64,
    },
}

impl ImagePayload {
    /// The payload bytes, whether decoded RGBA or encoded file data.
    pub fn bytes(&self) -> &[u8] {
        match self {
            Self::Rgba { bytes, .. } | Self::Encoded { bytes, .. } => bytes,
        }
    }

    /// Conservative decoded-footprint estimate in bytes.
    pub fn estimated_decoded_bytes(&self) -> u64 {
        match self {
            Self::Rgba { width, height, .. } => u64::from(*width) * u64::from(*height) * 4,
            Self::Encoded {
                estimated_decoded_bytes,
                ..
            } => *estimated_decoded_bytes,
        }
    }

    /// Whether the payload is still an encoded file for the caller to decode.
    pub fn is_encoded(&self) -> bool {
        matches!(self, Self::Encoded { .. })
    }
}

impl fmt::Debug for ImagePayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rgba {
                width,
                height,
                bytes,
            } => f
                .debug_struct("Rgba")
                .field("width", width)
                .field("height", height)
                .field("bytes_len", &bytes.len())
                .finish(),
            Self::Encoded {
                bytes,
                estimated_decoded_bytes,
            } => f
                .debug_struct("Encoded")
                .field("bytes_len", &bytes.len())
                .field("estimated_decoded_bytes", estimated_decoded_bytes)
                .finish(),
        }
    }
}

/// A completed media request.
///
/// Events are drained with [`crate::MediaManager::try_recv`]. Each enqueued
/// request yields at most one event, and a cancelled request yields none.
#[derive(PartialEq, Eq)]
pub enum MediaEvent {
    /// The image payload is ready.
    ImageReady {
        /// Caller-assigned id of the request.
        id: RequestId,
        /// Which image slot this is for.
        kind: ImageKind,
        /// Whether the bytes came from the network or the disk cache.
        source: ImageSource,
        /// Decoded pixels or the original encoded bytes.
        payload: ImagePayload,
    },
    /// The request failed; no image is available.
    ImageFailed {
        /// Caller-assigned id of the request.
        id: RequestId,
        /// Which image slot this was for.
        kind: ImageKind,
        /// Why the fetch failed.
        error: MediaError,
    },
}

impl fmt::Debug for MediaEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ImageReady {
                id,
                kind,
                source,
                payload,
            } => f
                .debug_struct("ImageReady")
                .field("id", id)
                .field("kind", kind)
                .field("source", source)
                .field("payload", payload)
                .finish(),
            Self::ImageFailed { id, kind, error } => f
                .debug_struct("ImageFailed")
                .field("id", id)
                .field("kind", kind)
                .field("error", error)
                .finish(),
        }
    }
}

use crate::error::MediaError;

#[cfg(test)]
mod tests {
    use super::{ImageKind, ImagePayload, ImageRequest, MediaEvent, RequestId};
    use crate::MediaError;

    #[test]
    fn request_id_round_trips() {
        let id = RequestId::from(42_u64);
        assert_eq!(id.value(), 42);
        assert_eq!(u64::from(id), 42);
        assert_eq!(id.to_string(), "42");
        assert_eq!(RequestId::new(42), id);
    }

    #[test]
    fn image_kinds_have_stable_labels() {
        assert_eq!(ImageKind::Avatar.as_str(), "avatar");
        assert_eq!(ImageKind::Image.as_str(), "image");
        assert_eq!(ImageKind::Poster.as_str(), "poster");
    }

    #[test]
    fn request_constructor_normalises_id() {
        let request = ImageRequest::new(7_u64, "https://pbs.twimg.com/x.jpg", ImageKind::Image);
        assert_eq!(request.id, RequestId(7));
        assert_eq!(request.kind, ImageKind::Image);
        assert_eq!(request.target, None, "the default keeps encoded bytes");
    }

    #[test]
    fn request_target_builders_clamp_to_one_pixel() {
        let request =
            ImageRequest::new(1_u64, "https://x.com/a.jpg", ImageKind::Image).with_target(0);
        assert_eq!(request.target, Some(1));
        let request = ImageRequest::new(1_u64, "https://x.com/a.jpg", ImageKind::Image)
            .with_optional_target(Some(720));
        assert_eq!(request.target, Some(720));
        let request = ImageRequest::new(1_u64, "https://x.com/a.jpg", ImageKind::Image)
            .with_optional_target(None);
        assert_eq!(request.target, None);
    }

    #[test]
    fn payload_reports_body_and_budget_estimate() {
        let rgba = ImagePayload::Rgba {
            width: 4,
            height: 2,
            bytes: vec![0; 32],
        };
        assert_eq!(rgba.bytes().len(), 32);
        assert_eq!(rgba.estimated_decoded_bytes(), 32);
        assert!(!rgba.is_encoded());

        let encoded = ImagePayload::Encoded {
            bytes: vec![1, 2, 3],
            estimated_decoded_bytes: 4096,
        };
        assert_eq!(encoded.bytes(), &[1, 2, 3]);
        assert_eq!(encoded.estimated_decoded_bytes(), 4096);
        assert!(encoded.is_encoded());
    }

    #[test]
    fn event_debug_does_not_dump_bytes() {
        let event = MediaEvent::ImageReady {
            id: RequestId(1),
            kind: ImageKind::Image,
            source: super::ImageSource::Network,
            payload: ImagePayload::Encoded {
                bytes: vec![1, 2, 3],
                estimated_decoded_bytes: 12,
            },
        };
        let text = format!("{event:?}");
        assert!(text.contains("bytes_len: 3"));
        assert!(text.contains("estimated_decoded_bytes: 12"));
        assert!(!text.contains("[1, 2, 3]"));
    }

    #[test]
    fn failure_display_is_short_and_url_free() {
        let error = MediaError::HttpStatus(404);
        assert_eq!(error.to_string(), "server returned HTTP 404");
        assert!(!error.to_string().contains("https://"));
    }
}
