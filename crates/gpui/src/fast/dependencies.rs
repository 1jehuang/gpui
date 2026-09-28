//! What a retained subtree read while it was built — entities, globals and versioned state — and how the app records it.

use std::{
    any::TypeId,
    cell::{Cell, RefCell},
    rc::Rc,
};

use collections::{FxHashSet, TypeIdHashMap};

use crate::{App, EntityId, EntityMap, ListOffset};

/// The app's side of recording what retained subtrees read: when each global
/// last changed, and what was read while a recording is open.
#[derive(Default)]
pub(crate) struct AppDependencies {
    /// Counts the changes to globals, each of which is stamped into
    /// `global_changed_at`, so a retained subtree can tell whether a global
    /// it read has changed since it was built.
    global_generation: u64,
    global_changed_at: TypeIdHashMap<u64>,
    /// Every global read while a recording is open. See
    /// [`App::begin_recording_dependencies`].
    global_read_log: RefCell<Vec<TypeId>>,
    /// Every [`StateVersion`] read while a recording is open, with the
    /// version it was at.
    state_read_log: RefCell<Vec<(StateVersion, u64)>>,
}

impl AppDependencies {
    /// Stamps a change to the global of type `global_type`.
    pub(crate) fn global_changed(&mut self, global_type: TypeId) {
        // Stamped on every change, not only the first one an effect is queued
        // for: a subtree built in between has seen only the first.
        self.global_generation += 1;
        self.global_changed_at
            .insert(global_type, self.global_generation);
    }
}

impl App {
    /// Starts recording what is read from here on — the entities accessed and
    /// the globals read — for a subtree that is drawn again from what it drew
    /// while none of it changes. Recordings nest; each sees everything read
    /// while it is open, including what nested ones saw.
    pub(crate) fn begin_recording_dependencies(&mut self) -> DependencyRecording {
        DependencyRecording {
            entities: self.entities.begin_recording(),
            globals: self.dependencies.global_read_log.get_mut().len(),
            states: self.dependencies.state_read_log.get_mut().len(),
            generation: self.dependencies.global_generation,
        }
    }

    /// Ends `recording`, returning what was read while it was open.
    pub(crate) fn finish_recording_dependencies(
        &mut self,
        recording: DependencyRecording,
    ) -> RenderDependencies {
        let log = &mut self.dependencies;
        let mut globals = log.global_read_log.get_mut()[recording.globals..].to_vec();
        let states = dedup_states(&log.state_read_log.get_mut()[recording.states..]);
        let entities = self.entities.finish_recording(recording.entities);
        if !self.entities.is_recording() {
            self.dependencies.global_read_log.get_mut().clear();
            self.dependencies.state_read_log.get_mut().clear();
        }
        globals.sort_unstable();
        globals.dedup();
        RenderDependencies {
            entities: entities.into(),
            globals: globals.into(),
            states,
            // As of when the recording began, so that a global written while
            // it was open, after being read, counts as changed.
            generation: recording.generation,
        }
    }

    /// Tells the window, and any recording that is open, that `dependencies`
    /// were read again, as they are when a subtree built from them is reused.
    pub(crate) fn replay_dependencies(&mut self, dependencies: &RenderDependencies) {
        self.entities.extend_accessed(dependencies.entities.iter());
        if self.entities.is_recording() {
            self.dependencies
                .global_read_log
                .get_mut()
                .extend(dependencies.globals.iter().copied());
            self.dependencies
                .state_read_log
                .get_mut()
                .extend(dependencies.states.iter().cloned());
        }
    }

    /// Records, for any recording that is open, that the state `version`
    /// belongs to was read as it is now.
    #[inline]
    pub(crate) fn note_state_read(&self, version: &StateVersion) {
        if self.entities.is_recording() {
            self.dependencies
                .state_read_log
                .borrow_mut()
                .push((version.clone(), version.get()));
        }
    }

    /// Whether anything in `dependencies` may have changed since they were
    /// recorded: one of the entities is among `notified`, or one of the
    /// globals has been written.
    pub(crate) fn dependencies_changed(
        &self,
        dependencies: &RenderDependencies,
        notified: &FxHashSet<EntityId>,
    ) -> bool {
        (!notified.is_empty()
            && dependencies
                .entities
                .iter()
                .any(|entity| notified.contains(entity)))
            || dependencies.globals.iter().any(|global| {
                self.dependencies
                    .global_changed_at
                    .get(global)
                    .is_some_and(|changed_at| *changed_at > dependencies.generation)
            })
            || dependencies
                .states
                .iter()
                .any(|(version, read_at)| version.get() != *read_at)
    }

    /// Records, for any recording that is open, that the global of type
    /// `global` was read.
    #[inline]
    pub(crate) fn note_global_read(&self, global: TypeId) {
        if self.entities.is_recording() {
            self.dependencies.global_read_log.borrow_mut().push(global);
        }
    }
}

/// The entity map's side of recording what retained subtrees read.
#[derive(Default)]
pub(crate) struct EntityAccessLog {
    /// Every entity accessed while a recording is open, in order and with
    /// repeats, for a retained subtree to learn what it was built from. See
    /// [`App::begin_recording_dependencies`].
    access_log: RefCell<Vec<EntityId>>,
    /// How many recordings are open.
    recordings: Cell<usize>,
}

impl EntityMap {
    /// Records, for any recording that is open, that `entity_id` was
    /// accessed.
    #[inline]
    pub(crate) fn note_access(&self, entity_id: EntityId) {
        if self.access_log.recordings.get() > 0 {
            self.access_log.access_log.borrow_mut().push(entity_id);
        }
    }

    pub fn extend_accessed<'a>(&mut self, entities: impl IntoIterator<Item = &'a EntityId>) {
        let accessed_entities = self.accessed_entities.get_mut();
        let recording = self.access_log.recordings.get() > 0;
        for entity_id in entities {
            accessed_entities.insert(*entity_id);
            if recording {
                self.access_log.access_log.get_mut().push(*entity_id);
            }
        }
    }

    /// Whether any recording is open.
    #[inline]
    pub(crate) fn is_recording(&self) -> bool {
        self.access_log.recordings.get() > 0
    }

    /// Opens a recording, returning where in the access log it starts.
    pub(crate) fn begin_recording(&mut self) -> usize {
        let log = &mut self.access_log;
        log.recordings.set(log.recordings.get() + 1);
        log.access_log.get_mut().len()
    }

    /// Closes the recording that started at `start`, returning the entities it
    /// saw, sorted and without repeats.
    pub(crate) fn finish_recording(&mut self, start: usize) -> Vec<EntityId> {
        let EntityAccessLog {
            access_log,
            recordings,
        } = &mut self.access_log;
        let log = access_log.get_mut();
        let mut entities = log[start..].to_vec();
        let open = recordings.get() - 1;
        recordings.set(open);
        if open == 0 {
            log.clear();
        }
        entities.sort_unstable();
        entities.dedup();
        entities
    }
}

/// Where a recording started by [`App::begin_recording_dependencies`] begins.
#[derive(Clone, Copy)]
pub(crate) struct DependencyRecording {
    entities: usize,
    globals: usize,
    states: usize,
    generation: u64,
}

/// What a retained subtree read while it was built: the entities it accessed
/// and the globals it read, as of a global generation. While none of them has
/// changed, building the subtree again would build the same thing.
#[derive(Clone, Default)]
pub(crate) struct RenderDependencies {
    pub(crate) entities: Rc<[EntityId]>,
    pub(crate) globals: Rc<[TypeId]>,
    /// Element state kept outside of entities — scroll handles, list states
    /// — with the version each was read at.
    pub(crate) states: Rc<[(StateVersion, u64)]>,
    pub(crate) generation: u64,
}

/// A counter that state shared outside of entities — a scroll handle, a list
/// state — increments whenever it changes, so that a retained subtree that
/// read it is built again, as it would be for an entity that was notified.
#[derive(Clone, Default, Debug)]
pub(crate) struct StateVersion(Rc<Cell<u64>>);

impl StateVersion {
    pub(crate) fn get(&self) -> u64 {
        self.0.get()
    }

    /// Marks the state as changed.
    pub(crate) fn bump(&self) {
        self.0.set(self.0.get().wrapping_add(1));
    }

    /// Marks the state as changed if `changed`, for a change that may leave
    /// it as it was.
    #[inline]
    pub(crate) fn bump_if(&self, changed: bool) {
        if changed {
            self.bump();
        }
    }

    fn ptr(&self) -> *const Cell<u64> {
        Rc::as_ptr(&self.0)
    }
}

impl ListOffset {
    /// Whether scrolling a list scrolled to `current`, with `pending_scroll`,
    /// to this offset changes where it is scrolled to.
    ///
    /// Scrolling to where it already is, as a view that scrolls its list
    /// while rendering does every frame, changes nothing.
    pub(crate) fn moves_from<P>(
        &self,
        current: Option<ListOffset>,
        pending_scroll: &Option<P>,
    ) -> bool {
        let unchanged = current.is_some_and(|current| {
            current.item_ix == self.item_ix && current.offset_in_item == self.offset_in_item
        }) && pending_scroll.is_none();
        !unchanged
    }
}

/// `states` once each, at the earliest version read, so that a change in
/// between still counts.
fn dedup_states(states: &[(StateVersion, u64)]) -> Rc<[(StateVersion, u64)]> {
    if states.is_empty() {
        return Rc::new([]);
    }
    let mut seen = FxHashSet::default();
    states
        .iter()
        .filter(|(version, _)| seen.insert(version.ptr()))
        .cloned()
        .collect()
}

impl RenderDependencies {
    /// Both sets of dependencies at once, as of the earlier generation, so
    /// that a change either would have seen is still seen.
    pub(crate) fn union(&self, other: &Self) -> Self {
        if other.entities.is_empty() && other.globals.is_empty() && other.states.is_empty() {
            return self.clone();
        }
        let mut entities = self.entities.to_vec();
        entities.extend_from_slice(&other.entities);
        entities.sort_unstable();
        entities.dedup();
        let mut globals = self.globals.to_vec();
        globals.extend_from_slice(&other.globals);
        globals.sort_unstable();
        globals.dedup();
        let mut states = self.states.to_vec();
        states.extend_from_slice(&other.states);
        Self {
            entities: entities.into(),
            globals: globals.into(),
            states: dedup_states(&states),
            generation: self.generation.min(other.generation),
        }
    }
}
