//! Helpers shared by the render tests.

#![allow(dead_code, reason = "each test binary uses a different subset")]

use lawh_core::{Board, from_lawh};

const AREA: &str = include_str!("../../../../examples/area.lawh");

/// The "area of a circle" example board from `examples/area.lawh`.
pub fn area_board() -> Board {
    from_lawh(AREA).unwrap()
}

/// A board from `.lawh` source.
pub fn board(src: &str) -> Board {
    from_lawh(src).unwrap()
}

/// Premultiplied RGBA8 pixel at `(x, y)` of a `width`-wide buffer.
pub fn pixel(buf: &[u8], width: u16, x: u16, y: u16) -> [u8; 4] {
    let i = (usize::from(y) * usize::from(width) + usize::from(x)) * 4;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}
