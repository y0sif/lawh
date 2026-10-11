//! Milestone 2's acceptance test: the "area of a circle" example, typed from an empty board.

// Lets the helpers below unwrap and panic like test functions (see clippy.toml).
#![cfg(test)]

mod common;

use common::run_engine;
use insta::assert_snapshot;
use lawh_core::engine::{Effect, Engine};
use lawh_core::{Board, from_lawh};

const AREA: &str = include_str!("../../../examples/area.lawh");

/// The keys that type `examples/area.lawh`: the title five units up, the circle and its radius
/// at the origin, the label `r` just above the radius, the formula to the right, then `:w`.
const SCRIPT: &str =
    "5kiArea of a circle<Esc>5j:ci 3<CR>:li 3 0<CR>lkir<Esc>4lj:ma A = pi r^2<CR>:w<CR>";

/// 62 keys, counting each `<...>` as one, make the example board and write it.
#[test]
fn the_area_example_in_62_keys() {
    let engine = Engine::new(Board::new()).with_file_name(Some("area.lawh".to_owned()));
    let r = run_engine(engine, SCRIPT);
    assert_eq!(r.keys, 62);
    assert_eq!(r.engine.board(), &from_lawh(AREA).unwrap());
    assert_eq!(
        r.effects.last(),
        Some(&Effect::Write {
            path: "area.lawh".to_owned(),
            quit_after: false
        })
    );
    assert_snapshot!(r.snapshot());
}
