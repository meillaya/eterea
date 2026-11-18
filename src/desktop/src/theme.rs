//! Daylight palette and type styles.

use std::sync::atomic::{AtomicU32, Ordering};

use rune::color;
use rune::{Color, Family, TextStyle};

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color,
    pub ink: Color,
    pub muted: Color,
    pub line: Color,
    pub accent: Color,
    /// Quiet lavender used by the media-repair progress fill.
    pub progress: Color,
    pub quote_rule: Color,
    pub band: Color,
    #[allow(dead_code)]
    pub scrim: Color,
}

pub fn light() -> Palette {
    Palette {
        bg: color::hex("#f7f6f3").unwrap(),
        ink: color::hex("#1a1a18").unwrap(),
        muted: color::hex("#787770").unwrap(),
        line: color::hex("#dedcd6").unwrap(),
        accent: color::hex("#2447b8").unwrap(),
        progress: color::hex("#b3a4e8").unwrap(),
        quote_rule: color::hex("#c9c6bf").unwrap(),
        band: color::hex("#efeee9").unwrap(),
        scrim: color::hex("#0d0d0c").unwrap().alpha(0.86),
    }
}

#[allow(dead_code)]
pub fn night() -> Palette {
    Palette {
        bg: color::hex("#141412").unwrap(),
        ink: color::hex("#ecebe4").unwrap(),
        muted: color::hex("#98958b").unwrap(),
        line: color::hex("#2a2a25").unwrap(),
        accent: color::hex("#8fa8ff").unwrap(),
        progress: color::hex("#c9bcff").unwrap(),
        quote_rule: color::hex("#3a3a33").unwrap(),
        band: color::hex("#1c1d19").unwrap(),
        scrim: color::hex("#050505").unwrap().alpha(0.9),
    }
}

static TEXT_SCALE: AtomicU32 = AtomicU32::new(f32::to_bits(1.0));

pub fn set_text_scale(scale: f32) {
    TEXT_SCALE.store(f32::to_bits(scale), Ordering::Relaxed);
}

fn scaled(size: f32) -> f32 {
    size * f32::from_bits(TEXT_SCALE.load(Ordering::Relaxed))
}

pub fn serif(size: f32) -> TextStyle {
    TextStyle::new(scaled(size), 400.0).family(Family::Named("Newsreader"))
}

pub fn ui(size: f32, weight: f32) -> TextStyle {
    TextStyle::new(scaled(size), weight).family(Family::Named("Instrument Sans"))
}

pub fn mono(size: f32) -> TextStyle {
    TextStyle::new(scaled(size), 400.0).family(Family::Named("IBM Plex Mono"))
}
