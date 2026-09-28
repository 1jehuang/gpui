use crate::{
    AnyElement, AnyEntity, AnyWeakEntity, App, Bounds, ContentMask, Context, Element, ElementId,
    Entity, EntityId, GlobalElementId, HitboxId, InspectorElementId, IntoElement, LayoutId,
    PaintIndex, Pixels, PrepaintStateIndex, Render, RenderOnce, Style, StyleRefinement, TextStyle,
    WeakEntity,
};
use crate::{Empty, Window};
use anyhow::Result;
use collections::FxHashSet;
use refineable::Refineable;
use std::mem;
use std::{any::TypeId, fmt, ops::Range};

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

struct ViewElementState {
    prepaint_range: Range<PrepaintStateIndex>,
    paint_range: Range<PaintIndex>,
    cache_key: ViewElementCacheKey,
    accessed_entities: FxHashSet<EntityId>,
    /// Whether each hitbox whose hover the view was painted by was hovered
    /// then. The view is rendered again once any of them is hovered
    /// differently, as a memo is; see [`crate::memo`].
    hover_dependencies: Vec<(HitboxId, bool)>,
    /// The keys of the layout nodes the view was laid out with, kept alive
    /// while it is reused so that rendering it again reuses them.
    layout_keys: Vec<u64>,
}

struct ViewElementCacheKey {
    bounds: Bounds<Pixels>,
    content_mask: ContentMask<Pixels>,
    text_style: TextStyle,
}

impl<V: View> Element for ViewElement<V> {
    type RequestLayoutState = Option<AnyElement>;
    type PrepaintState = Option<AnyElement>;

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
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        if let Some(entity_id) = self.entity_id {
            // Stateful path: create a reactive boundary.
            window.with_rendered_view(entity_id, |window| {
                let caching_disabled = window.is_inspector_picking(cx);
                match self.cached_style.as_ref() {
                    Some(style) if !caching_disabled => {
                        let mut root_style = Style::default();
                        root_style.refine(style);
                        let layout_id = window.request_layout(root_style, None, cx);
                        (layout_id, None)
                    }
                    _ => {
                        let mut element = self
                            .view
                            .take()
                            .unwrap()
                            .render(window, cx)
                            .into_any_element();
                        let layout_id = element.request_layout(window, cx);
                        (layout_id, Some(element))
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
                    (layout_id, Some(element))
                },
            )
        }
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        if let Some(entity_id) = self.entity_id {
            // Stateful path.
            window.set_view_id(entity_id);
            window.with_rendered_view(entity_id, |window| {
                if let Some(mut element) = element.take() {
                    element.prepaint(window, cx);
                    return Some(element);
                }

                let global_id = global_id.unwrap();
                window.with_element_state::<ViewElementState, _>(
                    global_id,
                    |element_state, window| {
                        let content_mask = window.content_mask();
                        let text_style = window.text_style();

                        if let Some(mut element_state) = element_state
                            && element_state.cache_key.bounds == bounds
                            && element_state.cache_key.content_mask == content_mask
                            && element_state.cache_key.text_style == text_style
                            && !window.dirty_views.contains(&entity_id)
                            && !window.refreshing
                            && !window.dirty_memos.contains(global_id)
                            && !cx.has_active_drag()
                            && window.hovers_unchanged(&element_state.hover_dependencies)
                        {
                            window.keep_retained_layout(&element_state.layout_keys);
                            let prepaint_start = window.prepaint_index();
                            window.reuse_prepaint(element_state.prepaint_range.clone());
                            cx.entities
                                .extend_accessed(&element_state.accessed_entities);
                            let prepaint_end = window.prepaint_index();
                            element_state.prepaint_range = prepaint_start..prepaint_end;

                            return (None, element_state);
                        }

                        let refreshing = mem::replace(&mut window.refreshing, true);
                        window.memo_stack.push(global_id.clone());
                        let prepaint_start = window.prepaint_index();
                        let recording = window.record_claimed_layout_keys();
                        let (mut element, accessed_entities) = cx.detect_accessed_entities(|cx| {
                            let mut element = self
                                .view
                                .take()
                                .unwrap()
                                .render(window, cx)
                                .into_any_element();
                            element.layout_as_root(bounds.size.into(), window, cx);
                            element.prepaint_at(bounds.origin, window, cx);
                            element
                        });

                        let layout_keys = window.finish_recording_claimed_layout_keys(recording);
                        let prepaint_end = window.prepaint_index();
                        window.memo_stack.pop();
                        window.refreshing = refreshing;

                        (
                            Some(element),
                            ViewElementState {
                                accessed_entities,
                                hover_dependencies: Vec::new(),
                                layout_keys,
                                prepaint_range: prepaint_start..prepaint_end,
                                paint_range: PaintIndex::default()..PaintIndex::default(),
                                cache_key: ViewElementCacheKey {
                                    bounds,
                                    content_mask,
                                    text_style,
                                },
                            },
                        )
                    },
                )
            })
        } else {
            // Stateless path: just prepaint the element.
            window.with_id(
                ElementId::Name(std::any::type_name::<V>().into()),
                |window| {
                    element.as_mut().unwrap().prepaint(window, cx);
                },
            );
            Some(element.take().unwrap())
        }
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        element: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(entity_id) = self.entity_id {
            // Stateful path.
            paint_view(
                entity_id,
                self.cached_style.is_some(),
                global_id,
                element,
                window,
                cx,
            );
        } else {
            // Stateless path: just paint the element.
            paint_component(std::any::type_name::<V>(), element, window, cx);
        }
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
    cached: bool,
    global_id: Option<&GlobalElementId>,
    element: &mut Option<AnyElement>,
    window: &mut Window,
    cx: &mut App,
) {
    window.with_rendered_view(entity_id, |window| {
        let caching_disabled = window.is_inspector_picking(cx);
        if cached && !caching_disabled {
            window.with_element_state::<ViewElementState, _>(
                global_id.unwrap(),
                |element_state, window| {
                    let mut element_state = element_state.unwrap();

                    let paint_start = window.paint_index();
                    let global_id = global_id.unwrap();

                    if let Some(element) = element {
                        let refreshing = mem::replace(&mut window.refreshing, true);
                        window.memo_stack.push(global_id.clone());
                        let dependencies_start = window.memo_hover_dependencies.len();
                        element.paint(window, cx);
                        element_state.hover_dependencies =
                            window.memo_hover_dependencies[dependencies_start..].to_vec();
                        window.memo_stack.pop();
                        window.refreshing = refreshing;
                    } else {
                        // Checked against last frame's hitboxes before the view
                        // was reused; something drawn over it this frame shows
                        // only now, and the view is rendered on the next frame.
                        if !window.hovers_unchanged(&element_state.hover_dependencies) {
                            window
                                .memos_dirty_next_frame
                                .extend(window.memo_stack.iter().cloned());
                            window.memos_dirty_next_frame.insert(global_id.clone());
                            window.request_animation_frame();
                        }
                        window.reuse_paint(element_state.paint_range.clone());
                        if !window.memo_stack.is_empty() {
                            window
                                .memo_hover_dependencies
                                .extend_from_slice(&element_state.hover_dependencies);
                        }
                    }

                    let paint_end = window.paint_index();
                    element_state.paint_range = paint_start..paint_end;

                    ((), element_state)
                },
            )
        } else {
            element.as_mut().unwrap().paint(window, cx);
        }
    });
}

#[inline(never)]
fn paint_component(
    name: &'static str,
    element: &mut Option<AnyElement>,
    window: &mut Window,
    cx: &mut App,
) {
    window.with_id(ElementId::Name(name.into()), |window| {
        element.as_mut().unwrap().paint(window, cx);
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
}
