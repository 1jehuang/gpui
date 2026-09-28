//! Subtrees drawn again from what they drew on the last frame.
//!
//! Views, cached views and memos are retained subtrees. Each one drawn in a
//! frame leaves a record there: where its hitboxes, dispatch nodes, listeners
//! and primitives went, what it read while it was built, the hovers it was
//! painted by and the layout nodes it holds. On the next frame, a subtree
//! whose record says nothing it depends on has changed is drawn again by
//! copying those stretches of the last frame instead of building, laying out,
//! prepainting and painting it.
//!
//! The records live in the frame rather than in element state because the
//! stretches they point to belong to one frame. A subtree drawn again from
//! last frame is not visited, so the records of subtrees nested in it would
//! otherwise keep pointing into the frame they were last visited in. Instead,
//! drawing a subtree again copies its record and the records nested in it,
//! shifted to where the copy landed, so that any of them can be drawn again
//! on its own later, when what is around it has to be built.

use super::*;
use crate::fast::dependencies::{DependencyRecording, RenderDependencies};

/// The retained subtrees drawn in one frame, in the order they began
/// prepainting, which puts a subtree's nested subtrees right after it.
#[derive(Default)]
pub(crate) struct RetainedSubtrees {
    records: Vec<RetainedSubtree>,
    by_id: FxHashMap<GlobalElementId, usize>,
    /// The records whose prepaint is under way, innermost last.
    open: Vec<usize>,
    reused_any: bool,
}

struct RetainedSubtree {
    id: GlobalElementId,
    prepaint_range: Range<PrepaintStateIndex>,
    paint_range: Range<PaintIndex>,
    paint: PaintStatus,
    /// How many of the records following this one are nested inside it.
    nested: usize,
    context: Rc<RetainedContext>,
    dependencies: RenderDependencies,
    hover_dependencies: Rc<[(HitboxId, bool)]>,
    /// The layout nodes the subtree claimed while it was prepainted, list
    /// items for instance, kept while it is drawn again so that building it
    /// again finds them.
    layout_keys: Rc<[u64]>,
    layout: Option<Rc<RetainedLayout>>,
}

enum PaintStatus {
    /// Not painted, so `paint_range` means nothing.
    Unpainted,
    /// Painted this frame into `paint_range`. When that was drawn from last
    /// frame, `source` is where it started there, for the records copied
    /// along with it to shift their own ranges by.
    Painted { source: Option<PaintIndex> },
    /// Copied along with the subtree at `anchor` and still holding last
    /// frame's `paint_range`, which is shifted once that one is painted.
    Pending { anchor: usize },
}

/// What a subtree's prepaint and paint depended on besides what it read: the
/// place it was drawn in and what it inherited there.
#[derive(PartialEq)]
struct RetainedContext {
    bounds: Bounds<Pixels>,
    content_mask: ContentMask<Pixels>,
    text_style: TextStyle,
    opacity: f32,
}

/// What it takes to lay a view out as it was laid out last frame without
/// building it: the view is laid out by its content, so its layout is only
/// known from the nodes its content left.
pub(crate) struct RetainedLayout {
    /// The node its content is laid out at.
    root: LayoutId,
    /// Every node its content claimed while its layout was requested.
    keys: Vec<u64>,
    /// The element states its content used while its layout was requested,
    /// kept for as long as it is not built.
    element_states: Vec<(GlobalElementId, TypeId)>,
    text_style: TextStyle,
    rem_size: Pixels,
}

/// A layout request being recorded as a [`RetainedLayout`].
pub(crate) struct RetainedLayoutRecording {
    keys: usize,
    transient: usize,
    element_states: usize,
    dependencies: DependencyRecording,
    text_style: TextStyle,
    rem_size: Pixels,
}

/// A retained subtree being prepainted. See [`Window::begin_retained`].
pub(crate) struct RetainedRecording {
    index: Option<usize>,
    dependencies: DependencyRecording,
    layout_keys: usize,
}

/// A retained subtree being painted. See [`Window::begin_retained_paint`].
pub(crate) struct RetainedPaintRecording {
    index: Option<usize>,
    start: PaintIndex,
    hovers_start: usize,
    dependencies: DependencyRecording,
}

impl PrepaintStateIndex {
    /// This index, taken from a range that started at `from`, as it falls in
    /// a copy of that range starting at `to`.
    fn shifted(&self, from: &Self, to: &Self) -> Self {
        PrepaintStateIndex {
            hitboxes_index: self.hitboxes_index - from.hitboxes_index + to.hitboxes_index,
            tooltips_index: self.tooltips_index - from.tooltips_index + to.tooltips_index,
            deferred_draws_index: self.deferred_draws_index - from.deferred_draws_index
                + to.deferred_draws_index,
            dispatch_tree_index: self.dispatch_tree_index - from.dispatch_tree_index
                + to.dispatch_tree_index,
            accessed_element_states_index: self.accessed_element_states_index
                - from.accessed_element_states_index
                + to.accessed_element_states_index,
            line_layout_index: self
                .line_layout_index
                .shifted(&from.line_layout_index, &to.line_layout_index),
        }
    }
}

impl PaintIndex {
    /// See [`PrepaintStateIndex::shifted`].
    fn shifted(&self, from: &Self, to: &Self) -> Self {
        PaintIndex {
            scene_index: self.scene_index - from.scene_index + to.scene_index,
            window_control_hitboxes_index: self.window_control_hitboxes_index
                - from.window_control_hitboxes_index
                + to.window_control_hitboxes_index,
            mouse_listeners_index: self.mouse_listeners_index - from.mouse_listeners_index
                + to.mouse_listeners_index,
            input_handlers_index: self.input_handlers_index - from.input_handlers_index
                + to.input_handlers_index,
            cursor_styles_index: self.cursor_styles_index - from.cursor_styles_index
                + to.cursor_styles_index,
            accessed_element_states_index: self.accessed_element_states_index
                - from.accessed_element_states_index
                + to.accessed_element_states_index,
            tab_handle_index: self.tab_handle_index - from.tab_handle_index + to.tab_handle_index,
            line_layout_index: self
                .line_layout_index
                .shifted(&from.line_layout_index, &to.line_layout_index),
        }
    }
}

impl RetainedSubtrees {
    pub(crate) fn clear(&mut self) {
        self.records.clear();
        self.by_id.clear();
        self.open.clear();
        self.reused_any = false;
    }

    /// Whether any subtree was drawn from last frame.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn reused_any(&self) -> bool {
        self.reused_any
    }

    /// The painted record `id` left, if any.
    fn find(&self, id: &GlobalElementId) -> Option<usize> {
        let index = *self.by_id.get(id)?;
        matches!(self.records[index].paint, PaintStatus::Painted { .. }).then_some(index)
    }

    pub(crate) fn id(&self, index: usize) -> &GlobalElementId {
        &self.records[index].id
    }

    /// The records being prepainted right now, for something deferred from
    /// them to be counted as theirs.
    pub(crate) fn open_records(&self) -> SmallVec<[usize; 4]> {
        self.open.iter().copied().collect()
    }

    /// Adds what was read while something deferred from `records` was drawn.
    pub(crate) fn add_dependencies(
        &mut self,
        records: &[usize],
        dependencies: &RenderDependencies,
    ) {
        for &index in records {
            let record = &mut self.records[index];
            record.dependencies = record.dependencies.union(dependencies);
        }
    }

    /// Adds the hovers something deferred from `records` was painted by.
    pub(crate) fn add_hover_dependencies(
        &mut self,
        records: &[usize],
        hovers: &[(HitboxId, bool)],
    ) {
        if hovers.is_empty() {
            return;
        }
        for &index in records {
            let record = &mut self.records[index];
            let mut all = record.hover_dependencies.to_vec();
            all.extend_from_slice(hovers);
            record.hover_dependencies = all.into();
        }
    }

    fn push(&mut self, record: RetainedSubtree) -> usize {
        let index = self.records.len();
        // Two subtrees with one id can only both be drawn; neither can be
        // found to be drawn again.
        match self.by_id.entry(record.id.clone()) {
            collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(index);
            }
            collections::hash_map::Entry::Occupied(entry) => {
                let other = *entry.get();
                self.records[other].paint = PaintStatus::Unpainted;
            }
        }
        self.records.push(record);
        index
    }

    /// Shifts the paint ranges of records copied along with a subtree drawn
    /// from last frame, now that it has been painted, and forgets those that
    /// were not.
    pub(crate) fn finish_frame(&mut self) {
        debug_assert!(self.open.is_empty());
        for index in 0..self.records.len() {
            let PaintStatus::Pending { anchor } = self.records[index].paint else {
                continue;
            };
            let shift = match &self.records[anchor].paint {
                PaintStatus::Painted {
                    source: Some(source),
                } if anchor != index => Some((
                    source.clone(),
                    self.records[anchor].paint_range.start.clone(),
                )),
                _ => None,
            };
            let record = &mut self.records[index];
            match shift {
                Some((from, to)) => {
                    record.paint_range = record.paint_range.start.shifted(&from, &to)
                        ..record.paint_range.end.shifted(&from, &to);
                    record.paint = PaintStatus::Painted { source: None };
                }
                None => record.paint = PaintStatus::Unpainted,
            }
        }
    }
}

impl Window {
    /// Sets whether a view is drawn again from what it drew on the last frame
    /// while nothing it depends on has changed, which is the default. A view
    /// depends on itself, on every entity and global read while it was
    /// rendered, laid out, prepainted and painted, on what it inherits from
    /// where it is drawn — bounds, content mask, text style, opacity — and on
    /// the hovers it was painted by. Anything else a view's render reads, it
    /// is notified of, as a cached view is.
    ///
    /// Turning it off draws every view from scratch each frame, as upstream
    /// GPUI does. The `GPUI_VIEW_RETENTION=0` environment variable turns it
    /// off for every window.
    pub fn set_view_retention(&mut self, enabled: bool) {
        if self.view_retention != enabled {
            self.view_retention = enabled;
            self.refresh();
        }
    }

    /// Whether views are drawn again from what they drew on the last frame.
    /// See [`Window::set_view_retention`].
    pub fn view_retention(&self) -> bool {
        self.view_retention
    }

    /// The record `id` left last frame, if nothing about this frame rules out
    /// drawing it again: it did not read anything that changed, is not
    /// hovered differently and was not marked by an interaction.
    ///
    /// Where it is drawn is not checked here; see
    /// [`Window::retained_context_matches`].
    pub(crate) fn reusable_retained(&self, id: &GlobalElementId, cx: &App) -> Option<usize> {
        if self.refreshing
            || cx.has_active_drag()
            || self.a11y.is_active()
            || self.is_inspector_picking(cx)
            || self.dirty_memos.contains(id)
            || self.next_frame.retained.by_id.contains_key(id)
        {
            return None;
        }
        let index = self.rendered_frame.retained.find(id)?;
        let record = &self.rendered_frame.retained.records[index];
        if cx.dependencies_changed(&record.dependencies, &self.notified_entities)
            || !self.hovers_unchanged(&record.hover_dependencies)
        {
            return None;
        }
        Some(index)
    }

    /// Whether the subtree last frame's record `previous` stands for would be
    /// drawn at `bounds` just as it was.
    pub(crate) fn retained_context_matches(&self, previous: usize, bounds: Bounds<Pixels>) -> bool {
        let context = &self.rendered_frame.retained.records[previous].context;
        context.bounds == bounds
            && context.opacity == self.element_opacity
            && context.content_mask == self.content_mask()
            && context.text_style == self.text_style()
    }

    /// Lays out the view last frame's record `previous` stands for as it was
    /// laid out then, without building it, if it can be: its content left
    /// every node it was laid out with, and inherits what it did.
    pub(crate) fn reuse_retained_layout(
        &mut self,
        previous: usize,
        cx: &mut App,
    ) -> Option<LayoutId> {
        let record = &self.rendered_frame.retained.records[previous];
        let layout = record.layout.as_ref()?;
        if layout.rem_size != self.rem_size() || layout.text_style != self.text_style() {
            return None;
        }
        if !self
            .layout_engine
            .as_mut()
            .unwrap()
            .try_keep_retained(&layout.keys)
        {
            return None;
        }
        self.next_frame
            .accessed_element_states
            .extend(layout.element_states.iter().cloned());
        let root = layout.root;
        cx.replay_dependencies(&record.dependencies);
        Some(root)
    }

    /// Gives back the layout nodes [`Window::reuse_retained_layout`] kept, for
    /// a view that has to be built after all.
    pub(crate) fn release_retained_layout(&mut self, previous: usize) {
        if let Some(layout) = self.rendered_frame.retained.records[previous]
            .layout
            .as_ref()
        {
            self.layout_engine
                .as_mut()
                .unwrap()
                .release_kept(&layout.keys);
        }
    }

    /// The node the view last frame's record `previous` stands for was laid
    /// out at.
    pub(crate) fn retained_layout_root(&self, previous: usize) -> Option<LayoutId> {
        self.rendered_frame.retained.records[previous]
            .layout
            .as_ref()
            .map(|layout| layout.root)
    }

    /// Starts recording a view's layout request, to lay it out again as it
    /// was without building it. See [`RetainedLayout`].
    pub(crate) fn begin_retained_layout(&mut self, cx: &mut App) -> RetainedLayoutRecording {
        RetainedLayoutRecording {
            keys: self.record_claimed_layout_keys(),
            transient: self.layout_engine.as_ref().unwrap().transient_count(),
            element_states: self.next_frame.accessed_element_states.len(),
            dependencies: cx.begin_recording_dependencies(),
            text_style: self.text_style(),
            rem_size: self.rem_size(),
        }
    }

    /// Ends `recording` for a layout request that produced `root`, returning
    /// what was read during it and, if its layout can be reused, how.
    pub(crate) fn finish_retained_layout(
        &mut self,
        recording: RetainedLayoutRecording,
        root: LayoutId,
        cx: &mut App,
    ) -> (Option<Rc<RetainedLayout>>, RenderDependencies) {
        let keys = self.finish_recording_claimed_layout_keys(recording.keys);
        let dependencies = cx.finish_recording_dependencies(recording.dependencies);
        // A node nothing retains is gone at the end of the frame.
        if self.layout_engine.as_ref().unwrap().transient_count() != recording.transient {
            return (None, dependencies);
        }
        let layout = RetainedLayout {
            root,
            keys,
            element_states: self.next_frame.accessed_element_states[recording.element_states..]
                .to_vec(),
            text_style: recording.text_style,
            rem_size: recording.rem_size,
        };
        (Some(Rc::new(layout)), dependencies)
    }

    /// Runs `f` as though the element being prepainted were requesting its
    /// layout, so that what `f` lays out is keyed as that element's children
    /// were, and finds the nodes they had.
    pub(crate) fn with_layout_key_of_prepainting_element<R>(
        &mut self,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.layout_key_stack.push(LayoutKeyFrame {
            key: self.layout_prepaint_scope,
            next_unidentified_child: 0,
        });
        let result = f(self);
        self.layout_key_stack.pop();
        result
    }

    /// How many writes that change a layout the engine has made. See
    /// [`TaffyLayoutEngine::layout_changes`].
    pub(crate) fn layout_changes(&self) -> u64 {
        self.layout_engine.as_ref().unwrap().layout_changes()
    }

    /// See [`TaffyLayoutEngine::relayout_in_place`].
    pub(crate) fn relayout_in_place(
        &mut self,
        layout_id: LayoutId,
        available_space: Size<AvailableSpace>,
        cx: &mut App,
    ) {
        let mut layout_engine = self.layout_engine.take().unwrap();
        layout_engine.relayout_in_place(layout_id, available_space, self, cx);
        self.layout_engine = Some(layout_engine);
    }

    /// Draws the subtree last frame's record `previous` stands for again, as
    /// far as its prepaint goes, returning its record in this frame for
    /// [`Window::reuse_retained_paint`]. What it read is read again unless
    /// its layout already was reused, which did that.
    pub(crate) fn reuse_retained_prepaint(
        &mut self,
        previous: usize,
        layout_reused: bool,
        cx: &mut App,
    ) -> usize {
        let (prepaint_range, layout_keys) = {
            let record = &self.rendered_frame.retained.records[previous];
            (record.prepaint_range.clone(), record.layout_keys.clone())
        };
        self.keep_retained_layout(&layout_keys);
        let start = self.prepaint_index();
        self.reuse_prepaint(prepaint_range.clone());
        let end = self.prepaint_index();
        if !layout_reused {
            cx.replay_dependencies(&self.rendered_frame.retained.records[previous].dependencies);
        }

        // The nested records can be shifted into this frame only if the copy
        // is what it was copied from, entry for entry.
        let copied_whole = end == prepaint_range.end.shifted(&prepaint_range.start, &start);
        debug_assert!(copied_whole, "a reused prepaint range changed length");
        let source = &self.rendered_frame.retained;
        let target = &mut self.next_frame.retained;
        target.reused_any = true;
        let anchor = target.records.len();
        let nested = if copied_whole {
            source.records[previous].nested
        } else {
            0
        };
        for index in previous..=previous + nested {
            let record = &source.records[index];
            let paint = match record.paint {
                PaintStatus::Painted { .. } => PaintStatus::Pending { anchor },
                _ => PaintStatus::Unpainted,
            };
            let prepaint_range = if index == previous {
                start.clone()..end.clone()
            } else {
                record
                    .prepaint_range
                    .start
                    .shifted(&prepaint_range.start, &start)
                    ..record
                        .prepaint_range
                        .end
                        .shifted(&prepaint_range.start, &start)
            };
            target.push(RetainedSubtree {
                id: record.id.clone(),
                prepaint_range,
                paint_range: record.paint_range.clone(),
                paint,
                nested: if index == previous {
                    nested
                } else {
                    record.nested
                },
                context: record.context.clone(),
                dependencies: record.dependencies.clone(),
                hover_dependencies: record.hover_dependencies.clone(),
                layout_keys: record.layout_keys.clone(),
                layout: record.layout.clone(),
            });
        }
        anchor
    }

    /// Draws the subtree whose prepaint [`Window::reuse_retained_prepaint`]
    /// drew again as far as its paint goes.
    pub(crate) fn reuse_retained_paint(&mut self, index: usize) {
        let record = &self.next_frame.retained.records[index];
        let PaintStatus::Pending { .. } = record.paint else {
            return;
        };
        let source = record.paint_range.clone();
        let hovers = record.hover_dependencies.clone();
        let id = record.id.clone();

        let start = self.paint_index();
        self.reuse_paint(source.clone());
        let end = self.paint_index();
        let copied_whole = end == source.end.shifted(&source.start, &start);
        debug_assert!(copied_whole, "a reused paint range changed length");
        let record = &mut self.next_frame.retained.records[index];
        record.paint_range = start..end;
        record.paint = PaintStatus::Painted {
            source: copied_whole.then_some(source.start),
        };

        // The hovers were checked against last frame's hitboxes; this
        // frame's could put something over the subtree. That is found out
        // only now, too late to build it again, so it is built on the next
        // frame, which is asked for.
        if !self.hovers_unchanged(&hovers) {
            self.memos_dirty_next_frame
                .extend(self.memo_stack.iter().cloned());
            self.memos_dirty_next_frame.insert(id);
            self.request_animation_frame();
        }
        // Subtrees around this one depend on these hovers too.
        if !self.memo_stack.is_empty() {
            self.memo_hover_dependencies.extend_from_slice(&hovers);
        }
    }

    /// Starts recording the prepaint of the retained subtree `id`, which is
    /// being built. Interactions inside it mark it to be built again, and
    /// whatever it lays out, reads and inherits is recorded.
    pub(crate) fn begin_retained(
        &mut self,
        id: &GlobalElementId,
        cx: &mut App,
    ) -> RetainedRecording {
        let start = self.prepaint_index();
        let retained = &mut self.next_frame.retained;
        let index = (!retained.by_id.contains_key(id)).then(|| {
            let index = retained.push(RetainedSubtree {
                id: id.clone(),
                prepaint_range: start.clone()..start,
                paint_range: PaintIndex::default()..PaintIndex::default(),
                paint: PaintStatus::Unpainted,
                nested: 0,
                context: Rc::new(RetainedContext {
                    bounds: Bounds::default(),
                    content_mask: ContentMask::default(),
                    text_style: TextStyle::default(),
                    opacity: 1.,
                }),
                dependencies: RenderDependencies::default(),
                hover_dependencies: Rc::new([]),
                layout_keys: Rc::new([]),
                layout: None,
            });
            retained.open.push(index);
            index
        });
        self.memo_stack.push(id.clone());
        RetainedRecording {
            index,
            dependencies: cx.begin_recording_dependencies(),
            layout_keys: self.record_claimed_layout_keys(),
        }
    }

    /// Ends `recording` for a subtree prepainted at `bounds`, returning its
    /// record, if it has one, for [`Window::begin_retained_paint`].
    /// `layout` and `layout_dependencies` come from its layout request, when
    /// it was laid out by its content.
    pub(crate) fn finish_retained_prepaint(
        &mut self,
        recording: RetainedRecording,
        bounds: Bounds<Pixels>,
        layout: Option<Rc<RetainedLayout>>,
        layout_dependencies: Option<RenderDependencies>,
        cx: &mut App,
    ) -> Option<usize> {
        let layout_keys = self.finish_recording_claimed_layout_keys(recording.layout_keys);
        let mut dependencies = cx.finish_recording_dependencies(recording.dependencies);
        self.memo_stack.pop();
        let index = recording.index?;
        if let Some(layout_dependencies) = layout_dependencies {
            dependencies = layout_dependencies.union(&dependencies);
        }
        let context = RetainedContext {
            bounds,
            content_mask: self.content_mask(),
            text_style: self.text_style(),
            opacity: self.element_opacity,
        };
        let end = self.prepaint_index();
        let retained = &mut self.next_frame.retained;
        debug_assert_eq!(retained.open.last(), Some(&index));
        retained.open.pop();
        let nested = retained.records.len() - index - 1;
        let record = &mut retained.records[index];
        record.prepaint_range.end = end;
        record.nested = nested;
        record.context = Rc::new(context);
        record.dependencies = dependencies;
        record.layout_keys = layout_keys.into();
        record.layout = layout;
        Some(index)
    }

    /// Starts recording the paint of the retained subtree `id`, whose
    /// prepaint left the record `index`.
    pub(crate) fn begin_retained_paint(
        &mut self,
        index: Option<usize>,
        id: &GlobalElementId,
        cx: &mut App,
    ) -> RetainedPaintRecording {
        self.memo_stack.push(id.clone());
        RetainedPaintRecording {
            index,
            start: self.paint_index(),
            hovers_start: self.memo_hover_dependencies.len(),
            dependencies: cx.begin_recording_dependencies(),
        }
    }

    /// Ends `recording`, keeping what the subtree painted, the hovers it was
    /// painted by and what it read.
    pub(crate) fn finish_retained_paint(
        &mut self,
        recording: RetainedPaintRecording,
        cx: &mut App,
    ) {
        self.memo_stack.pop();
        let dependencies = cx.finish_recording_dependencies(recording.dependencies);
        let Some(index) = recording.index else {
            return;
        };
        let end = self.paint_index();
        let hovers: Rc<[(HitboxId, bool)]> =
            self.memo_hover_dependencies[recording.hovers_start..].into();
        let record = &mut self.next_frame.retained.records[index];
        record.paint_range = recording.start..end;
        record.paint = PaintStatus::Painted { source: None };
        record.hover_dependencies = hovers;
        record.dependencies = record.dependencies.union(&dependencies);
    }
}
