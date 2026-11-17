//! Persistent, bounded, content-addressed cache for downloaded video files.
//!
//! Video bytes are expensive to fetch and are reused across plays, so the cache
//! is keyed by the SHA-256 digest of the URL. File names never contain the URL
//! or any other imported text: an entry is `<sha256-hex>.media` inside
//! `<caller dir>/video/`. The cache owns that subdirectory, so
//! [`VideoCache::clear`] can never delete unrelated files.
//!
//! Downloads stream to a hidden `.<key>.part.<pid>.<n>` file in the same
//! directory and are promoted with an atomic rename, so a crashed download can
//! never be mistaken for a complete video. Partial files are removed on every
//! failure path and swept on [`VideoCache::open`].
//!
//! The cache is bounded by [`VideoCacheConfig::max_bytes`] (2 GiB by default).
//! Reading an entry touches its modification time, and when an insert would
//! exceed the budget the least recently used entries are removed first.
//!
//! Concurrent callers that ask for the same URL share one download: the leader
//! fetches, the followers wait for its result. A cancelled waiter returns
//! [`MediaError::Cancelled`] immediately; the leader's download continues for
//! the other waiters unless it is cancelled too.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime};

use crate::digest::{hex_encode, sha256};
use crate::error::MediaError;
use crate::http::HttpClient;
use crate::url::is_allowed_media_url;
use crate::HTTP_TIMEOUT;

/// Default persistent video cache budget: 2 GiB.
pub const DEFAULT_VIDEO_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

const ENTRY_EXTENSION: &str = "media";
const PARTIAL_MARKER: &str = ".part.";

/// Configuration of the persistent video cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoCacheConfig {
    /// Caller-supplied base directory. The cache owns the `video/`
    /// subdirectory of this path and never touches anything outside it.
    pub dir: PathBuf,
    /// Total byte budget for cached video files. Must be greater than zero.
    pub max_bytes: u64,
}

impl VideoCacheConfig {
    /// Enable the cache under `dir` with [`DEFAULT_VIDEO_CACHE_BYTES`] of
    /// budget.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            max_bytes: DEFAULT_VIDEO_CACHE_BYTES,
        }
    }

    /// Override the total byte budget.
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }
}

/// Current video cache occupancy and budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoCacheStats {
    /// Total bytes occupied by cached videos.
    pub bytes: u64,
    /// Number of cached videos.
    pub entries: usize,
    /// Configured total byte budget.
    pub max_bytes: u64,
}

/// A resolved cached video file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedVideo {
    /// Absolute path of the local file.
    pub path: PathBuf,
    /// File size in bytes.
    pub byte_len: u64,
    /// Whether the bytes were already cached (`false` means freshly downloaded).
    pub from_cache: bool,
}

/// Injectable transport used by [`VideoCache`]; unit tests substitute a
/// scripted fake so no cache test touches the network.
pub(crate) trait VideoFetcher: Send + Sync {
    fn fetch_to(
        &self,
        url: &str,
        destination: &Path,
        max_bytes: u64,
        cancel: &AtomicBool,
    ) -> Result<u64, MediaError>;
}

struct HttpVideoFetcher {
    client: HttpClient,
}

impl VideoFetcher for HttpVideoFetcher {
    fn fetch_to(
        &self,
        url: &str,
        destination: &Path,
        max_bytes: u64,
        cancel: &AtomicBool,
    ) -> Result<u64, MediaError> {
        self.client
            .download_to_cancellable(url, destination, max_bytes, Some(cancel))
    }
}

#[derive(Debug, Default)]
struct CacheState {
    total_bytes: u64,
    entries: usize,
}

#[derive(Default)]
struct DownloadSlot {
    result: Mutex<Option<Result<CachedVideo, MediaError>>>,
    ready: Condvar,
}

/// Persistent, bounded video file cache. Cheap to share via [`Arc`].
pub struct VideoCache {
    root: PathBuf,
    max_bytes: u64,
    client: Arc<dyn VideoFetcher>,
    state: Mutex<CacheState>,
    inflight: Mutex<HashMap<[u8; 32], Arc<DownloadSlot>>>,
}

impl std::fmt::Debug for VideoCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoCache")
            .field("root", &self.root)
            .field("max_bytes", &self.max_bytes)
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

impl VideoCache {
    /// Open or create the cache under `<config.dir>/video`.
    ///
    /// Scans existing entries to account for them, removes stale partial
    /// downloads, and evicts the least recently used entries if the directory
    /// already exceeds the configured budget.
    pub fn open(config: VideoCacheConfig) -> std::io::Result<Self> {
        let client: Arc<dyn VideoFetcher> = Arc::new(HttpVideoFetcher {
            client: HttpClient::new(HTTP_TIMEOUT),
        });
        Self::open_with_fetcher(config, client)
    }

    pub(crate) fn open_with_fetcher(
        config: VideoCacheConfig,
        client: Arc<dyn VideoFetcher>,
    ) -> std::io::Result<Self> {
        if config.max_bytes == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "video cache max_bytes must be greater than zero",
            ));
        }
        let root = config.dir.join("video");
        std::fs::create_dir_all(&root)?;
        remove_partial_files(&root)?;
        let (total_bytes, entries) = scan_stats(&root)?;
        let cache = Self {
            root,
            max_bytes: config.max_bytes,
            client,
            state: Mutex::new(CacheState {
                total_bytes,
                entries,
            }),
            inflight: Mutex::new(HashMap::new()),
        };
        cache.evict_until_fits();
        Ok(cache)
    }

    /// The directory the cache owns (`<caller dir>/video`).
    pub fn dir(&self) -> &Path {
        &self.root
    }

    /// Configured total byte budget.
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// Snapshot of occupancy and budget.
    pub fn stats(&self) -> VideoCacheStats {
        let state = self.state.lock().expect("video cache state lock poisoned");
        VideoCacheStats {
            bytes: state.total_bytes,
            entries: state.entries,
            max_bytes: self.max_bytes,
        }
    }

    /// Remove every cached video, keeping the owned directory.
    pub fn clear(&self) -> std::io::Result<()> {
        let mut state = self.state.lock().expect("video cache state lock poisoned");
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                std::fs::remove_file(&path)?;
            }
        }
        state.total_bytes = 0;
        state.entries = 0;
        Ok(())
    }

    /// Cache key for a URL: lowercase SHA-256 hex, also the file stem.
    pub fn key_for(url: &str) -> String {
        hex_encode(&sha256(url.as_bytes()))
    }

    /// A cache hit for `url`, if one exists. Touches the entry's modification
    /// time for LRU accounting and never downloads or validates the URL.
    pub fn cached(&self, url: &str) -> Option<CachedVideo> {
        if !is_allowed_media_url(url) {
            return None;
        }
        let key = sha256(url.as_bytes());
        self.lookup(&key)
    }

    /// Return the local file for `url`, downloading it at most once.
    ///
    /// `max_bytes` caps a single download (the caller's per-video byte limit);
    /// it must fit inside the cache budget or the download is refused with
    /// [`MediaError::TooLarge`]. The URL is validated with
    /// [`crate::is_allowed_media_url`] before any network access.
    pub fn ensure(&self, url: &str, max_bytes: u64) -> Result<CachedVideo, MediaError> {
        static NEVER_CANCELLED: AtomicBool = AtomicBool::new(false);
        self.ensure_cancellable(url, max_bytes, &NEVER_CANCELLED)
    }

    /// [`Self::ensure`] with a cancellation flag.
    ///
    /// A leader download observes the flag between HTTP chunks. A waiter that
    /// is cancelled returns without removing the shared download.
    pub fn ensure_cancellable(
        &self,
        url: &str,
        max_bytes: u64,
        cancel: &AtomicBool,
    ) -> Result<CachedVideo, MediaError> {
        if !is_allowed_media_url(url) {
            return Err(MediaError::UnsafeUrl);
        }
        if max_bytes == 0 {
            return Err(MediaError::InvalidConfig);
        }
        if max_bytes > self.max_bytes {
            return Err(MediaError::TooLarge);
        }
        let key = sha256(url.as_bytes());
        if let Some(hit) = self.lookup(&key) {
            return Ok(hit);
        }

        let (slot, leader) = {
            let mut inflight = self
                .inflight
                .lock()
                .expect("video cache in-flight lock poisoned");
            match inflight.get(&key) {
                Some(slot) => (Arc::clone(slot), false),
                None => {
                    let slot = Arc::new(DownloadSlot::default());
                    inflight.insert(key, Arc::clone(&slot));
                    (slot, true)
                }
            }
        };

        if !leader {
            return wait_for_download(&slot, cancel);
        }

        let guard = LeaderGuard {
            cache: self,
            key,
            slot: Arc::clone(&slot),
            released: false,
        };
        let result = self.download(url, max_bytes, cancel, key);
        {
            let mut done = slot
                .result
                .lock()
                .expect("video cache download slot lock poisoned");
            *done = Some(result.clone());
        }
        slot.ready.notify_all();
        drop(guard);
        result
    }

    fn lookup(&self, key: &[u8; 32]) -> Option<CachedVideo> {
        let path = self.entry_path(key);
        let metadata = std::fs::metadata(&path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        if metadata.len() == 0 {
            self.remove_file_accounted(&path);
            return None;
        }
        // Touch the entry so the LRU order reflects this use; failure to touch
        // only weakens the heuristic, so it is ignored.
        if let Ok(file) = std::fs::File::open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
        Some(CachedVideo {
            path,
            byte_len: metadata.len(),
            from_cache: true,
        })
    }

    fn download(
        &self,
        url: &str,
        max_bytes: u64,
        cancel: &AtomicBool,
        key: [u8; 32],
    ) -> Result<CachedVideo, MediaError> {
        // Another leader may have finished between the fast-path lookup and
        // winning the in-flight slot.
        if let Some(hit) = self.lookup(&key) {
            return Ok(hit);
        }
        let destination = self.entry_path(&key);
        let temp = self.temp_path(&key);
        let result = self.client.fetch_to(url, &temp, max_bytes, cancel);
        match result {
            Ok(byte_len) if byte_len > 0 => {
                if byte_len > self.max_bytes {
                    let _ = std::fs::remove_file(&temp);
                    return Err(MediaError::TooLarge);
                }
                if std::fs::rename(&temp, &destination).is_err() {
                    let _ = std::fs::remove_file(&temp);
                    return Err(MediaError::Io);
                }
                self.refresh_stats();
                self.evict_until_fits();
                Ok(CachedVideo {
                    path: destination,
                    byte_len,
                    from_cache: false,
                })
            }
            Ok(_) => {
                let _ = std::fs::remove_file(&temp);
                Err(MediaError::EmptyResponse)
            }
            Err(error) => {
                let _ = std::fs::remove_file(&temp);
                Err(error)
            }
        }
    }

    fn entry_path(&self, key: &[u8; 32]) -> PathBuf {
        self.root
            .join(format!("{}.{ENTRY_EXTENSION}", hex_encode(key)))
    }

    fn temp_path(&self, key: &[u8; 32]) -> PathBuf {
        static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        self.root.join(format!(
            ".{}{PARTIAL_MARKER}{}.{sequence}",
            hex_encode(key),
            std::process::id()
        ))
    }

    fn refresh_stats(&self) {
        let Ok((bytes, entries)) = scan_stats(&self.root) else {
            return;
        };
        let mut state = self.state.lock().expect("video cache state lock poisoned");
        state.total_bytes = bytes;
        state.entries = entries;
    }

    fn remove_file_accounted(&self, path: &Path) {
        if std::fs::remove_file(path).is_ok() {
            self.refresh_stats();
        }
    }

    /// Evict least recently used entries until the total fits the budget.
    fn evict_until_fits(&self) {
        loop {
            let total = self
                .state
                .lock()
                .expect("video cache state lock poisoned")
                .total_bytes;
            if total <= self.max_bytes {
                return;
            }
            let Some(oldest) = oldest_entry(&self.root) else {
                return;
            };
            if std::fs::remove_file(&oldest).is_err() {
                return;
            }
            self.refresh_stats();
        }
    }
}

/// Guard that releases the in-flight slot even if the leader panics.
struct LeaderGuard<'a> {
    cache: &'a VideoCache,
    key: [u8; 32],
    slot: Arc<DownloadSlot>,
    released: bool,
}

impl LeaderGuard<'_> {
    fn release(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let mut done = self
            .slot
            .result
            .lock()
            .expect("video cache download slot lock poisoned");
        if done.is_none() {
            *done = Some(Err(MediaError::Io));
        }
        drop(done);
        self.slot.ready.notify_all();
        self.cache
            .inflight
            .lock()
            .expect("video cache in-flight lock poisoned")
            .remove(&self.key);
    }
}

impl Drop for LeaderGuard<'_> {
    fn drop(&mut self) {
        self.release();
    }
}

fn wait_for_download(slot: &DownloadSlot, cancel: &AtomicBool) -> Result<CachedVideo, MediaError> {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(MediaError::Cancelled);
        }
        let done = slot
            .result
            .lock()
            .expect("video cache download slot lock poisoned");
        if let Some(result) = done.as_ref() {
            return match result.clone() {
                Ok(mut video) => {
                    // This caller did not perform the network fetch itself, so
                    // the bytes are effectively cache-served for it.
                    video.from_cache = true;
                    Ok(video)
                }
                Err(error) => Err(error),
            };
        }
        let (_done, _timeout) = slot
            .ready
            .wait_timeout(done, Duration::from_millis(50))
            .expect("video cache download slot lock poisoned");
    }
}

fn scan_stats(root: &Path) -> std::io::Result<(u64, usize)> {
    let mut bytes = 0_u64;
    let mut entries = 0_usize;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if !metadata.is_file() || !is_entry_file(&path) {
            continue;
        }
        if metadata.len() == 0 {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        bytes = bytes.saturating_add(metadata.len());
        entries += 1;
    }
    Ok((bytes, entries))
}

fn is_entry_file(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some(ENTRY_EXTENSION)
}

fn is_partial_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.contains(PARTIAL_MARKER))
}

fn remove_partial_files(root: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && is_partial_file(&path) {
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(())
}

fn oldest_entry(root: &Path) -> Option<PathBuf> {
    let mut oldest: Option<(SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(root).ok()? {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if !metadata.is_file() || !is_entry_file(&path) {
            continue;
        }
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if oldest
            .as_ref()
            .map(|(oldest_modified, _)| modified < *oldest_modified)
            .unwrap_or(true)
        {
            oldest = Some((modified, path));
        }
    }
    oldest.map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::{VideoCache, VideoCacheConfig, VideoFetcher};
    use crate::error::MediaError;
    use crate::test_support::ScratchDir;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::{channel, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    type FetchBehavior = Box<dyn Fn(&Path) -> Result<u64, MediaError> + Send + Sync>;

    struct FakeFetcher {
        calls: AtomicUsize,
        started: Option<Sender<()>>,
        release: Option<Mutex<std::sync::mpsc::Receiver<()>>>,
        behavior: FetchBehavior,
    }

    impl FakeFetcher {
        fn writing(bytes: Vec<u8>) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                started: None,
                release: None,
                behavior: Box::new(move |destination| {
                    std::fs::write(destination, &bytes).map_err(|_| MediaError::Io)?;
                    Ok(bytes.len() as u64)
                }),
            })
        }

        fn blocking() -> (Arc<Self>, Sender<()>, std::sync::mpsc::Receiver<()>) {
            let (started_tx, started_rx) = channel();
            let (release_tx, release_rx) = channel();
            let fetcher = Arc::new(Self {
                calls: AtomicUsize::new(0),
                started: Some(started_tx),
                release: Some(Mutex::new(release_rx)),
                behavior: Box::new(|destination| {
                    std::fs::write(destination, b"blocked-video").map_err(|_| MediaError::Io)?;
                    Ok(11)
                }),
            });
            (fetcher, release_tx, started_rx)
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl VideoFetcher for FakeFetcher {
        fn fetch_to(
            &self,
            _url: &str,
            destination: &Path,
            max_bytes: u64,
            cancel: &AtomicBool,
        ) -> Result<u64, MediaError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(started) = &self.started {
                let _ = started.send(());
            }
            if let Some(release) = &self.release {
                release
                    .lock()
                    .expect("fake release lock")
                    .recv_timeout(Duration::from_secs(5))
                    .expect("test must release the fake download");
            }
            let result = (self.behavior)(destination);
            if cancel.load(Ordering::Relaxed) {
                return Err(MediaError::Cancelled);
            }
            match result {
                Ok(bytes) if bytes > max_bytes => Err(MediaError::TooLarge),
                other => other,
            }
        }
    }

    fn cache(scratch: &ScratchDir, max_bytes: u64) -> VideoCache {
        VideoCache::open(VideoCacheConfig::new(scratch.path()).with_max_bytes(max_bytes))
            .expect("open video cache")
    }

    fn url(suffix: &str) -> String {
        format!("https://video.twimg.com/ext_tw_video/{suffix}.mp4")
    }

    #[test]
    fn miss_downloads_then_hit_reuses_the_file() {
        let scratch = ScratchDir::new("video-cache-hit");
        let fetcher = FakeFetcher::writing(b"video-bytes".to_vec());
        let cache = VideoCache::open_with_fetcher(
            VideoCacheConfig::new(scratch.path()).with_max_bytes(1024 * 1024),
            fetcher.clone(),
        )
        .expect("cache");
        let target = url("hit");
        assert_eq!(cache.cached(&target), None);

        let first = cache.ensure(&target, 1024).expect("download");
        assert!(!first.from_cache);
        assert_eq!(first.byte_len, 11);
        assert!(first.path.is_file());
        assert_eq!(fetcher.calls(), 1);

        let second = cache.ensure(&target, 1024).expect("cache hit");
        assert!(second.from_cache);
        assert_eq!(second.path, first.path);
        assert_eq!(fetcher.calls(), 1);
        assert_eq!(cache.stats().entries, 1);
        assert!(cache.stats().bytes > 0);
    }

    #[test]
    fn downloaded_entry_name_is_a_sha256_digest() {
        let scratch = ScratchDir::new("video-cache-digest-name");
        let fetcher = FakeFetcher::writing(b"bytes".to_vec());
        let cache = VideoCache::open_with_fetcher(
            VideoCacheConfig::new(scratch.path()).with_max_bytes(1024 * 1024),
            fetcher,
        )
        .expect("cache");
        let target = url("digest");
        let hit = cache.ensure(&target, 1024).expect("download");
        let name = hit.path.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(name, format!("{}.media", VideoCache::key_for(&target)));
        assert!(!name.contains("https"));
        assert!(!name.contains("video.twimg.com"));
        assert_eq!(name.len(), 64 + ".media".len());
    }

    #[test]
    fn budget_evicts_least_recently_used_entries() {
        let scratch = ScratchDir::new("video-cache-lru");
        let fetcher = FakeFetcher::writing(vec![7_u8; 100]);
        let cache = VideoCache::open_with_fetcher(
            VideoCacheConfig::new(scratch.path()).with_max_bytes(250),
            fetcher,
        )
        .expect("cache");
        let first = url("first");
        let second = url("second");
        let third = url("third");
        cache.ensure(&first, 100).expect("first");
        std::thread::sleep(Duration::from_millis(10));
        cache.ensure(&second, 100).expect("second");
        // Touch the first entry so the second is the least recently used.
        std::thread::sleep(Duration::from_millis(10));
        assert!(cache.cached(&first).is_some());

        cache.ensure(&third, 100).expect("third");
        assert!(cache.stats().bytes <= cache.max_bytes());
        assert!(cache.cached(&first).is_some(), "recently used survives");
        assert!(cache.cached(&second).is_none(), "oldest entry evicted");
        assert!(cache.cached(&third).is_some());
    }

    #[test]
    fn clear_removes_entries_and_keeps_the_directory() {
        let scratch = ScratchDir::new("video-cache-clear");
        let fetcher = FakeFetcher::writing(b"clear-me".to_vec());
        let cache = VideoCache::open_with_fetcher(
            VideoCacheConfig::new(scratch.path()).with_max_bytes(1024 * 1024),
            fetcher,
        )
        .expect("cache");
        cache.ensure(&url("clear"), 1024).expect("download");
        assert_eq!(cache.stats().entries, 1);
        cache.clear().expect("clear");
        assert_eq!(cache.stats().entries, 0);
        assert_eq!(cache.stats().bytes, 0);
        assert!(cache.dir().is_dir());
        assert_eq!(std::fs::read_dir(cache.dir()).unwrap().count(), 0);
    }

    #[test]
    fn partial_files_are_swept_on_open() {
        let scratch = ScratchDir::new("video-cache-partials");
        let dir = scratch.path().join("video");
        std::fs::create_dir_all(&dir).expect("cache dir");
        let partial = dir.join(".abc.part.123.0");
        let stray = dir.join("notes.txt");
        std::fs::write(&partial, b"half a video").expect("partial");
        std::fs::write(&stray, b"stray").expect("stray");
        let _cache = cache(&scratch, 1024 * 1024);
        assert!(!partial.exists(), "partial download must be removed");
        assert!(stray.exists(), "unknown files are not touched");
    }

    #[test]
    fn failed_download_leaves_no_partial_file() {
        let scratch = ScratchDir::new("video-cache-failed-download");
        let fetcher = Arc::new(FakeFetcher {
            calls: AtomicUsize::new(0),
            started: None,
            release: None,
            behavior: Box::new(|destination: &Path| {
                std::fs::write(destination, vec![0_u8; 2048]).map_err(|_| MediaError::Io)?;
                Err(MediaError::TooLarge)
            }),
        });
        let cache = VideoCache::open_with_fetcher(
            VideoCacheConfig::new(scratch.path()).with_max_bytes(1024 * 1024),
            fetcher,
        )
        .expect("cache");
        let error = cache.ensure(&url("failed"), 1024).expect_err("must fail");
        assert_eq!(error, MediaError::TooLarge);
        assert_eq!(cache.stats().entries, 0);
        assert_eq!(std::fs::read_dir(cache.dir()).unwrap().count(), 0);
    }

    #[test]
    fn invalid_urls_are_rejected_before_any_download() {
        let scratch = ScratchDir::new("video-cache-unsafe");
        let fetcher = FakeFetcher::writing(b"bytes".to_vec());
        let cache = VideoCache::open_with_fetcher(
            VideoCacheConfig::new(scratch.path()).with_max_bytes(1024 * 1024),
            fetcher.clone(),
        )
        .expect("cache");
        for bad in [
            "http://video.twimg.com/x.mp4",
            "https://127.0.0.1/x.mp4",
            "https://video.twimg.com/x.mp4#t=1",
        ] {
            assert_eq!(cache.ensure(bad, 1024), Err(MediaError::UnsafeUrl));
            assert_eq!(cache.cached(bad), None);
        }
        assert_eq!(fetcher.calls(), 0);
    }

    #[test]
    fn oversized_download_cap_is_refused_without_downloading() {
        let scratch = ScratchDir::new("video-cache-cap");
        let fetcher = FakeFetcher::writing(b"bytes".to_vec());
        let cache = VideoCache::open_with_fetcher(
            VideoCacheConfig::new(scratch.path()).with_max_bytes(10),
            fetcher.clone(),
        )
        .expect("cache");
        assert_eq!(
            cache.ensure(&url("too-large"), 100),
            Err(MediaError::TooLarge)
        );
        assert_eq!(fetcher.calls(), 0);
    }

    #[test]
    fn duplicate_downloads_are_suppressed() {
        let scratch = ScratchDir::new("video-cache-dedupe");
        let (fetcher, release, started) = FakeFetcher::blocking();
        let cache = Arc::new(
            VideoCache::open_with_fetcher(
                VideoCacheConfig::new(scratch.path()).with_max_bytes(1024 * 1024),
                fetcher.clone(),
            )
            .expect("cache"),
        );
        let target = url("dedupe");
        let first = {
            let cache = Arc::clone(&cache);
            let target = target.clone();
            std::thread::spawn(move || cache.ensure(&target, 1024))
        };
        started
            .recv_timeout(Duration::from_secs(5))
            .expect("leader started");
        let second = {
            let cache = Arc::clone(&cache);
            let target = target.clone();
            std::thread::spawn(move || cache.ensure(&target, 1024))
        };
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(fetcher.calls(), 1, "followers must not download");
        release.send(()).expect("release leader");
        let first = first.join().expect("first thread").expect("first result");
        let second = second
            .join()
            .expect("second thread")
            .expect("second result");
        assert_eq!(first.path, second.path);
        assert!(second.from_cache);
        assert_eq!(fetcher.calls(), 1);
    }

    #[test]
    fn cancelled_waiter_returns_without_removing_the_download() {
        let scratch = ScratchDir::new("video-cache-cancel-waiter");
        let (fetcher, release, started) = FakeFetcher::blocking();
        let cache = Arc::new(
            VideoCache::open_with_fetcher(
                VideoCacheConfig::new(scratch.path()).with_max_bytes(1024 * 1024),
                fetcher,
            )
            .expect("cache"),
        );
        let target = url("cancel-waiter");
        let leader = {
            let cache = Arc::clone(&cache);
            let target = target.clone();
            std::thread::spawn(move || cache.ensure(&target, 1024))
        };
        started
            .recv_timeout(Duration::from_secs(5))
            .expect("leader started");
        let cancel = Arc::new(AtomicBool::new(true));
        let waiter = {
            let cache = Arc::clone(&cache);
            let target = target.clone();
            let cancel = Arc::clone(&cancel);
            std::thread::spawn(move || cache.ensure_cancellable(&target, 1024, &cancel))
        };
        assert_eq!(
            waiter.join().expect("waiter thread"),
            Err(MediaError::Cancelled)
        );
        release.send(()).expect("release leader");
        leader
            .join()
            .expect("leader thread")
            .expect("leader result");
        assert_eq!(cache.stats().entries, 1);
    }

    #[test]
    fn cancelled_leader_cleans_up_partial_file() {
        let scratch = ScratchDir::new("video-cache-cancel-leader");
        struct CancellableFetcher {
            cancel: Arc<AtomicBool>,
        }
        impl VideoFetcher for CancellableFetcher {
            fn fetch_to(
                &self,
                _url: &str,
                destination: &Path,
                _max_bytes: u64,
                cancel: &AtomicBool,
            ) -> Result<u64, MediaError> {
                std::fs::write(destination, b"partial").map_err(|_| MediaError::Io)?;
                while !cancel.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                self.cancel.store(true, Ordering::SeqCst);
                Err(MediaError::Cancelled)
            }
        }
        let flag = Arc::new(AtomicBool::new(false));
        let cache = VideoCache::open_with_fetcher(
            VideoCacheConfig::new(scratch.path()).with_max_bytes(1024 * 1024),
            Arc::new(CancellableFetcher {
                cancel: Arc::clone(&flag),
            }),
        )
        .expect("cache");
        let stopper = {
            let flag = Arc::clone(&flag);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(30));
                flag.store(true, Ordering::SeqCst);
            })
        };
        assert_eq!(
            cache.ensure_cancellable(&url("cancel-leader"), 1024, &flag),
            Err(MediaError::Cancelled)
        );
        stopper.join().expect("stopper");
        assert_eq!(cache.stats().entries, 0);
        assert_eq!(std::fs::read_dir(cache.dir()).unwrap().count(), 0);
    }

    #[test]
    fn zero_budget_is_rejected() {
        let scratch = ScratchDir::new("video-cache-zero");
        assert!(VideoCache::open(VideoCacheConfig::new(scratch.path()).with_max_bytes(0)).is_err());
    }

    #[test]
    fn cache_types_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<VideoCache>();
        assert_send_sync::<std::sync::Arc<VideoCache>>();
        assert_send_sync::<super::CachedVideo>();
    }

    #[test]
    fn stats_track_max_bytes() {
        let scratch = ScratchDir::new("video-cache-stats");
        let cache = cache(&scratch, 4096);
        let stats = cache.stats();
        assert_eq!(stats.max_bytes, 4096);
        assert_eq!(stats.entries, 0);
        assert_eq!(stats.bytes, 0);
    }
}
