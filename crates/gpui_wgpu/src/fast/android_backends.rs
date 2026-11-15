/// Android's BufferQueue can only be connected to one producer API at a time.
/// Creating a combined Vulkan/GLES surface can claim the native window for
/// Vulkan even when Vulkan exposes no usable adapter, causing EGL_BAD_ALLOC
/// ("already connected") when GLES subsequently configures the window.
/// Probe without a native surface, then create only the selected backend's
/// instance/surface. EGL itself applies its config's NATIVE_VISUAL_ID.
pub(crate) fn native_backends() -> wgpu::Backends {
    #[cfg(target_os = "android")]
    {
        let probe = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            flags: crate::fast::instance_flags::native_instance_flags(),
            backend_options: wgpu::BackendOptions::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            display: None,
        });
        let adapters = gpui::block_on(probe.enumerate_adapters(wgpu::Backends::VULKAN));
        let backends = android_backend(!adapters.is_empty());
        log::info!(
            "Android graphics backend selected before native surface creation: {backends:?}"
        );
        backends
    }
    #[cfg(not(target_os = "android"))]
    {
        wgpu::Backends::VULKAN | wgpu::Backends::GL
    }
}

#[cfg(any(target_os = "android", test))]
fn android_backend(has_vulkan_adapter: bool) -> wgpu::Backends {
    if has_vulkan_adapter {
        wgpu::Backends::VULKAN
    } else {
        wgpu::Backends::GL
    }
}

#[cfg(test)]
#[path = "tests/android_backends.rs"]
mod tests;
