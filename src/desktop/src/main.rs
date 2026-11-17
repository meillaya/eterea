//! eterea desktop shell, built on Rune.

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
    rune::run(app::spec());
}
