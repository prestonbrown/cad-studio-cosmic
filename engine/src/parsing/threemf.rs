//! 3MF: unzip to `3D/3dmodel.model`, then a two-phase read — collect objects,
//! color tables and build items in one XML pass, resolve build items into
//! `Part`s in a second (contract §3). One `Part` per build item, flattening
//! any component tree; an empty `<build>` falls back to one `Part` per object
//! id, sorted as strings. Transforms are 3MF spec token order (4 rows × 3,
//! row-vector application, translation at tokens 9/10/11) composed
//! child-first-then-parent; the model `unit` scales the resolved geometry to
//! millimeters.

use std::collections::HashMap;
use std::io::Read as _;

use quick_xml::Reader;
use quick_xml::events::Event;

use super::{Part, TriangleColor};

/// Row-vector identity in spec token order: ones at row-leading tokens
/// 0/4/8, translation at 9/10/11.
const IDENT: [f32; 12] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];

/// Component chains deeper than this are cyclic or hostile.
const MAX_DEPTH: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum ThreemfError {
    #[error("3MF zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("3MF archive has no 3D/3dmodel.model entry")]
    MissingModel,
    #[error("3MF model XML: {0}")]
    Xml(String),
    #[error("3MF component nesting deeper than {MAX_DEPTH} levels")]
    TooDeep,
}

pub fn parse_3mf(bytes: &[u8]) -> Result<Vec<Part>, ThreemfError> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let mut xml = String::new();
    match zip.by_name("3D/3dmodel.model") {
        Ok(mut entry) => {
            entry
                .read_to_string(&mut xml)
                .map_err(|e| ThreemfError::Xml(e.to_string()))?;
        }
        Err(zip::result::ZipError::FileNotFound) => return Err(ThreemfError::MissingModel),
        Err(e) => return Err(e.into()),
    }
    let c = collect(&xml)?;
    resolve(&c)
}

/// A triangle's vertex indices and raw color reference, kept until the color
/// tables are complete (tables may follow the objects in document order).
struct RawTriangle {
    v: [usize; 3],
    pid: Option<u32>,
    p1: Option<usize>,
}

struct RawMesh {
    vertices: Vec<[f32; 3]>,
    triangles: Vec<RawTriangle>,
}

struct RawObject {
    id: u32,
    name: String,
    pid: Option<u32>,
    pindex: Option<usize>,
    mesh: Option<RawMesh>,
    /// (target object id, transform) — accepted with or without a
    /// `<components>` wrapper.
    components: Vec<(u32, [f32; 12])>,
}

struct Collected {
    unit_scale: f32,
    /// basematerials and colorgroup tables alike, by resource id. Slots are
    /// positional: an unparseable color still occupies its p1 index.
    tables: HashMap<u32, Vec<Option<[u8; 3]>>>,
    objects: Vec<RawObject>,
    /// (object id, transform) per `<item>`.
    build: Vec<(u32, [f32; 12])>,
}

fn collect(xml: &str) -> Result<Collected, ThreemfError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut c = Collected {
        unit_scale: 1.0,
        tables: HashMap::new(),
        objects: Vec::new(),
        build: Vec::new(),
    };
    // Resource id of the basematerials/colorgroup currently open; `<base>`
    // colors come from `displaycolor`, `<color>` colors from `color`.
    let mut table: Option<u32> = None;
    let mut cur: Option<RawObject> = None;
    let mut in_mesh = false;

    loop {
        let ev = reader
            .read_event()
            .map_err(|e| ThreemfError::Xml(e.to_string()))?;
        match ev {
            // Leaf elements may arrive as `<vertex/>` or `<vertex></vertex>`;
            // only containers need their End events tracked.
            Event::Start(e) | Event::Empty(e)
                if matches!(
                    e.local_name().as_ref(),
                    b"base" | b"color" | b"vertex" | b"triangle" | b"component" | b"item"
                ) =>
            {
                match e.local_name().as_ref() {
                    b"base" => {
                        if let Some(id) = table {
                            let col = attr(&e, b"displaycolor").as_deref().and_then(hex_color);
                            c.tables.get_mut(&id).unwrap().push(col);
                        }
                    }
                    b"color" => {
                        if let Some(id) = table {
                            let col = attr(&e, b"color").as_deref().and_then(hex_color);
                            c.tables.get_mut(&id).unwrap().push(col);
                        }
                    }
                    b"vertex" => {
                        if in_mesh
                            && let Some(o) = cur.as_mut()
                            && let Some(m) = o.mesh.as_mut()
                        {
                            m.vertices.push([
                                attr(&e, b"x").and_then(|v| v.parse().ok()).unwrap_or(0.0),
                                attr(&e, b"y").and_then(|v| v.parse().ok()).unwrap_or(0.0),
                                attr(&e, b"z").and_then(|v| v.parse().ok()).unwrap_or(0.0),
                            ]);
                        }
                    }
                    b"triangle" => {
                        if in_mesh
                            && let Some(o) = cur.as_mut()
                            && let Some(m) = o.mesh.as_mut()
                        {
                            m.triangles.push(RawTriangle {
                                v: [
                                    attr(&e, b"v1").and_then(|v| v.parse().ok()).unwrap_or(0),
                                    attr(&e, b"v2").and_then(|v| v.parse().ok()).unwrap_or(0),
                                    attr(&e, b"v3").and_then(|v| v.parse().ok()).unwrap_or(0),
                                ],
                                pid: attr(&e, b"pid").and_then(|v| v.parse().ok()),
                                p1: attr(&e, b"p1").and_then(|v| v.parse().ok()),
                            });
                        }
                    }
                    b"component" => {
                        if let Some(o) = cur.as_mut() {
                            o.components.push((
                                attr(&e, b"objectid")
                                    .and_then(|v| v.parse().ok())
                                    .unwrap_or(0),
                                parse_xf(attr(&e, b"transform")),
                            ));
                        }
                    }
                    b"item" => {
                        c.build.push((
                            attr(&e, b"objectid")
                                .and_then(|v| v.parse().ok())
                                .unwrap_or(0),
                            parse_xf(attr(&e, b"transform")),
                        ));
                    }
                    _ => {}
                }
            }
            Event::Start(e) => match e.local_name().as_ref() {
                b"model" => {
                    if let Some(u) = attr(&e, b"unit") {
                        c.unit_scale = unit_scale(&u);
                    }
                }
                b"basematerials" | b"colorgroup" => {
                    if let Some(id) = attr(&e, b"id").and_then(|v| v.parse().ok()) {
                        c.tables.entry(id).or_default();
                        table = Some(id);
                    }
                }
                b"object" => {
                    cur = Some(RawObject {
                        id: attr(&e, b"id").and_then(|v| v.parse().ok()).unwrap_or(0),
                        name: attr(&e, b"name").unwrap_or_default(),
                        pid: attr(&e, b"pid").and_then(|v| v.parse().ok()),
                        pindex: attr(&e, b"pindex").and_then(|v| v.parse().ok()),
                        mesh: None,
                        components: Vec::new(),
                    });
                    in_mesh = false;
                }
                b"mesh" => {
                    in_mesh = true;
                    if let Some(o) = cur.as_mut() {
                        o.mesh.get_or_insert_with(|| RawMesh {
                            vertices: Vec::new(),
                            triangles: Vec::new(),
                        });
                    }
                }
                _ => {}
            },
            Event::End(e) => match e.local_name().as_ref() {
                b"basematerials" | b"colorgroup" => table = None,
                b"mesh" => in_mesh = false,
                b"object" => {
                    if let Some(o) = cur.take() {
                        c.objects.push(o);
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(c)
}

fn attr(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == key)
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

/// Model units to millimeters. Millimeter (and anything unrecognized) is 1:
/// the app's native unit is mm.
fn unit_scale(unit: &str) -> f32 {
    match unit {
        "micron" => 0.001,
        "centimeter" => 10.0,
        "inch" => 25.4,
        "foot" => 304.8,
        "meter" => 1000.0,
        _ => 1.0,
    }
}

/// `#RRGGBB` or `#RRGGBBAA`; alpha is dropped — the renderer has no
/// per-triangle alpha. Anything else yields no color.
fn hex_color(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    let v = u32::from_str_radix(h, 16).ok()?;
    match h.len() {
        6 => Some([(v >> 16) as u8, (v >> 8) as u8, v as u8]),
        8 => Some([(v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8]),
        _ => None,
    }
}

/// Not exactly 12 numbers degrades to identity, tolerating exporter noise.
fn parse_xf(s: Option<String>) -> [f32; 12] {
    match s {
        Some(s) => {
            let t: Vec<f32> = s
                .split_whitespace()
                .map(|v| v.parse().unwrap_or(0.0))
                .collect();
            if t.len() == 12 {
                t.try_into().unwrap()
            } else {
                IDENT
            }
        }
        None => IDENT,
    }
}

/// 3MF spec token order: 4 rows × 3, row-vector application (p' = p·M);
/// tokens 9/10/11 are the translation.
fn apply(p: [f32; 3], m: &[f32; 12]) -> [f32; 3] {
    [
        p[0] * m[0] + p[1] * m[3] + p[2] * m[6] + m[9],
        p[0] * m[1] + p[1] * m[4] + p[2] * m[7] + m[10],
        p[0] * m[2] + p[1] * m[5] + p[2] * m[8] + m[11],
    ]
}

/// `child` applied first, then `parent`: p·(child·parent). Both matrices
/// carry the implicit (0,0,0,1) last column, so the 4×3 product needs no
/// homogeneous row.
fn compose(child: &[f32; 12], parent: &[f32; 12]) -> [f32; 12] {
    let mut out = [0.0f32; 12];
    for i in 0..4 {
        for j in 0..3 {
            out[3 * i + j] = child[3 * i] * parent[j]
                + child[3 * i + 1] * parent[3 + j]
                + child[3 * i + 2] * parent[6 + j]
                + if i == 3 { parent[9 + j] } else { 0.0 };
        }
    }
    out
}

/// Geometry accumulated while flattening one build item's component tree.
struct Flat {
    positions: Vec<[f32; 3]>,
    colors: Vec<TriangleColor>,
    /// First non-empty mesh-object name in walk order — the name an unnamed
    /// item object falls through to.
    mesh_name: Option<String>,
    has_mesh: bool,
}

fn resolve(c: &Collected) -> Result<Vec<Part>, ThreemfError> {
    let objects: HashMap<u32, &RawObject> = c.objects.iter().map(|o| (o.id, o)).collect();

    // No build items: one part per object id, sorted as strings ("10" < "2").
    let roots: Vec<(u32, [f32; 12])> = if c.build.is_empty() {
        let mut ids: Vec<u32> = c.objects.iter().map(|o| o.id).collect();
        ids.sort_by_key(|id| id.to_string());
        ids.into_iter().map(|id| (id, IDENT)).collect()
    } else {
        c.build.to_vec()
    };

    let mut parts = Vec::new();
    for (oid, xf) in roots {
        let mut flat = Flat {
            positions: Vec::new(),
            colors: Vec::new(),
            mesh_name: None,
            has_mesh: false,
        };
        flatten(&objects, &c.tables, c.unit_scale, oid, xf, 0, &mut flat)?;
        if !flat.has_mesh {
            continue; // dangling reference or component-only object: nothing to show
        }
        let own = objects
            .get(&oid)
            .map(|o| o.name.clone())
            .unwrap_or_default();
        parts.push(Part {
            name: if !own.is_empty() {
                own
            } else {
                flat.mesh_name.unwrap_or_default()
            },
            positions: flat.positions,
            triangle_colors: flat.colors,
        });
    }
    // Objects with no name anywhere in their chain get a positional one.
    for (i, p) in parts.iter_mut().enumerate() {
        if p.name.is_empty() {
            p.name = format!("part {}", i + 1);
        }
    }
    Ok(parts)
}

/// Depth-first walk of one object: mesh triangles append per-corner,
/// transformed, unit-scaled geometry; components recurse with the child
/// transform composed onto the accumulated one.
fn flatten(
    objects: &HashMap<u32, &RawObject>,
    tables: &HashMap<u32, Vec<Option<[u8; 3]>>>,
    unit_scale: f32,
    oid: u32,
    m: [f32; 12],
    depth: usize,
    flat: &mut Flat,
) -> Result<(), ThreemfError> {
    if depth > MAX_DEPTH {
        return Err(ThreemfError::TooDeep);
    }
    let Some(o) = objects.get(&oid).copied() else {
        return Ok(()); // dangling objectid reference: skip the subtree
    };
    if let Some(mesh) = &o.mesh {
        flat.has_mesh = true;
        if flat.mesh_name.is_none() && !o.name.is_empty() {
            flat.mesh_name = Some(o.name.clone());
        }
        for t in &mesh.triangles {
            let corners = match (
                mesh.vertices.get(t.v[0]),
                mesh.vertices.get(t.v[1]),
                mesh.vertices.get(t.v[2]),
            ) {
                (Some(a), Some(b), Some(d)) => [*a, *b, *d],
                _ => continue, // dangling vertex index: drop the triangle
            };
            // Triangle pid/p1, else the object's pid/pindex, else uncolored.
            let pid = t.pid.or(o.pid);
            let idx = t.p1.or(o.pindex);
            flat.colors
                .push(match (pid.and_then(|p| tables.get(&p)), idx) {
                    (Some(table), Some(i)) => table.get(i).copied().flatten(),
                    _ => None,
                });
            for v in corners {
                let q = apply(v, &m);
                flat.positions
                    .push([q[0] * unit_scale, q[1] * unit_scale, q[2] * unit_scale]);
            }
        }
    }
    for (target, cxf) in &o.components {
        flatten(
            objects,
            tables,
            unit_scale,
            *target,
            compose(cxf, &m),
            depth + 1,
            flat,
        )?;
    }
    Ok(())
}
