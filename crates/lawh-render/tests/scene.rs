//! The window and the exports draw from one draw list (`lawh_render`'s `scene` module): the same
//! objects, with the same items, built by the same builders.
//!
//! The window's sink and an export's are recorded through doc-hidden routes
//! ([`Renderer::record_frame`], [`Renderer::record_export`]) and compared item by item, and a
//! window frame at zoom 1 is compared pixel by pixel with a PNG export at 100 %, which maps the
//! board the same way. The window draws grid dots and an export doesn't, so the pixel comparison
//! leaves out the 3 x 3 pixels around each grid point (where a dot can be) and compares every
//! other pixel, within the goldens' tolerance.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use std::io::Cursor;

use kurbo::{Point, Rect};
use lawh_core::{Board, ExportOptions, ObjectId};
use lawh_render::{Palette, Renderer, SceneItem, SceneObject, Viewport};
use vello_cpu::Pixmap;

/// Largest per-channel difference that still counts as the same pixel, as in the goldens.
const CHANNEL_TOLERANCE: u8 = 3;
/// Most pixels in a million that may differ by more than [`CHANNEL_TOLERANCE`], as in the
/// goldens.
const MAX_DIFFERENT_PER_MILLION: usize = 100;

/// Text at two sizes with a symbol Inter lacks (set in Libertinus Math), a formula, a formula
/// that fails (its source in red), an ellipse, a rectangle, a line and an arrow.
fn mixed_board() -> Board {
    common::board(
        "lawh version=1\n\
         text id=t1 x=0 y=0 \"Size 1: p \u{2227} q\"\n\
         text id=t2 x=0 y=1.5 size=2 \"Size 2,\\ntwo lines\"\n\
         math id=m1 x=7 y=0 \"A = pi r^2\"\n\
         math id=m2 x=7 y=2 \"sqrt(x^2 + 1) / 2\"\n\
         math id=m3 x=0 y=5 \"frac(1,\"\n\
         ellipse id=e1 x=13 y=2 rx=1.5 ry=1\n\
         rect id=r1 x=9 y=5 w=4 h=2\n\
         line id=l1 x1=0 y1=8 x2=6 y2=8\n\
         arrow id=a1 x1=7 y1=8 x2=12 y2=9\n",
    )
}

/// A renderer that has compiled `board`'s math and laid out its text, as an export leaves it.
fn ready(board: &Board) -> Renderer {
    let mut renderer = Renderer::new_trusted();
    renderer.resolve_math(board);
    renderer.board_bounds(board).unwrap();
    renderer
}

/// An item's kind and size, without its numbers (pixels in the window, grid units in an
/// export).
fn kind(item: &SceneItem) -> SceneItem {
    match item {
        SceneItem::Stroke { .. } => SceneItem::Stroke { width: 0.0 },
        other => other.clone(),
    }
}

fn kinds(objects: &[SceneObject]) -> Vec<(ObjectId, Vec<SceneItem>)> {
    objects
        .iter()
        .map(|object| (object.id.clone(), object.items.iter().map(kind).collect()))
        .collect()
}

#[test]
fn the_window_and_an_export_hand_their_sinks_the_same_objects_and_items() {
    let board = mixed_board();
    let mut renderer = ready(&board);
    let (frame, exported) = renderer.record_export(&board, None).unwrap();
    // Every object on screen: the export's frame, in a window that holds it.
    let vp = Viewport::fit(frame, 1600, 1000, 1.0);
    let window = renderer.record_frame(&board, &vp);
    let stats = renderer.last_frame();
    assert_eq!(
        (stats.drawn, stats.downgraded, stats.math_pending),
        (board.len(), 0, 0),
        "{stats:?}"
    );

    let ids: Vec<_> = board
        .objects()
        .iter()
        .map(|object| object.id.clone())
        .collect();
    let window_ids: Vec<_> = window.iter().map(|object| object.id.clone()).collect();
    assert_eq!(window_ids, ids);
    assert_eq!(kinds(&window), kinds(&exported));

    // What there is to see: every kind of item, the failed formula and the fallback symbol.
    let all: Vec<_> = exported.iter().flat_map(|object| &object.items).collect();
    assert!(all.iter().any(|item| matches!(item, SceneItem::Fill)));
    assert!(
        all.iter()
            .any(|item| matches!(item, SceneItem::Path { .. }))
    );
    assert!(all.iter().any(|item| matches!(
        item,
        SceneItem::Glyphs { text: Some(text), .. } if text == "frac(1,"
    )));
    assert!(all.iter().any(|item| matches!(
        item,
        SceneItem::Glyphs { font: 1, text: Some(text), .. } if text.contains('\u{2227}')
    )));
    assert!(all.iter().any(|item| matches!(
        item,
        SceneItem::Glyphs { text: None, glyphs, .. } if *glyphs > 0
    )));

    // Shapes keep their size on screen in the window (2 logical pixels) and their zoom-1 size
    // in an export (0.05 grid units).
    let width = |objects: &[SceneObject]| {
        objects
            .iter()
            .flat_map(|object| &object.items)
            .find_map(|item| match item {
                SceneItem::Stroke { width } => Some(*width),
                _ => None,
            })
            .unwrap()
    };
    assert!((width(&window) - 2.0).abs() < 1e-12);
    assert!((width(&exported) - 0.05).abs() < 1e-12);
    // At scale factor 2, 4 pixels.
    let vp2 = Viewport::fit(frame, 1600, 1000, 2.0);
    assert!((width(&renderer.record_frame(&board, &vp2)) - 4.0).abs() < 1e-12);
}

#[test]
fn the_window_hands_its_sink_only_what_reaches_the_view() {
    // 400 lines of text, and a circle far away.
    let lines = vec!["lorem ipsum"; 400].join("\\n");
    let board = common::board(&format!(
        "lawh version=1\ntext id=t1 x=0 y=0 \"{lines}\"\ncircle id=c1 x=1000 y=0 r=1\n"
    ));
    let mut renderer = ready(&board);
    let (_, exported) = renderer.record_export(&board, None).unwrap();
    let runs = |object: &SceneObject| object.items.len();
    assert_eq!(exported.len(), 2);
    assert!(runs(&exported[0]) >= 400, "{}", runs(&exported[0]));
    // Zoomed in on the text's first lines: a few lines' runs, and no circle.
    let vp = Viewport::new(Point::new(2.0, 0.5), 20.0, 800, 600, 1.0);
    let window = renderer.record_frame(&board, &vp);
    assert_eq!(window.len(), 1, "{window:?}");
    assert!(runs(&window[0]) <= 8, "{}", runs(&window[0]));
}

/// Pixels of a frame or a decoded PNG, premultiplied RGBA8, and their width.
struct Image<'a> {
    data: &'a [u8],
    width: usize,
}

impl Image<'_> {
    fn at(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.width + x) * 4;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "pixel positions inside a small image, checked against its size"
)]
fn pixel(v: f64) -> Option<usize> {
    (v >= 0.0 && v < f64::from(u16::MAX)).then_some(v as usize)
}

#[test]
fn a_window_frame_at_zoom_1_matches_a_png_export_at_100_percent() {
    let board = mixed_board();
    let mut renderer = ready(&board);
    let (frame, _) = renderer.record_export(&board, None).unwrap();
    let options = ExportOptions {
        png_scale_hundredths: 100,
        ..ExportOptions::default()
    };
    let png = renderer.export(&board, None, &options).unwrap().bytes;
    let exported = Pixmap::from_png(Cursor::new(png)).unwrap();
    let (width, height) = (exported.width(), exported.height());
    // An export centres its frame in the image; so does a window centred on the frame's
    // centre, at 40 pixels a unit.
    let vp = Viewport::new(frame.center(), 1.0, width, height, 1.0);
    let window = renderer.render(&board, &vp).to_vec();
    let stats = renderer.last_frame();
    assert_eq!(
        (stats.drawn, stats.downgraded, stats.math_pending),
        (board.len(), 0, 0),
        "{stats:?}"
    );
    let (cols, rows) = (usize::from(width), usize::from(height));
    let window = Image {
        data: &window,
        width: cols,
    };
    let exported = Image {
        data: exported.data_as_u8_slice(),
        width: cols,
    };

    // The pixels a grid dot can touch: 1.5 pixels across, centred on the pixel of each grid
    // point, so that pixel and the ones around it.
    let mut dots = vec![false; cols * rows];
    let visible = Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
    let board_rect = Rect::from_points(
        vp.to_board(Point::ZERO),
        vp.to_board(Point::new(visible.x1, visible.y1)),
    );
    let (mut board_x, mut grid_points) = (board_rect.x0.floor(), 0);
    while board_x <= board_rect.x1.ceil() {
        let mut board_y = board_rect.y0.floor();
        while board_y <= board_rect.y1.ceil() {
            let at = vp.to_screen(Point::new(board_x, board_y));
            if let (Some(col), Some(row)) = (pixel(at.x.floor()), pixel(at.y.floor())) {
                grid_points += 1;
                for near_row in row.saturating_sub(1)..=(row + 1).min(rows - 1) {
                    for near_col in col.saturating_sub(1)..=(col + 1).min(cols - 1) {
                        dots[near_row * cols + near_col] = true;
                    }
                }
            }
            board_y += 1.0;
        }
        board_x += 1.0;
    }
    let masked = dots.iter().filter(|dot| **dot).count();
    assert!(grid_points > 100, "{grid_points}");
    assert!(
        masked * 100 < cols * rows,
        "{masked} of {} pixels left out",
        cols * rows
    );

    // Everything else matches; ink is everywhere the objects are.
    let paper = Palette::LIGHT.paper.to_rgba8().to_u8_array();
    let (mut compared, mut changed, mut different, mut drawn) = (0, 0, 0, 0);
    for row in 0..rows {
        for col in 0..cols {
            if dots[row * cols + col] {
                continue;
            }
            let (on_screen, in_png) = (window.at(col, row), exported.at(col, row));
            compared += 1;
            changed += usize::from(on_screen != in_png);
            drawn += usize::from(in_png != paper);
            let apart = on_screen
                .iter()
                .zip(&in_png)
                .any(|(a, b)| a.abs_diff(*b) > CHANNEL_TOLERANCE);
            different += usize::from(apart);
        }
    }
    println!(
        "{cols}x{rows}: {different} of {compared} pixels differ by more than {CHANNEL_TOLERANCE}, \
         {changed} at all ({masked} by grid points left out, {drawn} not paper)"
    );
    assert!(drawn > 5_000, "{drawn}");
    assert!(
        different * 1_000_000 <= compared * MAX_DIFFERENT_PER_MILLION,
        "{different} of {compared} pixels differ"
    );
    // The export has no grid: where the window shows a dot on paper, the export shows paper.
    let window_dots = (0..cols * rows)
        .filter(|&i| dots[i] && window.at(i % cols, i / cols) != paper)
        .filter(|&i| exported.at(i % cols, i / cols) == paper)
        .count();
    assert!(window_dots > 100, "{window_dots}");
}
