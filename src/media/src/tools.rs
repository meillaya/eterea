//! Discovery of the external `ffmpeg` and `ffplay` executables.
//!
//! eterea shells out to ffmpeg for video frames and to ffplay for sound instead
//! of linking a native audio library. This module answers "is playback
//! possible on this machine?" with typed, UI-friendly status. Discovery is
//! explicit and side-effect free unless [`MediaTools::probe_versions`] is
//! called: constructing the media crate never requires `PATH` to be sane.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::MediaError;

/// Availability of one external media tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStatus {
    /// Whether a usable executable was found.
    pub available: bool,
    /// Path of the executable, when available.
    pub path: Option<PathBuf>,
    /// First line of `<tool> -version`, only filled by [`MediaTools::probe_versions`].
    pub version: Option<String>,
    /// Actionable explanation for the UI when the tool is missing.
    pub message: Option<String>,
}

impl ToolStatus {
    /// Build an available status for `path`.
    pub fn found(path: impl Into<PathBuf>) -> Self {
        Self {
            available: true,
            path: Some(path.into()),
            version: None,
            message: None,
        }
    }

    /// Build an unavailable status with an actionable `message`.
    pub fn missing(message: impl Into<String>) -> Self {
        Self {
            available: false,
            path: None,
            version: None,
            message: Some(message.into()),
        }
    }

    /// One-line status for settings or a diagnostics screen.
    pub fn describe(&self) -> String {
        match (&self.path, &self.message) {
            (Some(path), _) => match &self.version {
                Some(version) => format!("{} ({version})", path.display()),
                None => path.display().to_string(),
            },
            (None, Some(message)) => message.clone(),
            (None, None) => "not found".to_string(),
        }
    }
}

/// Discovered ffmpeg and ffplay executables.
///
/// `MediaTools::discover()` never spawns a process and never fails: a machine
/// without either tool simply reports unavailable statuses with an actionable
/// message. [`VideoPlayer`](crate::VideoPlayer) returns
/// [`MediaError::DecoderUnavailable`] when ffmpeg is missing and
/// [`MediaError::AudioUnavailable`] when ffplay is missing at unmute time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaTools {
    /// Discovery status of the `ffmpeg` binary (frames and duration probing).
    pub ffmpeg: ToolStatus,
    /// Discovery status of the `ffplay` binary (audio playback).
    pub ffplay: ToolStatus,
}

impl Default for MediaTools {
    fn default() -> Self {
        Self {
            ffmpeg: ToolStatus::missing("ffmpeg has not been discovered"),
            ffplay: ToolStatus::missing("ffplay has not been discovered"),
        }
    }
}

impl MediaTools {
    /// Search the process `PATH` for `ffmpeg` and `ffplay`.
    ///
    /// No process is spawned, so this is cheap enough to call from a settings
    /// screen. A missing or empty `PATH` yields unavailable statuses instead of
    /// an error.
    pub fn discover() -> Self {
        Self::discover_with_path(std::env::var_os("PATH").as_deref())
    }

    /// Search `path` instead of the process `PATH`; primarily for tests.
    ///
    /// `None` means the variable is not set at all.
    pub fn discover_with_path(path: Option<&OsStr>) -> Self {
        Self {
            ffmpeg: discover_tool("ffmpeg", path),
            ffplay: discover_tool("ffplay", path),
        }
    }

    /// Build a status from explicit paths, checking that each is an executable
    /// file. Used by the desktop when settings point at a specific binary and
    /// by tests that inject fake commands.
    pub fn from_paths(ffmpeg: impl Into<PathBuf>, ffplay: Option<PathBuf>) -> Self {
        Self {
            ffmpeg: status_for_explicit_path("ffmpeg", ffmpeg.into()),
            ffplay: match ffplay {
                Some(path) => status_for_explicit_path("ffplay", path),
                None => ToolStatus::missing(
                    "ffplay not found: install ffmpeg (it includes ffplay) or add it to PATH",
                ),
            },
        }
    }

    /// Whether `ffmpeg` is available for frame decoding.
    pub fn ffmpeg_available(&self) -> bool {
        self.ffmpeg.available
    }

    /// Whether `ffplay` is available for audio playback.
    pub fn ffplay_available(&self) -> bool {
        self.ffplay.available
    }

    /// The ffmpeg path, or a typed actionable error.
    pub fn ffmpeg(&self) -> Result<&Path, MediaError> {
        match self.ffmpeg.path.as_deref() {
            Some(path) => Ok(path),
            None => Err(MediaError::DecoderUnavailable),
        }
    }

    /// The ffplay path, or a typed actionable error.
    pub fn ffplay(&self) -> Result<&Path, MediaError> {
        match self.ffplay.path.as_deref() {
            Some(path) => Ok(path),
            None => Err(MediaError::AudioUnavailable),
        }
    }

    /// Run `<tool> -version` once for each available tool and record the first
    /// line. This is the only method that spawns processes; discovery itself
    /// never does.
    pub fn probe_versions(mut self) -> Self {
        self.ffmpeg = probe_version(self.ffmpeg);
        self.ffplay = probe_version(self.ffplay);
        self
    }
}

fn discover_tool(name: &str, path: Option<&OsStr>) -> ToolStatus {
    let Some(path) = path else {
        return ToolStatus::missing(format!(
            "{name} not found: install ffmpeg or add it to PATH"
        ));
    };
    for directory in std::env::split_paths(path) {
        let candidate = directory.join(name);
        if is_executable_file(&candidate) {
            return ToolStatus::found(candidate);
        }
    }
    ToolStatus::missing(format!(
        "{name} not found: install ffmpeg or add it to PATH"
    ))
}

fn status_for_explicit_path(name: &str, path: PathBuf) -> ToolStatus {
    if is_executable_file(&path) {
        ToolStatus::found(path)
    } else {
        ToolStatus::missing(format!(
            "{name} not found at {}: install ffmpeg or pass a valid path",
            path.display()
        ))
    }
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(metadata) => metadata.is_file() && metadata.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

fn probe_version(mut status: ToolStatus) -> ToolStatus {
    let Some(path) = status.path.clone() else {
        return status;
    };
    match Command::new(&path).arg("-version").output() {
        Ok(output) => {
            let text = if output.stdout.is_empty() {
                String::from_utf8_lossy(&output.stderr).into_owned()
            } else {
                String::from_utf8_lossy(&output.stdout).into_owned()
            };
            status.version = text.lines().next().map(str::trim).map(str::to_owned);
            if !output.status.success() && status.version.is_none() {
                status.available = false;
                status.path = None;
                status.message = Some(format!(
                    "{} could not run: install ffmpeg or pass a valid path",
                    path.display()
                ));
            }
            status
        }
        Err(_) => {
            status.available = false;
            status.path = None;
            status.message = Some(format!(
                "{} could not run: install ffmpeg or pass a valid path",
                path.display()
            ));
            status
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MediaTools;
    use crate::test_support::{write_script, ScratchDir};
    use crate::MediaError;

    #[test]
    fn empty_path_reports_unavailable_with_actionable_messages() {
        let scratch = ScratchDir::new("tools-empty-path");
        let tools = MediaTools::discover_with_path(Some(scratch.path().as_os_str()));
        assert!(!tools.ffmpeg_available());
        assert!(!tools.ffplay_available());
        assert!(tools.ffmpeg.message.as_deref().unwrap().contains("ffmpeg"));
        assert!(tools.ffplay.message.as_deref().unwrap().contains("ffplay"));
        assert_eq!(tools.ffmpeg(), Err(MediaError::DecoderUnavailable));
        assert_eq!(tools.ffplay(), Err(MediaError::AudioUnavailable));
        assert!(tools.ffmpeg.describe().contains("PATH"));
    }

    #[test]
    fn path_is_not_set_at_all() {
        let tools = MediaTools::discover_with_path(None);
        assert!(!tools.ffmpeg_available());
        assert!(!tools.ffplay_available());
    }

    #[cfg(unix)]
    #[test]
    fn discovers_executables_on_the_given_path() {
        let scratch = ScratchDir::new("tools-found");
        let ffmpeg = write_script(scratch.path(), "ffmpeg", "#!/bin/sh\nexit 0\n");
        let ffplay = write_script(scratch.path(), "ffplay", "#!/bin/sh\nexit 0\n");
        let tools = MediaTools::discover_with_path(Some(scratch.path().as_os_str()));
        assert!(tools.ffmpeg_available());
        assert!(tools.ffplay_available());
        assert_eq!(tools.ffmpeg().unwrap(), ffmpeg);
        assert_eq!(tools.ffplay().unwrap(), ffplay);
    }

    #[cfg(unix)]
    #[test]
    fn explicit_missing_path_is_typed_and_actionable() {
        let scratch = ScratchDir::new("tools-explicit-missing");
        let missing = scratch.path().join("ffmpeg-does-not-exist");
        let tools = MediaTools::from_paths(&missing, None);
        assert!(!tools.ffmpeg_available());
        assert_eq!(tools.ffmpeg(), Err(MediaError::DecoderUnavailable));
        let message = tools.ffmpeg.message.as_deref().unwrap();
        assert!(message.contains("ffmpeg"));
        assert!(message.contains("install ffmpeg"));
        assert_eq!(
            MediaError::DecoderUnavailable.to_string(),
            "ffmpeg not found: install ffmpeg or add it to PATH"
        );
        assert_eq!(
            MediaError::AudioUnavailable.to_string(),
            "ffplay not found: install ffmpeg (it includes ffplay) or add it to PATH"
        );
    }

    #[cfg(unix)]
    #[test]
    fn probing_versions_runs_the_tool_once() {
        let _process = crate::test_support::process_test_lock();
        let scratch = ScratchDir::new("tools-version");
        let ffmpeg = write_script(
            scratch.path(),
            "ffmpeg",
            "#!/bin/sh\necho 'ffmpeg version 9.0.2-test'\nexit 0\n",
        );
        let tools = MediaTools::from_paths(&ffmpeg, None).probe_versions();
        assert_eq!(
            tools.ffmpeg.version.as_deref(),
            Some("ffmpeg version 9.0.2-test"),
            "unexpected tool status: {:?}",
            tools.ffmpeg
        );
        assert!(tools.ffmpeg_available());
        assert!(tools.ffmpeg.describe().contains("9.0.2-test"));
    }

    #[test]
    fn from_paths_without_ffplay_keeps_frames_usable() {
        let tools = MediaTools::from_paths("/bin/sh", None);
        assert!(tools.ffmpeg_available());
        assert!(!tools.ffplay_available());
    }
}
