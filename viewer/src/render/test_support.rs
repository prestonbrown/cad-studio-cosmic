//! Fixtures the render unit tests share.

use engine::parsing::stl::parse_stl;
use engine::parsing::threemf::parse_3mf;

pub(crate) fn fixture(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/golden")
        .join(rel)
}

pub(crate) fn golden_stl_parts() -> Vec<engine::parsing::Part> {
    let bytes = std::fs::read(fixture("model.stl"))
        .unwrap_or_else(|e| panic!("read fixture model.stl: {e}"));
    vec![parse_stl(&bytes).expect("golden model.stl parses")]
}

pub(crate) fn golden_3mf_parts() -> Vec<engine::parsing::Part> {
    let bytes = std::fs::read(fixture("model.3mf"))
        .unwrap_or_else(|e| panic!("read fixture model.3mf: {e}"));
    parse_3mf(&bytes).expect("golden model.3mf parses: 2 parts x 12 triangles")
}

/// A 10mm axis-aligned cube as 6 quads -> 12 triangles, corners drawn
/// from a shared 8-entry table so identical positions repeat across
/// faces exactly like real unindexed engine parts.
pub(crate) fn cube_part() -> engine::parsing::Part {
    let c = |x: f32, y: f32, z: f32| [10.0 * x, 10.0 * y, 10.0 * z];
    let v = [
        c(0.0, 0.0, 0.0),
        c(1.0, 0.0, 0.0),
        c(1.0, 1.0, 0.0),
        c(0.0, 1.0, 0.0),
        c(0.0, 0.0, 1.0),
        c(1.0, 0.0, 1.0),
        c(1.0, 1.0, 1.0),
        c(0.0, 1.0, 1.0),
    ];
    let faces = [
        (0usize, 3, 2, 1),
        (4, 5, 6, 7),
        (0, 1, 5, 4),
        (1, 2, 6, 5),
        (2, 3, 7, 6),
        (3, 0, 4, 7),
    ];
    let mut positions = Vec::new();
    for &(a, b, c2, d) in &faces {
        positions.extend_from_slice(&[v[a], v[b], v[c2], v[a], v[c2], v[d]]);
    }
    engine::parsing::Part {
        name: "cube".into(),
        positions,
        triangle_colors: vec![None; 12],
    }
}
