//! Reading and writing whole `.lawh` files.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use lawh_core::board::{DEFAULT_TEXT_SIZE, MAX_COORDINATE, MAX_SIZE, MAX_TEXT_SIZE, MIN_TEXT_SIZE};
use lawh_core::{Board, Object, ObjectId, Point, Shape, from_lawh, to_lawh};

const AREA: &str = include_str!("../../../examples/area.lawh");

fn object(id: &str, shape: Shape) -> Object {
    Object::new(ObjectId::new(id).unwrap(), shape)
}

fn board(objects: impl IntoIterator<Item = Object>) -> Board {
    let mut board = Board::new();
    for o in objects {
        board.insert(o).unwrap();
    }
    board
}

#[test]
fn area_example_parses_into_the_expected_objects() {
    let expected = board([
        object(
            "t1",
            Shape::Text {
                at: Point::new(0.0, -5.0),
                text: "Area of a circle".to_owned(),
                size: DEFAULT_TEXT_SIZE,
            },
        ),
        object(
            "c1",
            Shape::Ellipse {
                center: Point::new(0.0, 0.0),
                rx: 3.0,
                ry: 3.0,
            },
        ),
        object(
            "l1",
            Shape::Line {
                a: Point::new(0.0, 0.0),
                b: Point::new(3.0, 0.0),
            },
        ),
        object(
            "t2",
            Shape::Text {
                at: Point::new(1.0, -1.0),
                text: "r".to_owned(),
                size: DEFAULT_TEXT_SIZE,
            },
        ),
        object(
            "m1",
            Shape::Math {
                at: Point::new(5.0, 0.0),
                src: "A = pi r^2".to_owned(),
                size: DEFAULT_TEXT_SIZE,
            },
        ),
    ]);
    assert_eq!(from_lawh(AREA).unwrap(), expected);
}

#[test]
fn area_example_is_written_in_the_fixed_shape() {
    insta::assert_snapshot!(to_lawh(&from_lawh(AREA).unwrap()));
}

/// One of each kind, written exactly as the format reference shows it.
#[test]
fn every_kind_is_written_in_the_documented_shape() {
    let all = board([
        object(
            "t1",
            Shape::Text {
                at: Point::new(0.0, -5.0),
                text: "Area of a circle".to_owned(),
                size: DEFAULT_TEXT_SIZE,
            },
        ),
        object(
            "m1",
            Shape::Math {
                at: Point::new(5.0, 0.0),
                src: "A = pi r^2".to_owned(),
                size: DEFAULT_TEXT_SIZE,
            },
        ),
        object(
            "c1",
            Shape::Ellipse {
                center: Point::ZERO,
                rx: 3.0,
                ry: 3.0,
            },
        ),
        object(
            "e1",
            Shape::Ellipse {
                center: Point::ZERO,
                rx: 3.0,
                ry: 2.0,
            },
        ),
        object(
            "r1",
            Shape::Rect {
                origin: Point::ZERO,
                w: 4.0,
                h: 2.0,
            },
        ),
        object(
            "l1",
            Shape::Line {
                a: Point::ZERO,
                b: Point::new(3.0, 0.0),
            },
        ),
        object(
            "a1",
            Shape::Arrow {
                a: Point::ZERO,
                b: Point::new(3.0, 0.0),
            },
        ),
    ]);
    let expected = "\
lawh version=1
text id=t1 x=0 y=-5 \"Area of a circle\"
math id=m1 x=5 y=0 \"A = pi r^2\"
circle id=c1 x=0 y=0 r=3
ellipse id=e1 x=0 y=0 rx=3 ry=2
rect id=r1 x=0 y=0 w=4 h=2
line id=l1 x1=0 y1=0 x2=3 y2=0
arrow id=a1 x1=0 y1=0 x2=3 y2=0
";
    assert_eq!(to_lawh(&all), expected);
    assert_eq!(from_lawh(expected).unwrap(), all);
}

#[test]
fn empty_board_is_just_the_header() {
    assert_eq!(to_lawh(&Board::new()), "lawh version=1\n");
}

#[test]
fn write_read_write_is_byte_identical() {
    let first = to_lawh(&from_lawh(AREA).unwrap());
    let board = from_lawh(&first).unwrap();
    assert_eq!(to_lawh(&board), first);
    assert_eq!(board, from_lawh(AREA).unwrap());
}

#[test]
fn tricky_strings_and_numbers_round_trip() {
    let tricky = board([
        object(
            "t1",
            Shape::Text {
                at: Point::new(0.1 + 0.2, -0.0),
                text: "quote \" backslash \\ newline\nCR\r tab\t NUL\0 ESC\u{1B} \
                       RLM\u{200F} RLO\u{202E} BOM\u{FEFF} LS\u{2028} // not a comment"
                    .to_owned(),
                size: DEFAULT_TEXT_SIZE,
            },
        ),
        object(
            "m1",
            Shape::Math {
                at: Point::new(1e-7, 123_456.789),
                src: "#let x = \"a\"; $x$ sum_(i=1)^n i".to_owned(),
                size: DEFAULT_TEXT_SIZE,
            },
        ),
        object(
            "true",
            Shape::Ellipse {
                center: Point::new(MAX_COORDINATE, f64::MIN_POSITIVE),
                rx: 5e-324,
                ry: MAX_SIZE,
            },
        ),
        object(
            "1",
            Shape::Rect {
                origin: Point::new(-1.0 / 3.0, -MAX_COORDINATE),
                w: 9.99e-7,
                h: 1e-6,
            },
        ),
    ]);
    let text = to_lawh(&tricky);
    let back = from_lawh(&text).unwrap();
    assert_eq!(back, tricky);
    assert_eq!(to_lawh(&back), text);
    insta::assert_snapshot!(text);
}

#[test]
fn tiny_numbers_are_written_in_exponent_form() {
    // The verifier's num_subnormal.lawh: 4.9e-324 used to be written with 300+ digits.
    let board = from_lawh("lawh version=1\ncircle id=c1 x=4.9e-324 y=-2.5e-320 r=1e-7\n").unwrap();
    let text = to_lawh(&board);
    assert_eq!(
        text,
        "lawh version=1\ncircle id=c1 x=5e-324 y=-2.5e-320 r=1e-7\n"
    );
    assert_eq!(from_lawh(&text).unwrap(), board);
}

#[test]
fn crlf_file_reads_the_same_as_lf() {
    // A Windows checkout may already have CRLF, so build both versions here.
    let lf = AREA.replace("\r\n", "\n");
    let crlf = lf.replace('\n', "\r\n");
    assert!(crlf.contains("\r\n") && !lf.contains('\r'));
    assert_eq!(from_lawh(&crlf).unwrap(), from_lawh(&lf).unwrap());
}

#[test]
fn crlf_errors_point_at_the_same_place_as_lf() {
    let lf = "lawh version=1\ncircle id=c1 x=0 y=0 r=3\nrect id=r1 x=0 y=0 w=4 depth=2\n";
    let crlf = lf.replace('\n', "\r\n");
    let (a, b) = (from_lawh(lf).unwrap_err(), from_lawh(&crlf).unwrap_err());
    assert_eq!(a, b);
    assert_eq!((a.line(), a.column()), (3, 24));
}

/// A file M2 wrote, every kind and the writer's number forms, reads and writes back to the same
/// bytes: sizes change nothing for a board without them.
#[test]
fn a_file_without_sizes_writes_back_byte_identical() {
    let m2 = "\
lawh version=1
text id=t1 x=0 y=-5 \"Area of a circle\"
math id=m1 x=5 y=0 \"A = pi r^2\"
text id=\"1\" x=0.30000000000000004 y=-0.5 \"quote \\\" tab\\t \\u{200F}\"
math id=size x=1e-7 y=123456.789 \"size = 2\"
circle id=c1 x=0 y=0 r=3
ellipse id=e1 x=0 y=0 rx=3 ry=2
rect id=r1 x=-1000000 y=1000000 w=5e-324 h=1000000
line id=l1 x1=0 y1=0 x2=3 y2=0
arrow id=a1 x1=0 y1=0 x2=3 y2=0
";
    let board = from_lawh(m2).unwrap();
    assert!(
        board
            .objects()
            .iter()
            .filter_map(|o| o.shape.size())
            .all(|s| s.to_bits() == DEFAULT_TEXT_SIZE.to_bits())
    );
    assert_eq!(to_lawh(&board), m2);
}

#[test]
fn sizes_are_read_and_written_after_x_and_y() {
    let src = "\
lawh version=1
text id=t1 x=0 y=0 size=2 \"Title\"
math size=0.25 id=m1 x=1 y=2 \"x^2\"
text id=t2 x=0 y=3 \"default\" size=16
math id=m2 x=0 y=4 size=1.0 \"y\"
text id=t3 x=0 y=5 size=1 \"one\"
math id=m3 x=0 y=6 size=1.0000000000000002 \"z\"
";
    let board = from_lawh(src).unwrap();
    let sizes: Vec<Option<f64>> = board.objects().iter().map(|o| o.shape.size()).collect();
    let next = f64::from_bits(DEFAULT_TEXT_SIZE.to_bits() + 1);
    assert_eq!(
        sizes,
        [2.0, MIN_TEXT_SIZE, MAX_TEXT_SIZE, 1.0, 1.0, next].map(Some)
    );
    assert_eq!(
        board.objects()[0].shape,
        Shape::Text {
            at: Point::ZERO,
            text: "Title".to_owned(),
            size: 2.0,
        }
    );
    // `size=` after `x y`, in the writer's number style; a size of 1 is left out.
    let written = "\
lawh version=1
text id=t1 x=0 y=0 size=2 \"Title\"
math id=m1 x=1 y=2 size=0.25 \"x^2\"
text id=t2 x=0 y=3 size=16 \"default\"
math id=m2 x=0 y=4 \"y\"
text id=t3 x=0 y=5 \"one\"
math id=m3 x=0 y=6 size=1.0000000000000002 \"z\"
";
    assert_eq!(to_lawh(&board), written);
    assert_eq!(from_lawh(written).unwrap(), board);
}
