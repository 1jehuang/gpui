//! Tests of text measurement that survives recoloring.

use crate::{
    App, Bounds, Element, ElementId, GlobalElementId, Hsla, InspectorElementId, IntoElement,
    LayoutId, Pixels, SharedString, TextLayout, TextRun, TextStyle, Window,
    fast::text::{decoration_key, shaping_key},
};
use std::{cell::RefCell, rc::Rc};

/// Replacing decorations in place has to land exactly where reshaping the
/// same text with the same runs would have. If it does not, recolored text
/// paints with the wrong colors on the wrong characters, and nothing in the
/// layout would give it away.
#[test]
fn replacing_decorations_in_place_lands_where_reshaping_would() {
    use crate::{AppContext as _, Empty, TestAppContext, hsla, px};

    let mut cx = TestAppContext::single();
    let window = cx.add_window(|_, _| Empty);
    cx.update_window(window.into(), |_, window, _| {
        let text = SharedString::from("hello\nworld wide");
        let font = window.text_style().font();
        let run = |len, color| TextRun {
            len,
            font: font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let red = hsla(0.0, 1.0, 0.5, 1.0);
        let blue = hsla(0.6, 1.0, 0.5, 1.0);
        let green = hsla(0.3, 1.0, 0.5, 1.0);

        // Same lengths and same boundaries, different colors.
        let before = [run(6, red), run(10, blue)];
        let after = [run(6, green), run(10, red)];

        let font_size = px(14.);
        let system = window.text_system();
        let mut recolored = system
            .shape_text(text.clone(), font_size, &before, None, None)
            .unwrap();
        let reshaped = system
            .shape_text(text, font_size, &after, None, None)
            .unwrap();
        crate::fast::text::update_decoration_runs(&mut recolored, &after);

        assert_eq!(recolored.len(), reshaped.len());
        for (recolored, reshaped) in recolored.iter().zip(reshaped.iter()) {
            assert_eq!(recolored.text, reshaped.text);
            assert_eq!(recolored.decoration_runs, reshaped.decoration_runs);
        }
    })
    .unwrap();
}

#[test]
fn recoloring_keeps_the_shaping_key_but_moving_a_boundary_does_not() {
    use crate::{FontStyle, FontWeight, hsla, px};

    let text = SharedString::from("abcd");
    let font = crate::Font {
        family: "Test".into(),
        features: Default::default(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    };
    let run = |len, color| TextRun {
        len,
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let red = hsla(0.0, 1.0, 0.5, 1.0);
    let blue = hsla(0.6, 1.0, 0.5, 1.0);
    let green = hsla(0.3, 1.0, 0.5, 1.0);
    let style = TextStyle::default();
    let key = |runs: &[TextRun]| shaping_key(&text, runs, &style, px(14.), px(18.));

    let two_colors = [run(2, red), run(2, blue)];
    let two_other_colors = [run(2, green), run(2, red)];
    let one_color = [run(2, red), run(2, red)];

    assert_eq!(
        key(&two_colors),
        key(&two_other_colors),
        "recoloring runs that still differ from each other cannot move a glyph"
    );
    assert_ne!(
        key(&two_colors),
        key(&one_color),
        "runs that used to be shaped apart and now shape together can kern across the join"
    );
    assert_ne!(
        decoration_key(&two_colors),
        decoration_key(&two_other_colors),
        "the colors themselves did change, and what is painted has to follow"
    );
}

const PROBED_TEXT: &str = "hello world wide web";

/// Text in a single color that hands out the layout it ended up with,
/// which is the one it adopted from its node rather than the one it
/// started the frame with.
struct ProbedText {
    color: Hsla,
    probe: Rc<RefCell<Option<TextLayout>>>,
}

impl IntoElement for ProbedText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for ProbedText {
    type RequestLayoutState = TextLayout;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let run = TextRun {
            color: self.color,
            ..window.text_style().to_run(PROBED_TEXT.len())
        };
        let mut layout = TextLayout::default();
        let layout_id = layout.layout(PROBED_TEXT.into(), Some(vec![run]), window, cx);
        self.probe.replace(Some(layout.clone()));
        (layout_id, layout)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
        layout.prepaint(bounds, PROBED_TEXT)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        layout.paint(PROBED_TEXT, window, cx)
    }
}

struct ProbedTextView {
    color: Hsla,
    width: Pixels,
    ellipsis: bool,
    probe: Rc<RefCell<Option<TextLayout>>>,
}

impl crate::Render for ProbedTextView {
    fn render(&mut self, _: &mut Window, _: &mut crate::Context<Self>) -> impl IntoElement {
        use crate::{ParentElement as _, Styled as _, div, prelude::FluentBuilder as _};
        div()
            .w(self.width)
            .when(self.ellipsis, |this| this.text_ellipsis())
            .child(ProbedText {
                color: self.color,
                probe: self.probe.clone(),
            })
    }
}

/// The text a probed layout ended up showing, line by line.
fn probed_lines(probe: &Rc<RefCell<Option<TextLayout>>>) -> Vec<String> {
    let layout = probe.borrow().clone().unwrap();
    let inner = layout.0.borrow();
    inner
        .as_ref()
        .unwrap()
        .lines
        .iter()
        .map(|line| line.text.to_string())
        .collect()
}

/// Only truncating text takes a line wrapper, so the one path that does has
/// to go on truncating: in a box too narrow for it, with an ellipsis, and
/// in full again once the box is wide enough, through the same retained
/// node and the measurement closure it kept.
#[test]
fn text_that_truncates_is_truncated_and_widening_it_shows_it_whole() {
    use crate::{AppContext as _, TestAppContext, hsla, px};

    let mut cx = TestAppContext::single();
    let probe = Rc::new(RefCell::new(None));
    let window = cx.add_window({
        let probe = probe.clone();
        move |_, _| ProbedTextView {
            color: hsla(0., 0., 0., 1.),
            width: px(30.),
            ellipsis: true,
            probe,
        }
    });
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap()
    };

    draw(&mut cx);
    let narrow = probed_lines(&probe);
    assert_eq!(
        narrow.len(),
        1,
        "truncated text stays on one line: {narrow:?}"
    );
    assert!(
        narrow[0].ends_with('…') && narrow[0].len() < PROBED_TEXT.len(),
        "text in a narrow box should be cut short with an ellipsis: {narrow:?}"
    );

    window
        .update(&mut cx, |view, _, cx| {
            view.width = px(1000.);
            cx.notify();
        })
        .unwrap();
    draw(&mut cx);
    assert_eq!(
        probed_lines(&probe),
        vec![PROBED_TEXT.to_string()],
        "text in a box wide enough for it should be shown whole"
    );
}

/// A retained text node keeps the measurement closure it has while nothing
/// it was built from changes. A color is one of those things, though it
/// changes nothing about the measurement: text that is measured again
/// after being recolored, because it was offered a different width, has
/// to be shaped in the new color, not the one the kept closure knew.
#[test]
fn text_measured_again_after_a_recolor_is_shaped_in_the_new_color() {
    use crate::{AppContext as _, TestAppContext, hsla, px};

    let red = hsla(0., 1., 0.5, 1.);
    let blue = hsla(0.66, 1., 0.5, 1.);
    let mut cx = TestAppContext::single();
    let probe = Rc::new(RefCell::new(None));
    let window = cx.add_window({
        let probe = probe.clone();
        move |_, _| ProbedTextView {
            color: red,
            width: px(1000.),
            ellipsis: false,
            probe,
        }
    });
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap()
    };
    let lines = |probe: &Rc<RefCell<Option<TextLayout>>>| {
        let layout = probe.borrow().clone().unwrap();
        let inner = layout.0.borrow();
        let inner = inner.as_ref().unwrap();
        let wraps = inner
            .lines
            .iter()
            .map(|line| line.wrap_boundaries.len())
            .sum::<usize>();
        let colors = inner
            .lines
            .iter()
            .flat_map(|line| line.decoration_runs.iter().map(|run| run.color))
            .collect::<Vec<_>>();
        (wraps, colors)
    };
    let change = |cx: &mut TestAppContext, change: &dyn Fn(&mut ProbedTextView)| {
        window
            .update(cx, |view, _, cx| {
                change(view);
                cx.notify();
            })
            .unwrap();
        draw(cx);
    };

    draw(&mut cx);
    change(&mut cx, &|view| view.color = blue);
    change(&mut cx, &|view| view.width = px(30.));
    let (wraps, colors) = lines(&probe);
    assert!(wraps > 0, "the narrow box should have made the text wrap");
    assert!(
        colors.iter().all(|color| *color == blue),
        "text shaped after the recolor should be blue, got {colors:?}"
    );

    // Measured once more with nothing but the width changed, the text
    // keeps the closure it has, which must still know the new color.
    change(&mut cx, &|view| view.width = px(1000.));
    let (wraps, colors) = lines(&probe);
    assert_eq!(wraps, 0, "the wide box should have unwrapped the text");
    assert!(
        colors.iter().all(|color| *color == blue),
        "text shaped with a kept closure should still be blue, got {colors:?}"
    );
}
