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
/// Framing (target + base distance) comes from [`camera::frame`] at
/// construction — aspect-free, against the pinned scene aabb — and `zoom`
/// scales that distance (scroll applies 0.9^steps, upstream-contract §5).
/// The overlay ([`grid::geometry`]) is built once alongside: it depends
/// only on the scene aabb, never on the camera.
pub struct SpikeScene {
    pub scene: Arc<PartScene>,
    pub grid: Arc<GridMesh>,
    /// Grid visibility (upstream `showsGrid`, default on — facts #38).
    pub show_grid: bool,
    /// Wireframe mode (upstream `showsWireframe` — facts #44).
    pub wireframe: bool,
    pub yaw: f32,
    /// keep within ±[`camera::MAX_PITCH`]: the orbit formula uses Y as up
    pub pitch: f32,
    pub zoom: f32,
    /// The aspect-free framing of `scene`: look-at target and the distance
    /// `zoom` scales.
    target: [f32; 3],
    distance: f32,
}

impl SpikeScene {
    pub fn new(scene: Arc<PartScene>) -> Self {
        let (target, distance) = camera::frame(scene.aabb_min, scene.aabb_max);
        let (yaw, pitch) = camera::Preset::Iso.yaw_pitch();
        let grid = Arc::new(grid::geometry((scene.aabb_min, scene.aabb_max), None));
        Self {
            scene,
            grid,
            show_grid: true,
            wireframe: false,
            yaw,
            pitch,
            zoom: 1.0,
            target,
            distance,
        }
    }

    fn camera(&self, aspect: f32) -> Camera {
        Camera::orbiting(
            self.distance * self.zoom,
            self.yaw,
            self.pitch,
            self.target,
            aspect,
        )
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
