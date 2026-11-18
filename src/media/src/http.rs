//! Blocking HTTPS client used by the image workers and the video scaffold.
//!
//! The client is deliberately small: a shared `ureq` agent with rustls, no
//! cookie jar (the dependency is compiled without the `cookies` feature), no
//! `Referer` header (ureq never adds one and this crate sets none), no
//! authorization or cookie headers, a stable user agent, one timeout, and
//! manual redirect handling. Every URL, including every redirect target,
//! passes [`crate::is_allowed_media_url`] before a socket is opened.
//!
//! Before any body byte is read, the response `Content-Type` is checked with
//! the same media policy the rest of the crate documents: image fetches accept
//! only the `eterea_logic::is_image_mime` allowlist, and video downloads accept
//! progressive MP4 (plus the generic binary types CDNs use in practice — see
//! [`video_mime_allowed`]). A disallowed response becomes
//! [`MediaError::UnsupportedType`] and is never decoded or cached.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use eterea_logic::is_image_mime;

use crate::error::MediaError;
use crate::url::is_allowed_media_url;
use crate::{MAX_REDIRECTS, USER_AGENT};

/// Content types accepted for a video download.
///
/// The player only supports progressive MP4, so the allowlist is deliberately
/// MP4-shaped. `application/octet-stream` (and `binary/octet-stream`) are
/// accepted because X's CDN and several generic file hosts label MP4 bodies
/// that way, and a missing `Content-Type` is tolerated for the same reason:
/// the bytes are still validated locally by ffmpeg, and a decode failure is
/// now reported as a typed [`MediaError::DecodeFailed`]. A wrong-but-generic
/// label can therefore never reach a decoder silently; an obviously wrong
/// label (`text/html`, JSON, HLS playlists) is refused before the body.
pub(crate) fn video_mime_allowed(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        return true;
    };
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    matches!(
        essence.as_str(),
        "video/mp4"
            | "application/mp4"
            | "video/x-m4v"
            | "application/octet-stream"
            | "binary/octet-stream"
    )
}

/// Return `true` when the image response's `Content-Type` may be decoded.
///
/// This is the runtime half of `INV-MED-09`: the image allowlist is enforced
/// before any body byte is read. Two generic labels are also accepted —
/// `application/octet-stream` and `binary/octet-stream` — because some avatar
/// mirrors and CDNs serve JPEG/PNG bodies that way; the decoder remains the
/// final validator, so a wrong body fails as a typed decode failure rather
/// than being displayed. Missing or non-ASCII header values and obvious
/// non-image types are still refused.
pub(crate) fn image_content_type_allowed(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        return false;
    };
    if is_image_mime(content_type) {
        return true;
    }
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    matches!(
        essence.as_str(),
        "application/octet-stream" | "binary/octet-stream"
    )
}

/// Blocking HTTP client shared by media workers.
pub(crate) struct HttpClient {
    agent: ureq::Agent,
    timeout: Duration,
}

impl HttpClient {
    /// Build the client. `timeout` bounds each HTTP exchange; the caller is
    /// responsible for providing a non-zero duration.
    pub(crate) fn new(timeout: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .https_only(true)
            // Redirects are followed manually so every hop can be validated.
            .max_redirects(0)
            .user_agent(USER_AGENT)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            timeout,
        }
    }

    /// Fetch a complete image body, capped at `max_bytes`.
    ///
    /// The response must carry an allowlisted image `Content-Type` (see
    /// [`image_content_type_allowed`]); anything else fails with
    /// [`MediaError::UnsupportedType`] before a byte is read, so non-image
    /// bodies never reach the decoder or the disk cache.
    ///
    /// Cancellation is observed between redirect hops and between body chunks;
    /// a blocking socket read is not interrupted mid-syscall.
    pub(crate) fn fetch_bytes(
        &self,
        url: &str,
        cancel: &AtomicBool,
        max_bytes: usize,
    ) -> Result<Vec<u8>, MediaError> {
        let response = self.open(url, Some(cancel), None)?;
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok());
        if !image_content_type_allowed(content_type) {
            return Err(MediaError::UnsupportedType);
        }
        if let Some(length) = response.body().content_length() {
            if length > max_bytes as u64 {
                return Err(MediaError::TooLarge);
            }
        }
        let mut reader = response
            .into_body()
            .into_with_config()
            .limit(max_bytes as u64)
            .reader();
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            check_cancelled(Some(cancel))?;
            match reader.read(&mut buffer) {
                Ok(0) => return Ok(bytes),
                Ok(read) => {
                    if bytes.len().saturating_add(read) > max_bytes {
                        return Err(MediaError::TooLarge);
                    }
                    bytes.extend_from_slice(&buffer[..read]);
                }
                Err(error) => return Err(map_body_read_error(error)),
            }
        }
    }

    /// Streaming video download with an optional cancellation flag.
    ///
    /// The response must be a progressive-video or generic binary type (see
    /// [`video_mime_allowed`]). Cancellation is observed between redirect hops
    /// and between body chunks. The partial file is removed on every failure
    /// path, including cancellation, a truncated body, or a disallowed
    /// `Content-Type`. The completed file is flushed to disk before success is
    /// reported, so a later rename cannot promote unflushed bytes.
    pub(crate) fn download_to_cancellable(
        &self,
        url: &str,
        destination: &Path,
        max_bytes: u64,
        cancel: Option<&AtomicBool>,
    ) -> Result<u64, MediaError> {
        self.download_to_cancellable_from(url, destination, max_bytes, cancel, 0)
    }

    /// Download a video body, resuming with an HTTP `Range` request after a
    /// dropped connection instead of restarting from zero.
    ///
    /// At most three attempts are made. A partial body is kept between attempts
    /// and the next request asks for the remaining bytes; a server that ignores
    /// `Range` (answers 200) restarts the file cleanly. The destination is only
    /// promoted once the declared total length has been written.
    pub(crate) fn download_to_cancellable_from(
        &self,
        url: &str,
        destination: &Path,
        max_bytes: u64,
        cancel: Option<&AtomicBool>,
        mut resume_from: u64,
    ) -> Result<u64, MediaError> {
        const ATTEMPTS: u32 = 3;
        let mut attempt = 0_u32;
        loop {
            attempt += 1;
            let response = match self.open(url, cancel, (resume_from > 0).then_some(resume_from)) {
                Ok(response) => response,
                Err(error) if is_retryable(&error) && attempt < ATTEMPTS => {
                    sleep_before_retry(attempt);
                    continue;
                }
                Err(error) => {
                    let _ = std::fs::remove_file(destination);
                    return Err(error);
                }
            };
            let status = response.status().as_u16();
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok());
            if !video_mime_allowed(content_type) {
                let _ = std::fs::remove_file(destination);
                return Err(MediaError::UnsupportedType);
            }
            // A 200 to a ranged request means the server ignored the range:
            // start over rather than appending duplicate bytes.
            if resume_from > 0 && status == 200 {
                resume_from = 0;
            }
            let content_range_total = response
                .headers()
                .get("content-range")
                .and_then(|value| value.to_str().ok())
                .and_then(parse_content_range_total);
            let part_length = response.body().content_length();
            let total = if status == 206 {
                content_range_total.or_else(|| part_length.map(|length| resume_from + length))
            } else {
                part_length
            };
            if total.is_some_and(|total| total > max_bytes) {
                let _ = std::fs::remove_file(destination);
                return Err(MediaError::TooLarge);
            }
            let mut reader = response
                .into_body()
                .into_with_config()
                .limit(max_bytes.saturating_sub(resume_from).saturating_add(1))
                .reader();
            let mut file = Some(if resume_from > 0 {
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(destination)
                    .map_err(|_| MediaError::Io)?
            } else {
                std::fs::File::create(destination).map_err(|_| MediaError::Io)?
            });
            let mut written = resume_from;
            let mut buffer = [0_u8; 32 * 1024];
            let retry = loop {
                if let Err(error) = check_cancelled(cancel) {
                    drop(reader);
                    let _ = std::fs::remove_file(destination);
                    return Err(error);
                }
                match reader.read(&mut buffer) {
                    Ok(0) => break None,
                    Ok(read) => {
                        written = written.saturating_add(read as u64);
                        if written > max_bytes {
                            let _ = std::fs::remove_file(destination);
                            return Err(MediaError::TooLarge);
                        }
                        if file
                            .as_mut()
                            .expect("download file is open")
                            .write_all(&buffer[..read])
                            .is_err()
                        {
                            let _ = std::fs::remove_file(destination);
                            return Err(MediaError::Io);
                        }
                    }
                    Err(error) => {
                        if let Some(open) = file.take() {
                            let _ = open.sync_all();
                        }
                        let mapped = map_body_read_error(error);
                        if is_retryable(&mapped) && attempt < ATTEMPTS {
                            break Some((written, mapped));
                        }
                        let _ = std::fs::remove_file(destination);
                        return Err(mapped);
                    }
                }
            };
            if let Some((partial, _mapped)) = retry {
                resume_from = partial;
                sleep_before_retry(attempt);
                continue;
            }
            // A body shorter than the declared total is a cut connection; never
            // promote a truncated file into the cache (there is no payload
            // digest to catch it later).
            if total.is_some_and(|total| written != total) {
                if let Some(open) = file.take() {
                    let _ = open.sync_all();
                }
                if attempt < ATTEMPTS {
                    resume_from = written;
                    sleep_before_retry(attempt);
                    continue;
                }
                let _ = std::fs::remove_file(destination);
                return Err(MediaError::Connection);
            }
            let Some(mut open) = file.take() else {
                let _ = std::fs::remove_file(destination);
                return Err(MediaError::Io);
            };
            if open.flush().is_err() || open.sync_all().is_err() {
                let _ = std::fs::remove_file(destination);
                return Err(MediaError::Io);
            }
            return Ok(written);
        }
    }

    /// Execute a GET, following at most    /// Execute a GET, following at most [`MAX_REDIRECTS`] hops.
    ///
    /// Every candidate URL (initial, and each `Location`) must pass the media
    /// policy. Redirects with a relative `Location` are refused because no URL
    /// parser is linked here.
    fn open(
        &self,
        url: &str,
        cancel: Option<&AtomicBool>,
        range_from: Option<u64>,
    ) -> Result<ureq::http::Response<ureq::Body>, MediaError> {
        if !is_allowed_media_url(url) {
            return Err(MediaError::UnsafeUrl);
        }
        let deadline = Instant::now().checked_add(self.timeout);
        let mut current = url.to_owned();
        let mut hops = 0_usize;
        loop {
            check_cancelled(cancel)?;
            if deadline
                .map(|deadline| Instant::now() >= deadline)
                .unwrap_or(false)
            {
                return Err(MediaError::Timeout);
            }
            if !is_allowed_media_url(&current) {
                return Err(MediaError::UnsafeUrl);
            }
            let mut request = self.agent.get(&current);
            if let Some(from) = range_from {
                request = request.header("range", format!("bytes={from}-"));
            }
            let response = request.call().map_err(map_transport_error)?;
            let status = response.status().as_u16();
            if (300..=399).contains(&status) {
                if hops >= MAX_REDIRECTS {
                    return Err(MediaError::RedirectRefused);
                }
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                let Some(location) = location else {
                    return Err(MediaError::RedirectRefused);
                };
                if !is_allowed_media_url(&location) {
                    return Err(MediaError::RedirectRefused);
                }
                current = location;
                hops += 1;
                continue;
            }
            if !(200..=299).contains(&status) {
                return Err(MediaError::HttpStatus(status));
            }
            return Ok(response);
        }
    }
}

fn is_retryable(error: &MediaError) -> bool {
    matches!(error, MediaError::Connection | MediaError::Timeout)
}

fn sleep_before_retry(attempt: u32) {
    std::thread::sleep(Duration::from_millis(250 * u64::from(attempt)));
}

/// Total resource length from a `Content-Range: bytes a-b/total` header.
pub(crate) fn parse_content_range_total(value: &str) -> Option<u64> {
    value
        .rsplit_once('/')
        .and_then(|(_, total)| total.trim().parse().ok())
}

fn check_cancelled(cancel: Option<&AtomicBool>) -> Result<(), MediaError> {
    match cancel {
        Some(flag) if flag.load(Ordering::Relaxed) => Err(MediaError::Cancelled),
        _ => Ok(()),
    }
}

pub(crate) fn map_transport_error(error: ureq::Error) -> MediaError {
    match error {
        ureq::Error::StatusCode(status) => MediaError::HttpStatus(status),
        ureq::Error::Timeout(_) => MediaError::Timeout,
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => MediaError::Connection,
        ureq::Error::RequireHttpsOnly(_) => MediaError::UnsafeUrl,
        ureq::Error::BodyExceedsLimit(_) => MediaError::TooLarge,
        _ => MediaError::Connection,
    }
}

fn map_body_read_error(error: std::io::Error) -> MediaError {
    match ureq::Error::from(error) {
        ureq::Error::BodyExceedsLimit(_) => MediaError::TooLarge,
        ureq::Error::Timeout(_) => MediaError::Timeout,
        _ => MediaError::Connection,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        image_content_type_allowed, map_body_read_error, map_transport_error, video_mime_allowed,
        HttpClient,
    };
    use crate::MediaError;
    use std::time::Duration;

    #[test]
    fn client_configures_without_network() {
        let _client = HttpClient::new(Duration::from_secs(20));
    }

    #[test]
    fn image_content_type_allowlist_is_enforced() {
        for allowed in [
            "image/jpeg",
            "image/png",
            "image/gif",
            "image/webp",
            "image/avif",
            "IMAGE/PNG",
            "image/jpeg; charset=binary",
            " image/webp ; q=1",
            "application/octet-stream",
            "binary/octet-stream",
        ] {
            assert!(
                image_content_type_allowed(Some(allowed)),
                "must accept {allowed}"
            );
        }
        for refused in [
            "text/html",
            "text/html; charset=utf-8",
            "application/json",
            "image/svg+xml",
            "video/mp4",
            "text/plain",
            "",
        ] {
            assert!(
                !image_content_type_allowed(Some(refused)),
                "must refuse {refused}"
            );
        }
        // A missing (or non-ASCII) Content-Type cannot be classified, so the
        // image is refused rather than decoded.
        assert!(!image_content_type_allowed(None));
    }

    #[test]
    fn video_content_type_allows_progressive_mp4_and_generic_binary() {
        for allowed in [
            "video/mp4",
            "VIDEO/MP4",
            "video/mp4; codecs=avc1",
            "application/mp4",
            "video/x-m4v",
            "application/octet-stream",
            "binary/octet-stream",
        ] {
            assert!(video_mime_allowed(Some(allowed)), "must accept {allowed}");
        }
        // Missing Content-Type is tolerated for video: generic hosts omit it and
        // ffmpeg validates the bytes (a decode failure is typed now).
        assert!(video_mime_allowed(None));
        // Obviously wrong envelope types are refused before the body is read.
        for refused in [
            "text/html",
            "application/json",
            "text/plain",
            "application/vnd.apple.mpegurl",
            "video/x-matroska",
        ] {
            assert!(!video_mime_allowed(Some(refused)), "must refuse {refused}");
        }
    }

    #[test]
    fn unsupported_content_type_has_a_typed_actionable_error() {
        assert_eq!(
            MediaError::UnsupportedType.to_string(),
            "unsupported media content type"
        );
        assert_eq!(MediaError::DecodeFailed.to_string(), "video decoder failed");
    }

    #[test]
    fn transport_errors_map_to_public_reasons() {
        assert_eq!(
            map_transport_error(ureq::Error::StatusCode(404)),
            MediaError::HttpStatus(404)
        );
        assert_eq!(
            map_transport_error(ureq::Error::HostNotFound),
            MediaError::Connection
        );
        assert_eq!(
            map_transport_error(ureq::Error::ConnectionFailed),
            MediaError::Connection
        );
    }

    #[test]
    fn oversized_bodies_map_to_too_large() {
        let oversized = ureq::Error::BodyExceedsLimit(10).into_io();
        assert_eq!(map_body_read_error(oversized), MediaError::TooLarge);
    }

    #[test]
    fn network_io_errors_map_to_connection() {
        let io = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
        assert_eq!(map_body_read_error(io), MediaError::Connection);
    }
}

#[cfg(test)]
mod content_range_tests {
    use super::parse_content_range_total;

    #[test]
    fn content_range_total_is_parsed() {
        assert_eq!(
            parse_content_range_total("bytes 1024-2047/4096"),
            Some(4096)
        );
        assert_eq!(parse_content_range_total("bytes 0-99/*"), None);
        assert_eq!(parse_content_range_total("garbage"), None);
    }
}
