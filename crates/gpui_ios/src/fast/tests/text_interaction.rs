use super::{caret_range, character_range};

#[test]
fn empty_field_hit_is_an_insertion_range_not_a_phantom_character() {
    assert_eq!(character_range(0, 0), 0..0);
    assert_eq!(caret_range(0, 0), 0..0);
}

#[test]
fn end_of_document_does_not_address_a_nonexistent_character() {
    assert_eq!(character_range(3, 3), 3..3);
    assert_eq!(caret_range(3, 3), 3..3);
    assert_eq!(character_range(1, 3), 1..2);
}

#[test]
fn stale_hit_after_document_shrinks_is_clamped() {
    assert_eq!(character_range(10, 3), 3..3);
    assert_eq!(caret_range(10, 3), 3..3);
    assert_eq!(character_range(usize::MAX, 0), 0..0);
    assert_eq!(
        character_range(usize::MAX, usize::MAX),
        usize::MAX..usize::MAX
    );
}

#[test]
fn every_generated_range_stays_inside_the_utf16_document() {
    for text in ["", "abc", "a🙂b", "é\n日本語"] {
        let len = text.encode_utf16().count();
        for index in 0..=len + 3 {
            for range in [character_range(index, len), caret_range(index, len)] {
                assert!(range.start <= range.end);
                assert!(range.end <= len, "{text:?}: {range:?} exceeds {len}");
            }
        }
    }
}
