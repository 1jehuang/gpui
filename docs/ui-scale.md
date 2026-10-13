# UI scale

`Window::set_ui_scale(scale, cx)` zooms everything a window draws, like zooming
a web page, on top of the display's scale factor. `Window::ui_scale()` reads it
back. The code is in `crates/gpui/src/fast/ui_scale.rs`, its tests in
`crates/gpui/src/fast/tests/ui_scale.rs`.

## How it works

A window with UI scale `s` on a display with scale factor `d` lays out and
paints as if the display's scale factor were `d * s` and its content were `s`
times smaller:

| Value | Without UI scale | With UI scale `s` |
| --- | --- | --- |
| `Window::scale_factor()` | `d` | `d * s` |
| `Window::viewport_size()` | platform content size | platform content size `/ s` |
| A `px(10.)` length on screen | `10 * d` device pixels | `10 * d * s` device pixels |

Layout, text shaping, glyph rasterization and images need nothing new, since
they already work in logical pixels and rasterize at the scale factor. Text and
icons stay sharp at any scale.

Only the boundary with the platform translates. `Window::bounds_changed`
multiplies the scale back in after every resize and display change, so the
scale survives both. The test-only `Window::set_scale_factor` does not.

| Crossing | Direction | Values |
| --- | --- | --- |
| Input events | platform to GPUI, divided by `s` | mouse, pressure, scroll wheel position and pixel delta, pinch, touch, long press, touch drag, file drop positions |
| Window geometry | platform to GPUI, divided by `s` | viewport size, mouse position, visual viewport bounds, safe area and IME insets |
| Text input | GPUI to platform, multiplied by `s` | `bounds_for_range`, `element_bounds`, IME candidate position |
| Text input | platform to GPUI, divided by `s` | `character_index_for_point` |
| Window chrome | GPUI to platform, multiplied by `s` | input region, client inset, window menu position |

Screen-space window geometry (`Window::bounds`, `Window::window_bounds`,
`Window::resize`, display bounds) is not window-local and is unchanged.

A scale of `1.0` is the default and skips every translation. Values that are
not finite and positive are ignored.
