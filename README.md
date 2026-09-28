# GPUI Fast

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

| What is retained                    | Drawn again from the last frame while                                                                                                                                                                                                                                                 |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Views**                           | nothing the view read while rendering changed — the entities it accessed, the globals it read, the list and scroll state it depends on — and it is drawn at the same place. It is then neither rendered, laid out, prepainted nor painted: its last frame's output is replayed.       |
| **`memo(id, key, build)` subtrees** | the key is unchanged. Any `PartialEq` value that stands for what the subtree depends on will do; `Version`, `ContentHash` and `AnyMemoKey` are ready-made ones.                                                                                                                       |
| **Layout nodes**                    | the element asks for the same style, children and measurement. Taffy's per-node cache survives, so unchanged parts of the tree are not laid out again. Elements keep their nodes by their path from the root, or by their `ElementId` wherever they move among their siblings.        |
| **Shaped text**                     | its text, font and runs are unchanged. Recoloured text keeps its shaping; text already fitting the width it is offered is not shaped again.                                                                                                                                           |
| **Primitive ordering**              | the bounds painted before it are unchanged; only what meets a changed bound is ordered afresh.                                                                                                                                                                                        |
| **Element identity**                | the element's path is the one it had last frame.                                                                                                                                                                                                                                      |

Hover, scrolling, bounds, content masks and window refreshes invalidate
exactly what they affect, without the application doing anything. Retention
can be turned off, for comparison or debugging, with
`Window::set_view_retention(false)` or `GPUI_VIEW_RETENTION=0`.

What it is worth, in main-thread CPU per frame on an Apple M4, median of five
runs, against GPUI as extracted:

| Workload                                 | Upstream | gpui-fast      |
| ---------------------------------------- | -------- | -------------- |
| 2500 live labels in a real window, still | 8.20 ms  | 3.64 ms (−56%) |
| 2500 live labels, every cell changing    | 8.25 ms  | 5.18 ms (−37%) |
| A wide table scrolled back and forth     | 7.89 ms  | 4.07 ms (−48%) |

The gain follows how little of the window changes. In gpui-kit's DataTable,
a table taking new values 30 times a second, drawing costs 37% less than
before, and 48% less once the table memoizes its rows.

A retained frame is checked against the frame drawing from scratch would
have produced: a test drives two windows through the same random history, one
drawing incrementally and one from scratch, and requires every frame to match.

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

## Following upstream

GPUI Fast is based on Zed at the commit recorded in [`UPSTREAM`](UPSTREAM) and
takes upstream's changes as Zed makes them. Its own code is kept apart from
upstream's, so a new upstream is a merge rather than a port. See
[`CONTRIBUTING.md`](CONTRIBUTING.md) for how that is kept true, and for building,
testing and measuring.

## License

Apache-2.0, as upstream — copyright Zed Industries, Inc. See `LICENSE-APACHE`.
This is a modified fork; the changes are the commits after `11a44c4`, the
import of upstream.
