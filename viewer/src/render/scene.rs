//! iced adapter for the part-keyed renderer: [`SpikeScene`] is a
//! `shader::Program` whose primitive drives [`MeshPipeline`]'s viewer-owned
//! render pass from `shader::Primitive::render()`. Mirrors iced's
//! `custom_shader` example structure (scene + Primitive + Pipeline). The
//! scene sits behind an `Arc`, so the per-frame primitive carries a
//! refcount bump, not a mesh copy.

use std::sync::Arc;

use iced::widget::shader::{self, Viewport};
use iced::{Rectangle, mouse};

use super::{Camera, GridMesh, MeshPipeline, Options, PartScene, camera, grid};

/// Interactive spike scene: fixed part scene, orbit camera state.
/// Framing (target + distance) comes from [`camera::frame`] at
/// construction — aspect-free, against the pinned scene aabb — and the
/// input methods ([`SpikeScene::orbit`], [`SpikeScene::zoom`],
/// [`SpikeScene::pan`]) are the parity formulas over that state. The
/// overlay ([`grid::geometry`]) is built once alongside: it depends only
/// on the scene aabb, never on the camera.
pub struct SpikeScene {
    pub scene: Arc<PartScene>,
    pub grid: Arc<GridMesh>,
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

impl SpikeScene {
    pub fn new(scene: Arc<PartScene>) -> Self {
        let (target, distance) = camera::frame(scene.aabb_min, scene.aabb_max);
        let radius = camera::framing_radius(scene.aabb_min, scene.aabb_max);
        let (yaw, pitch) = camera::Preset::Iso.yaw_pitch();
        let grid = Arc::new(grid::geometry((scene.aabb_min, scene.aabb_max), None));
        Self {
            scene,
            grid,
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

    /// pan(by:viewHeight:) (facts #22) — the parity formula. The drag and
    /// modifier-scroll WIRING is M4's; nothing calls this yet.
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

    fn camera(&self, aspect: f32) -> Camera {
        Camera::orbiting(self.distance, self.yaw, self.pitch, self.target, aspect)
    }
}

impl<Message> shader::Program<Message> for SpikeScene {
    type State = ();
    type Primitive = SpikePrimitive;

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
        SpikePrimitive {
            scene: self.scene.clone(),
            grid: self.show_grid.then(|| self.grid.clone()),
            camera: self.camera(aspect),
            options: Options {
                wireframe: self.wireframe,
                msaa: self.msaa,
            },
        }
    }
}

#[derive(Debug)]
pub struct SpikePrimitive {
    scene: Arc<PartScene>,
    grid: Option<Arc<GridMesh>>,
    camera: Camera,
    options: Options,
}

impl shader::Primitive for SpikePrimitive {
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
        pipeline.prepare_parts(
            device,
            queue,
            (size.width, size.height),
            &self.camera,
            &self.scene,
            self.grid.as_deref(),
            &self.options,
        );
    }

    fn render(
        &self,
        pipeline: &MeshPipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        // The load-bearing line of the spike: the mesh is drawn by a
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
