use super::FingerScroll;
use gpui::TouchPhase;
use wayland_client::protocol::wl_pointer::{Axis, AxisSource};

#[test]
fn finger_starts_moves_pauses_and_ends_once() {
    let mut scroll = FingerScroll::default();
    scroll.axis(Axis::VerticalScroll);
    assert_eq!(
        scroll.finish(AxisSource::Finger, true),
        (TouchPhase::Started, false)
    );
    for _ in 0..100 {
        assert_eq!(
            scroll.finish(AxisSource::Finger, false),
            (TouchPhase::Moved, false)
        );
    }
    scroll.axis(Axis::VerticalScroll);
    assert_eq!(
        scroll.finish(AxisSource::Finger, true),
        (TouchPhase::Moved, false)
    );
    scroll.stop(Axis::VerticalScroll);
    assert_eq!(
        scroll.finish(AxisSource::Finger, false),
        (TouchPhase::Moved, true)
    );
    scroll.stop(Axis::VerticalScroll);
    assert_eq!(
        scroll.finish(AxisSource::Finger, false),
        (TouchPhase::Moved, false)
    );
    scroll.axis(Axis::VerticalScroll);
    assert_eq!(
        scroll.finish(AxisSource::Finger, true),
        (TouchPhase::Started, false)
    );
}

#[test]
fn diagonal_scroll_waits_for_both_axis_stops() {
    let mut scroll = FingerScroll::default();
    scroll.axis(Axis::VerticalScroll);
    scroll.axis(Axis::HorizontalScroll);
    assert_eq!(
        scroll.finish(AxisSource::Finger, true),
        (TouchPhase::Started, false)
    );
    scroll.stop(Axis::VerticalScroll);
    assert_eq!(
        scroll.finish(AxisSource::Finger, false),
        (TouchPhase::Moved, false)
    );
    scroll.axis(Axis::HorizontalScroll);
    assert_eq!(
        scroll.finish(AxisSource::Finger, true),
        (TouchPhase::Moved, false)
    );
    scroll.stop(Axis::HorizontalScroll);
    assert_eq!(
        scroll.finish(AxisSource::Finger, false),
        (TouchPhase::Moved, true)
    );
}

#[test]
fn final_delta_and_both_stops_share_one_frame() {
    let mut scroll = FingerScroll::default();
    scroll.axis(Axis::HorizontalScroll);
    scroll.axis(Axis::VerticalScroll);
    scroll.stop(Axis::VerticalScroll);
    scroll.stop(Axis::HorizontalScroll);
    assert_eq!(
        scroll.finish(AxisSource::Finger, true),
        (TouchPhase::Started, true)
    );
    assert_eq!(
        scroll.finish(AxisSource::Finger, false),
        (TouchPhase::Moved, false)
    );
}

#[test]
fn stop_without_gesture_does_not_end() {
    let mut scroll = FingerScroll::default();
    scroll.stop(Axis::VerticalScroll);
    assert_eq!(
        scroll.finish(AxisSource::Finger, false),
        (TouchPhase::Moved, false)
    );
}

#[test]
fn wheels_and_continuous_devices_remain_moved() {
    for source in [
        AxisSource::Wheel,
        AxisSource::WheelTilt,
        AxisSource::Continuous,
    ] {
        let mut scroll = FingerScroll::default();
        for _ in 0..3 {
            scroll.axis(Axis::VerticalScroll);
            scroll.stop(Axis::VerticalScroll);
            assert_eq!(scroll.finish(source, true), (TouchPhase::Moved, false));
        }
    }
}

#[test]
fn source_switch_clears_old_finger_axes() {
    let mut scroll = FingerScroll::default();
    scroll.axis(Axis::VerticalScroll);
    scroll.finish(AxisSource::Finger, true);
    assert_eq!(
        scroll.finish(AxisSource::Wheel, true),
        (TouchPhase::Moved, false)
    );
    scroll.axis(Axis::HorizontalScroll);
    assert_eq!(
        scroll.finish(AxisSource::Finger, true),
        (TouchPhase::Started, false)
    );
    scroll.stop(Axis::HorizontalScroll);
    assert_eq!(
        scroll.finish(AxisSource::Finger, false),
        (TouchPhase::Moved, true)
    );
}
