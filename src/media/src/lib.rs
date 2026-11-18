//! Media fetching, caching and playback for eterea.
//!
//! The crate is a network trust boundary. Every URL goes through the
//! `eterea_logic` HTTPS policy in [`is_allowed_media_url`] before a socket is
//! opened: HTTPS only, no credentials, no fragments, and no private, loopback,
//! link-local or multicast hosts. Redirects are followed manually only when the
//! next hop is again a safe HTTPS URL, so a redirect can never downgrade the
//! scheme or reach a private address.
//!
//! Images are fetched by a fixed pool of blocking worker threads. Callers
//! enqueue an image by [`RequestId`] and drain [`MediaEvent`] values with
//! [`MediaManager::try_recv`]; a UI thread never blocks on the network. Queues
//! are bounded, so a burst of requests applies back pressure instead of
//! spawning threads or growing memory without limit. A request may be
//! cancelled by id; once [`MediaManager::cancel`] returns `true`, no event for
//! that request is emitted.
//!
//! HTTP goes through `ureq` with rustls, no cookie jar (the crate is compiled
//! without the `cookies` feature), no `Referer` header, a stable user agent, a
//! 20 second timeout, a 32 MiB image response cap, and at most four validated
//! redirect hops.
//!
//! Images use an opt-in, bounded, SHA-256-addressed disk cache. Video downloads
//! use [`VideoCache`]: a caller-supplied, persistent, content-addressed cache
//! (2 GiB by default) with atomic temp+rename downloads, least-recently-used
//! eviction, partial-file cleanup and duplicate-download suppression. Video
//! files are reused across plays and are kept when playback stops. Media bytes
//! are never written to SQLite, and no UI or Rune dependency lives here.
//!
//! Playback is local-only: [`VideoPlayer`] resolves a URL into a local file
//! through [`VideoCache`] (or a caller-supplied fallback directory), then
//! decodes frames with the external `ffmpeg` binary and plays sound with
//! `ffplay`. ffmpeg and ffplay never receive a remote URL. [`MediaTools`]
//! reports typed, actionable availability for the UI; sound is muted by
//! default, looping is optional, and pause/mute/volume/stop/ended/error state
//! plus current position and duration are exposed for polling. A decoder that
//! exits without producing frames is reported as [`MediaError::DecodeFailed`]
//! and never restarted, so a looping player cannot respawn a broken decoder
//! forever. Image fetches enforce the `eterea_logic` image MIME allowlist and
//! video downloads enforce a progressive-MP4/binary type gate; a disallowed
//! `Content-Type` fails with [`MediaError::UnsupportedType`] before the body is
//! read, decoded or cached.
//!
//! # Known limits
//!
//! - DNS is resolved normally, so policy checks literal host names only. A
//!   hostname that resolves to a private address is not detected (no custom
//!   resolver is installed).
//! - A blocking `ureq` read cannot be interrupted mid-syscall; cancellation is
//!   observed between redirect hops and between body chunks.
//! - Redirects with a relative `Location` are refused rather than resolved,
//!   because this crate does not depend on a URL parser.
//! - Video downloads and playback are progressive files only. There is no
//!   HLS/DASH support and no ffmpeg integration for playlists.
//! - [`VideoPlayer::stop`] and `Drop` are non-blocking: they kill ffmpeg and
//!   ffplay synchronously and detach the worker thread, which may linger for up
//!   to the HTTP timeout while a blocking download observes cancellation.
//! - Sound requires the external `ffplay` binary; there is no native audio
//!   backend dependency. Unmuting without ffplay returns
//!   [`MediaError::AudioUnavailable`].

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod cache;
mod digest;
mod error;
mod http;
mod manager;
mod player;
mod request;
mod syndication;
mod tools;
mod url;
mod video;
mod video_cache;

#[cfg(test)]
mod test_support;

pub use cache::{CacheKey, CacheStats, DiskCache, DiskCacheConfig, DEFAULT_CACHE_BYTES};
pub use error::MediaError;
pub use manager::{MediaManager, MediaManagerConfig};
pub use player::{
    PlaybackState, VideoFrame, VideoPlayer, VideoPlayerOptions, PLAYBACK_FPS, PLAYBACK_HEIGHT,
    PLAYBACK_WIDTH,
};
pub use request::{ImageKind, ImageRequest, ImageSource, MediaEvent, RequestId};
pub use syndication::{
    embed_token, endpoint_url, parse_tweet_payload, tweet_id_from_url, SyndicatedAuthor,
    SyndicatedLink, SyndicatedMedia, SyndicatedMediaKind, SyndicatedQuote, SyndicatedTweet,
    SyndicatedVariant, SyndicationClient, SyndicationOutcome, MAX_SYNDICATION_BYTES,
};
pub use tools::{MediaTools, ToolStatus};
pub use url::is_allowed_media_url;
pub use video::{VideoDownload, VideoJob, DEFAULT_VIDEO_BYTES};
pub use video_cache::{
    CachedVideo, VideoCache, VideoCacheConfig, VideoCacheStats, DEFAULT_VIDEO_CACHE_BYTES,
};

/// Maximum bytes accepted for a single image response: 32 MiB.
pub const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

/// Per-exchange HTTP timeout used by the media worker pool and video downloads.
pub const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Maximum number of validated redirect hops followed per request.
pub const MAX_REDIRECTS: usize = 4;

/// Stable user agent sent with every media request.
pub const USER_AGENT: &str = concat!("eterea/", env!("CARGO_PKG_VERSION"));

/// Default number of image fetch worker threads.
pub const DEFAULT_WORKERS: usize = 2;

/// Upper bound applied to [`MediaManagerConfig::worker_count`].
pub const MAX_WORKERS: usize = 8;

/// Default capacity of the image request queue.
pub const DEFAULT_QUEUE_BOUND: usize = 64;

/// Default capacity of the media event queue.
pub const DEFAULT_EVENT_BOUND: usize = 256;
