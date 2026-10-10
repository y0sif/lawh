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

/// A board at the limits `lawh-core` allows: shapes as large and as far out as a `.lawh` file
/// may hold them (coordinates ±1e6, sizes 1e6), one text of the longest length on one line and
/// one spread over many lines, and math at the far edge.
pub fn limits_board() -> Board {
    use lawh_core::board::MAX_TEXT_CHARS;
    let words = "lorem ipsum dolor sit amet ".repeat(MAX_TEXT_CHARS / 27 + 1);
    let one_line: String = words.chars().take(MAX_TEXT_CHARS).collect();
    let many_lines = "line\\n".repeat(MAX_TEXT_CHARS / 5);
    from_lawh(&format!(
        "lawh version=1\n\
         ellipse id=e1 x=0 y=0 rx=1000000 ry=1000000\n\
         ellipse id=e2 x=1000000 y=1000000 rx=1000000 ry=500000\n\
         rect id=r1 x=-1000000 y=-1000000 w=1000000 h=1000000\n\
         rect id=r2 x=0 y=0 w=1000000 h=1000000\n\
         line id=l1 x1=-1000000 y1=-1000000 x2=1000000 y2=1000000\n\
         arrow id=a1 x1=-1000000 y1=1000000 x2=1000000 y2=-1000000\n\
         text id=t1 x=-1000000 y=0 \"{one_line}\"\n\
         text id=t2 x=0 y=-1000000 \"{many_lines}\"\n\
         math id=m1 x=1000000 y=0 \"A = pi r^2\"\n"
    ))
    .unwrap()
}

/// Board points worth looking at on [`limits_board`]: where its huge shapes cross, and its
/// far corners and edges.
pub const LIMITS_CENTERS: [(f64, f64); 7] = [
    (0.0, 0.0),
    (1e6, 0.0),
    (-1e6, 0.0),
    (0.0, -1e6),
    (707_106.781_186_547_5, 707_106.781_186_547_5),
    (1e6, 1e6),
    (-1e6, -1e6),
];
