//! Every way a `.lawh` file can be rejected, with its message snapshotted.
//!
//! Messages start with `line:column:`, so each snapshot also checks the position.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

use insta::assert_snapshot;
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
