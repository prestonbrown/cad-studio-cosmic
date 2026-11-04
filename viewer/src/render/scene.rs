//! iced adapter for the part-keyed renderer: [`ModelScene`] is a
//! `shader::Program` whose primitive drives [`MeshPipeline`]'s viewer-owned
//! render pass from `shader::Primitive::render()`. Mirrors iced's
//! `custom_shader` example structure (scene + Primitive + Pipeline). The
//! scene sits behind an `Arc`, so the per-frame primitive carries a
//! refcount bump, not a mesh copy.

use std::sync::{Arc, Mutex};

use iced::widget::shader::{self, Viewport};
use iced::{Rectangle, mouse};

use super::{
    Camera, Frame, GridMesh, MarkerMesh, MeshPipeline, Options, PartAppearance, PartPose,
    PartScene, SectionPlane, camera, grid,
};

/// Identity poses for every slot of an n-part scene — the assembled
/// default `new` starts at and `replace_scene` resets to.
fn identity_poses(n: usize) -> Arc<[PartPose]> {
    Arc::from(vec![Some(glam::Mat4::IDENTITY.to_cols_array_2d()); n])
}

/// Opaque default appearances for every slot of an n-part scene — the
/// state `new` starts at and `replace_scene` resets to; the animation
/// pane's playback writes real flags per frame.
fn opaque_appearances(n: usize) -> Arc<[PartAppearance]> {
    Arc::from(vec![PartAppearance::default(); n])
}

/// The interactive model scene: part scene plus the [`camera::Orbit`] it
/// is drawn from. The pose starts at the iso framing of the pinned scene
/// aabb; the gesture owner ([`crate::camera_input::CameraInput`]) writes
/// [`ModelScene::orbit`] after every event, so this struct never decodes
/// input itself. The overlay ([`grid::geometry`]) is built alongside: it
/// depends only on the scene aabb, never on the camera.
pub struct ModelScene {
    pub scene: Arc<PartScene>,
    /// Per-part poses for the next frame — `Some(matrix)` draws the part
    /// transformed, `None` hides it. Identity here means assembled; the
    /// app's arrangement wiring (explode/visibility) writes real poses.
    /// Arc-shared with the per-frame primitive like `scene`, so a pose
    /// update costs one refcount bump, never a mesh copy or re-stage.
    pub poses: Arc<[PartPose]>,
    /// Per-part motion appearance flags for the next frame — opaque
    /// defaults until the animation pane's playback writes per-frame
    /// translucent static parts and flat-red colliding ones. Arc-shared
    /// like `poses`, so an appearance update is a refcount bump too.
    pub appearances: Arc<[PartAppearance]>,
    pub grid: Arc<GridMesh>,
    /// The staged measure markers, when a measurement is active — the
    /// measure flow stages [`crate::render::markers::geometry`] of the
    /// picked points here and clears it with the measurement; `None`
    /// draws none.
    pub markers: Option<Arc<MarkerMesh>>,
    /// The staged compare ghost, when a comparison is active — a
    /// `PartScene` built from the comparison version WITH THIS SCENE'S
    /// PIN (`PartScene::from_parts_with_pin(.., self.scene.pin)`,
    /// ModelViewer.swift:135: ghostEntity under `content`). The app layer
    /// owns the comparison state (`comparison_parameters`) and swaps
    /// the scene here on a version change; `None` draws none.
    pub ghost: Option<Arc<PartScene>>,
    /// Grid visibility (upstream `showsGrid`, default on — facts #38).
    pub show_grid: bool,
    /// Wireframe mode (upstream `showsWireframe` — facts #44).
    pub wireframe: bool,
    /// Samples per pixel, 1 or 4 (the example's `m` toggle): flows into
    /// `Options::msaa`, whose flip rebuilds the pipeline in place.
    pub msaa: u32,
    /// The camera pose the next frame draws from.
    pub orbit: camera::Orbit,
    /// The manifest bed the overlay was last built with — kept across
    /// [`ModelScene::replace_scene`] so a reload's grid derives from it
    /// (upstream's pane re-sends the bed every update; a mesh swap alone
    /// never clears it).
    pub bed: Option<[f64; 2]>,
    /// The section plane the frame carries: `Some` turns the mesh shader's
    /// discard and the cap quad on; `None` draws whole. The pane builds it
    /// with [`crate::tools::section::pinned_plane`]; the adapter only
    /// carries it.
    section: Option<SectionPlane>,
    /// The aabb [`ModelScene::grid`] was last built against — the change
    /// check [`ModelScene::set_overlay_frame`] compares against. Normally
    /// the staged scene's own; the pane's frozen stamp writes
    /// layout-switch-time values here instead.
    grid_aabb: ([f32; 3], [f32; 3]),
    /// The overlay rebuilt for the last non-default palette a [`Themed`]
    /// draw asked for, keyed by the [`ModelScene::grid`] it was derived
    /// from — a theme or overlay change rebuilds once, every other frame
    /// is a refcount bump.
    themed_grid: Mutex<Option<(grid::Palette, Arc<GridMesh>, Arc<GridMesh>)>>,
}

/// A [`ModelScene`] drawn under an app theme: the overlay takes
/// `palette`'s line and bed colors. The app resolves the palette at view
/// time, so a theme switch repaints on the next frame without a message.
pub struct Themed<'a> {
    scene: &'a ModelScene,
    palette: grid::Palette,
}

impl ModelScene {
    pub fn new(scene: Arc<PartScene>) -> Self {
        let orbit = camera::Orbit::framed(scene.aabb(), camera::Preset::Iso);
        let grid_aabb = (scene.aabb_min, scene.aabb_max);
        let grid = Arc::new(grid::geometry(grid_aabb, None));
        Self {
            poses: identity_poses(scene.parts.len()),
            appearances: opaque_appearances(scene.parts.len()),
            scene,
            grid,
            markers: None,
            ghost: None,
            show_grid: true,
            wireframe: false,
            msaa: 1,
            orbit,
            bed: None,
            section: None,
            grid_aabb,
            themed_grid: Mutex::new(None),
        }
    }

    /// This scene drawn under `palette` (see [`Themed`]).
    pub fn themed(&self, palette: grid::Palette) -> Themed<'_> {
        Themed {
            scene: self,
            palette,
        }
    }

    /// The overlay in `palette`'s colors: [`ModelScene::grid`] itself for
    /// the default palette, else a rebuild over the same inputs, cached
    /// against the grid it mirrors.
    pub fn grid_for(&self, palette: grid::Palette) -> Arc<GridMesh> {
        if palette == grid::Palette::DARK {
            return self.grid.clone();
        }
        let mut cache = self.themed_grid.lock().unwrap();
        if let Some((p, base, themed)) = cache.as_ref() {
            if *p == palette && Arc::ptr_eq(base, &self.grid) {
                return themed.clone();
            }
        }
        let themed = Arc::new(grid::geometry_with(self.grid_aabb, self.bed, palette));
        *cache = Some((palette, self.grid.clone(), themed.clone()));
        themed
    }

    /// The frame's primitive with `grid` as the overlay (when shown).
    fn primitive(&self, bounds: Rectangle, grid: impl FnOnce() -> Arc<GridMesh>) -> ModelPrimitive {
        // the projection aspect comes from the viewport being drawn — the
        // widget's own bounds — never a fixed ratio
        let aspect = if bounds.width > 0.0 && bounds.height > 0.0 {
            bounds.width / bounds.height
        } else {
            16.0 / 9.0 // zero-size layout pass: nothing draws; any finite ratio
        };
        ModelPrimitive {
            scene: self.scene.clone(),
            poses: self.poses.clone(),
            appearances: self.appearances.clone(),
            grid: self.show_grid.then(grid),
            markers: self.markers.clone(),
            ghost: self.ghost.clone(),
            camera: self.orbit.camera(aspect),
            section: self.section,
            options: Options {
                wireframe: self.wireframe,
                msaa: self.msaa,
            },
        }
    }

    /// Store the overlay frame — the aabb the grid/bed/tripod derive from
    /// and the manifest bed — rebuilding [`ModelScene::grid`] only when
    /// either input changed from what is stored (upstream's guards:
    /// `rebuildGrid` fires when the mesh changes, `showBed` when the size
    /// differs, ModelViewer.swift:165-190). The pane stamps this exactly on
    /// scene/layout/plate-identity changes and hands its FROZEN `grid_aabb`
    /// — upstream rebuilds the grid on sceneID, never on explode/hidden
    /// sweeps, so the grid's size freezes at layout-switch-time explode and
    /// the pane's stamp is what reproduces that quirk (the pane owns the
    /// freezing; this method only obeys the inputs it is given).
    pub fn set_overlay_frame(&mut self, aabb: ([f32; 3], [f32; 3]), bed: Option<[f64; 2]>) {
        if aabb == self.grid_aabb && bed == self.bed {
            return;
        }
        self.grid_aabb = aabb;
        self.bed = bed;
        self.grid = Arc::new(grid::geometry(aabb, bed));
    }

    /// Swap in a new part scene (an output reload): rebuilds the overlay
    /// against the new pinned aabb (deriving from the STORED bed — the pane
    /// re-sends it on its next stamp), resets the poses to the new scene's
    /// assembled identities (a new model has no carried explode offsets),
    /// and re-frames the orbit's target and distance on the new scene; a
    /// gesture owner that keeps its own pose (the > 50 % diagonal rule)
    /// writes [`ModelScene::orbit`] back afterwards.
    pub fn replace_scene(&mut self, scene: Arc<PartScene>) {
        self.grid_aabb = (scene.aabb_min, scene.aabb_max);
        self.grid = Arc::new(grid::geometry(self.grid_aabb, self.bed));
        // the measurement's points belong to the old model — a reload
        // drops them with the markers they staged
        self.markers = None;
        // and the ghost is staged in the OLD scene's pin (the whole point
        // of from_parts_with_pin), so it cannot survive the pin change —
        // the app re-stages it if the comparison is still active
        self.ghost = None;
        let (target, distance) = camera::frame(scene.aabb_min, scene.aabb_max);
        self.orbit.target = target;
        self.orbit.distance = distance;
        self.poses = identity_poses(scene.parts.len());
        self.appearances = opaque_appearances(scene.parts.len());
        self.scene = scene;
    }

    /// Store the section plane the frame carries, in THIS scene's pinned
    /// coordinates (build it with [`crate::tools::section::pinned_plane`]);
    /// `None` draws whole.
    pub fn set_section(&mut self, section: Option<SectionPlane>) {
        self.section = section;
    }

    /// The last-set section plane. The app's measure pick honors the same
    /// plane so a click never answers through cut-away geometry.
    pub fn section(&self) -> Option<SectionPlane> {
        self.section
    }

    /// The grid's content identity: its content hash and build half-extent
    /// (test observation — an explode/hidden sweep must leave both alone;
    /// kept `pub` because those tests sit in the app crate).
    pub fn grid_identity_for_tests(&self) -> (u64, f32) {
        (self.grid.content_hash, self.grid.half_extent)
    }
}

impl<Message> shader::Program<Message> for ModelScene {
    type State = ();
    type Primitive = ModelPrimitive;

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: mouse::Cursor,
        bounds: Rectangle,
    ) -> Self::Primitive {
        self.primitive(bounds, || self.grid.clone())
    }
}

impl<Message> shader::Program<Message> for Themed<'_> {
    type State = ();
    type Primitive = ModelPrimitive;

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: mouse::Cursor,
        bounds: Rectangle,
    ) -> Self::Primitive {
        self.scene
            .primitive(bounds, || self.scene.grid_for(self.palette))
    }
}

#[derive(Debug)]
pub struct ModelPrimitive {
    scene: Arc<PartScene>,
    poses: Arc<[PartPose]>,
    appearances: Arc<[PartAppearance]>,
    grid: Option<Arc<GridMesh>>,
    markers: Option<Arc<MarkerMesh>>,
    ghost: Option<Arc<PartScene>>,
    camera: Camera,
    section: Option<SectionPlane>,
    options: Options,
}

impl shader::Primitive for ModelPrimitive {
    type Pipeline = MeshPipeline;

    fn prepare(
        &self,
        pipeline: &mut MeshPipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        let size = viewport.physical_size();
        let frame = Frame {
            camera: &self.camera,
            scene: &self.scene,
            poses: &self.poses,
            appearances: &self.appearances,
            section: self.section,
            grid: self.grid.as_deref(),
            markers: self.markers.as_deref(),
            ghost: self.ghost.as_deref(),
            options: self.options,
        };
        pipeline.prepare_parts(device, queue, (size.width, size.height), &frame);
    }

    fn render(
        &self,
        pipeline: &MeshPipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        // The load-bearing line of this adapter: the mesh is drawn by a
        // viewer-owned pass on top of iced's output, with the viewer's own
        // Depth24PlusStencil8 attachment — not by iced's compositor pass.
        pipeline.render(
            encoder,
            target,
            [
                clip_bounds.x as f32,
                clip_bounds.y as f32,
                clip_bounds.width as f32,
                clip_bounds.height as f32,
            ],
        );
    }
}

impl shader::Pipeline for MeshPipeline {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        MeshPipeline::new(device, queue, format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::NEUTRAL;

    /// The mod.rs staging test's triangle, staged: pinned aabb exactly
    /// x/z ±3, y [0, 6] — small, known, GPU-free.
    fn tri_scene(scale: f32) -> Arc<PartScene> {
        let part = engine::parsing::Part {
            name: "tri".into(),
            positions: [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]]
                .map(|p| p.map(|c| c * scale))
                .to_vec(),
            triangle_colors: vec![None],
        };
        Arc::new(PartScene::from_parts(std::slice::from_ref(&part), NEUTRAL))
    }

    /// The bed outline's vertex color: the accent at full alpha.
    fn accent() -> [f32; 4] {
        [grid::ACCENT[0], grid::ACCENT[1], grid::ACCENT[2], 1.0]
    }

    /// `set_overlay_frame` is change-gated on BOTH inputs (upstream's
    /// rebuildGrid/showBed guards, ModelViewer.swift:165-190): the
    /// constructor's own (aabb, None) rebuilds nothing, a bed change
    /// rebuilds with the outline in the mesh, and an aabb change rebuilds
    /// at the new extents — the seam the pane stamps on scene/layout/plate
    /// identity changes, reproducing upstream's grid freeze through
    /// explode/hidden sweeps (those never reach this method).
    #[test]
    fn set_overlay_frame_rebuilds_only_on_change() {
        let scene = tri_scene(1.0);
        let mut model = ModelScene::new(scene.clone());
        let built = model.grid.clone();
        model.set_overlay_frame((scene.aabb_min, scene.aabb_max), None);
        assert!(
            Arc::ptr_eq(&built, &model.grid),
            "the constructor's own inputs must not rebuild"
        );
        // bed change: the accent outline lands in the rebuilt overlay
        model.set_overlay_frame((scene.aabb_min, scene.aabb_max), Some([80.0, 80.0]));
        assert!(!Arc::ptr_eq(&built, &model.grid));
        assert!(
            model.grid.vertices.iter().any(|v| v.color == accent()),
            "the stored bed draws the accent outline"
        );
        // aabb change with the bed held: rebuilt at the new extents —
        // footprint 400 -> half (floor(30)+1)*10 = 310
        model.set_overlay_frame(
            ([-200.0, 0.0, -200.0], [200.0, 50.0, 200.0]),
            Some([80.0, 80.0]),
        );
        assert_eq!(model.grid.half_extent, 310.0);
        assert!(
            model.grid.vertices.iter().any(|v| v.color == accent()),
            "the bed survives the aabb change"
        );
        // resending the stored pair is again a no-op
        let rebuilt = model.grid.clone();
        model.set_overlay_frame(
            ([-200.0, 0.0, -200.0], [200.0, 50.0, 200.0]),
            Some([80.0, 80.0]),
        );
        assert!(
            Arc::ptr_eq(&rebuilt, &model.grid),
            "matching stored inputs must not rebuild"
        );
    }

    /// `replace_scene` keeps the stored bed — the new overlay derives from
    /// it (upstream's pane re-sends the bed every update, so a mesh swap
    /// alone never clears it) — and records the new scene's aabb as the
    /// grid frame: resending that pair with the kept bed rebuilds nothing.
    #[test]
    fn replace_scene_keeps_bed_and_reframes() {
        let scene = tri_scene(1.0);
        let mut model = ModelScene::new(scene);
        model.set_overlay_frame(([-3.0, 0.0, -3.0], [3.0, 6.0, 3.0]), Some([80.0, 80.0]));
        // scale 30: pinned aabb grows to spans 180 -> grid half 140 (the
        // scale-1 scene's 6mm footprint reads the max(100, ·) floor)
        let scene2 = tri_scene(30.0);
        assert_eq!(model.grid.half_extent, 100.0);
        model.replace_scene(scene2.clone());
        assert_eq!(
            model.grid.half_extent, 140.0,
            "the overlay rebuilt against the new scene's aabb"
        );
        assert!(
            model.grid.vertices.iter().any(|v| v.color == accent()),
            "the new grid derives from the STORED bed"
        );
        let rebuilt = model.grid.clone();
        model.set_overlay_frame((scene2.aabb_min, scene2.aabb_max), Some([80.0, 80.0]));
        assert!(
            Arc::ptr_eq(&rebuilt, &model.grid),
            "replace_scene recorded the new aabb and kept the bed"
        );
    }

    /// `set_section` is the pane's seam onto the frame: the plane lands on
    /// the drawn primitive verbatim (cap box included), and `None` restores
    /// sectioning-off.
    #[test]
    fn set_section_drives_the_primitive() {
        let scene = tri_scene(1.0);
        let mut model = ModelScene::new(scene);
        let bounds = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        };
        let drawn = |m: &ModelScene| {
            shader::Program::<()>::draw(m, &(), mouse::Cursor::Unavailable, bounds).section
        };
        assert_eq!(drawn(&model), None, "off by default");
        let plane = SectionPlane {
            axis: crate::render::SceneAxis::Y,
            pos: 7.5,
            cap: crate::render::Box3::new([-9.0, 0.0, -9.0], [9.0, 40.0, 9.0]),
        };
        model.set_section(Some(plane));
        assert_eq!(model.section(), Some(plane));
        assert_eq!(drawn(&model), Some(plane));
        model.set_section(None);
        assert_eq!(drawn(&model), None, "None disables");
    }
}
