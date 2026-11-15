use super::android_backend;

#[test]
fn missing_vulkan_adapter_uses_only_gles() {
    assert_eq!(android_backend(false), wgpu::Backends::GL);
    assert!(!android_backend(false).contains(wgpu::Backends::VULKAN));
}

#[test]
fn usable_vulkan_keeps_preferred_backend_without_competing_gles_surface() {
    assert_eq!(android_backend(true), wgpu::Backends::VULKAN);
    assert!(!android_backend(true).contains(wgpu::Backends::GL));
}
