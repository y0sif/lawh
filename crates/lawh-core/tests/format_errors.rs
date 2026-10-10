//! Every way a `.lawh` file can be rejected, with its message snapshotted.
//!
//! Messages start with `line:column:`, so each snapshot also checks the position.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use insta::assert_snapshot;
use lawh_core::board::MAX_TEXT_CHARS;
use lawh_core::format::{MAX_COMMENT_DEPTH, MAX_FILE_BYTES};
use lawh_core::{FormatError, from_lawh};

const HEADER: &str = "lawh version=1\n";

/// Reads `src` and returns the error, which must exist.
fn err(src: &str) -> FormatError {
    match from_lawh(src) {
        Ok(board) => panic!("expected an error, read {board:?}"),
        Err(e) => e,
    }
}

/// Reads `body` after a valid header line.
fn body_err(body: &str) -> FormatError {
    err(&format!("{HEADER}{body}"))
}

#[test]
fn syntax_error_bad_float() {
    let e = body_err("circle id=c1 x=1. y=0 r=3\n");
    assert!(matches!(e, FormatError::Syntax { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn syntax_error_unterminated_string() {
    let e = body_err("text id=t1 x=0 y=0 \"no end\n");
    assert!(matches!(e, FormatError::Syntax { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn syntax_error_raw_bidi_character() {
    // KDL forbids a literal U+202E in a string; the writer always escapes it.
    let e = body_err("text id=t1 x=0 y=0 \"a\u{202E}b\"\n");
    assert!(matches!(e, FormatError::Syntax { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn syntax_error_raw_control_character() {
    // A terminal colour code pasted into a hand-edited file.
    let e = body_err("text id=t1 x=0 y=0 \"\u{1B}[31mred\"\n");
    assert!(matches!(e, FormatError::Syntax { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn syntax_error_large_integer() {
    let e = body_err("circle id=c1 x=0 y=0 r=170141183460469231731687303715884105728\n");
    assert!(matches!(e, FormatError::Syntax { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn missing_header_in_empty_file() {
    let e = err("");
    assert_eq!(e, FormatError::MissingHeader { line: 1, column: 1 });
    assert_snapshot!(e.to_string());
}

#[test]
fn missing_header_before_objects() {
    let e = err("// a board\ncircle id=c1 x=0 y=0 r=3\n");
    assert!(matches!(e, FormatError::MissingHeader { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn header_without_version() {
    let e = err("lawh\n");
    assert!(matches!(e, FormatError::MissingProperty { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn newer_version() {
    // Whatever a newer format adds after the header, the version is what gets reported.
    let e = err("lawh version=2 compression=#true\nshape3d id=s1 {\n  face\n}\n");
    assert!(
        matches!(e, FormatError::NewerVersion { version: 2, .. }),
        "{e:?}"
    );
    assert_snapshot!(e.to_string());
}

#[test]
fn invalid_versions() {
    for (name, src) in [
        ("invalid_version_zero", "lawh version=0\n"),
        ("invalid_version_negative", "lawh version=-1\n"),
        ("invalid_version_float", "lawh version=1.0\n"),
        ("invalid_version_string", "lawh version=\"1\"\n"),
    ] {
        let e = err(src);
        assert!(matches!(e, FormatError::InvalidVersion { .. }), "{e:?}");
        assert_snapshot!(name, e.to_string());
    }
}

#[test]
fn header_with_unknown_property() {
    let e = err("lawh version=1 author=me\n");
    assert!(matches!(e, FormatError::UnknownProperty { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn header_with_duplicate_version() {
    let e = err("lawh version=1 version=1\n");
    assert!(matches!(e, FormatError::DuplicateProperty { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn header_with_argument() {
    let e = err("lawh version=1 \"area\"\n");
    assert!(matches!(e, FormatError::ExtraArgument { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn header_with_children() {
    let e = err("lawh version=1 {\n  circle id=c1 x=0 y=0 r=3\n}\n");
    assert!(matches!(e, FormatError::Children { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn second_header_is_an_unknown_kind() {
    let e = body_err("lawh version=1\n");
    assert!(matches!(e, FormatError::UnknownKind { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn unknown_object_kind() {
    let e = body_err("circle id=c1 x=0 y=0 r=3\npolygon id=p1 sides=6\n");
    assert!(matches!(e, FormatError::UnknownKind { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn unknown_property() {
    let e = body_err("circle id=c1 x=0 y=0 r=3 color=red\n");
    assert!(matches!(e, FormatError::UnknownProperty { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn property_of_another_kind() {
    let e = body_err("rect id=r1 x=0 y=0 r=3 h=2\n");
    assert!(matches!(e, FormatError::UnknownProperty { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn duplicate_property() {
    let e = body_err("circle id=c1 x=0 y=0 x=1 r=3\n");
    assert!(matches!(e, FormatError::DuplicateProperty { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn missing_property() {
    let e = body_err("rect id=r1 x=0 y=0 w=4\n");
    assert!(
        matches!(e, FormatError::MissingProperty { property: "h", .. }),
        "{e:?}"
    );
    assert_snapshot!(e.to_string());
}

#[test]
fn missing_id() {
    let e = body_err("line x1=0 y1=0 x2=3 y2=0\n");
    assert!(
        matches!(e, FormatError::MissingProperty { property: "id", .. }),
        "{e:?}"
    );
    assert_snapshot!(e.to_string());
}

#[test]
fn wrong_value_types() {
    for (name, body) in [
        (
            "wrong_type_string_for_number",
            "circle id=c1 x=zero y=0 r=3\n",
        ),
        (
            "wrong_type_bool_for_number",
            "circle id=c1 x=0 y=#true r=3\n",
        ),
        (
            "wrong_type_null_for_number",
            "circle id=c1 x=0 y=0 r=#null\n",
        ),
        ("wrong_type_number_for_id", "circle id=7 x=0 y=0 r=3\n"),
    ] {
        let e = body_err(body);
        assert!(matches!(e, FormatError::WrongType { .. }), "{e:?}");
        assert_snapshot!(name, e.to_string());
    }
}

#[test]
fn non_finite_numbers() {
    for (name, body) in [
        ("non_finite_inf", "circle id=c1 x=#inf y=0 r=3\n"),
        ("non_finite_neg_inf", "circle id=c1 x=0 y=#-inf r=3\n"),
        ("non_finite_nan", "circle id=c1 x=0 y=0 r=#nan\n"),
        ("non_finite_overflow", "circle id=c1 x=0 y=0 r=1e999\n"),
    ] {
        let e = body_err(body);
        assert!(
            matches!(
                e,
                FormatError::WrongType {
                    expected: "a finite number",
                    ..
                }
            ),
            "{e:?}"
        );
        assert_snapshot!(name, e.to_string());
    }
}

#[test]
fn invalid_ids() {
    for (name, body) in [
        ("invalid_id_space", "circle id=\"c 1\" x=0 y=0 r=3\n"),
        ("invalid_id_empty", "circle id=\"\" x=0 y=0 r=3\n"),
        ("invalid_id_non_ascii", "circle id=dائرة x=0 y=0 r=3\n"),
    ] {
        let e = body_err(body);
        assert!(matches!(e, FormatError::InvalidId { .. }), "{e:?}");
        assert_snapshot!(name, e.to_string());
    }
}

#[test]
fn duplicate_id() {
    let e = body_err("circle id=c1 x=0 y=0 r=3\nrect id=c1 x=0 y=0 w=4 h=2\n");
    assert!(matches!(e, FormatError::DuplicateId { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn children_block() {
    let e = body_err("circle id=c1 x=0 y=0 r=3 {\n  text id=t1 x=0 y=0 \"inside\"\n}\n");
    assert!(matches!(e, FormatError::Children { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn empty_children_block() {
    let e = body_err("rect id=r1 x=0 y=0 w=4 h=2 {}\n");
    assert!(matches!(e, FormatError::Children { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn missing_text() {
    let e = body_err("text id=t1 x=0 y=0\n");
    assert!(matches!(e, FormatError::MissingText { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn missing_math_source() {
    let e = body_err("math id=m1 x=5 y=0\n");
    assert!(matches!(e, FormatError::MissingText { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn extra_text_argument() {
    let e = body_err("text id=t1 x=0 y=0 \"one\" \"two\"\n");
    assert!(matches!(e, FormatError::ExtraArgument { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn argument_on_a_shape() {
    let e = body_err("circle id=c1 x=0 y=0 r=3 \"label\"\n");
    assert!(matches!(e, FormatError::ExtraArgument { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn text_that_is_not_a_string() {
    let e = body_err("math id=m1 x=0 y=0 42\n");
    assert!(matches!(e, FormatError::TextNotString { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn type_annotations() {
    for (name, body) in [
        (
            "type_annotation_on_node",
            "(shape)circle id=c1 x=0 y=0 r=3\n",
        ),
        ("type_annotation_on_value", "circle id=c1 x=(px)0 y=0 r=3\n"),
        ("type_annotation_on_text", "text id=t1 x=0 y=0 (md)\"hi\"\n"),
    ] {
        let e = body_err(body);
        assert!(matches!(e, FormatError::TypeAnnotation { .. }), "{e:?}");
        assert_snapshot!(name, e.to_string());
    }
}

#[test]
fn columns_count_characters_not_bytes() {
    let e = body_err("text id=t1 x=0 y=0 \"مساحة 😀\" size=2\n");
    assert_eq!((e.line(), e.column()), (2, 30));
    assert_snapshot!(e.to_string());
}

#[test]
fn first_problem_in_the_line_is_reported() {
    let e = body_err("circle id=\"bad id\" x=#nan r=3 extra=1\n");
    assert!(matches!(e, FormatError::InvalidId { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

// --- Limits on numbers, texts and file size ---

#[test]
fn huge_radius_and_line() {
    // The verifier's hang_on_open.lawh: bounds overflowed and the renderer allocated without limit.
    let e = body_err("circle id=c1 x=0 y=0 r=1e60\nline id=l1 x1=-1e308 y1=0 x2=1e308 y2=0\n");
    assert!(
        matches!(e, FormatError::InvalidSize { ref property, .. } if property == "r"),
        "{e:?}"
    );
    assert_snapshot!(e.to_string());
}

#[test]
fn coordinates_out_of_range() {
    for (name, body) in [
        (
            "coordinate_out_of_range_line",
            "line id=l1 x1=-1e308 y1=0 x2=1e308 y2=0\n",
        ),
        (
            "coordinate_out_of_range_text",
            "text id=t1 x=1000000.0000001 y=0 \"far\"\n",
        ),
        (
            "coordinate_out_of_range_integer",
            "math id=m1 x=0 y=-1000001 \"x\"\n",
        ),
    ] {
        let e = body_err(body);
        assert!(
            matches!(e, FormatError::CoordinateOutOfRange { .. }),
            "{e:?}"
        );
        assert_snapshot!(name, e.to_string());
    }
}

#[test]
fn sizes_out_of_range() {
    for (name, body) in [
        ("invalid_size_zero_radius", "circle id=c1 x=0 y=0 r=0\n"),
        (
            "invalid_size_negative_width",
            "rect id=r1 x=0 y=0 w=-4 h=2\n",
        ),
        (
            "invalid_size_huge_ry",
            "ellipse id=e1 x=0 y=0 rx=1 ry=1e7\n",
        ),
    ] {
        let e = body_err(body);
        assert!(matches!(e, FormatError::InvalidSize { .. }), "{e:?}");
        assert_snapshot!(name, e.to_string());
    }
}

#[test]
fn numbers_at_the_limits_are_read() {
    let src = format!(
        "{HEADER}rect id=r1 x=-1000000 y=1000000 w=1000000 h=5e-324\n\
         line id=l1 x1=-1e6 y1=1e6 x2=0 y2=-0\n"
    );
    assert_eq!(from_lawh(&src).map(|b| b.len()), Ok(2));
}

#[test]
fn file_over_the_size_limit() {
    // Checked before parsing: the body would otherwise be a syntax error.
    let src = format!("{HEADER}{}", "}".repeat(MAX_FILE_BYTES + 1 - HEADER.len()));
    let e = err(&src);
    assert_eq!(
        e,
        FormatError::FileTooLarge {
            bytes: MAX_FILE_BYTES + 1
        }
    );
    assert_eq!((e.line(), e.column()), (1, 1));
    assert_snapshot!(e.to_string());
}

#[test]
fn file_at_the_size_limit_is_parsed() {
    let src = format!("{HEADER}{}", "}".repeat(MAX_FILE_BYTES - HEADER.len()));
    assert!(matches!(err(&src), FormatError::Syntax { .. }));
}

#[test]
fn text_over_the_length_limit() {
    for kind in ["text", "math"] {
        let at_limit = "π".repeat(MAX_TEXT_CHARS);
        let ok = format!("{HEADER}{kind} id=t1 x=0 y=0 \"{at_limit}\"\n");
        assert_eq!(from_lawh(&ok).map(|b| b.len()), Ok(1), "{kind}");
        let e = body_err(&format!("{kind} id=t1 x=0 y=0 \"{at_limit}x\"\n"));
        assert_eq!(
            e,
            FormatError::TextTooLong {
                line: 2,
                column: 20,
                kind: kind.to_owned()
            }
        );
    }
    let e = body_err(&format!(
        "text id=t1 x=0 y=0 \"{}\"\n",
        "x".repeat(MAX_TEXT_CHARS + 1)
    ));
    assert_snapshot!(e.to_string());
}

// --- Positions and wording of syntax errors ---

#[test]
fn line_one_errors_point_at_the_value() {
    for (name, src) in [
        ("line_one_version_dot_five", "lawh version=.5\n"),
        (
            "line_one_version_large_integer",
            "lawh version=99999999999999999999999999999999999999999\n",
        ),
    ] {
        let e = err(src);
        assert!(matches!(e, FormatError::Syntax { .. }), "{e:?}");
        assert_eq!((e.line(), e.column()), (1, 14), "{e}");
        assert_snapshot!(name, e.to_string());
    }
}

#[test]
fn invalid_escapes() {
    for (name, body, column) in [
        ("invalid_escape_x", "text id=t1 x=0 y=0 \"a\\xb\"\n", 22),
        (
            "invalid_escape_surrogate",
            "text id=t1 x=0 y=0 \"a\\u{D800}b\"\n",
            22,
        ),
        (
            "invalid_escape_too_many_digits",
            "math id=m1 x=0 y=0 \"\\u{1234567}\"\n",
            21,
        ),
    ] {
        let e = body_err(body);
        assert!(matches!(e, FormatError::Syntax { .. }), "{e:?}");
        assert_eq!((e.line(), e.column()), (2, column), "{e}");
        assert_snapshot!(name, e.to_string());
    }
}

#[test]
fn unclosed_block_comment() {
    let e = body_err("circle id=c1 x=0 y=0 r=1 /* /* */\nrect id=r1 x=0 y=0 w=1 h=1\n");
    assert!(matches!(e, FormatError::Syntax { .. }), "{e:?}");
    assert_eq!((e.line(), e.column()), (2, 26), "{e}");
    assert_snapshot!(e.to_string());
}

#[test]
fn slashdash_after_slashdash() {
    let e = body_err("circle id=c1 x=0 y=0 r=1 /- /-r=2\n");
    assert!(matches!(e, FormatError::Syntax { .. }), "{e:?}");
    assert_snapshot!(e.to_string());
}

#[test]
fn slashdashed_children_block() {
    let e = body_err("circle id=c1 x=0 y=0 r=1 /-{\n  child\n}\n");
    assert_eq!(
        e,
        FormatError::Children {
            line: 2,
            column: 28,
            kind: "circle".to_owned()
        }
    );
}

#[test]
fn children_block_of_a_slashdashed_node() {
    let e = body_err("circle id=c1 x=0 y=0 r=1\n/-group id=g1 {\n  child\n}\n");
    assert_eq!(
        e,
        FormatError::Children {
            line: 3,
            column: 15,
            kind: "group".to_owned()
        }
    );
}

#[test]
fn errors_before_a_children_block_come_first() {
    let e = body_err("circle id=c1 x=0 y=0 r=-1\nrect id=r1 x=0 y=0 w=1 h=1 {\n}\n");
    assert!(
        matches!(e, FormatError::InvalidSize { line: 2, .. }),
        "{e:?}"
    );
    let e = body_err("polygon id=p1 {\n}\n");
    assert!(matches!(e, FormatError::UnknownKind { .. }), "{e:?}");
}

#[test]
fn forbidden_character_outside_a_string() {
    let e = body_err("circle id=c1\u{202E} x=0 y=0 r=1\n");
    assert!(matches!(e, FormatError::Syntax { .. }), "{e:?}");
    assert_eq!((e.line(), e.column()), (2, 13), "{e}");
}

// --- Hostile files: deep nesting and error recovery ---
//
// kdl's parser recurses on these. Each runs on a thread with a 2 MiB stack, the default for test
// threads, so a scan that let them through to kdl would overflow it.

/// Reads `src` on a thread with a 2 MiB stack.
fn read_on_small_stack(src: String) -> Result<lawh_core::Board, FormatError> {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || from_lawh(&src))
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn deeply_nested_children_blocks() {
    // The verifier's bis_nest2000.lawh.
    let src = format!(
        "{HEADER}circle id=c1 x=0 y=0 r=1 {}{}\n",
        "a {".repeat(2000),
        "}".repeat(2000)
    );
    assert_eq!(
        read_on_small_stack(src),
        Err(FormatError::Children {
            line: 2,
            column: 28,
            kind: "circle".to_owned()
        })
    );
}

#[test]
fn deeply_nested_block_comments() {
    // The verifier's bis_comment5000.lawh.
    let src = format!(
        "{HEADER}{}{}\ncircle id=c1 x=0 y=0 r=1\n",
        "/*".repeat(5000),
        "*/".repeat(5000)
    );
    let e = read_on_small_stack(src).unwrap_err();
    // The 17th `/*` is one too deep.
    assert_eq!(
        e,
        FormatError::CommentTooDeep {
            line: 2,
            column: 33
        }
    );
    assert_snapshot!(e.to_string());
}

#[test]
fn block_comments_at_the_depth_limit_are_read() {
    let src = format!(
        "{HEADER}{}{} circle id=c1 x=0 y=0 r=1\n",
        "/*".repeat(MAX_COMMENT_DEPTH),
        "*/".repeat(MAX_COMMENT_DEPTH),
    );
    assert_eq!(read_on_small_stack(src).map(|b| b.len()), Ok(1));
}

#[test]
fn slashdashed_nested_children_blocks() {
    // The verifier's nestslash*.lawh.
    let src = format!(
        "{HEADER}/-a {}{}\ncircle id=c1 x=0 y=0 r=1\n",
        "{ a ".repeat(10_000),
        "}".repeat(10_000)
    );
    assert_eq!(
        read_on_small_stack(src),
        Err(FormatError::Children {
            line: 2,
            column: 5,
            kind: "a".to_owned()
        })
    );
}

#[test]
fn long_block_comment_is_read() {
    // kdl reads each `*` and `/` inside a comment one level deeper; the scan blanks them.
    let src = format!(
        "{HEADER}circle /* {}*/ id=c1 x=0 y=0 r=1\n",
        "** // ".repeat(200_000)
    );
    assert_eq!(read_on_small_stack(src).map(|b| b.len()), Ok(1));
}

#[test]
fn repeated_bad_tokens_end_at_the_first() {
    // kdl recurses once per error it recovers from; each of these overflowed a 2 MiB stack.
    for (body, line, column) in [
        (format!("circle {}", ")".repeat(100_000)), 2, 8),
        (format!("circle {}", "=".repeat(100_000)), 2, 8),
        (format!("circle {}", "}".repeat(100_000)), 2, 8),
        (format!("circle {}", "1=2 ".repeat(100_000)), 2, 9),
        (format!("circle {}", "(a) ".repeat(100_000)), 2, 12),
        (format!("circle {}", "#truex ".repeat(100_000)), 2, 13),
        (format!("circle {}", "x= ".repeat(100_000)), 2, 12),
        ("a=\n".repeat(100_000), 2, 2),
        ("circle )\n".repeat(100_000), 2, 8),
        ("(a)\n".repeat(100_000), 2, 4),
        ("a \\ x\n".repeat(100_000), 2, 5),
    ] {
        let e = read_on_small_stack(format!("{HEADER}{body}")).unwrap_err();
        assert!(matches!(e, FormatError::Syntax { .. }), "{e:?}");
        assert_eq!((e.line(), e.column()), (line, column), "{e}");
    }
    for body in [
        format!("circle {}", "/-".repeat(100_000)),
        format!("circle {}", "\"".repeat(100_000)),
        format!("text id=t1 x=0 y=0 \"a\\x{}\"", "{".repeat(100_000)),
    ] {
        let e = read_on_small_stack(format!("{HEADER}{body}")).unwrap_err();
        assert!(matches!(e, FormatError::Syntax { .. }), "{e:?}");
    }
}
