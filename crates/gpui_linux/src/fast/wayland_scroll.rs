//! Native finger scroll lifetimes follow compositor stops, never timers.
use gpui::{PlatformInput, ScrollDelta, ScrollWheelEvent, TouchPhase, point, px};
use wayland_client::protocol::wl_pointer::{Axis, AxisSource};

use crate::linux::wayland::{WaylandClientState, window::WaylandWindowStatePtr};

#[derive(Default)]
pub(crate) struct FingerScroll {
    active_axes: u8,
    frame_axes: u8,
    stopped_axes: u8,
    started: bool,
}

impl FingerScroll {
    fn mask(axis: Axis) -> u8 {
        match axis {
            Axis::VerticalScroll => 1,
            Axis::HorizontalScroll => 2,
            _ => 0,
        }
    }

    pub(crate) fn axis(&mut self, axis: Axis) {
        self.frame_axes |= Self::mask(axis);
    }

    pub(crate) fn stop(&mut self, axis: Axis) {
        self.stopped_axes |= Self::mask(axis);
    }

    fn finish(&mut self, source: AxisSource, has_delta: bool) -> (TouchPhase, bool) {
        if source != AxisSource::Finger {
            *self = Self::default();
            return (TouchPhase::Moved, false);
        }
        self.active_axes |= std::mem::take(&mut self.frame_axes);
        self.active_axes &= !std::mem::take(&mut self.stopped_axes);
        let phase = if has_delta && !self.started {
            self.started = true;
            TouchPhase::Started
        } else {
            TouchPhase::Moved
        };
        let ended = self.started && self.active_axes == 0;
        if ended {
            self.started = false;
        }
        (phase, ended)
    }
}

pub(crate) fn finish_frame(state: &mut WaylandClientState) -> Vec<ScrollWheelEvent> {
    state.scroll_event_received = false;
    let continuous = state.continuous_scroll_delta.take();
    let discrete = state.discrete_scroll_delta.take();
    let delta = continuous
        .map(ScrollDelta::Pixels)
        .or_else(|| discrete.map(ScrollDelta::Lines));
    let (phase, ended) = state
        .finger_scroll
        .finish(state.axis_source, delta.is_some());
    let mut events = Vec::new();
    let Some(position) = state.mouse_location else {
        return events;
    };
    if let Some(delta) = delta {
        events.push(ScrollWheelEvent {
            position,
            delta,
            modifiers: state.modifiers,
            touch_phase: phase,
        });
    }
    if ended {
        events.push(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(0.0), px(0.0))),
            modifiers: state.modifiers,
            touch_phase: TouchPhase::Ended,
        });
    }
    events
}

pub(crate) fn dispatch_frame(window: Option<WaylandWindowStatePtr>, events: Vec<ScrollWheelEvent>) {
    if let Some(window) = window {
        for event in events {
            window.handle_input(PlatformInput::ScrollWheel(event));
        }
    }
}

#[cfg(test)]
#[path = "tests/wayland_scroll.rs"]
mod tests;
