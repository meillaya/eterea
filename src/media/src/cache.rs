//! Opt-in, bounded, SHA-256-addressed disk cache for fetched media bytes.
//!
//! The cache is off unless a caller supplies a [`DiskCacheConfig`]. Files are
//! stored under `<caller dir>/media/` so [`DiskCache::clear`] can never delete
//! unrelated files: it only empties the directory this cache owns.
//!
//! Each entry is one file named after the SHA-256 digest of
//! `"version" || kind || NUL || url`, so no request URL, account handle or
//! other imported text appears in a file name. The file itself starts with a
//! short header (magic, key digest, payload length, payload digest) followed by
//! the raw image bytes; the payload digest makes the stored content
//! tamper-evident. A corrupt, truncated or mismatched entry is discarded on
//! read instead of being served.
//!
//! The cache never stores more than [`DiskCacheConfig::max_bytes`] in total.
//! When an insert would exceed the cap, the oldest modified entries are
//! removed first. Entries larger than the cap are not cached.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

use crate::digest::{hex_encode, sha256, Sha256};
use crate::request::ImageKind;

/// Default total cache cap: 256 MiB.
pub const DEFAULT_CACHE_BYTES: u64 = 256 * 1024 * 1024;

const MAGIC: &[u8; 8] = b"ETMEDIA1";
const KEY_LEN: usize = 32;
const HEADER_LEN: usize = 8 + KEY_LEN + 8 + 32;
const CACHE_VERSION_TAG: &[u8] = b"eterea-media-cache-v1\0";
const ENTRY_EXTENSION: &str = "media";

/// Configuration of the opt-in disk cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskCacheConfig {
    /// Caller-supplied base directory. The cache owns the `media/` subdirectory
    /// of this path and never touches anything outside it.
    pub dir: PathBuf,
    /// Total byte budget for cached entries. Must be greater than zero.
    pub max_bytes: u64,
}

impl DiskCacheConfig {
    /// Enable the cache under `dir` with [`DEFAULT_CACHE_BYTES`] of budget.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            max_bytes: DEFAULT_CACHE_BYTES,
        }
    }

    /// Override the total byte budget.
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }
}

/// Stable cache identity for one requested image.
///
/// Keys are digests, never plain URLs, so file names cannot leak imported
/// text. The same kind and URL always produce the same key across processes
/// and Rust versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey([u8; 32]);

impl CacheKey {
    /// Compute the key for an image request.
    pub fn for_request(kind: ImageKind, url: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(CACHE_VERSION_TAG);
        hasher.update(kind.as_str().as_bytes());
        hasher.update(&[0]);
        hasher.update(url.as_bytes());
        Self(hasher.finish())
    }

    /// Lowercase hex form, used for the cache file name.
    pub fn to_hex(self) -> String {
        hex_encode(&self.0)
    }

    /// Raw digest bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Current cache occupancy and budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheStats {
    /// Total bytes occupied by cache entries, including entry headers.
    pub bytes: u64,
    /// Number of cached entries.
    pub entries: usize,
    /// Configured total byte budget.
    pub max_bytes: u64,
}

#[derive(Debug, Clone)]
struct CacheEntryFile {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
}

#[derive(Debug, Default)]
struct CacheInner {
    total_bytes: u64,
    entries: usize,
}

/// Bounded, content-addressed media byte cache.
pub struct DiskCache {
    root: PathBuf,
    max_bytes: u64,
    /// Serializes entry mutations so occupancy accounting cannot interleave
    /// with a concurrent insert, eviction, or corruption cleanup.
    mutation: Mutex<()>,
    inner: Mutex<CacheInner>,
}

impl std::fmt::Debug for DiskCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskCache")
            .field("root", &self.root)
            .field("max_bytes", &self.max_bytes)
            .finish_non_exhaustive()
    }
}

impl DiskCache {
    /// Open or create the cache under `config.dir/media`.
    ///
    /// Scans existing entries to account for them, then evicts the oldest
    /// entries if the directory already exceeds the configured budget.
    pub fn open(config: DiskCacheConfig) -> std::io::Result<Self> {
        if config.max_bytes == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "disk cache max_bytes must be greater than zero",
            ));
        }
        let root = config.dir.join("media");
        std::fs::create_dir_all(&root)?;
        let mut total_bytes = 0_u64;
        let mut entries = 0_usize;
        for file in scan_entries(&root)? {
            total_bytes = total_bytes.saturating_add(file.len);
            entries += 1;
        }
        let cache = Self {
            root,
            max_bytes: config.max_bytes,
            mutation: Mutex::new(()),
            inner: Mutex::new(CacheInner {
                total_bytes,
                entries,
            }),
        };
        cache.evict_until_fits(None);
        Ok(cache)
    }

    /// Directory the cache owns. [`Self::clear`] only empties this directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Configured total byte budget.
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// Look up a cached image.
    ///
    /// Returns `None` for a miss. A corrupt, truncated or mismatched entry is
    /// removed from disk and also reported as a miss.
    pub fn get(&self, key: &CacheKey) -> Option<Vec<u8>> {
        let path = self.entry_path(key);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => return None,
        };
        if !metadata.is_file() || metadata.len() > self.max_bytes.saturating_add(HEADER_LEN as u64)
        {
            self.remove_entry(&path, metadata.len());
            return None;
        }
        let data = match std::fs::read(&path) {
            Ok(data) => data,
            Err(_) => {
                self.remove_entry(&path, metadata.len());
                return None;
            }
        };
        match decode_entry(&data, key) {
            Some(payload) => Some(payload),
            None => {
                self.remove_entry(&path, data.len() as u64);
                None
            }
        }
    }

    /// Insert image bytes. Returns `false` when the entry could not be cached
    /// (too large for the budget, or a filesystem error); caching is always
    /// best-effort and never fails a fetch.
    pub fn insert(&self, key: &CacheKey, bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return false;
        }
        let _mutation = self.mutation.lock().expect("cache mutation lock poisoned");
        let file_len = HEADER_LEN as u64 + bytes.len() as u64;
        if file_len > self.max_bytes {
            return false;
        }
        let encoded = encode_entry(key, bytes);
        let path = self.entry_path(key);
        let previous_len = std::fs::metadata(&path)
            .ok()
            .filter(|metadata| metadata.is_file())
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if !write_atomically(&self.root, &path, &encoded) {
            return false;
        }
        {
            let mut inner = self.inner.lock().expect("cache stats lock poisoned");
            inner.total_bytes = inner
                .total_bytes
                .saturating_sub(previous_len)
                .saturating_add(file_len);
            if previous_len == 0 {
                inner.entries += 1;
            }
        }
        self.evict_until_fits(Some(&path));
        true
    }

    /// Remove every entry from the cache, keeping the owned directory.
    pub fn clear(&self) -> std::io::Result<()> {
        let _mutation = self.mutation.lock().expect("cache mutation lock poisoned");
        let mut inner = self.inner.lock().expect("cache stats lock poisoned");
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }
        inner.total_bytes = 0;
        inner.entries = 0;
        Ok(())
    }

    /// Total bytes currently occupied by cache entries, including headers.
    pub fn byte_len(&self) -> u64 {
        self.inner
            .lock()
            .expect("cache stats lock poisoned")
            .total_bytes
    }

    /// Number of cached entries.
    pub fn entry_count(&self) -> usize {
        self.inner
            .lock()
            .expect("cache stats lock poisoned")
            .entries
    }

    /// Snapshot of occupancy and budget.
    pub fn stats(&self) -> CacheStats {
        let inner = self.inner.lock().expect("cache stats lock poisoned");
        CacheStats {
            bytes: inner.total_bytes,
            entries: inner.entries,
            max_bytes: self.max_bytes,
        }
    }

    fn entry_path(&self, key: &CacheKey) -> PathBuf {
        self.root
            .join(format!("{}.{ENTRY_EXTENSION}", key.to_hex()))
    }

    fn remove_entry(&self, path: &Path, _len: u64) {
        let _mutation = self.mutation.lock().expect("cache mutation lock poisoned");
        if std::fs::remove_file(path).is_ok() {
            // The on-disk file may have been tampered with, so its length can
            // differ from what was accounted. Recompute from the directory
            // instead of guessing.
            self.refresh_stats();
        }
    }

    /// Recompute occupancy from the owned directory. The caller must hold the
    /// mutation lock so a concurrent insert cannot be double-counted.
    fn refresh_stats(&self) {
        let Ok(files) = scan_entries(&self.root) else {
            return;
        };
        let mut inner = self.inner.lock().expect("cache stats lock poisoned");
        inner.total_bytes = files.iter().map(|file| file.len).sum();
        inner.entries = files.len();
    }

    /// Evict oldest entries until the total fits the budget.
    ///
    /// `keep` is the entry that was just written; it is never evicted because
    /// a single entry larger than the entire cache is rejected before insert.
    fn evict_until_fits(&self, keep: Option<&Path>) {
        loop {
            let total = self
                .inner
                .lock()
                .expect("cache stats lock poisoned")
                .total_bytes;
            if total <= self.max_bytes {
                return;
            }
            let mut files = match scan_entries(&self.root) {
                Ok(files) => files,
                Err(_) => return,
            };
            if let Some(keep) = keep {
                files.retain(|file| file.path != keep);
            }
            files.sort_by_key(|file| file.modified);
            let Some(oldest) = files.into_iter().next() else {
                return;
            };
            if std::fs::remove_file(&oldest.path).is_err() {
                return;
            }
            let mut inner = self.inner.lock().expect("cache stats lock poisoned");
            inner.total_bytes = inner.total_bytes.saturating_sub(oldest.len);
            inner.entries = inner.entries.saturating_sub(1);
        }
    }
}

fn encode_entry(key: &CacheKey, bytes: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(HEADER_LEN + bytes.len());
    encoded.extend_from_slice(MAGIC);
    encoded.extend_from_slice(&key.0);
    encoded.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    encoded.extend_from_slice(&sha256(bytes));
    encoded.extend_from_slice(bytes);
    encoded
}

fn decode_entry(data: &[u8], key: &CacheKey) -> Option<Vec<u8>> {
    if data.len() < HEADER_LEN {
        return None;
    }
    if &data[..8] != MAGIC {
        return None;
    }
    if data[8..8 + KEY_LEN] != key.0 {
        return None;
    }
    let length = u64::from_le_bytes(
        data[8 + KEY_LEN..8 + KEY_LEN + 8]
            .try_into()
            .expect("header slice is eight bytes"),
    );
    if length != (data.len() - HEADER_LEN) as u64 {
        return None;
    }
    let payload = &data[HEADER_LEN..];
    if payload.is_empty() {
        return None;
    }
    let payload_digest = sha256(payload);
    if data[8 + KEY_LEN + 8..HEADER_LEN] != payload_digest {
        return None;
    }
    Some(payload.to_vec())
}

fn scan_entries(root: &Path) -> std::io::Result<Vec<CacheEntryFile>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if !metadata.is_file()
            || path.extension().and_then(|value| value.to_str()) != Some(ENTRY_EXTENSION)
        {
            continue;
        }
        files.push(CacheEntryFile {
            path,
            len: metadata.len(),
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        });
    }
    Ok(files)
}

fn write_atomically(root: &Path, destination: &Path, bytes: &[u8]) -> bool {
    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = root.join(format!(
        ".tmp-{}-{}-{sequence}",
        std::process::id(),
        destination
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("entry")
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.flush()?;
        drop(file);
        // Rename is atomic on Unix. On platforms where rename refuses to
        // replace an existing file, remove it and retry once.
        if std::fs::rename(&temp, destination).is_err() {
            let _ = std::fs::remove_file(destination);
            std::fs::rename(&temp, destination)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{CacheKey, DiskCache, DiskCacheConfig, HEADER_LEN, MAGIC};
    use crate::request::ImageKind;
    use crate::test_support::ScratchDir;

    fn cache(scratch: &ScratchDir, max_bytes: u64) -> DiskCache {
        DiskCache::open(DiskCacheConfig::new(scratch.path()).with_max_bytes(max_bytes))
            .expect("open cache")
    }

    fn key(suffix: &str) -> CacheKey {
        CacheKey::for_request(ImageKind::Image, &format!("https://x.com/{suffix}.jpg"))
    }

    #[test]
    fn disabled_by_default_is_a_manager_concern_but_cache_is_explicit() {
        // Opening requires a caller-supplied directory; there is no implicit
        // global cache path anywhere in the crate.
        assert!(
            DiskCache::open(DiskCacheConfig::new("/nonexistent-invalid").with_max_bytes(0))
                .is_err()
        );
    }

    #[test]
    fn miss_then_round_trip() {
        let scratch = ScratchDir::new("cache-round-trip");
        let cache = cache(&scratch, 1024 * 1024);
        let key = key("round-trip");
        assert_eq!(cache.get(&key), None);
        assert_eq!(cache.entry_count(), 0);

        let payload = b"fake jpeg bytes".to_vec();
        assert!(cache.insert(&key, &payload));
        assert_eq!(cache.get(&key), Some(payload.clone()));
        assert_eq!(cache.entry_count(), 1);
        assert_eq!(cache.byte_len(), HEADER_LEN as u64 + payload.len() as u64);
        assert_eq!(cache.stats().max_bytes, 1024 * 1024);
    }

    #[test]
    fn file_names_never_contain_the_url() {
        let scratch = ScratchDir::new("cache-names");
        let cache = cache(&scratch, 1024 * 1024);
        let key = key("secret");
        cache.insert(&key, b"bytes");
        let name = std::fs::read_dir(cache.root())
            .expect("read cache dir")
            .next()
            .expect("one entry")
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        assert_eq!(name, format!("{}.media", key.to_hex()));
        assert!(!name.contains("https"));
        assert!(!name.contains("secret"));
        assert_eq!(name.len(), 64 + ".media".len());
    }

    #[test]
    fn entries_larger_than_budget_are_not_cached() {
        let scratch = ScratchDir::new("cache-oversize");
        let cache = cache(&scratch, 200);
        let key = key("large");
        assert!(!cache.insert(&key, &[0xab; 200]));
        assert_eq!(cache.entry_count(), 0);
        assert_eq!(cache.byte_len(), 0);
    }

    #[test]
    fn budget_is_enforced_by_evicting_oldest_entries() {
        let scratch = ScratchDir::new("cache-cap");
        let cache = cache(&scratch, 1_000);
        let first = key("first");
        let second = key("second");
        assert!(cache.insert(&first, &[1_u8; 600]));
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(cache.insert(&second, &[2_u8; 600]));
        assert!(
            cache.byte_len() <= cache.max_bytes(),
            "cache must stay within budget"
        );
        assert_eq!(cache.entry_count(), 1);
        assert_eq!(cache.byte_len(), HEADER_LEN as u64 + 600);
        // The surviving entry is readable and self-consistent.
        let survivors = [first, second]
            .into_iter()
            .filter(|key| cache.get(key).is_some())
            .count();
        assert_eq!(survivors, 1);
    }

    #[test]
    fn clear_empties_the_owned_directory() {
        let scratch = ScratchDir::new("cache-clear");
        let cache = cache(&scratch, 1024 * 1024);
        cache.insert(&key("a"), b"a");
        cache.insert(&key("b"), b"b");
        assert_eq!(cache.entry_count(), 2);
        cache.clear().expect("clear cache");
        assert_eq!(cache.entry_count(), 0);
        assert_eq!(cache.byte_len(), 0);
        assert!(cache.root().is_dir());
        assert_eq!(
            std::fs::read_dir(cache.root())
                .expect("read cache dir")
                .count(),
            0
        );
        assert_eq!(cache.get(&key("a")), None);
    }

    #[test]
    fn cached_entries_survive_a_reopen() {
        let scratch = ScratchDir::new("cache-reopen");
        let key = key("persisted");
        {
            let cache = cache(&scratch, 1024 * 1024);
            assert!(cache.insert(&key, b"persist me"));
        }
        let cache = cache(&scratch, 1024 * 1024);
        assert_eq!(cache.entry_count(), 1);
        assert_eq!(cache.get(&key), Some(b"persist me".to_vec()));
    }

    #[test]
    fn corrupt_or_truncated_entries_are_discarded() {
        let scratch = ScratchDir::new("cache-corrupt");
        let cache = cache(&scratch, 1024 * 1024);
        let key = key("corrupt");
        assert!(cache.insert(&key, b"valid"));
        let path = cache.root().join(format!("{}.media", key.to_hex()));

        std::fs::write(&path, b"not a cache entry").expect("corrupt file");
        assert_eq!(cache.get(&key), None);
        assert!(!path.exists(), "corrupt entry must be removed");
        assert_eq!(cache.entry_count(), 0);
        assert_eq!(cache.byte_len(), 0);

        // A valid header with a mutated payload is also rejected.
        let mut encoded = super::encode_entry(&key, b"payload");
        encoded[HEADER_LEN] ^= 0xff;
        std::fs::write(&path, &encoded).expect("write mutated entry");
        assert_eq!(cache.get(&key), None);
        assert!(!path.exists());
    }

    #[test]
    fn wrong_key_is_discarded() {
        let scratch = ScratchDir::new("cache-wrong-key");
        let cache = cache(&scratch, 1024 * 1024);
        let stored = key("stored");
        let other = key("other");
        assert!(cache.insert(&stored, b"stored"));
        // Move the file to the other key's name: the key digest in the header
        // no longer matches and the entry must not be served.
        let from = cache.root().join(format!("{}.media", stored.to_hex()));
        let to = cache.root().join(format!("{}.media", other.to_hex()));
        std::fs::rename(from, to).expect("rename entry");
        assert_eq!(cache.get(&other), None);
    }

    #[test]
    fn unknown_files_are_ignored_but_clear_removes_them() {
        let scratch = ScratchDir::new("cache-strays");
        let cache = cache(&scratch, 1024 * 1024);
        std::fs::write(cache.root().join("stray.txt"), b"x").expect("stray file");
        cache.insert(&key("kept"), b"bytes");
        assert_eq!(cache.entry_count(), 1);
        cache.clear().expect("clear");
        assert_eq!(cache.entry_count(), 0);
        assert!(!cache.root().join("stray.txt").exists());
    }

    #[test]
    fn zero_budget_is_rejected() {
        let scratch = ScratchDir::new("cache-zero");
        assert!(DiskCache::open(DiskCacheConfig::new(scratch.path()).with_max_bytes(0)).is_err());
    }

    #[test]
    fn magic_is_checked() {
        let mut encoded = super::encode_entry(&key("magic"), b"x");
        encoded[..MAGIC.len()].copy_from_slice(b"BADMAGIC");
        assert_eq!(super::decode_entry(&encoded, &key("magic")), None);
    }
    #[test]
    fn empty_payloads_are_not_cached() {
        let scratch = ScratchDir::new("cache-empty");
        let cache = cache(&scratch, 1024 * 1024);
        assert!(!cache.insert(&key("empty"), b""));
        assert_eq!(cache.entry_count(), 0);
        assert_eq!(cache.byte_len(), 0);
    }

    #[test]
    fn reopening_with_a_smaller_budget_evicts_oldest_entries() {
        let scratch = ScratchDir::new("cache-shrink");
        let first = key("shrink-first");
        let second = key("shrink-second");
        {
            let cache = cache(&scratch, 1024 * 1024);
            assert!(cache.insert(&first, &[1_u8; 100]));
            std::thread::sleep(std::time::Duration::from_millis(5));
            assert!(cache.insert(&second, &[2_u8; 100]));
        }
        // Two entries are 2 * (HEADER_LEN + 100); a 200 byte budget fits one.
        let cache = cache(&scratch, 200);
        assert_eq!(cache.entry_count(), 1);
        assert!(cache.byte_len() <= cache.max_bytes());
        assert!(cache.byte_len() > 0);
    }
}
