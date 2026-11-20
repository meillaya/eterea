//! Test-only scratch directories for cache and manager tests.
//!
//! Scratch paths are deterministic, unique per process and label, and always
//! stay inside the workspace: under `$CARGO_TARGET_DIR/media-test-scratch` when
//! that variable is present, otherwise under `<workspace>/.work`. The
//! [`ScratchDir`] guard removes its directory on drop, including on panic.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Unique temporary directory inside the workspace.
pub(crate) struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    pub(crate) fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = scratch_root().join(format!("{label}-{}-{sequence}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create scratch directory");
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

static PROCESS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Serialize tests that spawn external processes.
///
/// The suite runs in parallel; spawning many shell/fake-decoder processes at
/// once can transiently fail under a busy machine (EAGAIN). Production spawns
/// one decoder and one audio process, so serializing the process-based tests
/// keeps them deterministic without weakening what they cover.
pub(crate) fn process_test_lock() -> std::sync::MutexGuard<'static, ()> {
    PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Write an executable shell script under `dir` and return its path.
///
/// Only available on Unix; the whole media test suite runs on the Linux
/// desktop target. Scripts ignore their arguments by convention so the same
/// fake can stand in for ffmpeg or ffplay.
#[cfg(unix)]
pub(crate) fn write_script(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join(name);
    std::fs::write(&path, body).expect("write test script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("make test script executable");
    path
}

fn scratch_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        return PathBuf::from(dir).join("media-test-scratch");
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.work/media-test-scratch")
}

// ---------------------------------------------------------------------------
// Generated image fixtures for the decode/manager tests.
//
// The tests never touch the network: these helpers encode tiny images in
// memory with the `image` crate. Animated fixtures are assembled by hand for
// the formats the encoder cannot animate (APNG, animated WebP), which is enough
// for header sniffing because the decode path is not expected to run on them.
// ---------------------------------------------------------------------------

/// A solid-colour RGBA8 PNG, encoded in memory.
pub(crate) fn png_bytes(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    use image::{ExtendedColorType, ImageEncoder};
    let pixels = solid_rgba(width, height, rgba);
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&pixels, width, height, ExtendedColorType::Rgba8)
        .expect("encode png fixture");
    out
}

/// A solid-colour JPEG, encoded in memory.
pub(crate) fn jpeg_bytes(width: u32, height: u32) -> Vec<u8> {
    use image::{ExtendedColorType, ImageEncoder};
    // JPEG has no alpha channel; the encoder accepts RGB8 directly.
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 3);
    for _ in 0..width as usize * height as usize {
        pixels.extend_from_slice(&[120, 80, 40]);
    }
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
        .write_image(&pixels, width, height, ExtendedColorType::Rgb8)
        .expect("encode jpeg fixture");
    out
}

/// A solid-colour lossless WebP (VP8L), encoded in memory.
pub(crate) fn webp_bytes(width: u32, height: u32) -> Vec<u8> {
    use image::{ExtendedColorType, ImageEncoder};
    let pixels = solid_rgba(width, height, [40, 160, 90, 255]);
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .write_image(&pixels, width, height, ExtendedColorType::Rgba8)
        .expect("encode webp fixture");
    out
}

/// A real multi-frame GIF written by the `image` crate encoder.
pub(crate) fn animated_gif_bytes(width: u32, height: u32, frames: u32) -> Vec<u8> {
    use image::{Frame, Rgba, RgbaImage};
    let mut out = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut out);
        for frame in 0..frames {
            let shade = 40 + (frame as u8).saturating_mul(20);
            let image = RgbaImage::from_pixel(width, height, Rgba([shade, 20, 20, 255]));
            encoder
                .encode_frame(Frame::new(image))
                .expect("encode gif frame");
        }
    }
    out
}

/// A header-only APNG: PNG signature, IHDR, acTL and IEND.
///
/// The decoder never runs on animated payloads, so a static IDAT is not needed
/// to exercise the animation sniffing and the conservative estimate.
pub(crate) fn apng_bytes(width: u32, height: u32, frames: u32) -> Vec<u8> {
    let mut out = PNG_SIGNATURE.to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    // Bit depth 8, colour type 6 (RGBA), deflate, adaptive filtering, no interlace.
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    out.extend_from_slice(&png_chunk(b"IHDR", &ihdr));
    let mut actl = Vec::new();
    actl.extend_from_slice(&frames.to_be_bytes());
    actl.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&png_chunk(b"acTL", &actl));
    out.extend_from_slice(&png_chunk(b"IEND", &[]));
    out
}

/// A header-only animated WebP: RIFF container with VP8X, ANIM and ANMF chunks.
pub(crate) fn animated_webp_bytes(width: u32, height: u32, frames: u32) -> Vec<u8> {
    let mut vp8x = Vec::new();
    // Animation flag (bit 1); reserved bytes; canvas dimensions minus one.
    vp8x.push(0x02);
    vp8x.extend_from_slice(&[0, 0, 0]);
    vp8x.extend_from_slice(&u24_le(width.saturating_sub(1)));
    vp8x.extend_from_slice(&u24_le(height.saturating_sub(1)));
    let mut anim = Vec::new();
    anim.extend_from_slice(&[0, 0, 0, 0]); // background colour
    anim.extend_from_slice(&0u16.to_le_bytes()); // loop forever
    let mut body = webp_chunk(b"VP8X", &vp8x);
    body.extend_from_slice(&webp_chunk(b"ANIM", &anim));
    for frame in 0..frames {
        let mut anmf = Vec::new();
        anmf.extend_from_slice(&u24_le(0)); // frame x
        anmf.extend_from_slice(&u24_le(0)); // frame y
        anmf.extend_from_slice(&u24_le(width.saturating_sub(1)));
        anmf.extend_from_slice(&u24_le(height.saturating_sub(1)));
        anmf.extend_from_slice(&u24_le(100 + frame)); // duration
        anmf.push(0); // blend/dispose flags
        body.extend_from_slice(&webp_chunk(b"ANMF", &anmf));
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(4 + body.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    out
}

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

fn solid_rgba(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for _ in 0..width as usize * height as usize {
        pixels.extend_from_slice(&rgba);
    }
    pixels
}

fn u24_le(value: u32) -> [u8; 3] {
    let bytes = value.to_le_bytes();
    [bytes[0], bytes[1], bytes[2]]
}

/// One RIFF-style chunk with its 8-byte header and even-length padding.
fn webp_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + data.len() + (data.len() & 1));
    out.extend_from_slice(kind);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() & 1 == 1 {
        out.push(0);
    }
    out
}

/// One PNG chunk with its CRC-32.
fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + data.len());
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    out
}

/// Standard CRC-32 (polynomial `0xEDB88320`) used by PNG chunks.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}
