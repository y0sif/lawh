//! Random boards survive a write and a read exactly.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use lawh_core::board::{MAX_COORDINATE, MAX_SIZE};
use lawh_core::{Board, Object, ObjectId, Point, Shape, from_lawh, to_lawh};
use proptest::prelude::*;

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
        (point(), text()).prop_map(|(at, text)| Shape::Text { at, text }),
        (point(), text()).prop_map(|(at, src)| Shape::Math { at, src }),
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
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn boards_round_trip_exactly(board in board()) {
        let text = to_lawh(&board);
        let read = from_lawh(&text)
            .map_err(|e| TestCaseError::fail(format!("{e}\n--- file ---\n{text}")))?;
        prop_assert_eq!(&read, &board);
        prop_assert_eq!(to_lawh(&read), text);
    }

    #[test]
    fn written_files_read_the_same_with_crlf(board in board()) {
        let text = to_lawh(&board);
        prop_assert_eq!(from_lawh(&text.replace('\n', "\r\n")), Ok(board));
    }
}
