//! Local video playback: ffmpeg frames plus ffplay audio.
//!
//! [`VideoPlayer`] never hands a remote URL to ffmpeg. A progressive MP4 is
//! first resolved into a local file through [`crate::VideoCache`] (or a
//! caller-supplied fallback directory when no cache is configured), and only
//! that local path is passed to the external processes.
//!
//! Frames are decoded by `ffmpeg` at a fixed 30 fps into RGBA
//! [`PLAYBACK_WIDTH`]×[`PLAYBACK_HEIGHT`] frames and delivered through a
//! bounded channel; the decoder thread paces itself, so a slow UI applies
//! backpressure instead of a dropped-frame storm. Sound is optional and muted
//! by default: when unmuted, `ffplay -nodisp -vn -autoexit` plays the same
//! local file in parallel. Pause, mute, loop, stop and volume changes are
//! handled without ever letting a child process outlive the player.
//!
//! A decoder that exits non-zero or produces no frames is a failure, not an
//! end of stream: the player reports [`MediaError::DecodeFailed`] with a short
//! diagnostic and stops. It never respawns a failed decoder, even when looping
//! is enabled, so an undecodable file cannot spin ffmpeg forever.
//!
//! The legacy API ([`VideoPlayer::start`], [`VideoPlayer::poll`],
//! [`VideoPlayer::is_ended`], [`VideoPlayer::error`],
//! [`VideoPlayer::toggle_pause`], [`VideoPlayer::is_paused`],
//! [`VideoPlayer::stop`]) keeps working unchanged; the methods that take
//! [`VideoPlayerOptions`] add the persistent cache, looping and sound.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::error::MediaError;
use crate::tools::MediaTools;
use crate::url::is_allowed_media_url;
use crate::video::DEFAULT_VIDEO_BYTES;
use crate::video_cache::{VideoCache, VideoCacheConfig};

/// Playback frame width. Frames are letterboxed to this size and 16:9.
pub const PLAYBACK_WIDTH: u32 = 960;
/// Playback frame height.
pub const PLAYBACK_HEIGHT: u32 = 540;
/// Frame rate requested from ffmpeg and used for pacing.
pub const PLAYBACK_FPS: u32 = 30;

const FRAME_BYTES: usize = (PLAYBACK_WIDTH * PLAYBACK_HEIGHT * 4) as usize;
const FRAME_INTERVAL: Duration = Duration::from_micros(1_000_000 / PLAYBACK_FPS as u64);
const EVENT_QUEUE: usize = 3;
const PAUSE_POLL: Duration = Duration::from_millis(5);

/// One decoded RGBA frame in [`PLAYBACK_WIDTH`] × [`PLAYBACK_HEIGHT`] pixels.
#[derive(Clone)]
pub struct VideoFrame {
    /// Frame pixels, row-major RGBA, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

impl VideoFrame {
    /// Width in pixels.
    pub fn width(&self) -> u32 {
        PLAYBACK_WIDTH
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        PLAYBACK_HEIGHT
    }
}

/// Options for [`VideoPlayer`].
///
/// The defaults keep the legacy behavior of [`VideoPlayer::start`]: no
/// persistent cache, muted, not looping, full volume. Feed-style inline
/// playback (muted and looping) uses [`Self::with_looping`].
#[derive(Debug, Clone)]
pub struct VideoPlayerOptions {
    /// Optional persistent cache. `None` uses a private fallback directory
    /// under [`VideoPlayerOptions::scratch_dir`] (or the OS temp dir).
    pub cache: Option<Arc<VideoCache>>,
    /// Download byte cap applied when a URL must be fetched.
    pub max_bytes: u64,
    /// Start with sound off. Default `true`.
    pub muted: bool,
    /// Restart from the beginning when the file ends. Default `false`.
    pub looping: bool,
    /// ffplay volume, `0..=100`. Clamped. Default `100`.
    pub volume: u8,
    /// Discovered tools. `None` discovers at [`VideoPlayer::start_with_options`]
    /// time; tests and the desktop can inject explicit paths.
    pub tools: Option<MediaTools>,
    /// Directory used when [`Self::cache`] is `None`; the player creates a
    /// bounded cache in `<scratch_dir>/video`. `None` uses
    /// `<os temp>/eterea-media`.
    pub scratch_dir: Option<PathBuf>,
    /// Probe the file duration with ffmpeg before playing. Default `true`.
    pub probe_duration: bool,
}

impl Default for VideoPlayerOptions {
    fn default() -> Self {
        Self {
            cache: None,
            max_bytes: DEFAULT_VIDEO_BYTES,
            muted: true,
            looping: false,
            volume: 100,
            tools: None,
            scratch_dir: None,
            probe_duration: true,
        }
    }
}

impl VideoPlayerOptions {
    /// Start from the defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Use a persistent video cache.
    pub fn with_cache(mut self, cache: Arc<VideoCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Set the download byte cap.
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes.max(1);
        self
    }

    /// Start muted (`true`) or with sound enabled (`false`).
    pub fn with_muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }

    /// Loop the video.
    pub fn with_looping(mut self, looping: bool) -> Self {
        self.looping = looping;
        self
    }

    /// Set the ffplay volume, clamped to `0..=100`.
    pub fn with_volume(mut self, volume: u8) -> Self {
        self.volume = volume.min(100);
        self
    }

    /// Inject discovered (or fake) tools instead of searching `PATH`.
    pub fn with_tools(mut self, tools: MediaTools) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Set the fallback directory used when no cache is configured.
    pub fn with_scratch_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.scratch_dir = Some(dir.into());
        self
    }

    /// Enable or disable duration probing.
    pub fn with_probe_duration(mut self, probe: bool) -> Self {
        self.probe_duration = probe;
        self
    }
}

/// Snapshot of player state for the UI.
///
/// Call [`VideoPlayer::state`] after [`VideoPlayer::poll`] to refresh it; the
/// snapshot itself performs no I/O and never blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackState {
    /// Playback is paused.
    pub paused: bool,
    /// Sound is muted (the default).
    pub muted: bool,
    /// Current ffplay volume, `0..=100`.
    pub volume: u8,
    /// Playback loops at the end.
    pub looping: bool,
    /// Playback finished or failed.
    pub ended: bool,
    /// Decoder/download failure message, if any.
    pub error: Option<String>,
    /// Typed decoder/download failure, if any.
    ///
    /// The message above is the human-readable detail; this is the stable
    /// [`MediaError`] the UI can match on (for example
    /// [`MediaError::DecodeFailed`] for a decoder that produced no frames).
    pub error_kind: Option<MediaError>,
    /// Current position within the file, derived from delivered frames.
    pub position: Duration,
    /// File duration when ffmpeg could probe it.
    pub duration: Option<Duration>,
    /// Whether the file came from the persistent cache (`None` for local files).
    pub from_cache: Option<bool>,
    /// Number of completed loops.
    pub loop_count: u32,
    /// Whether ffplay is currently running.
    pub audio_active: bool,
    /// Last sound failure, if any (for example ffplay missing).
    pub audio_error: Option<String>,
}

struct Control {
    stop: AtomicBool,
    paused: AtomicBool,
    looping: AtomicBool,
}

impl Control {
    fn new(looping: bool) -> Self {
        Self {
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            looping: AtomicBool::new(looping),
        }
    }
}

enum PlaybackEvent {
    Started {
        path: PathBuf,
        duration: Option<Duration>,
        from_cache: Option<bool>,
    },
    Frame(VideoFrame),
    Looped {
        count: u32,
    },
    Failed {
        error: MediaError,
        message: String,
    },
    Ended,
}

enum Source {
    Url(String),
    LocalFile(PathBuf),
}

/// A playing video.
///
/// The URL is resolved through the validated media client into a local file
/// first; ffmpeg and ffplay only ever see that path. Frames are decoded off the
/// UI thread into a bounded channel and paced at [`PLAYBACK_FPS`].
pub struct VideoPlayer {
    events: Option<Receiver<PlaybackEvent>>,
    control: Arc<Control>,
    decoder: Arc<Mutex<Option<Child>>>,
    thread: Option<JoinHandle<()>>,
    audio: AudioController,
    frame: Option<VideoFrame>,
    frames: u64,
    duration: Option<Duration>,
    from_cache: Option<bool>,
    loop_count: u32,
    ended: bool,
    error: Option<String>,
    error_kind: Option<MediaError>,
    local_path: Option<PathBuf>,
    volume: u8,
    muted: bool,
}

impl VideoPlayer {
    /// Validate the URL, resolve it to a local file and start decoding.
    ///
    /// This is the legacy entry point: no persistent cache, muted, no looping.
    /// Use [`Self::start_with_options`] for cache, sound and loop control.
    pub fn start(url: impl Into<String>) -> Result<Self, MediaError> {
        Self::start_with_options(url, VideoPlayerOptions::default())
    }

    /// Validate `url`, resolve it to a local file and start decoding.
    ///
    /// Returns [`MediaError::UnsafeUrl`] when the URL fails the media policy,
    /// [`MediaError::DecoderUnavailable`] when ffmpeg cannot be found, and
    /// [`MediaError::Io`] when the player thread cannot be spawned. Download
    /// and decoder failures surface through [`Self::error`].
    pub fn start_with_options(
        url: impl Into<String>,
        options: VideoPlayerOptions,
    ) -> Result<Self, MediaError> {
        let url = url.into();
        if !is_allowed_media_url(&url) {
            return Err(MediaError::UnsafeUrl);
        }
        Self::launch(Source::Url(url), options)
    }

    /// Play an already-downloaded local file. The caller owns the file; the
    /// player never deletes it.
    pub fn start_local_file(
        path: impl Into<PathBuf>,
        options: VideoPlayerOptions,
    ) -> Result<Self, MediaError> {
        let path = path.into();
        if !path.is_file() {
            return Err(MediaError::Io);
        }
        Self::launch(Source::LocalFile(path), options)
    }

    fn launch(source: Source, mut options: VideoPlayerOptions) -> Result<Self, MediaError> {
        // Resolve tools synchronously so a missing decoder is a typed,
        // actionable error instead of a background failure.
        let tools = options.tools.take().unwrap_or_else(MediaTools::discover);
        let ffmpeg = tools.ffmpeg()?.to_path_buf();
        let ffplay = tools.ffplay().ok().map(Path::to_path_buf);

        let control = Arc::new(Control::new(options.looping));
        let decoder: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let (tx, rx) = sync_channel(EVENT_QUEUE);
        let thread_options = options.clone();
        let thread_control = Arc::clone(&control);
        let thread_decoder = Arc::clone(&decoder);
        let handle = std::thread::Builder::new()
            .name("eterea-video".into())
            .spawn(move || {
                video_thread(
                    source,
                    thread_options,
                    ffmpeg,
                    tx,
                    thread_control,
                    thread_decoder,
                );
            })
            .map_err(|_| MediaError::Io)?;
        let volume = options.volume.min(100);
        let muted = options.muted;
        Ok(Self {
            events: Some(rx),
            control,
            decoder,
            thread: Some(handle),
            audio: AudioController::new(ffplay),
            frame: None,
            frames: 0,
            duration: None,
            from_cache: None,
            loop_count: 0,
            ended: false,
            error: None,
            error_kind: None,
            local_path: None,
            volume,
            muted,
        })
    }

    /// Latest decoded frame, if any. Never blocks and never downloads.
    ///
    /// Frame events are only drained while playing; a paused player returns
    /// the last shown frame.
    pub fn poll(&mut self) -> Option<&VideoFrame> {
        if !self.is_paused() {
            self.drain_events();
        }
        self.frame.as_ref()
    }

    /// True once the decoder stopped or playback failed.
    pub fn is_ended(&self) -> bool {
        self.ended
    }

    /// The decoder/download error, if playback failed. Empty when playback was
    /// stopped deliberately.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The typed decoder/download error, if playback failed.
    ///
    /// A decoder that exits without producing frames is reported as
    /// [`MediaError::DecodeFailed`]; it is never restarted, even when looping
    /// is enabled.
    pub fn error_kind(&self) -> Option<MediaError> {
        self.error_kind
    }

    /// True when playback is paused.
    pub fn is_paused(&self) -> bool {
        self.control.paused.load(Ordering::SeqCst)
    }

    /// Pause frame delivery and stop sound. The decoder blocks on its bounded
    /// channel instead of running ahead.
    pub fn pause(&mut self) {
        self.control.paused.store(true, Ordering::SeqCst);
        self.audio.stop();
    }

    /// Resume playback, restarting sound at the current position when unmuted.
    ///
    /// Returns [`MediaError::AudioUnavailable`] when the sound could not be
    /// restarted; frames keep playing either way.
    pub fn play(&mut self) -> Result<(), MediaError> {
        self.control.paused.store(false, Ordering::SeqCst);
        if !self.muted && !self.ended && self.local_path.is_some() {
            self.audio.start(self.position(), self.volume)?;
        }
        Ok(())
    }

    /// Legacy play/pause toggle. Sound-restart failures are recorded in
    /// [`PlaybackState::audio_error`] instead of being returned.
    pub fn toggle_pause(&mut self) {
        if self.is_paused() {
            if let Err(error) = self.play() {
                self.audio.set_error(&error);
            }
        } else {
            self.pause();
        }
    }

    /// Mute or unmute. Unmuting starts ffplay at the current position unless
    /// the player is paused, ended, or still resolving the file.
    ///
    /// Returns [`MediaError::AudioUnavailable`] when `ffplay` is missing or
    /// cannot start; the player stays muted in that case.
    pub fn set_muted(&mut self, muted: bool) -> Result<(), MediaError> {
        if muted {
            self.muted = true;
            self.audio.stop();
            return Ok(());
        }
        if self.local_path.is_some() && !self.is_paused() && !self.ended {
            self.audio.start(self.position(), self.volume)?;
            self.muted = false;
            Ok(())
        } else {
            // The sound starts when playback is ready or resumed.
            self.muted = false;
            Ok(())
        }
    }

    /// Whether sound is muted.
    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// Set the ffplay volume, `0..=100` (clamped). Applies on the next audio
    /// start; a running ffplay is restarted at the current position.
    pub fn set_volume(&mut self, volume: u8) {
        self.volume = volume.min(100);
        if self.audio.is_active() {
            let position = self.position();
            let volume = self.volume;
            self.audio.stop();
            if let Err(error) = self.audio.start(position, volume) {
                self.audio.set_error(&error);
            }
        }
    }

    /// Current volume, `0..=100`.
    pub fn volume(&self) -> u8 {
        self.volume
    }

    /// Whether playback loops at the end.
    pub fn is_looping(&self) -> bool {
        self.control.looping.load(Ordering::SeqCst)
    }

    /// Enable or disable looping.
    pub fn set_looping(&mut self, looping: bool) {
        self.control.looping.store(looping, Ordering::SeqCst);
    }

    /// Approximate position within the file, derived from delivered frames.
    pub fn position(&self) -> Duration {
        Duration::from_secs_f64(self.frames as f64 / PLAYBACK_FPS as f64)
    }

    /// File duration when ffmpeg could probe it.
    pub fn duration(&self) -> Option<Duration> {
        self.duration
    }

    /// Number of completed loops.
    pub fn loop_count(&self) -> u32 {
        self.loop_count
    }

    /// Frames delivered to the UI since the current loop started.
    pub fn frames_delivered(&self) -> u64 {
        self.frames
    }

    /// Whether ffplay is currently running.
    pub fn is_audio_active(&mut self) -> bool {
        self.audio.is_active()
    }

    /// Snapshot of playback state for the UI. Call [`Self::poll`] first to
    /// fold in any pending frames/events.
    pub fn state(&mut self) -> PlaybackState {
        PlaybackState {
            paused: self.is_paused(),
            muted: self.muted,
            volume: self.volume,
            looping: self.is_looping(),
            ended: self.ended,
            error: self.error.clone(),
            error_kind: self.error_kind,
            position: self.position(),
            duration: self.duration,
            from_cache: self.from_cache,
            loop_count: self.loop_count,
            audio_active: self.audio.is_active(),
            audio_error: self.audio.error().map(str::to_owned),
        }
    }

    /// Stop playback now: signal cancellation, kill ffmpeg/ffplay, close the
    /// frame channel and detach the background thread.
    ///
    /// This never blocks on network or process I/O, so it is safe from a UI
    /// thread. Cached files are left in the persistent cache; only partial
    /// downloads are removed by the cache itself.
    pub fn stop(&mut self) {
        self.control.stop.store(true, Ordering::SeqCst);
        // Closing the receiver unblocks a thread waiting to send a frame.
        self.events.take();
        if let Ok(mut slot) = self.decoder.lock() {
            if let Some(mut child) = slot.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        self.audio.stop();
        self.ended = true;
        // Detach rather than join: the worker may still be inside a blocking
        // HTTP read and the UI must not stall.
        self.thread.take();
    }

    fn drain_events(&mut self) {
        let Some(rx) = self.events.as_ref() else {
            self.ended = true;
            return;
        };
        loop {
            match rx.try_recv() {
                Ok(PlaybackEvent::Started {
                    path,
                    duration,
                    from_cache,
                }) => {
                    self.local_path = Some(path);
                    self.duration = duration;
                    self.from_cache = from_cache;
                    self.audio.set_file(self.local_path.clone());
                    if !self.muted {
                        let position = self.position();
                        if let Err(error) = self.audio.start(position, self.volume) {
                            self.audio.set_error(&error);
                        }
                    }
                }
                Ok(PlaybackEvent::Frame(frame)) => {
                    self.frames = self.frames.saturating_add(1);
                    self.frame = Some(frame);
                }
                Ok(PlaybackEvent::Looped { count }) => {
                    self.loop_count = count;
                    self.frames = 0;
                    if !self.muted {
                        self.audio.stop();
                        if let Err(error) = self.audio.start(Duration::ZERO, self.volume) {
                            self.audio.set_error(&error);
                        }
                    }
                }
                Ok(PlaybackEvent::Failed { error, message }) => {
                    self.error = Some(message);
                    self.error_kind = Some(error);
                    self.ended = true;
                    self.audio.stop();
                }
                Ok(PlaybackEvent::Ended) => {
                    self.ended = true;
                    self.audio.stop();
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.ended = true;
                    self.audio.stop();
                    return;
                }
            }
        }
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

struct AudioController {
    command: Option<PathBuf>,
    file: Option<PathBuf>,
    child: Option<Child>,
    error: Option<String>,
}

impl AudioController {
    fn new(command: Option<PathBuf>) -> Self {
        Self {
            command,
            file: None,
            child: None,
            error: None,
        }
    }

    fn set_file(&mut self, file: Option<PathBuf>) {
        self.file = file;
    }

    fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn set_error(&mut self, error: &MediaError) {
        self.error = Some(error.to_string());
    }

    fn start(&mut self, position: Duration, volume: u8) -> Result<(), MediaError> {
        self.stop();
        let Some(command) = self.command.as_ref() else {
            self.error = Some(MediaError::AudioUnavailable.to_string());
            return Err(MediaError::AudioUnavailable);
        };
        let Some(file) = self.file.as_ref() else {
            // The file is not resolved yet; the start is retried once the
            // `Started` event arrives.
            return Ok(());
        };
        let mut child = Command::new(command);
        child.arg("-nodisp").arg("-vn").arg("-autoexit");
        if position > Duration::ZERO {
            child
                .arg("-ss")
                .arg(format!("{:.3}", position.as_secs_f64()));
        }
        child
            .arg("-volume")
            .arg(volume.min(100).to_string())
            .arg("-loglevel")
            .arg("quiet")
            .arg(file)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match child.spawn() {
            Ok(spawned) => {
                self.child = Some(spawned);
                self.error = None;
                Ok(())
            }
            Err(_) => {
                self.error = Some(MediaError::AudioUnavailable.to_string());
                Err(MediaError::AudioUnavailable)
            }
        }
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn is_active(&mut self) -> bool {
        let active = match self.child.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        };
        if !active {
            if let Some(mut child) = self.child.take() {
                let _ = child.wait();
            }
        }
        active
    }
}

impl Drop for AudioController {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// Background worker
// ---------------------------------------------------------------------------

fn video_thread(
    source: Source,
    options: VideoPlayerOptions,
    ffmpeg: PathBuf,
    events: SyncSender<PlaybackEvent>,
    control: Arc<Control>,
    decoder: Arc<Mutex<Option<Child>>>,
) {
    let (path, from_cache) = match resolve_source(&source, &options, &control) {
        Ok(resolved) => resolved,
        Err(error) => {
            if !control.stop.load(Ordering::SeqCst) {
                let _ = events.send(PlaybackEvent::Failed {
                    error,
                    message: error.to_string(),
                });
            }
            return;
        }
    };
    if control.stop.load(Ordering::SeqCst) {
        return;
    }
    let duration = if options.probe_duration {
        probe_duration(&ffmpeg, &path)
    } else {
        None
    };
    let started = events.send(PlaybackEvent::Started {
        path: path.clone(),
        duration,
        from_cache,
    });
    if started.is_err() {
        return;
    }

    let mut loops: u32 = 0;
    'playback: loop {
        if control.stop.load(Ordering::SeqCst) {
            break;
        }
        let mut child = match spawn_decoder(&ffmpeg, &path) {
            Ok(child) => child,
            Err(error) => {
                let _ = events.send(PlaybackEvent::Failed {
                    error,
                    message: error.to_string(),
                });
                break;
            }
        };
        let Some(mut stdout) = child.stdout.take() else {
            kill_child(&mut child);
            let _ = events.send(PlaybackEvent::Failed {
                error: MediaError::DecodeFailed,
                message: "video decoder produced no output pipe".to_string(),
            });
            break;
        };
        let stderr = child.stderr.take().and_then(drain_child_stderr);
        if let Ok(mut slot) = decoder.lock() {
            *slot = Some(child);
        }

        let mut buffer = vec![0_u8; FRAME_BYTES];
        let mut next_frame = Instant::now();
        let mut frames_in_run: u64 = 0;
        loop {
            if control.stop.load(Ordering::SeqCst) {
                break;
            }
            while control.paused.load(Ordering::SeqCst) && !control.stop.load(Ordering::SeqCst) {
                std::thread::sleep(PAUSE_POLL);
            }
            match stdout.read_exact(&mut buffer) {
                Ok(()) => {
                    frames_in_run = frames_in_run.saturating_add(1);
                    let frame = VideoFrame {
                        rgba: buffer.clone(),
                    };
                    if events.send(PlaybackEvent::Frame(frame)).is_err() {
                        // The player was dropped: shut down.
                        break 'playback;
                    }
                    next_frame += FRAME_INTERVAL;
                    let now = Instant::now();
                    if next_frame > now {
                        std::thread::sleep(next_frame - now);
                    } else {
                        next_frame = now;
                    }
                }
                Err(_) => break,
            }
        }
        drop(stdout);

        let stopped = control.stop.load(Ordering::SeqCst);
        let mut child = take_decoder(&decoder);
        if stopped {
            if let Some(child) = child.as_mut() {
                kill_child(child);
            }
            // Killing the decoder closes its stderr pipe, so the drain thread
            // ends; join it so it cannot outlive this playback run.
            drop(join_stderr(stderr));
            break;
        }

        // The decoder closes stdout before it exits. Reap it here so the exit
        // status is known: a non-zero exit or a run that produced no frame is
        // a decode failure, never an "ended" or a loop. Before this check the
        // desktop's always-on looping turned any undecodable file into an
        // infinite ffmpeg respawn loop.
        let status = child.as_mut().and_then(reap_after_stream_end);
        let exit_failed = status.map(|status| !status.success()).unwrap_or(true);
        let detail = join_stderr(stderr);
        if exit_failed || frames_in_run == 0 {
            let _ = events.send(PlaybackEvent::Failed {
                error: MediaError::DecodeFailed,
                message: decode_failure_message(frames_in_run, status, detail),
            });
            break;
        }

        if control.looping.load(Ordering::SeqCst) {
            loops = loops.saturating_add(1);
            if events.send(PlaybackEvent::Looped { count: loops }).is_err() {
                break;
            }
            continue 'playback;
        }
        let _ = events.send(PlaybackEvent::Ended);
        break;
    }
    kill_decoder(&decoder);
}

fn resolve_source(
    source: &Source,
    options: &VideoPlayerOptions,
    control: &Control,
) -> Result<(PathBuf, Option<bool>), MediaError> {
    match source {
        Source::LocalFile(path) => Ok((path.clone(), None)),
        Source::Url(url) => {
            let cache = match &options.cache {
                Some(cache) => Arc::clone(cache),
                None => {
                    let dir = options
                        .scratch_dir
                        .clone()
                        .unwrap_or_else(|| std::env::temp_dir().join("eterea-media"));
                    Arc::new(
                        VideoCache::open(
                            VideoCacheConfig::new(dir).with_max_bytes(options.max_bytes.max(1)),
                        )
                        .map_err(|_| MediaError::Io)?,
                    )
                }
            };
            let cached = cache.ensure_cancellable(url, options.max_bytes, &control.stop)?;
            Ok((cached.path, Some(cached.from_cache)))
        }
    }
}

fn spawn_decoder(ffmpeg: &Path, path: &Path) -> Result<Child, MediaError> {
    let filter = format!(
        "scale={PLAYBACK_WIDTH}:{PLAYBACK_HEIGHT}:force_original_aspect_ratio=decrease,\
         pad={PLAYBACK_WIDTH}:{PLAYBACK_HEIGHT}:(ow-iw)/2:(oh-ih)/2"
    );
    Command::new(ffmpeg)
        .args(["-loglevel", "error", "-nostdin", "-i"])
        .arg(path)
        .args([
            "-an", "-vf", &filter, "-r", "30", "-f", "rawvideo", "-pix_fmt", "rgba", "-",
        ])
        .stdout(Stdio::piped())
        // stderr carries the decoder's diagnostic for failure messages; it is
        // drained on a helper thread so a chatty decoder cannot block on a
        // full pipe, and it is bounded (see `drain_child_stderr`).
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| MediaError::DecoderUnavailable)
}

fn kill_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn kill_decoder(decoder: &Mutex<Option<Child>>) {
    if let Ok(mut slot) = decoder.lock() {
        if let Some(mut child) = slot.take() {
            kill_child(&mut child);
        }
    }
}

fn take_decoder(decoder: &Mutex<Option<Child>>) -> Option<Child> {
    decoder.lock().ok().and_then(|mut slot| slot.take())
}

/// Grace period for a decoder to exit after it closed its stdout pipe.
const DECODER_EXIT_GRACE: Duration = Duration::from_secs(2);

/// Reap a decoder whose stdout already closed, without blocking forever.
///
/// ffmpeg exits immediately after closing its output pipe; a stuck child is
/// killed after [`DECODER_EXIT_GRACE`] instead of blocking the worker thread
/// and leaving an orphan process behind.
fn reap_after_stream_end(child: &mut Child) -> Option<ExitStatus> {
    let deadline = Instant::now() + DECODER_EXIT_GRACE;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(None) => {
                kill_child(child);
                return child.try_wait().ok().flatten();
            }
            Err(_) => return None,
        }
    }
}

/// How many trailing stderr bytes are kept for a decode-failure message.
const DECODER_STDERR_TAIL: usize = 2048;
/// Maximum human-readable decoder detail appended to the message.
const DECODER_DETAIL_CHARS: usize = 300;

/// Drain a decoder's stderr on its own thread, keeping a bounded tail.
///
/// The pipe must be drained or a chatty decoder could block on a full pipe;
/// only the last [`DECODER_STDERR_TAIL`] bytes are kept so a broken decoder
/// cannot grow memory without bound.
fn drain_child_stderr(mut stderr: ChildStderr) -> Option<JoinHandle<Vec<u8>>> {
    std::thread::Builder::new()
        .name("eterea-video-stderr".into())
        .spawn(move || {
            let mut tail: Vec<u8> = Vec::new();
            let mut chunk = [0_u8; 1024];
            loop {
                match stderr.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        tail.extend_from_slice(&chunk[..read]);
                        if tail.len() > DECODER_STDERR_TAIL {
                            let excess = tail.len() - DECODER_STDERR_TAIL;
                            tail.drain(..excess);
                        }
                    }
                    Err(_) => break,
                }
            }
            tail
        })
        .ok()
}

fn join_stderr(handle: Option<JoinHandle<Vec<u8>>>) -> Option<String> {
    let bytes = handle?.join().ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return None;
    }
    Some(cleaned.chars().take(DECODER_DETAIL_CHARS).collect())
}

/// Actionable, URL-free failure message for a decoder run that produced no
/// usable frames.
fn decode_failure_message(
    frames: u64,
    status: Option<ExitStatus>,
    detail: Option<String>,
) -> String {
    let mut message = String::from("video decoder failed");
    if let Some(status) = status {
        match status.code() {
            Some(code) => message.push_str(&format!(" (exit code {code})")),
            None => message.push_str(" (terminated by signal)"),
        }
    }
    if frames == 0 {
        message.push_str(": no frames decoded");
    }
    if let Some(detail) = detail {
        message.push_str(": ");
        message.push_str(&detail);
    }
    message
}

fn probe_duration(ffmpeg: &Path, path: &Path) -> Option<Duration> {
    let output = Command::new(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    parse_ffmpeg_duration(&String::from_utf8_lossy(&output.stderr))
}

/// Parse `Duration: HH:MM:SS.ff` from ffmpeg's banner. Pure and unit-tested.
pub(crate) fn parse_ffmpeg_duration(text: &str) -> Option<Duration> {
    let start = text.find("Duration: ")? + "Duration: ".len();
    let rest = &text[start..];
    let value = rest.split(',').next()?.trim();
    if value == "N/A" {
        return None;
    }
    let mut parts = value.split(':');
    let hours: u64 = parts.next()?.trim().parse().ok()?;
    let minutes: u64 = parts.next()?.trim().parse().ok()?;
    let seconds_part = parts.next()?.trim();
    let (seconds, fraction) = match seconds_part.split_once('.') {
        Some((seconds, fraction)) => (seconds, fraction),
        None => (seconds_part, "0"),
    };
    let seconds: u64 = seconds.parse().ok()?;
    let millis: u64 = if fraction.is_empty() {
        0
    } else {
        let digits = fraction.chars().take(3).collect::<String>();
        format!("{digits:0<3}").parse().ok()?
    };
    Some(Duration::from_millis(
        ((hours * 60 + minutes) * 60 + seconds) * 1000 + millis,
    ))
}

#[cfg(test)]
mod playback_tests {
    use super::{
        parse_ffmpeg_duration, VideoPlayer, VideoPlayerOptions, PLAYBACK_HEIGHT, PLAYBACK_WIDTH,
    };
    use crate::error::MediaError;
    use crate::test_support::{write_script, ScratchDir};
    use crate::tools::MediaTools;
    use std::time::{Duration, Instant};

    const FRAME_BYTES: usize = (PLAYBACK_WIDTH * PLAYBACK_HEIGHT * 4) as usize;

    #[test]
    fn players_reject_unsafe_urls_before_any_thread() {
        for url in [
            "http://video.twimg.com/x.mp4",
            "https://127.0.0.1/x.mp4",
            "https://video.twimg.com/x.mp4#t=2",
        ] {
            assert_eq!(VideoPlayer::start(url).err(), Some(MediaError::UnsafeUrl));
        }
    }

    #[test]
    fn frame_dimensions_are_the_playback_size() {
        let frame = super::VideoFrame {
            rgba: vec![0; (PLAYBACK_WIDTH * PLAYBACK_HEIGHT * 4) as usize],
        };
        assert_eq!(frame.width(), PLAYBACK_WIDTH);
        assert_eq!(frame.height(), PLAYBACK_HEIGHT);
    }

    #[cfg(unix)]
    fn fake_decoder_script(scratch: &ScratchDir, frames: usize) -> std::path::PathBuf {
        let bytes = FRAME_BYTES * frames;
        write_script(
            scratch.path(),
            "fake-ffmpeg",
            &format!("#!/bin/sh\nhead -c {bytes} /dev/zero\n"),
        )
    }

    #[cfg(unix)]
    fn fake_sound_script(scratch: &ScratchDir) -> std::path::PathBuf {
        write_script(scratch.path(), "fake-ffplay", "#!/bin/sh\nexec sleep 30\n")
    }

    #[cfg(unix)]
    fn local_video(scratch: &ScratchDir) -> std::path::PathBuf {
        let path = scratch.path().join("clip.mp4");
        std::fs::write(&path, b"not really an mp4").expect("local fixture");
        path
    }

    /// A fake decoder that logs each spawn, writes `bytes` bytes of "frames",
    /// writes `stderr_line` to stderr and exits with `exit_code`. Used to prove
    /// a failed decode is reported once and never respawned.
    #[cfg(unix)]
    fn failing_decoder_script(
        scratch: &ScratchDir,
        spawn_log: &std::path::Path,
        bytes: usize,
        exit_code: i32,
        stderr_line: &str,
    ) -> std::path::PathBuf {
        write_script(
            scratch.path(),
            "failing-ffmpeg",
            &format!(
                "#!/bin/sh\necho run >> \"{}\"\nhead -c {bytes} /dev/zero\nprintf '%s\\n' '{stderr_line}' >&2\nexit {exit_code}\n",
                spawn_log.display()
            ),
        )
    }

    #[cfg(unix)]
    fn spawn_count(spawn_log: &std::path::Path) -> usize {
        std::fs::read_to_string(spawn_log)
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    #[cfg(unix)]
    #[test]
    fn local_file_decodes_paced_frames_and_ends() {
        let _process = crate::test_support::process_test_lock();
        let scratch = ScratchDir::new("player-frames");
        let decoder = fake_decoder_script(&scratch, 4);
        let file = local_video(&scratch);
        let options = VideoPlayerOptions::new()
            .with_tools(MediaTools::from_paths(decoder, None))
            .with_probe_duration(false);
        let mut player =
            VideoPlayer::start_local_file(&file, options).expect("start local playback");
        assert_eq!(player.frames_delivered(), 0);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !player.is_ended() && Instant::now() < deadline {
            if let Some(frame) = player.poll() {
                assert_eq!(frame.rgba.len(), FRAME_BYTES);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            player.is_ended(),
            "player must end after the fake decoder (error={:?})",
            player.error()
        );
        assert_eq!(player.error(), None);
        assert_eq!(player.frames_delivered(), 4);
        assert!(player.position() >= Duration::from_millis(90));
        assert!(player.state().ended);
    }

    #[cfg(unix)]
    #[test]
    fn failed_decoder_is_a_typed_error_and_never_respawned() {
        let _process = crate::test_support::process_test_lock();
        let scratch = ScratchDir::new("player-decode-failure");
        let spawn_log = scratch.path().join("decoder-spawns.log");
        let decoder = failing_decoder_script(
            &scratch,
            &spawn_log,
            0,
            1,
            "invalid data found when processing input",
        );
        let file = local_video(&scratch);
        // The desktop always enables looping, so the failure path must be safe
        // with looping on: one spawn, one typed error, no restart.
        let options = VideoPlayerOptions::new()
            .with_tools(MediaTools::from_paths(decoder, None))
            .with_probe_duration(false)
            .with_looping(true);
        let mut player = VideoPlayer::start_local_file(&file, options).expect("start");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !player.is_ended() && Instant::now() < deadline {
            let _ = player.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            player.is_ended(),
            "a failed decoder must end the player (error={:?})",
            player.error()
        );
        assert_eq!(player.error_kind(), Some(MediaError::DecodeFailed));
        let message = player.error().unwrap_or_default();
        assert!(
            message.contains("video decoder failed"),
            "message must be actionable: {message}"
        );
        assert!(
            message.contains("exit code 1"),
            "message must carry the decoder exit status: {message}"
        );
        assert!(
            message.contains("invalid data found"),
            "decoder stderr must be surfaced: {message}"
        );
        assert_eq!(
            player.loop_count(),
            0,
            "a failed decode must not count or perform a loop"
        );
        assert_eq!(player.frames_delivered(), 0);
        let state = player.state();
        assert_eq!(state.error_kind, Some(MediaError::DecodeFailed));
        assert!(state.ended);
        assert!(state.error.is_some());
        // Give a buggy respawn loop room to show itself, then prove it did not
        // happen: exactly one decoder process for the whole playback.
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(
            spawn_count(&spawn_log),
            1,
            "a failed decoder must never be respawned"
        );
        player.stop();
    }

    #[cfg(unix)]
    #[test]
    fn truncated_decoder_output_is_a_failure_not_a_quiet_end() {
        let _process = crate::test_support::process_test_lock();
        let scratch = ScratchDir::new("player-truncated-frame");
        let spawn_log = scratch.path().join("decoder-spawns.log");
        // Exits 0 but closes the pipe mid-frame: zero complete frames were
        // decoded, so this must be a typed failure, not Ended.
        let decoder =
            failing_decoder_script(&scratch, &spawn_log, FRAME_BYTES / 2, 0, "partial frame");
        let file = local_video(&scratch);
        let options = VideoPlayerOptions::new()
            .with_tools(MediaTools::from_paths(decoder, None))
            .with_probe_duration(false)
            .with_looping(true);
        let mut player = VideoPlayer::start_local_file(&file, options).expect("start");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !player.is_ended() && Instant::now() < deadline {
            let _ = player.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(player.is_ended());
        assert_eq!(player.error_kind(), Some(MediaError::DecodeFailed));
        assert_eq!(player.frames_delivered(), 0);
        assert_eq!(player.loop_count(), 0);
        assert!(
            player
                .error()
                .unwrap_or_default()
                .contains("no frames decoded"),
            "zero-frame run must say so: {:?}",
            player.error()
        );
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(spawn_count(&spawn_log), 1);
        player.stop();
    }

    #[cfg(unix)]
    #[test]
    fn real_ffmpeg_on_an_undecodable_file_reports_decode_failed_once() {
        let _process = crate::test_support::process_test_lock();
        let tools = MediaTools::discover();
        let Ok(ffmpeg) = tools.ffmpeg() else {
            eprintln!("skipping: ffmpeg is not installed");
            return;
        };
        let ffmpeg = ffmpeg.to_path_buf();
        let scratch = ScratchDir::new("player-real-ffmpeg-garbage");
        let file = scratch.path().join("garbage.mp4");
        std::fs::write(&file, vec![0_u8; 4096]).expect("garbage fixture");
        let options = VideoPlayerOptions::new()
            .with_tools(MediaTools::from_paths(ffmpeg, None))
            .with_probe_duration(false)
            .with_looping(true);
        let mut player = VideoPlayer::start_local_file(&file, options).expect("start");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !player.is_ended() && Instant::now() < deadline {
            let _ = player.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            player.is_ended(),
            "real ffmpeg on garbage must end the player (error={:?})",
            player.error()
        );
        assert_eq!(player.error_kind(), Some(MediaError::DecodeFailed));
        assert_eq!(player.loop_count(), 0, "must not loop a failed decode");
        assert_eq!(player.frames_delivered(), 0);
        assert!(
            player.error().unwrap_or_default().contains("exit code"),
            "message should carry ffmpeg's exit status: {:?}",
            player.error()
        );
        player.stop();
    }

    #[cfg(unix)]
    #[test]
    fn failed_decoder_without_looping_is_not_a_quiet_end() {
        let _process = crate::test_support::process_test_lock();
        let scratch = ScratchDir::new("player-decode-failure-no-loop");
        let spawn_log = scratch.path().join("decoder-spawns.log");
        let decoder = failing_decoder_script(&scratch, &spawn_log, 0, 2, "no stream");
        let file = local_video(&scratch);
        let options = VideoPlayerOptions::new()
            .with_tools(MediaTools::from_paths(decoder, None))
            .with_probe_duration(false);
        let mut player = VideoPlayer::start_local_file(&file, options).expect("start");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !player.is_ended() && Instant::now() < deadline {
            let _ = player.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(player.is_ended());
        // Regression for the old behaviour: this used to be `Ended` with
        // error == None, i.e. a broken video looked like a finished one.
        assert!(
            player.error().is_some(),
            "failure must not look like a normal end"
        );
        assert_eq!(player.error_kind(), Some(MediaError::DecodeFailed));
        assert_eq!(player.loop_count(), 0);
        player.stop();
    }

    #[cfg(unix)]
    #[test]
    fn missing_decoder_is_a_typed_actionable_error() {
        let scratch = ScratchDir::new("player-missing-decoder");
        let file = local_video(&scratch);
        let options = VideoPlayerOptions::new().with_tools(MediaTools::from_paths(
            scratch.path().join("no-ffmpeg-here"),
            None,
        ));
        assert_eq!(
            VideoPlayer::start_local_file(&file, options).err(),
            Some(MediaError::DecoderUnavailable)
        );
        assert!(MediaError::DecoderUnavailable
            .to_string()
            .contains("install ffmpeg"));
    }

    #[test]
    fn local_missing_file_is_a_typed_error() {
        let options = VideoPlayerOptions::new().with_probe_duration(false);
        assert_eq!(
            VideoPlayer::start_local_file("/definitely/not/a/file.mp4", options).err(),
            Some(MediaError::Io)
        );
    }

    #[cfg(unix)]
    #[test]
    fn looping_replays_without_ending() {
        let _process = crate::test_support::process_test_lock();
        let scratch = ScratchDir::new("player-loop");
        let decoder = fake_decoder_script(&scratch, 2);
        let file = local_video(&scratch);
        let options = VideoPlayerOptions::new()
            .with_tools(MediaTools::from_paths(decoder, None))
            .with_probe_duration(false)
            .with_looping(true);
        let mut player = VideoPlayer::start_local_file(&file, options).expect("start");
        let deadline = Instant::now() + Duration::from_secs(5);
        while player.loop_count() < 2 && Instant::now() < deadline && !player.is_ended() {
            let _ = player.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            player.loop_count() >= 2,
            "playback must loop (loop_count={}, error={:?})",
            player.loop_count(),
            player.error()
        );
        assert_eq!(player.error(), None, "valid looping must not report errors");
        assert_eq!(player.error_kind(), None);
        assert!(!player.is_ended());
        player.stop();
        assert!(player.is_ended());
    }

    #[cfg(unix)]
    #[test]
    fn audio_starts_mutes_pauses_and_resumes() {
        let _process = crate::test_support::process_test_lock();
        let scratch = ScratchDir::new("player-audio");
        let decoder = fake_decoder_script(&scratch, 150);
        let sound = fake_sound_script(&scratch);
        let file = local_video(&scratch);
        let options = VideoPlayerOptions::new()
            .with_tools(MediaTools::from_paths(decoder, Some(sound)))
            .with_probe_duration(false)
            .with_muted(false);
        let mut player = VideoPlayer::start_local_file(&file, options).expect("start");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !player.is_audio_active() && Instant::now() < deadline {
            let _ = player.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            player.is_audio_active(),
            "unmuted playback must spawn ffplay"
        );
        assert!(!player.is_muted());

        player.set_muted(true).expect("mute");
        assert!(player.is_muted());
        assert!(!player.is_audio_active());

        player.set_muted(false).expect("unmute");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !player.is_audio_active() && Instant::now() < deadline {
            let _ = player.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(player.is_audio_active());

        player.pause();
        assert!(player.is_paused());
        assert!(!player.is_audio_active());
        player.play().expect("resume");
        assert!(!player.is_paused());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !player.is_audio_active() && Instant::now() < deadline {
            let _ = player.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(player.is_audio_active());

        player.set_volume(250);
        assert_eq!(player.volume(), 100);
        player.stop();
        assert!(!player.is_audio_active());
    }

    #[cfg(unix)]
    #[test]
    fn unmuting_without_ffplay_is_a_typed_error() {
        let _process = crate::test_support::process_test_lock();
        let scratch = ScratchDir::new("player-no-ffplay");
        let decoder = fake_decoder_script(&scratch, 60);
        let file = local_video(&scratch);
        let options = VideoPlayerOptions::new()
            .with_tools(MediaTools::from_paths(decoder, None))
            .with_probe_duration(false);
        let mut player = VideoPlayer::start_local_file(&file, options).expect("start");
        // Wait until the player resolved the local file (the `Started` event).
        let deadline = Instant::now() + Duration::from_secs(5);
        while player.poll().is_none() && Instant::now() < deadline && !player.is_ended() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            player.poll().is_some(),
            "player failed before the first frame: {:?}",
            player.error()
        );
        let error = player.set_muted(false).unwrap_err();
        assert_eq!(error, MediaError::AudioUnavailable);
        assert_eq!(
            player.state().audio_error.as_deref(),
            Some("ffplay not found: install ffmpeg (it includes ffplay) or add it to PATH")
        );
        player.stop();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stop_kills_the_decoder_process() {
        let _process = crate::test_support::process_test_lock();
        let scratch = ScratchDir::new("player-kill-child");
        let pid_file = scratch.path().join("decoder.pid");
        let script = write_script(
            scratch.path(),
            "slow-ffmpeg",
            &format!(
                "#!/bin/sh\necho $$ > {}\nexec sleep 30\n",
                pid_file.display()
            ),
        );
        let file = local_video(&scratch);
        let options = VideoPlayerOptions::new()
            .with_tools(MediaTools::from_paths(script, None))
            .with_probe_duration(false);
        let mut player = VideoPlayer::start_local_file(&file, options).expect("start");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !pid_file.exists() && Instant::now() < deadline && !player.is_ended() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            pid_file.exists(),
            "decoder process did not start: {:?}",
            player.error()
        );
        let pid: u32 = std::fs::read_to_string(&pid_file)
            .expect("decoder pid file")
            .trim()
            .parse()
            .expect("decoder pid");
        let proc_path = format!("/proc/{pid}");
        assert!(std::path::Path::new(&proc_path).exists());
        player.stop();
        assert!(
            !std::path::Path::new(&proc_path).exists(),
            "stop must kill and reap the decoder child"
        );
    }

    #[cfg(unix)]
    #[test]
    fn looping_playback_keeps_restarting_the_decoder() {
        let _process = crate::test_support::process_test_lock();
        let scratch = ScratchDir::new("player-loop-audio");
        let decoder = fake_decoder_script(&scratch, 2);
        let sound = fake_sound_script(&scratch);
        let file = local_video(&scratch);
        let options = VideoPlayerOptions::new()
            .with_tools(MediaTools::from_paths(decoder, Some(sound)))
            .with_probe_duration(false)
            .with_looping(true)
            .with_muted(false);
        let mut player = VideoPlayer::start_local_file(&file, options).expect("start");
        let deadline = Instant::now() + Duration::from_secs(5);
        while player.loop_count() < 1 && Instant::now() < deadline {
            let _ = player.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(player.loop_count() >= 1, "playback must loop");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !player.is_audio_active() && Instant::now() < deadline {
            let _ = player.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(player.is_audio_active(), "sound restarts after a loop");
        player.stop();
        assert!(!player.is_audio_active());
    }

    #[test]
    fn duration_parser_reads_ffmpeg_banners() {
        let text = "Input #0, mov, from 'clip.mp4':\n  Duration: 00:00:10.02, start: 0.000000, bitrate: 1 kb/s";
        assert_eq!(
            parse_ffmpeg_duration(text),
            Some(Duration::from_millis(10_020))
        );
        assert_eq!(
            parse_ffmpeg_duration("  Duration: 01:02:03.50, start: 0"),
            Some(Duration::from_millis(((60 + 2) * 60 + 3) * 1000 + 500))
        );
        assert_eq!(parse_ffmpeg_duration("no duration here"), None);
        assert_eq!(parse_ffmpeg_duration("Duration: N/A, start: 0"), None);
    }

    #[cfg(unix)]
    #[test]
    fn real_ffmpeg_decodes_a_generated_local_fixture() {
        let _process = crate::test_support::process_test_lock();
        let tools = MediaTools::discover().probe_versions();
        let Ok(ffmpeg) = tools.ffmpeg() else {
            eprintln!("skipping: ffmpeg is not installed");
            return;
        };
        let scratch = ScratchDir::new("player-real-ffmpeg");
        let fixture = scratch.path().join("fixture.mp4");
        // testsrc 64x36 at 10 fps for half a second, encoded with the built-in
        // mpeg4 encoder so no optional codec library is required.
        let encoded = std::process::Command::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=64x36:rate=10:duration=0.5",
                "-c:v",
                "mpeg4",
                "-y",
            ])
            .arg(&fixture)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if !matches!(encoded, Ok(status) if status.success()) {
            eprintln!("skipping: ffmpeg could not encode the fixture");
            return;
        }
        // Use explicit tools but a real ffmpeg; duration probe is real and the
        // 960x540 decode path must letterbox the tiny source.
        let options = VideoPlayerOptions::new().with_tools(MediaTools::from_paths(ffmpeg, None));
        let mut player = VideoPlayer::start_local_file(&fixture, options).expect("start");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !player.is_ended() && Instant::now() < deadline {
            let _ = player.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(player.is_ended(), "fixture must finish decoding");
        assert_eq!(player.error(), None);
        assert!(
            player.frames_delivered() >= 3,
            "expected decoded frames, got {}",
            player.frames_delivered()
        );
        assert!(player.poll().is_some(), "a decoded frame must be available");
        assert!(player.duration().is_some(), "duration should be probed");
        player.stop();
    }
}

#[cfg(test)]
mod network_playback_smoke {
    use super::VideoPlayer;

    #[test]
    #[ignore = "network: downloads a public sample video"]
    fn a_public_sample_decodes_frames() {
        let url = "https://test-videos.co.uk/vids/bigbuckbunny/mp4/h264/360/Big_Buck_Bunny_360_10s_1MB.mp4";
        let mut player = VideoPlayer::start(url).expect("start");
        let started = std::time::Instant::now();
        loop {
            if let Some(frame) = player.poll() {
                assert_eq!(frame.rgba.len(), 960 * 540 * 4);
                assert!(frame.rgba.iter().any(|byte| *byte != 0));
                return;
            }
            assert!(!player.is_ended(), "player failed: {:?}", player.error());
            assert!(
                started.elapsed() < std::time::Duration::from_secs(15),
                "timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod cache_playback_tests {
    use super::{VideoPlayer, VideoPlayerOptions};
    use crate::error::MediaError;
    use crate::test_support::{write_script, ScratchDir};
    use crate::tools::MediaTools;
    use crate::video_cache::{VideoCache, VideoCacheConfig, VideoFetcher};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const FRAME_BYTES: usize = 960 * 540 * 4;

    struct FakeVideoFetcher {
        calls: AtomicUsize,
    }

    impl VideoFetcher for FakeVideoFetcher {
        fn fetch_to(
            &self,
            _url: &str,
            destination: &Path,
            _max_bytes: u64,
            _cancel: &AtomicBool,
        ) -> Result<u64, MediaError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::fs::write(destination, b"fake video bytes").map_err(|_| MediaError::Io)?;
            Ok(16)
        }
    }

    #[cfg(unix)]
    #[test]
    fn players_reuse_a_persistent_cache_across_plays() {
        let _process = crate::test_support::process_test_lock();
        let scratch = ScratchDir::new("player-cache-reuse");
        let fetcher = Arc::new(FakeVideoFetcher {
            calls: AtomicUsize::new(0),
        });
        let cache = Arc::new(
            VideoCache::open_with_fetcher(
                VideoCacheConfig::new(scratch.path()).with_max_bytes(1024 * 1024),
                fetcher.clone(),
            )
            .expect("cache"),
        );
        let decoder = write_script(
            scratch.path(),
            "fake-ffmpeg-cache",
            &format!("#!/bin/sh\nhead -c {} /dev/zero\n", FRAME_BYTES * 3),
        );
        let tools = MediaTools::from_paths(decoder, None);
        let url = "https://video.twimg.com/ext_tw_video/cached.mp4";

        for expected_from_cache in [false, true] {
            let options = VideoPlayerOptions::new()
                .with_cache(Arc::clone(&cache))
                .with_tools(tools.clone())
                .with_max_bytes(1024)
                .with_probe_duration(false);
            let mut player = VideoPlayer::start_with_options(url, options).expect("start");
            let deadline = Instant::now() + Duration::from_secs(5);
            while player.state().from_cache.is_none() && Instant::now() < deadline {
                let _ = player.poll();
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(player.state().from_cache, Some(expected_from_cache));
            while !player.is_ended() && Instant::now() < deadline {
                let _ = player.poll();
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(player.is_ended());
            assert_eq!(player.error(), None);
            player.stop();
        }
        assert_eq!(
            fetcher.calls.load(Ordering::SeqCst),
            1,
            "one download total"
        );
        assert_eq!(cache.stats().entries, 1);
    }
}
