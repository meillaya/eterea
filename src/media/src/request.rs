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
}

impl ImageRequest {
    /// Build a request.
    pub fn new(id: impl Into<RequestId>, url: impl Into<String>, kind: ImageKind) -> Self {
        Self {
            id: id.into(),
            url: url.into(),
            kind,
        }
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

/// A completed media request.
///
/// Events are drained with [`crate::MediaManager::try_recv`]. Each enqueued
/// request yields at most one event, and a cancelled request yields none.
#[derive(PartialEq, Eq)]
pub enum MediaEvent {
    /// The image bytes are ready.
    ImageReady {
        /// Caller-assigned id of the request.
        id: RequestId,
        /// Which image slot this is for.
        kind: ImageKind,
        /// Whether the bytes came from the network or the disk cache.
        source: ImageSource,
        /// Complete image payload (at most [`crate::MAX_RESPONSE_BYTES`]).
        bytes: Vec<u8>,
    },
    /// The request failed; no bytes are available.
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
                bytes,
            } => f
                .debug_struct("ImageReady")
                .field("id", id)
                .field("kind", kind)
                .field("source", source)
                .field("bytes_len", &bytes.len())
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
    use super::{ImageKind, ImageRequest, MediaEvent, RequestId};
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
    }

    #[test]
    fn event_debug_does_not_dump_bytes() {
        let event = MediaEvent::ImageReady {
            id: RequestId(1),
            kind: ImageKind::Image,
            source: super::ImageSource::Network,
            bytes: vec![1, 2, 3],
        };
        let text = format!("{event:?}");
        assert!(text.contains("bytes_len: 3"));
        assert!(!text.contains("[1, 2, 3]"));
    }

    #[test]
    fn failure_display_is_short_and_url_free() {
        let error = MediaError::HttpStatus(404);
        assert_eq!(error.to_string(), "server returned HTTP 404");
        assert!(!error.to_string().contains("https://"));
    }
}
