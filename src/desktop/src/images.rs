//! Image slots, the media worker bridge and the decoded-image budget.
//!
//! The view asks for an image by key. A local demo key is preloaded; an HTTPS
//! key is queued on the media workers and appears when its event arrives.
//!
//! Every decoded image is charged to a byte budget. The shell calls
//! [`ImageStore::evict_to_budget`] each frame with the keys the current view
//! still needs, so a long browsing session keeps a bounded working set instead
//! of pinning every image it has ever shown.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use eterea_logic::image_budget::ByteBudget;
use eterea_media::{
    CacheStats, DiskCacheConfig, ImageKind, ImagePayload, MediaError, MediaEvent, MediaManager,
    MediaManagerConfig,
};
use rune::{Image, ImageCache};

/// Default decoded-image budget: 192 MiB.
const DEFAULT_BUDGET_BYTES: u64 = 192 * 1024 * 1024;
/// Smallest and largest budget the environment override may select.
const MIN_BUDGET_BYTES: u64 = 16 * 1024 * 1024;
const MAX_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;
/// Terminal failures remembered per session, so a broken URL is not retried
/// forever but a healthy session cannot grow this map without bound.
const MAX_FAILED_KEYS: usize = 1024;
/// How often `ETEREA_MEMORY_LOG=1` prints the cache accounting.
const MEMORY_LOG_INTERVAL: Duration = Duration::from_secs(1);

/// One displayed image and what it costs the budget.
struct Slot {
    image: Image,
    /// Longest decoded edge in device pixels; `0` is the original size.
    target_px: u32,
}

/// A request that is on the media workers right now.
struct PendingRequest {
    key: String,
    kind: ImageKind,
    target: u32,
}

/// A key the bounded worker queue refused; retried from `poll` when room frees.
struct DeferredRequest {
    key: String,
    kind: ImageKind,
    target: u32,
}

/// Which requested size a decoded slot serves.
///
/// `0` means "the original image", which covers every display size; otherwise
/// a slot covers the target it was decoded for and anything smaller, but not
/// something larger (that is a quality upgrade).
fn target_covers(existing_px: u32, wanted_px: u32) -> bool {
    existing_px == 0 || (wanted_px != 0 && existing_px >= wanted_px)
}

/// Decoded RGBA footprint including the mip chain Rune builds beside it.
fn mip_weight(width: u32, height: u32) -> u64 {
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(4)
        .saturating_mul(4)
        / 3
}

/// The decoded bytes one payload is charged to the budget.
fn payload_weight(payload: &ImagePayload) -> u64 {
    match payload {
        ImagePayload::Rgba {
            width,
            height,
            bytes,
        } => mip_weight(*width, *height).max(bytes.len() as u64),
        ImagePayload::Encoded {
            estimated_decoded_bytes,
            bytes,
        } => (*estimated_decoded_bytes).max(bytes.len() as u64),
    }
}

/// The budget selected by `ETEREA_IMAGE_BUDGET_MB`, if the test override is set.
fn budget_from_env() -> u64 {
    let Some(value) = std::env::var_os("ETEREA_IMAGE_BUDGET_MB") else {
        return DEFAULT_BUDGET_BYTES;
    };
    value
        .to_string_lossy()
        .trim()
        .parse::<u64>()
        .map(|mebibytes| {
            mebibytes
                .saturating_mul(1024 * 1024)
                .clamp(MIN_BUDGET_BYTES, MAX_BUDGET_BYTES)
        })
        .unwrap_or(DEFAULT_BUDGET_BYTES)
}

fn memory_log_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("ETEREA_MEMORY_LOG").is_some())
}

pub struct ImageStore {
    cache: Option<ImageCache>,
    manager: Option<MediaManager>,
    /// Image handles shown on screen.
    images: HashMap<String, Slot>,
    /// Which slot size serves which key, and when it was last needed.
    budget: ByteBudget<String>,
    /// The byte ceiling for `budget`.
    budget_limit: u64,
    /// Ordering clock for the LRU: bumped by every touch or eviction pass.
    clock: u64,
    pending: HashMap<u64, PendingRequest>,
    /// Terminal failures remembered this session, oldest first for the cap.
    failed: HashMap<String, ()>,
    failed_order: VecDeque<String>,
    /// Transient failures retried so far, per key.
    retries: HashMap<String, u8>,
    /// Terminal failures since the shell last read them, for an honest status.
    new_failures: usize,
    /// Keys the bounded worker queue refused; re-driven from `poll` once room
    /// frees up instead of being marked failed for the session.
    deferred: Vec<DeferredRequest>,
    next_id: u64,
    evictions: u64,
    last_memory_log: Option<Instant>,
}

impl ImageStore {
    /// A placeholder store used during `State::new`, before the Rune waker
    /// exists. `setup` replaces it with [`Self::new`].
    pub fn empty() -> Self {
        Self {
            cache: None,
            manager: None,
            images: HashMap::new(),
            budget: ByteBudget::new(),
            budget_limit: DEFAULT_BUDGET_BYTES,
            clock: 0,
            pending: HashMap::new(),
            failed: HashMap::new(),
            failed_order: VecDeque::new(),
            retries: HashMap::new(),
            new_failures: 0,
            deferred: Vec::new(),
            next_id: 1,
            evictions: 0,
            last_memory_log: None,
        }
    }

    #[allow(dead_code)]
    pub fn new(cache: ImageCache) -> Self {
        Self::with_cache(cache, None)
    }

    pub fn with_cache(cache: ImageCache, cache_dir: Option<&Path>) -> Self {
        let mut config = MediaManagerConfig::default();
        if let Some(dir) = cache_dir {
            config = config.with_cache(DiskCacheConfig::new(dir));
        }
        let manager = MediaManager::new(config).ok();
        Self {
            cache: Some(cache),
            manager,
            images: HashMap::new(),
            budget: ByteBudget::new(),
            budget_limit: budget_from_env(),
            clock: 0,
            pending: HashMap::new(),
            failed: HashMap::new(),
            failed_order: VecDeque::new(),
            retries: HashMap::new(),
            new_failures: 0,
            deferred: Vec::new(),
            next_id: 1,
            evictions: 0,
            last_memory_log: None,
        }
    }

    pub fn resolve(&self, key: &str) -> Option<&Image> {
        self.images.get(key).map(|slot| &slot.image)
    }

    /// Build an image from a decoded video frame.
    pub fn rgba(&self, width: u32, height: u32, bytes: Vec<u8>) -> Option<Image> {
        self.cache
            .as_ref()
            .map(|cache| cache.rgba(width, height, bytes))
    }

    /// Load a local fixture image for the demo deck.
    pub fn preload_file(&mut self, key: &str, path: &str, size: f32) {
        let Some(cache) = self.cache.as_ref() else {
            return;
        };
        let image = cache.file(path, size, 1.0);
        let _ = image.wait();
        let weight = image
            .size()
            .map(|[width, height]| mip_weight(width, height))
            .unwrap_or(0);
        self.clock = self.clock.wrapping_add(1);
        let clock = self.clock;
        self.budget.insert(key.to_string(), weight, clock);
        self.images.insert(
            key.to_string(),
            Slot {
                image,
                target_px: 0,
            },
        );
    }

    /// Requests still in flight.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Keys the worker queue refused that are waiting for room.
    pub fn has_deferred(&self) -> bool {
        !self.deferred.is_empty()
    }

    /// Whether a key failed terminally this session.
    pub fn is_failed(&self, key: &str) -> bool {
        self.failed.contains_key(key)
    }

    /// Terminal failures since the last call, for the status line.
    pub fn take_new_failures(&mut self) -> usize {
        std::mem::take(&mut self.new_failures)
    }

    /// Forget terminal failures so a later request can try again (offline→online).
    pub fn retry_failed(&mut self) {
        self.failed.clear();
        self.failed_order.clear();
        self.retries.clear();
    }

    pub fn cache_stats(&self) -> Option<CacheStats> {
        self.manager
            .as_ref()
            .and_then(|manager| manager.cache_stats())
    }

    /// Whether `ETEREA_MEMORY_LOG=1` is on, so the shell can keep waking.
    pub fn memory_logging(&self) -> bool {
        memory_log_enabled()
    }

    /// Clear the disk cache and forget every in-memory slot, so a cleared
    /// cache can repopulate from the network instead of keeping stale failures.
    pub fn clear_cache(&mut self) -> bool {
        if let Some(manager) = self.manager.as_ref() {
            let _ = manager.cancel_all();
        }
        let cleared = self
            .manager
            .as_ref()
            .is_some_and(|manager| manager.clear_cache().unwrap_or(false));
        self.images.clear();
        self.budget.clear();
        self.clock = self.clock.wrapping_add(1);
        self.pending.clear();
        self.failed.clear();
        self.failed_order.clear();
        self.retries.clear();
        self.deferred.clear();
        self.new_failures = 0;
        self.evictions = 0;
        cleared
    }

    /// Queue one HTTPS image decoded for a display size.
    ///
    /// `target` is the longest edge in device pixels; `None` asks for the
    /// original bytes, which the shell's decoder renders at full resolution
    /// (used when the image can be zoomed past the display size). A request
    /// that needs a larger size than a pending or resident one replaces it;
    /// a smaller one is ignored.
    pub fn request_display(&mut self, key: &str, kind: ImageKind, target: Option<u32>) {
        let wanted = target.unwrap_or(0);
        // A resident image already at a sufficient size needs no request; this
        // is also how preloaded local demo assets are served.
        if let Some(slot) = self.images.get(key) {
            if target_covers(slot.target_px, wanted) {
                return;
            }
        }
        if self.manager.is_none() || !eterea_logic::is_safe_https_url(key) {
            self.mark_failed(key);
            return;
        }
        if self.failed.contains_key(key) {
            return;
        }
        if let Some(deferred) = self.deferred.iter_mut().find(|queued| queued.key == key) {
            if !target_covers(deferred.target, wanted) {
                deferred.target = wanted;
                deferred.kind = kind;
            }
            return;
        }
        if let Some((&id, pending)) = self.pending.iter().find(|(_, queued)| queued.key == key) {
            if target_covers(pending.target, wanted) {
                return;
            }
            // A bigger display size arrived while the smaller decode was in
            // flight: drop it and fetch the size the view needs now.
            if let Some(manager) = self.manager.as_ref() {
                let _ = manager.cancel(id);
            }
            self.pending.remove(&id);
        }
        self.enqueue_or_defer(key, kind, wanted);
    }

    /// Mark `key` as needed by the current view without requesting it.
    ///
    /// Keeps a resident image warm for the LRU and lets a deferred or pending
    /// request know the view still wants it.
    pub fn touch(&mut self, key: &str) {
        self.clock = self.clock.wrapping_add(1);
        self.budget.touch(&key.to_string(), self.clock);
    }

    /// Enforce the byte budget, protecting the keys the current view needs.
    ///
    /// Returns the number of images evicted. Protected keys are never evicted,
    /// so a screen whose working set exceeds the budget keeps its images and
    /// the total temporarily exceeds [`Self::budget_limit`].
    pub fn evict_to_budget(&mut self, protected: &HashSet<String>) -> usize {
        self.clock = self.clock.wrapping_add(1);
        let clock = self.clock;
        for key in protected {
            self.budget.touch(key, clock);
        }
        let evicted = self
            .budget
            .evict_to_budget(self.budget_limit, |key| protected.contains(key));
        let count = evicted.len();
        for key in evicted {
            self.images.remove(&key);
            // A queued retry for an image the budget dropped is pointless, and
            // an in-flight one would only be evicted again on arrival.
            self.deferred.retain(|queued| queued.key != key);
            let ids: Vec<u64> = self
                .pending
                .iter()
                .filter_map(|(id, queued)| (queued.key == key).then_some(*id))
                .collect();
            for id in ids {
                if let Some(manager) = self.manager.as_ref() {
                    let _ = manager.cancel(id);
                }
                self.pending.remove(&id);
            }
        }
        self.evictions = self.evictions.saturating_add(count as u64);
        self.log_memory();
        count
    }

    /// Request set keys and decode completed events. Returns true when a
    /// visible image became available or failed.
    pub fn poll(&mut self) -> bool {
        if self.manager.is_none() {
            self.log_memory();
            return false;
        }
        let mut changed = false;
        while let Some(event) = self.manager.as_ref().and_then(|manager| manager.try_recv()) {
            match event {
                MediaEvent::ImageReady {
                    id,
                    kind: _,
                    source: _,
                    payload,
                } => {
                    let Some(queued) = self.pending.remove(&id.value()) else {
                        // Cancelled while queued: the late payload is dropped.
                        continue;
                    };
                    self.install(&queued.key, queued.target, payload);
                    changed = true;
                }
                MediaEvent::ImageFailed { id, kind, error } => {
                    let Some(queued) = self.pending.remove(&id.value()) else {
                        continue;
                    };
                    let attempts = self.retries.entry(queued.key.clone()).or_insert(0);
                    // A timeout, a dropped connection, a 429 or a 5xx is
                    // worth two more tries; anything else is terminal for
                    // this session (a bad URL, an unsafe host, a 404).
                    if is_transient(&error) && *attempts < 2 {
                        *attempts += 1;
                        self.deferred.push(DeferredRequest {
                            key: queued.key,
                            kind: queued.kind,
                            target: queued.target,
                        });
                    } else {
                        self.mark_failed(&queued.key);
                    }
                    let _ = kind;
                    changed = true;
                }
            }
        }
        // The request queue drained while events were handled: give deferred
        // keys another try before waiting for the next poll.
        while let Some(deferred) = self.deferred.first() {
            let key = deferred.key.clone();
            let kind = deferred.kind;
            let target = deferred.target;
            let Some(manager) = self.manager.as_ref() else {
                break;
            };
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1).max(1);
            let request = eterea_media::ImageRequest::new(id, key.clone(), kind)
                .with_optional_target((target != 0).then_some(target));
            match manager.enqueue_image(request) {
                Ok(()) => {
                    self.deferred.remove(0);
                    self.pending
                        .insert(id, PendingRequest { key, kind, target });
                }
                Err(MediaError::QueueFull) => break,
                Err(_) => {
                    self.deferred.remove(0);
                    self.mark_failed(&key);
                }
            }
        }
        self.log_memory();
        changed
    }

    fn enqueue_or_defer(&mut self, key: &str, kind: ImageKind, target: u32) {
        let Some(manager) = self.manager.as_ref() else {
            self.mark_failed(key);
            return;
        };
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let request = eterea_media::ImageRequest::new(id, key.to_string(), kind)
            .with_optional_target((target != 0).then_some(target));
        match manager.enqueue_image(request) {
            Ok(()) => {
                self.pending.insert(
                    id,
                    PendingRequest {
                        key: key.to_string(),
                        kind,
                        target,
                    },
                );
            }
            // Retryable: the workers hold a bounded queue. Keep the key and
            // flush it later; only terminal errors belong in `failed`.
            Err(MediaError::QueueFull) => self.deferred.push(DeferredRequest {
                key: key.to_string(),
                kind,
                target,
            }),
            Err(_) => self.mark_failed(key),
        }
    }

    fn install(&mut self, key: &str, target: u32, payload: ImagePayload) {
        let Some(cache) = self.cache.as_ref() else {
            return;
        };
        let weight = payload_weight(&payload);
        let image = match payload {
            ImagePayload::Rgba {
                width,
                height,
                bytes,
            } => cache.rgba(width, height, bytes),
            ImagePayload::Encoded { bytes, .. } => cache.load(bytes),
        };
        if let Some(slot) = self.images.remove(key) {
            drop(slot);
        }
        self.clock = self.clock.wrapping_add(1);
        self.budget.insert(key.to_string(), weight, self.clock);
        self.images.insert(
            key.to_string(),
            Slot {
                image,
                target_px: target,
            },
        );
    }

    fn mark_failed(&mut self, key: &str) {
        if self.failed.insert(key.to_string(), ()).is_none() {
            self.failed_order.push_back(key.to_string());
            self.new_failures = self.new_failures.saturating_add(1);
        }
        while self.failed_order.len() > MAX_FAILED_KEYS {
            if let Some(oldest) = self.failed_order.pop_front() {
                self.failed.remove(&oldest);
            }
        }
    }

    fn log_memory(&mut self) {
        if !memory_log_enabled() {
            return;
        }
        let now = Instant::now();
        let due = self
            .last_memory_log
            .is_none_or(|last| now.saturating_duration_since(last) >= MEMORY_LOG_INTERVAL);
        if !due {
            return;
        }
        self.last_memory_log = Some(now);
        eprintln!(
            "eterea: memory image_cache_bytes={} image_cache_slots={} image_cache_pending={} image_cache_evictions={}",
            self.budget.total_bytes(),
            self.images.len(),
            self.pending.len(),
            self.evictions,
        );
    }
}

fn is_transient(error: &MediaError) -> bool {
    match error {
        MediaError::Timeout | MediaError::Connection => true,
        MediaError::HttpStatus(status) => *status == 429 || *status >= 500,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{mip_weight, target_covers};

    #[test]
    fn a_target_covers_itself_and_smaller_sizes() {
        assert!(target_covers(960, 960));
        assert!(target_covers(960, 720));
        assert!(!target_covers(960, 1440));
    }

    #[test]
    fn the_original_covers_every_display_size() {
        assert!(target_covers(0, 0));
        assert!(target_covers(0, 720));
        assert!(target_covers(0, 4000));
        assert!(
            !target_covers(720, 0),
            "a display decode cannot serve a zoom"
        );
    }

    #[test]
    fn weight_includes_the_mip_chain() {
        // 1000x500 RGBA is 2,000,000 bytes; the mip chain adds a third.
        assert_eq!(mip_weight(1000, 500), 2_666_666);
        assert_eq!(mip_weight(0, 500), 0);
    }
}
