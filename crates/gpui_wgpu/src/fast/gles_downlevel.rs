//! Native GLES adapters without vertex-stage storage buffers (GLES 3.0, e.g.
//! the Android 11 emulator's SwiftShader translator) cannot satisfy
//! `Limits::downlevel_defaults()`: device creation fails on the compute and
//! storage limits and no adapter is left, even though the WebGL2 shader
//! variant, which reads instance records from a texture, would run there.
//! Choose that variant from the adapter's capabilities instead of from
//! `target_family = "wasm"`, and request the matching WebGL2 limits.

/// True when the adapter cannot read storage buffers in the vertex stage, so
/// instance records must travel through the texture transport.
pub(crate) fn needs_texture_instance_data(adapter: &wgpu::Adapter) -> bool {
    lacks_vertex_storage(adapter.get_downlevel_capabilities().flags)
}

/// Device limits for `adapter`, matching the instance transport it will use.
pub(crate) fn required_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
    base_limits(adapter.get_downlevel_capabilities().flags)
        .using_resolution(adapter.limits())
        .using_alignment(adapter.limits())
}

/// The texture transport has no subpixel pipeline. Report the same capability
/// to window glyph rasterization so it requests grayscale rather than LCD masks.
pub(crate) fn supports_dual_source_blending(
    device_supports_dual_source: bool,
    uses_texture_instance_data: bool,
) -> bool {
    device_supports_dual_source && !uses_texture_instance_data
}

fn lacks_vertex_storage(flags: wgpu::DownlevelFlags) -> bool {
    !flags.contains(wgpu::DownlevelFlags::VERTEX_STORAGE)
}

fn base_limits(flags: wgpu::DownlevelFlags) -> wgpu::Limits {
    if lacks_vertex_storage(flags) {
        wgpu::Limits::downlevel_webgl2_defaults()
    } else {
        wgpu::Limits::downlevel_defaults()
    }
}

#[cfg(test)]
#[path = "tests/gles_downlevel.rs"]
mod tests;
