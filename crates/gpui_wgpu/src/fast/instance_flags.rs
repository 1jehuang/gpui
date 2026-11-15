/// Disable native debug naming on Android, not wgpu validation.
///
/// The API 36 16 KiB emulator's ranchu Vulkan driver crashes in
/// vk_common_SetDebugUtilsObjectNameEXT while wgpu creates its internal
/// indirect-validation descriptor set layout, before adapter selection finishes.
/// In wgpu 29, DEBUG enables VK_EXT_debug_utils. Removing it prevents the HAL
/// from loading/calling that extension. DISCARD_HAL_LABELS alone is insufficient:
/// internal validation resources and command buffers can still be named.
/// Apply this before instance creation, including in debug APKs. This is a driver
/// workaround, not a page-size workaround or a substitute for 16 KiB runtime tests.
pub(crate) fn native_instance_flags() -> wgpu::InstanceFlags {
    platform_flags(wgpu::InstanceFlags::default(), cfg!(target_os = "android"))
}

fn platform_flags(mut flags: wgpu::InstanceFlags, android: bool) -> wgpu::InstanceFlags {
    if android {
        flags.remove(wgpu::InstanceFlags::DEBUG);
    }
    flags
}

#[cfg(test)]
#[path = "tests/instance_flags.rs"]
mod tests;
