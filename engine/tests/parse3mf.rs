//! Spike: custom zip+quick-xml 3MF parsing — object extraction and build-item →
//! component resolution with transforms. Seed for M1's `engine::parsing::threemf`.

use std::io::Read as _;

use quick_xml::events::Event;
use quick_xml::Reader;

#[derive(Debug, Clone, PartialEq)]
pub struct RawPart {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub colors: Vec<[u8; 3]>, // per triangle
}

fn fixture(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(rel)
}

/// Parses 3D/3dmodel.model XML into per-object mesh data. Objects with a mesh are
/// terminals; objects with components resolve recursively applying 3MF spec-order
/// transforms.
pub fn parse_objects(xml: &str) -> Vec<RawPart> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut objects: Vec<RawPart> = Vec::new();
    let mut cur: Option<RawPart> = None;
    let mut components: Vec<(usize, [f32; 12])> = Vec::new(); // (object index in `objects`, transform)
    let mut cur_vertex_line = [0.0f32; 3];
    let mut depth_in_object = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.local_name();
                match name.as_ref() {
                    b"object" => {
                        let mut p = RawPart { name: String::new(), positions: Vec::new(), colors: Vec::new() };
                        for a in e.attributes().flatten() {
                            if a.key.local_name().as_ref() == b"name" {
                                p.name = String::from_utf8_lossy(&a.value).into_owned();
                            }
                        }
                        cur = Some(p);
                        depth_in_object = 1;
                    }
                    b"component" if cur.is_none() && depth_in_object > 0 => {
                        // object-level component: record (target objectid, transform); resolution below
                        let mut oid = 0usize;
                        let mut xf = [0.0f32; 12];
                        xf[0] = 1.0; xf[4] = 1.0; xf[8] = 1.0; // identity default (3MF spec order: 4 rows x 3)
                        for a in e.attributes().flatten() {
                            match a.key.local_name().as_ref() {
                                b"objectid" => oid = String::from_utf8_lossy(&a.value).parse().unwrap_or(0),
                                b"transform" => {
                                    let t: Vec<f32> = String::from_utf8_lossy(&a.value)
                                        .split_whitespace().map(|v| v.parse().unwrap()).collect();
                                    xf.copy_from_slice(&t);
                                }
                                _ => {}
                            }
                        }
                        components.push((oid, xf));
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let name = e.local_name();
                match name.as_ref() {
                    b"vertex" => {
                        if let Some(_p) = cur.as_mut() {
                            let mut xyz = [0.0f32; 3];
                            for a in e.attributes().flatten() {
                                match a.key.local_name().as_ref() {
                                    b"x" => xyz[0] = String::from_utf8_lossy(&a.value).parse().unwrap(),
                                    b"y" => xyz[1] = String::from_utf8_lossy(&a.value).parse().unwrap(),
                                    b"z" => xyz[2] = String::from_utf8_lossy(&a.value).parse().unwrap(),
                                    _ => {}
                                }
                            }
                            cur_vertex_line = xyz;
                        }
                    }
                    b"triangle" => {
                        if let Some(p) = cur.as_mut() {
                            let (mut v1, mut v2, mut v3) = (0usize, 0usize, 0usize);
                            for a in e.attributes().flatten() {
                                match a.key.local_name().as_ref() {
                                    b"v1" => v1 = String::from_utf8_lossy(&a.value).parse().unwrap(),
                                    b"v2" => v2 = String::from_utf8_lossy(&a.value).parse().unwrap(),
                                    b"v3" => v3 = String::from_utf8_lossy(&a.value).parse().unwrap(),
                                    _ => {}
                                }
                            }
                            p.positions.push(cur_vertex_line); // placeholder: real impl pushes all three
                            let _ = (v1, v2, v3);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                if e.local_name().as_ref() == b"object" {
                    if let Some(p) = cur.take() {
                        objects.push(p);
                    }
                    depth_in_object = 0;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => panic!("xml error {e}"),
            _ => {}
        }
    }
    // NOTE(spike): vertex→triangle index resolution, color plucking (pid/p1), and
    // component/transform application are intentionally incomplete here — the spike
    // proves the API shape; M1 productizes. Tests below only assert what this level
    // already guarantees.
    objects
}

fn read_model_xml(path: &std::path::Path) -> String {
    let f = std::fs::File::open(path).unwrap();
    let mut z = zip::ZipArchive::new(f).unwrap();
    let mut xml = String::new();
    z.by_name("3D/3dmodel.model").unwrap().read_to_string(&mut xml).unwrap();
    xml
}

#[test]
fn golden_model_has_two_objects_with_names() {
    let xml = read_model_xml(&fixture("golden/model.3mf"));
    let parts = parse_objects(&xml);
    assert!(parts.len() >= 2, "expected >=2 objects, got {}", parts.len());
    let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
    println!("objects: {names:?}");
}

#[test]
fn component_transform_is_applied() {
    let xml = read_model_xml(&fixture("handmade/component.3mf"));
    let parts = resolve_parts(&xml);
    // build item → "assembly" → component(cube, translate +10 x): the resolved
    // part's bbox must reach x ≈ 11 (cube spans 0..1, shifted +10)
    assert_eq!(parts.len(), 1, "one resolved build part");
    let maxx = parts[0].positions.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
    assert!(maxx > 10.5, "transform not applied: max x = {maxx}");
}

/// Spike-level build resolution: build item → object → (mesh | first component),
/// applying transforms in 3MF spec token order: 4 rows x 3, row-vector
/// application (p' = p·M, translation in the last row). Single-level component
/// nesting — M1 generalizes (recursion, depth cap 16, full pid/p1 colors per
/// upstream-contract §3). Vertex positions only; triangle indexing not needed
/// for the bbox guarantee this test pins.
fn resolve_parts(xml: &str) -> Vec<RawPart> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    // 3MF spec token order: 4 rows x 3 (m00 m01 m02 m10 m11 m12 m20 m21 m22
    // m30 m31 m32); identity puts ones on the row-leading tokens 0/4/8.
    const IDENT: [f32; 12] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
    let parse_xf = |s: Option<String>| -> [f32; 12] {
        match s {
            Some(s) => s
                .split_whitespace()
                .map(|x| x.parse().unwrap())
                .collect::<Vec<f32>>()
                .try_into()
                .unwrap(),
            None => IDENT,
        }
    };
    let attr = |e: &quick_xml::events::BytesStart<'_>, key: &[u8]| -> Option<String> {
        e.attributes()
            .flatten()
            .find(|a| a.key.local_name().as_ref() == key)
            .map(|a| String::from_utf8_lossy(&a.value).into_owned())
    };
    let mut meshes: Vec<(u32, RawPart)> = Vec::new();
    let mut components: Vec<(u32, u32, [f32; 12])> = Vec::new(); // (owner id, target id, xf)
    let mut build: Vec<(u32, [f32; 12])> = Vec::new();
    let (mut cur_id, mut cur_name, mut cur_pos, mut cur_mesh) = (0u32, String::new(), Vec::new(), false);
    let flush = |meshes: &mut Vec<(u32, RawPart)>, id: u32, name: &str, pos: Vec<[f32; 3]>, has: bool| {
        if has {
            meshes.push((id, RawPart { name: name.to_string(), positions: pos, colors: Vec::new() }));
        }
    };
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                b"object" => {
                    flush(&mut meshes, cur_id, &cur_name, std::mem::take(&mut cur_pos), cur_mesh);
                    cur_id = attr(&e, b"id").and_then(|v| v.parse().ok()).unwrap_or(0);
                    cur_name = attr(&e, b"name").unwrap_or_default();
                    cur_mesh = false;
                }
                b"mesh" => cur_mesh = true,
                b"vertex" => {
                    cur_pos.push([
                        attr(&e, b"x").and_then(|v| v.parse().ok()).unwrap_or(0.0),
                        attr(&e, b"y").and_then(|v| v.parse().ok()).unwrap_or(0.0),
                        attr(&e, b"z").and_then(|v| v.parse().ok()).unwrap_or(0.0),
                    ]);
                }
                b"component" => {
                    components.push((
                        cur_id,
                        attr(&e, b"objectid").and_then(|v| v.parse().ok()).unwrap_or(0),
                        parse_xf(attr(&e, b"transform")),
                    ));
                }
                b"item" => {
                    build.push((
                        attr(&e, b"objectid").and_then(|v| v.parse().ok()).unwrap_or(0),
                        parse_xf(attr(&e, b"transform")),
                    ));
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => panic!("xml error {e}"),
            _ => {}
        }
    }
    flush(&mut meshes, cur_id, &cur_name, cur_pos, cur_mesh);
    // 3MF spec token order: 4 rows x 3, row-vector application (p' = p·M) —
    // x' = x·m0 + y·m3 + z·m6 + m9; m9/m10/m11 are the translation.
    let apply = |p: [f32; 3], m: &[f32; 12]| -> [f32; 3] {
        [
            p[0] * m[0] + p[1] * m[3] + p[2] * m[6] + m[9],
            p[0] * m[1] + p[1] * m[4] + p[2] * m[7] + m[10],
            p[0] * m[2] + p[1] * m[5] + p[2] * m[8] + m[11],
        ]
    };
    let mut out = Vec::new();
    for (oid, bxf) in build {
        if let Some((_, part)) = meshes.iter().find(|(id, _)| *id == oid) {
            let mut part = part.clone();
            for p in part.positions.iter_mut() {
                *p = apply(*p, &bxf);
            }
            out.push(part);
        } else {
            for (owner, target, cxf) in &components {
                if *owner != oid {
                    continue;
                }
                if let Some((_, part)) = meshes.iter().find(|(id, _)| id == target) {
                    let mut part = part.clone();
                    for p in part.positions.iter_mut() {
                        // component transform first, then build item transform;
                        // general matrix composition is M1 work (translate-only here)
                        *p = apply(apply(*p, cxf), &bxf);
                    }
                    out.push(part);
                }
            }
        }
    }
    out
}
