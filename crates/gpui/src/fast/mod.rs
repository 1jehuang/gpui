//! Everything gpui-fast adds to GPUI lives under this module.
//!
//! The rest of this crate is kept as close to upstream GPUI (Zed's
//! `crates/gpui`) as possible, so that pulling in upstream changes stays a
//! matter of merging rather than of rewriting. Upstream files only *call into*
//! this module: a field holding this module's state, a line forwarding a method
//! to it, a hook at the point something happens. The logic itself — data
//! structures, algorithms, bookkeeping, tests — is written here, one file per
//! topic. See `docs/upstream-sync.md` for the rules and how they are checked.
//!
//! Every submodule's items are re-exported here, and from here at the crate
//! root, so that they keep the paths they would have had upstream.

pub(crate) mod dependencies;
pub(crate) mod global_id;
pub(crate) mod interactivity;
pub(crate) mod keyed;
pub(crate) mod layout;
pub(crate) mod layout_key;
pub(crate) mod memo;
pub(crate) mod retained;
pub(crate) mod scene;
pub(crate) mod stats;
pub(crate) mod text;

#[cfg(test)]
mod tests;
