//! iced adapter for the spike renderer: [`SpikeScene`] is a
//! `shader::Program` whose primitive drives [`MeshPipeline`]'s viewer-owned
//! render pass from `shader::Primitive::render()`. Mirrors iced's
//! `custom_shader` example structure (scene + Primitive + Pipeline).

use iced::widget::shader::{self, Viewport};
use iced::{mouse, Rectangle};

use super::{Camera, MeshPipeline, TriMesh, orbit_camera};

/// Interactive spike scene: fixed mesh, orbit camera state.
/// `zoom` scales the framing distance (scroll applies 0.9^steps,
/// upstream-contract §5).
pub struct SpikeScene {
    pub mesh: TriMesh,
    pub yaw: f32,
    /// keep within (-π/2, π/2): the orbit formula uses Y as up
    pub pitch: f32,
    pub zoom: f32,
}

impl SpikeScene {
    pub fn new(mesh: TriMesh) -> Self {
        Self {
            mesh,
            yaw: std::f32::consts::FRAC_PI_4,
            pitch: 0.6,
            zoom: 1.0,
        }
    }

    fn camera(&self) -> Camera {
        orbit_camera(&self.mesh, self.yaw, self.pitch, self.zoom)
    }
}

impl<Message> shader::Program<Message> for SpikeScene {
    type State = ();
    type Primitive = SpikePrimitive;

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: mouse::Cursor,
        _bounds: Rectangle,
    ) -> Self::Primitive {
        SpikePrimitive {
            mesh: self.mesh.clone(),
            camera: self.camera(),
        }
    }
}

#[derive(Debug)]
pub struct SpikePrimitive {
    mesh: TriMesh,
    camera: Camera,
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
        pipeline.prepare(
            device,
            queue,
            (size.width, size.height),
            &self.camera,
            &self.mesh,
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
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        MeshPipeline::new(device, queue, format)
    }
}
