//! Suite-global headless GPU context.
//!
//! `cargo test` runs tests in parallel by default, and GPU tests that each
//! construct their own wgpu `Instance` deadlock inside instance creation on
//! some driver stacks (NVIDIA's proprietary Vulkan ICD among them). Every
//! GPU test — the in-crate suite and the integration binaries alike —
//! obtains its device and queue from the one process-wide context here, so
//! instance creation happens exactly once per test binary however many
//! threads run. The in-crate suite and each integration binary are separate
//! processes and hold separate contexts; within one binary there is exactly
//! one.

use std::sync::{
    OnceLock,
    atomic::{AtomicUsize, Ordering},
};

/// Instance, adapter, device and queue created together, surfaceless.
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// The one context per process (see the module docs).
static CONTEXT: OnceLock<GpuContext> = OnceLock::new();

/// Constructions of the suite context this process has begun.
static CREATIONS: AtomicUsize = AtomicUsize::new(0);

async fn create() -> GpuContext {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: None,
            ..Default::default()
        })
        .await
        .expect(
            "surfaceless wgpu adapter (Metal on macOS, Vulkan on thelio; \
             if none: check WGPU_BACKEND / Vulkan ICDs in this session)",
        );
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .unwrap_or_else(|e| panic!("surfaceless wgpu device: {e}"));
    GpuContext {
        instance,
        adapter,
        device,
        queue,
    }
}

/// The shared headless context, created on first use. Creation failure
/// panics with the underlying error — loud, never a hang.
pub fn context() -> &'static GpuContext {
    CONTEXT.get_or_init(|| {
        CREATIONS.fetch_add(1, Ordering::SeqCst);
        pollster::block_on(create())
    })
}

/// The shared context's device and queue: the pair every GPU test drives
/// the pipeline with.
pub fn device_queue() -> (&'static wgpu::Device, &'static wgpu::Queue) {
    let ctx = context();
    (&ctx.device, &ctx.queue)
}

/// How many times this process has begun constructing the suite context —
/// the parallel-safety contract is exactly one (test visibility).
#[cfg(test)]
pub(crate) fn creation_count_for_tests() -> usize {
    CREATIONS.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parallel safety of the getter itself: threads released together on a
    /// barrier must all observe the ONE context — same address, and the
    /// process-wide construction count stays at exactly one. A per-call or
    /// racy check-then-create construction fails both halves.
    #[test]
    fn shared_context_created_once() {
        let n = 8;
        let barrier = std::sync::Barrier::new(n);
        let ptrs = std::thread::scope(|s| {
            let handles: Vec<_> = (0..n)
                .map(|_| {
                    s.spawn(|| {
                        barrier.wait();
                        context() as *const GpuContext as usize
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        let ctx = context() as *const GpuContext as usize;
        for p in ptrs {
            assert!(p == ctx, "every thread must see the same context");
        }
        assert_eq!(
            creation_count_for_tests(),
            1,
            "the suite context must be constructed exactly once per process"
        );
    }
}
