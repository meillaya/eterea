//! Public API smoke tests for `eterea-media`.
//!
//! These tests exercise the crate from outside, like the desktop shell will.
//! They never touch the network: everything either fails policy or uses the
//! opt-in disk cache. Scratch directories live under the cargo target
//! directory, never outside this workspace.

use std::path::{Path, PathBuf};
use std::time::Duration;

use eterea_media::{
    is_allowed_media_url, CacheKey, DiskCache, DiskCacheConfig, ImageKind, ImageSource, MediaError,
    MediaManager, MediaManagerConfig, RequestId, VideoJob, DEFAULT_CACHE_BYTES,
    DEFAULT_VIDEO_BYTES, HTTP_TIMEOUT, MAX_RESPONSE_BYTES, USER_AGENT,
};

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let base = std::env::var_os("CARGO_TARGET_TMPDIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("CARGO_TARGET_DIR").map(|dir| PathBuf::from(dir).join("tmp"))
            })
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.work/media-test-scratch")
            });
        let path = base.join(format!("public-api-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&path).expect("scratch dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn constants_match_the_documented_policy() {
    assert_eq!(MAX_RESPONSE_BYTES, 32 * 1024 * 1024);
    assert_eq!(HTTP_TIMEOUT, Duration::from_secs(20));
    assert_eq!(DEFAULT_CACHE_BYTES, 256 * 1024 * 1024);
    assert_eq!(DEFAULT_VIDEO_BYTES, 512 * 1024 * 1024);
    assert_eq!(USER_AGENT, "eterea/0.1.0");
}

#[test]
fn url_policy_is_public_and_strict() {
    assert!(is_allowed_media_url("https://pbs.twimg.com/media/x.jpg"));
    assert!(!is_allowed_media_url("http://pbs.twimg.com/media/x.jpg"));
    assert!(!is_allowed_media_url("https://127.0.0.1/x.jpg"));
    assert!(!is_allowed_media_url("https://user:pass@x.com/x.jpg"));
    assert!(!is_allowed_media_url("https://x.com/x.jpg#frag"));
}

#[test]
fn manager_defaults_are_opt_in_for_the_cache() {
    let manager = MediaManager::new(MediaManagerConfig {
        worker_count: 1,
        ..MediaManagerConfig::default()
    })
    .expect("manager starts without a cache");
    assert_eq!(manager.worker_count(), 1);
    assert_eq!(manager.cache_stats(), None);
    assert_eq!(manager.clear_cache(), Ok(false));
    assert_eq!(manager.try_recv(), None);
    assert_eq!(manager.pending(), 0);
    assert!(!manager.cancel(RequestId::new(1)));
}

#[test]
fn manager_rejects_unsafe_requests_without_network() {
    let dir = Scratch::new("manager-unsafe");
    let manager = MediaManager::new(MediaManagerConfig {
        worker_count: 1,
        queue_bound: 2,
        cache: Some(DiskCacheConfig::new(dir.path())),
        ..MediaManagerConfig::default()
    })
    .expect("manager");
    assert_eq!(
        manager.enqueue(
            RequestId::new(1),
            "http://pbs.twimg.com/media/x.jpg",
            ImageKind::Image
        ),
        Err(MediaError::UnsafeUrl)
    );
    assert_eq!(manager.pending(), 0);
    assert_eq!(manager.try_recv(), None);
}

#[test]
fn disk_cache_round_trips_from_outside_the_crate() {
    let dir = Scratch::new("cache");
    let config = DiskCacheConfig::new(dir.path()).with_max_bytes(64 * 1024);
    let cache = DiskCache::open(config).expect("open cache");
    let key = CacheKey::for_request(ImageKind::Poster, "https://pbs.twimg.com/media/x.jpg");
    assert_eq!(cache.get(&key), None);
    assert!(cache.insert(&key, b"poster bytes"));
    assert_eq!(cache.get(&key), Some(b"poster bytes".to_vec()));
    assert_eq!(cache.entry_count(), 1);
    assert!(cache.byte_len() > 0);
    cache.clear().expect("clear");
    assert_eq!(cache.entry_count(), 0);
    assert!(cache.insert(&key, b"poster bytes"));
    let config = DiskCacheConfig::new(dir.path()).with_max_bytes(64 * 1024);
    drop(cache);
    let reopened = DiskCache::open(config).expect("reopen");
    assert_eq!(reopened.get(&key), Some(b"poster bytes".to_vec()));
}

#[test]
fn video_jobs_validate_before_any_io() {
    let job = VideoJob::new("https://video.twimg.com/x.mp4", "video.mp4").expect("safe url");
    assert_eq!(job.max_bytes(), DEFAULT_VIDEO_BYTES);
    assert_eq!(job.url(), "https://video.twimg.com/x.mp4");
    assert_eq!(job.destination(), Path::new("video.mp4"));
    assert_eq!(
        VideoJob::new("https://127.0.0.1/x.mp4", "video.mp4").unwrap_err(),
        MediaError::UnsafeUrl
    );
}

#[test]
fn image_event_debug_is_compact() {
    // Type-level checks: the event and source variants the shell matches on.
    let source = ImageSource::Network;
    assert_ne!(source, ImageSource::DiskCache);
    let request = eterea_media::ImageRequest::new(1_u64, "https://x.com/a.jpg", ImageKind::Avatar);
    assert_eq!(request.id, RequestId(1));
}

// ---------------------------------------------------------------------------
// Media v2 public surface: tools discovery, persistent video cache, playback.
// Everything here is network-free: policy failures and cancellation happen
// before a socket would be opened, and cache scratch dirs stay in the target.
// ---------------------------------------------------------------------------

use eterea_media::{
    CachedVideo, MediaTools, PlaybackState, ToolStatus, VideoCache, VideoCacheConfig,
    VideoCacheStats, VideoPlayerOptions, PLAYBACK_FPS,
};
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn tool_discovery_is_typed_and_never_panics() {
    let tools = MediaTools::discover();
    assert_eq!(tools.ffmpeg_available(), tools.ffmpeg.path.is_some());
    assert_eq!(tools.ffplay_available(), tools.ffplay.path.is_some());
    if !tools.ffmpeg_available() {
        assert!(tools
            .ffmpeg
            .message
            .as_deref()
            .unwrap()
            .contains("install ffmpeg"));
    }
    let missing = MediaTools::from_paths("/definitely/not/ffmpeg", None);
    assert!(!missing.ffmpeg_available());
    assert!(!missing.ffplay_available());
    assert_eq!(
        missing.ffmpeg(),
        Err(eterea_media::MediaError::DecoderUnavailable)
    );
    assert_eq!(
        missing.ffplay(),
        Err(eterea_media::MediaError::AudioUnavailable)
    );
    assert!(missing.ffmpeg.describe().contains("ffmpeg"));
    assert_eq!(
        ToolStatus::found("/usr/bin/ffmpeg").describe(),
        "/usr/bin/ffmpeg"
    );
    assert!(!ToolStatus::missing("ffmpeg not found").available);
}

#[test]
fn media_errors_are_typed_and_actionable() {
    assert!(eterea_media::MediaError::DecoderUnavailable
        .to_string()
        .contains("install ffmpeg"));
    assert!(eterea_media::MediaError::AudioUnavailable
        .to_string()
        .contains("ffplay"));
    // Typed media-policy failures introduced with the MIME/decode hardening.
    assert_eq!(
        eterea_media::MediaError::UnsupportedType.to_string(),
        "unsupported media content type"
    );
    assert_eq!(
        eterea_media::MediaError::DecodeFailed.to_string(),
        "video decoder failed"
    );
}

#[test]
fn video_cache_public_surface_is_network_free() {
    let dir = Scratch::new("video-cache-public");
    let config = VideoCacheConfig::new(dir.path()).with_max_bytes(64 * 1024);
    let cache = VideoCache::open(config).expect("open video cache");
    assert_eq!(cache.max_bytes(), 64 * 1024);
    assert!(cache.dir().is_dir());
    let stats: VideoCacheStats = cache.stats();
    assert_eq!(stats.bytes, 0);
    assert_eq!(stats.entries, 0);
    assert_eq!(stats.max_bytes, 64 * 1024);

    let key = VideoCache::key_for("https://video.twimg.com/x.mp4");
    assert_eq!(key.len(), 64);
    assert!(key.chars().all(|character| character.is_ascii_hexdigit()));

    let url = "https://video.twimg.com/ext_tw_video/public-cancel.mp4";
    assert_eq!(cache.cached("http://video.twimg.com/x.mp4"), None);
    assert_eq!(
        cache.ensure("http://video.twimg.com/x.mp4", 1024),
        Err(eterea_media::MediaError::UnsafeUrl)
    );
    // A pre-cancelled request fails before any socket is opened.
    let cancel = AtomicBool::new(true);
    assert_eq!(
        cache.ensure_cancellable(url, 1024, &cancel),
        Err(eterea_media::MediaError::Cancelled)
    );
    assert_eq!(cache.stats().entries, 0);
    cache.clear().expect("clear empty cache");
    assert_eq!(cache.stats().bytes, 0);
}

#[test]
fn cached_video_type_exposes_path_and_source() {
    let video = CachedVideo {
        path: PathBuf::from("/tmp/example.media"),
        byte_len: 10,
        from_cache: true,
    };
    assert_eq!(video.byte_len, 10);
    assert!(video.from_cache);
    assert_eq!(video.path.extension().unwrap(), "media");
}

#[test]
fn player_options_defaults_are_safe() {
    let options = VideoPlayerOptions::new();
    assert!(options.muted);
    assert!(!options.looping);
    assert_eq!(options.volume, 100);
    assert!(options.cache.is_none());
    assert!(options.tools.is_none());
    assert!(options.probe_duration);
    assert_eq!(options.max_bytes, DEFAULT_VIDEO_BYTES);
    let options = options
        .with_volume(250)
        .with_muted(false)
        .with_looping(true);
    assert_eq!(options.volume, 100);
    assert!(!options.muted);
    assert!(options.looping);
}

#[test]
fn playback_state_is_constructible_from_the_public_api() {
    let state = PlaybackState {
        paused: false,
        muted: true,
        volume: 100,
        looping: false,
        ended: false,
        error: None,
        error_kind: None,
        position: Duration::ZERO,
        duration: Some(Duration::from_secs(1)),
        from_cache: Some(true),
        loop_count: 0,
        audio_active: false,
        audio_error: None,
    };
    assert_eq!(state.duration, Some(Duration::from_secs(1)));
    assert_eq!(PLAYBACK_FPS, 30);
    assert_eq!(eterea_media::PLAYBACK_WIDTH, 960);
    assert_eq!(eterea_media::PLAYBACK_HEIGHT, 540);
}

#[test]
fn player_rejects_unsafe_urls_and_missing_local_files() {
    assert_eq!(
        eterea_media::VideoPlayer::start("http://video.twimg.com/x.mp4").err(),
        Some(eterea_media::MediaError::UnsafeUrl)
    );
    let options = VideoPlayerOptions::new();
    assert_eq!(
        eterea_media::VideoPlayer::start_local_file("/definitely/not/a/file.mp4", options).err(),
        Some(eterea_media::MediaError::Io)
    );
}

#[test]
fn video_job_cancellation_precedes_network_access() {
    let dir = Scratch::new("video-job-cancel");
    let job = VideoJob::new(
        "https://video.twimg.com/x.mp4",
        dir.path().join("cancel.mp4"),
    )
    .expect("safe url");
    let cancel = AtomicBool::new(true);
    assert_eq!(
        job.download_cancellable(&cancel),
        Err(eterea_media::MediaError::Cancelled)
    );
    assert!(!dir.path().join("cancel.mp4").exists());
}

#[test]
fn video_cache_constants_and_zero_budget_are_explicit() {
    assert_eq!(
        eterea_media::DEFAULT_VIDEO_CACHE_BYTES,
        2 * 1024 * 1024 * 1024
    );
    let dir = Scratch::new("video-cache-zero");
    assert!(VideoCache::open(VideoCacheConfig::new(dir.path()).with_max_bytes(0)).is_err());
    let cancel = AtomicBool::new(false);
    assert!(!cancel.load(Ordering::Relaxed));
}
