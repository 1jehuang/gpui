//! Tests of retained layout: layout nodes carried from one frame to the next,
//! the keys that match them to elements, and the statistics that show it.

use smallvec::SmallVec;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use crate::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, DivInspectorState, ElementId, Entity,
    Hsla, InspectorElementId, InteractiveElement as _, IntoElement, LayoutStats, Length, Modifiers,
    MouseButton, MouseDownEvent, MouseUpEvent, ParentElement, Pixels, PlatformInput, Render,
    RenderOnce, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled,
    TestAppContext, UniformListScrollHandle, Window, WindowHandle, WindowOptions, canvas, div,
    hsla, point, px, size, uniform_list,
};
/// Drives the retained-layout tests.
///
/// The shape, the styling and the text of the tree are each controllable on
/// their own, and every row records the bounds its trailing probe resolved
/// to, so a frame assembled out of retained nodes can be compared against
/// the frame a fresh tree produces for the same inputs.
struct RetainedLayoutView {
    rows: usize,
    row_width: Pixels,
    label: SharedString,
    probes: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    /// Identities of the rows, in order. Rows are keyed by these when
    /// `keyed` is set, and by their position otherwise.
    row_ids: Vec<u64>,
    keyed: bool,
    text_color: Hsla,
}

impl Render for RetainedLayoutView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let probes = self.probes.clone();
        probes.borrow_mut().clear();
        let label = self.label.clone();
        let row_width = self.row_width;
        let keyed = self.keyed;
        let row_ids = self.row_ids.clone();
        let text_color = self.text_color;
        div()
            .flex()
            .flex_col()
            .children((0..self.rows).map(move |ix| {
                let probes = probes.clone();
                // Rows have to be distinguishable for the test to mean
                // anything: interchangeable rows can be matched to the
                // wrong node and nobody is any the wiser.
                let row_id = row_ids.get(ix).copied().unwrap_or(ix as u64);
                let row = div()
                    .flex()
                    .flex_row()
                    .w(row_width + px((row_id % 5) as f32 * 10.))
                    .h(px(20.))
                    .text_color(text_color)
                    .child(label.clone())
                    .child(
                        canvas(
                            move |bounds, _, _| probes.borrow_mut().push(bounds),
                            |_, _, _, _| {},
                        )
                        .flex_1()
                        .h_full(),
                    );
                if keyed {
                    row.id(("row", row_ids[ix])).into_any_element()
                } else {
                    row.into_any_element()
                }
            }))
    }
}

/// Draws one frame and returns the layout work it took.
fn draw_frame(cx: &mut TestAppContext, window: AnyWindowHandle) -> LayoutStats {
    cx.update_window(window, |_, window, cx| {
        window.reset_layout_stats();
        window.draw(cx).clear(cx);
        window.layout_stats()
    })
    .unwrap()
}

/// Applies a change to the view and returns the layout work that followed.
///
/// Notifying a view can draw a frame of its own before the explicit one
/// here, so the counters start before the change rather than before the
/// draw; otherwise the work the change caused would be measured a frame too
/// late, once the tree had already settled.
fn change_and_draw<V: Render>(
    cx: &mut TestAppContext,
    window: WindowHandle<V>,
    change: impl FnOnce(&mut V),
) -> LayoutStats {
    cx.update_window(window.into(), |_, window, _| window.reset_layout_stats())
        .unwrap();
    window
        .update(cx, |view, _, cx| {
            change(view);
            cx.notify();
        })
        .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.layout_stats()
    })
    .unwrap()
}

fn retained_layout_window(
    cx: &mut TestAppContext,
    probes: Rc<RefCell<Vec<Bounds<Pixels>>>>,
) -> WindowHandle<RetainedLayoutView> {
    cx.add_window(move |_, _| RetainedLayoutView {
        rows: 4,
        row_width: px(200.),
        label: "ab".into(),
        probes,
        row_ids: (0..4).collect(),
        keyed: false,
        text_color: hsla(0.0, 0.0, 0.1, 1.0),
    })
}

#[test]
fn an_unchanged_frame_reuses_every_layout_node_and_writes_to_none() {
    let mut cx = TestAppContext::single();
    let probes = Rc::new(RefCell::new(Vec::new()));
    let window = retained_layout_window(&mut cx, probes.clone());

    draw_frame(&mut cx, window.into());
    let first = probes.borrow().clone();

    let stats = draw_frame(&mut cx, window.into());
    assert_eq!(
        stats.nodes_created, 0,
        "an unchanged frame should not allocate a single node"
    );
    assert!(stats.nodes_reused > 0);
    assert_eq!(
        stats.style_writes, 0,
        "writing a style dirties the node and its ancestors, undoing the point of retaining it"
    );
    assert_eq!(stats.children_writes, 0);
    assert_eq!(stats.measure_rebinds, 0);
    assert_eq!(
        &first,
        &*probes.borrow(),
        "a frame laid out from retained nodes must land in the same place as the frame before it"
    );
}

#[test]
fn a_retained_frame_follows_a_style_change() {
    let mut cx = TestAppContext::single();
    let probes = Rc::new(RefCell::new(Vec::new()));
    let window = retained_layout_window(&mut cx, probes.clone());

    draw_frame(&mut cx, window.into());
    let before = probes.borrow()[0];

    change_and_draw(&mut cx, window, |view| view.row_width = px(400.));
    let after = probes.borrow()[0];

    assert_eq!(
        after.size.width - before.size.width,
        px(200.),
        "the probe fills what is left of the row, so widening the row must widen it too"
    );
    assert_eq!(
        probes.borrow().len(),
        4,
        "widening rows should not have changed how many there are"
    );
}

#[test]
fn a_retained_frame_follows_a_text_change() {
    let mut cx = TestAppContext::single();
    let probes = Rc::new(RefCell::new(Vec::new()));
    let window = retained_layout_window(&mut cx, probes.clone());

    draw_frame(&mut cx, window.into());
    let before = probes.borrow()[0];

    let stats = change_and_draw(&mut cx, window, |view| {
        view.label = "abcdefghijklmnop".into()
    });
    let after = probes.borrow()[0];

    assert!(
        stats.measure_rebinds > 0,
        "changed text must invalidate the measurement it is cached under: {stats:?}"
    );
    assert!(
        after.origin.x > before.origin.x,
        "longer text should push the probe further along the row, \
         got {before:?} then {after:?}"
    );
}

#[test]
fn a_retained_frame_follows_a_structural_change() {
    let mut cx = TestAppContext::single();
    let probes = Rc::new(RefCell::new(Vec::new()));
    let window = retained_layout_window(&mut cx, probes.clone());

    draw_frame(&mut cx, window.into());
    assert_eq!(probes.borrow().len(), 4);
    let row_height = probes.borrow()[1].origin.y - probes.borrow()[0].origin.y;

    change_and_draw(&mut cx, window, |view| view.rows = 7);
    assert_eq!(probes.borrow().len(), 7);
    assert_eq!(
        probes.borrow()[6].origin.y - probes.borrow()[0].origin.y,
        row_height * 6.,
        "rows added to a retained tree must stack like the ones already there"
    );

    let stats = change_and_draw(&mut cx, window, |view| view.rows = 2);
    assert_eq!(probes.borrow().len(), 2);
    assert!(
        stats.nodes_freed > 0,
        "nodes that left the tree must be released rather than accumulated: {stats:?}"
    );
}

/// Recoloring text changes nothing about how much space it takes, so it has
/// no business invalidating a measurement. It only does when the color is
/// baked into the shaped lines, which is why decoration is replaced on them
/// in place instead.
#[test]
fn recoloring_text_leaves_the_layout_alone() {
    let mut cx = TestAppContext::single();
    let probes = Rc::new(RefCell::new(Vec::new()));
    let window = retained_layout_window(&mut cx, probes.clone());
    draw_frame(&mut cx, window.into());
    let before = probes.borrow().clone();

    let stats = change_and_draw(&mut cx, window, |view| {
        view.text_color = hsla(0.6, 0.9, 0.5, 1.0)
    });

    assert_eq!(
        stats.measure_rebinds, 0,
        "a color cannot change how much room the text needs: {stats:?}"
    );
    assert_eq!(
        stats.style_writes, 0,
        "colors are not part of a Taffy style in the first place: {stats:?}"
    );
    assert_eq!(
        &before,
        &*probes.borrow(),
        "recolored text should land exactly where it did before"
    );

    // Lengthening it, on the other hand, has to.
    let stats = change_and_draw(&mut cx, window, |view| {
        view.label = "abcdefghijklmnop".into()
    });
    assert!(
        stats.measure_rebinds > 0,
        "changed text must still invalidate its measurement: {stats:?}"
    );
}

/// Rows are matched to their nodes by position unless they say otherwise,
/// so inserting at the front of a list makes every row that follows look
/// like a different row. An `ElementId` is how a row says otherwise.
#[test]
fn rows_identified_by_an_element_id_keep_their_nodes_when_one_is_inserted_ahead() {
    fn insert_at_head(cx: &mut TestAppContext, keyed: bool) -> LayoutStats {
        let probes = Rc::new(RefCell::new(Vec::new()));
        let window = retained_layout_window(cx, probes.clone());
        change_and_draw(cx, window, |view| view.keyed = keyed);

        let stats = change_and_draw(cx, window, |view| {
            view.rows += 1;
            view.row_ids.insert(0, 100);
        });
        assert_eq!(probes.borrow().len(), 5);
        stats
    }

    let mut cx = TestAppContext::single();
    let positional = insert_at_head(&mut cx, false);
    let keyed = insert_at_head(&mut cx, true);

    // Shifting keys do not throw nodes away — the row now at index 1
    // claims the node index 1 had — they hand each node to a different row,
    // which then has to write its own style over it. That write is what
    // dirties the node and every ancestor above it.
    assert!(
        positional.style_writes > 0,
        "rows matched by position should be restyled once they shift: {positional:?}"
    );
    assert_eq!(
        keyed.style_writes, 0,
        "rows matched by an ElementId should keep the node they styled, \
         wrote {} against {} for positional rows",
        keyed.style_writes, positional.style_writes
    );
}

/// A row built as a component, which is what lists are mostly made of. It
/// identifies the element it renders into, which is as far as a
/// component's own id reaches: the component itself reports none.
#[derive(IntoElement)]
struct ComponentRow {
    id: u64,
    probes: Rc<RefCell<Vec<Bounds<Pixels>>>>,
}

impl RenderOnce for ComponentRow {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let probes = self.probes;
        div()
            .id(("row", self.id))
            .flex()
            .flex_row()
            .w(px(200.) + px((self.id % 5) as f32 * 10.))
            .h(px(20.))
            .child("ab")
            .child(
                canvas(
                    move |bounds, _, _| probes.borrow_mut().push(bounds),
                    |_, _, _, _| {},
                )
                .flex_1()
                .h_full(),
            )
    }
}

/// A list of [`ComponentRow`]s, each keyed by its id when `keyed` is set.
struct ComponentRows {
    row_ids: Vec<u64>,
    keyed: bool,
    probes: Rc<RefCell<Vec<Bounds<Pixels>>>>,
}

impl Render for ComponentRows {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.probes.borrow_mut().clear();
        let keyed = self.keyed;
        let probes = self.probes.clone();
        div()
            .flex()
            .flex_col()
            .children(self.row_ids.iter().map(move |&id| {
                let row = ComponentRow {
                    id,
                    probes: probes.clone(),
                };
                if keyed {
                    row.key(("row", id)).into_any_element()
                } else {
                    row.into_any_element()
                }
            }))
    }
}

fn component_rows_window(
    cx: &mut TestAppContext,
    keyed: bool,
) -> (
    WindowHandle<ComponentRows>,
    Rc<RefCell<Vec<Bounds<Pixels>>>>,
) {
    let probes = Rc::new(RefCell::new(Vec::new()));
    let window = cx.add_window({
        let probes = probes.clone();
        move |_, _| ComponentRows {
            row_ids: (0..4).collect(),
            keyed,
            probes,
        }
    });
    draw_frame(cx, window.into());
    (window, probes)
}

/// A component's id stops at the element it renders into, so a list of
/// components is matched by position however its rows are identified
/// inside. A key is what reaches the list.
#[test]
fn components_given_a_key_keep_their_nodes_when_one_is_inserted_ahead() {
    fn insert_at_head(cx: &mut TestAppContext, keyed: bool) -> LayoutStats {
        let (window, probes) = component_rows_window(cx, keyed);
        let stats = change_and_draw(cx, window, |view| view.row_ids.insert(0, 100));
        assert_eq!(probes.borrow().len(), 5);
        stats
    }

    let mut cx = TestAppContext::single();
    let identified_inside = insert_at_head(&mut cx, false);
    let keyed = insert_at_head(&mut cx, true);

    // Identified only inside, a shifted row is not handed its
    // neighbour's node, since the id inside is part of the path; it gets a
    // new one, which is as much a rebuild. Keyed, only the new row does.
    assert_eq!(
        keyed.style_writes, 0,
        "keyed components should keep the node they styled: {keyed:?}"
    );
    assert!(
        keyed.nodes_created > 0,
        "the inserted row needs nodes of its own: {keyed:?}"
    );
    assert_eq!(
        identified_inside.nodes_created,
        5 * keyed.nodes_created,
        "every component identified only inside should be rebuilt once the rows shift, \
         and only the inserted one when they are keyed: {identified_inside:?} against {keyed:?}"
    );
}

/// A key is only a step in the path a node is found by. It must not add a
/// node of its own or move anything.
#[test]
fn a_key_adds_no_layout_node_and_moves_nothing() {
    let mut cx = TestAppContext::single();
    let (plain, plain_probes) = component_rows_window(&mut cx, false);
    let (keyed, keyed_probes) = component_rows_window(&mut cx, true);

    let node_count = |cx: &mut TestAppContext, window: WindowHandle<ComponentRows>| {
        cx.update_window(window.into(), |_, window, _| window.layout_node_count())
            .unwrap()
    };
    assert_eq!(node_count(&mut cx, plain), node_count(&mut cx, keyed));
    assert_eq!(*plain_probes.borrow(), *keyed_probes.borrow());
}

/// Timing a measurement or a shaped line reads the clock twice, which is
/// not free on a frame full of text, so the times are kept only once the
/// stats have been reset — which is how a benchmark asks for them. The
/// counts are kept all along.
#[test]
fn layout_times_are_kept_only_once_the_stats_are_reset() {
    let mut cx = TestAppContext::single();
    let probes = Rc::new(RefCell::new(Vec::new()));
    let window = retained_layout_window(&mut cx, probes);
    let stats = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            window.layout_stats()
        })
        .unwrap()
    };

    let untimed = stats(&mut cx);
    assert!(untimed.compute_layout_calls > 0 && untimed.lines_shaped > 0);
    assert_eq!(untimed.compute_layout_time, Duration::ZERO);
    assert_eq!(untimed.measure_time, Duration::ZERO);
    assert_eq!(untimed.shape_time, Duration::ZERO);

    cx.update_window(window.into(), |_, window, _| window.reset_layout_stats())
        .unwrap();
    change_and_draw(&mut cx, window, |view| {
        view.label = "a label to shape".into()
    });
    let timed = stats(&mut cx);
    assert!(timed.compute_layout_time > Duration::ZERO);
}

/// Elements are given the ids the inspector finds them by only while it is
/// open, since building one copies the whole element id stack. Opening it
/// has to bring them back on the next frame.
#[test]
fn inspector_ids_are_built_only_while_the_inspector_is_open() {
    let mut cx = TestAppContext::single();
    let probes = Rc::new(RefCell::new(Vec::new()));
    let window = retained_layout_window(&mut cx, probes);
    let inspector_ids = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, _| {
            window.rendered_frame.next_inspector_instance_ids.len()
        })
        .unwrap()
    };

    draw_frame(&mut cx, window.into());
    assert_eq!(inspector_ids(&mut cx), 0);

    cx.update_window(window.into(), |_, window, cx| window.toggle_inspector(cx))
        .unwrap();
    draw_frame(&mut cx, window.into());
    assert!(
        inspector_ids(&mut cx) > 0,
        "opening the inspector should give elements their ids again"
    );

    cx.update_window(window.into(), |_, window, cx| window.toggle_inspector(cx))
        .unwrap();
    draw_frame(&mut cx, window.into());
    assert_eq!(inspector_ids(&mut cx), 0);
}

/// A chip whose identity is given by a key or by an id.
struct ReparentedChip {
    keyed: bool,
    probes: Rc<RefCell<Vec<Bounds<Pixels>>>>,
}

impl Render for ReparentedChip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let probes = self.probes.clone();
        probes.borrow_mut().clear();
        let chip = div().pl(px(7.)).child(
            canvas(
                move |bounds, _, _| probes.borrow_mut().push(bounds),
                |_, _, _, _| {},
            )
            .w(px(10.))
            .h(px(10.)),
        );
        div().flex().pl(px(50.)).child(if self.keyed {
            chip.key("chip").into_any_element()
        } else {
            chip.id("chip").into_any_element()
        })
    }
}

/// A chip that trades its key for an id of the same value takes the key
/// its child used to find its node by, so it gets a new node, and the
/// child is handed the node the chip had. That node is still listed under
/// the row when the chip's new node adopts it, and the row rewriting its
/// children must not cut the link the child's position is added up along.
#[test]
fn a_node_adopted_from_another_parent_keeps_its_position() {
    let mut cx = TestAppContext::single();
    let probes = Rc::new(RefCell::new(Vec::new()));
    let window = cx.add_window({
        let probes = probes.clone();
        move |_, _| ReparentedChip {
            keyed: true,
            probes,
        }
    });

    draw_frame(&mut cx, window.into());
    assert_eq!(probes.borrow()[0].origin.x, px(57.));

    change_and_draw(&mut cx, window, |view| view.keyed = false);
    assert_eq!(
        probes.borrow()[0].origin.x,
        px(57.),
        "the child should still be offset by the row's and the chip's padding"
    );
}

/// Rows of a uniform list five rows tall, scrolled to `scroll_top`.
struct ScrolledRows {
    row_ids: Vec<u64>,
    keyed: bool,
    scroll_top: Pixels,
    scroll: UniformListScrollHandle,
}

impl Render for ScrolledRows {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.), -self.scroll_top));
        let row_ids = self.row_ids.clone();
        let keyed = self.keyed;
        div().w(px(300.)).h(px(100.)).child(
            uniform_list("rows", row_ids.len(), move |range, _, _| {
                range
                    .map(|ix| {
                        // Rows have to be distinguishable for a row landing
                        // on a neighbour's node to show.
                        let id = row_ids[ix];
                        let row = div().w(px(200.) + px((id % 5) as f32 * 10.)).h(px(20.));
                        if keyed {
                            row.id(("row", id)).into_any_element()
                        } else {
                            row.into_any_element()
                        }
                    })
                    .collect()
            })
            .track_scroll(&self.scroll)
            .size_full(),
        )
    }
}

fn scrolled_rows_window(cx: &mut TestAppContext, keyed: bool) -> WindowHandle<ScrolledRows> {
    let window = cx.add_window(move |_, _| ScrolledRows {
        row_ids: (0..50).collect(),
        keyed,
        scroll_top: px(0.),
        scroll: UniformListScrollHandle::new(),
    });
    draw_frame(cx, window.into());
    draw_frame(cx, window.into());
    window
}

/// A list lays out only the items in view, so an item without an id was
/// matched by where it came among them, and a list scrolled by one row
/// handed every item its neighbour's nodes. Matched by its index, an item
/// keeps its nodes while it stays in view.
#[test]
fn unidentified_list_items_keep_their_nodes_when_the_list_scrolls() {
    let mut cx = TestAppContext::single();
    let window = scrolled_rows_window(&mut cx, false);

    let scrolled = change_and_draw(&mut cx, window, |view| view.scroll_top = px(20.));
    assert_eq!(
        scrolled.style_writes, 0,
        "rows still in view should keep the node they styled: {scrolled:?}"
    );
    assert_eq!(
        scrolled.nodes_created, 1,
        "only the row scrolling in should need a node: {scrolled:?}"
    );
}

/// Keying list items by index must not come between an item and an id of
/// its own: an item identified by its data keeps its nodes when an item
/// is inserted ahead of it, which its index could not do.
#[test]
fn identified_list_items_keep_their_nodes_when_one_is_inserted_ahead() {
    let mut cx = TestAppContext::single();
    let window = scrolled_rows_window(&mut cx, true);

    let inserted = change_and_draw(&mut cx, window, |view| view.row_ids.insert(0, 100));
    assert_eq!(
        inserted.style_writes, 0,
        "identified rows should keep the node they styled: {inserted:?}"
    );
    // The inserted row needs a node, and so does the first row, which the
    // list lays out on its own to find the height of every row. The five
    // rows in view keeping theirs is what an index would have broken.
    assert!(
        inserted.nodes_created < 5,
        "identified rows should not be rebuilt when one is inserted ahead: {inserted:?}"
    );
}

/// Shaping is counted only when the text cache cannot answer, so a frame
/// that shows the same text as the last one shapes nothing, and one that
/// shows new text shapes exactly that.
#[test]
fn only_text_the_cache_does_not_hold_is_counted_as_shaped() {
    let mut cx = TestAppContext::single();
    let probes = Rc::new(RefCell::new(Vec::new()));
    let window = retained_layout_window(&mut cx, probes);
    draw_frame(&mut cx, window.into());

    let unchanged = draw_frame(&mut cx, window.into());
    assert_eq!(
        unchanged.lines_shaped, 0,
        "text shown last frame should come from the cache: {unchanged:?}"
    );

    let relabeled = change_and_draw(&mut cx, window, |view| view.label = "cd".into());
    assert!(
        relabeled.lines_shaped > 0,
        "text not shown before has to be shaped: {relabeled:?}"
    );
}

/// Rows that each show text of their own, matched to their nodes by
/// position.
struct ShiftingRows {
    row_ids: Vec<u64>,
}

impl Render for ShiftingRows {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .children(self.row_ids.iter().map(|id| {
                div()
                    .h(px(20.))
                    .child(SharedString::from(format!("row {id}")))
            }))
    }
}

/// A retained text node answers from the lines it already holds and never
/// asks the line layout cache for them. Those lines still have to stay in
/// the cache: when unidentified rows shift by one, every row lands on a
/// neighbour's node, and the text it brings was on screen all along.
#[test]
fn text_kept_by_its_node_is_not_reshaped_when_rows_shift_onto_other_nodes() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|_, _| ShiftingRows {
        row_ids: (0..8).collect(),
    });
    let handle: AnyWindowHandle = window.into();
    // Enough frames for anything only the first frame asked the cache for
    // to have been forgotten, had nobody asked since.
    for _ in 0..3 {
        draw_frame(&mut cx, handle);
    }

    // Counted from before the change, which can draw a frame of its own;
    // see `change_and_draw`.
    cx.update_window(handle, |_, window, _| window.reset_layout_stats())
        .unwrap();
    window
        .update(&mut cx, |view, _, cx| {
            view.row_ids.remove(0);
            cx.notify();
        })
        .unwrap();
    let shifted = cx
        .update_window(handle, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.layout_stats()
        })
        .unwrap();

    assert!(
        shifted.style_writes > 0 || shifted.measure_calls > 0,
        "rows should have moved onto other nodes for this to test anything: {shifted:?}"
    );
    assert_eq!(
        shifted.lines_shaped, 0,
        "every row's text was on screen the frame before and should come from the cache: {shifted:?}"
    );
}

/// Lines outlive the frames that asked for them only while something
/// holds them. Once the text is gone from the tree, and with it the nodes
/// that held its lines, the cache has to let them go too.
#[test]
fn text_nothing_holds_any_more_leaves_the_line_layout_cache() {
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|_, _| ShiftingRows {
        row_ids: (0..8).collect(),
    });
    let handle: AnyWindowHandle = window.into();
    draw_frame(&mut cx, handle);

    window
        .update(&mut cx, |view, _, cx| {
            view.row_ids.clear();
            cx.notify();
        })
        .unwrap();
    for _ in 0..3 {
        draw_frame(&mut cx, handle);
    }

    cx.update_window(handle, |_, window, _| window.reset_layout_stats())
        .unwrap();
    window
        .update(&mut cx, |view, _, cx| {
            view.row_ids = (0..8).collect();
            cx.notify();
        })
        .unwrap();
    let shown_again = cx
        .update_window(handle, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.layout_stats()
        })
        .unwrap();
    assert_eq!(
        shown_again.lines_shaped, 8,
        "text removed frames ago should have left the cache: {shown_again:?}"
    );
}

/// A window root, sized explicitly or left to fill the window, that records
/// where its child ends up. The same as the one `window.rs`'s tests use.
struct RootView {
    explicit_size: bool,
    child_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl Render for RootView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let child_bounds = self.child_bounds.clone();
        let root = div().flex().flex_col().child(
            canvas(
                move |bounds, _, _| child_bounds.set(bounds),
                |_, _, _, _| {},
            )
            .size_full(),
        );
        if self.explicit_size {
            root.w(px(300.)).h(px(200.))
        } else {
            root
        }
    }
}

/// The window root is the one node whose style Taffy does not hold as the
/// element wrote it, because an `auto` size is rewritten to fill the
/// viewport. Retaining that node means the rewrite has to stay recoverable
/// across frames, or the root silently stops following the window.
#[test]
fn a_retained_auto_sized_root_keeps_filling_a_resized_window() {
    let mut cx = TestAppContext::single();
    let child_bounds = Rc::new(Cell::new(Bounds::default()));
    let window = cx.add_window({
        let child_bounds = child_bounds.clone();
        move |_, _| RootView {
            explicit_size: false,
            child_bounds,
        }
    });
    let handle: AnyWindowHandle = window.into();

    for resized_size in [
        size(px(800.), px(600.)),
        size(px(640.), px(480.)),
        size(px(1024.), px(768.)),
        // Back to a size already seen, to catch a stale record of the
        // previous fill rather than of the request behind it.
        size(px(800.), px(600.)),
    ] {
        cx.simulate_window_resize(handle, resized_size);
        draw_frame(&mut cx, handle);
        assert_eq!(
            child_bounds.get().size,
            resized_size,
            "an auto-sized root must still fill the window after it is resized"
        );
    }

    // And a frame that changes nothing must leave the stretched root alone
    // rather than rewriting it and dirtying the whole tree.
    let stats = draw_frame(&mut cx, handle);
    assert_eq!(
        stats.style_writes, 0,
        "a settled auto-sized root should not be restyled every frame: {stats:?}"
    );
}

#[test]
fn retaining_layout_nodes_does_not_grow_the_tree_over_time() {
    let mut cx = TestAppContext::single();
    let probes = Rc::new(RefCell::new(Vec::new()));
    let window = retained_layout_window(&mut cx, probes);

    for frame in 0..12 {
        // Oscillate the shape so nodes are created and released repeatedly
        // rather than settling.
        change_and_draw(&mut cx, window, |view| view.rows = 2 + frame % 5);
    }

    let live = cx
        .update_window(window.into(), |_, window, _| window.layout_node_count())
        .unwrap();
    change_and_draw(&mut cx, window, |view| view.rows = 2);
    let settled = cx
        .update_window(window.into(), |_, window, _| window.layout_node_count())
        .unwrap();

    assert!(
        settled <= live,
        "the tree should shrink back down, held {live} nodes and settled at {settled}"
    );
    assert!(
        settled < 40,
        "two rows should not need {settled} layout nodes"
    );
}
