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
}

impl ModelScene {
    pub fn new(scene: Arc<PartScene>) -> Self {
        let (target, distance) = camera::frame(scene.aabb_min, scene.aabb_max);
        let radius = camera::framing_radius(scene.aabb_min, scene.aabb_max);
        let (yaw, pitch) = camera::Preset::Iso.yaw_pitch();
        let grid = Arc::new(grid::geometry((scene.aabb_min, scene.aabb_max), None));
        Self {
            poses: identity_poses(scene.parts.len()),
            scene,
            grid,
            markers: None,
            show_grid: true,
            wireframe: false,
            msaa: 1,
            yaw,
            pitch,
            distance,
            target,
            radius,
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

    /// Swap in a new part scene (an output reload): rebuilds the overlay
    /// against the new pinned aabb, resets the poses to the new scene's
    /// assembled identities (a new model has no carried explode offsets),
    /// and re-derives the framing fields so the struct stays
    /// self-consistent; the caller decides whether the camera actually
    /// re-frames (the > 50 % diagonal rule) and pushes the settled pose
    /// through [`ModelScene::set_pose`].
    pub fn replace_scene(&mut self, scene: Arc<PartScene>) {
        self.grid = Arc::new(grid::geometry((scene.aabb_min, scene.aabb_max), None));
        // the measurement's points belong to the old model — a reload
        // drops them with the markers they staged
        self.markers = None;
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
