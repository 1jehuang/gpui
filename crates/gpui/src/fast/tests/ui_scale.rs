use std::{cell::RefCell, rc::Rc};

use crate::{
    Bounds, Context, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent,
    ParentElement as _, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, Styled as _,
    TestAppContext, VisualContext as _, Window, canvas, div, point, px, size,
};

struct Probe {
    target: Rc<RefCell<Option<Bounds<Pixels>>>>,
    clicks: Rc<RefCell<Vec<Point<Pixels>>>>,
    scrolls: Rc<RefCell<Vec<Point<Pixels>>>>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let target = self.target.clone();
        let clicks = self.clicks.clone();
        let scrolls = self.scrolls.clone();
        div().size_full().child(
            div()
                .id("target")
                .absolute()
                .left(px(100.))
                .top(px(50.))
                .w(px(200.))
                .h(px(40.))
                .on_mouse_down(MouseButton::Left, move |event, _, _| {
                    clicks.borrow_mut().push(event.position);
                })
                .on_scroll_wheel(move |event, _, _| {
                    if let ScrollDelta::Pixels(delta) = event.delta {
                        scrolls.borrow_mut().push(delta);
                    }
                })
                .child(
                    canvas(
                        move |bounds, _, _| *target.borrow_mut() = Some(bounds),
                        |_, _, _, _| {},
                    )
                    .size_full(),
                ),
        )
    }
}

fn probe() -> Probe {
    Probe {
        target: Rc::default(),
        clicks: Rc::default(),
        scrolls: Rc::default(),
    }
}

#[test]
fn ui_scale_multiplies_the_scale_factor_and_shrinks_the_viewport() {
    let mut cx = TestAppContext::single();
    let probe = probe();
    let target = probe.target.clone();
    let (_, cx) = cx.add_window_view(|_, _| probe);

    let (display_scale, display_viewport) =
        cx.update(|window, _| (window.scale_factor(), window.viewport_size()));
    cx.update(|window, cx| window.set_ui_scale(1.5, cx));
    cx.run_until_parked();

    cx.update(|window, _| {
        assert_eq!(window.ui_scale(), 1.5);
        assert_eq!(window.scale_factor(), display_scale * 1.5);
        assert_eq!(
            window.viewport_size(),
            display_viewport.map(|length| length / 1.5)
        );
    });
    assert_eq!(
        *target.borrow(),
        Some(Bounds::new(
            point(px(100.), px(50.)),
            size(px(200.), px(40.))
        )),
        "layout stays in logical pixels"
    );
}

#[test]
fn ui_scale_survives_a_platform_resize_and_display_change() {
    let mut cx = TestAppContext::single();
    let (_, cx) = cx.add_window_view(|_, _| probe());
    cx.update(|window, cx| window.set_ui_scale(2.0, cx));

    let window = cx.window_handle();
    let mut platform = cx.test_window(window);
    platform.simulate_resize(size(px(800.), px(600.)));
    cx.run_until_parked();
    cx.update(|window, _| {
        assert_eq!(window.viewport_size(), size(px(400.), px(300.)));
        assert_eq!(window.scale_factor(), 4.0);
    });

    platform.simulate_scale_factor_change(1.0);
    cx.run_until_parked();
    cx.update(|window, _| {
        assert_eq!(window.scale_factor(), 2.0);
        assert_eq!(window.viewport_size(), size(px(400.), px(300.)));
    });
}

#[test]
fn ui_scale_maps_pointer_positions_and_pixel_scrolls_into_logical_pixels() {
    let mut cx = TestAppContext::single();
    let probe = probe();
    let clicks = probe.clicks.clone();
    let scrolls = probe.scrolls.clone();
    let (_, cx) = cx.add_window_view(|_, _| probe);
    cx.update(|window, cx| window.set_ui_scale(2.0, cx));
    cx.run_until_parked();

    cx.simulate_event(MouseDownEvent {
        position: point(px(220.), px(120.)),
        button: MouseButton::Left,
        ..Default::default()
    });
    cx.simulate_event(MouseDownEvent {
        position: point(px(120.), px(60.)),
        button: MouseButton::Left,
        ..Default::default()
    });
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(220.), px(120.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-30.))),
        ..Default::default()
    });

    assert_eq!(
        *clicks.borrow(),
        vec![point(px(110.), px(60.))],
        "a press at platform (120, 60) is logical (60, 30), outside the target"
    );
    assert_eq!(*scrolls.borrow(), vec![point(px(0.), px(-15.))]);
}

#[test]
fn ui_scale_ignores_values_that_are_not_finite_and_positive() {
    let mut cx = TestAppContext::single();
    let (_, cx) = cx.add_window_view(|_, _| probe());
    let display_scale = cx.update(|window, _| window.scale_factor());

    for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        cx.update(|window, cx| window.set_ui_scale(scale, cx));
        cx.update(|window, _| {
            assert_eq!(window.ui_scale(), 1.0);
            assert_eq!(window.scale_factor(), display_scale);
        });
    }
}

#[test]
fn ui_scale_reset_restores_the_display_geometry() {
    let mut cx = TestAppContext::single();
    let (_, cx) = cx.add_window_view(|_, _| probe());
    let before = cx.update(|window, _| (window.scale_factor(), window.viewport_size()));

    cx.update(|window, cx| window.set_ui_scale(1.25, cx));
    cx.update(|window, cx| window.set_ui_scale(1.0, cx));

    assert_eq!(
        cx.update(|window, _| (window.scale_factor(), window.viewport_size())),
        before
    );
}
