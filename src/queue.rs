use crate::track::Track;
use rand::seq::SliceRandom;

/// What a [`Queue::remove`] call actually removed.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Removed {
    /// Index out of range, nothing changed.
    Nothing,
    /// A track that was not playing.
    Other,
    /// The track that was playing; `pos` already points at its successor (if any).
    Current,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Repeat {
    Off,
    All,
    One,
}

/// `items` keeps insertion order; `order` is the play order (a permutation when shuffled);
/// `pos` is the current position inside `order`.
pub struct Queue {
    items: Vec<Track>,
    order: Vec<usize>,
    pos: Option<usize>,
    pub shuffle: bool,
    pub repeat: Repeat,
}

impl Queue {
    pub fn new() -> Self {
        Self {
            items: vec![],
            order: vec![],
            pos: None,
            shuffle: false,
            repeat: Repeat::Off,
        }
    }

    fn rebuild(&mut self, first: Option<usize>) {
        self.order = (0..self.items.len()).collect();
        if self.shuffle {
            self.order.shuffle(&mut rand::thread_rng());
            if let Some(f) = first {
                if let Some(p) = self.order.iter().position(|&x| x == f) {
                    self.order.swap(0, p);
                }
                self.pos = Some(0);
            } else {
                self.pos = None;
            }
        } else {
            self.pos = first;
        }
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn pos(&self) -> Option<usize> {
        self.pos
    }

    pub fn current(&self) -> Option<&Track> {
        self.pos.map(|p| &self.items[self.order[p]])
    }

    /// Replace the queue and start at `start` (index into `items`).
    pub fn set(&mut self, items: Vec<Track>, start: usize) -> Option<&Track> {
        self.items = items;
        let first = if self.items.is_empty() {
            None
        } else {
            Some(start.min(self.items.len() - 1))
        };
        self.rebuild(first);
        self.current()
    }

    /// Append; returns the play-order position of the new entry.
    pub fn push(&mut self, t: Track) -> usize {
        self.items.push(t);
        self.order.push(self.items.len() - 1);
        self.order.len() - 1
    }

    pub fn next(&mut self, auto: bool) -> Option<&Track> {
        let p = self.pos?;
        if auto && self.repeat == Repeat::One {
            return self.current();
        }
        if p + 1 < self.order.len() {
            self.pos = Some(p + 1);
        } else if self.repeat == Repeat::All {
            if self.shuffle {
                self.rebuild(None); // new random lap
            }
            self.pos = Some(0);
        } else {
            return None;
        }
        self.current()
    }

    pub fn prev(&mut self) -> Option<&Track> {
        let p = self.pos?;
        if p > 0 {
            self.pos = Some(p - 1);
        }
        self.current()
    }

    pub fn jump(&mut self, order_pos: usize) -> Option<&Track> {
        if order_pos < self.order.len() {
            self.pos = Some(order_pos);
        }
        self.current()
    }

    /// Remove the entry at play-order position `order_pos`.
    pub fn remove(&mut self, order_pos: usize) -> Removed {
        if order_pos >= self.order.len() {
            return Removed::Nothing;
        }
        let item = self.order[order_pos];
        let cur_item = self.pos.map(|p| self.order[p]);
        let removed_current = cur_item == Some(item);

        self.items.remove(item);
        self.order.remove(order_pos);
        for x in self.order.iter_mut() {
            if *x > item {
                *x -= 1;
            }
        }

        if removed_current {
            // Playback stays on the same play-order slot, which now holds the next track.
            self.pos = (order_pos < self.order.len()).then_some(order_pos);
            if self.pos.is_none() && self.repeat == Repeat::All && !self.order.is_empty() {
                self.pos = Some(0); // last track removed, wrap a shuffled/repeating lap
            }
            return Removed::Current;
        }

        self.pos = cur_item
            .filter(|c| *c != item)
            .map(|c| if c > item { c - 1 } else { c })
            .and_then(|c| self.order.iter().position(|&x| x == c));
        Removed::Other
    }

    pub fn toggle_shuffle(&mut self) {
        let cur = self.pos.map(|p| self.order[p]);
        self.shuffle = !self.shuffle;
        self.rebuild(cur);
    }

    pub fn cycle_repeat(&mut self) {
        self.repeat = match self.repeat {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        };
    }

    pub fn ordered(&self) -> Vec<&Track> {
        self.order.iter().map(|&i| &self.items[i]).collect()
    }

    pub fn to_vec(&self) -> Vec<Track> {
        self.ordered().into_iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::Source;
    use std::path::PathBuf;

    fn t(n: &str) -> Track {
        Track {
            title: n.into(),
            artist: String::new(),
            duration: None,
            album: None,
            thumb: None,
            source: Source::Local(PathBuf::from(n)),
        }
    }

    #[test]
    fn shuffle_keeps_current_first_and_all_tracks() {
        let mut q = Queue::new();
        q.set((0..10).map(|i| t(&i.to_string())).collect(), 4);
        q.toggle_shuffle();
        assert_eq!(q.current().unwrap().title, "4");
        let mut seen: Vec<String> = q.ordered().iter().map(|t| t.title.clone()).collect();
        seen.sort();
        assert_eq!(seen.len(), 10);
    }

    #[test]
    fn repeat_all_wraps_and_off_stops() {
        let mut q = Queue::new();
        q.set(vec![t("a"), t("b")], 0);
        assert_eq!(q.next(true).unwrap().title, "b");
        assert!(q.next(true).is_none());
        q.cycle_repeat();
        assert_eq!(q.next(true).unwrap().title, "a");
    }

    #[test]
    fn remove_keeps_current() {
        let mut q = Queue::new();
        q.set(vec![t("a"), t("b"), t("c")], 2);
        assert_eq!(q.remove(0), Removed::Other);
        assert_eq!(q.current().unwrap().title, "c");
    }

    #[test]
    fn removing_current_advances_to_next() {
        let mut q = Queue::new();
        q.set(vec![t("a"), t("b"), t("c")], 0);
        assert_eq!(q.remove(0), Removed::Current);
        assert_eq!(q.current().unwrap().title, "b");
        assert_eq!(q.len(), 2);

        // Removing the last playing entry empties the queue instead of keeping a stale pos.
        assert_eq!(q.remove(0), Removed::Current);
        assert_eq!(q.current().unwrap().title, "c");
        assert_eq!(q.remove(0), Removed::Current);
        assert!(q.current().is_none());
        assert!(q.next(false).is_none());
    }

    #[test]
    fn removing_current_while_shuffled_stays_on_slot() {
        let mut q = Queue::new();
        q.set((0..5).map(|i| t(&i.to_string())).collect(), 0);
        q.toggle_shuffle();
        let next_title = q.next(false).unwrap().title.clone();
        q.prev();
        assert_eq!(q.remove(0), Removed::Current);
        assert_eq!(q.current().unwrap().title, next_title);
    }

    #[test]
    fn remove_out_of_range_is_a_noop() {
        let mut q = Queue::new();
        q.set(vec![t("a")], 0);
        assert_eq!(q.remove(9), Removed::Nothing);
        assert_eq!(q.current().unwrap().title, "a");
    }
}
