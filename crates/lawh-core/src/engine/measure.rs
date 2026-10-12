//! Object bounds, for motions and placing objects. Text and math take their size from how they
//! are laid out, which this crate leaves to the renderer behind [`Measure`].

use kurbo::Rect;

use crate::board::{Object, Shape};

/// Tells the engine the bounds of objects, in grid units.
///
/// Measuring counts against the work a key may do
/// ([`MAX_KEY_WORK`](super::MAX_KEY_WORK)). The engine counts a fixed amount for every object
/// and every character it has measured (`MEASURE_WORK` in `work.rs`, measured with
/// [`FixedMeasure`]); a measure that can do more than that, such as a renderer laying out a
/// text it has not laid out before, says how much more with [`Measure::take_work`].
pub trait Measure {
    /// The box `object` covers on the board, or `None` when it cannot be measured.
    fn bounds(&mut self, object: &Object) -> Option<Rect>;

    /// The work [`Measure::bounds`] did since this was last called, beyond what the engine
    /// counts for it, in units of [`MAX_KEY_WORK`](super::MAX_KEY_WORK) (about a nanosecond of
    /// work each), and starts counting again from nothing.
    ///
    /// The engine asks after every time it measures, and counts the answer against the key
    /// being run. It also asks when a key, a timeout or a click starts, and drops the answer:
    /// measuring done between keys (for drawing, say) is no key's work.
    ///
    /// The default is 0, for a measure whose work the engine's own count covers, as
    /// [`FixedMeasure`]'s does.
    fn take_work(&mut self) -> usize {
        0
    }
}

/// The bounds of shapes that need no layout: ellipses, rectangles and the box between a line's
/// or an arrow's ends. `None` for text and math.
pub fn shape_bounds(shape: &Shape) -> Option<Rect> {
    match shape {
        Shape::Ellipse { center, rx, ry } => Some(Rect::new(
            center.x - rx,
            center.y - ry,
            center.x + rx,
            center.y + ry,
        )),
        Shape::Rect { origin, w, h } => {
            Some(Rect::new(origin.x, origin.y, origin.x + w, origin.y + h))
        }
        Shape::Line { a, b } | Shape::Arrow { a, b } => Some(Rect::from_points(*a, *b)),
        Shape::Text { .. } | Shape::Math { .. } => None,
    }
}

/// Bounds without a renderer, for tests and headless use: shapes exactly, text and math as a
/// fixed-width grid of [`FixedMeasure::CHAR_WIDTH`] by [`FixedMeasure::LINE_HEIGHT`] per
/// character, both times the object's size. An empty text is one line of width 0.
#[derive(Debug, Clone, Copy, Default)]
pub struct FixedMeasure;

impl FixedMeasure {
    /// Width of one character at size 1, in grid units.
    pub const CHAR_WIDTH: f64 = 0.3;
    /// Height of one line at size 1, in grid units.
    pub const LINE_HEIGHT: f64 = 0.6;
}

impl Measure for FixedMeasure {
    fn bounds(&mut self, object: &Object) -> Option<Rect> {
        let (at, content, size) = match &object.shape {
            Shape::Text { at, text, size } => (*at, text, *size),
            Shape::Math { at, src, size } => (*at, src, *size),
            shape => return shape_bounds(shape),
        };
        let (longest, lines) = text_size(content);
        let extent = (
            longest as f64 * Self::CHAR_WIDTH * size,
            lines as f64 * Self::LINE_HEIGHT * size,
        );
        Some(Rect::from_origin_size(at, extent))
    }
}

/// Characters in the longest line of `s` and its number of lines, breaking lines where the
/// board counts line breaks (see [`MAX_TEXT_LINES`](crate::board::MAX_TEXT_LINES)).
fn text_size(s: &str) -> (usize, usize) {
    let (mut longest, mut lines, mut line) = (0, 1, 0);
    let mut after_cr = false;
    for c in s.chars() {
        match c {
            '\n' if after_cr => {}
            '\n' | '\r' | '\u{B}' | '\u{C}' | '\u{85}' | '\u{2028}' | '\u{2029}' => {
                longest = longest.max(line);
                line = 0;
                lines += 1;
            }
            _ => line += 1,
        }
        after_cr = c == '\r';
    }
    (longest.max(line), lines)
}

#[cfg(test)]
mod tests {
    use kurbo::Point;

    use super::*;
    use crate::board::{DEFAULT_TEXT_SIZE, ObjectId};

    fn object(shape: Shape) -> Object {
        Object::new(ObjectId::new("o1").unwrap(), shape)
    }

    #[test]
    fn shapes_are_measured_exactly() {
        let ellipse = Shape::Ellipse {
            center: Point::new(1.0, 2.0),
            rx: 3.0,
            ry: 0.5,
        };
        assert_eq!(shape_bounds(&ellipse), Some(Rect::new(-2.0, 1.5, 4.0, 2.5)));
        let rect = Shape::Rect {
            origin: Point::new(1.0, 2.0),
            w: 4.0,
            h: 2.0,
        };
        assert_eq!(shape_bounds(&rect), Some(Rect::new(1.0, 2.0, 5.0, 4.0)));
        // A line from right to left still has a positive box.
        let (a, b) = (Point::new(3.0, 0.0), Point::new(-1.0, 2.0));
        let expected = Some(Rect::new(-1.0, 0.0, 3.0, 2.0));
        assert_eq!(shape_bounds(&Shape::Line { a, b }), expected);
        assert_eq!(shape_bounds(&Shape::Arrow { a, b }), expected);
        let text = Shape::Text {
            at: Point::ZERO,
            text: "x".to_owned(),
            size: DEFAULT_TEXT_SIZE,
        };
        assert_eq!(shape_bounds(&text), None);
        assert_eq!(
            FixedMeasure.bounds(&object(rect)),
            Some(Rect::new(1.0, 2.0, 5.0, 4.0))
        );
    }

    /// The bounds of a text and a math object of `content` at size `size` and (1, 1), which
    /// must be the same.
    fn measure_sized(content: &str, size: f64) -> Option<(f64, f64, f64, f64)> {
        let text = FixedMeasure.bounds(&object(Shape::Text {
            at: Point::new(1.0, 1.0),
            text: content.to_owned(),
            size,
        }));
        let math = FixedMeasure.bounds(&object(Shape::Math {
            at: Point::new(1.0, 1.0),
            src: content.to_owned(),
            size,
        }));
        assert_eq!(text, math);
        text.map(|r| (r.x0, r.y0, r.x1, r.y1))
    }

    #[test]
    fn fixed_measure_sizes_text_by_characters_and_lines() {
        let measure = |content: &str| measure_sized(content, DEFAULT_TEXT_SIZE);
        // `n` characters by `lines` lines, at (1, 1).
        let size = |n: f64, lines: f64| Some((1.0, 1.0, 1.0 + n * 0.3, 1.0 + lines * 0.6));
        assert_eq!(measure(""), size(0.0, 1.0));
        assert_eq!(measure("abcd"), size(4.0, 1.0));
        // Characters, not bytes; CRLF is one break; the longest line sets the width.
        assert_eq!(measure("é\r\nabc\n"), size(3.0, 3.0));
        assert_eq!(measure("ab\r\rc\u{2028}"), size(2.0, 4.0));
    }

    #[test]
    fn fixed_measure_scales_with_the_size() {
        // 4 characters by 2 lines, at (1, 1): the corner stays, the extent scales.
        for (size, w, h) in [(0.25, 0.3, 0.3), (2.0, 2.4, 2.4), (16.0, 19.2, 19.2)] {
            assert_eq!(
                measure_sized("abcd\nab", size),
                Some((1.0, 1.0, 1.0 + w, 1.0 + h)),
                "{size}"
            );
        }
    }
}
