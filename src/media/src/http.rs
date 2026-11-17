//! Blocking HTTPS client used by the image workers and the video scaffold.
//!
//! The client is deliberately small: a shared `ureq` agent with rustls, no
//! cookie jar (the dependency is compiled without the `cookies` feature), no
//! `Referer` header (ureq never adds one and this crate sets none), no
//! authorization or cookie headers, a stable user agent, one timeout, and
//! manual redirect handling. Every URL, including every redirect target,
//! passes [`crate::is_allowed_media_url`] before a socket is opened.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::error::MediaError;
use crate::url::is_allowed_media_url;
use crate::{MAX_REDIRECTS, USER_AGENT};

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
    /// Cancellation is observed between redirect hops and between body chunks;
    /// a blocking socket read is not interrupted mid-syscall.
    pub(crate) fn fetch_bytes(
        &self,
        url: &str,
        cancel: &AtomicBool,
        max_bytes: usize,
    ) -> Result<Vec<u8>, MediaError> {
        let response = self.open(url, Some(cancel))?;
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

    /// Streaming download with an optional cancellation flag.
    ///
    /// Cancellation is observed between redirect hops and between body chunks.
    /// The partial file is removed on every failure path, including
    /// cancellation.
    pub(crate) fn download_to_cancellable(
        &self,
        url: &str,
        destination: &Path,
        max_bytes: u64,
        cancel: Option<&AtomicBool>,
    ) -> Result<u64, MediaError> {
        let response = self.open(url, cancel)?;
        if let Some(length) = response.body().content_length() {
            if length > max_bytes {
                return Err(MediaError::TooLarge);
            }
        }
        let mut reader = response
            .into_body()
            .into_with_config()
            .limit(max_bytes)
            .reader();
        let mut file = std::fs::File::create(destination).map_err(|_| MediaError::Io)?;
        let mut buffer = [0_u8; 32 * 1024];
        let mut written = 0_u64;
        loop {
            if let Err(error) = check_cancelled(cancel) {
                drop(reader);
                drop(file);
                let _ = std::fs::remove_file(destination);
                return Err(error);
            }
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    written = written.saturating_add(read as u64);
                    if written > max_bytes {
                        drop(file);
                        let _ = std::fs::remove_file(destination);
                        return Err(MediaError::TooLarge);
                    }
                    if file.write_all(&buffer[..read]).is_err() {
                        drop(file);
                        let _ = std::fs::remove_file(destination);
                        return Err(MediaError::Io);
                    }
                }
                Err(error) => {
                    drop(file);
                    let _ = std::fs::remove_file(destination);
                    return Err(map_body_read_error(error));
                }
            }
        }
        if file.flush().is_err() {
            drop(file);
            let _ = std::fs::remove_file(destination);
            return Err(MediaError::Io);
        }
        Ok(written)
    }

    /// Execute a GET, following at most [`MAX_REDIRECTS`] hops.
    ///
    /// Every candidate URL (initial, and each `Location`) must pass the media
    /// policy. Redirects with a relative `Location` are refused because no URL
    /// parser is linked here.
    fn open(
        &self,
        url: &str,
        cancel: Option<&AtomicBool>,
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
            let response = self
                .agent
                .get(&current)
                .call()
                .map_err(map_transport_error)?;
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

fn check_cancelled(cancel: Option<&AtomicBool>) -> Result<(), MediaError> {
    match cancel {
        Some(flag) if flag.load(Ordering::Relaxed) => Err(MediaError::Cancelled),
        _ => Ok(()),
    }
}

fn map_transport_error(error: ureq::Error) -> MediaError {
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
    use super::{map_body_read_error, map_transport_error, HttpClient};
    use crate::MediaError;
    use std::time::Duration;

    #[test]
    fn client_configures_without_network() {
        let _client = HttpClient::new(Duration::from_secs(20));
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
