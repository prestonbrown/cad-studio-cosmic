//! iced adapter for the part-keyed renderer: [`SpikeScene`] is a
//! `shader::Program` whose primitive drives [`MeshPipeline`]'s viewer-owned
//! render pass from `shader::Primitive::render()`. Mirrors iced's
//! `custom_shader` example structure (scene + Primitive + Pipeline). The
//! scene sits behind an `Arc`, so the per-frame primitive carries a
//! refcount bump, not a mesh copy.

use std::sync::Arc;

use iced::widget::shader::{self, Viewport};
use iced::{mouse, Rectangle};

use super::{Camera, MeshPipeline, PartScene, orbit_camera};

/// Interactive spike scene: fixed part scene, orbit camera state.
/// `zoom` scales the framing distance (scroll applies 0.9^steps,
/// upstream-contract §5).
pub struct SpikeScene {
    pub scene: Arc<PartScene>,
    pub yaw: f32,
    /// keep within (-π/2, π/2): the orbit formula uses Y as up
    pub pitch: f32,
    pub zoom: f32,
}

impl SpikeScene {
    pub fn new(scene: Arc<PartScene>) -> Self {
        Self {
            scene,
            yaw: std::f32::consts::FRAC_PI_4,
            pitch: 0.6,
            zoom: 1.0,
        }
    }

    fn camera(&self) -> Camera {
        orbit_camera(&self.scene, self.yaw, self.pitch, self.zoom)
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
            scene: self.scene.clone(),
            camera: self.camera(),
        }
    }
}

#[derive(Debug)]
pub struct SpikePrimitive {
    scene: Arc<PartScene>,
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
        pipeline.prepare_parts(
            device,
            queue,
            (size.width, size.height),
            &self.camera,
            &self.scene,
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
