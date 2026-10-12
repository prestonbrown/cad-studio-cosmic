use engine::parsing::stl::{is_binary_stl, parse_stl};
use std::path::PathBuf;

fn fixture(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures").join(p)
}

#[test]
fn binary_predicate() {
    let golden = std::fs::read(fixture("golden/model.stl")).unwrap();
    let n = u32::from_le_bytes([golden[80], golden[81], golden[82], golden[83]]);
    assert!(is_binary_stl(golden.len() as u64, n));
    let ascii = std::fs::read(fixture("handmade/ascii.stl")).unwrap();
    let n = u32::from_le_bytes([ascii[80], ascii[81], ascii[82], ascii[83]]);
    assert!(!is_binary_stl(ascii.len() as u64, n));
}

#[test]
fn binary_golden_parses_with_known_geometry() {
    let bytes = std::fs::read(fixture("golden/model.stl")).unwrap();
    let part = parse_stl(&bytes).unwrap();
    assert_eq!(part.triangle_count(), 24);
    assert_eq!(part.positions.len(), 24 * 3);
    let (mn, mx) = part.aabb();
    assert_eq!(mn, [-30.0, -30.0, -4.0]);
    assert_eq!(mx, [30.0, 30.0, 22.0]);
    assert!(part.triangle_colors.iter().all(|c| c.is_none()));
    assert_eq!(part.name, "model"); // fixed name: STL carries none; callers rename per display need
}

#[test]
fn ascii_fixture_parses_one_triangle() {
    let bytes = std::fs::read(fixture("handmade/ascii.stl")).unwrap();
    let part = parse_stl(&bytes).unwrap();
    assert_eq!(part.triangle_count(), 1);
    assert_eq!(part.positions[0], [0.0, 0.0, 0.0]);
    assert_eq!(part.positions[1], [1.0, 0.0, 0.0]);
    assert_eq!(part.positions[2], [0.0, 1.0, 0.0]);
}

#[test]
fn ascii_skips_bad_lines_errors_only_on_zero_triangles() {
    // upstream skips unparseable vertex lines; error only if nothing parsed
    let text = b"solid x\n  facet normal 0 0 1\n    outer loop\n      vertex 0 0 0\n      vertex bogus\n      vertex 1 0 0\n      vertex 0 1 0\n    endloop\n  endfacet\nendsolid x\n";
    let part = parse_stl(text).unwrap();
    assert_eq!(part.triangle_count(), 1, "bad line skipped, three good vertices still form the triangle");
    assert!(parse_stl(b"solid empty\nendsolid empty\n").is_err());
}

#[test]
fn truncated_binary_is_an_error_not_a_panic() {
    let golden = std::fs::read(fixture("golden/model.stl")).unwrap();
    assert!(parse_stl(&golden[..golden.len() - 20]).is_err());
}
