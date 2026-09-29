//! A trading workspace, built the way Longbridge Pro builds its main window
//! on GPUI Kit, with live market data streaming in.
//!
//! The other scenarios keep to one screen. A real application puts many of
//! them in one window and wires them to a shared data feed, and most of what
//! a frame costs there comes from how the parts are wired together:
//!
//! - The root view holds a focused search box. The window asks the focused
//!   input whether it accepts text on every frame, through its entity.
//! - The root also holds a text selection layer, as GPUI Kit's `Root` does:
//!   while it is prepainted it makes sure a global exists (`has_global`) and
//!   resets that global's per-frame counter (`global_mut`). A small overlay
//!   view reads that global.
//! - A dock area splits the window into resizable panels. Each panel writes
//!   its bounds into the dock's resize state while it is prepainted, and
//!   notifies it whether they changed or not, as GPUI Kit's resizable panel
//!   does.
//! - Each dock slot is a tab group whose active panel is drawn as a cached
//!   view (`AnyView::cached`), as GPUI Kit's tab panel draws it.
//! - Panels: a watchlist table (a `uniform_list` of rows that highlight on
//!   hover), a quote header with a grid of statistics, time and sales, an
//!   order book and a chart, plus a status bar of market indices.
//! - A market feed entity emits one event per quote. Every panel subscribes
//!   to it, visible or not — a dozen hidden panels do too — so every event
//!   updates every subscriber, and most of them ignore it without notifying.
//!   The quote store is updated and notified, and the watchlist observes it.
//!
//! The scenarios differ in what the user does while quotes stream in: nothing,
//! moving the pointer over the watchlist, or scrolling it. The `quiet` ones
//! have the dock and the selection layer write their state only when it
//! changes, which retained views need to draw them from last frame.

use std::{cell::Cell, ops::Range};

use gpui::{
    AnyView, App, Bounds, Context, ElementInputHandler, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, FontWeight, Global, Hsla, InputEvent as _, IntoElement, MouseMoveEvent,
    Pixels, Render, ScrollDelta, ScrollWheelEvent, SharedString, StyleRefinement, Subscription,
    TouchPhase, UTF16Selection, UniformListScrollHandle, Window, canvas, div, hsla, point,
    prelude::*, px, uniform_list,
};

pub fn scenarios() -> Vec<Box<dyn crate::Scenario>> {
    vec![
        Box::new(WorkspaceScenario {
            name: "workspace-quotes",
            description: "A docked trading workspace at rest while eight quotes a frame stream in and every panel, visible or not, receives each one.",
            kind: Kind::Quotes,
            quiet: false,
        }),
        Box::new(WorkspaceScenario {
            name: "workspace-hover",
            description: "The trading workspace while the pointer moves over the watchlist rows, highlighting them, with two quotes a frame streaming in.",
            kind: Kind::Hover,
            quiet: false,
        }),
        Box::new(WorkspaceScenario {
            name: "workspace-scroll",
            description: "The trading workspace while the watchlist scrolls under the wheel, with two quotes a frame streaming in.",
            kind: Kind::Scroll,
            quiet: false,
        }),
        Box::new(WorkspaceScenario {
            name: "workspace-quiet-quotes",
            description: "workspace-quotes, with the dock and the text selection layer writing their state only when it changes.",
            kind: Kind::Quotes,
            quiet: true,
        }),
        Box::new(WorkspaceScenario {
            name: "workspace-quiet-hover",
            description: "workspace-hover, with the dock and the text selection layer writing their state only when it changes.",
            kind: Kind::Hover,
            quiet: true,
        }),
        Box::new(WorkspaceScenario {
            name: "workspace-quiet-scroll",
            description: "workspace-scroll, with the dock and the text selection layer writing their state only when it changes.",
            kind: Kind::Scroll,
            quiet: true,
        }),
    ]
}

const SYMBOLS: usize = 120;
const HIDDEN_PANELS: usize = 12;
const WATCHLIST_ROW_HEIGHT: Pixels = px(32.);
const TRADES: usize = 40;
const BOOK_LEVELS: usize = 10;
const CHART_BARS: usize = 90;
/// The symbol the quote panels show.
const SELECTED: usize = 7;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Quotes,
    Hover,
    Scroll,
}

struct WorkspaceScenario {
    name: &'static str,
    description: &'static str,
    kind: Kind,
    /// Whether the dock and the text selection layer write their per-frame
    /// state only when it changes, as GPUI Kit does once it stops notifying
    /// and writing globals on every frame. Otherwise they write it on every
    /// prepaint, as GPUI Kit does today.
    quiet: bool,
}

impl crate::Scenario for WorkspaceScenario {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn build(&self, window: &mut Window, cx: &mut App) -> AnyView {
        let quiet = self.quiet;
        let root = cx.new(|cx| Workspace::new(window, quiet, cx));
        let focus = root.read(cx).search.read(cx).focus.clone();
        window.focus(&focus, cx);
        root.into()
    }

    fn step(&self, root: &AnyView, frame: usize, window: &mut Window, cx: &mut App) {
        let workspace: Entity<Workspace> = root.clone().downcast().unwrap();
        let (store, feed, status) = {
            let workspace = workspace.read(cx);
            (
                workspace.store.clone(),
                workspace.feed.clone(),
                workspace.status.clone(),
            )
        };
        let quotes = match self.kind {
            Kind::Quotes => 8,
            Kind::Hover | Kind::Scroll => 2,
        };
        for i in 0..quotes {
            // The quote panels' symbol ticks every third frame.
            let symbol = if i == 0 && frame.is_multiple_of(3) {
                SELECTED
            } else {
                (frame * 7 + i * 13) % SYMBOLS
            };
            let event = store.update(cx, |store, cx| {
                let event = store.tick(symbol, frame);
                cx.notify();
                event
            });
            feed.update(cx, |_, cx| cx.emit(event));
        }
        if frame.is_multiple_of(30) {
            status.update(cx, |status, cx| {
                status.tick += 1;
                cx.notify();
            });
        }

        match self.kind {
            Kind::Quotes => {}
            Kind::Hover => {
                // Down and back up the visible watchlist rows.
                let rows = 22;
                let row = frame % (rows * 2);
                let row = if row < rows { row } else { rows * 2 - 1 - row };
                let y = 48. + 36. + 32. + (row as f32 + 0.5) * f32::from(WATCHLIST_ROW_HEIGHT);
                window.dispatch_event(
                    MouseMoveEvent {
                        position: point(px(300.), px(y)),
                        pressed_button: None,
                        modifiers: Default::default(),
                    }
                    .to_platform_input(),
                    cx,
                );
            }
            Kind::Scroll => {
                // Forty notches down, forty back up.
                let down = (frame / 40).is_multiple_of(2);
                window.dispatch_event(
                    ScrollWheelEvent {
                        position: point(px(300.), px(400.)),
                        delta: ScrollDelta::Pixels(point(
                            px(0.),
                            px(if down { -24. } else { 24. }),
                        )),
                        modifiers: Default::default(),
                        touch_phase: TouchPhase::Moved,
                    }
                    .to_platform_input(),
                    cx,
                );
            }
        }
    }
}

/// One quote as the feed delivers it.
#[derive(Clone, Copy)]
struct QuoteEvent {
    symbol: usize,
    last: f64,
    volume: u64,
}

#[derive(Clone, Copy)]
struct Quote {
    last: f64,
    open: f64,
    high: f64,
    low: f64,
    volume: u64,
}

/// Every symbol's latest quote.
struct QuoteStore {
    quotes: Vec<Quote>,
    names: Vec<SharedString>,
    codes: Vec<SharedString>,
}

impl QuoteStore {
    fn new() -> Self {
        let quotes = (0..SYMBOLS)
            .map(|ix| {
                let base = 10. + (ix * 37 % 500) as f64 + (ix % 7) as f64 * 0.13;
                Quote {
                    last: base,
                    open: base,
                    high: base * 1.02,
                    low: base * 0.98,
                    volume: 1_000_000 + (ix as u64 * 7_919) % 9_000_000,
                }
            })
            .collect();
        Self {
            quotes,
            names: (0..SYMBOLS)
                .map(|ix| SharedString::from(format!("Company {ix} Holdings")))
                .collect(),
            codes: (0..SYMBOLS)
                .map(|ix| {
                    let market = ["US", "HK", "SH", "SZ", "SG"][ix % 5];
                    SharedString::from(format!("{market} {:05}", ix * 97 % 99_991))
                })
                .collect(),
        }
    }

    fn tick(&mut self, symbol: usize, frame: usize) -> QuoteEvent {
        let quote = &mut self.quotes[symbol];
        let step = ((frame * 31 + symbol * 17) % 21) as f64 - 10.;
        quote.last = (quote.last * (1. + step / 2_000.)).max(0.01);
        quote.high = quote.high.max(quote.last);
        quote.low = quote.low.min(quote.last);
        quote.volume += 100 + (frame as u64 % 9) * 100;
        QuoteEvent {
            symbol,
            last: quote.last,
            volume: quote.volume,
        }
    }
}

/// The feed every panel subscribes to.
struct MarketFeed;

impl EventEmitter<QuoteEvent> for MarketFeed {}

/// Per-frame bookkeeping kept in a global, as GPUI Kit's text selection keeps
/// the order its selectable texts paint in.
#[derive(Default)]
struct SelectionFrame {
    order: Cell<u64>,
    participants: usize,
}

impl Global for SelectionFrame {}

fn up_color(change: f64) -> Hsla {
    if change >= 0. {
        hsla(0.36, 0.6, 0.45, 1.)
    } else {
        hsla(0.0, 0.7, 0.55, 1.)
    }
}

const TEXT: Hsla = hsla(0., 0., 0.9, 1.);
const MUTED: Hsla = hsla(0., 0., 0.55, 1.);
const BORDER: Hsla = hsla(0., 0., 0.2, 1.);
const PANEL_BG: Hsla = hsla(0., 0., 0.07, 1.);
const HOVER_BG: Hsla = hsla(0., 0., 0.14, 1.);

/// The window's root: header with the search box, the dock, the status bar,
/// and the text selection layer.
struct Workspace {
    store: Entity<QuoteStore>,
    feed: Entity<MarketFeed>,
    search: Entity<SearchBox>,
    dock: Entity<DockArea>,
    status: Entity<StatusBar>,
    overlay: Entity<SelectionOverlay>,
    /// Panels that are not shown but still receive the feed, as panels in
    /// background tabs and closed docks do.
    _hidden: Vec<Entity<HiddenPanel>>,
    /// See [`WorkspaceScenario::quiet`].
    quiet: bool,
}

impl Workspace {
    fn new(window: &mut Window, quiet: bool, cx: &mut Context<Self>) -> Self {
        let store = cx.new(|_| QuoteStore::new());
        let feed = cx.new(|_| MarketFeed);
        let search = cx.new(|cx| SearchBox {
            focus: cx.focus_handle(),
            text: String::new(),
        });

        let watchlist: AnyView = cx.new(|cx| Watchlist::new(store.clone(), &feed, cx)).into();
        let detail: AnyView = cx
            .new(|cx| QuoteDetail::new(store.clone(), &feed, cx))
            .into();
        let trades: AnyView = cx.new(|cx| TimeAndSales::new(&feed, cx)).into();
        let book: AnyView = cx.new(|cx| OrderBook::new(&feed, cx)).into();
        let chart: AnyView = cx.new(|cx| Chart::new(store.clone(), &feed, cx)).into();
        let news: AnyView = cx.new(|cx| HiddenPanel::new("News", &feed, cx)).into();

        let groups = vec![
            cx.new(|_| TabGroup::new(vec![("Watchlist", watchlist)])),
            cx.new(|_| TabGroup::new(vec![("Quote", detail)])),
            cx.new(|_| TabGroup::new(vec![("Trades", trades), ("News", news)])),
            cx.new(|_| TabGroup::new(vec![("Order Book", book)])),
            cx.new(|_| TabGroup::new(vec![("Chart", chart)])),
        ];
        let resize = cx.new(|_| ResizeState {
            sizes: vec![Bounds::default(); groups.len()],
        });
        let dock = cx.new(|_| DockArea {
            groups,
            resize,
            quiet,
        });
        let _ = window;

        Self {
            status: cx.new(|_| StatusBar { tick: 0 }),
            overlay: cx.new(|_| SelectionOverlay),
            _hidden: (0..HIDDEN_PANELS)
                .map(|ix| {
                    cx.new(|cx| {
                        HiddenPanel::new(
                            ["Options", "Warrants", "Filings", "Ranks"][ix % 4],
                            &feed,
                            cx,
                        )
                    })
                })
                .collect(),
            store,
            feed,
            search,
            dock,
            quiet,
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let quiet = self.quiet;
        div()
            .id("workspace")
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(0., 0., 0.04, 1.))
            .text_color(TEXT)
            .text_size(px(13.))
            .child(
                div()
                    .h(px(48.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .border_b_1()
                    .border_color(BORDER)
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child("Watchlist")
                            .child("Markets")
                            .child("Portfolio"),
                    )
                    .child(div().w(px(320.)).child(self.search.clone()))
                    .child(div().text_color(MUTED).child("Account A/C(1637)")),
            )
            .child(div().flex_1().min_h_0().child(self.dock.clone()))
            .child(
                self.status
                    .clone()
                    .cached(StyleRefinement::default().h(px(24.)).w_full()),
            )
            // GPUI Kit's `Root` keeps its window-wide text selection here.
            .child(
                canvas(
                    move |_, _, cx| {
                        if !cx.has_global::<SelectionFrame>() {
                            cx.set_global(SelectionFrame::default());
                        }
                        // Quiet, the counter is reset through a cell in the
                        // global rather than by writing the global.
                        if quiet {
                            cx.global::<SelectionFrame>().order.set(1);
                        } else {
                            cx.global_mut::<SelectionFrame>().order.set(1);
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_0(),
            )
            .child(self.overlay.clone())
    }
}

/// Reads the selection global, as the selection handles' overlay does.
struct SelectionOverlay;

impl Render for SelectionOverlay {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let participants = cx
            .try_global::<SelectionFrame>()
            .map_or(0, |frame| frame.participants);
        div()
            .absolute()
            .size_0()
            .when(participants > 1_000, |this| this.child("…"))
    }
}

struct SearchBox {
    focus: FocusHandle,
    text: String,
}

impl Focusable for SearchBox {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SearchBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let focus = self.focus.clone();
        div()
            .id("search")
            .track_focus(&self.focus)
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(BORDER)
            .text_color(MUTED)
            .child(if self.text.is_empty() {
                SharedString::from("Press / to search")
            } else {
                SharedString::from(self.text.clone())
            })
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                    },
                )
                .absolute()
                .size_full(),
            )
    }
}

impl EntityInputHandler for SearchBox {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        self.text.get(range).map(str::to_string)
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.text.len()..self.text.len(),
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text.push_str(text);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text_in_range(range, text, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.text.len())
    }
}

/// Where the dock's panels were laid out, written as they are prepainted.
struct ResizeState {
    sizes: Vec<Bounds<Pixels>>,
}

/// The dock: the watchlist on the left, the quote panels in a column on the
/// right, each slot a resizable panel holding a tab group.
struct DockArea {
    groups: Vec<Entity<TabGroup>>,
    resize: Entity<ResizeState>,
    /// See [`WorkspaceScenario::quiet`].
    quiet: bool,
}

impl DockArea {
    fn panel(&self, ix: usize, cx: &Context<Self>) -> impl IntoElement + use<> {
        let resize = self.resize.clone();
        let quiet = self.quiet;
        let size = self.resize.read(cx).sizes[ix].size;
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .when(size.width > px(0.), |this| {
                this.flex_basis(size.width.min(px(2000.)))
            })
            .border_1()
            .border_color(BORDER)
            .child(self.groups[ix].clone())
            // As GPUI Kit's resizable panel does: the bounds are written back
            // and the state notified on every prepaint, changed or not, or,
            // quiet, only when they changed.
            .child(
                canvas(
                    move |bounds, _, cx| {
                        resize.update(cx, |state, cx| {
                            if !quiet || state.sizes[ix] != bounds {
                                state.sizes[ix] = bounds;
                                cx.notify();
                            }
                        })
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
    }
}

impl Render for DockArea {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_row()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(760.))
                    .h_full()
                    .child(self.panel(0, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .h_full()
                    .child(self.panel(1, cx))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_1()
                            .child(self.panel(2, cx))
                            .child(self.panel(3, cx)),
                    )
                    .child(self.panel(4, cx)),
            )
    }
}

/// A tab bar over the active panel, which is drawn as a cached view.
struct TabGroup {
    tabs: Vec<(&'static str, AnyView)>,
    active: usize,
}

impl TabGroup {
    fn new(tabs: Vec<(&'static str, AnyView)>) -> Self {
        Self { tabs, active: 0 }
    }
}

impl Render for TabGroup {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(PANEL_BG)
            .child(
                div()
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(BORDER)
                    .children(self.tabs.iter().enumerate().map(|(ix, (title, _))| {
                        div()
                            .id(ix)
                            .px_3()
                            .h_full()
                            .flex()
                            .items_center()
                            .when(ix == self.active, |this| {
                                this.font_weight(FontWeight::SEMIBOLD)
                                    .border_b_2()
                                    .border_color(TEXT)
                            })
                            .hover(|style| style.bg(HOVER_BG))
                            .child(*title)
                    })),
            )
            .child(
                div().relative().flex_1().min_h_0().child(
                    self.tabs[self.active]
                        .1
                        .clone()
                        .cached(StyleRefinement::default().absolute().size_full()),
                ),
            )
    }
}

fn price(value: f64) -> SharedString {
    format!("{value:.3}").into()
}

fn volume(value: u64) -> SharedString {
    if value >= 1_000_000 {
        format!("{:.2}M", value as f64 / 1e6).into()
    } else {
        format!("{:.1}K", value as f64 / 1e3).into()
    }
}

/// The watchlist table: a header over a `uniform_list` of rows.
struct Watchlist {
    store: Entity<QuoteStore>,
    scroll: UniformListScrollHandle,
    _subscriptions: [Subscription; 2],
}

impl Watchlist {
    fn new(store: Entity<QuoteStore>, feed: &Entity<MarketFeed>, cx: &mut Context<Self>) -> Self {
        Self {
            _subscriptions: [
                cx.observe(&store, |_, _, cx| cx.notify()),
                // Its alerts check every quote; nothing to show for most.
                cx.subscribe(feed, |_, _, event: &QuoteEvent, _| {
                    let _ = event.volume;
                }),
            ],
            store,
            scroll: UniformListScrollHandle::new(),
        }
    }
}

const COLUMNS: [(&str, f32); 9] = [
    ("Symbol", 90.),
    ("Name", 170.),
    ("Price", 80.),
    ("%Chg", 70.),
    ("Chg", 70.),
    ("Volume", 80.),
    ("High", 70.),
    ("Low", 70.),
    ("Open", 70.),
];

impl Render for Watchlist {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(BORDER)
                    .text_color(MUTED)
                    .children(
                        COLUMNS
                            .iter()
                            .map(|(title, width)| div().w(px(*width)).px_2().child(*title)),
                    ),
            )
            .child(
                uniform_list(
                    "watchlist-rows",
                    SYMBOLS,
                    cx.processor(move |_, range: Range<usize>, _, cx| {
                        let store = store.read(cx);
                        range
                            .map(|ix| {
                                let quote = store.quotes[ix];
                                let change = quote.last - quote.open;
                                let color = up_color(change);
                                let cells: [SharedString; 9] = [
                                    store.codes[ix].clone(),
                                    store.names[ix].clone(),
                                    price(quote.last),
                                    format!("{:+.2}%", change / quote.open * 100.).into(),
                                    format!("{change:+.3}").into(),
                                    volume(quote.volume),
                                    price(quote.high),
                                    price(quote.low),
                                    price(quote.open),
                                ];
                                div()
                                    .id(ix)
                                    .h(WATCHLIST_ROW_HEIGHT)
                                    .flex()
                                    .items_center()
                                    .border_b_1()
                                    .border_color(BORDER)
                                    .hover(|style| style.bg(HOVER_BG))
                                    .children(cells.into_iter().zip(COLUMNS).enumerate().map(
                                        |(column, (text, (_, width)))| {
                                            div()
                                                .w(px(width))
                                                .px_2()
                                                .overflow_hidden()
                                                .when(column >= 2 && column <= 4, |this| {
                                                    this.text_color(color)
                                                })
                                                .child(text)
                                        },
                                    ))
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1(),
            )
    }
}

/// The selected symbol's price, change and a grid of statistics.
struct QuoteDetail {
    store: Entity<QuoteStore>,
    _subscription: Subscription,
}

impl QuoteDetail {
    fn new(store: Entity<QuoteStore>, feed: &Entity<MarketFeed>, cx: &mut Context<Self>) -> Self {
        Self {
            store,
            _subscription: cx.subscribe(feed, |_, _, event: &QuoteEvent, cx| {
                if event.symbol == SELECTED {
                    cx.notify();
                }
            }),
        }
    }
}

impl Render for QuoteDetail {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let quote = store.quotes[SELECTED];
        let change = quote.last - quote.open;
        let stats: [(&str, SharedString); 16] = [
            ("Open", price(quote.open)),
            ("High", price(quote.high)),
            ("Low", price(quote.low)),
            ("Prev. Close", price(quote.open)),
            ("Volume", volume(quote.volume)),
            ("Turnover", volume(quote.volume * 12)),
            ("Mkt Cap", "4938.67B".into()),
            ("Float Cap", "4930.19B".into()),
            ("P/E (TTM)", "38.31".into()),
            ("P/B", "45.93".into()),
            ("EPS (TTM)", "8.83".into()),
            ("Dividend", "1.060".into()),
            ("52wk High", price(quote.high * 1.2)),
            ("52wk Low", price(quote.low * 0.7)),
            (
                "Amplitude",
                format!("{:.2}%", (quote.high - quote.low) / quote.open * 100.).into(),
            ),
            ("Avg Price", price((quote.high + quote.low) / 2.)),
        ];
        div()
            .size_full()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(28.))
                            .text_color(up_color(change))
                            .child(price(quote.last)),
                    )
                    .child(
                        div()
                            .text_color(up_color(change))
                            .child(format!("{change:+.3}")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .children(stats.into_iter().map(|(label, value)| {
                        div()
                            .w_1_2()
                            .flex()
                            .justify_between()
                            .pr_4()
                            .child(div().text_color(MUTED).child(label))
                            .child(value)
                    })),
            )
    }
}

/// The latest trades in the selected symbol.
struct TimeAndSales {
    trades: Vec<(f64, u64)>,
    _subscription: Subscription,
}

impl TimeAndSales {
    fn new(feed: &Entity<MarketFeed>, cx: &mut Context<Self>) -> Self {
        Self {
            trades: (0..TRADES)
                .map(|ix| (100. + ix as f64 * 0.01, 100))
                .collect(),
            _subscription: cx.subscribe(feed, |this, _, event: &QuoteEvent, cx| {
                if event.symbol == SELECTED {
                    this.trades.insert(0, (event.last, event.volume % 1_000));
                    this.trades.truncate(TRADES);
                    cx.notify();
                }
            }),
        }
    }
}

impl Render for TimeAndSales {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_2()
            .flex()
            .flex_col()
            .overflow_hidden()
            .children(self.trades.iter().enumerate().map(|(ix, (price_, size))| {
                div()
                    .flex()
                    .justify_between()
                    .child(div().text_color(MUTED).child(format!(
                        "10:{:02}:{:02}",
                        ix / 60,
                        ix % 60
                    )))
                    .child(
                        div()
                            .text_color(up_color(if ix % 3 == 0 { -1. } else { 1. }))
                            .child(price(*price_)),
                    )
                    .child(format!("{size}"))
            }))
    }
}

/// Bids and asks for the selected symbol.
struct OrderBook {
    mid: f64,
    _subscription: Subscription,
}

impl OrderBook {
    fn new(feed: &Entity<MarketFeed>, cx: &mut Context<Self>) -> Self {
        Self {
            mid: 100.,
            _subscription: cx.subscribe(feed, |this, _, event: &QuoteEvent, cx| {
                if event.symbol == SELECTED {
                    this.mid = event.last;
                    cx.notify();
                }
            }),
        }
    }
}

impl Render for OrderBook {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let level = |ix: usize, bid: bool| {
            let offset = (ix + 1) as f64 * 0.01;
            let value = if bid {
                self.mid - offset
            } else {
                self.mid + offset
            };
            div()
                .flex()
                .justify_between()
                .px_2()
                .bg(if bid {
                    hsla(0.36, 0.5, 0.2, 0.3)
                } else {
                    hsla(0., 0.5, 0.25, 0.3)
                })
                .child(price(value))
                .child(format!("{}", (ix * 37 + 11) % 400))
        };
        div()
            .size_full()
            .flex()
            .flex_row()
            .gap_1()
            .p_2()
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .children((0..BOOK_LEVELS).map(|ix| level(ix, true))),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .children((0..BOOK_LEVELS).map(|ix| level(ix, false))),
            )
    }
}

/// An intraday chart of the selected symbol, one bar per minute.
struct Chart {
    bars: Vec<f64>,
    _subscription: Subscription,
}

impl Chart {
    fn new(store: Entity<QuoteStore>, feed: &Entity<MarketFeed>, cx: &mut Context<Self>) -> Self {
        let open = store.read(cx).quotes[SELECTED].open;
        Self {
            bars: (0..CHART_BARS)
                .map(|ix| open * (1. + ((ix * 7 % 13) as f64 - 6.) / 500.))
                .collect(),
            _subscription: cx.subscribe(feed, |this, _, event: &QuoteEvent, cx| {
                if event.symbol == SELECTED {
                    *this.bars.last_mut().unwrap() = event.last;
                    cx.notify();
                }
            }),
        }
    }
}

impl Render for Chart {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (low, high) = self
            .bars
            .iter()
            .fold((f64::MAX, f64::MIN), |(low, high), bar| {
                (low.min(*bar), high.max(*bar))
            });
        let span = (high - low).max(0.0001);
        div()
            .size_full()
            .p_2()
            .flex()
            .flex_row()
            .items_end()
            .gap(px(1.))
            .children(self.bars.iter().enumerate().map(|(ix, bar)| {
                let height = 8. + ((bar - low) / span * 120.) as f32;
                div()
                    .flex_1()
                    .h(px(height))
                    .bg(up_color(if ix % 4 == 0 { -1. } else { 1. }))
            }))
    }
}

/// A panel in a background tab or a closed dock: it receives the feed like
/// every other and ignores what it does not show.
struct HiddenPanel {
    title: &'static str,
    received: usize,
    _subscription: Subscription,
}

impl HiddenPanel {
    fn new(title: &'static str, feed: &Entity<MarketFeed>, cx: &mut Context<Self>) -> Self {
        Self {
            title,
            received: 0,
            _subscription: cx.subscribe(feed, |this, _, _: &QuoteEvent, _| {
                this.received += 1;
            }),
        }
    }
}

impl Render for HiddenPanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_3()
            .child(self.title)
            .child(format!("{} updates", self.received))
    }
}

/// Market indices along the bottom of the window.
struct StatusBar {
    tick: usize,
}

impl Render for StatusBar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .gap_4()
            .px_3()
            .border_t_1()
            .border_color(BORDER)
            .text_size(px(12.))
            .children(
                ["HSI", "HSCEI", "HSTECH", "HSCCI", "SPX", "IXIC"]
                    .iter()
                    .enumerate()
                    .map(|(ix, name)| {
                        let value = 20_000. + (ix * 1_234 + self.tick * 7) as f64 * 0.37;
                        div()
                            .flex()
                            .gap_1()
                            .child(div().text_color(MUTED).child(*name))
                            .child(
                                div()
                                    .text_color(up_color(if ix % 2 == 0 { -1. } else { 1. }))
                                    .child(price(value)),
                            )
                    }),
            )
    }
}
