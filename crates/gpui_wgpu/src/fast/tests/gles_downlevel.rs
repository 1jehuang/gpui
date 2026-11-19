use super::{base_limits, lacks_vertex_storage, supports_dual_source_blending};
use wgpu::{DownlevelFlags, Limits};

#[test]
fn adapters_with_vertex_storage_keep_storage_transport_and_limits() {
    let flags = DownlevelFlags::all();
    assert!(!lacks_vertex_storage(flags));
    assert_eq!(base_limits(flags), Limits::downlevel_defaults());
}

#[test]
fn gles3_without_vertex_storage_uses_texture_transport_and_webgl2_limits() {
    // GLES 3.0 (Android 11 emulator, SwiftShader): no compute, no storage.
    let flags = DownlevelFlags::all()
        - DownlevelFlags::VERTEX_STORAGE
        - DownlevelFlags::COMPUTE_SHADERS
        - DownlevelFlags::FRAGMENT_STORAGE;
    assert!(lacks_vertex_storage(flags));
    let limits = base_limits(flags);
    assert_eq!(limits, Limits::downlevel_webgl2_defaults());
    assert_eq!(limits.max_storage_buffers_per_shader_stage, 0);
    assert_eq!(limits.max_compute_workgroups_per_dimension, 0);
}

#[test]
fn texture_transport_never_advertises_subpixel_blending() {
    let flags = DownlevelFlags::all() - DownlevelFlags::VERTEX_STORAGE;
    assert!(!supports_dual_source_blending(
        true,
        lacks_vertex_storage(flags)
    ));
    assert!(!supports_dual_source_blending(false, true));
}

#[test]
fn storage_transport_preserves_device_subpixel_capability() {
    let uses_texture = lacks_vertex_storage(DownlevelFlags::all());
    assert!(supports_dual_source_blending(true, uses_texture));
    assert!(!supports_dual_source_blending(false, uses_texture));
}

#[test]
fn fragment_only_storage_is_not_enough_for_the_storage_transport() {
    // Some GLES 3.1 drivers expose storage blocks only in fragment shaders.
    let flags = DownlevelFlags::all() - DownlevelFlags::VERTEX_STORAGE;
    assert!(lacks_vertex_storage(flags));
}
