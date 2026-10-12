//! Random boards survive a write and a read exactly.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use lawh_core::board::{DEFAULT_TEXT_SIZE, MAX_COORDINATE, MAX_SIZE, MAX_TEXT_SIZE, MIN_TEXT_SIZE};
use lawh_core::{Board, Object, ObjectId, Point, Shape, from_lawh, to_lawh};
use proptest::prelude::*;

/// Cases for the property tests: `LAWH_PROPTEST_CASES` if set, otherwise `default`.
fn cases(default: u32) -> u32 {
    std::env::var("LAWH_PROPTEST_CASES")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// Coordinates: any bit pattern within the limit, plus the round and fractional values real
/// boards hold.
fn coordinate() -> impl Strategy<Value = f64> {
    use proptest::num::f64 as f;
    prop_oneof![
        // Out-of-range bit patterns wrap into range, keeping their mantissa bits.
        3 => (f::POSITIVE | f::NEGATIVE | f::NORMAL | f::SUBNORMAL | f::ZERO)
            .prop_map(|v| if v.abs() <= MAX_COORDINATE { v } else { v % MAX_COORDINATE }),
        3 => (-4000_i32..4000).prop_map(|i| f64::from(i) / 8.0),
        2 => (-1000_i32..1000, 1_i32..1000).prop_map(|(a, b)| f64::from(a) / f64::from(b)),
        1 => prop::sample::select(vec![
            -0.0,
            0.1 + 0.2,
            1e-6,
            9.99e-7,
            1e-7,
            999_999.999_999_999_9,
            MAX_COORDINATE,
            -MAX_COORDINATE,
            f64::MIN_POSITIVE,
            f64::EPSILON,
            5e-324,
            -5e-324,
        ]),
    ]
}

/// Sizes: above zero and at most the limit.
fn size() -> impl Strategy<Value = f64> {
    prop_oneof![
        4 => coordinate().prop_map(f64::abs),
        1 => prop::sample::select(vec![MAX_SIZE, 5e-324, 1e-7, 0.5, 3.0]),
    ]
    .prop_filter("sizes are above zero", |v| *v > 0.0)
}

/// Sizes of text and math: mostly the default, which the writer leaves out, then any number in
/// range, the bounds, the size keys' steps, and the numbers next to the default and the bounds.
fn text_size() -> impl Strategy<Value = f64> {
    let next = |v: f64| f64::from_bits(v.to_bits() + 1);
    let previous = |v: f64| f64::from_bits(v.to_bits() - 1);
    prop_oneof![
        3 => Just(DEFAULT_TEXT_SIZE),
        2 => MIN_TEXT_SIZE..=MAX_TEXT_SIZE,
        1 => (1_i32..=64).prop_map(|q| f64::from(q) / 4.0),
        1 => prop::sample::select(vec![
            MIN_TEXT_SIZE,
            MAX_TEXT_SIZE,
            next(MIN_TEXT_SIZE),
            previous(MAX_TEXT_SIZE),
            next(DEFAULT_TEXT_SIZE),
            previous(DEFAULT_TEXT_SIZE),
            0.1 + 0.2,
        ]),
    ]
}

fn point() -> impl Strategy<Value = Point> {
    (coordinate(), coordinate()).prop_map(|(x, y)| Point::new(x, y))
}

/// Characters KDL or a terminal treat specially.
const SPECIAL: &[char] = &[
    '"', '\\', '\n', '\r', '\t', '\0', '\u{8}', '\u{C}', '\u{B}', '\u{1B}', '\u{7F}', '\u{85}',
    '\u{9F}', '\u{AD}', '\u{A0}', '\u{61C}', '\u{200B}', '\u{200D}', '\u{200E}', '\u{200F}',
    '\u{2028}', '\u{2029}', '\u{202A}', '\u{202E}', '\u{2066}', '\u{2069}', '\u{FEFF}', '\u{FFFD}',
    '/', '*', '#', '$', '{', '}', ';', '=', '(', ')', ' ', 'é', 'π', 'م', '日', '😀',
];

fn text() -> impl Strategy<Value = String> {
    let c = prop_oneof![
        3 => any::<char>(),
        3 => prop::char::range(' ', '~'),
        2 => prop::sample::select(SPECIAL),
    ];
    prop::collection::vec(c, 0..24).prop_map(String::from_iter)
}

fn id() -> impl Strategy<Value = ObjectId> {
    prop_oneof![
        3 => "[a-z][0-9]{1,2}",
        2 => "[A-Za-z0-9_-]{1,6}",
        1 => prop::sample::select(vec!["true", "false", "null", "inf", "nan", "-inf", "-", "_", "1", "-1", "0x1f", "1e3"])
            .prop_map(str::to_owned),
    ]
    .prop_map(|s| ObjectId::new(s).unwrap())
}

fn shape() -> impl Strategy<Value = Shape> {
    prop_oneof![
        (point(), text(), text_size()).prop_map(|(at, text, size)| Shape::Text { at, text, size }),
        (point(), text(), text_size()).prop_map(|(at, src, size)| Shape::Math { at, src, size }),
        (point(), size()).prop_map(|(center, r)| Shape::Ellipse {
            center,
            rx: r,
            ry: r
        }),
        (point(), size(), size()).prop_map(|(center, rx, ry)| Shape::Ellipse { center, rx, ry }),
        (point(), size(), size()).prop_map(|(origin, w, h)| Shape::Rect { origin, w, h }),
        (point(), point()).prop_map(|(a, b)| Shape::Line { a, b }),
        (point(), point()).prop_map(|(a, b)| Shape::Arrow { a, b }),
    ]
}

fn board() -> impl Strategy<Value = Board> {
    prop::collection::vec((id(), shape()), 0..12).prop_map(|objects| {
        let mut board = Board::new();
        for (id, shape) in objects {
            // Drop repeated ids; every other object is valid.
            if !board.contains(&id) {
                board.insert(Object::new(id, shape)).unwrap();
            }
        }
        board
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases(512)))]

    #[test]
    fn boards_round_trip_exactly(board in board()) {
        let text = to_lawh(&board);
        let read = from_lawh(&text)
            .map_err(|e| TestCaseError::fail(format!("{e}\n--- file ---\n{text}")))?;
        prop_assert_eq!(&read, &board);
        prop_assert_eq!(to_lawh(&read), text);
    }

    /// A size of exactly the default is left out of the file; every other size is written.
    #[test]
    fn only_sizes_other_than_the_default_are_written(board in board()) {
        let text = to_lawh(&board);
        let sized = board.objects().iter().filter_map(|o| o.shape.size());
        let expected = sized.filter(|s| s.to_bits() != DEFAULT_TEXT_SIZE.to_bits()).count();
        // `text id=t1 x=0 y=0 size=2 "..."`: ids and numbers hold no spaces, and the content
        // starts with a quote.
        let written = text
            .lines()
            .filter(|line| line.split(' ').nth(4).is_some_and(|t| t.starts_with("size=")))
            .count();
        prop_assert_eq!(written, expected, "{}", text);
    }

    #[test]
    fn written_files_read_the_same_with_crlf(board in board()) {
        let text = to_lawh(&board);
        prop_assert_eq!(from_lawh(&text.replace('\n', "\r\n")), Ok(board));
    }
}
