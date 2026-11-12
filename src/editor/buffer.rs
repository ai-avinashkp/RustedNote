//! A rope-backed plain-text buffer addressed in **UTF-8 byte offsets**
//! (matching GPUI's text-input / IME conventions) with a simple snapshot-based
//! undo/redo history. UI-framework agnostic and unit-testable on its own.

use ropey::Rope;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// One indent level. Spaces rather than a tab character, so the text lines up
/// the same width wherever it is later opened.
pub const INDENT: &str = "    ";

#[derive(Debug, Clone)]
struct Snapshot {
    text: String,
    cursor: usize,
}

pub struct Buffer {
    rope: Rope,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    dirty: bool,
}

impl Default for Buffer {
    fn default() -> Self {
        Buffer::from_str("")
    }
}

impl Buffer {
    pub fn from_str(text: &str) -> Self {
        Buffer {
            rope: Rope::from_str(text),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            dirty: false,
        }
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.rope.len_bytes() == 0
    }

    /// Number of lines as counted by newline separators (`"a\nb"` -> 2).
    pub fn len_lines(&self) -> usize {
        self.text().split('\n').count()
    }

    pub fn slice(&self, range: Range<usize>) -> String {
        self.rope.byte_slice(range).to_string()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// Replace `range` (byte offsets) with `new_text`; returns the new cursor
    /// byte offset (collapsed at the end of the inserted text).
    pub fn replace(&mut self, range: Range<usize>, new_text: &str) -> usize {
        self.push_undo(range.start);
        let cs = self.rope.byte_to_char(range.start);
        let ce = self.rope.byte_to_char(range.end);
        if ce > cs {
            self.rope.remove(cs..ce);
        }
        if !new_text.is_empty() {
            self.rope.insert(cs, new_text);
        }
        self.dirty = true;
        range.start + new_text.len()
    }

    fn push_undo(&mut self, cursor: usize) {
        self.undo_stack.push(Snapshot {
            text: self.rope.to_string(),
            cursor,
        });
        self.redo_stack.clear();
        const MAX_HISTORY: usize = 500;
        if self.undo_stack.len() > MAX_HISTORY {
            self.undo_stack.remove(0);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Undo the last edit; returns a cursor byte offset to place the caret at.
    pub fn undo(&mut self) -> Option<usize> {
        let snap = self.undo_stack.pop()?;
        self.redo_stack.push(Snapshot {
            text: self.rope.to_string(),
            cursor: snap.cursor,
        });
        self.rope = Rope::from_str(&snap.text);
        self.dirty = true;
        Some(snap.cursor.min(self.rope.len_bytes()))
    }

    pub fn redo(&mut self) -> Option<usize> {
        let snap = self.redo_stack.pop()?;
        self.undo_stack.push(Snapshot {
            text: self.rope.to_string(),
            cursor: snap.cursor,
        });
        self.rope = Rope::from_str(&snap.text);
        self.dirty = true;
        Some(snap.cursor.min(self.rope.len_bytes()))
    }

    /// Byte offset of the grapheme boundary before `offset` (or 0).
    pub fn prev_grapheme(&self, offset: usize) -> usize {
        let s = self.text();
        s.grapheme_indices(true)
            .rev()
            .find_map(|(i, _)| (i < offset).then_some(i))
            .unwrap_or(0)
    }

    /// Byte offset of the grapheme boundary after `offset` (or end of text).
    pub fn next_grapheme(&self, offset: usize) -> usize {
        let s = self.text();
        let len = s.len();
        s.grapheme_indices(true)
            .find_map(|(i, _)| (i > offset).then_some(i))
            .unwrap_or(len)
    }

    /// Start of the previous word (Ctrl+Left semantics: jump to the start of
    /// the word before `offset`, skipping intervening whitespace).
    pub fn prev_word(&self, offset: usize) -> usize {
        let text = self.text();
        let mut prev = 0;
        for (i, seg) in text.split_word_bound_indices() {
            if i >= offset {
                break;
            }
            if !seg.trim().is_empty() {
                prev = i;
            }
        }
        prev
    }

    /// Start of the next word (Ctrl+Right semantics: jump to the start of the
    /// word after `offset`, skipping intervening whitespace).
    pub fn next_word(&self, offset: usize) -> usize {
        let text = self.text();
        for (i, seg) in text.split_word_bound_indices() {
            if i > offset && !seg.trim().is_empty() {
                return i;
            }
        }
        text.len()
    }

    /// Byte range of the logical line containing `offset`, **excluding** the
    /// trailing newline.
    pub fn line_bounds(&self, offset: usize) -> (usize, usize) {
        let text = self.text();
        let offset = offset.min(text.len());
        let start = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let end = text[offset..]
            .find('\n')
            .map(|i| offset + i)
            .unwrap_or(text.len());
        (start, end)
    }

    /// Byte range of the word under `offset` — the double-click selection unit.
    /// A click on whitespace or punctuation selects that run instead, and the
    /// result never crosses a line break. On a boundary between two segments
    /// the one to the right wins, matching every other editor.
    pub fn word_range_at(&self, offset: usize) -> Range<usize> {
        let text = self.text();
        let offset = offset.min(text.len());
        let (ls, le) = self.line_bounds(offset);
        let line = &text[ls..le];
        let rel = offset - ls;
        let mut last = 0..0;
        for (i, seg) in line.split_word_bound_indices() {
            let end = i + seg.len();
            if rel < end {
                return ls + i..ls + end;
            }
            last = i..end;
        }
        // Past the last segment (caret at end of line): take that segment, or
        // collapse onto the caret when the line is empty.
        ls + last.start..ls + last.end
    }

    /// Byte range of the whole line under `offset`, **including** its trailing
    /// newline — the triple-click selection unit.
    pub fn line_range_at(&self, offset: usize) -> Range<usize> {
        let (start, end) = self.line_bounds(offset);
        let len = self.len_bytes();
        start..if end < len { end + 1 } else { end }
    }

    /// The leading whitespace to delete to pull the line holding `offset` back
    /// by one indent level: up to [`INDENT`]'s width in spaces, or a single
    /// leading tab (one tab is already a whole stop). A line with less than a
    /// full level of leading space loses all of it, so repeated outdents always
    /// end flush at the margin. Empty when there is nothing to remove.
    pub fn outdent_range_at(&self, offset: usize) -> Range<usize> {
        let text = self.text();
        let (start, end) = self.line_bounds(offset);
        let line = &text[start..end];
        if line.starts_with('\t') {
            return start..start + 1;
        }
        let spaces = line
            .bytes()
            .take_while(|b| *b == b' ')
            .count()
            .min(INDENT.len());
        start..start + spaces
    }

    /// Byte offsets at which each line starts (always begins with 0).
    pub fn line_starts(&self) -> Vec<usize> {
        let text = self.text();
        let mut starts = vec![0usize];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        starts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_text() {
        let mut b = Buffer::from_str("");
        let c = b.replace(0..0, "hello");
        assert_eq!(b.text(), "hello");
        assert_eq!(c, 5);
        assert!(b.is_dirty());
    }

    #[test]
    fn replace_range() {
        let mut b = Buffer::from_str("hello world");
        let c = b.replace(0..5, "HI");
        assert_eq!(b.text(), "HI world");
        assert_eq!(c, 2);
    }

    #[test]
    fn slice_bytes() {
        let b = Buffer::from_str("hello world");
        assert_eq!(b.slice(6..11), "world");
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut b = Buffer::from_str("");
        b.replace(0..0, "a");
        b.replace(1..1, "b");
        assert_eq!(b.text(), "ab");
        assert_eq!(b.undo(), Some(1));
        assert_eq!(b.text(), "a");
        assert_eq!(b.redo(), Some(1));
        assert_eq!(b.text(), "ab");
    }

    #[test]
    fn line_starts_and_count() {
        let b = Buffer::from_str("ab\ncd\n");
        assert_eq!(b.line_starts(), vec![0, 3, 6]);
        assert_eq!(b.len_lines(), 3);
    }

    #[test]
    fn word_navigation() {
        let b = Buffer::from_str("hello brave world");
        assert_eq!(b.next_word(0), 6); // start of "brave"
        assert_eq!(b.next_word(7), 12); // inside "brave" -> start of "world"
        assert_eq!(b.next_word(13), 17); // inside last word -> end
        assert_eq!(b.prev_word(17), 12); // from end -> start of "world"
        assert_eq!(b.prev_word(12), 6); // at word start -> previous word
        assert_eq!(b.prev_word(3), 0); // inside first word -> 0
    }

    #[test]
    fn word_navigation_across_lines() {
        let b = Buffer::from_str("one\ntwo");
        assert_eq!(b.next_word(0), 4); // skips the newline to "two"
        assert_eq!(b.prev_word(4), 0);
    }

    #[test]
    fn word_range_selects_word_under_offset() {
        let b = Buffer::from_str("hello brave world");
        assert_eq!(b.word_range_at(0), 0..5);
        assert_eq!(b.word_range_at(3), 0..5);
        // A boundary picks the segment to the right...
        assert_eq!(b.word_range_at(5), 5..6); // the space
        assert_eq!(b.word_range_at(6), 6..11); // "brave"
        // ...and at end-of-text there is nothing to the right, so take the last.
        assert_eq!(b.word_range_at(17), 12..17);
    }

    #[test]
    fn word_range_never_crosses_a_line_break() {
        let b = Buffer::from_str("one\ntwo");
        assert_eq!(b.word_range_at(3), 0..3); // end of line 1 stays on line 1
        assert_eq!(b.word_range_at(4), 4..7);
    }

    #[test]
    fn word_range_on_empty_line_collapses() {
        let b = Buffer::from_str("a\n\nb");
        assert_eq!(b.word_range_at(2), 2..2);
    }

    #[test]
    fn line_range_includes_trailing_newline() {
        let b = Buffer::from_str("ab\ncd\nef");
        assert_eq!(b.line_range_at(0), 0..3);
        assert_eq!(b.line_range_at(4), 3..6);
        assert_eq!(b.line_range_at(7), 6..8); // last line: no newline to include
    }

    #[test]
    fn outdent_removes_one_full_level() {
        let b = Buffer::from_str("        deep");
        assert_eq!(b.outdent_range_at(9), 0..4);
    }

    #[test]
    fn outdent_removes_a_partial_level_completely() {
        // Two spaces is less than a full level, so all of it goes.
        let b = Buffer::from_str("  odd");
        assert_eq!(b.outdent_range_at(4), 0..2);
    }

    #[test]
    fn outdent_treats_a_leading_tab_as_one_stop() {
        let b = Buffer::from_str("\ttabbed");
        assert_eq!(b.outdent_range_at(3), 0..1);
    }

    #[test]
    fn outdent_on_an_unindented_line_is_empty() {
        let b = Buffer::from_str("flush");
        assert!(b.outdent_range_at(2).is_empty());
        let b = Buffer::from_str("");
        assert!(b.outdent_range_at(0).is_empty());
    }

    #[test]
    fn outdent_works_on_a_later_line() {
        let b = Buffer::from_str("first\n      second");
        // Offset 10 is inside "second"; only that line's indent is touched.
        assert_eq!(b.outdent_range_at(10), 6..10);
    }

    #[test]
    fn outdent_ignores_whitespace_after_the_indent() {
        let b = Buffer::from_str("  a  b");
        assert_eq!(b.outdent_range_at(5), 0..2);
    }

    #[test]
    fn grapheme_navigation() {
        let b = Buffer::from_str("abc");
        assert_eq!(b.next_grapheme(0), 1);
        assert_eq!(b.prev_grapheme(2), 1);
        assert_eq!(b.prev_grapheme(0), 0);
        assert_eq!(b.next_grapheme(3), 3);
    }
}
