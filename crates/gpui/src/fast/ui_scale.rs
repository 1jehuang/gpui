use crate::{
    App, Bounds, Edges, FileDropEvent, Pixels, PlatformInput, Point, ScrollDelta, Size, Window,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct UiScale(f32);

impl Default for UiScale {
    fn default() -> Self {
        Self(1.0)
    }
}

impl UiScale {
    fn is_identity(self) -> bool {
        self.0 == 1.0
    }

    pub(crate) fn to_zoomed_point(self, point: Point<Pixels>) -> Point<Pixels> {
        if self.is_identity() {
            return point;
        }
        point.map(|coordinate| coordinate / self.0)
    }

    pub(crate) fn to_zoomed_size(self, size: Size<Pixels>) -> Size<Pixels> {
        if self.is_identity() {
            return size;
        }
        size.map(|length| length / self.0)
    }

    pub(crate) fn to_zoomed_bounds(self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        if self.is_identity() {
            return bounds;
        }
        bounds.map(|length| length / self.0)
    }

    pub(crate) fn to_zoomed_edges(self, edges: Edges<Pixels>) -> Edges<Pixels> {
        if self.is_identity() {
            return edges;
        }
        edges.map(|length| *length / self.0)
    }

    pub(crate) fn to_platform_length(self, length: Pixels) -> Pixels {
        length * self.0
    }

    pub(crate) fn to_platform_point(self, point: Point<Pixels>) -> Point<Pixels> {
        if self.is_identity() {
            return point;
        }
        point.map(|coordinate| coordinate * self.0)
    }

    pub(crate) fn to_platform_bounds(self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        if self.is_identity() {
            return bounds;
        }
        bounds.map(|length| length * self.0)
    }

    pub(crate) fn to_platform_region(self, region: &[Bounds<Pixels>]) -> Vec<Bounds<Pixels>> {
        region
            .iter()
            .map(|bounds| self.to_platform_bounds(*bounds))
            .collect()
    }

    pub(crate) fn to_zoomed_input(self, event: PlatformInput) -> PlatformInput {
        if self.is_identity() {
            return event;
        }
        let point = |point| self.to_zoomed_point(point);
        match event {
            PlatformInput::MouseDown(mut event) => {
                event.position = point(event.position);
                PlatformInput::MouseDown(event)
            }
            PlatformInput::MouseUp(mut event) => {
                event.position = point(event.position);
                PlatformInput::MouseUp(event)
            }
            PlatformInput::MousePressure(mut event) => {
                event.position = point(event.position);
                PlatformInput::MousePressure(event)
            }
            PlatformInput::MouseMove(mut event) => {
                event.position = point(event.position);
                PlatformInput::MouseMove(event)
            }
            PlatformInput::MouseExited(mut event) => {
                event.position = point(event.position);
                PlatformInput::MouseExited(event)
            }
            PlatformInput::ScrollWheel(mut event) => {
                event.position = point(event.position);
                if let ScrollDelta::Pixels(delta) = event.delta {
                    event.delta = ScrollDelta::Pixels(point(delta));
                }
                PlatformInput::ScrollWheel(event)
            }
            PlatformInput::Pinch(mut event) => {
                event.position = point(event.position);
                PlatformInput::Pinch(event)
            }
            PlatformInput::LongPress(mut event) => {
                event.start_position = point(event.start_position);
                event.position = point(event.position);
                PlatformInput::LongPress(event)
            }
            PlatformInput::TouchDrag(mut event) => {
                event.start_position = point(event.start_position);
                event.position = point(event.position);
                PlatformInput::TouchDrag(event)
            }
            PlatformInput::Touch(mut event) => {
                event.position = point(event.position);
                event.predicted_position = event.predicted_position.map(point);
                PlatformInput::Touch(event)
            }
            PlatformInput::FileDrop(event) => PlatformInput::FileDrop(match event {
                FileDropEvent::Entered { position, paths } => FileDropEvent::Entered {
                    position: point(position),
                    paths,
                },
                FileDropEvent::Pending { position } => FileDropEvent::Pending {
                    position: point(position),
                },
                FileDropEvent::Submit { position } => FileDropEvent::Submit {
                    position: point(position),
                },
                event @ (FileDropEvent::Exited | FileDropEvent::Ended) => event,
            }),
            event @ (PlatformInput::KeyDown(_)
            | PlatformInput::KeyUp(_)
            | PlatformInput::ModifiersChanged(_)) => event,
        }
    }
}

#[expect(missing_docs)]
impl Window {
    pub fn ui_scale(&self) -> f32 {
        self.ui_scale.0
    }

    pub fn set_ui_scale(&mut self, scale: f32, cx: &mut App) {
        if !scale.is_finite() || scale <= 0.0 || scale == self.ui_scale.0 {
            return;
        }
        self.ui_scale = UiScale(scale);
        if let Some(inset) = self.client_inset {
            self.platform_window
                .set_client_inset(self.ui_scale.to_platform_length(inset));
        }
        self.bounds_changed(cx);
    }

    pub(crate) fn apply_ui_scale(&mut self) {
        self.scale_factor *= self.ui_scale.0;
        self.viewport_size = self.ui_scale.to_zoomed_size(self.viewport_size);
        self.mouse_position = self.ui_scale.to_zoomed_point(self.mouse_position);
    }
}
