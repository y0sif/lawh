//! Jump labels ([`Overlay::labels`](crate::Overlay::labels)): while `s` waits for a label, a
//! small box on each object it can jump to, with the label to type in it.
//!
//! The boxes are pixel-aligned rectangles, which `vello_cpu` fills fastest, and the letters
//! are one glyph run per font and colour (the characters typed so far are dimmer than the ones
//! left to type), so however many labels there are, the letters take a fixed number of glyph
//! runs. Nearer labels go over farther ones: the boxes are all one colour, so they look the same
//! filled in any order, and a farther label's letter is left out where a nearer label's box
//! covers it.
//!
//! Cost: every label is drawn, whatever it costs, or it couldn't be typed. 500 two-letter labels
//! spread over the window add about 2.7 ms to a frame at 1920x1080 and 4.1 ms at 2880x1800 with
//! scale factor 2, two thirds of it the letters (release, Linux `x86_64`, 2026-10-12). Filling
//! the letters as one path of outlines read from the font once, or the boxes as one path, was no
//! faster. So the labels' estimated cost ([`Renderer::labels_cost`]) is taken from the budget of
//! the Visual selection's highlights (`HIGHLIGHT_BUDGET_NS`), drawn under them: with labels on
//! hundreds of objects, fewer selected objects are outlined (the Visual rectangle still shows
//! the selection), and the overlay stays within its target (`OVERLAY_TARGET` in
//! `tests/bench.rs`).

use kurbo::{Affine, Rect, Shape as _};
use lawh_core::engine::JumpLabel;
use vello_cpu::Glyph;

use super::{CURSOR, LABEL_LEFT, LABEL_TYPED, Renderer, to_f32};
use crate::budget::Estimate;
use crate::overlay::status_bar_height_px;
use crate::{Viewport, draw};

// Sizes in logical pixels, times the scale factor on screen, so labels read the same at any
// zoom.

/// Height of a label's box, and its least width: the box of a one-letter label is square.
const LABEL_BOX_PX: f64 = 14.0;
/// Size (em) of a label's letters.
const LABEL_FONT_PX: f64 = 11.0;
/// Space between a label's letters and the sides of its box.
const LABEL_PAD_PX: f64 = 3.0;

/// Where a label's letters come from, by index into `lawh_math::fonts`: the text font, then the
/// math font for a character the text font lacks (`:set jumplabels` takes any character).
const LABEL_FONTS: [usize; 2] = [0, 1];

/// Jump labels placed on screen ([`Renderer::place_labels`]), to estimate and draw.
pub(super) struct PlacedLabels {
    /// Every label's box, nearest first.
    boxes: Vec<Rect>,
    /// The letters drawn: not those under a nearer label's box.
    letters: Vec<Letter>,
    /// Pixels to the em of the letters.
    font_px: f64,
}

/// One letter of a label, placed on screen.
struct Letter {
    /// Index into `lawh_math::fonts`.
    font: usize,
    /// Typed already: drawn dimmer.
    typed: bool,
    glyph: Glyph,
    /// Where the letter is: its advance across, the box's height.
    cell: Rect,
}

impl Renderer {
    /// Places the labels of `labels` (nearest object first) that start with `typed`, the
    /// characters typed so far, on the window seen through `vp`.
    pub(super) fn place_labels(
        &self,
        vp: &Viewport,
        labels: &[JumpLabel],
        typed: &str,
    ) -> PlacedLabels {
        let sf = vp.scale_factor;
        let (width, height) = (f64::from(vp.width), f64::from(vp.height));
        let bottom = (height - f64::from(status_bar_height_px(sf))).max(0.0);
        let box_height = (LABEL_BOX_PX * sf).round();
        let (font_px, pad) = (LABEL_FONT_PX * sf, LABEL_PAD_PX * sf);
        // The baseline sits below the box's middle by a quarter of the cap height and the
        // x-height together, so lowercase and capital labels both look centred.
        let (cap, x_height) = self.glyphs.heights(LABEL_FONTS[0]).unwrap_or((0.7, 0.5));
        let below_middle = (cap + x_height) / 4.0 * font_px;
        let to_screen = vp.transform();

        // Nearest first: a label's letters are checked against the boxes before it.
        let mut boxes: Vec<Rect> = Vec::new();
        let mut letters: Vec<Letter> = Vec::new();
        for label in labels.iter().filter(|label| label.text.starts_with(typed)) {
            let anchor = to_screen * label.at;
            if !anchor.is_finite() {
                continue;
            }
            let first = letters.len();
            let mut advance = 0.0;
            for (byte, c) in label.text.char_indices() {
                let (font, id, em) = self.label_glyph(c);
                let x = advance;
                advance += em * font_px;
                letters.push(Letter {
                    font,
                    typed: byte < typed.len(),
                    glyph: Glyph { id, x: 0.0, y: 0.0 },
                    cell: Rect::new(x, 0.0, advance, 0.0),
                });
            }
            let box_width = (advance + 2.0 * pad).max(box_height).ceil();
            // Centred on the anchor, then moved inside the window above the status bar, on
            // whole pixels so its edges are sharp.
            let x0 = (anchor.x - box_width / 2.0)
                .min(width - box_width)
                .max(0.0)
                .round();
            let y0 = (anchor.y - box_height / 2.0)
                .min(bottom - box_height)
                .max(0.0)
                .round();
            let bounds = Rect::new(x0, y0, x0 + box_width, y0 + box_height);
            let left = x0 + (box_width - advance) / 2.0;
            let baseline = y0 + box_height / 2.0 + below_middle;
            let covering: Vec<Rect> = boxes
                .iter()
                .filter(|nearer| overlap(**nearer, bounds))
                .copied()
                .collect();
            let mut kept = first;
            for index in first..letters.len() {
                let Some(letter) = letters.get_mut(index) else {
                    continue;
                };
                letter.cell = Rect::new(
                    left + letter.cell.x0,
                    bounds.y0,
                    left + letter.cell.x1,
                    bounds.y1,
                );
                letter.glyph.x = to_f32(letter.cell.x0);
                letter.glyph.y = to_f32(baseline);
                let cell = letter.cell;
                if !covering.iter().any(|nearer| overlap(*nearer, cell)) {
                    letters.swap(kept, index);
                    kept += 1;
                }
            }
            letters.truncate(kept);
            boxes.push(bounds);
        }
        PlacedLabels {
            boxes,
            letters,
            font_px,
        }
    }

    /// The estimated cost of [`Self::draw_labels`] drawing `placed` through `vp`, in
    /// nanoseconds, as the frame budget estimates fills and glyphs (`crate::budget`).
    pub(super) fn labels_cost(&mut self, vp: &Viewport, placed: &PlacedLabels) -> f64 {
        let mut cost = Estimate::new(draw::clip_rect(vp), vp.width, vp.height, f64::INFINITY);
        for rect in &placed.boxes {
            cost.fill(&rect.to_path(draw::TOLERANCE_PX));
        }
        for font in LABEL_FONTS {
            for typed in [true, false] {
                let mut run = placed
                    .letters
                    .iter()
                    .filter(|letter| letter.font == font && letter.typed == typed)
                    .peekable();
                if run.peek().is_some() {
                    cost.run();
                }
                for letter in run {
                    let stats = self.glyphs.stats(font, letter.glyph.id);
                    cost.glyph(Some((stats, placed.font_px)));
                }
            }
        }
        cost.ns
    }

    /// Draws labels placed by [`Self::place_labels`].
    pub(super) fn draw_labels(&mut self, placed: &PlacedLabels) {
        if placed.boxes.is_empty() {
            return;
        }
        self.work.labels += placed.boxes.len();
        self.work.label_letters += placed.letters.len();
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.set_paint(CURSOR);
        for rect in &placed.boxes {
            self.ctx.fill_rect(rect);
        }
        for font in LABEL_FONTS {
            for typed in [true, false] {
                self.fill_letters(&placed.letters, font, typed, placed.font_px);
            }
        }
    }

    /// Fills the letters of `letters` from font `font` that are `typed` (or not), `font_px`
    /// pixels to the em, in one glyph run.
    fn fill_letters(&mut self, letters: &[Letter], font: usize, typed: bool, font_px: f64) {
        let mut picked = letters
            .iter()
            .filter(|letter| letter.font == font && letter.typed == typed)
            .peekable();
        if picked.peek().is_none() {
            return;
        }
        let Some(font_data) = self.math_fonts.get(font) else {
            return;
        };
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx
            .set_paint(if typed { LABEL_TYPED } else { LABEL_LEFT });
        let drawn = self
            .ctx
            .glyph_run(&mut self.resources, font_data)
            .font_size(to_f32(font_px))
            .hint(false)
            .fill_glyphs(picked.map(|letter| letter.glyph));
        self.work.label_runs += 1;
        if let Err(err) = drawn {
            tracing::debug!(?err, "some jump label glyphs failed to draw");
        }
    }

    /// The font, glyph and advance (in ems) `c` is drawn with in a label: from the first of
    /// [`LABEL_FONTS`] that has it, else the text font's missing-glyph box.
    fn label_glyph(&self, c: char) -> (usize, u32, f64) {
        LABEL_FONTS
            .iter()
            .find_map(|font| {
                self.glyphs
                    .char_glyph(*font, c)
                    .map(|(id, advance)| (*font, id, advance))
            })
            .unwrap_or_else(|| {
                let (id, advance) = self.glyphs.missing_glyph(LABEL_FONTS[0]);
                (LABEL_FONTS[0], id, advance)
            })
    }
}

/// Whether `a` and `b` share some area (touching edges don't count).
fn overlap(a: Rect, b: Rect) -> bool {
    a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
}

#[cfg(test)]
mod tests {
    use kurbo::Point;
    use lawh_core::{Board, ObjectId};

    use super::*;
    use crate::overlay::Overlay;

    fn label(name: &str, at: Point, text: &str) -> JumpLabel {
        JumpLabel {
            id: ObjectId::new(name).unwrap(),
            at,
            text: text.to_owned(),
        }
    }

    /// Draws `labels` with `typed` over an empty board through `vp`, and returns the image.
    fn draw(renderer: &mut Renderer, vp: &Viewport, labels: &[JumpLabel], typed: &str) -> Vec<u8> {
        let overlay = Overlay {
            labels: Some((labels, typed)),
            ..Overlay::default()
        };
        renderer.render_with(&Board::new(), vp, &overlay).to_vec()
    }

    #[test]
    fn many_labels_take_a_fixed_number_of_glyph_runs() {
        let letters: Vec<char> = "asdfghjklqwertyuiopzxcvbnm".chars().collect();
        let labels: Vec<JumpLabel> = (0..500_u32)
            .map(|i| {
                let i = usize::try_from(i).unwrap();
                let text: String = [letters[i / 26 % 26], letters[i % 26]].iter().collect();
                let at = Point::new((i % 25) as f64 * 2.0 - 24.0, (i / 25) as f64 - 10.0);
                label(&format!("o{i}"), at, &text)
            })
            .collect();
        let vp = Viewport::new(Point::ZERO, 1.0, 1920, 1080, 1.0);
        let mut renderer = Renderer::new_trusted();
        draw(&mut renderer, &vp, &labels, "");
        let work = renderer.work;
        assert_eq!((work.labels, work.label_letters), (500, 1000), "{work:?}");
        assert_eq!(work.label_runs, 1, "{work:?}");
        // After a typed `s`, only the labels that start with it: their `s` dim, the rest not.
        draw(&mut renderer, &vp, &labels, "s");
        let work = renderer.work;
        assert_eq!(
            (work.labels, work.label_letters, work.label_runs),
            (26, 52, 2)
        );
        // A label no one has.
        draw(&mut renderer, &vp, &labels, "\u{E9}");
        assert_eq!(renderer.work.labels, 0);
    }

    /// What labels cost is taken from the selection highlights' budget: with labels on every
    /// object of a full board, fewer selected objects are outlined; with a few, about as many
    /// (each of these highlights crosses the window, about 140 us of estimated work).
    #[test]
    fn labels_take_from_the_highlights_budget() {
        let mut board = Board::new();
        for i in 0..lawh_core::board::MAX_OBJECTS {
            let rect = lawh_core::Shape::Rect {
                origin: Point::new(-20.0, -10.0),
                w: 40.0,
                h: 20.0,
            };
            let id = ObjectId::new(format!("r{i}")).unwrap();
            board.insert(lawh_core::Object::new(id, rect)).unwrap();
        }
        let selection: Vec<ObjectId> = board.objects().iter().map(|o| o.id.clone()).collect();
        let letters: Vec<char> = "asdfghjklqwertyuiopzxcvbnm".chars().collect();
        let labels: Vec<JumpLabel> = (0..500)
            .map(|i: usize| {
                let at = Point::new(f64::from(u32::try_from(i % 25).unwrap()) * 1.4 - 17.0, 0.0);
                let text: String = [letters[i / 26 % 26], letters[i % 26]].iter().collect();
                label(&format!("o{i}"), at, &text)
            })
            .collect();
        let vp = Viewport::new(Point::ZERO, 1.0, 1920, 1080, 1.0);
        let mut renderer = Renderer::new_trusted();
        let mut highlighted = |labels: &[JumpLabel]| {
            let overlay = Overlay {
                selection: &selection,
                labels: Some((labels, "")),
                ..Overlay::default()
            };
            renderer.render_with(&board, &vp, &overlay);
            renderer.work.highlighted
        };
        let alone = highlighted(&[]);
        let few = highlighted(&labels[..5]);
        let many = highlighted(&labels);
        assert!(alone > 1 && few + 1 >= alone, "{alone} {few}");
        assert!(many < alone / 2, "{many} {alone}");
    }

    #[test]
    fn a_nearer_label_covers_a_farther_one() {
        let vp = Viewport::new(Point::ZERO, 1.0, 400, 300, 1.0);
        let mut renderer = Renderer::new_trusted();
        // Nearest first: `a` is drawn over `s`, and `d`, a few pixels off, shows beside it.
        let labels = [
            label("o1", Point::ZERO, "a"),
            label("o2", Point::ZERO, "s"),
            label("o3", Point::new(0.5, 0.0), "d"),
        ];
        draw(&mut renderer, &vp, &labels, "");
        let work = renderer.work;
        assert_eq!((work.labels, work.label_letters), (3, 2), "{work:?}");
    }

    #[test]
    fn labels_stay_inside_the_window_above_the_status_bar() {
        let (width, height) = (400_u16, 300_u16);
        let vp = Viewport::new(Point::ZERO, 1.0, width, height, 1.0);
        let bar = status_bar_height_px(1.0);
        let blue = CURSOR.to_rgba8().to_u8_array();
        let pixel = |image: &[u8], x: u32, y: u32| {
            let i = usize::try_from((y * u32::from(width) + x) * 4).unwrap();
            [image[i], image[i + 1], image[i + 2], image[i + 3]]
        };
        let mut renderer = Renderer::new_trusted();
        for (at, inside) in [
            // Far up and left: in the top-left corner.
            (Point::new(-1e6, -1e6), (1, 1)),
            // Far down and right: in the bottom-right corner, above the status bar.
            (
                Point::new(1e6, 1e6),
                (u32::from(width) - 2, u32::from(height) - bar - 2),
            ),
        ] {
            let image = draw(&mut renderer, &vp, &[label("o1", at, "a")], "");
            assert_eq!(pixel(&image, inside.0, inside.1), blue, "{at:?}");
            // Nothing of it on the status bar.
            assert!(
                (0..u32::from(width))
                    .all(|x| pixel(&image, x, u32::from(height) - bar + 2) != blue),
                "{at:?}"
            );
        }
        // An anchor that isn't a number: no label.
        draw(
            &mut renderer,
            &vp,
            &[label("o1", Point::new(f64::NAN, 0.0), "a")],
            "",
        );
        assert_eq!(renderer.work.labels, 0);
    }
}
