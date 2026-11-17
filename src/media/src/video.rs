//! Video download and local playback.
//!
//! [`VideoJob`] validates its URL with the media policy, downloads a progressive
//! media file to a caller-supplied temporary path with a byte cap, and exposes
//! the resulting path. [`VideoPlayer`] downloads the same way, then decodes the
//! local file with ffmpeg into a bounded frame channel for the UI thread. The
//! network trust boundary stays inside [`VideoJob`]; ffmpeg never receives a
//! remote URL.
//!
//! Playlists (HLS/DASH) are not supported: only a URL that returns the media
//! file itself can be downloaded. Audio is not wired yet. The caller owns
//! cleanup of a [`VideoJob`] destination; [`VideoPlayer`] cleans its own temp
//! file.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::MediaError;
use crate::http::HttpClient;
use crate::url::is_allowed_media_url;
use crate::HTTP_TIMEOUT;

/// Default video byte cap: 512 MiB.
pub const DEFAULT_VIDEO_BYTES: u64 = 512 * 1024 * 1024;

/// A validated, not-yet-downloaded video.
///
/// Construction validates the URL, so the scaffold cannot be pointed at an
/// unsafe target by accident. Downloading re-checks anyway.
#[derive(Debug, Clone)]
pub struct VideoJob {
    url: String,
    destination: PathBuf,
    max_bytes: u64,
    timeout: Duration,
}

impl VideoJob {
    /// Validate `url` and prepare a download into `destination`.
    ///
    /// `destination` is a caller-supplied temporary file path. Its parent
    /// directory is created on download. Returns [`MediaError::UnsafeUrl`] when
    /// the URL fails [`crate::is_allowed_media_url`].
    pub fn new(
        url: impl Into<String>,
        destination: impl Into<PathBuf>,
    ) -> Result<Self, MediaError> {
        let url = url.into();
        if !is_allowed_media_url(&url) {
            return Err(MediaError::UnsafeUrl);
        }
        Ok(Self {
            url,
            destination: destination.into(),
            max_bytes: DEFAULT_VIDEO_BYTES,
            timeout: HTTP_TIMEOUT,
        })
    }

    /// Override the byte cap. A zero cap is treated as one byte so an
    /// accidental zero cannot mean "unlimited".
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes.max(1);
        self
    }

    /// Override the per-exchange timeout. A zero duration falls back to
    /// [`HTTP_TIMEOUT`].
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = if timeout.is_zero() {
            HTTP_TIMEOUT
        } else {
            timeout
        };
        self
    }

    /// The validated URL.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The caller-supplied destination path.
    pub fn destination(&self) -> &Path {
        &self.destination
    }

    /// The byte cap applied to the download.
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// The per-exchange timeout applied to the download.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Blocking download. Creates parent directories, streams the body to
    /// [`Self::destination`], and removes the partial file on failure.
    ///
    /// On success returns the path and byte length. The caller owns the file
    /// and must remove it after playback.
    pub fn download(&self) -> Result<VideoDownload, MediaError> {
        self.download_with_cancel(None)
    }

    /// Download while observing a cancellation flag.
    ///
    /// Cancellation is checked between HTTP chunks; the destination is left
    /// without a partial file when the download is cancelled.
    pub fn download_cancellable(
        &self,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<VideoDownload, MediaError> {
        self.download_with_cancel(Some(cancel))
    }

    fn download_with_cancel(
        &self,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<VideoDownload, MediaError> {
        if !is_allowed_media_url(&self.url) {
            return Err(MediaError::UnsafeUrl);
        }
        if let Some(parent) = self.destination.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|_| MediaError::Io)?;
            }
        }
        let client = HttpClient::new(self.timeout);
        let byte_len =
            client.download_to_cancellable(&self.url, &self.destination, self.max_bytes, cancel)?;
        if byte_len == 0 {
            let _ = std::fs::remove_file(&self.destination);
            return Err(MediaError::EmptyResponse);
        }
        Ok(VideoDownload {
            path: self.destination.clone(),
            byte_len,
        })
    }
}

/// A completed video download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoDownload {
    /// Path the caller supplied, now holding the media file.
    pub path: PathBuf,
    /// Number of bytes downloaded.
    pub byte_len: u64,
}

impl VideoDownload {
    /// Path of the downloaded file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Number of bytes downloaded.
    pub fn byte_len(&self) -> u64 {
        self.byte_len
    }
}

#[cfg(test)]
mod tests {
    use super::{VideoJob, DEFAULT_VIDEO_BYTES};
    use crate::test_support::ScratchDir;
    use crate::MediaError;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    #[test]
    fn safe_urls_build_a_job() {
        let job = VideoJob::new(
            "https://video.twimg.com/ext_tw_video/1/pu/vid/720x1280/abc.mp4",
            "not/a/real/path/video.mp4",
        )
        .expect("safe url");
        assert!(job.url().starts_with("https://"));
        assert_eq!(job.max_bytes(), DEFAULT_VIDEO_BYTES);
        assert_eq!(
            job.destination().to_string_lossy(),
            "not/a/real/path/video.mp4"
        );
    }

    #[test]
    fn unsafe_urls_are_rejected_at_construction() {
        for url in [
            "http://video.twimg.com/x.mp4",
            "https://127.0.0.1/x.mp4",
            "https://user:pass@video.twimg.com/x.mp4",
            "https://video.twimg.com/x.mp4#t=1",
        ] {
            assert_eq!(
                VideoJob::new(url, "video.mp4").err(),
                Some(MediaError::UnsafeUrl),
                "{url}"
            );
        }
    }

    #[test]
    fn byte_cap_is_never_unlimited() {
        let job = VideoJob::new("https://video.twimg.com/x.mp4", "x.mp4")
            .expect("safe url")
            .with_max_bytes(0);
        assert_eq!(job.max_bytes(), 1);
        let job = job.with_max_bytes(1024);
        assert_eq!(job.max_bytes(), 1024);
    }

    #[test]
    fn zero_timeout_falls_back_to_default() {
        let job = VideoJob::new("https://video.twimg.com/x.mp4", "x.mp4")
            .expect("safe url")
            .with_timeout(Duration::ZERO);
        assert_eq!(job.timeout(), crate::HTTP_TIMEOUT);
        let job = job.with_timeout(Duration::from_secs(5));
        assert_eq!(job.timeout(), Duration::from_secs(5));
    }

    #[test]
    fn cancellation_before_the_first_byte_returns_cancelled() {
        let scratch = ScratchDir::new("video-job-cancel");
        let destination = scratch.path().join("cancelled.mp4");
        let job = VideoJob::new("https://video.twimg.com/x.mp4", &destination).expect("safe url");
        let cancel = AtomicBool::new(true);
        assert_eq!(
            job.download_cancellable(&cancel),
            Err(MediaError::Cancelled)
        );
        assert!(!destination.exists(), "no partial file may be left behind");
    }

    #[test]
    fn video_download_type_is_constructible() {
        let download = super::VideoDownload {
            path: std::path::PathBuf::from("x.mp4"),
            byte_len: 10,
        };
        assert_eq!(download.path(), std::path::Path::new("x.mp4"));
        assert_eq!(download.byte_len(), 10);
    }
}
