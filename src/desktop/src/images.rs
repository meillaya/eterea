//! Image slots and the media worker bridge.
//!
//! The view asks for an image by key. A local demo key is preloaded; an HTTPS
//! key is queued on the media workers and appears when its event arrives.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use eterea_media::{CacheStats, DiskCacheConfig};

use eterea_media::{ImageKind, MediaError, MediaEvent, MediaManager, MediaManagerConfig};
use rune::{Image, ImageCache};

pub struct ImageStore {
    cache: Option<ImageCache>,
    manager: Option<MediaManager>,
    images: HashMap<String, Image>,
    pending: HashMap<u64, String>,
    requested: HashSet<String>,
    failed: HashSet<String>,
    /// Keys the bounded worker queue refused; re-driven from `poll` once room
    /// frees up instead of being marked failed for the session.
    deferred: Vec<(String, ImageKind)>,
    next_id: u64,
}

impl ImageStore {
    /// A placeholder store used during `State::new`, before the Rune waker
    /// exists. `setup` replaces it with [`Self::new`].
    pub fn empty() -> Self {
        Self {
            cache: None,
            manager: None,
            images: HashMap::new(),
            pending: HashMap::new(),
            requested: HashSet::new(),
            failed: HashSet::new(),
            deferred: Vec::new(),
            next_id: 1,
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
            pending: HashMap::new(),
            requested: HashSet::new(),
            failed: HashSet::new(),
            deferred: Vec::new(),
            next_id: 1,
        }
    }

    pub fn resolve(&self, key: &str) -> Option<&Image> {
        self.images.get(key)
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
        self.images.insert(key.to_string(), image);
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
        self.failed.contains(key)
    }

    pub fn cache_stats(&self) -> Option<CacheStats> {
        self.manager
            .as_ref()
            .and_then(|manager| manager.cache_stats())
    }

    /// Clear the disk cache and forget every in-memory slot, so a cleared
    /// cache can repopulate from the network instead of keeping stale failures.
    pub fn clear_cache(&mut self) -> bool {
        let cleared = self
            .manager
            .as_ref()
            .is_some_and(|manager| manager.clear_cache().unwrap_or(false));
        self.images.clear();
        self.pending.clear();
        self.requested.clear();
        self.failed.clear();
        self.deferred.clear();
        cleared
    }

    /// Queue one HTTPS image if it has not been requested.
    pub fn request(&mut self, key: &str, kind: ImageKind) {
        if self.images.contains_key(key)
            || self.failed.contains(key)
            || self.requested.contains(key)
            || self.deferred.iter().any(|(queued, _)| queued == key)
        {
            return;
        }
        if self.manager.is_none() {
            self.failed.insert(key.to_string());
            return;
        }
        if !eterea_logic::is_safe_https_url(key) {
            self.failed.insert(key.to_string());
            return;
        }
        self.enqueue_or_defer(key, kind);
    }

    fn enqueue_or_defer(&mut self, key: &str, kind: ImageKind) {
        let Some(manager) = self.manager.as_ref() else {
            self.failed.insert(key.to_string());
            return;
        };
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        match manager.enqueue(id, key, kind) {
            Ok(()) => {
                self.pending.insert(id, key.to_string());
                self.requested.insert(key.to_string());
            }
            // Retryable: the workers hold a bounded queue. Keep the key and
            // flush it later; only terminal errors belong in `failed`.
            Err(MediaError::QueueFull) => self.deferred.push((key.to_string(), kind)),
            Err(_) => {
                self.failed.insert(key.to_string());
            }
        }
    }

    /// Request a set of keys and decode completed events. Returns true when a
    /// visible image became available or failed.
    pub fn poll(&mut self) -> bool {
        if self.manager.is_none() {
            return false;
        }
        let mut changed = false;
        while let Some(event) = self.manager.as_ref().and_then(|manager| manager.try_recv()) {
            match event {
                MediaEvent::ImageReady { id, bytes, .. } => {
                    if let Some(key) = self.pending.remove(&id.value()) {
                        if let Some(cache) = self.cache.as_ref() {
                            let image = cache.load(bytes);
                            self.images.insert(key, image);
                            changed = true;
                        }
                    }
                }
                MediaEvent::ImageFailed { id, .. } => {
                    if let Some(key) = self.pending.remove(&id.value()) {
                        self.failed.insert(key);
                        changed = true;
                    }
                }
            }
        }
        // The request queue drained while events were handled: give deferred
        // keys another try before waiting for the next poll.
        while let Some((key, kind)) = self.deferred.first().cloned() {
            let Some(manager) = self.manager.as_ref() else {
                break;
            };
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1).max(1);
            match manager.enqueue(id, &key, kind) {
                Ok(()) => {
                    self.deferred.remove(0);
                    self.pending.insert(id, key.clone());
                    self.requested.insert(key);
                }
                Err(MediaError::QueueFull) => break,
                Err(_) => {
                    self.deferred.remove(0);
                }
            }
        }
        changed
    }
}
