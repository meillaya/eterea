//! eterea desktop shell, built on Rune.

use std::io::Write;

mod app;
mod images;
mod model;
mod motion;
mod settings;
mod theme;

fn main() {
    // The app icon is window/taskbar metadata on macOS and Windows; Linux
    // packages carry it as a desktop-entry icon. Failure is never fatal.
    let _ = rune::set_app_icon(include_bytes!("../../../docs/assets/brand/icon-256.png").to_vec());
    let lock = acquire_instance_lock();
    let options = window_options();
    rune::App::new(app::spec())
        .window_options(options)
        .on_close(app::persist_window_on_close)
        .run();
    if let Some(path) = lock {
        let _ = std::fs::remove_file(path);
    }
}

/// Refuse a second window on the same archive/profile.
///
/// The lock is a pid file; a stale pid (crashed process) is reclaimed. Runs
/// with `ETEREA_CONFIG_DIR`, `ETEREA_DB` or `ETEREA_DEMO` set (tests, headless
/// tooling) are isolated and skip the guard.
fn acquire_instance_lock() -> Option<std::path::PathBuf> {
    let path = settings::instance_lock_path()?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    for attempt in 0..2 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                let _ = write!(file, "{}", std::process::id());
                return Some(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempt == 0 => {
                let stale = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|pid| pid.trim().parse::<u32>().ok())
                    .map(|pid| !std::path::Path::new(&format!("/proc/{pid}")).exists())
                    .unwrap_or(true);
                if !stale {
                    eprintln!("eterea is already running");
                    std::process::exit(0);
                }
                let _ = std::fs::remove_file(&path);
            }
            Err(error) => {
                eprintln!("eterea could not use its lock file: {error}");
                return None;
            }
        }
    }
    None
}

/// Restore the last window geometry, except in tooling runs where `--size`
/// (and the harness) own the viewport.
fn window_options() -> rune::WindowOptions {
    let tooling = std::env::args().skip(1).any(|arg| {
        matches!(
            arg.split('=').next().unwrap_or_default(),
            "--snap"
                | "--tree"
                | "--script"
                | "--at"
                | "--size"
                | "--theme"
                | "--scale"
                | "--video"
                | "--strip"
                | "--trace"
                | "--frame"
        )
    });
    if tooling {
        return rune::WindowOptions::default();
    }
    let saved = settings::load().window;
    rune::WindowOptions {
        position: saved.map(|geometry| (geometry.x, geometry.y)),
        physical_size: saved.map(|geometry| (geometry.width as u32, geometry.height as u32)),
        ..Default::default()
    }
}
