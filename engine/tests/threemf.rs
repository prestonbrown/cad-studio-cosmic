use engine::parsing::threemf::parse_3mf;
use std::path::PathBuf;

fn fixture(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures").join(p)
}
fn parse(p: &str) -> Vec<engine::parsing::Part> {
    parse_3mf(&std::fs::read(fixture(p)).unwrap()).unwrap()
}

#[test]
fn golden_two_named_colored_parts() {
    let parts = parse("golden/model.3mf");
    assert_eq!(parts.len(), 2, "one Part per build item");
    let by_name: std::collections::BTreeMap<&str, &engine::parsing::Part> =
        parts.iter().map(|p| (p.name.as_str(), p)).collect();
    let base = by_name.get("base").expect("name survived export");
    assert_eq!(base.triangle_count(), 12);
    assert!(base.triangle_colors.iter().all(|c| c.is_some()));
    assert_eq!(base.triangle_colors[0], Some([0x1F, 0x78, 0xB5]));
    let lid = by_name.get("lid").unwrap();
    assert_eq!(lid.triangle_count(), 12);
    assert_eq!(lid.triangle_colors[0], Some([0xD6, 0x26, 0x29]));
}

#[test]
fn component_transform_spec_order() {
    let parts = parse("handmade/component.3mf");
    assert_eq!(parts.len(), 1);
    let (_, mx) = parts[0].aabb();
    assert!((mx[0] - 11.0).abs() < 1e-4, "+10mm x translate at tokens 9-11");
}

#[test]
fn unit_centimeter_scales_x10() {
    let parts = parse("handmade/units.3mf");
    assert_eq!(parts.len(), 1);
    let (_, mx) = parts[0].aabb();
    assert!((mx[0] - 10.0).abs() < 1e-4, "1cm cube -> 10mm");
}

#[test]
fn colorgroup_and_per_triangle_p1() {
    let parts = parse("handmade/colorgroup.3mf");
    assert_eq!(parts.len(), 1);
    let p = &parts[0];
    assert_eq!(p.triangle_colors[0], Some([0, 255, 0]), "p1=0 -> green");
    assert_eq!(p.triangle_colors[1], Some([255, 0, 0]), "p1=1 -> red");
    assert_eq!(p.triangle_colors[2], None, "no p1 and object has no pindex -> uncolored");
}

#[test]
fn nested_components_compose_and_name_falls_through() {
    let parts = parse("handmade/nested.3mf");
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].name, "cube", "name from first descendant mesh");
    let (_, mx) = parts[0].aabb();
    assert!((mx[0] - 6.0).abs() < 1e-4, "inner +5 x then cube span");
    assert!((mx[1] - 8.0).abs() < 1e-4, "outer +7 y then cube span");
}

#[test]
fn no_build_items_sorts_object_ids_as_strings() {
    let parts = parse("handmade/nosort.3mf");
    assert_eq!(parts.len(), 2, "one per object id");
    assert_eq!(parts[0].name, "ten", "id '10' sorts before id '2' as STRINGS");
}
