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
gpui-fast retains two things:

| What is retained | Drawn again from the last frame while                                                                                                                                                                                                                                           |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Views**        | nothing the view read while rendering changed — the entities it accessed, the globals it read, the list and scroll state it depends on — and it is drawn at the same place. It is then neither rendered, laid out, prepainted nor painted: its last frame's output is replayed. |
| **Layout nodes** | the element asks for the same style, children and measurement. Taffy's per-node cache survives, so unchanged parts of the tree are not laid out again. Elements keep their nodes by their path from the root, or by their `ElementId` wherever they move among their siblings.  |

Hover, scrolling, bounds, content masks and window refreshes invalidate
exactly what they affect, without the application doing anything. Retention
can be turned off, for comparison or debugging, with `GPUI_VIEW_RETENTION=0`.
[`docs/retained-mode.md`](docs/retained-mode.md) describes how it works.

What it is worth, in headless CPU time per frame for a window of 60 panel
views with 64 labels each, in release builds on Linux:

| Panels notified per frame | Upstream | gpui-fast       |
| ------------------------- | -------- | --------------- |
| None, still               | 10.43 ms | 0.25 ms (−98%)  |
| One                       | 11.58 ms | 1.35 ms (−88%)  |
| Six                       | 13.06 ms | 3.86 ms (−70%)  |
| All sixty                 | 18.25 ms | 14.57 ms (−20%) |

"Upstream" is the same build drawing every view from scratch, as with
`GPUI_VIEW_RETENTION=0`; the figures come from the `retained_bench` test. The
gain follows how little of the window changes.

A retained frame is checked against the frame drawing from scratch would
have produced: a test drives two windows through the same random history, one
drawing incrementally and one from scratch, and requires every frame to match.

## Using it

The public API is upstream's, and code written for upstream GPUI compiles here
untouched. One thing to know: state a view's render reads outside entities and
globals — an `Rc<RefCell<..>>`, the time, `window.modifiers()` — needs a
`cx.notify()` when it changes, as it already does for a cached view.

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
