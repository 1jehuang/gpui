# gpui-fast

A fork of Zed's GPUI (`crates/gpui` and the Zed crates it depends on) that we
keep merging upstream changes into. To keep those merges easy:

- **Put gpui-fast's code in `crates/<crate>/src/fast/`**, one file per topic.
  New work goes in a new `crates/gpui/src/fast/<topic>.rs` (or `fast/<topic>/`);
  its tests go in `crates/gpui/src/fast/tests/<topic>.rs`.
- **Upstream files only get small hooks**: one field holding a `fast/` struct,
  one-line calls or forwarding method bodies, `pub(crate)` visibility bumps,
  `mod`/`use` lines, or a `#[path = "fast/<file>.rs"]` redirect to a rewrite.
  No new types, algorithms or tests in upstream files, and no reformatting of
  upstream code. Methods on upstream types can live in `impl` blocks in `fast/`.
- **New files only inside `fast/` or in our own crates** (`crates/gpui_perf`).
- **Run `script/check-upstream` before committing.** It fails when a change to an
  upstream file is more than a hook.

Upstream directories and the commit they came from are in `UPSTREAM`. The full
rules, the check and the upstream sync procedure are in `docs/upstream-sync.md`.
