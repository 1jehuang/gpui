//! The trading workspace page: a docked window of market panels built the way
//! Longbridge Pro builds its main window on GPUI Kit, with a live stream of
//! quotes that a timer pushes in.
//!
//! The component pages keep to one screen. A real application puts many of
//! them in one window and wires them to a shared data feed, and most of what
//! a frame costs there comes from how the parts are wired together:
//!
//! - The workspace holds a focused search box. The window asks the focused
//!   input whether it accepts text on every frame, through its entity.
//! - It also holds a text selection layer, as GPUI Kit's `Root` does: while
//!   it is prepainted it makes sure a global exists (`has_global`) and resets
//!   that global's per-frame counter (`global_mut`). A small overlay view
//!   reads that global.
//! - A dock area splits it into resizable panels. Each panel writes its
//!   bounds into the dock's resize state while it is prepainted, and notifies
//!   it whether they changed or not, as GPUI Kit's resizable panel does.
//! - Each dock slot is a tab group whose active panel is drawn as a cached
//!   view (`AnyView::cached`), as GPUI Kit's tab panel draws it.
//! - Panels: a watchlist table (a `uniform_list` of rows that highlight on
//!   hover), a quote header with a grid of statistics, time and sales, an
//!   order book and a chart, and a status bar of market indices.
//! - A market feed entity emits one event per quote. Every panel subscribes
//!   to it, visible or not — a dozen hidden panels do too — so every event
//!   updates every subscriber, and most of them ignore it without notifying.
//!   The quote store is updated and notified, and the watchlist observes it.
//!
//! The same workspace, without a window, is the headless `workspace-*`
//! scenarios in `scenarios/workspace.rs`.

use std::{cell::Cell, ops::Range, rc::Rc, time::Duration};

use gpui::{
    AnyView, Bounds, Context, ElementInputHandler, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, FontWeight, Global, Hsla, IntoElement, Pixels, Render, SharedString,
    StyleRefinement, Subscription, Task, UTF16Selection, UniformListScrollHandle, Window, canvas,
    div, prelude::*, px, uniform_list,
};

use super::theme::{Theme, theme};

pub const SYMBOLS: usize = 120;
const HIDDEN_PANELS: usize = 12;
pub const WATCHLIST_ROW_HEIGHT: Pixels = px(32.);
const TRADES: usize = 40;
const BOOK_LEVELS: usize = 10;
const CHART_BARS: usize = 90;
/// The symbol the quote panels show.
const SELECTED: usize = 7;

/// How often the stream pushes quotes: about sixty times a second, as a busy
/// market's feed does.
const STREAM_EVERY: Duration = Duration::from_millis(16);

/// Quotes the stream pushes each time, unless an automatic run says
/// otherwise.
pub const QUOTES_PER_TICK: usize = 8;

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

    fn tick(&mut self, symbol: usize, tick: usize) -> QuoteEvent {
        let quote = &mut self.quotes[symbol];
        let step = ((tick * 31 + symbol * 17) % 21) as f64 - 10.;
        quote.last = (quote.last * (1. + step / 2_000.)).max(0.01);
        quote.high = quote.high.max(quote.last);
        quote.low = quote.low.min(quote.last);
        quote.volume += 100 + (tick as u64 % 9) * 100;
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
    order: u64,
    participants: usize,
}

impl Global for SelectionFrame {}

/// A gain or a loss, told by its sign as well as this color.
fn change_color(change: f64, theme: &Theme) -> Hsla {
    if change >= 0. {
        theme.success
    } else {
        theme.danger
    }
}

/// The page: a header with the search box, the dock, the status bar, and the
/// text selection layer. Its timer streams quotes into the store and the
/// feed; it does not notify the workspace itself.
pub struct Workspace {
    store: Entity<QuoteStore>,
    feed: Entity<MarketFeed>,
    pub search: Entity<SearchBox>,
    pub watchlist: Entity<Watchlist>,
    dock: Entity<DockArea>,
    status: Entity<StatusBar>,
    overlay: Entity<SelectionOverlay>,
    /// Panels that are not shown but still receive the feed, as panels in
    /// background tabs and closed docks do.
    _hidden: Vec<Entity<HiddenPanel>>,
    stream: Option<Task<()>>,
    /// Quotes each tick of the stream pushes.
    pub quotes_per_tick: usize,
    ticks: usize,
}

impl Workspace {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = cx.new(|_| QuoteStore::new());
        let feed = cx.new(|_| MarketFeed);
        let search = cx.new(|cx| SearchBox {
            focus: cx.focus_handle(),
            text: String::new(),
        });

        let watchlist = cx.new(|cx| Watchlist::new(store.clone(), &feed, cx));
        let detail: AnyView = cx
            .new(|cx| QuoteDetail::new(store.clone(), &feed, cx))
            .into();
        let trades: AnyView = cx.new(|cx| TimeAndSales::new(&feed, cx)).into();
        let book: AnyView = cx.new(|cx| OrderBook::new(&feed, cx)).into();
        let chart: AnyView = cx.new(|cx| Chart::new(store.clone(), &feed, cx)).into();
        let news: AnyView = cx.new(|cx| HiddenPanel::new("News", &feed, cx)).into();

        let groups = vec![
            cx.new(|_| TabGroup::new(vec![("Watchlist", watchlist.clone().into())])),
            cx.new(|_| TabGroup::new(vec![("Quote", detail)])),
            cx.new(|_| TabGroup::new(vec![("Trades", trades), ("News", news)])),
            cx.new(|_| TabGroup::new(vec![("Order Book", book)])),
            cx.new(|_| TabGroup::new(vec![("Chart", chart)])),
        ];
        let resize = cx.new(|_| ResizeState {
            sizes: vec![Bounds::default(); groups.len()],
        });
        let dock = cx.new(|_| DockArea { groups, resize });

        Self {
            status: cx.new(|_| StatusBar { tick: 0 }),
            overlay: cx.new(|_| SelectionOverlay),
            _hidden: (0..HIDDEN_PANELS)
                .map(|ix| {
                    let title = ["Options", "Warrants", "Filings", "Ranks"][ix % 4];
                    cx.new(|cx| HiddenPanel::new(title, &feed, cx))
                })
                .collect(),
            store,
            feed,
            search,
            watchlist,
            dock,
            stream: None,
            quotes_per_tick: QUOTES_PER_TICK,
            ticks: 0,
        }
    }

    /// Starts or stops streaming quotes, returning whether it streams.
    pub fn toggle_stream(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.stream.take().is_some() {
            return false;
        }
        self.stream = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(STREAM_EVERY).await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        }));
        true
    }

    /// Pushes one tick's quotes: each updates the store, which notifies the
    /// watchlist, and goes out on the feed to every panel. The quote panels'
    /// symbol ticks every third time, and the indices every thirtieth.
    fn tick(&mut self, cx: &mut Context<Self>) {
        self.ticks += 1;
        let tick = self.ticks;
        for i in 0..self.quotes_per_tick {
            let symbol = if i == 0 && tick.is_multiple_of(3) {
                SELECTED
            } else {
                (tick * 7 + i * 13) % SYMBOLS
            };
            let event = self.store.update(cx, |store, cx| {
                let event = store.tick(symbol, tick);
                cx.notify();
                event
            });
            self.feed.update(cx, |_, cx| cx.emit(event));
        }
        if tick.is_multiple_of(30) {
            self.status.update(cx, |status, cx| {
                status.tick += 1;
                cx.notify();
            });
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .id("workspace")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .text_sm()
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .h_12()
                    .px_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Watchlist")
                            .child(div().text_color(theme.muted_foreground).child("Markets"))
                            .child(div().text_color(theme.muted_foreground).child("Portfolio")),
                    )
                    .child(div().w_72().child(self.search.clone()))
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child("Account A/C(1637)"),
                    ),
            )
            .child(div().flex_1().min_h_0().child(self.dock.clone()))
            .child(
                self.status
                    .clone()
                    .cached(StyleRefinement::default().h_6().w_full().flex_shrink_0()),
            )
            // GPUI Kit's `Root` keeps its window-wide text selection here.
            .child(
                canvas(
                    |_, _, cx| {
                        if !cx.has_global::<SelectionFrame>() {
                            cx.set_global(SelectionFrame::default());
                        }
                        cx.global_mut::<SelectionFrame>().order = 1;
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
        let (order, participants) = cx
            .try_global::<SelectionFrame>()
            .map_or((0, 0), |frame| (frame.order, frame.participants));
        div()
            .absolute()
            .size_0()
            .when(order > 1 && participants > 1_000, |this| this.child("…"))
    }
}

/// The search box, focused while the page is shown, as a trading window's
/// symbol search is.
pub struct SearchBox {
    pub focus: FocusHandle,
    text: String,
}

impl Render for SearchBox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let entity = cx.entity();
        let focus = self.focus.clone();
        let focused = self.focus.is_focused(window);
        div()
            .id("search")
            .track_focus(&self.focus)
            .relative()
            .flex()
            .items_center()
            .h_7()
            .px_2()
            .rounded(theme.radius)
            .border_1()
            .border_color(if focused {
                theme.foreground
            } else {
                theme.border
            })
            .text_color(theme.muted_foreground)
            .child(if self.text.is_empty() {
                SharedString::from("Search symbols")
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

/// The direction a panel's container lays panels out in, which its size is
/// measured along.
#[derive(Clone, Copy)]
enum Axis {
    Horizontal,
    Vertical,
}

/// The dock: the watchlist on the left, the quote panels in a column on the
/// right, each slot a resizable panel holding a tab group.
struct DockArea {
    groups: Vec<Entity<TabGroup>>,
    resize: Entity<ResizeState>,
}

impl DockArea {
    fn panel(&self, ix: usize, axis: Axis, cx: &Context<Self>) -> impl IntoElement + use<> {
        let border = theme(cx).border;
        let resize = self.resize.clone();
        let size = self.resize.read(cx).sizes[ix].size;
        let basis = match axis {
            Axis::Horizontal => size.width,
            Axis::Vertical => size.height,
        };
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .when(basis > px(0.), |this| this.flex_basis(basis.min(px(2000.))))
            .border_1()
            .border_color(border)
            .child(self.groups[ix].clone())
            // As GPUI Kit's resizable panel does: the bounds are written back
            // and the state notified on every prepaint, changed or not.
            .child(
                canvas(
                    move |bounds, _, cx| {
                        resize.update(cx, |state, cx| {
                            state.sizes[ix] = bounds;
                            cx.notify();
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
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.panel(0, Axis::Vertical, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(440.))
                    .flex_shrink_0()
                    .h_full()
                    .child(self.panel(1, Axis::Vertical, cx))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_1()
                            .min_h_0()
                            .child(self.panel(2, Axis::Horizontal, cx))
                            .child(self.panel(3, Axis::Horizontal, cx)),
                    )
                    .child(self.panel(4, Axis::Vertical, cx)),
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .h_8()
                    .bg(theme.muted)
                    .border_b_1()
                    .border_color(theme.border)
                    .children(self.tabs.iter().enumerate().map(|(ix, (title, _))| {
                        let active = ix == self.active;
                        div()
                            .id(ix)
                            .flex()
                            .items_center()
                            .h_full()
                            .px_3()
                            .when(active, |this| {
                                this.font_weight(FontWeight::MEDIUM)
                                    .bg(theme.background)
                                    .border_b_2()
                                    .border_color(theme.foreground)
                            })
                            .when(!active, |this| {
                                this.text_color(theme.muted_foreground)
                                    .hover(|this| this.bg(theme.accent))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.active = ix;
                                cx.notify();
                            }))
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

/// The watchlist table: a header over a `uniform_list` of rows, which owns
/// its scrolling.
pub struct Watchlist {
    store: Entity<QuoteStore>,
    pub scroll: UniformListScrollHandle,
    /// Where the rows were last laid out, in window coordinates, for an
    /// automatic run to move the pointer over them.
    pub rows_bounds: Rc<Cell<Bounds<Pixels>>>,
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
            rows_bounds: Rc::default(),
        }
    }
}

/// The watchlist's columns: a title, a width, and whether the values are
/// numbers, which align to the trailing edge so that they can be compared.
const COLUMNS: [(&str, f32, bool); 9] = [
    ("Symbol", 84., false),
    ("Name", 160., false),
    ("Price", 76., true),
    ("%Chg", 68., true),
    ("Chg", 68., true),
    ("Volume", 76., true),
    ("High", 68., true),
    ("Low", 68., true),
    ("Open", 68., true),
];

fn watchlist_cell(width: f32, numeric: bool) -> gpui::Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .w(px(width))
        .px_2()
        .overflow_hidden()
        .whitespace_nowrap()
        .when(numeric, |this| this.justify_end())
}

impl Render for Watchlist {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let store = self.store.clone();
        let rows_bounds = self.rows_bounds.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .h_8()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_color(theme.muted_foreground)
                    .font_weight(FontWeight::MEDIUM)
                    .children(COLUMNS.iter().map(|(title, width, numeric)| {
                        watchlist_cell(*width, *numeric).child(*title)
                    })),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list("watchlist-rows", SYMBOLS, move |range, _, cx| {
                            let theme = super::theme::theme(cx);
                            let store = store.read(cx);
                            range
                                .map(|ix| watchlist_row(ix, store, theme).into_any_element())
                                .collect::<Vec<_>>()
                        })
                        .flex_1()
                        .track_scroll(&self.scroll),
                    )
                    .child(
                        canvas(move |bounds, _, _| rows_bounds.set(bounds), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    ),
            )
    }
}

fn watchlist_row(ix: usize, store: &QuoteStore, theme: &Theme) -> impl IntoElement {
    let quote = store.quotes[ix];
    let change = quote.last - quote.open;
    let color = change_color(change, theme);
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
        .flex()
        .items_center()
        .h(WATCHLIST_ROW_HEIGHT)
        .border_b_1()
        .border_color(theme.border)
        .hover(|this| this.bg(theme.accent))
        .children(cells.into_iter().zip(COLUMNS).enumerate().map(
            |(column, (text, (_, width, numeric)))| {
                watchlist_cell(width, numeric)
                    .when((2..=4).contains(&column), |this| this.text_color(color))
                    .child(text)
            },
        ))
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
        let theme = theme(cx);
        let store = self.store.read(cx);
        let quote = store.quotes[SELECTED];
        let change = quote.last - quote.open;
        let color = change_color(change, theme);
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
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(store.codes[SELECTED].clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(store.names[SELECTED].clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .text_color(color)
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(price(quote.last)),
                    )
                    .child(format!("{change:+.3}"))
                    .child(format!("{:+.2}%", change / quote.open * 100.)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .text_xs()
                    .children(stats.into_iter().map(|(label, value)| {
                        div()
                            .flex()
                            .justify_between()
                            .w_1_2()
                            .pr_4()
                            .child(div().text_color(theme.muted_foreground).child(label))
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .p_2()
            .text_xs()
            .overflow_hidden()
            .children(self.trades.iter().enumerate().map(|(ix, (last, size))| {
                // A trade at the bid is a sell, at the ask a buy.
                let sell = ix % 3 == 0;
                div()
                    .flex()
                    .justify_between()
                    .child(div().text_color(theme.muted_foreground).child(format!(
                        "10:{:02}:{:02}",
                        ix / 60,
                        ix % 60
                    )))
                    .child(
                        div()
                            .text_color(change_color(if sell { -1. } else { 1. }, theme))
                            .child(price(*last)),
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
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
                .bg(if bid { theme.success } else { theme.danger }.opacity(0.12))
                .child(price(value))
                .child(format!("{}", (ix * 37 + 11) % 400))
        };
        div()
            .size_full()
            .flex()
            .flex_row()
            .gap_1()
            .p_2()
            .text_xs()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .children((0..BOOK_LEVELS).map(|ix| level(ix, true))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let (low, high) = self
            .bars
            .iter()
            .fold((f64::MAX, f64::MIN), |(low, high), bar| {
                (low.min(*bar), high.max(*bar))
            });
        let span = (high - low).max(0.0001);
        div()
            .size_full()
            .flex()
            .flex_row()
            .items_end()
            .gap(px(1.))
            .p_2()
            .overflow_hidden()
            .children(self.bars.iter().enumerate().map(|(ix, bar)| {
                let height = 8. + ((bar - low) / span * 120.) as f32;
                div()
                    .flex_1()
                    .h(px(height))
                    .bg(change_color(if ix % 4 == 0 { -1. } else { 1. }, theme))
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .child(self.title)
            .child(
                div()
                    .text_xs()
                    .text_color(theme(cx).muted_foreground)
                    .child(format!("{} updates", self.received)),
            )
    }
}

/// Market indices along the bottom of the workspace.
struct StatusBar {
    tick: usize,
}

impl Render for StatusBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .size_full()
            .flex()
            .items_center()
            .gap_4()
            .px_3()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .children(
                ["HSI", "HSCEI", "HSTECH", "HSCCI", "SPX", "IXIC"]
                    .iter()
                    .enumerate()
                    .map(|(ix, name)| {
                        let value = 20_000. + (ix * 1_234 + self.tick * 7) as f64 * 0.37;
                        div()
                            .flex()
                            .gap_1()
                            .child(div().text_color(theme.muted_foreground).child(*name))
                            .child(
                                div()
                                    .text_color(change_color(
                                        if ix % 2 == 0 { -1. } else { 1. },
                                        theme,
                                    ))
                                    .child(price(value)),
                            )
                    }),
            )
    }
}
