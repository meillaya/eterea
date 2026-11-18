//! Bounded, cancellable image fetch worker pool.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};

use crate::cache::{CacheKey, CacheStats, DiskCache, DiskCacheConfig};
use crate::error::MediaError;
use crate::http::HttpClient;
use crate::request::{ImageKind, ImageRequest, ImageSource, MediaEvent, RequestId};
use crate::url::is_allowed_media_url;
use crate::{
    DEFAULT_EVENT_BOUND, DEFAULT_QUEUE_BOUND, DEFAULT_WORKERS, HTTP_TIMEOUT, MAX_RESPONSE_BYTES,
    MAX_WORKERS,
};

/// Configuration for [`MediaManager`].
///
/// The disk cache is opt-in: `cache` defaults to `None`. Every field is kept
/// inside a safe range by [`MediaManager::new`]; see [`MediaManager::worker_count`]
/// and [`MediaManager::cache_stats`] for the effective values.
#[derive(Debug, Clone)]
pub struct MediaManagerConfig {
    /// Number of blocking worker threads. Clamped to `1..=`[`MAX_WORKERS`].
    pub worker_count: usize,
    /// Capacity of the request queue. At least 1.
    pub queue_bound: usize,
    /// Capacity of the event queue. At least 1.
    pub event_bound: usize,
    /// Per-response byte cap. Clamped to `1..=`[`MAX_RESPONSE_BYTES`].
    pub max_response_bytes: usize,
    /// Per-exchange HTTP timeout. A zero duration falls back to [`HTTP_TIMEOUT`].
    pub timeout: Duration,
    /// Optional disk cache configuration. `None` disables the cache.
    pub cache: Option<DiskCacheConfig>,
}

impl Default for MediaManagerConfig {
    fn default() -> Self {
        Self {
            worker_count: DEFAULT_WORKERS,
            queue_bound: DEFAULT_QUEUE_BOUND,
            event_bound: DEFAULT_EVENT_BOUND,
            max_response_bytes: MAX_RESPONSE_BYTES,
            timeout: HTTP_TIMEOUT,
            cache: None,
        }
    }
}

impl MediaManagerConfig {
    /// Start from the defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable the disk cache with the given configuration.
    pub fn with_cache(mut self, cache: DiskCacheConfig) -> Self {
        self.cache = Some(cache);
        self
    }

    fn normalized(mut self) -> Self {
        self.worker_count = self.worker_count.clamp(1, MAX_WORKERS);
        self.queue_bound = self.queue_bound.max(1);
        self.event_bound = self.event_bound.max(1);
        self.max_response_bytes = self.max_response_bytes.clamp(1, MAX_RESPONSE_BYTES);
        if self.timeout.is_zero() {
            self.timeout = HTTP_TIMEOUT;
        }
        self
    }
}

/// Fixed worker pool that fetches images over HTTPS.
///
/// The manager owns [`MediaManagerConfig::worker_count`] blocking threads and
/// two bounded queues: requests in, [`MediaEvent`] out. Callers submit with
/// [`Self::enqueue`], poll with [`Self::try_recv`], and may cancel by id with
/// [`Self::cancel`] or drop everything with [`Self::cancel_all`]. Dropping the
/// manager closes the request queue; workers exit after the current fetch and
/// any remaining events are discarded.
///
/// The worker pool never spawns threads per request, never blocks the caller,
/// and never writes media bytes to SQLite.
pub struct MediaManager {
    jobs: Sender<Job>,
    events: Receiver<MediaEvent>,
    shared: Arc<Shared>,
    cache: Option<Arc<DiskCache>>,
    config: MediaManagerConfig,
    workers: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for MediaManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaManager")
            .field("worker_count", &self.worker_count())
            .field("pending", &self.shared.pending_count())
            .field("cache", &self.cache)
            .finish_non_exhaustive()
    }
}

impl MediaManager {
    /// Build a manager with the real HTTPS fetcher.
    ///
    /// Fails with [`MediaError::Io`] when the configured disk cache cannot be
    /// opened, and with [`MediaError::InvalidConfig`] when a worker thread
    /// cannot be spawned.
    pub fn new(config: MediaManagerConfig) -> Result<Self, MediaError> {
        // Normalize first: the HTTP client must see the same clamped timeout
        // that the rest of the manager records.
        let config = config.normalized();
        let fetcher: Arc<dyn Fetcher> = Arc::new(HttpsFetcher {
            client: HttpClient::new(config.timeout),
        });
        Self::build(config, fetcher)
    }

    fn build(config: MediaManagerConfig, fetcher: Arc<dyn Fetcher>) -> Result<Self, MediaError> {
        let config = config.normalized();
        let cache = match &config.cache {
            Some(cache_config) => Some(Arc::new(
                DiskCache::open(cache_config.clone()).map_err(|_| MediaError::Io)?,
            )),
            None => None,
        };
        let (jobs, job_receiver) = bounded::<Job>(config.queue_bound);
        let (event_sender, events) = bounded::<MediaEvent>(config.event_bound);
        let shared = Arc::new(Shared::default());
        let mut workers = Vec::with_capacity(config.worker_count);
        for index in 0..config.worker_count {
            let job_receiver = job_receiver.clone();
            let event_sender = event_sender.clone();
            let shared = Arc::clone(&shared);
            let fetcher = Arc::clone(&fetcher);
            let cache = cache.clone();
            let max_response_bytes = config.max_response_bytes;
            let handle = std::thread::Builder::new()
                .name(format!("eterea-media-{index}"))
                .spawn(move || {
                    worker_loop(
                        job_receiver,
                        event_sender,
                        shared,
                        fetcher,
                        cache,
                        max_response_bytes,
                    );
                })
                .map_err(|_| MediaError::InvalidConfig)?;
            workers.push(handle);
        }
        Ok(Self {
            jobs,
            events,
            shared,
            cache,
            config,
            workers,
        })
    }

    /// Enqueue an image request by id, URL and kind.
    ///
    /// The URL is checked with [`crate::is_allowed_media_url`] before the
    /// request is queued. Returns [`MediaError::UnsafeUrl`] for a rejected URL,
    /// [`MediaError::DuplicateRequest`] when the id is still outstanding,
    /// [`MediaError::QueueFull`] when the bounded queue is full, and
    /// [`MediaError::Closed`] when the manager is shutting down.
    ///
    /// This call never blocks on the network.
    pub fn enqueue(
        &self,
        id: impl Into<RequestId>,
        url: impl Into<String>,
        kind: ImageKind,
    ) -> Result<(), MediaError> {
        self.enqueue_request(ImageRequest::new(id, url, kind))
    }

    /// Enqueue a prepared [`ImageRequest`]; the same as [`Self::enqueue`].
    pub fn enqueue_request(&self, request: ImageRequest) -> Result<(), MediaError> {
        if !is_allowed_media_url(&request.url) {
            return Err(MediaError::UnsafeUrl);
        }
        let id = request.id;
        let cancel = self.shared.register(id)?;
        let job = Job {
            request,
            cancel: Arc::clone(&cancel),
        };
        match self.jobs.try_send(job) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.shared.unregister_if_current(id, &cancel);
                Err(MediaError::QueueFull)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.shared.unregister_if_current(id, &cancel);
                Err(MediaError::Closed)
            }
        }
    }

    /// Cancel an outstanding request.
    ///
    /// Returns `true` when the request was still outstanding. In that case no
    /// [`MediaEvent`] is emitted for the id: a result that arrives after the
    /// cancellation is dropped. A request that already produced an event
    /// returns `false`.
    pub fn cancel(&self, id: impl Into<RequestId>) -> bool {
        self.shared.cancel(id.into())
    }

    /// Cancel every outstanding request. Returns how many were cancelled.
    pub fn cancel_all(&self) -> usize {
        self.shared.cancel_all()
    }

    /// Number of requests that are queued or currently being fetched.
    pub fn pending(&self) -> usize {
        self.shared.pending_count()
    }

    /// Drain one completed request, if any. Never blocks.
    pub fn try_recv(&self) -> Option<MediaEvent> {
        self.events.try_recv().ok()
    }

    /// Effective worker count after configuration clamping.
    pub fn worker_count(&self) -> usize {
        self.config.worker_count
    }

    /// Disk cache occupancy, or `None` when the cache is disabled (default).
    pub fn cache_stats(&self) -> Option<CacheStats> {
        self.cache.as_ref().map(|cache| cache.stats())
    }

    /// Empty the opt-in disk cache.
    ///
    /// Returns `Ok(false)` when the cache is disabled, `Ok(true)` when it was
    /// cleared, and [`MediaError::Io`] on a filesystem error.
    pub fn clear_cache(&self) -> Result<bool, MediaError> {
        match &self.cache {
            Some(cache) => cache.clear().map(|()| true).map_err(|_| MediaError::Io),
            None => Ok(false),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_fetcher(
        config: MediaManagerConfig,
        fetcher: Arc<dyn Fetcher>,
    ) -> Result<Self, MediaError> {
        Self::build(config, fetcher)
    }
}

impl Drop for MediaManager {
    fn drop(&mut self) {
        // Mark everything outstanding as cancelled so in-flight workers abort
        // between chunks; then the request sender drops and the workers exit.
        self.shared.cancel_all();
        // Owning the handles keeps the workers tied to the manager's lifetime;
        // dropping a handle detaches the thread, which exits on its own once
        // the closed request queue is observed. There is deliberately no join
        // here: a blocking TLS read must not stall the UI thread on shutdown.
        for worker in self.workers.drain(..) {
            drop(worker);
        }
    }
}

struct Job {
    request: ImageRequest,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct Shared {
    pending: Mutex<HashMap<RequestId, Arc<AtomicBool>>>,
}

impl Shared {
    fn register(&self, id: RequestId) -> Result<Arc<AtomicBool>, MediaError> {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut pending = self.pending.lock().expect("media pending lock poisoned");
        if pending.contains_key(&id) {
            return Err(MediaError::DuplicateRequest);
        }
        pending.insert(id, Arc::clone(&cancel));
        Ok(cancel)
    }

    fn is_current(&self, id: RequestId, cancel: &Arc<AtomicBool>) -> bool {
        self.pending
            .lock()
            .expect("media pending lock poisoned")
            .get(&id)
            .map(|current| Arc::ptr_eq(current, cancel))
            .unwrap_or(false)
    }

    fn unregister_if_current(&self, id: RequestId, cancel: &Arc<AtomicBool>) -> bool {
        let mut pending = self.pending.lock().expect("media pending lock poisoned");
        let matches = pending
            .get(&id)
            .map(|current| Arc::ptr_eq(current, cancel))
            .unwrap_or(false);
        if matches {
            pending.remove(&id);
        }
        matches
    }

    fn cancel(&self, id: RequestId) -> bool {
        let mut pending = self.pending.lock().expect("media pending lock poisoned");
        match pending.remove(&id) {
            Some(cancel) => {
                cancel.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    fn cancel_all(&self) -> usize {
        let mut pending = self.pending.lock().expect("media pending lock poisoned");
        let count = pending.len();
        for (_, cancel) in pending.drain() {
            cancel.store(true, Ordering::SeqCst);
        }
        count
    }

    fn pending_count(&self) -> usize {
        self.pending
            .lock()
            .expect("media pending lock poisoned")
            .len()
    }
}

/// Injectable image transport. Production uses [`HttpsFetcher`]; unit tests
/// substitute a scripted fake so no test touches the network.
pub(crate) trait Fetcher: Send + Sync {
    fn fetch(
        &self,
        request: &ImageRequest,
        cancel: &AtomicBool,
        max_bytes: usize,
    ) -> Result<Vec<u8>, MediaError>;
}

struct HttpsFetcher {
    client: HttpClient,
}

impl Fetcher for HttpsFetcher {
    fn fetch(
        &self,
        request: &ImageRequest,
        cancel: &AtomicBool,
        max_bytes: usize,
    ) -> Result<Vec<u8>, MediaError> {
        self.client.fetch_bytes(&request.url, cancel, max_bytes)
    }
}

fn worker_loop(
    jobs: Receiver<Job>,
    events: Sender<MediaEvent>,
    shared: Arc<Shared>,
    fetcher: Arc<dyn Fetcher>,
    cache: Option<Arc<DiskCache>>,
    max_response_bytes: usize,
) {
    while let Ok(job) = jobs.recv() {
        let id = job.request.id;
        let kind = job.request.kind;
        // Cancelled while queued, or the id was reused by a newer request:
        // the stale queue entry is dropped without touching the network.
        if !shared.is_current(id, &job.cancel) || job.cancel.load(Ordering::SeqCst) {
            shared.unregister_if_current(id, &job.cancel);
            continue;
        }
        let outcome = load(&job, fetcher.as_ref(), cache.as_deref(), max_response_bytes);
        // Remove the id before emitting: if `cancel` won the race, the entry is
        // already gone and the late result is dropped.
        if !shared.unregister_if_current(id, &job.cancel) {
            continue;
        }
        let event = match outcome {
            Ok((bytes, source)) => MediaEvent::ImageReady {
                id,
                kind,
                source,
                bytes,
            },
            Err(MediaError::Cancelled) => continue,
            Err(error) => MediaEvent::ImageFailed { id, kind, error },
        };
        if events.send(event).is_err() {
            return;
        }
    }
}

fn load(
    job: &Job,
    fetcher: &dyn Fetcher,
    cache: Option<&DiskCache>,
    max_response_bytes: usize,
) -> Result<(Vec<u8>, ImageSource), MediaError> {
    // Defense in depth: the URL was validated at enqueue, but the policy is
    // cheap and the worker is the last place before a socket is opened.
    if !is_allowed_media_url(&job.request.url) {
        return Err(MediaError::UnsafeUrl);
    }
    let key = CacheKey::for_request(job.request.kind, &job.request.url);
    if let Some(cache) = cache {
        if let Some(bytes) = cache.get(&key) {
            return Ok((bytes, ImageSource::DiskCache));
        }
    }
    let bytes = fetcher.fetch(&job.request, &job.cancel, max_response_bytes)?;
    if bytes.is_empty() {
        return Err(MediaError::EmptyResponse);
    }
    if let Some(cache) = cache {
        // Best effort: a cache write failure never fails a fetch.
        cache.insert(&key, &bytes);
    }
    Ok((bytes, ImageSource::Network))
}

#[cfg(test)]
mod tests {
    use super::{Fetcher, MediaManager, MediaManagerConfig};
    use crate::cache::{CacheKey, DiskCache, DiskCacheConfig};
    use crate::request::{ImageKind, ImageRequest, ImageSource, MediaEvent, RequestId};
    use crate::test_support::ScratchDir;
    use crate::{MediaError, MAX_WORKERS};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    type Behavior = Box<dyn Fn(&ImageRequest) -> Result<Vec<u8>, MediaError> + Send + Sync>;

    struct FakeFetcher {
        calls: AtomicUsize,
        behavior: Behavior,
        started: Option<Sender<RequestId>>,
        release: Option<Mutex<Receiver<()>>>,
    }

    impl FakeFetcher {
        fn new(behavior: Behavior) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                behavior,
                started: None,
                release: None,
            })
        }

        fn blocking(
            behavior: Behavior,
            started: Sender<RequestId>,
            release: Receiver<()>,
        ) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                behavior,
                started: Some(started),
                release: Some(Mutex::new(release)),
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl Fetcher for FakeFetcher {
        fn fetch(
            &self,
            request: &ImageRequest,
            _cancel: &std::sync::atomic::AtomicBool,
            _max_bytes: usize,
        ) -> Result<Vec<u8>, MediaError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(started) = &self.started {
                let _ = started.send(request.id);
            }
            if let Some(release) = &self.release {
                release
                    .lock()
                    .expect("fake release lock")
                    .recv_timeout(Duration::from_secs(5))
                    .expect("test must release the fake fetch");
            }
            (self.behavior)(request)
        }
    }

    fn echo_bytes() -> Behavior {
        Box::new(|request| Ok(request.url.clone().into_bytes()))
    }

    fn test_config() -> MediaManagerConfig {
        MediaManagerConfig {
            worker_count: 1,
            queue_bound: 8,
            event_bound: 32,
            ..MediaManagerConfig::default()
        }
    }

    fn manager_with(fetcher: Arc<FakeFetcher>, config: MediaManagerConfig) -> MediaManager {
        MediaManager::with_fetcher(config, fetcher).expect("media manager")
    }

    fn blocking_setup(behavior: Behavior) -> (Arc<FakeFetcher>, Sender<()>, Receiver<RequestId>) {
        let (release_tx, release_rx) = channel();
        let (started_tx, started_rx) = channel();
        let fetcher = FakeFetcher::blocking(behavior, started_tx, release_rx);
        (fetcher, release_tx, started_rx)
    }

    fn next_event(manager: &MediaManager) -> MediaEvent {
        manager
            .events
            .recv_timeout(Duration::from_secs(5))
            .expect("media event")
    }

    struct Ready {
        id: RequestId,
        kind: ImageKind,
        source: ImageSource,
        bytes: Vec<u8>,
    }

    fn expect_ready(event: MediaEvent) -> Ready {
        match event {
            MediaEvent::ImageReady {
                id,
                kind,
                source,
                bytes,
            } => Ready {
                id,
                kind,
                source,
                bytes,
            },
            other => panic!("expected image ready, got {other:?}"),
        }
    }

    fn expect_failed(event: MediaEvent) -> (RequestId, MediaError) {
        match event {
            MediaEvent::ImageFailed { id, error, .. } => (id, error),
            other => panic!("expected image failure, got {other:?}"),
        }
    }

    #[test]
    fn fetches_image_bytes_and_reports_source() {
        let fetcher = FakeFetcher::new(echo_bytes());
        let manager = manager_with(Arc::clone(&fetcher), test_config());
        manager
            .enqueue(
                1_u64,
                "https://pbs.twimg.com/media/a.jpg",
                ImageKind::Avatar,
            )
            .expect("enqueue");
        let ready = expect_ready(next_event(&manager));
        assert_eq!(ready.id, RequestId(1));
        assert_eq!(ready.kind, ImageKind::Avatar);
        assert_eq!(ready.source, ImageSource::Network);
        assert_eq!(ready.bytes, b"https://pbs.twimg.com/media/a.jpg");
        assert_eq!(fetcher.calls(), 1);
        assert_eq!(manager.pending(), 0);
        assert_eq!(manager.try_recv(), None);
    }

    #[test]
    fn unsafe_urls_are_rejected_before_the_worker_runs() {
        let fetcher = FakeFetcher::new(echo_bytes());
        let manager = manager_with(Arc::clone(&fetcher), test_config());
        for url in [
            "http://pbs.twimg.com/media/a.jpg",
            "https://127.0.0.1/a.jpg",
            "https://pbs.twimg.com/media/a.jpg#frag",
            "https://user:pass@pbs.twimg.com/a.jpg",
        ] {
            assert_eq!(
                manager.enqueue(1_u64, url, ImageKind::Image),
                Err(MediaError::UnsafeUrl),
                "{url}"
            );
        }
        assert_eq!(fetcher.calls(), 0);
        assert_eq!(manager.try_recv(), None);
        assert!(!manager.cancel(1_u64));
    }

    #[test]
    fn duplicate_pending_ids_are_rejected() {
        let (fetcher, release, started) = blocking_setup(echo_bytes());
        let manager = manager_with(Arc::clone(&fetcher), test_config());
        manager
            .enqueue(1_u64, "https://pbs.twimg.com/media/a.jpg", ImageKind::Image)
            .expect("enqueue");
        assert_eq!(
            started.recv_timeout(Duration::from_secs(5)).unwrap(),
            RequestId(1)
        );
        assert_eq!(
            manager.enqueue(1_u64, "https://pbs.twimg.com/media/b.jpg", ImageKind::Image),
            Err(MediaError::DuplicateRequest)
        );
        release.send(()).expect("release");
        assert_eq!(expect_ready(next_event(&manager)).id, RequestId(1));
        assert_eq!(fetcher.calls(), 1);
        // The id is free again after completion and can be reused.
        manager
            .enqueue(1_u64, "https://pbs.twimg.com/media/c.jpg", ImageKind::Image)
            .expect("reuse id");
        release.send(()).expect("release reused fetch");
        assert_eq!(expect_ready(next_event(&manager)).id, RequestId(1));
    }

    #[test]
    fn failures_are_reported_without_bytes() {
        let fetcher = FakeFetcher::new(Box::new(|_| Err(MediaError::HttpStatus(404))));
        let manager = manager_with(Arc::clone(&fetcher), test_config());
        manager
            .enqueue(
                7_u64,
                "https://pbs.twimg.com/media/missing.jpg",
                ImageKind::Image,
            )
            .expect("enqueue");
        let (id, error) = expect_failed(next_event(&manager));
        assert_eq!(id, RequestId(7));
        assert_eq!(error, MediaError::HttpStatus(404));
        assert_eq!(manager.pending(), 0);
    }

    #[test]
    fn request_queue_is_bounded_and_cancelled_queued_requests_never_fetch() {
        let (fetcher, release, started) = blocking_setup(echo_bytes());
        let config = MediaManagerConfig {
            worker_count: 1,
            queue_bound: 1,
            event_bound: 8,
            ..MediaManagerConfig::default()
        };
        let manager = manager_with(Arc::clone(&fetcher), config);
        manager
            .enqueue(1_u64, "https://pbs.twimg.com/media/a.jpg", ImageKind::Image)
            .expect("first request fits");
        assert_eq!(
            started.recv_timeout(Duration::from_secs(5)).unwrap(),
            RequestId(1)
        );
        manager
            .enqueue(2_u64, "https://pbs.twimg.com/media/b.jpg", ImageKind::Image)
            .expect("second request fills the queue");
        assert_eq!(
            manager.enqueue(3_u64, "https://pbs.twimg.com/media/c.jpg", ImageKind::Image),
            Err(MediaError::QueueFull)
        );
        assert!(manager.cancel(2_u64));
        release.send(()).expect("release");
        assert_eq!(expect_ready(next_event(&manager)).id, RequestId(1));
        assert_eq!(manager.try_recv(), None);
        assert_eq!(
            fetcher.calls(),
            1,
            "cancelled queued request must not fetch"
        );
        assert_eq!(manager.pending(), 0);
    }

    #[test]
    fn cancel_drops_a_result_that_was_already_in_flight() {
        let (fetcher, release, started) = blocking_setup(echo_bytes());
        let manager = manager_with(Arc::clone(&fetcher), test_config());
        manager
            .enqueue(1_u64, "https://pbs.twimg.com/media/a.jpg", ImageKind::Image)
            .expect("enqueue");
        assert_eq!(
            started.recv_timeout(Duration::from_secs(5)).unwrap(),
            RequestId(1)
        );
        assert!(manager.cancel(1_u64));
        assert!(!manager.cancel(1_u64), "already cancelled");
        // Queue a marker request; it is the only event that may surface.
        manager
            .enqueue(2_u64, "https://pbs.twimg.com/media/b.jpg", ImageKind::Image)
            .expect("enqueue marker");
        release.send(()).expect("release first");
        release.send(()).expect("release second");
        let ready = expect_ready(next_event(&manager));
        assert_eq!(ready.id, RequestId(2));
        assert_eq!(manager.try_recv(), None);
        assert_eq!(fetcher.calls(), 2);
    }

    #[test]
    fn reused_id_ignores_the_stale_queue_entry() {
        let (fetcher, release, started) = blocking_setup(echo_bytes());
        let manager = manager_with(Arc::clone(&fetcher), test_config());
        manager
            .enqueue(1_u64, "https://pbs.twimg.com/media/a.jpg", ImageKind::Image)
            .expect("enqueue a");
        assert_eq!(
            started.recv_timeout(Duration::from_secs(5)).unwrap(),
            RequestId(1)
        );
        manager
            .enqueue(2_u64, "https://pbs.twimg.com/media/b.jpg", ImageKind::Image)
            .expect("enqueue stale b");
        assert!(manager.cancel(2_u64));
        manager
            .enqueue(2_u64, "https://pbs.twimg.com/media/c.jpg", ImageKind::Image)
            .expect("enqueue replacement c");
        release.send(()).expect("release a");
        release.send(()).expect("release c");
        let first = expect_ready(next_event(&manager));
        assert_eq!(first.id, RequestId(1));
        assert_eq!(first.bytes, b"https://pbs.twimg.com/media/a.jpg");
        let second = expect_ready(next_event(&manager));
        assert_eq!(second.id, RequestId(2));
        assert_eq!(second.bytes, b"https://pbs.twimg.com/media/c.jpg");
        assert_eq!(manager.try_recv(), None);
        assert_eq!(
            fetcher.calls(),
            2,
            "the stale queued request must not fetch"
        );
    }

    #[test]
    fn cancel_all_drops_everything_outstanding() {
        let (fetcher, release, started) = blocking_setup(echo_bytes());
        let manager = manager_with(Arc::clone(&fetcher), test_config());
        manager
            .enqueue(1_u64, "https://pbs.twimg.com/media/a.jpg", ImageKind::Image)
            .expect("enqueue a");
        manager
            .enqueue(2_u64, "https://pbs.twimg.com/media/b.jpg", ImageKind::Image)
            .expect("enqueue b");
        assert_eq!(
            started.recv_timeout(Duration::from_secs(5)).unwrap(),
            RequestId(1)
        );
        assert_eq!(manager.cancel_all(), 2);
        assert_eq!(manager.pending(), 0);
        manager
            .enqueue(3_u64, "https://pbs.twimg.com/media/c.jpg", ImageKind::Image)
            .expect("enqueue marker");
        release.send(()).expect("release first");
        release.send(()).expect("release second");
        let ready = expect_ready(next_event(&manager));
        assert_eq!(ready.id, RequestId(3));
        assert_eq!(manager.try_recv(), None);
        assert_eq!(fetcher.calls(), 2, "only the marker request is fetched");
    }

    #[test]
    fn pending_tracks_queued_and_in_flight_requests() {
        let (fetcher, release, started) = blocking_setup(echo_bytes());
        let manager = manager_with(Arc::clone(&fetcher), test_config());
        assert_eq!(manager.pending(), 0);
        manager
            .enqueue(1_u64, "https://pbs.twimg.com/media/a.jpg", ImageKind::Image)
            .expect("enqueue a");
        manager
            .enqueue(2_u64, "https://pbs.twimg.com/media/b.jpg", ImageKind::Image)
            .expect("enqueue b");
        assert_eq!(
            started.recv_timeout(Duration::from_secs(5)).unwrap(),
            RequestId(1)
        );
        assert_eq!(manager.pending(), 2);
        assert!(manager.cancel(1_u64));
        assert_eq!(manager.pending(), 1);
        assert!(manager.cancel(2_u64));
        assert_eq!(manager.pending(), 0);
        release.send(()).expect("release");
    }

    #[test]
    fn cache_hit_skips_the_network() {
        let scratch = ScratchDir::new("manager-cache-hit");
        let url = "https://pbs.twimg.com/media/cached.jpg";
        let key = CacheKey::for_request(ImageKind::Image, url);
        let disk = DiskCache::open(DiskCacheConfig::new(scratch.path())).expect("open cache");
        assert!(disk.insert(&key, b"cached bytes"));
        drop(disk);

        let fetcher = FakeFetcher::new(Box::new(|_| Err(MediaError::Connection)));
        let config = test_config().with_cache(DiskCacheConfig::new(scratch.path()));
        let manager = manager_with(Arc::clone(&fetcher), config);
        manager
            .enqueue(1_u64, url, ImageKind::Image)
            .expect("enqueue");
        let ready = expect_ready(next_event(&manager));
        assert_eq!(ready.source, ImageSource::DiskCache);
        assert_eq!(ready.bytes, b"cached bytes");
        assert_eq!(fetcher.calls(), 0);
    }

    #[test]
    fn unsupported_content_type_failures_are_typed_and_never_cached() {
        let scratch = ScratchDir::new("manager-mime-reject");
        let url = "https://pbs.twimg.com/media/not-an-image.jpg";
        // The real HttpsFetcher returns UnsupportedType from the response
        // Content-Type gate in http.rs before it reads the body; the manager
        // contract under test is "typed failure, no cache write, no poisoning".
        let fetcher = FakeFetcher::new(Box::new(|_| Err(MediaError::UnsupportedType)));
        let config = test_config().with_cache(DiskCacheConfig::new(scratch.path()));
        let manager = manager_with(Arc::clone(&fetcher), config);

        manager
            .enqueue(11_u64, url, ImageKind::Image)
            .expect("enqueue");
        let (id, error) = expect_failed(next_event(&manager));
        assert_eq!(id, RequestId(11));
        assert_eq!(error, MediaError::UnsupportedType);
        assert_eq!(
            manager.cache_stats().expect("cache stats").entries,
            0,
            "a rejected content type must not be cached"
        );

        // A second request must go back to the network: the failure did not
        // create a cache entry that would be served without re-validation.
        manager
            .enqueue(12_u64, url, ImageKind::Image)
            .expect("retry enqueue");
        let (id, error) = expect_failed(next_event(&manager));
        assert_eq!(id, RequestId(12));
        assert_eq!(error, MediaError::UnsupportedType);
        assert_eq!(fetcher.calls(), 2);
        assert_eq!(manager.cache_stats().expect("cache stats").entries, 0);
    }

    #[test]
    fn fetches_populate_the_cache_and_clear_empties_it() {
        let scratch = ScratchDir::new("manager-cache-write");
        let url = "https://pbs.twimg.com/media/written.jpg";
        let fetcher = FakeFetcher::new(Box::new(|_| Ok(b"network bytes".to_vec())));
        let config = test_config().with_cache(DiskCacheConfig::new(scratch.path()));
        let manager = manager_with(Arc::clone(&fetcher), config);

        manager
            .enqueue(1_u64, url, ImageKind::Image)
            .expect("enqueue");
        let ready = expect_ready(next_event(&manager));
        assert_eq!(ready.source, ImageSource::Network);
        assert_eq!(ready.bytes, b"network bytes");
        let stats = manager.cache_stats().expect("cache stats");
        assert_eq!(stats.entries, 1);
        assert!(stats.bytes > 0);

        manager
            .enqueue(1_u64, url, ImageKind::Image)
            .expect("reuse id after completion");
        let ready = expect_ready(next_event(&manager));
        assert_eq!(ready.source, ImageSource::DiskCache);
        assert_eq!(fetcher.calls(), 1);

        assert!(manager.clear_cache().expect("clear cache"));
        let stats = manager.cache_stats().expect("cache stats");
        assert_eq!(stats.entries, 0);
        assert_eq!(stats.bytes, 0);

        manager
            .enqueue(2_u64, url, ImageKind::Image)
            .expect("enqueue after clear");
        let ready = expect_ready(next_event(&manager));
        assert_eq!(ready.source, ImageSource::Network);
        assert_eq!(fetcher.calls(), 2);
    }

    #[test]
    fn cache_is_off_by_default() {
        let manager = MediaManager::new(MediaManagerConfig {
            worker_count: 1,
            ..MediaManagerConfig::default()
        })
        .expect("manager without cache");
        assert_eq!(manager.cache_stats(), None);
        assert_eq!(manager.clear_cache(), Ok(false));
        assert_eq!(manager.try_recv(), None);
    }

    #[test]
    fn configuration_is_clamped() {
        let manager = MediaManager::new(MediaManagerConfig {
            worker_count: 0,
            queue_bound: 0,
            event_bound: 0,
            timeout: Duration::ZERO,
            max_response_bytes: usize::MAX,
            ..MediaManagerConfig::default()
        })
        .expect("manager");
        assert_eq!(manager.worker_count(), 1);
        drop(manager);
        let manager = MediaManager::new(MediaManagerConfig {
            worker_count: 99,
            ..MediaManagerConfig::default()
        })
        .expect("manager");
        assert_eq!(manager.worker_count(), MAX_WORKERS);
    }

    #[test]
    fn debug_output_does_not_dump_image_bytes() {
        let fetcher = FakeFetcher::new(Box::new(|_| Ok(vec![9, 9, 9, 9])));
        let manager = manager_with(Arc::clone(&fetcher), test_config());
        manager
            .enqueue(1_u64, "https://pbs.twimg.com/media/a.jpg", ImageKind::Image)
            .expect("enqueue");
        let event = next_event(&manager);
        let text = format!("{event:?}");
        assert!(text.contains("bytes_len: 4"));
        let manager_text = format!("{manager:?}");
        assert!(manager_text.contains("worker_count"));
    }
    #[test]
    fn empty_bodies_are_failures_not_images() {
        let fetcher = FakeFetcher::new(Box::new(|_| Ok(Vec::new())));
        let manager = manager_with(Arc::clone(&fetcher), test_config());
        manager
            .enqueue(
                4_u64,
                "https://pbs.twimg.com/media/empty.jpg",
                ImageKind::Image,
            )
            .expect("enqueue");
        let (id, error) = expect_failed(next_event(&manager));
        assert_eq!(id, RequestId(4));
        assert_eq!(error, MediaError::EmptyResponse);
        assert_eq!(manager.pending(), 0);
    }

    #[test]
    fn manager_and_events_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MediaManager>();
        assert_send_sync::<MediaEvent>();
        assert_send_sync::<Arc<MediaManager>>();
    }
}
