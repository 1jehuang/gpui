use crate::fast::dependencies::RenderDependencies;
use crate::{
    AnyElement, AnyEntity, AnyWeakEntity, App, Bounds, Context, Element, ElementId, Entity,
    EntityId, GlobalElementId, InspectorElementId, IntoElement, LayoutId, Pixels, Render,
    RenderOnce, RetainedLayout, Style, StyleRefinement, WeakEntity,
};
use crate::{Empty, Window};
use anyhow::Result;
use refineable::Refineable;
use std::mem;
use std::rc::Rc;
use std::{any::TypeId, fmt};

/// A dynamically-typed view handle that can be downcast to a specific `Entity<V>`.
///
/// This is the type-erased counterpart to [`ViewElement`]: it holds an entity plus
/// a function pointer to its render, and is itself a [`View`], so embedding it as an
/// element goes through the same [`ViewElement`] machinery as any other view.
#[derive(Clone, Debug)]
pub struct AnyView {
    entity: AnyEntity,
    render: fn(&AnyView, &mut Window, &mut App) -> AnyElement,
}

impl<V: Render> From<Entity<V>> for AnyView {
    fn from(value: Entity<V>) -> Self {
        AnyView {
            entity: value.into_any(),
            render: any_view::render::<V>,
        }
    }
}

impl AnyView {
    /// Embed this view as a cached [`ViewElement`] laid out at `style`.
    ///
    /// The rendered subtree is recycled from the previous frame unless
    /// [Context::notify] was called on the backing entity since it was rendered
    /// (or [Window::refresh] is called, which ignores caching).
    pub fn cached(self, style: StyleRefinement) -> ViewElement<AnyView> {
        ViewElement::new(self).cached(style)
    }

    /// Convert this to a weak handle.
    pub fn downgrade(&self) -> AnyWeakView {
        AnyWeakView {
            entity: self.entity.downgrade(),
            render: self.render,
        }
    }

    /// Convert this to a [Entity] of a specific type.
    /// If this handle does not contain a view of the specified type, returns itself in an `Err` variant.
    pub fn downcast<T: 'static>(self) -> Result<Entity<T>, Self> {
        match self.entity.downcast() {
            Ok(entity) => Ok(entity),
            Err(entity) => Err(Self {
                entity,
                render: self.render,
            }),
        }
    }

    /// Gets the [TypeId] of the underlying view.
    pub fn entity_type(&self) -> TypeId {
        self.entity.entity_type
    }

    /// The [`EntityId`] of this view.
    pub fn entity_id(&self) -> EntityId {
        self.entity.entity_id()
    }
}

impl PartialEq for AnyView {
    fn eq(&self, other: &Self) -> bool {
        self.entity == other.entity
    }
}

impl Eq for AnyView {}

/// `AnyView` is the type-erased [`View`]: its `render` is a function pointer rather
/// than a concrete type, but it participates in the reactive graph exactly like any
/// other view via [`ViewElement`].
impl View for AnyView {
    fn entity_id(&self) -> Option<EntityId> {
        Some(self.entity.entity_id())
    }

    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        (self.render)(&self, window, cx)
    }
}

impl<V: 'static + Render> IntoElement for Entity<V> {
    type Element = ViewElement<Entity<V>>;

    fn into_element(self) -> Self::Element {
        ViewElement::new(self)
    }
}

impl IntoElement for AnyView {
    type Element = ViewElement<AnyView>;

    fn into_element(self) -> Self::Element {
        ViewElement::new(self)
    }
}

/// A weak, dynamically-typed view handle.
pub struct AnyWeakView {
    entity: AnyWeakEntity,
    render: fn(&AnyView, &mut Window, &mut App) -> AnyElement,
}

impl AnyWeakView {
    /// Upgrade to a strong `AnyView` handle, if the view is still alive.
    pub fn upgrade(&self) -> Option<AnyView> {
        let entity = self.entity.upgrade()?;
        Some(AnyView {
            entity,
            render: self.render,
        })
    }
}

impl<V: 'static + Render> From<WeakEntity<V>> for AnyWeakView {
    fn from(view: WeakEntity<V>) -> Self {
        AnyWeakView {
            entity: view.into(),
            render: any_view::render::<V>,
        }
    }
}

impl PartialEq for AnyWeakView {
    fn eq(&self, other: &Self) -> bool {
        self.entity == other.entity
    }
}

impl std::fmt::Debug for AnyWeakView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnyWeakView")
            .field("entity_id", &self.entity.entity_id)
            .finish_non_exhaustive()
    }
}

mod any_view {
    use crate::{AnyElement, AnyView, App, IntoElement, Render, Window};

    pub(crate) fn render<V: 'static + Render>(
        view: &AnyView,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let view = view.clone().downcast::<V>().unwrap();
        // Record the view's Render type name so the accessibility debug dump can
        // attribute nodes to the view that produced them.
        #[cfg(debug_assertions)]
        window
            .a11y
            .view_type_names
            .insert(view.entity_id(), std::any::type_name::<V>());
        view.update(cx, |view, cx| view.render(window, cx).into_any_element())
    }
}

/// A renderable that participates in GPUI's reactive graph — the unifying model
/// behind [`Render`] and [`RenderOnce`].
///
/// When `entity_id()` returns `Some`, that id becomes the view's identity: it gets
/// a unique element-id space (so internal `use_state` / `.id(..)` never collide
/// across siblings) and `cx.notify()` on that entity re-renders only this view's
/// subtree. `None` behaves like a stateless component.
///
/// You rarely implement `View` directly. `Entity<T: Render>` and any `T: RenderOnce`
/// get a blanket impl below; implement it by hand only when a component needs both
/// parent-supplied props *and* a backing entity for identity.
pub trait View: 'static + Sized {
    /// This view's identity, if it has one. A view typically holds the backing
    /// entity as a field and returns its [`EntityId`] here.
    ///
    /// The id becomes this view's [`ElementId`], so two views keyed on the same
    /// entity must not be rendered at the same position in the element tree
    /// (e.g. as siblings under the same parent): their internal element state
    /// (`use_state`, scroll offsets, etc.) would silently collide. Nesting is
    /// fine — the id is scoped by the parent path.
    fn entity_id(&self) -> Option<EntityId>;

    /// Render this view into an element tree, consuming `self`.
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement;
}

/// A stateless component (`RenderOnce`) is a `View` with no identity.
impl<T: RenderOnce> View for T {
    fn entity_id(&self) -> Option<EntityId> {
        None
    }

    #[inline]
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        RenderOnce::render(self, window, cx)
    }
}

/// An entity that renders itself (`Render`) is a `View` keyed on its own id.
impl<T: Render> View for Entity<T> {
    fn entity_id(&self) -> Option<EntityId> {
        Some(Entity::entity_id(self))
    }

    #[inline]
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        self.update(cx, |this, cx| {
            Render::render(this, window, cx).into_any_element()
        })
    }
}

impl<T: Render> Entity<T> {
    /// Embed this entity as a cached [`ViewElement`] laid out at `style`.
    ///
    /// The rendered subtree is reused until the entity is notified (or the
    /// cached bounds / text style change). Caching requires a definite size:
    /// a cached view is laid out from `style` and is *not* measured from its
    /// contents. Use [`ViewElement::new`] (or `.child(entity)`) for the
    /// uncached case.
    #[track_caller]
    pub fn cached(self, style: StyleRefinement) -> ViewElement<Entity<T>> {
        ViewElement::new(self).cached(style)
    }
}

/// The element type for [`View`] implementations. Wraps a `View` and hooks it
/// into layout, prepaint, and paint. Constructed via [`ViewElement::new`].
#[doc(hidden)]
pub struct ViewElement<V: View> {
    view: Option<V>,
    entity_id: Option<EntityId>,
    cached_style: Option<StyleRefinement>,
    #[cfg(debug_assertions)]
    source: &'static core::panic::Location<'static>,
}

impl<V: View> ViewElement<V> {
    /// Wrap a [`View`] as an element.
    #[track_caller]
    pub fn new(view: V) -> Self {
        let entity_id = view.entity_id();
        ViewElement {
            entity_id,
            cached_style: None,
            view: Some(view),
            #[cfg(debug_assertions)]
            source: core::panic::Location::caller(),
        }
    }

    /// Enable caching of this view's rendered subtree, laid out at `style`.
    /// The composer supplies the layout style because caching skips rendering
    /// the contents to measure them.
    ///
    /// Crate-private on purpose: caching is only sound for entity-backed views,
    /// where [`Context::notify`] is the contract that busts the cache. A stateless
    /// view has no such contract, so a frozen subtree could never be invalidated.
    /// Reach this through [`Entity::cached`] or [`AnyView::cached`], which are
    /// entity-backed by construction.
    pub(crate) fn cached(mut self, style: StyleRefinement) -> Self {
        self.cached_style = Some(style);
        self
    }
}

impl<V: View> IntoElement for ViewElement<V> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// How a view was laid out, for its prepaint to follow up on.
#[doc(hidden)]
pub struct ViewLayoutState(ViewLayout);

/// What a view's prepaint left for its paint.
#[doc(hidden)]
pub struct ViewPrepaintState(ViewPrepaint);

enum ViewLayout {
    /// Laid out by the style it is cached with; built at prepaint if at all.
    Cached,
    /// Built, and laid out by its content.
    Built {
        element: AnyElement,
        /// How to lay it out again without building it, and what it read
        /// while it was built and laid out, when it is retained.
        retained: Option<(Option<Rc<RetainedLayout>>, RenderDependencies)>,
    },
    /// Laid out as it was last frame without being built, from the record
    /// it left then, which it is drawn again from if nothing moved it.
    Retained { previous: usize },
    /// Moved on to prepaint.
    Taken,
}

enum ViewPrepaint {
    /// Built this frame, into its record in this frame if it has one.
    Built {
        element: AnyElement,
        record: Option<usize>,
    },
    /// Drawn from last frame, as the record at this index in this frame.
    Reused(usize),
}

impl<V: View> Element for ViewElement<V> {
    type RequestLayoutState = ViewLayoutState;
    type PrepaintState = ViewPrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.entity_id.map(ElementId::View)
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        #[cfg(debug_assertions)]
        return Some(self.source);

        #[cfg(not(debug_assertions))]
        return None;
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let (layout_id, layout) = self.request_view_layout(global_id, window, cx);
        (layout_id, ViewLayoutState(layout))
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> ViewPrepaintState {
        let layout = mem::replace(&mut layout.0, ViewLayout::Taken);
        ViewPrepaintState(self.prepaint_view(global_id, bounds, layout, window, cx))
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(entity_id) = self.entity_id {
            // Stateful path.
            paint_view(entity_id, global_id, &mut prepaint.0, window, cx);
        } else {
            // Stateless path: just paint the element.
            paint_component(std::any::type_name::<V>(), &mut prepaint.0, window, cx);
        }
    }
}

impl<V: View> ViewElement<V> {
    fn request_view_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ViewLayout) {
        if let Some(entity_id) = self.entity_id {
            // Stateful path: create a reactive boundary.
            window.with_rendered_view(entity_id, |window| {
                let caching_disabled = window.is_inspector_picking(cx);
                match self.cached_style.as_ref() {
                    Some(style) if !caching_disabled => {
                        let mut root_style = Style::default();
                        root_style.refine(style);
                        let layout_id = window.request_layout(root_style, None, cx);
                        (layout_id, ViewLayout::Cached)
                    }
                    _ if window.view_retention() => {
                        let global_id = global_id.expect("a view always has an id");
                        if !window.dirty_views.contains(&entity_id)
                            && let Some(previous) = window.reusable_retained(global_id, cx)
                            && let Some(layout_id) = window.reuse_retained_layout(previous, cx)
                        {
                            return (layout_id, ViewLayout::Retained { previous });
                        }
                        let recording = window.begin_retained_layout(cx);
                        let mut element = self
                            .view
                            .take()
                            .unwrap()
                            .render(window, cx)
                            .into_any_element();
                        let layout_id = element.request_layout(window, cx);
                        let retained = window.finish_retained_layout(recording, layout_id, cx);
                        (
                            layout_id,
                            ViewLayout::Built {
                                element,
                                retained: Some(retained),
                            },
                        )
                    }
                    _ => {
                        let mut element = self
                            .view
                            .take()
                            .unwrap()
                            .render(window, cx)
                            .into_any_element();
                        let layout_id = element.request_layout(window, cx);
                        (
                            layout_id,
                            ViewLayout::Built {
                                element,
                                retained: None,
                            },
                        )
                    }
                }
            })
        } else {
            // Stateless path: isolate subtree via type name (no entity identity).
            window.with_id(
                ElementId::Name(std::any::type_name::<V>().into()),
                |window| {
                    let mut element = self
                        .view
                        .take()
                        .unwrap()
                        .render(window, cx)
                        .into_any_element();
                    let layout_id = element.request_layout(window, cx);
                    (
                        layout_id,
                        ViewLayout::Built {
                            element,
                            retained: None,
                        },
                    )
                },
            )
        }
    }

    fn prepaint_view(
        &mut self,
        global_id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        layout: ViewLayout,
        window: &mut Window,
        cx: &mut App,
    ) -> ViewPrepaint {
        let Some(entity_id) = self.entity_id else {
            // Stateless path: just prepaint the element.
            let ViewLayout::Built { mut element, .. } = layout else {
                unreachable!("a stateless view is always built");
            };
            window.with_id(
                ElementId::Name(std::any::type_name::<V>().into()),
                |window| {
                    element.prepaint(window, cx);
                },
            );
            return ViewPrepaint::Built {
                element,
                record: None,
            };
        };

        window.set_view_id(entity_id);
        window.with_rendered_view(entity_id, |window| {
            let global_id = global_id.expect("a view always has an id");
            match layout {
                ViewLayout::Built {
                    mut element,
                    retained: None,
                } => {
                    element.prepaint(window, cx);
                    ViewPrepaint::Built {
                        element,
                        record: None,
                    }
                }
                ViewLayout::Built {
                    mut element,
                    retained: Some((layout, dependencies)),
                } => {
                    let recording = window.begin_retained(global_id, cx);
                    element.prepaint(window, cx);
                    let record = window.finish_retained_prepaint(
                        recording,
                        bounds,
                        layout,
                        Some(dependencies),
                        cx,
                    );
                    ViewPrepaint::Built { element, record }
                }
                ViewLayout::Retained { previous } => {
                    if window.retained_context_matches(previous, bounds) {
                        return ViewPrepaint::Reused(
                            window.reuse_retained_prepaint(previous, true, cx),
                        );
                    }
                    self.build_at_retained_layout(previous, global_id, bounds, window, cx)
                }
                ViewLayout::Cached => {
                    if !window.dirty_views.contains(&entity_id)
                        && let Some(previous) = window.reusable_retained(global_id, cx)
                        && window.retained_context_matches(previous, bounds)
                    {
                        return ViewPrepaint::Reused(
                            window.reuse_retained_prepaint(previous, false, cx),
                        );
                    }
                    let recording = window.begin_retained(global_id, cx);
                    let mut element = self
                        .view
                        .take()
                        .unwrap()
                        .render(window, cx)
                        .into_any_element();
                    element.layout_as_root(bounds.size.into(), window, cx);
                    element.prepaint_at(bounds.origin, window, cx);
                    let record = window.finish_retained_prepaint(recording, bounds, None, None, cx);
                    ViewPrepaint::Built { element, record }
                }
                ViewLayout::Taken => unreachable!("a view is prepainted once"),
            }
        })
    }

    /// Builds a view whose layout was reused, but which cannot be drawn again
    /// from last frame because it is drawn somewhere else: it moved, or what it
    /// inherits changed. It is laid out at the nodes it kept, which it finds
    /// again as it requests them. Nothing it depends on changed, so it asks
    /// for the layout it had; if it asks for another after all, it is laid out
    /// within the bounds it was given, and on the next frame from scratch.
    #[inline(never)]
    fn build_at_retained_layout(
        &mut self,
        previous: usize,
        global_id: &GlobalElementId,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> ViewPrepaint {
        let root = window.retained_layout_root(previous);
        window.release_retained_layout(previous);
        let layout_recording = window.begin_retained_layout(cx);
        let changes_before = window.layout_changes();
        let view = self.view.take().unwrap();
        let (mut element, layout_id) = window.with_layout_key_of_prepainting_element(|window| {
            let mut element = view.render(window, cx).into_any_element();
            let layout_id = element.request_layout(window, cx);
            (element, layout_id)
        });
        let unchanged = window.layout_changes() == changes_before;
        let (layout, dependencies) = window.finish_retained_layout(layout_recording, layout_id, cx);

        let recording = window.begin_retained(global_id, cx);
        if Some(layout_id) == root {
            if !unchanged {
                window.relayout_in_place(layout_id, bounds.size.into(), cx);
                window.request_animation_frame();
            }
            element.prepaint(window, cx);
        } else {
            element.layout_as_root(bounds.size.into(), window, cx);
            element.prepaint_at(bounds.origin, window, cx);
            window.request_animation_frame();
        }
        let record =
            window.finish_retained_prepaint(recording, bounds, layout, Some(dependencies), cx);
        ViewPrepaint::Built { element, record }
    }
}

/// A view that renders nothing
pub struct EmptyView;

impl Render for EmptyView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

#[inline(never)]
fn paint_view(
    entity_id: EntityId,
    global_id: Option<&GlobalElementId>,
    prepaint: &mut ViewPrepaint,
    window: &mut Window,
    cx: &mut App,
) {
    window.with_rendered_view(entity_id, |window| match prepaint {
        ViewPrepaint::Reused(index) => window.reuse_retained_paint(*index),
        ViewPrepaint::Built {
            element,
            record: None,
        } => element.paint(window, cx),
        ViewPrepaint::Built {
            element,
            record: Some(record),
        } => {
            let global_id = global_id.expect("a view always has an id");
            let recording = window.begin_retained_paint(Some(*record), global_id, cx);
            element.paint(window, cx);
            window.finish_retained_paint(recording, cx);
        }
    });
}

#[inline(never)]
fn paint_component(
    name: &'static str,
    prepaint: &mut ViewPrepaint,
    window: &mut Window,
    cx: &mut App,
) {
    let ViewPrepaint::Built { element, .. } = prepaint else {
        unreachable!("a stateless view is always built");
    };
    window.with_id(ElementId::Name(name.into()), |window| {
        element.paint(window, cx);
    });
}

#[cfg(test)]
mod tests {
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
}
