//! A keyboard request is not readiness: the focused handler is installed by paint.
//! Hold events until that frame has returned, never inside a GPUI draw callback.
use std::collections::VecDeque;

const MAX_PENDING_EVENTS: usize = 256;

pub(crate) enum Route<T> {
    Now(T),
    Deferred,
    Overflow,
}

pub(crate) struct InputReadiness<T> {
    requested: bool,
    generation: u64,
    pending_frame: bool,
    events: VecDeque<T>,
}

impl<T> Default for InputReadiness<T> {
    fn default() -> Self {
        Self {
            requested: false,
            generation: 0,
            pending_frame: false,
            events: VecDeque::new(),
        }
    }
}

impl<T> InputReadiness<T> {
    pub(crate) fn request_keyboard(&mut self) {
        // A new explicit user gesture may target a different field. Never carry
        // the earlier target's buffered edits into the newly requested focus.
        self.events.clear();
        self.generation = self.generation.wrapping_add(1);
        self.requested = true;
        self.pending_frame = true;
    }

    pub(crate) fn focus_gained(&mut self) {
        // GPUI emits this after painting the handler for the same gesture.
        // Unlike a new gesture, it must retain that transition's queued prefix.
        self.requested = true;
        self.pending_frame = true;
    }

    pub(crate) fn keyboard_requested(&self) -> bool {
        self.requested
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub(crate) fn pending_frame(&self) -> bool {
        self.pending_frame
    }

    pub(crate) fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.requested = false;
        self.pending_frame = false;
        self.events.clear();
    }

    pub(crate) fn route(&mut self, event: T) -> Route<T> {
        if !self.pending_frame {
            return Route::Now(event);
        }
        if self.events.len() == MAX_PENDING_EVENTS {
            // Fail closed rather than replaying a truncated edit stream into a
            // later field. A new user request starts a fresh transition.
            self.cancel();
            return Route::Overflow;
        }
        self.events.push_back(event);
        Route::Deferred
    }

    pub(crate) fn finish_frame(&mut self, has_focused_handler: bool) -> Option<Vec<T>> {
        if !self.pending_frame {
            return None;
        }
        if !has_focused_handler {
            self.cancel();
            return None;
        }
        self.pending_frame = false;
        Some(self.events.drain(..).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_prefix_waits_for_painted_handler_and_replays_every_event_in_order() {
        let mut state = InputReadiness::default();
        state.request_keyboard();
        let mut delivered = String::new();
        for key in "paritydraft sample".chars() {
            assert!(matches!(state.route(key), Route::Deferred));
        }
        assert!(
            delivered.is_empty(),
            "nothing may hit an absent/stale handler"
        );
        let replay = state
            .finish_frame(true)
            .expect("paint installed focused handler");
        delivered.extend(replay);
        assert_eq!(delivered, "paritydraft sample");
        assert!(state.finish_frame(true).is_none(), "never replay twice");
        assert!(matches!(state.route('!'), Route::Now('!')));
    }

    #[test]
    fn canceled_focus_never_leaks_prefix_into_another_field() {
        let mut state = InputReadiness::default();
        state.request_keyboard();
        assert!(matches!(state.route("old field"), Route::Deferred));
        state.cancel(); // hide, lost OS focus, or surface suspension
        state.request_keyboard();
        assert!(matches!(state.route("new field"), Route::Deferred));
        assert_eq!(state.finish_frame(true).unwrap(), ["new field"]);
    }

    #[test]
    fn no_handler_after_frame_cancels_instead_of_retaining_stale_edits() {
        let mut state = InputReadiness::default();
        state.request_keyboard();
        assert!(matches!(state.route("obsolete"), Route::Deferred));
        assert!(state.finish_frame(false).is_none());
        assert!(!state.keyboard_requested());
        state.request_keyboard();
        assert!(state.finish_frame(true).unwrap().is_empty());
    }

    #[test]
    fn repeated_requests_coalesce_without_losing_prefix() {
        let mut state = InputReadiness::default();
        state.request_keyboard();
        assert!(matches!(state.route('p'), Route::Deferred));
        state.focus_gained(); // post-paint FocusGained for the same gesture
        assert!(matches!(state.route('a'), Route::Deferred));
        assert_eq!(state.finish_frame(true).unwrap(), ['p', 'a']);
    }

    #[test]
    fn pending_queue_is_bounded_and_overflow_cancels_the_whole_transition() {
        let mut state = InputReadiness::default();
        state.request_keyboard();
        for index in 0..MAX_PENDING_EVENTS {
            assert!(matches!(state.route(index), Route::Deferred));
        }
        assert!(matches!(state.route(MAX_PENDING_EVENTS), Route::Overflow));
        assert!(!state.keyboard_requested());
        assert!(state.finish_frame(true).is_none());
        state.request_keyboard();
        assert!(state.finish_frame(true).unwrap().is_empty());
    }

    #[test]
    fn a_new_gesture_cannot_replay_the_previous_fields_buffered_prefix() {
        let mut state = InputReadiness::default();
        state.request_keyboard();
        assert!(matches!(state.route("old field"), Route::Deferred));
        state.request_keyboard();
        assert!(matches!(state.route("new field"), Route::Deferred));
        state.focus_gained();
        assert_eq!(state.finish_frame(true).unwrap(), ["new field"]);
    }

    #[test]
    fn replay_can_detect_cancel_or_new_focus_without_sending_the_tail_to_a_new_field() {
        let mut state = InputReadiness::default();
        state.request_keyboard();
        state.route("enter");
        state.route("old tail");
        let generation = state.generation();
        let events = state.finish_frame(true).unwrap();
        assert_eq!(events, ["enter", "old tail"]);
        state.cancel(); // processing Enter closes the first field
        state.request_keyboard(); // or focuses a different field
        assert_ne!(
            generation,
            state.generation(),
            "caller must stop replaying the old tail"
        );
        assert!(state.pending_frame());
    }

    #[test]
    fn already_focused_keyboard_can_be_requested_again_without_new_input() {
        let mut state = InputReadiness::<()>::default();
        state.request_keyboard();
        assert_eq!(state.finish_frame(true), Some(vec![]));
        state.cancel();
        state.request_keyboard();
        assert_eq!(state.finish_frame(true), Some(vec![]));
    }
}
