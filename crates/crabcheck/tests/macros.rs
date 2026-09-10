//! Integration tests for `#[derive(Arbitrary)]` and `#[crabcheck::test]`.
#![cfg(feature = "derive")]

use crabcheck::{
    Arbitrary,
    Config,
    Gen,
    Mutate,
    Shrink,
    Source,
    any,
    check,
    check_mutator,
    check_shrinker,
    gens,
};

#[derive(Debug, Clone, PartialEq, Arbitrary)]
struct Point {
    x: i32,
    y: i32,
}

#[derive(Debug, Clone, Arbitrary)]
#[allow(dead_code)]
enum Shape {
    Dot,
    Line(Point, Point),
    Polygon { points: Vec<Point>, closed: bool },
}

#[derive(Debug, Clone, Arbitrary)]
struct Wrapper<T>(T);

#[derive(Debug, Clone, Arbitrary)]
struct Roll {
    #[crabcheck(gen = gens::int(1u8..=6))]
    die: u8,
    #[crabcheck(gen = gens::ascii_char())]
    tag: char,
}

fn not_dot(shape: &Guarded) -> bool {
    !matches!(shape, Guarded::Dot)
}

#[derive(Debug, Clone, PartialEq, Arbitrary)]
#[crabcheck(support = not_dot)]
enum Guarded {
    Dot,
    Value(u8),
}

#[crabcheck::test]
fn derived_types_generate(shape: Shape, wrapper: Wrapper<u8>, roll: Roll) -> bool {
    let _ = (&shape, wrapper.0);
    (1..=6).contains(&roll.die) && roll.tag.is_ascii()
}

#[crabcheck::test(tests = 50, seed = 3)]
fn sorting_is_idempotent(mut v: Vec<i32>) -> bool {
    v.sort();
    let once = v.clone();
    v.sort();
    v == once
}

#[crabcheck::test(config = Config::default().tests(20).no_regressions())]
fn config_expression_is_accepted(a: u8, b: u8) -> Result<(), String> {
    if u16::from(a) + u16::from(b) == u16::from(b) + u16::from(a) {
        Ok(())
    } else {
        Err("not commutative".into())
    }
}

#[test]
fn derived_struct_shrinks_field_by_field() {
    let config = Config::default().seed(1).tests(200).no_regressions();
    let report = check(&config, any::<Point>(), |p| p.x <= 10);
    assert_eq!(report.counterexample().unwrap().debug, "Point { x: 11, y: 0 }", "{report}");
    // any::<i32>() is a mix of edges and a uniform draw, so the candidates are the
    // union of both shrinkers: all closer to zero, zero first.
    let candidates = Point { x: 4, y: 0 }.shrink();
    assert_eq!(candidates[0], Point { x: 0, y: 0 });
    assert!(
        candidates.contains(&Point { x: 2, y: 0 }) && candidates.contains(&Point { x: 3, y: 0 }),
        "{candidates:?}"
    );
    assert!(candidates.iter().all(|p| p.x.abs() < 4 && p.y == 0), "{candidates:?}");
}

#[test]
fn enum_payloads_shrink_within_the_variant() {
    let config = Config::default().seed(2).tests(200).no_regressions();
    let report = check(&config, any::<Shape>(), |s| matches!(s, Shape::Dot));
    let debug = &report.counterexample().unwrap().debug;
    assert!(
        debug == "Line(Point { x: 0, y: 0 }, Point { x: 0, y: 0 })"
            || debug == "Polygon { points: [], closed: false }",
        "{debug}"
    );
    // Earlier data-less variants are candidates for later ones.
    let candidates = Shape::Line(Point { x: 0, y: 0 }, Point { x: 0, y: 0 }).shrink();
    assert!(matches!(candidates[0], Shape::Dot));
}

#[test]
fn support_predicate_filters_candidates_and_mutants() {
    let candidates = Guarded::Value(3).shrink();
    assert!(!candidates.contains(&Guarded::Dot), "{candidates:?}");
    assert!(candidates.contains(&Guarded::Value(0)));
    let mut g = Source::new(7, 5);
    for _ in 0..100 {
        if let Some(m) = Guarded::Value(3).mutate(&mut g) {
            assert!(not_dot(&m), "{m:?}");
        }
    }
}

#[test]
fn derived_hooks_pass_the_checkers() {
    let config = Config::default().seed(5).tests(200).no_regressions();
    let support = |r: &Roll| (1..=6).contains(&r.die) && r.tag.is_ascii();
    // Chars shrink toward 'a', so the size measure is the distance from 'a'.
    let size = |r: &Roll| u64::from(r.die) * 1000 + u64::from((r.tag as u32).abs_diff('a' as u32));
    check_shrinker(&config, any::<Roll>(), support, size).assert_passed();
    check_mutator(&config, any::<Roll>(), support).assert_passed();
    let point_support = |_: &Point| true;
    let point_size = |p: &Point| p.x.unsigned_abs() as u64 + p.y.unsigned_abs() as u64;
    check_shrinker(&config, any::<Point>(), point_support, point_size).assert_passed();
}

#[test]
fn every_variant_is_produced() {
    let mut source = Source::new(9, 10);
    let mut seen = [false; 3];
    for _ in 0..200 {
        match any::<Shape>().generate(&mut source) {
            Shape::Dot => seen[0] = true,
            Shape::Line(..) => seen[1] = true,
            Shape::Polygon { .. } => seen[2] = true,
        }
    }
    assert_eq!(seen, [true; 3]);
}
