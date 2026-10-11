# ADR: 3MF parsing — threemf crate vs custom zip+quick-xml

Date: 2026-10-11 · Status: decided

Criteria (spec §4.2): build-item transforms, component recursion, color groups
(basematerials/colorgroup), per-triangle pid/p1, unit attribute.

## threemf 0.8 findings

Source read at `~/.cargo/registry/src/*/threemf-0.8.0/src/` (identical content
verified on thelio and the Mac). Evidence per criterion; "data" means the value
survives deserialization, "applied" means the crate does the work for us:

- **(a) build `<item>` transforms — data yes, applied no.**
  `Item { objectid: usize, transform: Option<[f64; 12]>, .. }` (model/mod.rs).
  Our probe (`threemf_transform_probe`) round-tripped
  `transform="1 0 0 0 1 0 0 0 1 5 0 0"` to `Some([1,0,0,0,1,0,0,0,1,5,0,0])`
  (3MF spec token order).
  No code anywhere applies the matrix — `read()` returns the raw `Model`.

- **(b) `<component>` recursion — data yes, recursion no.**
  `Object.components: Option<Components>` with
  `Component { objectid, transform: Option<[f64; 12]> }` parses (probe: the
  component transform came back intact). The crate contains zero
  resolution/flattening: build items are never walked into parts, components are
  never expanded. Our spike would still write that logic ourselves.

- **(c) color groups — FAILS on real output.** Two problems:
  1. `Resources` keys materials as the *materials-namespace* element:
     `#[serde(rename = "m:basematerials")] pub basematerials: Option<Vec<BaseMaterials>>`
     with bases renamed `"m:base"`. build123d (and our golden) write
     `<basematerials>` in the **core** namespace, so serde matches nothing:
     `threemf::read` on `fixtures/golden/model.3mf` returns
     `resources.basematerials == None` while the file contains two color groups
     (asserted in `threemf_reads_golden_model`). Colors are silently dropped.
  2. `colorgroup` does not exist in the crate's model at all (`grep colorgroup`
     → no hits).

- **(d) per-triangle pid/p1 — ABSENT.** `Triangle` in model/mesh.rs has exactly
  three fields, `v1/v2/v3: usize`. Triangle-level `pid`/`p1` attributes (and the
  object-level `pid`/`pindex` → color fallback the upstream contract §3
  requires) have nowhere to land; our probe's `pid="1" p1="0"` attributes were
  dropped without error.

- **(e) `unit` attribute — yes.** `Model { unit: Unit }` with
  `Micron|Millimeter|Centimeter|Inch|Foot|Meter` (default mm). Exposes the enum
  only; the mm-per-unit factor table (0.001/10/25.4/304.8/1000) would be ours.

Also relevant: the crate self-describes as write-first ("So far, functionality
is limited to writing 3MF files" — lib.rs docs); reading is a serde pass. It
additionally loses spec-nonconforming but real-world structures silently (the
handmade fixture's bare `<component>` child comes back as an object with neither
mesh nor components — asserted in `threemf_transform_probe`), and it pulls its
own zip 8 + quick-xml 0.39 copies into the tree. `read()` itself works on our
golden file (one `Model`, 4 objects, names `base`/`lid`, 12 triangles each,
build items `[(1, no transform), (4, no transform)]`, unit millimeter) — the
container/XML layer is fine; it is the semantics above that are missing.

Fixture/convention notes for M1: the handmade fixture omits the spec-required
`<components>` wrapper element around `<component>` (the golden 3MF has it), so
M1's parser must accept both shapes. Transforms everywhere use the 3MF spec
token order — 12 tokens m00..m32, 4 rows × 3, row-vector application,
translation at tokens 9/10/11 — which the handmade fixture and the spike's
`apply()` now both encode; M1's parser must keep that order (build123d-produced
goldens with transforms carry the same spec order).

## Custom parser estimate

Measured from the spike (`engine/tests/parse3mf.rs`, quick-xml 0.41 + zip 8.6,
transcribed code compiling as-is):

- `parse_objects` — 98 LoC (objects, names, mesh vertices placeholder level)
- `resolve_parts` — 102 LoC (build item → object → first component, transform
  application in 3MF spec token order — 4 rows × 3, row-vector p' = p·M,
  translation at tokens 9/10/11; the handmade fixture and the spike's `apply()`
  both encode it (+10 mm x-translation); component-then-build order)
- Shared scaffolding (`RawPart`, zip `3D/3dmodel.model` extraction, fixture
  helper) — ~25 LoC

≈ 225 LoC measured for partial semantics. Delta to full upstream-contract §3
(the honest M1 estimate, ~150–250 LoC on top):

1. Triangle expansion via `v1/v2/v3` indexing (spike pushes a placeholder).
2. `basematerials` + `colorgroup` parse, `#RRGGBB[AA]` decode, id-keyed lookup.
3. Per-triangle `pid`/`p1` with object-level `pid`/`pindex` fallback;
   uncolored → neutral material 0.
4. Full component recursion (depth cap 16), naming fallback chain (object name
   → first descendant component name → `part N+1`), no-build-items fallback
   (one part per object id, sorted).
5. `unit` → mm-per-unit scaling.
6. General matrix composition (spike applies component-then-build sequentially,
   already p·C·B in spec row-vector order; M1 composes N levels).
7. Errors instead of `unwrap()` panics on malformed input.

## Decision: custom

threemf covers 2 of 5 criteria (unit fully; transforms/recursion as inert
data), and hard-fails the two that carry product visible behavior — colors are
silently dropped from exactly the files build123d produces, and per-triangle
pid/p1 has no representation. Even where it parses, the resolution, recursion,
transform application and color assignment — the actual work — would be ours,
so the crate would only ever save the struct definitions while adding a serde
model that mismatches our real inputs. The custom path is ~225 LoC measured
plus a bounded, enumerated delta (~400 LoC total, no new heavy dependencies —
zip and quick-xml are already in the tree).
