# gpui-fast

**GPUI with a retained-mode frame, kept in step with upstream.**

[GPUI](https://gpui.rs), the UI framework of the [Zed](https://github.com/zed-industries/zed)
editor, draws in immediate mode: every frame renders every view, builds a
fresh layout tree, lays it out, shapes its text, and paints the whole window
again, even when almost nothing changed. gpui-fast keeps what the last frame
worked out and redoes only what changed since. Applications are written
exactly as for upstream GPUI; they just draw less.

It is a fork that means to stay one: upstream's source is kept as upstream has
it, gpui-fast's code lives beside it, and upstream's changes are merged in as
Zed makes them.

## Retained mode

A frame walks the element tree three times: **build** renders views and asks
for layout, **prepaint** computes layout and places elements, **paint** turns
them into the scene handed to the GPU. Upstream does all three from scratch.
gpui-fast retains each level:

| What is retained | Drawn again from the last frame while |
|---|---|
| **Views** | nothing the view read while rendering changed — the entities it accessed, the globals it read, the list and scroll state it depends on — and it is drawn at the same place. It is then neither rendered, laid out, prepainted nor painted: its last frame's output is replayed. |
| **`memo(id, key, build)` subtrees** | the key is unchanged. Any `PartialEq` value that stands for what the subtree depends on will do; `Version`, `ContentHash` and `AnyMemoKey` are ready-made ones. |
| **Layout nodes** | the element asks for the same style, children and measurement. Taffy's per-node cache survives, so unchanged parts of the tree are not laid out again. Elements keep their nodes by their path from the root, or by `.key(id)` / `ElementId` wherever they move among their siblings. |
| **Shaped text** | its text, font and runs are unchanged. Recoloured text keeps its shaping; text already fitting the width it is offered is not shaped again. |
| **Primitive ordering** | the bounds painted before it are unchanged; only what meets a changed bound is ordered afresh. |
| **Element identity** | the element's path is the one it had last frame. |

Hover, scrolling, bounds, content masks and window refreshes invalidate
exactly what they affect, without the application doing anything. Retention
can be turned off, for comparison or debugging, with
`Window::set_view_retention(false)` or `GPUI_VIEW_RETENTION=0`.

What it is worth, in main-thread CPU per frame on an Apple M4, median of five
runs, against GPUI as extracted:

| Workload | Upstream | gpui-fast |
|---|---|---|
| 2500 live labels in a real window, still | 8.20 ms | 3.64 ms (−56%) |
| 2500 live labels, every cell changing | 8.25 ms | 5.18 ms (−37%) |
| A wide table scrolled back and forth | 7.89 ms | 4.07 ms (−48%) |

The gain follows how little of the window changes. In gpui-kit's DataTable,
a table taking new values 30 times a second, drawing costs 37% less than
before, and 48% less once the table memoizes its rows.

### Correct by construction, checked anyway

A retained frame has to be the frame that drawing from scratch would have
produced. An oracle test drives two windows through the same random history —
one drawing incrementally, one from scratch — and requires every frame to
match, and `gpui_perf --verify` does the same for whole simulated screens.

### Getting more out of it

Nothing has to change for the gains above. Three things take them further;
[`docs/performance-guide.md`](docs/performance-guide.md) covers each:

- **Key list items by their data**, so a row keeps its layout when something
  is inserted ahead of it. `.key()` works on anything, components included,
  and adds no box to the layout:

  ```rust
  .children(rows.iter().map(|row| render_row(row).key(row.id)))
  ```

  Inserting at the head of a 200-row list goes from 7.43 ms to 1.97 ms.

- **`memo`** a subtree whose inputs you can name cheaply.
- **Split large views** into smaller ones, so a change re-renders only the
  view that read it.

## Staying in step with upstream

gpui-fast tracks Zed's `crates/gpui` and the Zed crates it depends on. The
upstream commit it is based on is recorded in [`UPSTREAM`](UPSTREAM); the
source layout and crate paths are upstream's, so any file compares path for
path with a Zed checkout.

To keep merging upstream cheap, gpui-fast's changes never spread through
upstream's files:

- **All of gpui-fast's logic lives in `crates/<crate>/src/fast/`**, one file per
  topic — `fast/retained.rs`, `fast/layout.rs`, `fast/text.rs`, … — and is
  always referred to by its path, `crate::fast::retained::…`, so it is obvious
  where code comes from.
- **Upstream files hold only hooks**: a field holding a `fast` struct, a
  one-line call into `fast`, a visibility bump. No algorithms, no new types,
  no tests, no reformatting.
- **`script/check-upstream` enforces it**, comparing every upstream file with
  upstream's own copy and failing on anything more than a hook:

  ```sh
  script/check-upstream                      # against upstream as imported
  script/check-upstream --zed ~/github/zed   # against a Zed checkout
  ```

[`docs/upstream-sync.md`](docs/upstream-sync.md) has the rules in full and the
procedure for taking a new upstream commit.

## Using it

The public API is upstream's. Two things differ, and code using neither
compiles here untouched:

- `GlobalElementId` does not implement `DerefMut`: it keeps its path's hash, so
  a path changed in place would no longer match it.
- `Window::with_inspector_state` returns `Option<R>` and calls its closure only
  for the element being inspected, as upstream's does since.

Point a project at it in place of upstream GPUI:

```toml
[dependencies]
gpui = { git = "https://github.com/longbridge/gpui-fast" }
```

## Working on it

```sh
cargo run -p gpui --example hello_world
cargo test -p gpui --features test-support
script/check-upstream
```

Measuring:

```sh
# Simulated forms, lists, tables and settings screens, retained and not
cargo run -p gpui_perf --release
# 2500 labels in a real window, 25% of them changing every frame
cargo run -p gpui_perf --example grid_frames --release -- 50 50 25
# A list scrolled back and forth
cargo run -p gpui_perf --example scroll_frames --release -- uniform oscillate 12 index
```

`Window::layout_stats()` reports where a frame's time went — build, prepaint,
layout, paint, text shaping — and how many layout nodes were reused.
[`docs/frame-budget.html`](docs/frame-budget.html) is the measurement behind
every figure above, step by step.

```
crates/gpui                 the framework; gpui-fast's code is in src/fast/
crates/gpui_platform        platform backend dispatch
crates/gpui_{linux,macos,windows,web,apple,wgpu}
                            per-platform backends and renderers
crates/{collections,util,sum_tree,scheduler,refineable,...}
                            supporting crates from Zed
crates/gpui_perf            gpui-fast's benchmarks (not upstream)
tooling/perf                test-perf harness from Zed
```

## License

Apache-2.0, as upstream — copyright Zed Industries, Inc. See `LICENSE-APACHE`.
This is a modified fork; the changes are the commits after `11a44c4`, the
import of upstream.
