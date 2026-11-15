# GPUI Android

Native Android backend for GPUI. It renders the normal GPUI `Scene` with
`gpui_wgpu::WgpuRenderer` directly to an `ANativeWindow`. Text shaping and
rasterization use `gpui_wgpu::CosmicTextSystem`. There is no Java widget UI.

## Embedding

Build the embedding application's library as a `cdylib` and use a NativeActivity:

```rust,ignore
#[unsafe(no_mangle)]
fn android_main(app: gpui_android::android_activity::AndroidApp) {
    gpui_android::application(app).run(|cx| {
        // Open exactly the same GPUI root view as the desktop/iOS application.
    });
}
```

`application` stores the activity before constructing the platform. It delays the
GPUI launch callback until `InitWindow`, then owns the native event loop. The
activity may destroy and recreate its surface without recreating the GPUI view.
Android touch pointer IDs are preserved and forwarded as GPUI touch events.
Density converts physical coordinates to GPUI logical pixels.

## Input connection and system services

NativeActivity's stock input glue provides hardware keys but not a full Android
IME text connection. An embedding activity can supply an `InputConnection`
adapter without adding an alternative visual UI. Its JNI entry points should
call `dispatch_text_input` with `AndroidTextInput::{Commit, Compose,
FinishComposition, DeleteSurrounding, Enter}`. This queue is thread-safe and edits
are applied to GPUI's focused `PlatformInputHandler` on the native event thread.

Register `AndroidPlatformServices` with `set_platform_services` to integrate the
activity's custom IME host (`set_keyboard_visible`), clipboard, and external URL
handler. The default clipboard and URL handlers use Android system APIs through
JNI. `android_app()` exposes the current activity on the GPUI thread for other
platform-service adapters.

## Validation

Host integration checks use hosted `justrust check -p gpui_android -p gpui_platform`.
Hosted `justrust test -p gpui_android --lib` passes 21 tests covering the
accessibility bridge and inset projection. The broader
`justrust test -p gpui -p gpui_linux -p gpui_android --lib` run passed 470 tests
with one ignored test before the three additional physical-inset tests were
added. These host tests do not exercise an Android GPU, IME, or screen reader.
Android cross checks use `justrust check --local="Android cross compilation
requires installed local NDK" -p gpui_android -p gpui_platform --target
x86_64-linux-android`, with the target compiler/linker environment pointing at
the local NDK. Compilation alone does not establish successful emulator rendering.

A NativeActivity `jcode-mobile` APK has also been run on the API 35 x86_64
emulator using `-gpu lavapipe`: the shared GPUI sign-in view and animated orb
rendered, touch focused the email field, the Android keyboard appeared, and
injected text appeared in that GPUI field. Background/reopen preserved the view.
This does not establish full composing-IME, TalkBack, or hardware-device parity.
The emulator's SwiftShader Vulkan backend crashed on a SPIR-V source-language
operand before presenting the first frame, so that emulator configuration is
not a passing rendering target.

NativeActivity content rectangles provide a fallback for logical safe-area and
keyboard insets, but API 35 edge-to-edge mode did not report enough information
to keep the focused email field above the keyboard in the emulator run. The
embedding activity should forward its actual `WindowInsets` via
`dispatch_window_insets(safe, ime)`, with physical-pixel arrays in left, top,
right, bottom order. This thread-safe path overrides the inferred fallback and
reprojects the insets when density changes. Host tests cover physical edge
order, negative-edge clamping, density reprojection, invalid densities, and
maximum rather than additive safe-area/IME occlusion. The embedding activity's
bridge and actual keyboard-avoidance behavior require runtime validation.
