//! Text measurement that survives recoloring, shaping statistics, and glyph painting that works out a run's rendering once.

use crate::{
    Bounds, ContentMask, DecorationRun, FontId, FontRun, FrameCache, GlyphId, Hsla, IsZero,
    LayoutId, LineLayout, LineLayoutCache, LineLayoutIndex, MonochromeSprite, Pixels,
    PlatformTextSystem, Point, RenderGlyphParams, SUBPIXEL_VARIANTS_X, SUBPIXEL_VARIANTS_Y,
    ScaledPixels, SharedString, Size, StrikethroughStyle, SubpixelSprite, TextLayout,
    TextLayoutInner, TextOverflow, TextRun, TextStyle, TransformationMatrix, TruncateFrom,
    UnderlineStyle, WhiteSpace, Window, WindowTextSystem, WrappedLine,
    util::round_half_toward_zero,
};
use anyhow::Result;
use collections::{FxHashMap, FxHasher};
use gpui_util::ResultExt;
use scheduler::Instant;
use smallvec::SmallVec;
use std::{
    borrow::Cow,
    cmp,
    hash::{Hash, Hasher},
    mem,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

// Text measurement.

/// The decorations of a run, which are what shaping splits font runs on.
fn decoration_of(
    run: &TextRun,
) -> (
    Hsla,
    Option<Hsla>,
    Option<UnderlineStyle>,
    Option<StrikethroughStyle>,
) {
    (
        run.color,
        run.background_color,
        run.underline,
        run.strikethrough,
    )
}

/// Hashes everything the *shape* of the text depends on.
///
/// Decoration values are deliberately absent, but the places decoration
/// *changes* are not. Shaping runs against font runs that are split wherever
/// decoration changes, so whether a color boundary falls between two characters
/// decides whether they are allowed to kern or ligate. Recoloring within the
/// same boundaries leaves the geometry alone and can be applied to the shaped
/// lines in place; moving a boundary cannot.
///
/// Wrap width is also absent: it comes from the space Taffy offers the node,
/// which Taffy already keys its own cache on.
pub(crate) fn shaping_key(
    text: &SharedString,
    runs: &[TextRun],
    text_style: &TextStyle,
    font_size: Pixels,
    line_height: Pixels,
) -> u64 {
    let mut hasher = FxHasher::default();
    text.hash(&mut hasher);
    font_size.0.to_bits().hash(&mut hasher);
    line_height.0.to_bits().hash(&mut hasher);
    mem::discriminant(&text_style.white_space).hash(&mut hasher);
    text_style.line_clamp.hash(&mut hasher);
    match &text_style.text_overflow {
        None => 0u8.hash(&mut hasher),
        Some(TextOverflow::Truncate(affix)) => {
            1u8.hash(&mut hasher);
            affix.hash(&mut hasher);
        }
        Some(TextOverflow::TruncateStart(affix)) => {
            2u8.hash(&mut hasher);
            affix.hash(&mut hasher);
        }
        Some(TextOverflow::TruncateMiddle(affix)) => {
            3u8.hash(&mut hasher);
            affix.hash(&mut hasher);
        }
    }
    let mut previous = None;
    for run in runs.iter().filter(|run| run.len > 0) {
        run.len.hash(&mut hasher);
        run.font.hash(&mut hasher);
        // Whether shaping may join this run to the one before it.
        previous
            .replace(decoration_of(run))
            .is_some_and(|previous| previous == decoration_of(run))
            .hash(&mut hasher);
    }
    hasher.finish()
}

/// Hashes the decoration values, which decide only how shaped text is painted.
pub(crate) fn decoration_key(runs: &[TextRun]) -> u64 {
    let mut hasher = FxHasher::default();
    for run in runs.iter().filter(|run| run.len > 0) {
        run.len.hash(&mut hasher);
        run.color.hash(&mut hasher);
        run.background_color.hash(&mut hasher);
        run.underline.hash(&mut hasher);
        run.strikethrough.hash(&mut hasher);
    }
    hasher.finish()
}

impl TextLayout {
    /// Requests the layout of `text`, measured by a closure Taffy keeps
    /// while nothing the text is shaped from changes, and recolored in place
    /// when only its decorations do. See [`shaping_key`].
    pub(crate) fn layout_keyed(
        &mut self,
        text: SharedString,
        runs: Option<Vec<TextRun>>,
        window: &mut Window,
    ) -> LayoutId {
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = window.pixel_snap(
            text_style
                .line_height
                .to_pixels(font_size.into(), window.rem_size()),
        );

        // Plain text is one run, which stays inline: most frames only hash it,
        // and a measurement that has to keep it is the exception.
        let runs: SmallVec<[TextRun; 1]> = if let Some(runs) = runs {
            SmallVec::from_vec(runs)
        } else {
            SmallVec::from_buf([text_style.to_run(text.len())])
        };
        let shaping_key = shaping_key(&text, &runs, &text_style, font_size, line_height);
        let decoration_key = decoration_key(&runs);
        // Truncated text is shaped from a rewritten string whose runs no longer
        // line up with these, so its decorations cannot be replaced in place.
        let truncating = text_style.text_overflow.is_some();
        // Everything the measurement below captures that the shaping key does
        // not: the decorations it shapes with, and, when it truncates, the font
        // it truncates with. Text that does not truncate never reads the font,
        // so building one to hash it would be for nothing.
        let closure_key = {
            let mut hasher = FxHasher::default();
            shaping_key.hash(&mut hasher);
            decoration_key.hash(&mut hasher);
            if truncating {
                text_style.font().hash(&mut hasher);
            }
            hasher.finish()
        };

        let (layout_id, state) = window.request_measured_layout_cached(
            Default::default(),
            shaping_key,
            closure_key,
            self.0.clone(),
            |state| {
                // Lines are in here only when the shaping still stands, in
                // which case recoloring is a matter of replacing what is
                // painted over them. Doing it here rather than through a
                // measurement is the point: a measurement would have had to
                // dirty the node, and the whole tree above it, to run.
                if !truncating
                    && let Some(layout) = state.borrow_mut().as_mut()
                    && layout.decoration_key != decoration_key
                {
                    update_decoration_runs(&mut layout.lines, &runs);
                    layout.decoration_key = decoration_key;
                }

                let element_state = TextLayout(state.clone());

                move |known_dimensions, available_space, window, cx| {
                    let wrap_width = if text_style.white_space == WhiteSpace::Normal {
                        known_dimensions.width.or(match available_space.width {
                            crate::AvailableSpace::Definite(x) => Some(x),
                            _ => None,
                        })
                    } else {
                        None
                    };

                    // Only the width is needed to decide whether the kept
                    // result still answers. Which affix to truncate with, and
                    // from which end, is needed only if we go on to shape, and
                    // most calls here do not.
                    let truncate_width = text_style.text_overflow.as_ref().and_then(|_| {
                        known_dimensions.width.or(match available_space.width {
                            crate::AvailableSpace::Definite(x) => match text_style.line_clamp {
                                Some(max_lines) => Some(x * max_lines),
                                None => Some(x),
                            },
                            _ => None,
                        })
                    });

                    // Only use cached layout if:
                    // 1. We have a cached size
                    // 2. the wrap width is one the cached layout already answers
                    // 3. truncate_width is None (if truncate_width is Some, we need to re-layout
                    //    because the previous layout may have been computed without truncation)
                    // 4. the cached layout was not truncated (a truncated layout answers an
                    //    unconstrained probe with the truncated size, which poisons intrinsic
                    //    sizing with whatever width some earlier measure pass happened to use)
                    //
                    // Taffy asks for a node's intrinsic size before it lays the
                    // node out, so a wrapping leaf is measured unconstrained
                    // and then again at the width it ends up with. Shaping it
                    // twice is only necessary when the width actually bites:
                    // text that already fits wraps nowhere, and the lines
                    // shaped without a wrap width are the same lines.
                    if let Some(text_layout) = element_state.0.borrow().as_ref()
                        && let Some(size) = text_layout.size
                        && (wrap_width.is_none()
                            || wrap_width == text_layout.wrap_width
                            || (text_layout.wrap_width.is_none()
                                && wrap_width.is_some_and(|wrap_width| size.width <= wrap_width)))
                        && truncate_width.is_none()
                        && text_layout.truncate_width.is_none()
                    {
                        window.record_measure_reuse();
                        return size;
                    }

                    let (text, runs) = if let Some(truncate_width) = truncate_width {
                        // Only truncation needs a wrapper, whose font has to be
                        // resolved and whose slot in the pool has to be taken
                        // and handed back; text that is not truncated would
                        // pay for that on every measurement for nothing.
                        let (truncation_affix, truncate_from) =
                            match text_style.text_overflow.clone() {
                                Some(TextOverflow::Truncate(affix)) => (affix, TruncateFrom::End),
                                Some(TextOverflow::TruncateStart(affix)) => {
                                    (affix, TruncateFrom::Start)
                                }
                                Some(TextOverflow::TruncateMiddle(affix)) => {
                                    (affix, TruncateFrom::Middle)
                                }
                                None => (SharedString::default(), TruncateFrom::End),
                            };
                        let mut line_wrapper =
                            cx.text_system().line_wrapper(text_style.font(), font_size);
                        if let Some(max_lines) = text_style.line_clamp
                            && let Some(wrap_width) = wrap_width
                        {
                            line_wrapper.truncate_wrapped_line(
                                text.clone(),
                                wrap_width,
                                max_lines,
                                &truncation_affix,
                                &runs,
                                truncate_from,
                            )
                        } else if let Some(unclipped) = window
                            .text_system()
                            .shape_text(text.clone(), font_size, &runs, None, None)
                            .log_err()
                            && unclipped
                                .iter()
                                .all(|line| line.size(line_height).width <= truncate_width)
                        {
                            // The truncation decision below sums per-character advances,
                            // which overestimates the shaped width (no kerning), truncating
                            // text that fits exactly in its measured width. Skip truncation
                            // whenever the honestly-shaped text fits; the shaping result
                            // comes from the line layout cache when the same text was
                            // already measured untruncated this frame.
                            (text.clone(), Cow::Borrowed(&*runs))
                        } else {
                            line_wrapper.truncate_line(
                                text.clone(),
                                truncate_width,
                                &truncation_affix,
                                &runs,
                                truncate_from,
                            )
                        }
                    } else {
                        (text.clone(), Cow::Borrowed(&*runs))
                    };
                    let len = text.len();

                    let Some(lines) = window
                        .text_system()
                        .shape_text(
                            text,
                            font_size,
                            &runs,
                            wrap_width,            // Wrap if we know the width.
                            text_style.line_clamp, // Limit the number of lines if line_clamp is set.
                        )
                        .log_err()
                    else {
                        element_state.0.borrow_mut().replace(TextLayoutInner {
                            lines: Default::default(),
                            len: 0,
                            decoration_key,
                            line_height,
                            wrap_width,
                            truncate_width,
                            size: Some(Size::default()),
                            bounds: None,
                        });
                        return Size::default();
                    };

                    let mut size: Size<Pixels> = Size::default();
                    for line in &lines {
                        let line_size = line.size(line_height);
                        size.height += line_size.height;
                        size.width = size.width.max(line_size.width).ceil();
                    }

                    element_state.0.borrow_mut().replace(TextLayoutInner {
                        lines,
                        len,
                        decoration_key,
                        line_height,
                        wrap_width,
                        truncate_width,
                        size: Some(size),
                        bounds: None,
                    });

                    size
                }
            },
        );
        // Adopt the cell the measurement wrote into. When the key matched,
        // Taffy may have answered this node's size from cache without measuring
        // at all, and the lines to paint are the ones an earlier frame produced.
        self.0 = state;
        layout_id
    }
}

/// Rewrites the decorations of lines that [`WindowTextSystem::shape_text`]
/// already shaped, leaving the shaping itself untouched.
///
/// Shaping is the expensive half and the only half that decides how much space
/// the text takes; decorations — colors, underlines, strikethroughs — only
/// decide how it is painted. Recoloring text is therefore a matter of replacing
/// these runs rather than shaping it all over again.
///
/// `runs` must still split `lines` exactly as they were split when shaped: the
/// same lengths, the same fonts, and decoration changing in the same places.
/// That last one matters as much as the others, because `shape_text` splits its
/// font runs wherever decoration changes and shapes each separately. Callers
/// establish it by comparing a key over those inputs before calling; see
/// [`shaping_key`].
pub(crate) fn update_decoration_runs(lines: &mut [WrappedLine], runs: &[TextRun]) {
    let mut runs = runs.iter().filter(|run| run.len > 0).cloned().peekable();

    for line in lines.iter_mut() {
        let line_len = line.text.len();
        line.decoration_runs.clear();

        let mut offset = 0;
        while offset < line_len {
            let Some(run) = runs.peek_mut() else {
                log::warn!("`TextRun`s do not cover the entire shaped text");
                break;
            };
            let len_within_line = cmp::min(line_len - offset, run.len);

            if let Some(last_run) = line.decoration_runs.last_mut()
                && last_run.color == run.color
                && last_run.underline == run.underline
                && last_run.strikethrough == run.strikethrough
                && last_run.background_color == run.background_color
            {
                last_run.len += len_within_line as u32;
            } else {
                line.decoration_runs.push(DecorationRun {
                    len: len_within_line as u32,
                    color: run.color,
                    background_color: run.background_color,
                    underline: run.underline,
                    strikethrough: run.strikethrough,
                });
            }

            run.len -= len_within_line;
            if run.len == 0 {
                runs.next();
            }
            offset += len_within_line;
        }

        // Skip the `\n` that separated this line from the next.
        if let Some(run) = runs.peek_mut() {
            run.len -= 1;
            if run.len == 0 {
                runs.next();
            }
        }
    }
}

// The line layout cache.

/// Leaves in `previous` everything the next frame may ask for: what this
/// frame asked for, which is in `current`, and what it did not but something
/// still holds. `current` is left empty.
///
/// Whichever of the two is larger is kept and the other moved into it, so a
/// frame that asked for little costs little, and so does one that asked for
/// everything.
fn carry_over<K: Eq + Hash, V>(
    previous: &mut FxHashMap<Arc<K>, Arc<V>>,
    current: &mut FxHashMap<Arc<K>, Arc<V>>,
) {
    previous.retain(|_, layout| Arc::strong_count(layout) > 1);
    if previous.len() < current.len() {
        std::mem::swap(previous, current);
    }
    previous.extend(current.drain());
}

/// Ends a frame of the line layout cache: what it laid out, in `current`,
/// becomes what the next frame can reuse, in `previous`.
///
/// A line the frame did not ask for is dropped, unless something still
/// holds its layout. That something is usually a retained text node, which
/// answers from the lines it keeps without asking the cache for them, and
/// whose text can reach a different node at any moment — a row sliding into
/// its neighbour's slot — and ask for the same lines there.
pub(crate) fn carry_over_line_layouts(previous: &mut FrameCache, current: &mut FrameCache) {
    // Wrapped lines hold the lines they were wrapped from, so they are
    // swept first, letting a line they were the last to hold go with them.
    carry_over(&mut previous.wrapped_lines, &mut current.wrapped_lines);
    carry_over(
        &mut previous.wrapped_lines_by_hash,
        &mut current.wrapped_lines_by_hash,
    );
    carry_over(&mut previous.lines, &mut current.lines);
    carry_over(&mut previous.lines_by_hash, &mut current.lines_by_hash);

    // The used lists index what this frame laid out, which is what a view
    // reused next frame looks its lines up by.
    mem::swap(&mut previous.used_lines, &mut current.used_lines);
    mem::swap(
        &mut previous.used_wrapped_lines,
        &mut current.used_wrapped_lines,
    );
    mem::swap(
        &mut previous.used_lines_by_hash,
        &mut current.used_lines_by_hash,
    );
    mem::swap(
        &mut previous.used_wrapped_lines_by_hash,
        &mut current.used_wrapped_lines_by_hash,
    );
    current.used_lines.clear();
    current.used_wrapped_lines.clear();
    current.used_lines_by_hash.clear();
    current.used_wrapped_lines_by_hash.clear();
}

impl LineLayoutCache {
    /// Forgets every line laid out so far, so the next frame shapes what it
    /// shows from scratch.
    #[cfg(test)]
    pub(crate) fn forget(&self) {
        *self.previous_frame.lock() = FrameCache::default();
        *self.current_frame.write() = FrameCache::default();
    }
}

impl LineLayoutIndex {
    /// This index, taken from a range that started at `from`, as it falls in
    /// a copy of that range starting at `to`.
    pub(crate) fn shifted(&self, from: &Self, to: &Self) -> Self {
        LineLayoutIndex {
            lines_index: self.lines_index - from.lines_index + to.lines_index,
            wrapped_lines_index: self.wrapped_lines_index - from.wrapped_lines_index
                + to.wrapped_lines_index,
            lines_by_hash_index: self.lines_by_hash_index - from.lines_by_hash_index
                + to.lines_by_hash_index,
            wrapped_lines_by_hash_index: self.wrapped_lines_by_hash_index
                - from.wrapped_lines_by_hash_index
                + to.wrapped_lines_by_hash_index,
        }
    }
}

/// Counts the lines the line layout cache hands to the platform to be shaped,
/// because neither this frame nor the last one had them, and times them.
#[derive(Default)]
pub(crate) struct LineShaping {
    /// Lines handed to the platform to be shaped. See [`LineShaping::stats`].
    lines_shaped: AtomicU64,
    /// Time spent in those calls, in nanoseconds.
    shape_nanos: AtomicU64,
    /// Whether to time shaping, which it does once the stats have been reset.
    shape_timed: AtomicBool,
}

impl LineShaping {
    /// How many lines have been shaped, and how long that took, since the last
    /// [`LineShaping::reset`]. A line answered from the cache is not counted,
    /// so this is the text work the cache failed to save.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn stats(&self) -> (u64, std::time::Duration) {
        (
            self.lines_shaped.load(Ordering::Relaxed),
            std::time::Duration::from_nanos(self.shape_nanos.load(Ordering::Relaxed)),
        )
    }

    /// Zeroes the counters reported by [`LineShaping::stats`], and from then
    /// on times shaping too.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn reset(&self) {
        self.lines_shaped.store(0, Ordering::Relaxed);
        self.shape_nanos.store(0, Ordering::Relaxed);
        self.shape_timed.store(true, Ordering::Relaxed);
    }

    /// Shapes a line the cache does not have, counting it.
    pub(crate) fn shape_line(
        &self,
        platform_text_system: &dyn PlatformTextSystem,
        text: &str,
        font_size: Pixels,
        runs: &[FontRun],
    ) -> LineLayout {
        let started_at = self.shape_timed.load(Ordering::Relaxed).then(Instant::now);
        let layout = platform_text_system.layout_line(text, font_size, runs);
        self.lines_shaped.fetch_add(1, Ordering::Relaxed);
        if let Some(started_at) = started_at {
            self.shape_nanos
                .fetch_add(started_at.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        layout
    }
}

impl WindowTextSystem {
    /// Lines shaped by the platform, and the time that took, since the last
    /// [`Self::reset_shaping_stats`]. Lines answered from the cache do not count.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn shaping_stats(&self) -> (u64, std::time::Duration) {
        self.line_layout_cache.shaping.stats()
    }

    /// Zeroes the counters reported by [`Self::shaping_stats`].
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn reset_shaping_stats(&self) {
        self.line_layout_cache.shaping.reset()
    }

    /// Forgets every line laid out so far. See [`LineLayoutCache::forget`].
    #[cfg(test)]
    pub(crate) fn forget_line_layouts(&self) {
        self.line_layout_cache.forget()
    }
}

// Glyph painting.

/// How the glyphs of a run are rendered: what painting a glyph needs that
/// depends on its run, not on the glyph. See [`Window::glyph_run_rendering`].
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct GlyphRunRendering {
    subpixel_rendering: bool,
    dilation: u8,
}

impl Window {
    /// How the glyphs of a run in `font_id` at `font_size` and in `color` are
    /// rendered, which [`Window::paint_glyph_in_run`] takes so that painting a
    /// line works it out once a run rather than once a glyph: it asks the
    /// window how it is drawn and converts the colour to find its dilation.
    pub(crate) fn glyph_run_rendering(
        &self,
        font_id: FontId,
        font_size: Pixels,
        color: Hsla,
    ) -> GlyphRunRendering {
        GlyphRunRendering {
            subpixel_rendering: self.should_use_subpixel_rendering(font_id, font_size),
            dilation: self.text_system().glyph_dilation_for_color(color),
        }
    }

    /// [`Window::paint_glyph`], for a glyph in a run whose rendering and
    /// snapped content mask the caller has already worked out.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_glyph_in_run(
        &mut self,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
        color: Hsla,
        rendering: GlyphRunRendering,
        content_mask: ContentMask<ScaledPixels>,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        let element_opacity = self.element_opacity();
        let scale_factor = self.scale_factor();
        let glyph_origin = origin.scale(scale_factor);

        let quantized_origin = Point::new(
            round_half_toward_zero(glyph_origin.x.0 * SUBPIXEL_VARIANTS_X as f32)
                / SUBPIXEL_VARIANTS_X as f32,
            round_half_toward_zero(glyph_origin.y.0 * SUBPIXEL_VARIANTS_Y as f32)
                / SUBPIXEL_VARIANTS_Y as f32,
        );
        let subpixel_variant = Point::new(
            (quantized_origin.x.fract() * SUBPIXEL_VARIANTS_X as f32) as u8,
            (quantized_origin.y.fract() * SUBPIXEL_VARIANTS_Y as f32) as u8,
        );
        let integer_origin = quantized_origin.map(|c| ScaledPixels(c.trunc()));
        let GlyphRunRendering {
            subpixel_rendering,
            dilation,
        } = rendering;
        let params = RenderGlyphParams {
            font_id,
            glyph_id,
            font_size,
            subpixel_variant,
            scale_factor,
            is_emoji: false,
            subpixel_rendering,
            dilation,
        };

        let raster_bounds = self.text_system().raster_bounds(&params)?;
        if !raster_bounds.is_zero() {
            let tile = self
                .sprite_atlas
                .get_or_insert_with(&params.clone().into(), &mut || {
                    let (size, bytes) = self.text_system().rasterize_glyph(&params)?;
                    Ok(Some((size, Cow::Owned(bytes))))
                })?
                .expect("Callback above only errors or returns Some");
            let bounds = Bounds {
                origin: integer_origin + raster_bounds.origin.map(Into::into),
                size: tile.bounds.size.map(Into::into),
            };

            if subpixel_rendering {
                self.next_frame.scene.insert_primitive(SubpixelSprite {
                    order: 0,
                    pad: 0,
                    bounds,
                    content_mask,
                    color: color.opacity(element_opacity),
                    tile,
                    transformation: TransformationMatrix::unit(),
                });
            } else {
                self.next_frame.scene.insert_primitive(MonochromeSprite {
                    order: 0,
                    pad: 0,
                    bounds,
                    content_mask,
                    color: color.opacity(element_opacity),
                    tile,
                    transformation: TransformationMatrix::unit(),
                });
            }
        }
        Ok(())
    }
}

/// Paints the glyphs of a line, working out what they share once rather than
/// once a glyph: the snapped content mask, which nothing painted along a line
/// changes, and the rendering of each run, which only changes with the run's
/// font or color.
pub(crate) struct LineGlyphPainter {
    snapped_content_mask: ContentMask<ScaledPixels>,
    run_rendering: Option<(FontId, Hsla, GlyphRunRendering)>,
}

impl LineGlyphPainter {
    pub(crate) fn new(window: &Window) -> Self {
        Self {
            snapped_content_mask: window.snapped_content_mask(),
            run_rendering: None,
        }
    }

    /// [`Window::paint_glyph`], for the next glyph of the line.
    pub(crate) fn paint_glyph(
        &mut self,
        window: &mut Window,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
        color: Hsla,
    ) -> Result<()> {
        let rendering = match self.run_rendering {
            Some((run_font_id, run_color, rendering))
                if run_font_id == font_id && run_color == color =>
            {
                rendering
            }
            _ => {
                let rendering = window.glyph_run_rendering(font_id, font_size, color);
                self.run_rendering = Some((font_id, color, rendering));
                rendering
            }
        };
        window.paint_glyph_in_run(
            origin,
            font_id,
            glyph_id,
            font_size,
            color,
            rendering,
            self.snapped_content_mask,
        )
    }
}
