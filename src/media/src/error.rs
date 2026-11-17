//! Error types shared by the media manager, cache and video scaffold.

use std::fmt;

/// Why a media request failed.
///
/// The variants are deliberately small and free of URL text so they can be
/// logged or shown without leaking imported, attacker-controlled URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaError {
    /// The URL failed the HTTPS media policy.
    UnsafeUrl,
    /// Another request with this [`crate::RequestId`] is still outstanding.
    DuplicateRequest,
    /// The request queue is full; retry after draining events.
    QueueFull,
    /// The manager has been shut down; no worker is available.
    Closed,
    /// The request was cancelled locally.
    Cancelled,
    /// No response arrived within the HTTP timeout.
    Timeout,
    /// The connection failed, the TLS handshake failed, or the body was cut.
    Connection,
    /// The server answered with a non-success status.
    HttpStatus(u16),
    /// The response exceeded the configured byte cap.
    TooLarge,
    /// The server sent an empty body, which is never a usable image or video.
    EmptyResponse,
    /// A redirect was refused: relative, unsafe, or too many hops.
    RedirectRefused,
    /// `ffmpeg` is not installed or not usable; video frames cannot decode.
    DecoderUnavailable,
    /// `ffplay` is not installed or could not start; sound cannot play.
    AudioUnavailable,
    /// The request or destination path was invalid.
    InvalidConfig,
    /// A local file operation failed (destination path, cache I/O).
    Io,
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsafeUrl => f.write_str("unsafe media URL"),
            Self::DuplicateRequest => f.write_str("request already pending"),
            Self::QueueFull => f.write_str("request queue is full"),
            Self::Closed => f.write_str("media workers are closed"),
            Self::Cancelled => f.write_str("request cancelled"),
            Self::Timeout => f.write_str("request timed out"),
            Self::Connection => f.write_str("connection failed"),
            Self::HttpStatus(status) => write!(f, "server returned HTTP {status}"),
            Self::TooLarge => f.write_str("response too large"),
            Self::EmptyResponse => f.write_str("empty response"),
            Self::RedirectRefused => f.write_str("redirect refused"),
            Self::DecoderUnavailable => {
                f.write_str("ffmpeg not found: install ffmpeg or add it to PATH")
            }
            Self::AudioUnavailable => f.write_str(
                "ffplay not found: install ffmpeg (it includes ffplay) or add it to PATH",
            ),
            Self::InvalidConfig => f.write_str("invalid media configuration"),
            Self::Io => f.write_str("local file error"),
        }
    }
}

impl std::error::Error for MediaError {}
