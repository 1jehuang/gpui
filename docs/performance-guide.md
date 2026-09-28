# Performance guide

What gpui-fast changes relative to its baseline, and how an application uses
those changes. The baseline is gpui as extracted from Zed, commit `11a44c4`
(upstream `zed-industries/zed` at `7960b2a`); everything described here is a
commit after it. The measurements behind every figure are in
[`frame-budget.html`](frame-budget.html).

- [Changes that need nothing from the application](#changes-that-need-nothing-from-the-application)
- [API changes](#api-changes)
- [Using the new features](#using-the-new-features)
  1. [`.key()`: identity for list items](#1-key-identity-for-list-items)
  2. [`memo`: skipping a subtree whose key is unchanged](#2-memo-skipping-a-subtree-whose-key-is-unchanged)
  3. [Cached views](#3-cached-views)
  4. [What the retained frame rewards](#4-what-the-retained-frame-rewards)
- [Measuring](#measuring)

## Changes that need nothing from the application

A frame walks the element tree three times: **build** renders every view and
asks for layout, **prepaint** computes layout and places elements, **paint**
turns them into the scene handed to the GPU. The baseline does all three from
scratch every frame. gpui-fast keeps what the last frame worked out, and redoes
only what changed.

| Area | Baseline | gpui-fast |
|---|---|---|
| Layout nodes | Clears the whole Taffy tree at the end of every frame, so Taffy's layout cache is never used | Keeps nodes between frames, keyed by the element's path from the root, and releases a node the first frame it goes unclaimed |
| Layout writes | Writes every node's style, children and measurement every frame | Writes them only when they differ from last frame's, so unchanged nodes keep Taffy's cached layout; a text node keeps the measurement it has |
| List items | Nothing is kept, so an item has no layout to find again | An item with an `ElementId` keeps its nodes wherever it moves; an item without one in a `uniform_list` or `list` is matched by its index, so rows still in view keep their layout while the list scrolls |
| Text shaping | Reshapes text whose colour changed, and text measured again at a width it already fits | Recolours shaped text without reshaping it; leaves text that fits alone |
| Line cache | Keeps a line only for the frame after it was last used | Also keeps a line while a retained node holds it, so text that slides onto another row's node is not shaped again |
| Truncation | Every measurement resolves a font and borrows a line wrapper in case it truncates | Only text that truncates takes a line wrapper |
| Fonts (macOS) | Makes a native font for every line | Keeps one per size, so CoreText's shaping caches outlive the line |
| Scene | Copies every primitive for each paint operation | Records where the primitive went |
| Primitive ordering | A bounds tree that never splits a full node, nesting dozens of levels deep | Kept balanced; last frame's orderings are replayed for everything that did not move, and only what meets a changed bound is ordered afresh |
| Element identity | Copies and hashes the whole element id stack several times per element | Hashes a path once, and gives an element the global id it had last frame |
| Element size | A `div` is 1360 bytes, copied through every builder call | Listener lists and accessibility stay out of line until used: 752 bytes |
| Inspector | Builds inspector ids and state for every element in debug builds or with the `inspector` feature | Only while the inspector is open |

Main-thread CPU per frame on an Apple M4, median of five runs:

| Workload | Baseline | gpui-fast |
|---|---|---|
| 2500-cell grid, still | 8.20 ms | 3.64 ms (−56%) |
| 2500-cell grid, every cell changing | 8.25 ms | 5.18 ms (−37%) |
| Wide table scrolled back and forth | 7.89 ms | 4.07 ms (−48%) |

The gain follows how much of the window changes: a still frame gains most,
and a frame where everything changes still gains a third.

## API changes

Two changes can break code written against the baseline:

- `GlobalElementId` no longer implements `DerefMut`. It keeps its path's hash,
  so a path changed in place would no longer match it.
- `Window::with_inspector_state` returns `Option<R>` and calls its closure only
  for the element being inspected.

Additions:

| Addition | Purpose |
|---|---|
| `IntoElement::key(id) -> Keyed` | Identity among siblings for any element, without a layout box |
| `memo(id, key, build) -> Memo<K>` | A subtree drawn again from last frame while its key is equal |
| `Version`, `ContentHash`, `AnyMemoKey` | Ready-made memo keys |
| `Window::layout_stats()`, `Window::reset_layout_stats()`, `LayoutStats` | Counters and timings for a frame's layout, shaping and phases |

Changed behaviour with an unchanged signature: `Entity::cached` and
`AnyView::cached` keep their layout nodes while reused and follow hover, see
[Cached views](#3-cached-views).

## Using the new features

### 1. `.key()`: identity for list items

Retained layout nodes follow an element's path from the root. Each step of
that path is the element's id, or, when it has none, its position among the
siblings that have none either. When a row is inserted at the top of an
unkeyed list, every row below it is at a new path and has its layout rebuilt.

Key each item by the data it shows, not by its index:

```rust
.children(rows.iter().map(|row| render_row(row).key(row.id)))
```

- `.key()` works on anything, components included, and adds nothing to the
  layout. `div().id(row.id)` works too, on an element that takes an id.
- The key has to be on the item itself, the element placed directly in the
  list. A component built with `RenderOnce` reports no id of its own, whatever
  the element it renders into has, so a list of components is matched by
  position unless each one is keyed.
- A key also scopes element state, so a row's state follows the row rather
  than the slot it is drawn in.
- Inserting at the head of a list, unkeyed against keyed: 200 rows,
  7.43 ms → 1.97 ms; 800 rows, 33.95 ms → 9.98 ms.

A `uniform_list` or `list` that only scrolls already matches unkeyed rows by
index; a key matters once rows are inserted or removed.

### 2. `memo`: skipping a subtree whose key is unchanged

```rust
memo(("row", stock.id), (stock.id, Version(stock.version)), move |window, cx| {
    render_row(&stock, window, cx)
})
.w_full()
.h(px(24.))
```

`memo(id, key, build)` calls `build` only when `key` differs from the one the
subtree was drawn with last frame. Otherwise the subtree is replayed from last
frame, without being built, laid out, prepainted or painted, and its layout
nodes are kept for when it is built again.

- **The id is the identity, the key is the content.** The id says which
  subtree this is; the key must stand for everything the subtree reads, so
  that two frames with equal keys build the same thing. Whatever the key
  leaves out is shown as it was when the key last changed. Nothing checks
  this, as nothing checks that a view calls `notify`.
- **Keys:**
  - `Version(u64)`: a counter the data's owner increments on every change.
    Cheapest, for data changed in few places.
  - `ContentHash::of(&value)`: a hash of the inputs, for data changed in too
    many places to keep a version of. Two inputs can hash alike, very rarely.
  - The inputs themselves, as a tuple or a type deriving `PartialEq`, when a
    collision must never happen.
  - `AnyMemoKey::new(key)`, for an interface that cannot be generic over the
    key type, such as a trait method.
- **Include what surrounds the data**: selection, stripe parity, widths, sizes
  — anything the subtree's look reads.
- **Size it.** A memo is laid out by its own style, not by its content, like a
  cached view.
- **What the framework handles:** a memo is built again when its bounds,
  content mask or text style change, when the window is refreshed, while
  something is dragged or the inspector is picking, and when a hover, scroll or
  press inside it changes how it looks — including a pointer already over it
  when it was first drawn, and something drawn over it in the same frame, which
  is caught at paint and built on the next frame, asked for automatically.

A memo that moves — in a list being scrolled, or below an inserted row — is
built again, since its bounds changed. It pays off where a window changes a
little at a time, such as live data, not while it scrolls.

### 3. Cached views

`Entity::cached(style)` and `AnyView::cached(style)` exist in the baseline: a
cached view is replayed from last frame unless it was notified, the window was
refreshed, or its bounds, content mask or text style changed. gpui-fast gives
it what `memo` has:

- While it is reused, it keeps its layout nodes, so rendering it again finds
  them instead of allocating new ones. In the baseline, a reused view's nodes
  did not exist to keep.
- It is rendered again when a hover it was painted by changes. The baseline
  relies on the element that sees the pointer arrive notifying the view, so a
  pointer already over the view when it was first drawn is never noticed, and
  neither is something drawn over it in the same frame. Both are caught now.
- It is rendered again while something is being dragged, and when a hover,
  scroll or press inside it marks it dirty.

Choosing between the two:

| | Cached view | `memo` |
|---|---|---|
| Unchanged is decided by | Not notified | Key equal to last frame's |
| Needs | An entity | A key |
| In the baseline | Yes, without the two additions above | No |

A cached view fits a subtree that already is a view, or code that also has to
build against the baseline. `memo` fits a subtree inside stateless code, where
an entity per item would exist only to be cached.

### 4. What the retained frame rewards

gpui-fast saves work on whatever did not change since the last frame, so what
an application keeps unchanged is what it saves:

- **Stable bounds.** An element that keeps its size and position keeps its
  layout, and its primitives replay last frame's ordering. A value that resizes
  its column as it changes moves everything next to it.
- **Stable styles.** A node's style is written to Taffy only when it differs
  from last frame's. A style computed from something that changes every frame
  dirties the node and every ancestor every frame.
- **Colour changes over text changes.** Recolouring shaped text costs no
  shaping; changing its content does.
- **Truncation only where needed.** Text that fits takes no line wrapper; text
  that truncates does.
- **Notifying over refreshing.** `window.refresh()` rebuilds every cached view
  and memo; notifying the view that changed keeps the rest.

## Measuring

`Window::layout_stats()` reports counters accumulated since
`Window::reset_layout_stats()`:

```rust
window.reset_layout_stats(); // also turns on the timings
// ... draw some frames ...
let stats = window.layout_stats();
println!(
    "{} frames, {} nodes created, {} reused, {} style writes, {} lines shaped",
    stats.frames,
    stats.nodes_created,
    stats.nodes_reused,
    stats.style_writes,
    stats.lines_shaped,
);
```

| Field | What to expect of a mostly still window |
|---|---|
| `nodes_created` against `nodes_reused` | Few created, most reused |
| `style_writes`, `children_writes` | Near zero |
| `lines_shaped`, `shape_time` | Only the lines whose text changed |
| `build_time`, `prepaint_time`, `paint_time` | Where the frame's time goes |

The counters cost a few integer increments per node and are always on. The
times take a clock read per measurement, so they are kept only once
`reset_layout_stats()` has been called.

Two benchmarks draw through a real window:

```sh
# a grid of labels, 25% of them changing every frame
cargo run -p gpui --example grid_frames --release -- 50 50 25
# a list scrolled back and forth, rows keyed by index
cargo run -p gpui --example scroll_frames --release -- uniform oscillate 12 index
```
