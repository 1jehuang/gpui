# iOS paste interaction regression (2026-11-15)

## Verified native defect

`character_index_for_point` reports insertion offsets, including zero in an
empty field and document length when tapping after the last character.
Previously `characterRangeAtPoint:` unconditionally turned that offset into
`i..i + 1`. For an empty field this reported `0..1` although
`beginningOfDocument == endOfDocument == 0`. At the end of nonempty text it
also returned a range past `endOfDocument`. Those are invalid document ranges
on the gesture/edit-menu path, independent of whether `paste:` works.

`caretRectForPosition:` also initially requested `0..1` on empty text.
It now requests the actual insertion range first, so handlers that supply
empty-range geometry can anchor UIKit's caret and edit menu. Existing character
geometry fallback is preserved for handlers that do not support insertion
ranges. Point indices and character ranges are bounded by document length.

## Deliberately unchanged

- `edit_paste` already reads the native pasteboard and replaces the selection.
- `UITextInteractionModeEditable` is already zero.
- `take_input_handler` and `set_input_handler` only change a Rust slot. Neither
  detaches the interaction. GPUI sends focus notifications only when active
  text-input state changes after a frame. No speculative lifecycle reset or
  competing long-press recognizer is introduced.
- UIKit recognizers remain non-cancelling/non-delaying so outside-field taps
  can still reach GPUI and blur the field.
- A point outside the handler's field remains `None`, not a fabricated caret
  anywhere in the full-screen view. App-side hit testing must still supply a
  valid zero offset inside an empty field.

## Portable checks

Run the real policy module independently of the iOS-only crate gate:

```sh
rustc --edition=2024 --test crates/gpui_ios/src/fast/text_interaction.rs -o "$JCODE_SCRATCH_DIR/ios-text-interaction-tests"
"$JCODE_SCRATCH_DIR/ios-text-interaction-tests"
script/check-upstream
```

The tests cover empty, end, interior, stale/out-of-bounds offsets, overflow,
and document bounds measured in UTF-16. They do not execute UIKit.

On this checkout the four portable tests passed. The default upstream guard
reported 17 pre-existing violations outside `gpui_ios` (the crate is not in
`UPSTREAM.tracked`). No tracked-upstream files were modified by this fix.
`cargo check -p gpui_ios --target aarch64-apple-ios --offline` was attempted,
but dependency resolution stopped at the uncached `async-process` Git revision
before native code could compile.

## Required device acceptance

On an iPhone or iOS Simulator, copy text in another app, focus an empty composer,
then long-press and choose Paste. Repeat with a nonempty composer, tapping after
the final character and replacing a selection. Verify repeated taps/long presses
after repaint, emoji text, select-all/copy/cut, keyboard hide/refocus, and an
outside-field tap that dismisses focus. The edit menu must remain available and
pasted text must appear once. This Linux host has no UIKit runtime or `xcrun`,
so native gesture/menu acceptance requires the root's Apple build/device
workflow. The cross-check attempt did not reach SDK use or native compilation.
Passing portable tests alone is not device validation.
