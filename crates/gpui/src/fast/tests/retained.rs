//! Tests of views drawn again from what they drew on the last frame. See
//! [`crate::fast::retained`].

use crate::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, StyleRefinement, Styled as _, TestAppContext, Window, WindowHandle, div,
    prelude::FluentBuilder as _, px,
};
use std::{cell::Cell, rc::Rc};

struct Row {
    label: u32,
    builds: Rc<Cell<usize>>,
}

impl Render for Row {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.builds.set(self.builds.get() + 1);
        div()
            .size_full()
            .bg(crate::black())
            .hover(|style| style.bg(crate::white()))
            .child(format!("row {}", self.label))
    }
}

struct Rows {
    row: Entity<Row>,
    covered: bool,
}

impl Render for Rows {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size(px(300.))
            .child(
                self.row
                    .clone()
                    .cached(StyleRefinement::default().w(px(100.)).h(px(20.))),
            )
            .when(self.covered, |this| {
                this.child(div().absolute().top_0().left_0().size(px(200.)).occlude())
            })
    }
}

fn window(cx: &mut TestAppContext) -> (WindowHandle<Rows>, Entity<Row>, Rc<Cell<usize>>) {
    let builds = Rc::new(Cell::new(0));
    let window = cx.add_window({
        let builds = builds.clone();
        move |_, cx| Rows {
            row: cx.new(|_| Row { label: 0, builds }),
            covered: false,
        }
    });
    let row = window.update(cx, |rows, _, _| rows.row.clone()).unwrap();
    (window, row, builds)
}

fn draw(cx: &mut TestAppContext, window: WindowHandle<Rows>) -> Vec<String> {
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.describe_rendered_frame()
    })
    .unwrap()
}

fn notify_parent(cx: &mut TestAppContext, window: WindowHandle<Rows>) {
    window.update(cx, |_, _, cx| cx.notify()).unwrap();
}

fn move_mouse(cx: &mut TestAppContext, window: WindowHandle<Rows>, x: f32, y: f32) {
    cx.update_window(window.into(), |_, window, cx| {
        window.simulate_mouse_move(crate::point(px(x), px(y)), cx);
    })
    .unwrap();
}

/// A cached view painted while the pointer was over something in it that
/// has a hover style is rendered again once the pointer leaves, even
/// though the element never saw the pointer arrive.
#[test]
fn a_cached_view_is_rendered_again_when_a_hover_it_was_painted_by_changes() {
    let mut cx = TestAppContext::single();
    let (window, _, builds) = window(&mut cx);
    move_mouse(&mut cx, window, 10., 10.);
    let hovered = draw(&mut cx, window);
    notify_parent(&mut cx, window);
    draw(&mut cx, window);
    let builds_before = builds.get();

    move_mouse(&mut cx, window, 250., 250.);
    let left = draw(&mut cx, window);
    assert_eq!(builds.get(), builds_before + 1);
    assert_ne!(hovered, left);
}

/// A cached view reused for a while keeps the layout nodes it was laid
/// out with, so rendering it again finds them all.
#[test]
fn a_reused_cached_view_keeps_its_layout_nodes() {
    let mut cx = TestAppContext::single();
    let (window, row, builds) = window(&mut cx);
    draw(&mut cx, window);
    for _ in 0..3 {
        notify_parent(&mut cx, window);
        draw(&mut cx, window);
    }
    assert_eq!(
        builds.get(),
        1,
        "the view is reused while its parent renders"
    );

    cx.update_window(window.into(), |_, window, _| window.reset_layout_stats())
        .unwrap();
    row.update(&mut cx, |row, cx| {
        row.label = 7;
        cx.notify();
    });
    draw(&mut cx, window);
    assert_eq!(builds.get(), 2);
    let stats = cx
        .update_window(window.into(), |_, window, _| window.layout_stats())
        .unwrap();
    assert_eq!(
        stats.nodes_created, 0,
        "the view's nodes should have been kept while it was reused"
    );
    assert!(stats.nodes_reused > 0);
}

/// Something drawn over a hovered cached view is found out only when the
/// view paints; it is rendered on the next frame, which is asked for.
#[test]
fn a_cached_view_covered_while_hovered_is_rendered_on_the_next_frame() {
    let mut cx = TestAppContext::single();
    let (window, _, builds) = window(&mut cx);
    move_mouse(&mut cx, window, 10., 10.);
    let hovered = draw(&mut cx, window);
    notify_parent(&mut cx, window);
    draw(&mut cx, window);
    let builds_before = builds.get();

    window
        .update(&mut cx, |rows, _, cx| {
            rows.covered = true;
            cx.notify();
        })
        .unwrap();
    let mut look = None;
    for _ in 0..3 {
        if builds.get() > builds_before {
            break;
        }
        look = Some(draw(&mut cx, window));
    }
    assert_eq!(builds.get(), builds_before + 1);
    assert_ne!(Some(hovered), look);
}

struct Counted {
    label: usize,
    model: Option<Entity<Model>>,
    builds: Rc<Cell<usize>>,
}

struct Model(usize);

impl Render for Counted {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.builds.set(self.builds.get() + 1);
        let model = self.model.as_ref().map_or(0, |model| model.read(cx).0);
        div()
            .flex()
            .flex_row()
            .child(format!("{} {}", self.label, model))
    }
}

struct Siblings {
    first: Entity<Counted>,
    second: Entity<Counted>,
    spacer: f32,
}

impl Render for Siblings {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .child(div().h(px(self.spacer)))
            .child(self.first.clone())
            .child(self.second.clone())
    }
}

struct SiblingsWindow {
    window: WindowHandle<Siblings>,
    first: Entity<Counted>,
    model: Entity<Model>,
    first_builds: Rc<Cell<usize>>,
    second_builds: Rc<Cell<usize>>,
}

fn siblings(cx: &mut TestAppContext) -> SiblingsWindow {
    let first_builds = Rc::new(Cell::new(0));
    let second_builds = Rc::new(Cell::new(0));
    let model = cx.new(|_| Model(0));
    let window = cx.add_window({
        let (first_builds, second_builds, model) =
            (first_builds.clone(), second_builds.clone(), model.clone());
        move |_, cx| Siblings {
            first: cx.new(|_| Counted {
                label: 1,
                model: None,
                builds: first_builds,
            }),
            second: cx.new(|_| Counted {
                label: 2,
                model: Some(model),
                builds: second_builds,
            }),
            spacer: 10.,
        }
    });
    let first = window.update(cx, |view, _, _| view.first.clone()).unwrap();
    SiblingsWindow {
        window,
        first,
        model,
        first_builds,
        second_builds,
    }
}

fn draw_siblings(cx: &mut TestAppContext, window: WindowHandle<Siblings>) -> Vec<String> {
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.describe_rendered_frame()
    })
    .unwrap()
}

/// A view that is neither cached nor memoized is drawn again from the
/// last frame while nothing it read changed, even when the view around it
/// is rendered again, and rendered again once something it read did.
#[test]
fn a_view_is_rendered_again_only_when_something_it_read_changed() {
    let mut cx = TestAppContext::single();
    let s = siblings(&mut cx);
    draw_siblings(&mut cx, s.window);
    assert_eq!((s.first_builds.get(), s.second_builds.get()), (1, 1));

    s.window.update(&mut cx, |_, _, cx| cx.notify()).unwrap();
    draw_siblings(&mut cx, s.window);
    assert_eq!(
        (s.first_builds.get(), s.second_builds.get()),
        (1, 1),
        "notifying the parent leaves its children alone"
    );

    s.first.update(&mut cx, |first, cx| {
        first.label = 3;
        cx.notify();
    });
    draw_siblings(&mut cx, s.window);
    assert_eq!((s.first_builds.get(), s.second_builds.get()), (2, 1));

    s.model.update(&mut cx, |model, cx| {
        model.0 = 5;
        cx.notify();
    });
    draw_siblings(&mut cx, s.window);
    assert_eq!(
        (s.first_builds.get(), s.second_builds.get()),
        (2, 2),
        "a model the view read changing renders it again, unobserved"
    );

    cx.update_window(s.window.into(), |_, window, _| window.refresh())
        .unwrap();
    draw_siblings(&mut cx, s.window);
    assert_eq!((s.first_builds.get(), s.second_builds.get()), (3, 3));
}

/// A view that moved is built again where it went, at the layout nodes it
/// kept, and draws what a window drawing from scratch draws.
#[test]
fn a_moved_view_is_built_again_at_its_layout() {
    let mut cx = TestAppContext::single();
    let s = siblings(&mut cx);
    draw_siblings(&mut cx, s.window);
    s.window
        .update(&mut cx, |view, _, cx| {
            view.spacer = 30.;
            cx.notify();
        })
        .unwrap();
    cx.update_window(s.window.into(), |_, window, _| window.reset_layout_stats())
        .unwrap();
    let moved = draw_siblings(&mut cx, s.window);
    assert_eq!((s.first_builds.get(), s.second_builds.get()), (2, 2));
    let stats = cx
        .update_window(s.window.into(), |_, window, _| window.layout_stats())
        .unwrap();
    assert_eq!(stats.nodes_created, 0, "the moved views keep their nodes");

    cx.update_window(s.window.into(), |_, window, _| {
        window.forget_retained_state()
    })
    .unwrap();
    assert_eq!(moved, draw_siblings(&mut cx, s.window));
}

/// With retention turned off, every view is rendered every frame.
#[test]
fn views_are_rendered_every_frame_without_retention() {
    let mut cx = TestAppContext::single();
    let s = siblings(&mut cx);
    cx.update_window(s.window.into(), |_, window, _| {
        window.set_view_retention(false)
    })
    .unwrap();
    draw_siblings(&mut cx, s.window);
    let before = (s.first_builds.get(), s.second_builds.get());
    s.window.update(&mut cx, |_, _, cx| cx.notify()).unwrap();
    draw_siblings(&mut cx, s.window);
    assert!(s.first_builds.get() > before.0 && s.second_builds.get() > before.1);
}
