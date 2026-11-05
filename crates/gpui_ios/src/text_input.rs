//! `UITextInput` conformance for the GPUI view.
//!
//! Bridges UIKit's text system (autocorrect, marked text / IME, dictation,
//! selection handles, the loupe and the edit menu via `UITextInteraction`)
//! to the focused GPUI `InputHandler`. Positions are UTF-16 offsets wrapped
//! in tiny `UITextPosition` / `UITextRange` subclasses.

use crate::window::{WindowState, state_of};
use crate::{CGPoint, CGRect, CGSize, NSRange, id, nil, ns_string, to_str};
use gpui::{Bounds, Pixels, point, px};
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{BOOL, Class, NO, Object, Sel, YES},
    sel, sel_impl,
};
use std::{cell::Cell, ops::Range, sync::OnceLock};

const OFFSET: &str = "gpuiOffset";
const START: &str = "gpuiStart";
const END: &str = "gpuiEnd";
const DELEGATE: &str = "gpuiInputDelegate";
const TOKENIZER: &str = "gpuiTokenizer";
const INTERACTION: &str = "gpuiInteraction";
const MARKED_STYLE: &str = "gpuiMarkedStyle";

thread_local! {
    /// Last (selection, document length) reported to the input delegate.
    static LAST: Cell<Option<(usize, usize, usize)>> = const { Cell::new(None) };
    /// Set while UIKit itself is editing, so we don't echo its change back.
    static PLATFORM_EDIT: Cell<bool> = const { Cell::new(false) };
}

fn position_class() -> &'static Class {
    static CLASS: OnceLock<&'static Class> = OnceLock::new();
    CLASS.get_or_init(|| {
        let mut decl = ClassDecl::new("GPUITextPosition", class!(UITextPosition)).unwrap();
        decl.add_ivar::<usize>(OFFSET);
        decl.register()
    })
}

fn range_class() -> &'static Class {
    static CLASS: OnceLock<&'static Class> = OnceLock::new();
    CLASS.get_or_init(|| {
        let mut decl = ClassDecl::new("GPUITextRange", class!(UITextRange)).unwrap();
        decl.add_ivar::<usize>(START);
        decl.add_ivar::<usize>(END);
        unsafe {
            decl.add_method(sel!(start), range_start as extern "C" fn(&Object, Sel) -> id);
            decl.add_method(sel!(end), range_end as extern "C" fn(&Object, Sel) -> id);
            decl.add_method(sel!(isEmpty), range_empty as extern "C" fn(&Object, Sel) -> BOOL);
        }
        decl.register()
    })
}

extern "C" fn range_start(this: &Object, _: Sel) -> id {
    position(unsafe { *this.get_ivar::<usize>(START) })
}
extern "C" fn range_end(this: &Object, _: Sel) -> id {
    position(unsafe { *this.get_ivar::<usize>(END) })
}
extern "C" fn range_empty(this: &Object, _: Sel) -> BOOL {
    let (s, e) = unsafe { (*this.get_ivar::<usize>(START), *this.get_ivar::<usize>(END)) };
    if s == e { YES } else { NO }
}

fn position(offset: usize) -> id {
    unsafe {
        let p: id = msg_send![position_class(), new];
        (*p).set_ivar::<usize>(OFFSET, offset);
        msg_send![p, autorelease]
    }
}

fn text_range(r: Range<usize>) -> id {
    unsafe {
        let o: id = msg_send![range_class(), new];
        (*o).set_ivar::<usize>(START, r.start.min(r.end));
        (*o).set_ivar::<usize>(END, r.start.max(r.end));
        msg_send![o, autorelease]
    }
}

fn offset_of(p: id) -> Option<usize> {
    if p.is_null() {
        return None;
    }
    unsafe {
        let is: BOOL = msg_send![p, isKindOfClass: position_class()];
        if is == NO {
            return None;
        }
        Some(*(*p).get_ivar::<usize>(OFFSET))
    }
}

fn range_of(r: id) -> Option<Range<usize>> {
    if r.is_null() {
        return None;
    }
    unsafe {
        let is: BOOL = msg_send![r, isKindOfClass: range_class()];
        if is == NO {
            return None;
        }
        Some(*(*r).get_ivar::<usize>(START)..*(*r).get_ivar::<usize>(END))
    }
}

fn doc_len(state: &WindowState) -> usize {
    state
        .with_handler(|h| {
            let mut adjusted = None;
            h.text_for_range(0..usize::MAX / 2, &mut adjusted)
                .map(|t| t.encode_utf16().count())
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

fn selection(state: &WindowState) -> Option<Range<usize>> {
    state.with_handler(|h| h.selected_text_range(false)).flatten().map(|s| s.range)
}

fn to_rect(b: Bounds<Pixels>) -> CGRect {
    CGRect {
        origin: CGPoint { x: f32::from(b.origin.x) as f64, y: f32::from(b.origin.y) as f64 },
        size: CGSize { width: f32::from(b.size.width) as f64, height: f32::from(b.size.height) as f64 },
    }
}

fn rect_for(state: &WindowState, r: Range<usize>) -> CGRect {
    state.with_handler(|h| h.bounds_for_range(r)).flatten().map(to_rect).unwrap_or_default()
}

pub(crate) fn platform_edit(f: impl FnOnce()) {
    PLATFORM_EDIT.with(|c| c.set(true));
    f();
    PLATFORM_EDIT.with(|c| c.set(false));
    // UIKit already knows about its own edit; just re-baseline.
    LAST.with(|c| c.set(None));
}

/// Adds ivars and `UITextInput` methods to the view class being declared.
pub(crate) fn add_methods(decl: &mut ClassDecl) {
    decl.add_ivar::<id>(DELEGATE);
    decl.add_ivar::<id>(TOKENIZER);
    decl.add_ivar::<id>(INTERACTION);
    decl.add_ivar::<id>(MARKED_STYLE);
    unsafe {
        decl.add_method(sel!(hasText), has_text as extern "C" fn(&Object, Sel) -> BOOL);
        decl.add_method(sel!(textInRange:), text_in_range as extern "C" fn(&Object, Sel, id) -> id);
        decl.add_method(sel!(replaceRange:withText:), replace_range as extern "C" fn(&Object, Sel, id, id));
        decl.add_method(sel!(selectedTextRange), selected_range as extern "C" fn(&Object, Sel) -> id);
        decl.add_method(sel!(setSelectedTextRange:), set_selected_range as extern "C" fn(&Object, Sel, id));
        decl.add_method(sel!(markedTextRange), marked_range as extern "C" fn(&Object, Sel) -> id);
        decl.add_method(sel!(markedTextStyle), marked_style as extern "C" fn(&Object, Sel) -> id);
        decl.add_method(sel!(setMarkedTextStyle:), set_marked_style as extern "C" fn(&mut Object, Sel, id));
        decl.add_method(sel!(setMarkedText:selectedRange:), set_marked_text as extern "C" fn(&Object, Sel, id, NSRange));
        decl.add_method(sel!(unmarkText), unmark_text as extern "C" fn(&Object, Sel));
        decl.add_method(sel!(beginningOfDocument), beginning as extern "C" fn(&Object, Sel) -> id);
        decl.add_method(sel!(endOfDocument), end_of_doc as extern "C" fn(&Object, Sel) -> id);
        decl.add_method(sel!(textRangeFromPosition:toPosition:), range_from as extern "C" fn(&Object, Sel, id, id) -> id);
        decl.add_method(sel!(positionFromPosition:offset:), position_offset as extern "C" fn(&Object, Sel, id, isize) -> id);
        decl.add_method(
            sel!(positionFromPosition:inDirection:offset:),
            position_direction as extern "C" fn(&Object, Sel, id, isize, isize) -> id,
        );
        decl.add_method(sel!(comparePosition:toPosition:), compare as extern "C" fn(&Object, Sel, id, id) -> isize);
        decl.add_method(sel!(offsetFromPosition:toPosition:), offset_between as extern "C" fn(&Object, Sel, id, id) -> isize);
        decl.add_method(sel!(inputDelegate), input_delegate as extern "C" fn(&Object, Sel) -> id);
        decl.add_method(sel!(setInputDelegate:), set_input_delegate as extern "C" fn(&mut Object, Sel, id));
        decl.add_method(sel!(tokenizer), tokenizer as extern "C" fn(&mut Object, Sel) -> id);
        decl.add_method(
            sel!(positionWithinRange:farthestInDirection:),
            farthest as extern "C" fn(&Object, Sel, id, isize) -> id,
        );
        decl.add_method(
            sel!(characterRangeByExtendingPosition:inDirection:),
            extend as extern "C" fn(&Object, Sel, id, isize) -> id,
        );
        decl.add_method(
            sel!(baseWritingDirectionForPosition:inDirection:),
            writing_direction as extern "C" fn(&Object, Sel, id, isize) -> isize,
        );
        decl.add_method(
            sel!(setBaseWritingDirection:forRange:),
            set_writing_direction as extern "C" fn(&Object, Sel, isize, id),
        );
        decl.add_method(sel!(firstRectForRange:), first_rect as extern "C" fn(&Object, Sel, id) -> CGRect);
        decl.add_method(sel!(caretRectForPosition:), caret_rect as extern "C" fn(&Object, Sel, id) -> CGRect);
        decl.add_method(sel!(selectionRectsForRange:), selection_rects as extern "C" fn(&Object, Sel, id) -> id);
        decl.add_method(sel!(closestPositionToPoint:), closest as extern "C" fn(&Object, Sel, CGPoint) -> id);
        decl.add_method(
            sel!(closestPositionToPoint:withinRange:),
            closest_within as extern "C" fn(&Object, Sel, CGPoint, id) -> id,
        );
        decl.add_method(sel!(characterRangeAtPoint:), range_at_point as extern "C" fn(&Object, Sel, CGPoint) -> id);
        // Edit menu (UITextInteraction) and hardware-keyboard shortcuts.
        decl.add_method(sel!(copy:), edit_copy as extern "C" fn(&Object, Sel, id));
        decl.add_method(sel!(cut:), edit_cut as extern "C" fn(&Object, Sel, id));
        decl.add_method(sel!(paste:), edit_paste as extern "C" fn(&Object, Sel, id));
        decl.add_method(sel!(selectAll:), edit_select_all as extern "C" fn(&Object, Sel, id));
        decl.add_method(
            sel!(canPerformAction:withSender:),
            can_perform as extern "C" fn(&Object, Sel, Sel, id) -> BOOL,
        );
    }
}

fn selected_text(state: &WindowState) -> Option<(Range<usize>, String)> {
    let r = selection(state).filter(|r| !r.is_empty())?;
    let text = state
        .with_handler(|h| {
            let mut adjusted = None;
            h.text_for_range(r.clone(), &mut adjusted)
        })
        .flatten()?;
    Some((r, text))
}

fn pasteboard() -> id {
    unsafe { msg_send![class!(UIPasteboard), generalPasteboard] }
}

extern "C" fn edit_copy(this: &Object, _: Sel, _: id) {
    if let Some((_, text)) = state_of(this).and_then(selected_text) {
        unsafe {
            let _: () = msg_send![pasteboard(), setString: ns_string(&text)];
        }
    }
}

extern "C" fn edit_cut(this: &Object, _: Sel, _: id) {
    let Some(state) = state_of(this) else { return };
    if let Some((r, text)) = selected_text(state) {
        unsafe {
            let _: () = msg_send![pasteboard(), setString: ns_string(&text)];
        }
        state.with_handler(|h| h.replace_text_in_range(Some(r), ""));
    }
}

extern "C" fn edit_paste(this: &Object, _: Sel, _: id) {
    let Some(state) = state_of(this) else { return };
    let s: id = unsafe { msg_send![pasteboard(), string] };
    if s.is_null() {
        return;
    }
    let text = unsafe { to_str(s) }.to_string();
    state.with_handler(|h| h.replace_text_in_range(None, &text));
}

extern "C" fn edit_select_all(this: &Object, _: Sel, _: id) {
    if let Some(state) = state_of(this) {
        let len = doc_len(state);
        state.with_handler(|h| h.set_selected_text_range(0..len));
    }
}

extern "C" fn can_perform(this: &Object, _: Sel, action: Sel, _: id) -> BOOL {
    let Some(state) = state_of(this) else { return NO };
    let has_selection = selection(state).is_some_and(|r| !r.is_empty());
    let ok = match action.name() {
        "copy:" | "cut:" => has_selection,
        "paste:" => unsafe {
            let has: BOOL = msg_send![pasteboard(), hasStrings];
            has == YES
        },
        "selectAll:" => doc_len(state) > 0,
        _ => false,
    };
    if ok { YES } else { NO }
}

extern "C" fn has_text(this: &Object, _: Sel) -> BOOL {
    match state_of(this) {
        Some(s) if doc_len(s) > 0 => YES,
        _ => NO,
    }
}

extern "C" fn text_in_range(this: &Object, _: Sel, range: id) -> id {
    let (Some(state), Some(r)) = (state_of(this), range_of(range)) else { return nil };
    let text = state
        .with_handler(|h| {
            let mut adjusted = None;
            h.text_for_range(r, &mut adjusted)
        })
        .flatten()
        .unwrap_or_default();
    ns_string(&text)
}

extern "C" fn replace_range(this: &Object, _: Sel, range: id, text: id) {
    let Some(state) = state_of(this) else { return };
    let r = range_of(range);
    let text = unsafe { to_str(text) }.to_string();
    platform_edit(|| {
        state.with_handler(|h| h.replace_text_in_range(r, &text));
    });
}

extern "C" fn selected_range(this: &Object, _: Sel) -> id {
    match state_of(this).and_then(selection) {
        Some(r) => text_range(r),
        None => nil,
    }
}

extern "C" fn set_selected_range(this: &Object, _: Sel, range: id) {
    let (Some(state), Some(r)) = (state_of(this), range_of(range)) else { return };
    platform_edit(|| {
        state.with_handler(|h| h.set_selected_text_range(r));
    });
}

extern "C" fn marked_range(this: &Object, _: Sel) -> id {
    match state_of(this).and_then(|s| s.with_handler(|h| h.marked_text_range()).flatten()) {
        Some(r) => text_range(r),
        None => nil,
    }
}

extern "C" fn marked_style(this: &Object, _: Sel) -> id {
    unsafe { *this.get_ivar::<id>(MARKED_STYLE) }
}

extern "C" fn set_marked_style(this: &mut Object, _: Sel, style: id) {
    unsafe {
        let old = *this.get_ivar::<id>(MARKED_STYLE);
        let style: id = if style.is_null() { nil } else { msg_send![style, copy] };
        this.set_ivar::<id>(MARKED_STYLE, style);
        if !old.is_null() {
            let _: () = msg_send![old, release];
        }
    }
}

extern "C" fn set_marked_text(this: &Object, _: Sel, text: id, sel: NSRange) {
    let Some(state) = state_of(this) else { return };
    let text = unsafe { to_str(text) }.to_string();
    let selected = Some(sel.location..sel.location + sel.length);
    platform_edit(|| {
        state.with_handler(|h| h.replace_and_mark_text_in_range(None, &text, selected));
    });
}

extern "C" fn unmark_text(this: &Object, _: Sel) {
    if let Some(state) = state_of(this) {
        platform_edit(|| {
            state.with_handler(|h| h.unmark_text());
        });
    }
}

extern "C" fn beginning(_: &Object, _: Sel) -> id {
    position(0)
}

extern "C" fn end_of_doc(this: &Object, _: Sel) -> id {
    position(state_of(this).map(doc_len).unwrap_or(0))
}

extern "C" fn range_from(_: &Object, _: Sel, from: id, to: id) -> id {
    match (offset_of(from), offset_of(to)) {
        (Some(a), Some(b)) => text_range(a..b),
        _ => nil,
    }
}

fn shifted(this: &Object, from: id, delta: isize) -> id {
    let Some(start) = offset_of(from) else { return nil };
    let len = state_of(this).map(doc_len).unwrap_or(0) as isize;
    let target = start as isize + delta;
    if target < 0 || target > len { nil } else { position(target as usize) }
}

extern "C" fn position_offset(this: &Object, _: Sel, from: id, delta: isize) -> id {
    shifted(this, from, delta)
}

// UITextLayoutDirection: right 2, left 3, up 4, down 5. Up/down have no
// line model here, so they behave like left/right.
extern "C" fn position_direction(this: &Object, _: Sel, from: id, dir: isize, delta: isize) -> id {
    let delta = if dir == 3 || dir == 4 { -delta } else { delta };
    shifted(this, from, delta)
}

extern "C" fn compare(_: &Object, _: Sel, a: id, b: id) -> isize {
    // NSComparisonResult: ascending -1, same 0, descending 1.
    match (offset_of(a), offset_of(b)) {
        (Some(a), Some(b)) => (a as isize - b as isize).signum(),
        _ => 0,
    }
}

extern "C" fn offset_between(_: &Object, _: Sel, a: id, b: id) -> isize {
    match (offset_of(a), offset_of(b)) {
        (Some(a), Some(b)) => b as isize - a as isize,
        _ => 0,
    }
}

extern "C" fn input_delegate(this: &Object, _: Sel) -> id {
    unsafe { *this.get_ivar::<id>(DELEGATE) }
}

extern "C" fn set_input_delegate(this: &mut Object, _: Sel, delegate: id) {
    // Weak by UIKit contract.
    unsafe { this.set_ivar::<id>(DELEGATE, delegate) }
}

extern "C" fn tokenizer(this: &mut Object, _: Sel) -> id {
    unsafe {
        let existing = *this.get_ivar::<id>(TOKENIZER);
        if !existing.is_null() {
            return existing;
        }
        let t: id = msg_send![class!(UITextInputStringTokenizer), alloc];
        let t: id = msg_send![t, initWithTextInput: this as *mut Object];
        this.set_ivar::<id>(TOKENIZER, t);
        t
    }
}

extern "C" fn farthest(_: &Object, _: Sel, range: id, dir: isize) -> id {
    match range_of(range) {
        Some(r) if dir == 3 || dir == 4 => position(r.start),
        Some(r) => position(r.end),
        None => nil,
    }
}

extern "C" fn extend(this: &Object, _: Sel, pos: id, dir: isize) -> id {
    let Some(p) = offset_of(pos) else { return nil };
    if dir == 3 || dir == 4 {
        text_range(0..p)
    } else {
        text_range(p..state_of(this).map(doc_len).unwrap_or(p))
    }
}

extern "C" fn writing_direction(_: &Object, _: Sel, _: id, _: isize) -> isize {
    0 // NSWritingDirectionLeftToRight
}

extern "C" fn set_writing_direction(_: &Object, _: Sel, _: isize, _: id) {}

extern "C" fn first_rect(this: &Object, _: Sel, range: id) -> CGRect {
    match (state_of(this), range_of(range)) {
        (Some(s), Some(r)) => rect_for(s, r),
        _ => CGRect::default(),
    }
}

extern "C" fn caret_rect(this: &Object, _: Sel, pos: id) -> CGRect {
    let (Some(state), Some(p)) = (state_of(this), offset_of(pos)) else { return CGRect::default() };
    // Measure the character after the caret, or the one before at the end.
    let mut rect = rect_for(state, p..p + 1);
    if rect.size.height == 0.0 && p > 0 {
        rect = rect_for(state, p - 1..p);
        rect.origin.x += rect.size.width;
    }
    rect.size.width = 2.0;
    rect
}

extern "C" fn selection_rects(this: &Object, _: Sel, range: id) -> id {
    unsafe {
        let array: id = msg_send![class!(NSMutableArray), array];
        let (Some(state), Some(r)) = (state_of(this), range_of(range)) else { return array };
        let rect = rect_for(state, r);
        if rect.size.height > 0.0 {
            let sr: id = msg_send![selection_rect_class(), new];
            (*sr).set_ivar::<CGRect>("gpuiRect", rect);
            let _: () = msg_send![array, addObject: sr];
            let _: () = msg_send![sr, release];
        }
        array
    }
}

fn selection_rect_class() -> &'static Class {
    static CLASS: OnceLock<&'static Class> = OnceLock::new();
    CLASS.get_or_init(|| {
        let mut decl = ClassDecl::new("GPUITextSelectionRect", class!(UITextSelectionRect)).unwrap();
        decl.add_ivar::<CGRect>("gpuiRect");
        extern "C" fn rect(this: &Object, _: Sel) -> CGRect {
            unsafe { *this.get_ivar::<CGRect>("gpuiRect") }
        }
        extern "C" fn yes(_: &Object, _: Sel) -> BOOL {
            YES
        }
        extern "C" fn zero(_: &Object, _: Sel) -> isize {
            0
        }
        unsafe {
            decl.add_method(sel!(rect), rect as extern "C" fn(&Object, Sel) -> CGRect);
            decl.add_method(sel!(containsStart), yes as extern "C" fn(&Object, Sel) -> BOOL);
            decl.add_method(sel!(containsEnd), yes as extern "C" fn(&Object, Sel) -> BOOL);
            decl.add_method(sel!(writingDirection), zero as extern "C" fn(&Object, Sel) -> isize);
            decl.add_method(sel!(isVertical), zero as extern "C" fn(&Object, Sel) -> isize);
        }
        decl.register()
    })
}

fn index_at(this: &Object, p: CGPoint) -> Option<usize> {
    state_of(this)?
        .with_handler(|h| h.character_index_for_point(point(px(p.x as f32), px(p.y as f32))))
        .flatten()
}

extern "C" fn closest(this: &Object, _: Sel, p: CGPoint) -> id {
    match index_at(this, p) {
        Some(i) => position(i),
        None => nil,
    }
}

extern "C" fn closest_within(this: &Object, _: Sel, p: CGPoint, range: id) -> id {
    let (Some(i), Some(r)) = (index_at(this, p), range_of(range)) else { return nil };
    position(i.clamp(r.start, r.end))
}

extern "C" fn range_at_point(this: &Object, _: Sel, p: CGPoint) -> id {
    match index_at(this, p) {
        Some(i) => text_range(i..i + 1),
        None => nil,
    }
}

/// Adds a `UITextInteraction` (selection handles, loupe, edit menu) while the
/// view is first responder.
pub(crate) fn attach_interaction(view: id) {
    unsafe {
        let ivar = *(*view).get_ivar::<id>(INTERACTION);
        if !ivar.is_null() {
            return;
        }
        // UITextInteractionModeEditable = 0
        let i: id = msg_send![class!(UITextInteraction), textInteractionForMode: 0isize];
        if i.is_null() {
            return;
        }
        let _: () = msg_send![i, setTextInput: view];
        let _: () = msg_send![view, addInteraction: i];
        let _: id = msg_send![i, retain];
        (*view).set_ivar::<id>(INTERACTION, i);
    }
    LAST.with(|c| c.set(None));
}

pub(crate) fn detach_interaction(view: id) {
    unsafe {
        let i = *(*view).get_ivar::<id>(INTERACTION);
        if i.is_null() {
            return;
        }
        let _: () = msg_send![view, removeInteraction: i];
        let _: () = msg_send![i, release];
        (*view).set_ivar::<id>(INTERACTION, nil);
    }
}

/// After each frame, tells UIKit about selection/content changes the app made
/// itself (typing via key bindings, programmatic edits), so autocorrect state,
/// the caret and the selection handles stay in sync.
pub(crate) fn sync_after_frame(this: &Object, state: &WindowState) {
    if PLATFORM_EDIT.with(|c| c.get()) {
        return;
    }
    let delegate = unsafe { *this.get_ivar::<id>(DELEGATE) };
    if delegate.is_null() {
        return;
    }
    let Some(sel) = selection(state) else { return };
    let now = (sel.start, sel.end, doc_len(state));
    let prev = LAST.with(|c| c.replace(Some(now)));
    if prev == Some(now) || prev.is_none() {
        return;
    }
    let me = this as *const Object as id;
    unsafe {
        if prev.map(|p| p.2) != Some(now.2) {
            let _: () = msg_send![delegate, textWillChange: me];
            let _: () = msg_send![delegate, textDidChange: me];
        } else {
            let _: () = msg_send![delegate, selectionWillChange: me];
            let _: () = msg_send![delegate, selectionDidChange: me];
        }
    }
}
