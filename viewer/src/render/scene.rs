//! iced adapter for the part-keyed renderer: [`ModelScene`] is a
//! `shader::Program` whose primitive drives [`MeshPipeline`]'s viewer-owned
//! render pass from `shader::Primitive::render()`. Mirrors iced's
//! `custom_shader` example structure (scene + Primitive + Pipeline). The
//! scene sits behind an `Arc`, so the per-frame primitive carries a
//! refcount bump, not a mesh copy.

use std::sync::Arc;

use iced::widget::shader::{self, Viewport};
use iced::{Rectangle, mouse};

use super::{
    Camera, Frame, GridMesh, MarkerMesh, MeshPipeline, Options, PartPose, PartScene, camera, grid,
};

/// Identity poses for every slot of an n-part scene — the assembled
/// default `new` starts at and `replace_scene` resets to.
fn identity_poses(n: usize) -> Arc<[PartPose]> {
    Arc::from(vec![Some(glam::Mat4::IDENTITY.to_cols_array_2d()); n])
}

/// The interactive model scene: part scene, orbit camera state.
/// Framing (target + distance) comes from [`camera::frame`] at
/// construction — aspect-free, against the pinned scene aabb — and the
/// input methods ([`ModelScene::orbit`], [`ModelScene::zoom`],
/// [`ModelScene::pan`]) are the parity formulas over that state. The
/// overlay ([`grid::geometry`]) is built once alongside: it depends only
/// on the scene aabb, never on the camera.
///
/// Two owners drive the pose: the example mutates it through those input
/// methods directly, while the app's pane keeps its own pure state machine
/// (`app::camera_input::CameraInput`, which calls the same [`camera`]
/// formulas) and pushes the settled pose in through [`ModelScene::set_pose`]
/// after every event. The framing-derived fields stay private so both paths
/// read one consistent camera.
pub struct ModelScene {
    pub scene: Arc<PartScene>,
    /// Per-part poses for the next frame — `Some(matrix)` draws the part
    /// transformed, `None` hides it. Identity here means assembled; the
    /// app's arrangement wiring (explode/visibility) writes real poses.
    /// Arc-shared with the per-frame primitive like `scene`, so a pose
    /// update costs one refcount bump, never a mesh copy or re-stage.
    pub poses: Arc<[PartPose]>,
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
    /// owns the comparison state (M5's `comparison_parameters`) and swaps
    /// the scene here on a version change; `None` draws none.
    pub ghost: Option<Arc<PartScene>>,
    /// Grid visibility (upstream `showsGrid`, default on — facts #38).
    pub show_grid: bool,
    /// Wireframe mode (upstream `showsWireframe` — facts #44).
    pub wireframe: bool,
    /// Samples per pixel, 1 or 4 (the example's `m` toggle): flows into
    /// `Options::msaa`, whose flip rebuilds the pipeline in place.
    pub msaa: u32,
    pub yaw: f32,
    /// keep within ±[`camera::MAX_PITCH`]: the orbit formula uses Y as up
    pub pitch: f32,
    /// Camera distance from `target` — scroll zooms this itself, not a
    /// multiplier, so the clamp is the radius-relative parity one.
    pub distance: f32,
    /// The aspect-free framing of `scene`: look-at target and the framing
    /// radius the zoom clamp scales.
    target: [f32; 3],
    radius: f32,
    /// The manifest bed the overlay was last built with — kept across
    /// [`ModelScene::replace_scene`] so a reload's grid derives from it
    /// (upstream's pane re-sends the bed every update; a mesh swap alone
    /// never clears it).
    bed: Option<[f64; 2]>,
    /// The aabb [`ModelScene::grid`] was last built against — the change
    /// check [`ModelScene::set_overlay_frame`] compares against. Normally
    /// the staged scene's own; the pane's frozen stamp (T10) writes
    /// layout-switch-time values here instead.
    grid_aabb: ([f32; 3], [f32; 3]),
}

impl ModelScene {
    pub fn new(scene: Arc<PartScene>) -> Self {
        let (target, distance) = camera::frame(scene.aabb_min, scene.aabb_max);
        let radius = camera::framing_radius(scene.aabb_min, scene.aabb_max);
        let (yaw, pitch) = camera::Preset::Iso.yaw_pitch();
        let grid_aabb = (scene.aabb_min, scene.aabb_max);
        let grid = Arc::new(grid::geometry(grid_aabb, None));
        Self {
            poses: identity_poses(scene.parts.len()),
            scene,
            grid,
            markers: None,
            ghost: None,
            show_grid: true,
            wireframe: false,
            msaa: 1,
            yaw,
            pitch,
            distance,
            target,
            radius,
            bed: None,
            grid_aabb,
        }
    }

    /// orbit(by:) (facts #18): yaw −= dx·0.008, pitch += dy·0.008 clamped
    /// to ±[`camera::MAX_PITCH`]. `dx`/`dy` are AppKit-convention deltas
    /// (dy bottom-up) — iced's top-down cursor y must be negated by the
    /// caller.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        (self.yaw, self.pitch) = camera::orbit_drag(self.yaw, self.pitch, dx, dy);
    }

    /// zoom(by:) (facts #19): distance × factor clamped to
    /// [0.1·radius, 200·radius].
    pub fn zoom(&mut self, factor: f32) {
        self.distance = camera::zoomed(self.distance, factor, self.radius);
    }

    /// pan(by:viewHeight:) (facts #22) — the parity formula. The app's
    /// input wiring lives in its own state machine
    /// (`app::camera_input::CameraInput`), which calls [`camera::panned`]
    /// directly; this method stays the adapter-local convenience for the
    /// example.
    pub fn pan(&mut self, dx: f32, dy: f32, view_height: f32) {
        self.target = camera::panned(
            self.target,
            self.yaw,
            self.pitch,
            self.distance,
            view_height,
            dx,
            dy,
        );
    }

    /// Adopt a pose computed by an external owner (the app pane's
    /// `CameraInput`): every field the draw reads, set atomically. The
    /// framing-derived private fields are NOT touched — a re-frame arrives
    /// through [`ModelScene::replace_scene`] or construction, and the
    /// external state machine tracks its own framing radius.
    pub fn set_pose(&mut self, yaw: f32, pitch: f32, distance: f32, target: [f32; 3]) {
        self.yaw = yaw;
        self.pitch = pitch;
        self.distance = distance;
        self.target = target;
    }

    /// Store the overlay frame — the aabb the grid/bed/tripod derive from
    /// and the manifest bed — rebuilding [`ModelScene::grid`] only when
    /// either input changed from what is stored (upstream's guards:
    /// `rebuildGrid` fires when the mesh changes, `showBed` when the size
    /// differs, ModelViewer.swift:165-190). The pane stamps this exactly on
    /// scene/layout/plate-identity changes and hands its FROZEN `grid_aabb`
    /// — upstream rebuilds the grid on sceneID, never on explode/hidden
    /// sweeps, so the grid's size freezes at layout-switch-time explode and
    /// the pane's stamp is what reproduces that quirk (T10 owns the
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
    /// and re-derives the framing fields so the struct stays
    /// self-consistent; the caller decides whether the camera actually
    /// re-frames (the > 50 % diagonal rule) and pushes the settled pose
    /// through [`ModelScene::set_pose`].
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
        self.radius = camera::framing_radius(scene.aabb_min, scene.aabb_max);
        self.poses = identity_poses(scene.parts.len());
        self.scene = scene;
        self.target = target;
        self.distance = distance;
    }

    fn camera(&self, aspect: f32) -> Camera {
        Camera::orbiting(self.distance, self.yaw, self.pitch, self.target, aspect)
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
            grid: self.show_grid.then(|| self.grid.clone()),
            markers: self.markers.clone(),
            ghost: self.ghost.clone(),
            camera: self.camera(aspect),
            options: Options {
                wireframe: self.wireframe,
                msaa: self.msaa,
            },
        }
    }
}

#[derive(Debug)]
pub struct ModelPrimitive {
    scene: Arc<PartScene>,
    poses: Arc<[PartPose]>,
    grid: Option<Arc<GridMesh>>,
    markers: Option<Arc<MarkerMesh>>,
    ghost: Option<Arc<PartScene>>,
    camera: Camera,
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
        // effective_aabb defaults to the staged scene's own — the
        // arrangement wiring replaces it with the arranged/plate-aware box
        let frame = Frame {
            camera: &self.camera,
            scene: &self.scene,
            poses: &self.poses,
            effective_aabb: (self.scene.aabb_min, self.scene.aabb_max),
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
    /// at the new extents — the seam T10 stamps on scene/layout/plate
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
}
