//! Translate reusable Wayland contact slots into lifetime-unique GPUI touches.
use collections::HashMap;
use gpui::{Pixels, Point, TouchEvent, TouchId, TouchPhase};

pub(super) struct Touches<T> {
    next_id: u64,
    contacts: HashMap<i32, (T, TouchEvent)>,
}

impl<T> Default for Touches<T> {
    fn default() -> Self {
        // Keep native contacts away from GPUI's u64::MAX mouse-emulation ID.
        Self {
            next_id: 1,
            contacts: HashMap::default(),
        }
    }
}

impl<T: Clone> Touches<T> {
    pub fn down(&mut self, slot: i32, target: T, position: Point<Pixels>) -> Vec<(T, TouchEvent)> {
        let mut events = Vec::new();
        // Defensive recovery if a compositor reuses an active slot.
        if let Some((target, mut event)) = self.contacts.remove(&slot) {
            event.phase = TouchPhase::Cancelled;
            events.push((target, event));
        }
        let event = TouchEvent {
            id: TouchId(self.next_id),
            phase: TouchPhase::Started,
            position,
            ..Default::default()
        };
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("touch identifier exhausted");
        self.contacts.insert(slot, (target.clone(), event.clone()));
        events.push((target, event));
        events
    }

    pub fn motion(&mut self, slot: i32, position: Point<Pixels>) -> Option<(T, TouchEvent)> {
        let (target, event) = self.contacts.get_mut(&slot)?;
        event.phase = TouchPhase::Moved;
        event.position = position;
        Some((target.clone(), event.clone()))
    }

    pub fn up(&mut self, slot: i32) -> Option<(T, TouchEvent)> {
        let (target, mut event) = self.contacts.remove(&slot)?;
        event.phase = TouchPhase::Ended;
        Some((target, event))
    }

    pub fn cancel(&mut self) -> Vec<(T, TouchEvent)> {
        self.contacts
            .drain()
            .map(|(_, (target, mut event))| {
                event.phase = TouchPhase::Cancelled;
                (target, event)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px};

    #[test]
    fn slots_keep_their_surface_and_last_position() {
        let mut touches = Touches::default();
        let a = touches.down(4, "first", point(px(10.), px(20.)))[0].1.id;
        let b = touches.down(8, "second", point(px(30.), px(40.)))[0].1.id;
        assert_ne!(a, b);
        assert_ne!(a, TouchId(0));
        let (target, event) = touches.motion(4, point(px(50.), px(60.))).unwrap();
        assert_eq!(target, "first");
        assert_eq!(event.id, a);
        assert_eq!(event.phase, TouchPhase::Moved);
        let (_, event) = touches.up(4).unwrap();
        assert_eq!(event.position, point(px(50.), px(60.)));
        assert_eq!(event.phase, TouchPhase::Ended);
        assert!(touches.motion(4, Point::default()).is_none());
        assert_eq!(touches.up(8).unwrap().0, "second");
    }

    #[test]
    fn reused_slots_get_new_ids() {
        let mut touches = Touches::default();
        let first = touches.down(0, (), Point::default())[0].1.id;
        touches.up(0).unwrap();
        let second = touches.down(0, (), Point::default())[0].1.id;
        assert_ne!(first, second);
    }

    #[test]
    fn cancellation_unwinds_all_contacts_without_releases() {
        let mut touches = Touches::default();
        touches.down(0, (), Point::default());
        touches.down(1, (), Point::default());
        let events = touches.cancel();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|(_, e)| e.phase == TouchPhase::Cancelled));
        assert!(touches.up(0).is_none());
        assert!(touches.up(1).is_none());
        assert!(touches.cancel().is_empty());
    }

    #[test]
    fn duplicate_down_cancels_old_contact_before_starting_new_one() {
        let mut touches = Touches::default();
        let old = touches.down(2, "old", Point::default())[0].1.id;
        let events = touches.down(2, "new", Point::default());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, "old");
        assert_eq!(events[0].1.id, old);
        assert_eq!(events[0].1.phase, TouchPhase::Cancelled);
        assert_eq!(events[1].1.phase, TouchPhase::Started);
        assert_ne!(events[1].1.id, old);
    }
}
