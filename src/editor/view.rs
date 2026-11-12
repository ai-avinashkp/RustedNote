//! The editable text view: a multi-line, plain-text editor built on GPUI's
//! custom-`Element` + `EntityInputHandler` (IME) pattern, backed by the
//! rope [`Buffer`]. Addressed throughout in UTF-8 byte offsets.

use std::cell::RefCell;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    actions, div, fill, point, prelude::*, px, rgb, size, App, AvailableSpace, Bounds, Context,
    CursorStyle, DispatchPhase, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, Focusable, GlobalElementId, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, Pixels, Point, ScrollHandle, ScrollWheelEvent, SharedString, Style,
    TextRun, UTF16Selection, UnderlineStyle, Window, WindowAppearance, WrappedLine,
};

use crate::app::CloseSettings;
use crate::prefs::{LineEnding, Preferences, PREFS_FILE};
use crate::state::UiState;
use crate::storage;
use crate::theme::Palette;

use super::buffer::{Buffer, INDENT};

/// True when the OS reports a dark appearance.
pub fn os_is_dark(window: &Window) -> bool {
    matches!(
        window.appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

actions!(
    editor,
    [
        Backspace, Delete, Left, Right, Up, Down, Home, End, SelectLeft, SelectRight, SelectUp,
        SelectDown, SelectAll, Newline, Indent, Outdent, Copy, Cut, Paste, Undo, Redo, Save, SaveAs,
        DocStart, DocEnd, SelectDocStart, SelectDocEnd, CtrlLineUp, CtrlLineDown,
        WordLeft, WordRight, SelectWordLeft, SelectWordRight, DeleteWordPrev, DeleteWordNext,
    ]
);

/// What one mouse gesture selects at a time. Set from the click count on
/// mouse-down and held for the whole drag, so dragging after a double-click
/// extends word by word and after a triple-click line by line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectMode {
    #[default]
    Char,
    Word,
    Line,
}

/// State for a run of consecutive Ctrl+Up / Ctrl+Down presses (see
/// `ctrl_line_move`): the direction they're going (-1 up, +1 down), the
/// logical line the run started from, and how many presses in. Any other
/// caret movement clears this, so a fresh Ctrl+Up/Down always starts its own
/// run from wherever the caret ends up.
#[derive(Debug, Clone, Copy)]
struct CtrlVerticalCycle {
    direction: isize,
    base_row: usize,
    step: usize,
}

// Right-click menu geometry (px). Fixed, so the menu can be clamped into the
// viewport before it is laid out.
const MENU_WIDTH: f32 = 216.;
const MENU_ITEM_H: f32 = 26.;
const MENU_SEP_H: f32 = 9.;
const MENU_PAD_V: f32 = 4.;
/// 7 items + 2 separators + vertical padding + the 1px border on each side.
const MENU_HEIGHT: f32 = 7. * MENU_ITEM_H + 2. * MENU_SEP_H + 2. * MENU_PAD_V + 2.;

/// One row of the right-click menu.
type MenuAction = fn(&mut EditorView, &mut Window, &mut Context<EditorView>);

pub struct EditorView {
    pub focus_handle: FocusHandle,
    pub buffer: Buffer,
    pub path: Option<PathBuf>,

    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    is_selecting: bool,
    /// Granularity of the drag in progress (see [`SelectMode`]).
    select_mode: SelectMode,
    /// The range pinned at drag start — a caret, a word, or a line. The drag
    /// always keeps at least this much selected, in either direction.
    drag_anchor: Range<usize>,
    /// Last pointer position seen during a drag, in window coordinates. The
    /// auto-scroll ticker re-reads it, so holding the pointer still past the
    /// edge keeps scrolling and extending.
    drag_pos: Option<Point<Pixels>>,
    /// True while the auto-scroll ticker task is alive.
    autoscrolling: bool,
    /// Right-click menu anchor, relative to the editor's scroll viewport.
    context_menu: Option<Point<Pixels>>,
    /// Which visual row the caret belongs to when its offset lands exactly on a
    /// wrap boundary: true means the start of the following row, false the end
    /// of the preceding one. See [`row_position`].
    caret_downstream: bool,
    /// In-progress run of consecutive Ctrl+Up / Ctrl+Down presses. See
    /// [`CtrlVerticalCycle`] and `ctrl_line_move`.
    ctrl_vertical: Option<CtrlVerticalCycle>,
    /// Set when the caret has moved and the view should scroll to show it.
    /// Acted on in `TextElement::paint`, not at the point of the move: right
    /// after an edit the cached layout still describes the *previous* text, so
    /// the caret's new position isn't known until the next measure pass.
    pending_caret_reveal: bool,
    /// Target x kept while moving vertically, so Up/Down don't drift columns.
    preferred_x: Option<Pixels>,

    // Layout cache from the last paint, for hit-testing & vertical movement.
    // Each entry is one *logical* line (split on '\n'); when word-wrap is on,
    // a WrappedLine may internally span multiple visual rows.
    scroll_handle: ScrollHandle,
    last_wrapped: Vec<WrappedLine>,
    /// Top y (px, relative to the element's content origin) of each logical line.
    last_line_ys: Vec<f32>,
    last_line_starts: Vec<usize>,
    last_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    /// Width of the line-number gutter (px) in the last paint.
    last_gutter: f32,
    /// Whether the last paint used word-wrapped layout.
    wrap_active: bool,
}

impl EditorView {
    pub fn new(cx: &mut Context<Self>, buffer: Buffer, path: Option<PathBuf>) -> Self {
        // Re-render whenever preferences (theme/font/size/zoom) change.
        cx.observe_global::<Preferences>(|_, cx| cx.notify()).detach();
        EditorView {
            focus_handle: cx.focus_handle(),
            buffer,
            path,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            is_selecting: false,
            select_mode: SelectMode::Char,
            drag_anchor: 0..0,
            drag_pos: None,
            autoscrolling: false,
            context_menu: None,
            caret_downstream: true,
            ctrl_vertical: None,
            pending_caret_reveal: false,
            preferred_x: None,
            scroll_handle: ScrollHandle::new(),
            last_wrapped: Vec::new(),
            last_line_ys: vec![0.],
            last_line_starts: vec![0],
            last_bounds: None,
            line_height: px(21.),
            last_gutter: 0.,
            wrap_active: false,
        }
    }

    // --- selection helpers ------------------------------------------------

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    /// True while a workspace modal is open — the editor must ignore input then.
    fn locked(&self, cx: &Context<Self>) -> bool {
        cx.global::<UiState>().modal_open
    }

    /// Ask for the view to scroll to the caret after the next layout pass.
    fn request_caret_reveal(&mut self) {
        self.pending_caret_reveal = true;
    }

    /// Book-keeping shared by every caret move that isn't vertical: the caret
    /// belongs at the start of a wrapped row (moving right past the end of one
    /// row, or left to the first position of the next, both land there), and
    /// any Ctrl+Up/Down run in progress is finished with.
    fn caret_moved_horizontally(&mut self) {
        self.caret_downstream = true;
        self.ctrl_vertical = None;
    }

    /// The caret's visual row and x within its logical line, honouring
    /// affinity. Returns `None` when the cached layout doesn't cover it.
    fn caret_row_position(&self, row: usize, col: usize) -> Option<(usize, Pixels)> {
        let line = self.last_wrapped.get(row)?;
        row_position(line, col, self.caret_downstream)
    }

    /// Window-space top-left of the caret, from the last painted layout.
    fn caret_point(&self) -> Option<Point<Pixels>> {
        let bounds = self.last_bounds?;
        let cursor = self.cursor_offset();
        let starts = &self.last_line_starts;
        let row = starts.partition_point(|&s| s <= cursor).saturating_sub(1);
        let col = cursor - starts.get(row).copied().unwrap_or(0);
        let (visual_row, x) = self.caret_row_position(row, col)?;
        let line_y = self.last_line_ys.get(row).copied().unwrap_or(0.);
        Some(point(
            bounds.left() + px(self.last_gutter) + x,
            bounds.top() + px(line_y) + self.line_height * visual_row as f32,
        ))
    }

    /// Scroll the least amount that brings the caret back inside the viewport.
    /// Returns true if the view actually moved.
    fn reveal_caret(&mut self) -> bool {
        let viewport = self.scroll_handle.bounds();
        if viewport.size.height <= px(0.) {
            return false;
        }
        let Some(caret) = self.caret_point() else {
            return false;
        };

        // Keep a line of breathing room at the edge, but never so much that a
        // very short viewport has no room left to place the caret in.
        let margin_y = self.line_height.min(viewport.size.height / 4.);
        let mut delta = point(px(0.), px(0.));

        let top = caret.y;
        let bottom = caret.y + self.line_height;
        if top < viewport.top() + margin_y {
            delta.y = viewport.top() + margin_y - top;
        } else if bottom > viewport.bottom() - margin_y {
            delta.y = viewport.bottom() - margin_y - bottom;
        }

        // Horizontally there is only something to do when wrap is off; with
        // wrap on the line never extends past the right edge.
        if !self.wrap_active {
            let margin_x = (viewport.size.width / 8.).min(px(48.));
            if caret.x < viewport.left() + margin_x {
                delta.x = viewport.left() + margin_x - caret.x;
            } else if caret.x > viewport.right() - margin_x {
                delta.x = viewport.right() - margin_x - caret.x;
            }
        }

        if delta.x == px(0.) && delta.y == px(0.) {
            return false;
        }
        let max = self.scroll_handle.max_offset();
        let current = self.scroll_handle.offset();
        let next = point(
            (current.x + delta.x).clamp(-max.width, px(0.)),
            (current.y + delta.y).clamp(-max.height, px(0.)),
        );
        if next == current {
            return false;
        }
        self.scroll_handle.set_offset(next);
        true
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.locked(cx) {
            return;
        }
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.preferred_x = None;
        self.caret_moved_horizontally();
        self.request_caret_reveal();
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.locked(cx) {
            return;
        }
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.preferred_x = None;
        self.caret_moved_horizontally();
        self.request_caret_reveal();
        cx.notify();
    }

    fn row_col(&self, offset: usize) -> (usize, usize) {
        let starts = self.buffer.line_starts();
        let row = starts.partition_point(|&s| s <= offset).saturating_sub(1);
        (row, offset - starts[row])
    }

    fn line_end(&self, row: usize) -> usize {
        let starts = self.buffer.line_starts();
        if row + 1 < starts.len() {
            starts[row + 1] - 1
        } else {
            self.buffer.len_bytes()
        }
    }

    // --- key actions ------------------------------------------------------

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.buffer.prev_grapheme(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.buffer.next_grapheme(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.buffer.prev_grapheme(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.buffer.next_grapheme(self.cursor_offset()), cx);
    }

    // --- word-wise movement / deletion (Ctrl+Left/Right/Backspace/Delete) --

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.buffer.prev_word(self.cursor_offset()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.buffer.next_word(self.cursor_offset()), cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.buffer.prev_word(self.cursor_offset()), cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.buffer.next_word(self.cursor_offset()), cx);
    }

    fn delete_word_prev(&mut self, _: &DeleteWordPrev, window: &mut Window, cx: &mut Context<Self>) {
        if self.locked(cx) {
            return;
        }
        if self.selected_range.is_empty() {
            let prev = self.buffer.prev_word(self.cursor_offset());
            self.selected_range = prev..self.cursor_offset();
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete_word_next(&mut self, _: &DeleteWordNext, window: &mut Window, cx: &mut Context<Self>) {
        if self.locked(cx) {
            return;
        }
        if self.selected_range.is_empty() {
            let next = self.buffer.next_word(self.cursor_offset());
            self.selected_range = self.cursor_offset()..next;
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, false, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, false, cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, true, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, true, cx);
    }

    fn move_vertical(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        if self.locked(cx) || self.last_wrapped.is_empty() {
            return;
        }
        let starts = &self.last_line_starts;
        let cur = self.cursor_offset();
        let row = starts.partition_point(|&s| s <= cur).saturating_sub(1);
        let col = cur - starts.get(row).copied().unwrap_or(0);
        let lh = f32::from(self.line_height);

        let Some(w) = self.last_wrapped.get(row) else { return };
        // Affinity matters here above all: taking gpui's answer for a caret
        // parked on a wrap boundary would report the row *above* the one the
        // caret is really on, so Down would aim at the row it already occupies
        // and never move.
        let (visual_row, caret_x) = row_position(w, col, self.caret_downstream)
            .unwrap_or((0, px(0.)));
        let cur_x = self.preferred_x.unwrap_or(caret_x);

        // Running off either end of the document lands on the very start or
        // end of the text, instead of leaving the caret stranded mid-line on a
        // row with nowhere further to go. Both flags are worked out here, while
        // the line layout is still borrowed; `wrap_boundaries().len()` is the
        // index of this line's last visual row.
        let at_doc_top = row == 0 && visual_row == 0;
        let at_doc_bottom =
            row + 1 == self.last_wrapped.len() && visual_row >= w.wrap_boundaries().len();

        if delta < 0 && at_doc_top {
            if extend {
                self.select_to(0, cx);
            } else {
                self.move_to(0, cx);
            }
            return;
        }
        if delta > 0 && at_doc_bottom {
            let end = self.buffer.len_bytes();
            if extend {
                self.select_to(end, cx);
            } else {
                self.move_to(end, cx);
            }
            return;
        }

        let y_abs = self.last_line_ys.get(row).copied().unwrap_or(0.)
            + visual_row as f32 * lh
            + lh / 2.;
        let target_y = (y_abs + delta as f32 * lh).max(0.);

        // Find the logical line whose visual band contains target_y.
        let mut trow = 0;
        for (i, y) in self.last_line_ys.iter().enumerate() {
            if *y <= target_y {
                trow = i;
            } else {
                break;
            }
        }
        let Some(tw) = self.last_wrapped.get(trow) else { return };
        // Clamp into the target line's own band. Asked for a y past its last
        // visual row, gpui's `closest_index_for_position` bails with `Err(0)` —
        // the line *start* — which would fling the caret backwards instead of
        // leaving it at the end of the document.
        let rows = tw.wrap_boundaries().len() + 1;
        let max_y = (rows as f32 - 0.5) * lh;
        let within = point(
            cur_x,
            px((target_y - self.last_line_ys[trow]).clamp(0., max_y)),
        );
        let idx = match tw.closest_index_for_position(within, self.line_height) {
            Ok(i) => i,
            Err(i) => i,
        };
        let new_off = starts[trow] + idx;

        // The caret belongs on the visual row we aimed at. When the offset we
        // landed on is that row's first, only downstream affinity will draw it
        // there rather than trailing the row above.
        let target_vr = (f32::from(within.y) / lh) as usize;
        let downstream = visual_rows(tw)
            .get(target_vr)
            .is_some_and(|r| target_vr > 0 && r.start == idx);

        if extend {
            self.select_to(new_off, cx);
        } else {
            self.selected_range = new_off..new_off;
            self.selection_reversed = false;
            self.ctrl_vertical = None;
            self.request_caret_reveal();
            cx.notify();
        }
        // Set after the move: `select_to`/`move_to` assume a horizontal step.
        self.caret_downstream = downstream;
        self.preferred_x = Some(cur_x);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let (row, _) = self.row_col(self.cursor_offset());
        let starts = self.buffer.line_starts();
        self.move_to(starts[row], cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let (row, _) = self.row_col(self.cursor_offset());
        self.move_to(self.line_end(row), cx);
    }

    // --- Ctrl+Up / Ctrl+Down: cycle line-start / line-end -----------------

    fn ctrl_line_up(&mut self, _: &CtrlLineUp, _: &mut Window, cx: &mut Context<Self>) {
        self.ctrl_line_move(-1, cx);
    }

    fn ctrl_line_down(&mut self, _: &CtrlLineDown, _: &mut Window, cx: &mut Context<Self>) {
        self.ctrl_line_move(1, cx);
    }

    /// Step through line boundaries one at a time: a lone Home/End would jump
    /// straight to this line's edge and stop being useful, so held-down
    /// Ctrl+Up keeps walking — end of the line above, then its start, then
    /// the end of the line above that, and so on (mirrored for Ctrl+Down:
    /// start of the next line, then its end, then the next line's start...).
    /// Only a run of the *same* direction continues the walk; anything else
    /// (typing, a click, a plain arrow key) starts the next run fresh from
    /// wherever the caret then is — see `caret_moved_horizontally`.
    fn ctrl_line_move(&mut self, direction: isize, cx: &mut Context<Self>) {
        if self.locked(cx) {
            return;
        }
        let starts = self.buffer.line_starts();
        let last_row = starts.len().saturating_sub(1);

        let (base_row, step) = match self.ctrl_vertical {
            Some(cycle) if cycle.direction == direction => {
                let step = cycle.step + 1;
                self.ctrl_vertical = Some(CtrlVerticalCycle { step, ..cycle });
                (cycle.base_row, step)
            }
            _ => {
                let (row, _) = self.row_col(self.cursor_offset());
                self.ctrl_vertical = Some(CtrlVerticalCycle { direction, base_row: row, step: 1 });
                (row, 1)
            }
        };

        // Two presses per line: the far edge first, then the near one. Once
        // past either end of the document, further presses just keep landing
        // on that edge's line rather than erroring off the end.
        let lines_moved = step.div_ceil(2) as isize;
        let target_row = (base_row as isize + direction * lines_moved).clamp(0, last_row as isize) as usize;
        let at_line_end = if direction > 0 { step % 2 == 0 } else { step % 2 == 1 };
        let offset = if at_line_end {
            self.line_end(target_row)
        } else {
            starts[target_row]
        };

        self.selected_range = offset..offset;
        self.selection_reversed = false;
        // A line's end can sit on a wrap boundary; it's the trailing edge of
        // that row, never the start of a row after it within this line.
        self.caret_downstream = !at_line_end;
        self.preferred_x = None;
        self.request_caret_reveal();
        cx.notify();
    }

    // --- whole-document movement (Ctrl+Home / Ctrl+End) -------------------

    fn doc_start(&mut self, _: &DocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.buffer.len_bytes(), cx);
    }

    fn select_doc_start(&mut self, _: &SelectDocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }

    fn select_doc_end(&mut self, _: &SelectDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.buffer.len_bytes(), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if self.locked(cx) {
            return;
        }
        self.selected_range = 0..self.buffer.len_bytes();
        self.selection_reversed = false;
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.locked(cx) {
            return;
        }
        if self.selected_range.is_empty() {
            let prev = self.buffer.prev_grapheme(self.cursor_offset());
            self.selected_range = prev..self.cursor_offset();
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.locked(cx) {
            return;
        }
        if self.selected_range.is_empty() {
            let next = self.buffer.next_grapheme(self.cursor_offset());
            self.selected_range = self.cursor_offset()..next;
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn indent(&mut self, _: &Indent, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, INDENT, window, cx);
    }

    /// Shift+Tab: pull the caret's line back one indent level, taking whatever
    /// leading whitespace is there if it is less than a full level.
    fn outdent(&mut self, _: &Outdent, window: &mut Window, cx: &mut Context<Self>) {
        if self.locked(cx) {
            return;
        }
        let cursor = self.cursor_offset();
        let range = self.buffer.outdent_range_at(cursor);
        if range.is_empty() {
            return;
        }
        let removed = range.len();
        // Delete the indent, then put the caret back where it was on the line
        // rather than leaving it parked at the margin.
        self.selected_range = range.clone();
        self.selection_reversed = false;
        self.replace_text_in_range(None, "", window, cx);
        let restored = cursor.saturating_sub(removed).max(range.start);
        self.selected_range = restored..restored;
        cx.notify();
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            let text = self.buffer.slice(self.selected_range.clone());
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            let text = self.buffer.slice(self.selected_range.clone());
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    pub fn perform_undo(&mut self, cx: &mut Context<Self>) {
        if self.locked(cx) {
            return;
        }
        if let Some(cursor) = self.buffer.undo() {
            self.move_to(cursor, cx);
        }
    }

    pub fn perform_redo(&mut self, cx: &mut Context<Self>) {
        if self.locked(cx) {
            return;
        }
        if let Some(cursor) = self.buffer.redo() {
            self.move_to(cursor, cx);
        }
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.perform_undo(cx);
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.perform_redo(cx);
    }

    // --- tab metadata -----------------------------------------------------

    /// Tab title: file name (or "Untitled"), with a `●` marker when unsaved.
    /// Tab display name (file name, or "Untitled"). Does NOT include the
    /// unsaved-changes dot — callers that truncate long titles should render
    /// that indicator as a separate, non-truncated element (see `is_dirty`),
    /// otherwise it gets clipped away by the ellipsis on long filenames.
    pub fn title(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("Untitled")
            .to_string()
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    /// Whether this tab has work worth saving — what drives the unsaved dot on
    /// the tab, and the confirmation when closing it.
    ///
    /// An *untitled* note that is currently empty has nothing to lose, however
    /// much was typed and deleted along the way, so it reports clean. A note
    /// backed by a file stays dirty when emptied: deleting its contents is a
    /// real change against what is on disk.
    pub fn is_dirty(&self) -> bool {
        self.buffer.is_dirty() && !(self.path.is_none() && self.buffer.is_empty())
    }

    // --- find / replace support ------------------------------------------

    pub fn buffer_text(&self) -> String {
        self.buffer.text()
    }

    /// End of the current selection (search-from anchor).
    pub fn selection_end(&self) -> usize {
        self.selected_range.end
    }

    pub fn selection_start(&self) -> usize {
        self.selected_range.start
    }

    /// Replace the entire buffer contents (used by Replace All).
    pub fn set_all(&mut self, text: &str, cx: &mut Context<Self>) {
        let len = self.buffer.len_bytes();
        self.buffer.replace(0..len, text);
        self.selected_range = 0..0;
        self.selection_reversed = false;
        cx.notify();
    }

    pub fn selected_text(&self) -> String {
        self.buffer.slice(self.selected_range.clone())
    }

    /// Select the byte range `start..end` and reveal it.
    pub fn select_range(&mut self, start: usize, end: usize, cx: &mut Context<Self>) {
        let max = self.buffer.len_bytes();
        self.selected_range = start.min(max)..end.min(max);
        self.selection_reversed = false;
        self.preferred_x = None;
        self.caret_moved_horizontally();
        self.request_caret_reveal();
        cx.notify();
    }

    /// Replace the current selection with `text` (for Replace/Replace-All).
    pub fn replace_current_selection(&mut self, text: &str, cx: &mut Context<Self>) {
        let range = self.selected_range.clone();
        let cursor = self.buffer.replace(range, text);
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.caret_moved_horizontally();
        self.request_caret_reveal();
        cx.notify();
    }

    // --- file operations --------------------------------------------------

    pub fn load_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match std::fs::read(&path) {
            Ok(bytes) => {
                // Plain-text load: honor a UTF-8/UTF-16 BOM, else assume UTF-8.
                // Normalize CRLF → LF so the buffer is always LF internally;
                // the original line ending is reapplied on save per preference.
                let (text, _enc, _had_errors) = encoding_rs::UTF_8.decode(&bytes);
                let text = text.replace("\r\n", "\n");
                self.buffer = Buffer::from_str(&text);
                self.buffer.mark_saved();
                self.path = Some(path);
                self.selected_range = 0..0;
                self.selection_reversed = false;
                self.marked_range = None;
                cx.notify();
            }
            Err(e) => eprintln!("open failed: {e}"),
        }
    }

    pub fn perform_save(&mut self, cx: &mut Context<Self>) {
        match self.path.clone() {
            Some(path) => self.write_to(path, cx),
            None => self.perform_save_as(cx),
        }
    }

    pub fn perform_save_as(&mut self, cx: &mut Context<Self>) {
        let mut dialog = rfd::FileDialog::new().add_filter("Text", &["txt"]);
        if let Some(dir) = self.path.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.save_file() {
            self.write_to(path, cx);
        }
    }

    fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        self.perform_save(cx);
    }

    fn save_as(&mut self, _: &SaveAs, _: &mut Window, cx: &mut Context<Self>) {
        self.perform_save_as(cx);
    }

    fn write_to(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let line_ending = cx.global::<Preferences>().line_ending;
        let mut out = self.buffer.text();
        if line_ending == LineEnding::CrLf {
            out = out.replace('\n', "\r\n");
        }
        match std::fs::write(&path, out) {
            Ok(()) => {
                self.buffer.mark_saved();
                self.path = Some(path);
                cx.notify();
            }
            Err(e) => eprintln!("save failed: {e}"),
        }
    }

    /// Ctrl+scroll-wheel zoom (like VS Code/browsers): wheel up zooms in,
    /// wheel down zooms out. Persists immediately, same as the toolbar/Settings
    /// zoom controls.
    fn on_scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        // A right-click menu is anchored to the text under it; scrolling would
        // leave it pointing at the wrong place, so dismiss it.
        if self.context_menu.take().is_some() {
            cx.notify();
        }
        if !(event.modifiers.control || event.modifiers.platform) {
            return;
        }
        let dy = f32::from(event.delta.pixel_delta(self.line_height).y);
        if dy == 0.0 {
            return;
        }
        cx.update_global::<Preferences, _>(|p, _| {
            if dy > 0.0 {
                p.zoom_in();
            } else {
                p.zoom_out();
            }
        });
        let prefs = cx.global::<Preferences>().clone();
        let _ = storage::save(PREFS_FILE, &prefs);
        cx.notify();
    }

    fn toggle_word_wrap(&mut self, cx: &mut Context<Self>) {
        cx.update_global::<Preferences, _>(|p, _| p.word_wrap = !p.word_wrap);
        let prefs = cx.global::<Preferences>().clone();
        let _ = storage::save(PREFS_FILE, &prefs);
        cx.notify();
    }

    fn toggle_line_ending(&mut self, cx: &mut Context<Self>) {
        cx.update_global::<Preferences, _>(|p, _| {
            p.line_ending = match p.line_ending {
                LineEnding::Lf => LineEnding::CrLf,
                LineEnding::CrLf => LineEnding::Lf,
            };
        });
        let prefs = cx.global::<Preferences>().clone();
        let _ = storage::save(PREFS_FILE, &prefs);
        cx.notify();
    }

    /// (line, column) — both 1-based — plus total chars, for the status bar.
    fn status(&self) -> (usize, usize, usize) {
        let cursor = self.cursor_offset();
        let (row, col) = self.row_col(cursor);
        let char_col = self.buffer.slice(cursor - col..cursor).chars().count();
        (row + 1, char_col + 1, self.buffer.text().chars().count())
    }

    // --- mouse ------------------------------------------------------------

    /// The byte range one click selects at `offset`, for the given granularity.
    fn unit_range(&self, offset: usize, mode: SelectMode) -> Range<usize> {
        match mode {
            SelectMode::Char => offset..offset,
            SelectMode::Word => self.buffer.word_range_at(offset),
            SelectMode::Line => self.buffer.line_range_at(offset),
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.locked(cx) {
            return;
        }
        self.context_menu = None;
        window.focus(&self.focus_handle);

        // The click count cycles through the granularities, so a fourth click
        // drops back to a plain caret rather than sticking on whole lines:
        // 1 char, 2 word, 3 line, 4 char, and round again.
        self.select_mode = match (event.click_count.max(1) - 1) % 3 {
            1 => SelectMode::Word,
            2 => SelectMode::Line,
            _ => SelectMode::Char,
        };
        self.is_selecting = true;
        self.drag_pos = Some(event.position);

        if event.modifiers.shift {
            // Shift-click extends the existing selection: pin whichever end
            // is not moving and treat the click as the drag's far end.
            let anchor = if self.selection_reversed {
                self.selected_range.end
            } else {
                self.selected_range.start
            };
            self.drag_anchor = anchor..anchor;
        } else {
            let idx = self.index_for_mouse_position(event.position);
            self.drag_anchor = self.unit_range(idx, self.select_mode);
        }
        self.drag_to(event.position, cx);
    }

    /// Right-click: show the context menu, moving the caret there first unless
    /// the click landed inside the existing selection (which it then keeps).
    fn on_right_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.locked(cx) {
            return;
        }
        window.focus(&self.focus_handle);
        let idx = self.index_for_mouse_position(event.position);
        if !(idx >= self.selected_range.start && idx < self.selected_range.end) {
            self.selected_range = idx..idx;
            self.selection_reversed = false;
            self.preferred_x = None;
        }
        let origin = self.scroll_handle.bounds().origin;
        self.context_menu = Some(point(
            event.position.x - origin.x,
            event.position.y - origin.y,
        ));
        cx.notify();
    }

    /// End of a drag selection, and with it any auto-scrolling.
    fn end_drag(&mut self) {
        self.is_selecting = false;
        self.drag_pos = None;
    }

    /// Pointer moved with the button held. Driven from a *window*-level
    /// listener (see `TextElement::paint`) rather than the editor div's own
    /// `on_mouse_move`, which gpui gates on the pointer being inside the
    /// element, so a drag leaving the editor used to stop selecting.
    fn on_drag_move(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.is_selecting || self.locked(cx) {
            return;
        }
        self.drag_pos = Some(position);
        self.drag_to(position, cx);
        self.start_autoscroll(cx);
    }

    /// Extend the selection to the offset under `position`, at the granularity
    /// the drag started with, never shrinking past the pinned anchor.
    fn drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let (idx, downstream) = self.index_and_affinity_for_mouse_position(position);
        let unit = self.unit_range(idx, self.select_mode);
        let anchor = self.drag_anchor.clone();
        self.selection_reversed = unit.start < anchor.start;
        self.selected_range = unit.start.min(anchor.start)..unit.end.max(anchor.end);
        self.caret_downstream = downstream;
        self.ctrl_vertical = None;
        self.preferred_x = None;
        cx.notify();
    }

    // --- drag auto-scroll -------------------------------------------------

    /// Scroll step (px per tick) for a pointer held `over` px past an edge:
    /// a small overshoot creeps, a large one flies.
    fn autoscroll_step(over: f32) -> f32 {
        (over / 6.0).clamp(1.5, 40.0)
    }

    /// How far, and which way, to scroll for the current drag position. Zero
    /// while the pointer is still inside the viewport.
    fn autoscroll_delta(&self) -> Point<Pixels> {
        let none = point(px(0.), px(0.));
        let Some(pos) = self.drag_pos else {
            return none;
        };
        let vp = self.scroll_handle.bounds();
        if vp.size.height <= px(0.) {
            return none;
        }
        let mut d = none;
        if pos.y < vp.top() {
            d.y = px(Self::autoscroll_step(f32::from(vp.top() - pos.y)));
        } else if pos.y > vp.bottom() {
            d.y = px(-Self::autoscroll_step(f32::from(pos.y - vp.bottom())));
        }
        if pos.x < vp.left() {
            d.x = px(Self::autoscroll_step(f32::from(vp.left() - pos.x)));
        } else if pos.x > vp.right() {
            d.x = px(-Self::autoscroll_step(f32::from(pos.x - vp.right())));
        }
        d
    }

    /// Apply one auto-scroll step; false when there was nothing left to scroll.
    fn autoscroll_tick(&mut self) -> bool {
        let d = self.autoscroll_delta();
        if d.x == px(0.) && d.y == px(0.) {
            return false;
        }
        let max = self.scroll_handle.max_offset();
        let cur = self.scroll_handle.offset();
        let next = point(
            (cur.x + d.x).clamp(-max.width, px(0.)),
            (cur.y + d.y).clamp(-max.height, px(0.)),
        );
        if next == cur {
            return false;
        }
        self.scroll_handle.set_offset(next);
        true
    }

    /// Keep scrolling, and extending the selection, while the pointer is held
    /// past the viewport edge. Mouse moves alone cannot do this: a drag that
    /// stops moving still has to keep reaching further into the document.
    fn start_autoscroll(&mut self, cx: &mut Context<Self>) {
        let d = self.autoscroll_delta();
        if self.autoscrolling || (d.x == px(0.) && d.y == px(0.)) {
            return;
        }
        self.autoscrolling = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let more = this
                    .update(cx, |this, cx| {
                        if !this.is_selecting || !this.autoscroll_tick() {
                            return false;
                        }
                        if let Some(pos) = this.drag_pos {
                            this.drag_to(pos, cx);
                        }
                        true
                    })
                    .unwrap_or(false);
                if !more {
                    break;
                }
            }
            let _ = this.update(cx, |this, _| this.autoscrolling = false);
        })
        .detach();
    }

    // --- right-click menu -------------------------------------------------

    /// Escape closes the menu; otherwise the key belongs to the workspace
    /// (Settings, focus mode, …), so hand it back rather than swallowing it.
    fn on_escape(&mut self, _: &CloseSettings, _: &mut Window, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    /// One row of the right-click menu. Disabled rows render muted and inert.
    fn menu_item(
        &self,
        label: &'static str,
        hint: &'static str,
        enabled: bool,
        palette: Palette,
        action: MenuAction,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let row = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .h(px(MENU_ITEM_H))
            .px_3()
            .child(label)
            .child(div().text_color(rgb(palette.muted)).child(hint));
        if !enabled {
            return row.text_color(rgb(palette.muted));
        }
        row.cursor_pointer()
            .text_color(rgb(palette.text))
            .hover(|s| s.bg(rgb(palette.selection)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    // The menu sits inside the editor: without this the click
                    // would fall through and move the caret.
                    cx.stop_propagation();
                    this.context_menu = None;
                    window.focus(&this.focus_handle);
                    action(this, window, cx);
                    cx.notify();
                }),
            )
    }

    /// The right-click menu, positioned at `at` (viewport-relative) and clamped
    /// so it always lands fully inside the editor.
    fn render_context_menu(
        &self,
        at: Point<Pixels>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let has_selection = !self.selected_range.is_empty();
        let can_paste = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .is_some_and(|t| !t.is_empty());
        let vp = self.scroll_handle.bounds().size;
        let x = at.x.clamp(px(0.), (vp.width - px(MENU_WIDTH)).max(px(0.)));
        let y = at.y.clamp(px(0.), (vp.height - px(MENU_HEIGHT)).max(px(0.)));

        let separator = || {
            div()
                .h(px(1.))
                .my(px(4.))
                .mx(px(6.))
                .bg(rgb(palette.border))
        };

        div()
            .id("editor-context-menu")
            .occlude()
            .absolute()
            .left(x)
            .top(y)
            .w(px(MENU_WIDTH))
            .flex()
            .flex_col()
            .py(px(MENU_PAD_V))
            .rounded_md()
            .bg(rgb(palette.surface))
            .border_1()
            .border_color(rgb(palette.border))
            .shadow_lg()
            // Fixed metrics: the menu must not grow with the editor's zoom.
            .text_size(px(13.))
            .line_height(px(18.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.context_menu = None;
                cx.notify();
            }))
            .child(self.menu_item(
                "Undo",
                "Ctrl+Z",
                self.buffer.can_undo(),
                palette,
                |this, _, cx| this.perform_undo(cx),
                cx,
            ))
            .child(self.menu_item(
                "Redo",
                "Ctrl+Y",
                self.buffer.can_redo(),
                palette,
                |this, _, cx| this.perform_redo(cx),
                cx,
            ))
            .child(separator())
            .child(self.menu_item(
                "Cut",
                "Ctrl+X",
                has_selection,
                palette,
                |this, window, cx| this.cut(&Cut, window, cx),
                cx,
            ))
            .child(self.menu_item(
                "Copy",
                "Ctrl+C",
                has_selection,
                palette,
                |this, window, cx| this.copy(&Copy, window, cx),
                cx,
            ))
            .child(self.menu_item(
                "Paste",
                "Ctrl+V",
                can_paste,
                palette,
                |this, window, cx| this.paste(&Paste, window, cx),
                cx,
            ))
            .child(self.menu_item(
                "Delete",
                "Del",
                has_selection,
                palette,
                |this, window, cx| this.replace_text_in_range(None, "", window, cx),
                cx,
            ))
            .child(separator())
            .child(self.menu_item(
                "Select All",
                "Ctrl+A",
                !self.buffer.is_empty(),
                palette,
                |this, window, cx| this.select_all(&SelectAll, window, cx),
                cx,
            ))
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        self.index_and_affinity_for_mouse_position(position).0
    }

    /// The byte offset under `position`, plus which visual row the click
    /// belongs to when that offset sits on a wrap boundary — clicking at the
    /// end of a wrapped row should keep the caret there, not bounce it to the
    /// start of the next one.
    fn index_and_affinity_for_mouse_position(&self, position: Point<Pixels>) -> (usize, bool) {
        let Some(bounds) = self.last_bounds else {
            return (0, true);
        };
        let starts = &self.last_line_starts;
        if starts.is_empty() {
            return (0, true);
        }
        let rel_x = (position.x - bounds.left() - px(self.last_gutter)).max(px(0.));
        let rel_y = f32::from((position.y - bounds.top()).max(px(0.)));
        // Find the logical line whose top y is <= rel_y.
        let mut row = 0;
        for (i, y) in self.last_line_ys.iter().enumerate() {
            if *y <= rel_y {
                row = i;
            } else {
                break;
            }
        }
        let Some(w) = self.last_wrapped.get(row) else {
            return (0, true);
        };
        let within = point(rel_x, px(rel_y - self.last_line_ys.get(row).copied().unwrap_or(0.)));
        let idx = match w.closest_index_for_position(within, self.line_height) {
            Ok(i) => i,
            Err(i) => i,
        };
        let clicked_vr = (f32::from(within.y) / f32::from(self.line_height)) as usize;
        let downstream = visual_rows(w)
            .get(clicked_vr)
            .is_some_and(|r| clicked_vr > 0 && r.start == idx);
        (starts.get(row).copied().unwrap_or(0) + idx, downstream)
    }

    // --- utf16 <-> utf8 byte conversions ---------------------------------

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let text = self.buffer.text();
        let mut utf8 = 0;
        let mut utf16 = 0;
        for ch in text.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let text = self.buffer.text();
        let mut utf16 = 0;
        let mut utf8 = 0;
        for ch in text.chars() {
            if utf8 >= offset {
                break;
            }
            utf8 += ch.len_utf8();
            utf16 += ch.len_utf16();
        }
        utf16
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.buffer.slice(range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Ignore all text input while a modal is open.
        if cx.global::<UiState>().modal_open {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        let cursor = self.buffer.replace(range, new_text);
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        self.preferred_x = None;
        self.caret_moved_horizontally();
        self.request_caret_reveal();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cx.global::<UiState>().modal_open {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        let start = range.start;
        let cursor = self.buffer.replace(range, new_text);
        if new_text.is_empty() {
            self.marked_range = None;
        } else {
            self.marked_range = Some(start..cursor);
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .map(|r| start + r.start..start + r.end)
            .unwrap_or(cursor..cursor);
        self.selection_reversed = false;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let start = self.offset_from_utf16(range_utf16.start);
        let starts = &self.last_line_starts;
        let row = starts.partition_point(|&s| s <= start).saturating_sub(1);
        let col = start - starts.get(row).copied().unwrap_or(0);
        let line_y = self.last_line_ys.get(row).copied().unwrap_or(0.);
        let gutter = px(self.last_gutter);
        let w = self.last_wrapped.get(row)?;
        let pos = w.position_for_index(col, self.line_height)?;
        Some(Bounds::new(
            point(bounds.left() + gutter + pos.x, bounds.top() + px(line_y) + pos.y),
            size(px(2.), self.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let idx = self.index_for_mouse_position(point).min(self.buffer.len_bytes());
        Some(self.offset_to_utf16(idx))
    }
}

// -------------------------------------------------------------------------
// The custom element that shapes and paints the multi-line text.
// -------------------------------------------------------------------------

/// Byte ranges (line-relative) of each visual row within a wrapped line. A
/// non-wrapped line yields a single row covering the whole line.
fn visual_rows(line: &WrappedLine) -> Vec<Range<usize>> {
    let mut rows = Vec::with_capacity(line.wrap_boundaries().len() + 1);
    let mut start = 0usize;
    for wb in line.wrap_boundaries() {
        let run = &line.unwrapped_layout.runs[wb.run_ix];
        let glyph = &run.glyphs[wb.glyph_ix];
        let end = glyph.index;
        rows.push(start..end);
        start = end;
    }
    rows.push(start..line.len());
    rows
}

/// Where the caret sits within a wrapped line: its visual row, and its x
/// within that row.
///
/// A byte offset that falls exactly on a wrap boundary is ambiguous — it is
/// both the end of one visual row and the start of the next — so which one is
/// meant depends on how the caret got there (its *affinity*). gpui's own
/// `position_for_index` always answers with the earlier row, which is why the
/// caret used to appear trailing the end of the row above, and why Down got
/// stuck: vertical movement kept deciding it was already on that upper row and
/// re-targeting the very offset it started from.
fn row_position(
    line: &WrappedLine,
    col: usize,
    downstream: bool,
) -> Option<(usize, Pixels)> {
    let rows = visual_rows(line);
    let mut chosen: Option<usize> = None;
    for (i, r) in rows.iter().enumerate() {
        if col >= r.start && col <= r.end {
            // On a boundary two rows match; take the later one only when the
            // caret is downstream of it.
            if chosen.is_none() || downstream {
                chosen = Some(i);
            }
        }
    }
    let i = chosen?;
    let x = line.unwrapped_layout.x_for_index(col)
        - line.unwrapped_layout.x_for_index(rows[i].start);
    Some((i, x))
}

/// Shared geometry produced by the measure pass, consumed by `prepaint`/`paint`.
#[derive(Default)]
struct Measured {
    lines: Vec<WrappedLine>,
    line_starts: Vec<usize>,
    /// Top y (px, relative to the element's content origin) of each logical line.
    line_ys: Vec<f32>,
    /// Width reserved for line numbers; zero when they're switched off.
    gutter: f32,
    show_numbers: bool,
    wrap_active: bool,
    line_height: Pixels,
}

struct TextElement {
    view: Entity<EditorView>,
    measured: Rc<RefCell<Option<Measured>>>,
}

struct PrepaintState {
    selections: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let view_entity = self.view.clone();
        let measured = self.measured.clone();

        // Style-derived values MUST be captured here, at `request_layout`
        // time — the div's `.text_color(...)` cascade is active now. The
        // measure closure below runs later, during taffy's own layout
        // resolution, outside that cascade; calling `window.text_style()`
        // there returns the window's *default* style (so text would render in
        // the default color, not the theme's — e.g. always dark, even in a
        // dark theme). Capture once, move the values in.
        let text_style = window.text_style();
        let font = text_style.font();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let text_color = text_style.color;
        let line_height = window.line_height();

        // Gutter width: sized to the digit count of the total line count. This
        // doesn't depend on wrap width, so it's computed once, up front. With
        // line numbers off there is no gutter at all, and the text starts at
        // the container's own padding.
        let show_numbers = cx.global::<Preferences>().show_line_numbers;
        let gutter = if show_numbers {
            let total_lines = view_entity.read(cx).buffer.len_lines().max(1);
            let digits = total_lines.to_string().len().max(2);
            let sample: SharedString = "0".repeat(digits).into();
            let sample_run = TextRun {
                len: sample.len(),
                font: font.clone(),
                color: text_color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let sample_line = window
                .text_system()
                .shape_line(sample, font_size, &[sample_run], None);
            f32::from(sample_line.width) + 24.0
        } else {
            0.0
        };

        let layout_id = window.request_measured_layout(Style::default(), {
            move |_known_dimensions, available_space, window, cx| {
                let (text, marked, wrap_on) = {
                    let view = view_entity.read(cx);
                    (
                        view.buffer.text(),
                        view.marked_range.clone(),
                        cx.global::<Preferences>().word_wrap,
                    )
                };

                let wrap_width = if wrap_on {
                    match available_space.width {
                        AvailableSpace::Definite(w) => Some((w - px(gutter)).max(px(20.))),
                        _ => None,
                    }
                } else {
                    None
                };

                let mut line_starts = vec![0usize];
                for (i, b) in text.bytes().enumerate() {
                    if b == b'\n' {
                        line_starts.push(i + 1);
                    }
                }

                let full: SharedString = text.into();
                let base_run = TextRun {
                    len: full.len(),
                    font: font.clone(),
                    color: text_color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let runs: Vec<TextRun> = match &marked {
                    Some(m) if m.end > m.start && m.end <= full.len() => vec![
                        TextRun { len: m.start, ..base_run.clone() },
                        TextRun {
                            len: m.end - m.start,
                            underline: Some(UnderlineStyle {
                                color: Some(text_color),
                                thickness: px(1.),
                                wavy: false,
                            }),
                            ..base_run.clone()
                        },
                        TextRun { len: full.len() - m.end, ..base_run.clone() },
                    ]
                    .into_iter()
                    .filter(|r| r.len > 0)
                    .collect(),
                    _ => vec![base_run],
                };

                let lines = window
                    .text_system()
                    .shape_text(full, font_size, &runs, wrap_width, None)
                    .unwrap_or_default()
                    .into_vec();

                let mut line_ys = Vec::with_capacity(lines.len());
                let mut y = 0f32;
                let mut max_width = px(0.);
                for line in &lines {
                    line_ys.push(y);
                    let sz = line.size(line_height);
                    y += f32::from(sz.height);
                    max_width = max_width.max(sz.width);
                }

                let total_width = if wrap_on {
                    match available_space.width {
                        AvailableSpace::Definite(w) => w,
                        _ => max_width + px(gutter),
                    }
                } else {
                    max_width + px(gutter)
                };

                measured.borrow_mut().replace(Measured {
                    lines,
                    line_starts,
                    line_ys,
                    gutter,
                    show_numbers,
                    wrap_active: wrap_on,
                    line_height,
                });

                size(total_width, px(y))
            }
        });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let (selected, cursor, downstream, focused) = {
            let view = self.view.read(cx);
            (
                view.selected_range.clone(),
                view.cursor_offset(),
                view.caret_downstream,
                view.focus_handle.is_focused(window),
            )
        };
        let palette =
            Palette::for_choice(cx.global::<Preferences>().theme.resolve(os_is_dark(window)));

        let cache = self.measured.borrow();
        let Some(m) = cache.as_ref() else {
            return PrepaintState { selections: Vec::new(), cursor: None };
        };

        let mut selections: Vec<PaintQuad> = Vec::new();
        let mut cursor_quad: Option<PaintQuad> = None;

        for (row, line) in m.lines.iter().enumerate() {
            let line_start = m.line_starts.get(row).copied().unwrap_or(0);
            let line_end = line_start + line.len();
            let line_y = m.line_ys.get(row).copied().unwrap_or(0.);
            let rows = visual_rows(line);

            if !selected.is_empty() && selected.start <= line_end && selected.end >= line_start {
                let sel_start_rel = selected.start.max(line_start) - line_start;
                let sel_end_rel = selected.end.min(line_end) - line_start;
                let last_row = rows.len().saturating_sub(1);
                for (ri, r) in rows.iter().enumerate() {
                    let ov_start = sel_start_rel.max(r.start);
                    let ov_end = sel_end_rel.min(r.end);
                    if ov_start >= ov_end {
                        continue;
                    }
                    let row_start_x = line.unwrapped_layout.x_for_index(r.start);
                    let x0 = line.unwrapped_layout.x_for_index(ov_start) - row_start_x;
                    let mut x1 = line.unwrapped_layout.x_for_index(ov_end) - row_start_x;
                    if ov_end == r.end && sel_end_rel > r.end && ri != last_row {
                        // Selection continues onto the next visual row: fill to the edge.
                        x1 = line.unwrapped_layout.width - row_start_x;
                    } else if ri == last_row && selected.end > line_end {
                        // Selection continues past this logical line (e.g. the newline).
                        x1 += px(6.);
                    }
                    let y0 = bounds.top() + px(line_y) + m.line_height * ri as f32;
                    selections.push(fill(
                        Bounds::from_corners(
                            point(bounds.left() + px(m.gutter) + x0, y0),
                            point(bounds.left() + px(m.gutter) + x1, y0 + m.line_height),
                        ),
                        rgb(palette.selection),
                    ));
                }
            }

            if selected.is_empty() && cursor >= line_start && cursor <= line_end {
                let col = cursor - line_start;
                if let Some((visual_row, cx_off)) = row_position(line, col, downstream) {
                    let x = bounds.left() + px(m.gutter) + cx_off;
                    let y = bounds.top() + px(line_y) + m.line_height * visual_row as f32;
                    cursor_quad = Some(fill(
                        Bounds::new(point(x, y), size(px(2.), m.line_height)),
                        rgb(palette.accent),
                    ));
                }
            }
        }

        PrepaintState {
            selections,
            cursor: if focused { cursor_quad } else { None },
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.view.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );

        // Drag-selection is tracked here, at the window level, rather than with
        // `on_mouse_move`/`on_mouse_up` on the editor div: gpui only delivers
        // those while the pointer is inside the element, so a drag that ran off
        // the editor (into the status bar, the tab strip, or off-window) stopped
        // extending the selection instead of scrolling on.
        window.on_mouse_event({
            let view = self.view.clone();
            move |event: &MouseMoveEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble && event.dragging() {
                    view.update(cx, |this, cx| this.on_drag_move(event.position, cx));
                }
            }
        });
        window.on_mouse_event({
            let view = self.view.clone();
            move |event: &MouseUpEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                    view.update(cx, |this, _| this.end_drag());
                }
            }
        });

        for quad in prepaint.selections.drain(..) {
            window.paint_quad(quad);
        }

        let palette =
            Palette::for_choice(cx.global::<Preferences>().theme.resolve(os_is_dark(window)));

        let cache = self.measured.borrow();
        if let Some(m) = cache.as_ref() {
            for (row, line) in m.lines.iter().enumerate() {
                let line_y = m.line_ys.get(row).copied().unwrap_or(0.);
                let origin = point(bounds.left() + px(m.gutter), bounds.top() + px(line_y));
                let _ = line.paint(origin, m.line_height, gpui::TextAlign::default(), Some(bounds), window, cx);

                if !m.show_numbers {
                    continue;
                }

                // Line number, right-aligned in the gutter.
                let num: SharedString = (row + 1).to_string().into();
                let num_run = TextRun {
                    len: num.len(),
                    font: window.text_style().font(),
                    color: rgb(palette.gutter).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let font_size = window.text_style().font_size.to_pixels(window.rem_size());
                let shaped = window
                    .text_system()
                    .shape_line(num, font_size, &[num_run], None);
                let num_x = bounds.left() + px(m.gutter) - shaped.width - px(10.);
                let _ = shaped.paint(point(num_x, bounds.top() + px(line_y)), m.line_height, window, cx);
            }
        }
        drop(cache);

        if let Some(cursor) = prepaint.cursor.take() {
            window.paint_quad(cursor);
        }

        let cache = self.measured.borrow_mut().take();
        let reveal_now = self.view.update(cx, |v, _| {
            if let Some(m) = cache {
                v.last_wrapped = m.lines;
                v.last_line_starts = m.line_starts;
                v.last_line_ys = m.line_ys;
                v.last_gutter = m.gutter;
                v.wrap_active = m.wrap_active;
                v.line_height = m.line_height;
            }
            // The element's bounds move with every scroll, so they are taken
            // fresh each paint whether or not the text needed re-measuring.
            v.last_bounds = Some(bounds);

            // Only now does the layout describe the current text, so this is
            // the first moment the caret's position can be worked out.
            let pending = v.pending_caret_reveal;
            v.pending_caret_reveal = false;
            pending
        });

        // ...but the scrolling itself has to wait until we are out of the paint
        // phase. The frame being drawn has already positioned its content, so
        // moving the offset now changes nothing on screen, and a `notify` from
        // inside paint doesn't schedule another frame to fix that. Deferring
        // hands the work to the next effect cycle, where an ordinary notify
        // produces an ordinary redraw.
        if reveal_now {
            let view = self.view.clone();
            cx.defer(move |cx| {
                view.update(cx, |v, cx| {
                    if v.reveal_caret() {
                        cx.notify();
                    }
                });
            });
        }
    }
}

impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let system_dark = os_is_dark(window);
        let prefs = cx.global::<Preferences>();
        let palette = Palette::for_choice(prefs.theme.resolve(system_dark));
        let font_family: SharedString = prefs.font_family.clone().into();
        let font_size = px(prefs.effective_font_size());
        let (line, col, chars) = self.status();
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("Untitled")
            .to_string();
        let dirty = if self.is_dirty() { " ●" } else { "" };
        let le_label = match prefs.line_ending {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
        };
        let wrap_label = if prefs.word_wrap { "Wrap: On" } else { "Wrap: Off" };
        let word_wrap = prefs.word_wrap;

        let editor_area = div()
            .key_context("Editor")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .flex_1()
            .overflow_hidden()
            .bg(rgb(palette.background))
            .text_color(rgb(palette.text))
            .font_family(font_family)
            .text_size(font_size)
            .line_height(font_size * 1.4)
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::delete_word_prev))
            .on_action(cx.listener(Self::delete_word_next))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::select_doc_start))
            .on_action(cx.listener(Self::select_doc_end))
            .on_action(cx.listener(Self::ctrl_line_up))
            .on_action(cx.listener(Self::ctrl_line_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::indent))
            .on_action(cx.listener(Self::outdent))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::on_escape))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_mouse_down))
            // Drag tracking (move + release) is registered on the *window* in
            // `TextElement::paint`, so it survives the pointer leaving the
            // editor's bounds.
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .child({
                let scroller = div()
                    .id("editor-scroll")
                    .size_full()
                    .track_scroll(&self.scroll_handle);
                // Wrap on: vertical scroll only. Wrap off: both axes.
                let scroller = if word_wrap {
                    scroller.overflow_y_scroll()
                } else {
                    scroller.overflow_scroll()
                };
                scroller.child(
                    div()
                        .px_3()
                        .py_2()
                        .min_h_full()
                        .child(TextElement {
                            view: cx.entity(),
                            measured: Rc::new(RefCell::new(None)),
                        }),
                )
            });

        let editor_area = match self.context_menu {
            Some(at) => editor_area.child(self.render_context_menu(at, palette, cx)),
            None => editor_area,
        };

        let status_bar = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .px_3()
            .py_1()
            .text_size(px(12.))
            .bg(rgb(palette.surface))
            .text_color(rgb(palette.muted))
            .border_t_1()
            .border_color(rgb(palette.border))
            .child(format!("{name}{dirty}"))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_4()
                    .child(format!("Ln {line}, Col {col}"))
                    .child(format!("{chars} chars"))
                    .child(
                        div()
                            .cursor_pointer()
                            .child(wrap_label)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.toggle_word_wrap(cx)),
                            ),
                    )
                    .child("UTF-8")
                    .child(
                        div()
                            .cursor_pointer()
                            .child(le_label)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.toggle_line_ending(cx)),
                            ),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(palette.background))
            .child(editor_area)
            .child(status_bar)
    }
}
