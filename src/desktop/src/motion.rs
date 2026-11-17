//! Named motion for the Daylight surface.
//!
//! Springs are durations and bounces, not curves: the runtime owns the curve.
//! When Reduce Motion is on, the view asks this module for instant springs and
//! transitions instead.

use rune::{Spring, SpringPair, Transition};

pub const SELECTION: Spring = Spring::new(0.12, 0.0);
pub const DETAIL: Spring = Spring::new(0.16, 0.0);
pub const HEADER: Spring = Spring::new(0.16, 0.0);
pub const OVERLAY: Spring = Spring::new(0.14, 0.0);
pub const LIGHTBOX: Spring = Spring::new(0.18, 0.0);
pub const MEDIA: Spring = Spring::new(0.18, 0.0);
pub const STATUS: Spring = Spring::new(0.12, 0.0);
pub const CAROUSEL: Spring = Spring::new(0.16, 0.08);
pub const GRAPH: Spring = Spring::new(0.12, 0.0);

pub fn spring(reduced: bool, normal: Spring) -> Spring {
    if reduced {
        Spring::NONE
    } else {
        normal
    }
}

pub fn route_enter(reduced: bool, normal: Spring) -> Transition {
    if reduced {
        Transition::none()
    } else {
        Transition::offset(0.0, 4.0)
            .combined(Transition::opacity())
            .springs(SpringPair::new(
                normal,
                Spring::new(normal.duration * 0.8, normal.bounce),
            ))
    }
}

pub fn overlay_enter(reduced: bool) -> Transition {
    if reduced {
        Transition::none()
    } else {
        Transition::opacity().springs(SpringPair::new(OVERLAY, Spring::new(0.12, 0.0)))
    }
}
