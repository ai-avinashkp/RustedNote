//! A minimal single-line text input (for Find/Replace and the command
//! palette), built on GPUI's `EntityInputHandler` + custom `Element` pattern.
//! Byte-offset addressed; no multi-line, no selection drag — just a caret.

use std::ops::Range;

use gpui::{
    div, fill, point, prelude::*, px, relative, size, App, Bounds, Context, CursorStyle, Element,
    ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable,
    GlobalElementId, LayoutId, MouseButton, MouseDownEvent, Pixels, Point, ShapedLine,
    SharedString, Style, TextRun, UTF16Selection, Window,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::editor::view::os_is_dark;
use crate::prefs::Preferences;
use crate::theme::Palette;

pub struct TextField {
    pub focus_handle: FocusHandle,
    content: String,
    cursor: usize,
    marked: Option<Range<usize>>,
    placeholder: SharedString,
    last_line: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
}

impl TextField {
    pub fn new(cx: &mut Context<Self>, placeholder: impl Into<SharedString>) -> Self {
        TextField {
            focus_handle: cx.focus_handle(),
            content: String::new(),
            cursor: 0,
            marked: None,
            placeholder: placeholder.into(),
            last_line: None,
            last_bounds: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.content.clear();
        self.cursor = 0;
        self.marked = None;
        cx.notify();
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.content = text.to_string();
        self.cursor = self.content.len();
        self.marked = None;
        cx.notify();
    }

    fn prev_grapheme(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(i, _)| (i < offset).then_some(i))
            .unwrap_or(0)
    }

    fn next_grapheme(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(i, _)| (i > offset).then_some(i))
            .unwrap_or(self.content.len())
    }

    fn backspace(&mut self, cx: &mut Context<Self>) {
        if self.cursor > 0 {
            let prev = self.prev_grapheme(self.cursor);
            self.content.replace_range(prev..self.cursor, "");
            self.cursor = prev;
            cx.notify();
        }
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        if self.cursor < self.content.len() {
            let next = self.next_grapheme(self.cursor);
            self.content.replace_range(self.cursor..next, "");
            cx.notify();
        }
    }

    fn offset_from_utf16(&self, off: usize) -> usize {
        let mut u8i = 0;
        let mut u16i = 0;
        for ch in self.content.chars() {
            if u16i >= off {
                break;
            }
            u16i += ch.len_utf16();
            u8i += ch.len_utf8();
        }
        u8i
    }

    fn offset_to_utf16(&self, off: usize) -> usize {
        let mut u16i = 0;
        let mut u8i = 0;
        for ch in self.content.chars() {
            if u8i >= off {
                break;
            }
            u8i += ch.len_utf8();
            u16i += ch.len_utf16();
        }
        u16i
    }

    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(r.start)..self.offset_from_utf16(r.end)
    }

    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(r.start)..self.offset_to_utf16(r.end)
    }
}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let c = self.offset_to_utf16(self.cursor);
        Some(UTF16Selection {
            range: c..c,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked.clone())
            .unwrap_or(self.cursor..self.cursor);
        self.content.replace_range(range.clone(), new_text);
        self.cursor = range.start + new_text.len();
        self.marked = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked.clone())
            .unwrap_or(self.cursor..self.cursor);
        self.content.replace_range(range.clone(), new_text);
        let start = range.start;
        self.marked = if new_text.is_empty() {
            None
        } else {
            Some(start..start + new_text.len())
        };
        self.cursor = start + new_text.len();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_line.as_ref()?;
        let start = self.offset_from_utf16(range_utf16.start);
        let x = bounds.left() + line.x_for_index(start);
        Some(Bounds::new(point(x, bounds.top()), size(px(2.), bounds.bottom() - bounds.top())))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let line = self.last_line.as_ref()?;
        let bounds = self.last_bounds?;
        let idx = line.closest_index_for_x(point.x - bounds.left());
        Some(self.offset_to_utf16(idx))
    }
}

struct FieldElement {
    field: Entity<TextField>,
    palette: Palette,
}

struct FieldPrepaint {
    line: Option<ShapedLine>,
    cursor: Option<gpui::PaintQuad>,
}

impl IntoElement for FieldElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for FieldElement {
    type RequestLayoutState = ();
    type PrepaintState = FieldPrepaint;

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
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> FieldPrepaint {
        let field = self.field.read(cx);
        let focused = field.focus_handle.is_focused(window);
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());

        let (display, color) = if field.content.is_empty() {
            (field.placeholder.clone(), rgb_muted(self.palette))
        } else {
            (SharedString::from(field.content.clone()), style.color)
        };
        let cursor_at = field.cursor;

        let run = TextRun {
            len: display.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(display, font_size, &[run], None);

        let cursor = if focused {
            let x = line.x_for_index(cursor_at.min(line.len()));
            Some(fill(
                Bounds::new(
                    point(bounds.left() + x, bounds.top()),
                    size(px(2.), bounds.bottom() - bounds.top()),
                ),
                gpui::rgb(self.palette.accent),
            ))
        } else {
            None
        };

        FieldPrepaint {
            line: Some(line),
            cursor,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut FieldPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let handle = self.field.read(cx).focus_handle.clone();
        window.handle_input(&handle, ElementInputHandler::new(bounds, self.field.clone()), cx);

        let line = prepaint.line.take().unwrap();
        let _ = line.paint(bounds.origin, window.line_height(), window, cx);
        if let Some(cursor) = prepaint.cursor.take() {
            window.paint_quad(cursor);
        }
        self.field.update(cx, |f, _| {
            f.last_line = Some(line);
            f.last_bounds = Some(bounds);
        });
    }
}

fn rgb_muted(p: Palette) -> gpui::Hsla {
    gpui::rgb(p.muted).into()
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::for_choice(cx.global::<Preferences>().theme.resolve(os_is_dark(window)));
        div()
            .key_context("TextField")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .w_full()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(gpui::rgb(palette.background))
            .text_color(gpui::rgb(palette.text))
            .border_1()
            .border_color(gpui::rgb(palette.border))
            .on_action(cx.listener(|this, _: &TfBackspace, _, cx| this.backspace(cx)))
            .on_action(cx.listener(|this, _: &TfDelete, _, cx| this.delete(cx)))
            .on_action(cx.listener(|this, _: &TfLeft, _, cx| {
                this.cursor = this.prev_grapheme(this.cursor);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &TfRight, _, cx| {
                this.cursor = this.next_grapheme(this.cursor);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &TfHome, _, cx| {
                this.cursor = 0;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &TfEnd, _, cx| {
                this.cursor = this.content.len();
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle);
                    cx.notify();
                }),
            )
            .child(FieldElement {
                field: cx.entity(),
                palette,
            })
    }
}

gpui::actions!(text_field, [TfBackspace, TfDelete, TfLeft, TfRight, TfHome, TfEnd]);
