//! Spike: STL binary predicate + threemf crate evaluation against the golden 3MF.
//! Seed for M1's `engine::parsing::threemf` (function names here are what M1 keeps).

use std::io::Write as _;

fn is_binary_stl(file_len: u64, header_tri_count: u32) -> bool {
    file_len == 84 + 50 * header_tri_count as u64
}

fn fixture(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(rel)
}

fn unit_name(unit: &threemf::model::Unit) -> &'static str {
    match unit {
        threemf::model::Unit::Micron => "micron",
        threemf::model::Unit::Millimeter => "millimeter",
        threemf::model::Unit::Centimeter => "centimeter",
        threemf::model::Unit::Inch => "inch",
        threemf::model::Unit::Foot => "foot",
        threemf::model::Unit::Meter => "meter",
    }
}

#[test]
fn stl_binary_detection() {
    let ascii = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/handmade/ascii.stl"),
    )
    .unwrap();
    // ascii file: first 80 bytes are "solid ...", count field would be garbage;
    // the size check must reject it for every plausible count
    let n = u32::from_le_bytes([ascii[80], ascii[81], ascii[82], ascii[83]]);
    assert!(!is_binary_stl(ascii.len() as u64, n));
    let golden = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden/model.stl"),
    )
    .unwrap();
    let n = u32::from_le_bytes([golden[80], golden[81], golden[82], golden[83]]);
    assert!(is_binary_stl(golden.len() as u64, n));
}

/// What threemf::read gets out of the golden 3MF: object/name/mesh/unit survive,
/// basematerials (core namespace) are silently dropped, per-triangle pid/p1 are
/// not modeled, and nothing resolves build items or applies transforms.
#[test]
fn threemf_reads_golden_model() {
    let models = threemf::read(std::fs::File::open(fixture("golden/model.3mf")).unwrap())
        .expect("threemf::read succeeds on the golden 3MF");
    assert_eq!(models.len(), 1, "one .model file in the package");
    let model = &models[0];
    println!("unit: {}", unit_name(&model.unit));
    assert!(matches!(model.unit, threemf::model::Unit::Millimeter));
    for object in &model.resources.object {
        let tris = object
            .mesh
            .as_ref()
            .map(|m| m.triangles.triangle.len());
        let comps = object.components.as_ref().map(|c| c.component.len());
        println!(
            "object id={} name={:?} pid={:?} pindex={:?} triangles={tris:?} components={comps:?}",
            object.id, object.name, object.pid, object.pindex
        );
    }
    assert_eq!(model.resources.object.len(), 4);
    let by_id = |id: usize| {
        model
            .resources
            .object
            .iter()
            .find(|o| o.id == id)
            .unwrap_or_else(|| panic!("object {id} missing"))
    };
    assert_eq!(by_id(1).name.as_deref(), Some("base"));
    assert_eq!(by_id(1).mesh.as_ref().unwrap().triangles.triangle.len(), 12);
    assert_eq!(by_id(4).name.as_deref(), Some("lid"));
    assert_eq!(by_id(4).mesh.as_ref().unwrap().triangles.triangle.len(), 12);
    assert_eq!(
        by_id(3).components.as_ref().unwrap().component.len(),
        1,
        "golden's wrapped <components><component objectid=1/> survives as data"
    );
    // Crate keys basematerials as "m:basematerials"; build123d writes them in the
    // core namespace, so the golden's two color groups deserialize to None.
    println!("basematerials: {:?}", model.resources.basematerials);
    assert!(
        model.resources.basematerials.is_none(),
        "core-namespace basematerials are dropped by threemf"
    );
    // Build items keep objectid + transform, but nothing applies or flattens them.
    let items: Vec<(usize, bool)> = model
        .build
        .item
        .iter()
        .map(|i| (i.objectid, i.transform.is_some()))
        .collect();
    println!("build items: {items:?}");
    assert_eq!(items, vec![(1, false), (4, false)]);
}

/// Does threemf even parse transform attributes (spec 12-number form) and the
/// spec-required <components> wrapper? Fed via an in-memory zip because the
/// handmade fixture (and the golden) carry no transforms at all.
#[test]
fn threemf_transform_probe() {
    // Same cube as fixtures/handmade/component.3mf, but spec-conforming:
    // <components> wrapper present, 12-number transforms (row-major 3x4,
    // translation in the 4th column), a colorgroup-style basematerials block.
    let model_xml = concat!(
        r#"<?xml version="1.0" encoding="UTF-8"?>"#,
        r#"<model unit="inch" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">"#,
        r#"<resources>"#,
        r##"<basematerials id="1"><base name="red" displaycolor="#FF0000FF"/></basematerials>"##,
        r#"<object id="2" name="cube" pid="1" pindex="0"><mesh><vertices>"#,
        r#"<vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="1" y="1" z="0"/>"#,
        r#"</vertices><triangles><triangle v1="0" v2="1" v3="2" pid="1" p1="0"/></triangles></mesh></object>"#,
        r#"<object id="3" name="assembly"><components>"#,
        r#"<component objectid="2" transform="1 0 0 10  0 1 0 0  0 0 1 0"/>"#,
        r#"</components></object></resources>"#,
        r#"<build><item objectid="3" transform="1 0 0 5  0 1 0 0  0 0 1 0"/></build></model>"#,
    );
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        z.start_file(
            "3D/3dmodel.model",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        z.write_all(model_xml.as_bytes()).unwrap();
        z.finish().unwrap();
    }
    let models = threemf::read(std::io::Cursor::new(buf.into_inner())).unwrap();
    assert_eq!(models.len(), 1);
    let model = &models[0];
    println!("probe unit: {}", unit_name(&model.unit));
    assert!(matches!(model.unit, threemf::model::Unit::Inch));
    let assembly = model.resources.object.iter().find(|o| o.id == 3).unwrap();
    let comp = &assembly.components.as_ref().unwrap().component[0];
    println!("component transform: {:?}", comp.transform);
    assert_eq!(
        comp.transform,
        Some([1.0, 0.0, 0.0, 10.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
        "12-number transform attribute parses into [f64; 12]"
    );
    let item = &model.build.item[0];
    println!("item objectid={} transform={:?}", item.objectid, item.transform);
    assert_eq!(item.objectid, 3);
    assert_eq!(
        item.transform,
        Some([1.0, 0.0, 0.0, 5.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0])
    );
    println!("probe basematerials: {:?}", model.resources.basematerials);
    assert!(model.resources.basematerials.is_none());
    let cube = model.resources.object.iter().find(|o| o.id == 2).unwrap();
    let tri = &cube.mesh.as_ref().unwrap().triangles.triangle[0];
    println!("triangle: v1={} v2={} v3={}", tri.v1, tri.v2, tri.v3);
    // Triangle struct has only v1/v2/v3 — pid/p1 are dropped (no fields exist).

    // The handmade fixture omits the spec-required <components> wrapper around
    // <component>, so threemf loses the assembly relation entirely.
    let handmade = threemf::read(std::fs::File::open(fixture("handmade/component.3mf")).unwrap())
        .expect("handmade zip is readable");
    let assembly = handmade[0].resources.object.iter().find(|o| o.id == 3).unwrap();
    println!(
        "handmade assembly: mesh={} components={:?}",
        assembly.mesh.is_some(),
        assembly.components.is_some()
    );
    assert!(assembly.mesh.is_none() && assembly.components.is_none());
}
