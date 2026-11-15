//! Document-bound geometry for UIKit's text interaction callbacks.
//!
//! Hit testing returns insertion offsets, including `len` and zero for an
//! empty document. They are not necessarily offsets of existing characters.
use std::ops::Range;

pub(crate) fn character_range(index: usize, len: usize) -> Range<usize> {
    let start = index.min(len);
    start..start.saturating_add(1).min(len)
}

/// UIKit must be able to anchor an edit menu even with no character to measure.
/// Ask the handler for the insertion caret first rather than an invalid range
/// beyond the end of the document.
pub(crate) fn caret_range(index: usize, len: usize) -> Range<usize> {
    let index = index.min(len);
    index..index
}

#[cfg(test)]
#[path = "tests/text_interaction.rs"]
mod tests;
