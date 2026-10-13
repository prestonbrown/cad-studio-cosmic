//! Part arrangement: grouping, explode offsets, and the plate flat layout —
//! the pure MODEL-space math behind the PartsLayout control, transcribed
//! from PartArrangement.swift. Upstream rebuilds one merged `TriangleMesh`
//! with offsets baked into the vertices; this port keeps every part's
//! geometry staged once and states the arrangement as [`Arranged`] — a
//! per-slot MODEL-space offset and visibility the renderer applies as
//! per-part data (the GPU-first law, spec §9.6).
//!
//! Port notes, all deliberate and narrow:
//! - the anchor is the FIRST maximal-volume group (Swift `max(by:)` keeps
//!   the first maximum; Rust's `Iterator::max_by` keeps the last — hence
//!   the explicit fold in [`exploded_offsets`]);
//! - upstream's two orderings (explode candidates by center distance,
//!   flat layout by depth) use Swift's `sort`, which is not guaranteed
//!   stable; Rust's `sort_by` is, so exact ties keep first-appearance
//!   order — a divergence only on ties;
//! - the explode floor lift's two minZ inputs see different part sets,
//!   exactly as upstream: the assembled floor iterates every part —
//!   hidden included (:78) — while the exploded minimum is hidden-filtered
//!   by construction (the mesh holds visible parts, :74-85). A hidden part
//!   sitting below everything therefore drags the exploded pile down to
//!   its floor; the hide stays observable through the fixed camera and
//!   grid frame against the translated pile.

use std::collections::{BTreeSet, HashMap};

use engine::parsing::Part;
use engine::settings::PartsLayout;

/// One row of the parts tree / legend: a base name shared by >1 part with
/// its members, or a lone part under its full name (PartArrangement.swift:
/// 26-32). `color` is the first `Some` triangle color of the first member
/// (:50) — the swatch upstream shows beside the group.
#[derive(Debug, Clone, PartialEq)]
pub struct PartGroup {
    pub name: String,
    pub members: Vec<String>,
    pub color: Option<[u8; 3]>,
}

/// A layout's answer, per input model-part slot: the MODEL-space offset to
/// add to the part's staged position, whether it draws, and whether the
/// caller should swap in the plate scene instead of the model scene.
#[derive(Debug, Clone)]
pub struct Arranged {
    pub offsets: Vec<[f32; 3]>,
    pub visible: Vec<bool>,
    /// True iff the layout is Plate AND the agent produced plate parts: the
    /// offsets/visibility arrays stay model-parts-shaped (zero offsets; the
    /// hidden filter still applies), and the caller renders `plate_parts`
    /// re-filtered by group instead of the model scene.
    pub plate_scene: bool,
}

/// Strip the trailing index from a part name: `[_\-\s]?\d+$` — all trailing
/// digits plus at most one separator — falling back to the full name when
/// that leaves nothing (`"2026"` stays `"2026"`) (PartArrangement.swift:35-38).
pub fn group_name(part: &str) -> String {
    let digits = part.trim_end_matches(|c: char| c.is_ascii_digit());
    let base = match (digits.len() < part.len(), digits.chars().next_back()) {
        // digits were stripped and one separator goes with them
        (true, Some(c)) if c == '_' || c == '-' || c.is_whitespace() => {
            &digits[..digits.len() - c.len_utf8()]
        }
        _ => digits,
    };
    if base.is_empty() {
        part.to_string()
    } else {
        base.to_string()
    }
}

/// Group parts by base name: a base with more than one member becomes a
/// group under the base name, singletons stay under their full part names.
/// Groups come in first-appearance order; `×N` counts are `members.len()`
/// (PartArrangement.swift:40-54).
pub fn groups(parts: &[Part]) -> Vec<PartGroup> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for part in parts {
        *counts.entry(group_name(&part.name)).or_default() += 1;
    }
    let mut out: Vec<PartGroup> = Vec::new();
    for part in parts {
        let base = group_name(&part.name);
        let name = if counts[&base] > 1 {
            base
        } else {
            part.name.clone()
        };
        match out.iter_mut().find(|g| g.name == name) {
            Some(g) => g.members.push(part.name.clone()),
            None => out.push(PartGroup {
                name,
                members: vec![part.name.clone()],
                color: part.triangle_colors.iter().flatten().copied().next(),
            }),
        }
    }
    out
}

/// The color legend: distinct triangle colors in first-appearance order —
/// upstream renders `mesh.colors.prefix(12)` with a total count beside
/// (ModelPane.swift:202-213). `None` (uncolored) triangles never list.
pub fn legend_colors(parts: &[Part]) -> Vec<[u8; 3]> {
    let mut out: Vec<[u8; 3]> = Vec::new();
    for part in parts {
        for color in part.triangle_colors.iter().flatten() {
            if !out.contains(color) {
                out.push(*color);
            }
        }
    }
    out
}

/// Arrange `model_parts` for a layout (PartArrangement.swift:56-93):
/// - **Assembled** — identity offsets, hidden groups filtered out.
/// - **Exploded** — per-group push offsets from [`exploded_offsets`], then
///   the floor lift re-pins the whole pile (anchor included) so the
///   exploded minimum sits at the assembled floor (:77-85). Hidden groups
///   still receive offsets — the collision math must see them (facts #58) —
///   they just never draw.
/// - **Plate** — the agent's `plate_parts` win when present ([`Arranged::
///   plate_scene`], the caller swaps scenes); otherwise the flat-layout
///   fallback lays every part on the plate ([`flat_offsets`], :176-203).
///
/// Hidden and attached are GROUP-name sets, matching upstream's keys.
pub fn arrange(
    model_parts: &[Part],
    plate_parts: &[Part],
    layout: PartsLayout,
    explode: f32,
    hidden: &BTreeSet<String>,
    attached: &BTreeSet<String>,
) -> Arranged {
    // part name -> group name (:64-65); every model part is some group's
    // member, and the fallback mirrors upstream's `?? groupName(of:)`
    let group_of: HashMap<String, String> = groups(model_parts)
        .iter()
        .flat_map(|g| g.members.iter().map(move |m| (m.clone(), g.name.clone())))
        .collect();
    let group = |name: &str| -> String {
        group_of
            .get(name)
            .cloned()
            .unwrap_or_else(|| group_name(name))
    };
    let visible: Vec<bool> = model_parts
        .iter()
        .map(|p| !hidden.contains(&group(&p.name)))
        .collect();
    let mut offsets = vec![[0.0f32; 3]; model_parts.len()];
    let mut plate_scene = false;
    match layout {
        PartsLayout::Assembled => {}
        PartsLayout::Exploded => {
            let group_offsets = exploded_offsets(model_parts, &group_of, explode, attached);
            for (slot, part) in model_parts.iter().enumerate() {
                if let Some(o) = group_offsets.get(&group(&part.name)) {
                    offsets[slot] = *o;
                }
            }
            // Floor lift (:77-85): the assembled floor over ALL parts with
            // triangles, hidden included (:78); the exploded minimum over
            // visible ones — exactly the geometry upstream's mesh holds
            // (:74-85).
            let assembled_floor = model_parts
                .iter()
                .filter(|p| p.triangle_count() > 0)
                .map(|p| p.aabb().0[2])
                .reduce(f32::min)
                .unwrap_or(0.0);
            let exploded_min = model_parts
                .iter()
                .enumerate()
                .filter(|(slot, p)| visible[*slot] && p.triangle_count() > 0)
                .map(|(slot, p)| p.aabb().0[2] + offsets[slot][2])
                .reduce(f32::min)
                .unwrap_or(0.0);
            let lift = assembled_floor - exploded_min;
            for o in &mut offsets {
                o[2] += lift;
            }
        }
        PartsLayout::Plate => {
            if plate_parts.is_empty() {
                offsets = flat_offsets(model_parts);
            } else {
                // the caller swaps in the plate scene; model slots carry
                // only the hidden filter, so offsets stay identity
                plate_scene = true;
            }
        }
    }
    Arranged {
        offsets,
        visible,
        plate_scene,
    }
}

/// The aabb the section plane, cap, and grid should frame: per-part
/// `aabb()` translated by its arrangement offset, over VISIBLE slots only —
/// upstream sections and grids against the arranged mesh, which excludes
/// hidden parts (ModelViewerState.swift:116-121). When no slot is visible
/// (the all-hidden fallback's input) it degrades to the all-parts union —
/// WITH each slot's arrangement offset applied, i.e. the arranged-but-
/// invisible scene, not the assembled box (an all-hidden Exploded fold
/// carries the explode pushes). T10 caller contract: that degrade feeds
/// framing only, so the all-hidden frame matches the layout the user lands
/// in on unhiding; section/cap/grid positions stay visible-only domains.
/// An empty scene keeps a finite zero box for the camera math (the
/// `PartScene` rule).
pub fn effective_model_aabb(parts: &[Part], arranged: &Arranged) -> ([f32; 3], [f32; 3]) {
    assert_eq!(
        arranged.offsets.len(),
        parts.len(),
        "Arranged is model-parts-shaped"
    );
    assert_eq!(arranged.visible.len(), parts.len());
    let mut slots: Vec<usize> = (0..parts.len()).filter(|&i| arranged.visible[i]).collect();
    if slots.is_empty() {
        slots = (0..parts.len()).collect();
    }
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    let mut any = false;
    for i in slots {
        if parts[i].triangle_count() == 0 {
            continue; // upstream's mesh holds triangles only
        }
        let (a, b) = parts[i].aabb();
        let o = &arranged.offsets[i];
        for k in 0..3 {
            mn[k] = mn[k].min(a[k] + o[k]);
            mx[k] = mx[k].max(b[k] + o[k]);
        }
        any = true;
    }
    if any { (mn, mx) } else { ([0.0; 3], [0.0; 3]) }
}

/// An axis-aligned box: (min, max), MODEL space.
type Box3 = ([f32; 3], [f32; 3]);

/// Per-group explode offsets (PartArrangement.swift:95-140), keyed by group
/// name — every member of a group rides its group's offset. Groups whose
/// parts are all triangle-less never enter the bounds and stay at zero;
/// the anchor and attached groups have no entry (identity at consumption).
fn exploded_offsets(
    parts: &[Part],
    group_of: &HashMap<String, String>,
    explode: f32,
    attached: &BTreeSet<String>,
) -> HashMap<String, [f32; 3]> {
    // bounds per group over ALL parts with triangles — hidden groups
    // included (facts #58) — in first-appearance order (:101-111)
    let mut bounds: HashMap<String, Box3> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for part in parts.iter().filter(|p| p.triangle_count() > 0) {
        let name = group_of
            .get(&part.name)
            .cloned()
            .unwrap_or_else(|| group_name(&part.name));
        let (mn, mx) = part.aabb();
        match bounds.get_mut(&name) {
            Some((gmn, gmx)) => {
                for a in 0..3 {
                    gmn[a] = gmn[a].min(mn[a]);
                    gmx[a] = gmx[a].max(mx[a]);
                }
            }
            None => {
                bounds.insert(name.clone(), (mn, mx));
                order.push(name);
            }
        }
    }
    // anchor = FIRST maximal-volume group in first-appearance order
    // (:112) — Swift's max(by:) keeps the first maximum, Rust's max_by
    // the last, so the fold is explicit
    let mut anchor: Option<(&str, Box3)> = None;
    for name in &order {
        let v = box_volume(&bounds[name]);
        if anchor.as_ref().map_or(true, |(_, b)| v > box_volume(b)) {
            anchor = Some((name, bounds[name]));
        }
    }
    let Some((anchor, anchor_box)) = anchor else {
        return HashMap::new();
    };
    let anchor_center = box_center(&anchor_box);
    // gap from the overall diagonal (:116-117)
    let mut overall = anchor_box;
    for b in bounds.values() {
        for a in 0..3 {
            overall.0[a] = overall.0[a].min(b.0[a]);
            overall.1[a] = overall.1[a].max(b.1[a]);
        }
    }
    let diag = glam::Vec3::from_array(overall.1) - glam::Vec3::from_array(overall.0);
    let gap = (diag.length() * 0.06).max(2.0);
    let mut placed: Vec<Box3> = vec![anchor_box];
    let mut offsets = HashMap::new();
    // others ascending by center distance to the anchor center (:120-122);
    // stable sort, so exact distance ties keep first-appearance order
    let mut others: Vec<&str> = order
        .iter()
        .map(|n| n.as_str())
        .filter(|&n| n != anchor)
        .collect();
    others.sort_by(|a, b| {
        center_distance(&bounds[*a], anchor_center)
            .total_cmp(&center_distance(&bounds[*b], anchor_center))
    });
    for name in others {
        let box3 = bounds[name];
        if attached.contains(name) {
            placed.push(box3); // placed as-is, no offset (:125-128)
            continue;
        }
        let direction = dominant_axis(sub(box_center(&box3), anchor_center));
        let step = (length(box_size(&box3)) * 0.05).max(0.5);
        let mut travel = 0.0f32;
        while travel < 10_000.0
            && placed
                .iter()
                .any(|p| overlaps(&box3, p, scaled(direction, travel), gap))
        {
            travel += step;
        }
        let shift = scaled(direction, travel + gap);
        placed.push((add(box3.0, shift), add(box3.1, shift)));
        offsets.insert(name.to_string(), scaled(shift, explode));
    }
    offsets
}

/// The plate flat-layout fallback (PartArrangement.swift:176-203): rows of
/// parts sorted descending by MODEL size.y, wrapped at the area-derived row
/// width, floored onto z=0, then centered as a whole. Per-slot offsets;
/// triangle-less parts stay at zero (they never place).
fn flat_offsets(parts: &[Part]) -> Vec<[f32; 3]> {
    let gap = 5.0f32;
    let mut items: Vec<(usize, Box3)> = parts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.triangle_count() > 0)
        .map(|(i, p)| (i, p.aabb()))
        .collect();
    let area: f32 = items
        .iter()
        .map(|(_, b)| (b.1[0] - b.0[0] + gap) * (b.1[1] - b.0[1] + gap))
        .sum();
    let row_width = (area.sqrt() * 1.3).max(
        items
            .iter()
            .map(|(_, b)| b.1[0] - b.0[0])
            .fold(0.0, f32::max),
    );
    // descending by size.y (:184); stable, so equal depths keep
    // first-appearance order
    items.sort_by(|a, b| box_size(&b.1)[1].total_cmp(&box_size(&a.1)[1]));
    let mut offsets = vec![[0.0f32; 3]; parts.len()];
    let mut cursor = [0.0f32; 2];
    let mut row_depth = 0.0f32;
    // componentwise max over placed boxes, seeded at zero (:196) — feeds
    // the final centering shift
    let mut max_placed = [0.0f32; 3];
    for (slot, b) in items {
        let size = box_size(&b);
        if cursor[0] > 0.0 && cursor[0] + size[0] > row_width {
            cursor = [0.0, cursor[1] + row_depth + gap];
            row_depth = 0.0;
        }
        // (cursor.x − min.x, cursor.y − min.y, −min.z): the z term FLOORS
        // the part onto the plate — a part floating at MODEL z 40..60
        // drops to 0..20 (:191)
        let off = [cursor[0] - b.0[0], cursor[1] - b.0[1], -b.0[2]];
        for a in 0..3 {
            max_placed[a] = max_placed[a].max(b.1[a] + off[a]);
        }
        offsets[slot] = off;
        cursor[0] += size[0] + gap;
        row_depth = row_depth.max(size[1]);
    }
    let shift = [-max_placed[0] / 2.0, -max_placed[1] / 2.0, 0.0];
    for off in &mut offsets {
        for a in 0..3 {
            off[a] += shift[a];
        }
    }
    offsets
}

/// Box volume, the anchor criterion (:142-146).
fn box_volume(b: &Box3) -> f32 {
    let s = box_size(b);
    s[0] * s[1] * s[2]
}

/// Box extents per axis.
fn box_size(b: &Box3) -> [f32; 3] {
    sub(b.1, b.0)
}

/// Distance from a box's center to a point (:148-151).
fn center_distance(b: &Box3, point: [f32; 3]) -> f32 {
    length(sub(box_center(b), point))
}

/// Box center.
fn box_center(b: &Box3) -> [f32; 3] {
    scaled(add(b.0, b.1), 0.5)
}

/// Unit axis of `v`, z-priority ladder: z wins ties with x/y, then x wins
/// ties with y; all components <= 0.01 is degenerate and reads +Z
/// (PartArrangement.swift:153-163).
fn dominant_axis(v: [f32; 3]) -> [f32; 3] {
    let m = [v[0].abs(), v[1].abs(), v[2].abs()];
    if m[0].max(m[1]).max(m[2]) <= 0.01 {
        return [0.0, 0.0, 1.0];
    }
    if m[2] >= m[0] && m[2] >= m[1] {
        return [0.0, 0.0, if v[2] > 0.0 { 1.0 } else { -1.0 }];
    }
    if m[0] >= m[1] {
        return [if v[0] > 0.0 { 1.0 } else { -1.0 }, 0.0, 0.0];
    }
    [0.0, if v[1] > 0.0 { 1.0 } else { -1.0 }, 0.0]
}

/// Does `b`, translated by `shift`, overlap `other` with `margin`'s half
/// inflated into each side of `other`? Strict on both edges
/// (PartArrangement.swift:165-174).
fn overlaps(b: &Box3, other: &Box3, shift: [f32; 3], margin: f32) -> bool {
    let m = margin * 0.5;
    (0..3).all(|a| b.0[a] + shift[a] < other.1[a] + m && b.1[a] + shift[a] > other.0[a] - m)
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scaled(v: [f32; 3], k: f32) -> [f32; 3] {
    [v[0] * k, v[1] * k, v[2] * k]
}

fn length(v: [f32; 3]) -> f32 {
    glam::Vec3::from_array(v).length()
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::settings::PartsLayout as PL;
    use std::collections::BTreeSet;

    fn part(name: &str, min: [f32; 3], max: [f32; 3]) -> engine::parsing::Part {
        // one big triangle covering the box corners is enough for aabb math;
        // a second keeps it non-degenerate
        let positions = vec![
            min,
            max,
            [min[0], max[1], max[2]],
            min,
            [max[0], min[1], max[2]],
            max,
        ];
        engine::parsing::Part {
            name: name.into(),
            positions,
            triangle_colors: vec![Some([1, 2, 3]); 2],
        }
    }

    #[test]
    fn group_names_match_upstream_table() {
        assert_eq!(group_name("bolt_1"), "bolt");
        assert_eq!(group_name("bolt-12"), "bolt");
        assert_eq!(group_name("screw 3"), "screw");
        assert_eq!(group_name("m4"), "m"); // no separator still strips
        assert_eq!(group_name("bolt"), "bolt"); // no trailing digits: intact
        assert_eq!(group_name("2026"), "2026"); // strip leaves EMPTY -> full name returns
    }

    #[test]
    fn singleton_bases_stay_ungrouped() {
        let parts = vec![
            part("bolt", [0.0; 3], [1.0; 3]),
            part("nut", [2.0; 3], [3.0; 3]),
        ];
        let gs = groups(&parts);
        assert_eq!(
            gs.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
            ["bolt", "nut"],
            "a base with one member is not a group"
        );
        let parts = vec![
            part("bolt_1", [0.0; 3], [1.0; 3]),
            part("bolt_2", [2.0; 3], [3.0; 3]),
        ];
        let gs = groups(&parts);
        assert_eq!(gs.len(), 1);
        assert_eq!(gs[0].name, "bolt");
        assert_eq!(gs[0].members, ["bolt_1", "bolt_2"]);
        assert_eq!(gs[0].color, Some([1, 2, 3]));
    }

    #[test]
    fn explode_pushes_the_small_part_away_from_the_volume_anchor() {
        // base 10x10x10 at origin (volume 1000, anchor), lid 2x2x2 at (5,5,12):
        // lid center is +z of anchor center -> dominant axis +Z, explode 1.
        let parts = vec![
            part("base", [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            part("lid", [5.0, 5.0, 12.0], [7.0, 7.0, 14.0]),
        ];
        let arranged = arrange(
            &parts,
            &[],
            PL::Exploded,
            1.0,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        assert_eq!(arranged.offsets.len(), 2);
        assert_eq!(arranged.offsets[0], [0.0, 0.0, 0.0], "anchor does not move");
        let off = arranged.offsets[1];
        assert!(
            off[2] > 0.0 && off[0] == 0.0 && off[1] == 0.0,
            "pushed along +Z: {off:?}"
        );
        // travel marches until clear of the anchor box + gap: offset.z >= gap,
        // gap = max(length(overall diagonal) * 0.06, 2)
        let (mn, mx) = parts.iter().map(|p| p.aabb()).fold(
            ([f32::MAX; 3], [f32::MIN; 3]),
            |(mut mn, mut mx), (a, b)| {
                for i in 0..3 {
                    mn[i] = mn[i].min(a[i]);
                    mx[i] = mx[i].max(b[i]);
                }
                (mn, mx)
            },
        );
        let diag = (mx[0] - mn[0]).hypot(mx[1] - mn[1]).hypot(mx[2] - mn[2]);
        let gap = (diag * 0.06).max(2.0);
        assert!(
            off[2] >= gap - 1e-3,
            "offset at least the gap: {:?} >= {gap}",
            off[2]
        );
        // explode 2 doubles the push; explode 0 collapses to assembled (lift 0 here)
        let x2 = arrange(
            &parts,
            &[],
            PL::Exploded,
            2.0,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        assert!((x2.offsets[1][2] - 2.0 * off[2]).abs() < 1e-3);
        let x0 = arrange(
            &parts,
            &[],
            PL::Exploded,
            0.0,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        assert_eq!(x0.offsets[1], [0.0, 0.0, 0.0]);
    }

    #[test]
    fn explode_floor_lift_repins_the_pile() {
        // anchor base z 0..10; small cube BELOW it z -20..-18 gets pushed
        // FURTHER down on explode — the lift then shifts the whole pile so
        // the exploded minimum sits at the assembled floor (-20).
        let parts = vec![
            part("base", [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            part("nut", [4.0, 4.0, -20.0], [6.0, 6.0, -18.0]),
        ];
        let arranged = arrange(
            &parts,
            &[],
            PL::Exploded,
            1.0,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        let aabb = effective_model_aabb(&parts, &arranged);
        assert!(
            (aabb.0[2] - (-20.0)).abs() < 1e-3,
            "exploded floor re-pinned to the assembled floor: {:?}",
            aabb
        );
        let assembled = arrange(
            &parts,
            &[],
            PL::Assembled,
            1.0,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        let a2 = effective_model_aabb(&parts, &assembled);
        assert!((a2.0[2] - (-20.0)).abs() < 1e-3);
        // hidden-lowest: the assembled floor reads the hidden part's z too
        // (:78 iterates ALL parts), so the pile re-pins to -40, not the
        // visible -20; the exploded-min side stays visible-only (the mesh,
        // :74-85)
        let mut hidden = BTreeSet::new();
        hidden.insert("bolt_1".to_string());
        let parts_h = vec![
            part("base", [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            part("nut", [4.0, 4.0, -20.0], [6.0, 6.0, -18.0]),
            part("bolt_1", [4.0, 4.0, -40.0], [6.0, 6.0, -38.0]),
        ];
        let h = arrange(&parts_h, &[], PL::Exploded, 1.0, &hidden, &BTreeSet::new());
        let haabb = effective_model_aabb(&parts_h, &h);
        assert!(
            (haabb.0[2] - (-40.0)).abs() < 1e-3,
            "hidden lowest part pins the lift: {:?}",
            haabb.0
        );
    }

    #[test]
    fn explode_march_clears_an_inflated_overlap() {
        // plug 2x2x2 sits INSIDE the 10x10x10 base: identical centers make
        // the direction degenerate (+Z, :155) and the march is the whole
        // push — the overlap ends once plug.min.z + travel reaches
        // base.max.z + gap/2 (4 + 7 = 11), so travel = 7 and
        // offset.z = travel + gap = 9. A port whose overlaps() never fires
        // stops at travel 0 and pushes only the gap (2.0) — this test is
        // that march-core's discriminator.
        let parts = vec![
            part("base", [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            part("plug", [4.0, 4.0, 4.0], [6.0, 6.0, 6.0]),
        ];
        let arranged = arrange(
            &parts,
            &[],
            PL::Exploded,
            1.0,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        let off = arranged.offsets[1];
        assert!(
            off[0] == 0.0 && off[1] == 0.0,
            "degenerate center diff reads +Z: {off:?}"
        );
        assert!(
            (off[2] - 9.0).abs() < 1e-3,
            "march clears the margin-inflated anchor box: travel 7 + gap 2 = 9, got {off:?}"
        );
    }

    #[test]
    fn attached_groups_stay_put_and_hidden_groups_filter_draws_only() {
        let parts = vec![
            part("base", [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            part("lid", [5.0, 5.0, 12.0], [7.0, 7.0, 14.0]),
        ];
        let mut attached = BTreeSet::new();
        attached.insert("lid".to_string());
        let a = arrange(&parts, &[], PL::Exploded, 1.0, &BTreeSet::new(), &attached);
        assert_eq!(
            a.offsets[1],
            [0.0, 0.0, 0.0],
            "attached group is placed as-is"
        );
        let mut hidden = BTreeSet::new();
        hidden.insert("lid".to_string());
        let h = arrange(&parts, &[], PL::Exploded, 1.0, &hidden, &BTreeSet::new());
        assert!(!h.visible[1] && h.visible[0], "hidden filters visibility");
        assert!(
            h.offsets[1] != [0.0, 0.0, 0.0],
            "hidden groups still get offsets (collision math)"
        );
        let haabb = effective_model_aabb(&parts, &h);
        assert!(
            (haabb.1[2] - 10.0).abs() < 1e-4,
            "effective aabb excludes the hidden lid"
        );
    }

    #[test]
    fn flat_layout_rows_and_centers() {
        // three 10x4 slabs: row width from the area formula = max(sqrt(405)*1.3, 10)
        // = 26.16 -> two per row, third wraps -> 2 rows of depth 4 + gap 5
        let parts = vec![
            part("a", [0.0, 0.0, 0.0], [10.0, 4.0, 5.0]),
            part("b", [0.0, 0.0, 0.0], [10.0, 4.0, 5.0]),
            part("c", [0.0, 0.0, 0.0], [10.0, 4.0, 5.0]),
        ];
        let arranged = arrange(
            &parts,
            &[],
            PL::Plate,
            1.0,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        assert!(
            !arranged.plate_scene,
            "no plate.3mf -> flat fallback transforms"
        );
        assert!(arranged.visible.iter().all(|v| *v));
        let aabb = effective_model_aabb(&parts, &arranged);
        let sx = aabb.1[0] - aabb.0[0];
        let sy = aabb.1[1] - aabb.0[1];
        assert!(
            (sx - 25.0).abs() < 1e-3,
            "two per row (0..10 + gap + 15..25): {sx}"
        );
        assert!((sy - 13.0).abs() < 1e-3, "two rows (4 + gap 5 + 4): {sy}");
        assert!((aabb.0[0] + aabb.1[0]).abs() < 1e-3, "x centered");
        assert!((aabb.0[1] + aabb.1[1]).abs() < 1e-3, "y centered");
        assert!(
            (aabb.0[2] - 0.0).abs() < 1e-3,
            "z floored onto the plate (parts already at z=0)"
        );
        // a floating part discriminates the z flooring: MODEL z 40..60 drops to 0..20
        let parts2 = vec![
            part("a", [0.0, 0.0, 0.0], [10.0, 4.0, 5.0]),
            part("f", [0.0, 0.0, 40.0], [10.0, 4.0, 60.0]),
        ];
        let arranged2 = arrange(
            &parts2,
            &[],
            PL::Plate,
            1.0,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        let aabb2 = effective_model_aabb(&parts2, &arranged2);
        assert!(
            (aabb2.0[2] - 0.0).abs() < 1e-3,
            "floating part floored to the plate: {:?}",
            aabb2.0
        );
        assert!(
            (aabb2.1[2] - 20.0).abs() < 1e-3,
            "its height above the plate is preserved"
        );
    }

    #[test]
    fn agent_plate_parts_select_the_plate_scene() {
        let model = vec![part("a", [0.0; 3], [1.0; 3])];
        let plate = vec![part("a", [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])];
        let arranged = arrange(
            &model,
            &plate,
            PL::Plate,
            1.0,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        assert!(arranged.plate_scene);
        let assembled = arrange(
            &model,
            &plate,
            PL::Assembled,
            1.0,
            &BTreeSet::new(),
            &BTreeSet::new(),
        );
        assert!(!assembled.plate_scene);
        assert!(assembled.offsets.iter().all(|o| *o == [0.0, 0.0, 0.0]));
    }

    #[test]
    fn legend_colors_dedup_first_appearance() {
        let mut a = part("a", [0.0; 3], [1.0; 3]);
        a.triangle_colors = vec![Some([10, 20, 30]), None, Some([10, 20, 30])];
        let mut b = part("b", [2.0; 3], [3.0; 3]);
        b.triangle_colors = vec![Some([40, 50, 60])];
        assert_eq!(legend_colors(&[a, b]), [([10, 20, 30]), ([40, 50, 60])]);
    }
}
