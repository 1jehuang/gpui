//! Where a reused range of shaped lines falls in a new frame, and shaping statistics.

use crate::{FontRun, LineLayout, LineLayoutIndex, Pixels, PlatformTextSystem, WindowTextSystem};
use scheduler::Instant;
use std::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

impl LineLayoutIndex {
    /// This index, taken from a range that started at `from`, as it falls in
    /// a copy of that range starting at `to`.
    pub(crate) fn shifted(&self, from: &Self, to: &Self) -> Self {
        LineLayoutIndex {
            lines_index: self.lines_index - from.lines_index + to.lines_index,
            wrapped_lines_index: self.wrapped_lines_index - from.wrapped_lines_index
                + to.wrapped_lines_index,
            lines_by_hash_index: self.lines_by_hash_index - from.lines_by_hash_index
                + to.lines_by_hash_index,
            wrapped_lines_by_hash_index: self.wrapped_lines_by_hash_index
                - from.wrapped_lines_by_hash_index
                + to.wrapped_lines_by_hash_index,
        }
    }
}

/// Counts the lines the line layout cache hands to the platform to be shaped,
/// because neither this frame nor the last one had them, and times them.
#[derive(Default)]
pub(crate) struct LineShaping {
    /// Lines handed to the platform to be shaped. See [`LineShaping::stats`].
    lines_shaped: AtomicU64,
    /// Time spent in those calls, in nanoseconds.
    shape_nanos: AtomicU64,
    /// Whether to time shaping, which it does once the stats have been reset.
    shape_timed: AtomicBool,
}

impl LineShaping {
    /// How many lines have been shaped, and how long that took, since the last
    /// [`LineShaping::reset`]. A line answered from the cache is not counted,
    /// so this is the text work the cache failed to save.
    pub(crate) fn stats(&self) -> (u64, Duration) {
        (
            self.lines_shaped.load(Ordering::Relaxed),
            Duration::from_nanos(self.shape_nanos.load(Ordering::Relaxed)),
        )
    }

    /// Zeroes the counters reported by [`LineShaping::stats`], and from then
    /// on times shaping too.
    pub(crate) fn reset(&self) {
        self.lines_shaped.store(0, Ordering::Relaxed);
        self.shape_nanos.store(0, Ordering::Relaxed);
        self.shape_timed.store(true, Ordering::Relaxed);
    }

    /// Shapes a line the cache does not have, counting it.
    pub(crate) fn shape_line(
        &self,
        platform_text_system: &dyn PlatformTextSystem,
        text: &str,
        font_size: Pixels,
        runs: &[FontRun],
    ) -> LineLayout {
        let started_at = self.shape_timed.load(Ordering::Relaxed).then(Instant::now);
        let layout = platform_text_system.layout_line(text, font_size, runs);
        self.lines_shaped.fetch_add(1, Ordering::Relaxed);
        if let Some(started_at) = started_at {
            self.shape_nanos
                .fetch_add(started_at.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        layout
    }
}

impl WindowTextSystem {
    /// Lines shaped by the platform, and the time that took, since the last
    /// [`Self::reset_shaping_stats`]. Lines answered from the cache do not count.
    pub(crate) fn shaping_stats(&self) -> (u64, Duration) {
        self.line_layout_cache.shaping.stats()
    }

    /// Zeroes the counters reported by [`Self::shaping_stats`].
    pub(crate) fn reset_shaping_stats(&self) {
        self.line_layout_cache.shaping.reset()
    }
}
