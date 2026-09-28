# GPUI Fast

**An experimental attempt to add Retained Mode to GPUI — to prove it out
here first, then propose it to GPUI upstream.**

Retained Mode reaches into the core of GPUI — how views render, how layout
nodes are kept, how a frame is painted — which makes it too large a change to
propose before it has been shown to work. gpui-fast is where it is built,
tested and measured first, and once the approach holds up, the aim is to
propose it to [Zed's GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui).

[GPUI](https://gpui.rs), the UI framework of the [Zed](https://github.com/zed-industries/zed)
editor, draws in immediate mode: every frame renders every view, builds a
fresh layout tree, lays it out, shapes its text, and paints the whole window
again, even when almost nothing changed. gpui-fast keeps what the last frame
worked out and redoes only what changed since.

**GPUI's existing API stays unchanged — a standing goal of the whole
effort.** Applications are written exactly as for upstream GPUI; they
just draw less. Retained Mode has to fit behind the API GPUI already has, so
that adopting it upstream asks nothing of Zed or of any application built on
GPUI.

To stay proposable, gpui-fast is kept in step with upstream: upstream's source
is kept as upstream has it, gpui-fast's code lives beside it in `fast/`
directories, and upstream's changes are merged in as Zed makes them. The work
reads as a diff against current GPUI, and can be handed upstream piece by
piece.

## Retained Mode

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

gpui-fast is for trying Retained Mode out, and for measuring it on real
applications; expect its internals to change as the experiment goes on, but
not its API: the public API is upstream's, and code written for upstream GPUI
compiles here untouched. One thing to know: state a view's render reads
outside entities and globals — an `Rc<RefCell<..>>`, the time,
`window.modifiers()` — needs a `cx.notify()` when it changes, as it already
does for a cached view.

Point a project at it in place of upstream GPUI:

```toml
[dependencies]
gpui = { git = "https://github.com/longbridge/gpui-fast" }
```

## gpui-fast, gpui-pre and gpui-ce

Several projects build on GPUI outside Zed:

- **gpui-pre** publishes snapshots of upstream GPUI to crates.io, unmodified,
  so that libraries such as [GPUI Kit](https://github.com/longbridge/gpui-kit)
  can depend on a released GPUI. gpui-fast is a separate experiment and is
  not part of it.
- **gpui-ce** is a community-maintained GPUI.

gpui-fast has a narrower focus: Retained Mode for GPUI. Work that makes it
into upstream GPUI reaches all of these projects, and Zed itself.

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
