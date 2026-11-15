//! NativeActivity content-rectangle projection into GPUI's logical inset space.
use gpui::{DevicePixels, Edges, Pixels, Size, WindowInsets, px};

/// Receives physical-pixel system and IME insets from the activity's
/// WindowInsets listener. Array order is left, top, right, bottom.
#[cfg(target_os = "android")]
pub fn dispatch_window_insets(safe: [i32; 4], ime: [i32; 4]) {
    let mut state = NATIVE_INSETS.lock();
    state.0 = Some((safe, ime));
    if let Some(waker) = &state.1 {
        waker.wake();
    }
}

#[cfg(target_os = "android")]
static NATIVE_INSETS: parking_lot::Mutex<(
    Option<([i32; 4], [i32; 4])>,
    Option<android_activity::AndroidAppWaker>,
)> = parking_lot::Mutex::new((None, None));

#[cfg(target_os = "android")]
pub(crate) fn initialize(waker: android_activity::AndroidAppWaker) {
    NATIVE_INSETS.lock().1 = Some(waker);
}

#[cfg(target_os = "android")]
pub(crate) fn take_native() -> Option<([i32; 4], [i32; 4])> {
    NATIVE_INSETS.lock().0.take()
}

pub(crate) fn physical_insets(scale: f32, safe: [i32; 4], ime: [i32; 4]) -> Option<WindowInsets> {
    if !scale.is_finite() || scale <= 0. {
        return None;
    }
    let edges = |[left, top, right, bottom]: [i32; 4]| Edges {
        left: px(left.max(0) as f32 / scale),
        top: px(top.max(0) as f32 / scale),
        right: px(right.max(0) as f32 / scale),
        bottom: px(bottom.max(0) as f32 / scale),
    };
    Some(WindowInsets {
        safe_area: edges(safe),
        ime: edges(ime),
    })
}

pub(crate) fn content_insets(
    bounds: Size<DevicePixels>,
    scale: f32,
    rect: [i32; 4],
    baseline: &mut Option<Edges<Pixels>>,
) -> Option<WindowInsets> {
    let [left, top, right, bottom] = rect;
    if right <= left || bottom <= top || !scale.is_finite() || scale <= 0. {
        return None;
    }
    let current = Edges {
        top: px(top.max(0) as f32 / scale),
        left: px(left.max(0) as f32 / scale),
        right: px((bounds.width.0 - right).max(0) as f32 / scale),
        bottom: px((bounds.height.0 - bottom).max(0) as f32 / scale),
    };
    // NativeActivity combines system bars and the IME in its content rectangle.
    // Keep the smallest observed bottom occlusion as the system-bar baseline.
    // A larger occlusion is the IME, including while it animates away.
    if let Some(base) = baseline.as_mut() {
        base.bottom = base.bottom.min(current.bottom);
    } else {
        *baseline = Some(current.clone());
    }
    let mut safe = baseline.as_ref().cloned().unwrap_or_default();
    safe.top = current.top;
    safe.left = current.left;
    safe.right = current.right;
    let ime_bottom = if current.bottom > safe.bottom {
        current.bottom
    } else {
        px(0.)
    };
    Some(WindowInsets {
        safe_area: safe,
        ime: Edges {
            bottom: ime_bottom,
            ..Edges::default()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::{content_insets, physical_insets};
    use gpui::{DevicePixels, px, size};

    #[test]
    fn content_rect_tracks_keyboard_without_losing_system_bars() {
        let bounds = size(DevicePixels(1080), DevicePixels(1920));
        let mut baseline = None;
        let normal = content_insets(bounds, 3., [0, 72, 1080, 1848], &mut baseline).unwrap();
        assert_eq!(normal.safe_area.top, px(24.));
        assert_eq!(normal.safe_area.bottom, px(24.));
        assert_eq!(normal.ime.bottom, px(0.));
        let keyboard = content_insets(bounds, 3., [0, 72, 1080, 1020], &mut baseline).unwrap();
        assert_eq!(keyboard.safe_area.bottom, px(24.));
        assert_eq!(keyboard.ime.bottom, px(300.));
        assert_eq!(keyboard.effective().bottom, px(300.));
        let hiding = content_insets(bounds, 3., [0, 72, 1080, 1500], &mut baseline).unwrap();
        assert_eq!(hiding.ime.bottom, px(140.));
        let hidden = content_insets(bounds, 3., [0, 72, 1080, 1848], &mut baseline).unwrap();
        assert_eq!(hidden, normal);
    }

    #[test]
    fn ignores_uninitialized_rect_and_invalid_density() {
        let bounds = size(DevicePixels(1080), DevicePixels(1920));
        let mut baseline = None;
        assert!(content_insets(bounds, 3., [0; 4], &mut baseline).is_none());
        assert!(content_insets(bounds, 0., [0, 0, 1080, 1920], &mut baseline).is_none());
        assert!(content_insets(bounds, f32::NAN, [0, 0, 1080, 1920], &mut baseline).is_none());
        assert!(baseline.is_none());
    }

    #[test]
    fn physical_insets_preserve_edges_and_use_maximum_occlusion() {
        let insets = physical_insets(3., [30, 72, 60, 48], [0, 0, 0, 900]).unwrap();
        assert_eq!(insets.safe_area.left, px(10.));
        assert_eq!(insets.safe_area.top, px(24.));
        assert_eq!(insets.safe_area.right, px(20.));
        assert_eq!(insets.safe_area.bottom, px(16.));
        assert_eq!(insets.ime.bottom, px(300.));
        assert_eq!(insets.effective().bottom, px(300.));
        let hidden = physical_insets(3., [30, 72, 60, 48], [0; 4]).unwrap();
        assert_eq!(hidden.effective().bottom, px(16.));
    }

    #[test]
    fn physical_insets_reproject_on_density_changes_and_clamp_negative_edges() {
        let safe = [-3, 72, -6, 48];
        let ime = [-9, -12, -15, 900];
        let initial = physical_insets(3., safe, ime).unwrap();
        let changed = physical_insets(2., safe, ime).unwrap();
        assert_eq!(initial.safe_area.left, px(0.));
        assert_eq!(initial.safe_area.right, px(0.));
        assert_eq!(initial.ime.top, px(0.));
        assert_eq!(changed.safe_area.top, px(36.));
        assert_eq!(changed.ime.bottom, px(450.));
    }

    #[test]
    fn physical_insets_reject_invalid_density() {
        for scale in [0., -1., f32::NAN, f32::INFINITY] {
            assert!(physical_insets(scale, [0; 4], [0; 4]).is_none());
        }
    }
}
