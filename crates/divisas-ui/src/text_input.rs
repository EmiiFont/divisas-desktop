//! Minimal single-line text input for GPUI.
//!
//! A trimmed-down version of gpui's `input.rs` example: caret-only (no
//! selection), typing, backspace, arrow keys, paste, and click-to-focus.
//! Supports masked rendering for password fields.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId,
    MouseButton, MouseDownEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style,
    TextRun, UTF16Selection, Window, actions, div, fill, hsla, prelude::*, px, relative,
    size, white,
};

actions!(
    text_input,
    [Backspace, Delete, Left, Right, Home, End, Paste, Submit]
);

/// A single-line text input entity.
pub struct TextInput {
    focus_handle: FocusHandle,
    pub content: SharedString,
    pub placeholder: SharedString,
    pub masked: bool,
    caret: usize,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
}

impl TextInput {
    pub fn new(content: &str, placeholder: &str, masked: bool, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: SharedString::from(content.to_string()),
            placeholder: SharedString::from(placeholder.to_string()),
            masked,
            caret: content.len(),
            last_layout: None,
            last_bounds: None,
        }
    }

    #[allow(dead_code)]
    pub fn reset(&mut self, content: &str) {
        self.content = SharedString::from(content.to_string());
        self.caret = content.len();
        self.last_layout = None;
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.caret = self.previous_boundary(self.caret);
        cx.notify();
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.caret = self.next_boundary(self.caret);
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.caret = 0;
        cx.notify();
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.caret = self.content.len();
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self.previous_boundary(self.caret);
        if previous < self.caret {
            self.replace_text_in_range(
                Some(self.offset_to_utf16(previous)..self.offset_to_utf16(self.caret)),
                "",
                window,
                cx,
            );
        }
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        let next = self.next_boundary(self.caret);
        if next > self.caret {
            self.replace_text_in_range(
                Some(self.offset_to_utf16(self.caret)..self.offset_to_utf16(next)),
                "",
                window,
                cx,
            );
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace('\n', " "), window, cx);
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.caret = self.index_for_mouse_position(event.position);
        cx.notify();
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content[..offset]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content[offset..]
            .chars()
            .next()
            .map(|ch| offset + ch.len_utf8())
            .unwrap_or(self.content.len())
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.content[..offset].chars().map(|ch| ch.len_utf16()).sum()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf16_count = 0;
        let mut utf8_offset = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let start = self.offset_from_utf16(range_utf16.start);
        let end = self.offset_from_utf16(range_utf16.end);
        Some(self.content[start..end].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let caret = self.offset_to_utf16(self.caret);
        Some(UTF16Selection {
            range: caret..caret,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|range| {
                self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
            })
            .unwrap_or_else(|| {
                let caret = self.offset_to_utf16(self.caret);
                self.offset_from_utf16(caret)..self.offset_from_utf16(caret)
            });

        self.content = (self.content[0..range.start].to_owned()
            + new_text
            + &self.content[range.end..])
            .into();
        self.caret = range.start + new_text.len();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text_in_range(range_utf16, new_text, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let start = last_layout.x_for_index(self.offset_from_utf16(range_utf16.start));
        let end = last_layout.x_for_index(self.offset_from_utf16(range_utf16.end));
        Some(Bounds::new(
            Point::new(bounds.left() + start, bounds.top()),
            size(end - start, bounds.bottom() - bounds.top()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds.as_ref()?;
        let line = self.last_layout.as_ref()?;
        let index = line.index_for_x(point.x - bounds.left())?;
        Some(self.offset_to_utf16(index))
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .key_context("text_input")
            .track_focus(&self.focus_handle.clone())
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .child(InputField { input: cx.entity() })
    }
}

/// Renders a [`TextInput`] entity's text and caret.
pub(crate) struct InputField {
    pub(crate) input: Entity<TextInput>,
}

pub(crate) struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
}

impl IntoElement for InputField {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for InputField {
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
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let caret = input.caret;
        let masked = input.masked;
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), hsla(220., 0.05, 0.55, 1.))
        } else if masked {
            ("•".repeat(content.chars().count()).into(), style.color)
        } else {
            (content.clone(), style.color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &[run], None);

        // Caret index in display-text coordinates: masked text maps every
        // source char to a single "•", so chars-before-caret works directly.
        let caret_chars = content[..caret.min(content.len())].chars().count();

        let cursor = input.focus_handle.is_focused(window).then(|| {
            fill(
                Bounds::new(
                    Point::new(
                        bounds.left() + line.x_for_index(caret_chars),
                        bounds.top(),
                    ),
                    size(px(2.), bounds.bottom() - bounds.top()),
                ),
                white(),
            )
        });

        PrepaintState {
            line: Some(line),
            cursor,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let line = prepaint.line.take().unwrap();
        line.paint(bounds.origin, window.line_height(), window, cx)
            .unwrap();

        if let Some(cursor) = prepaint.cursor.take() {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

/// Bind the standard key bindings for [`TextInput`] widgets.
pub fn bind_text_input_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("text_input")),
        KeyBinding::new("delete", Delete, Some("text_input")),
        KeyBinding::new("left", Left, Some("text_input")),
        KeyBinding::new("right", Right, Some("text_input")),
        KeyBinding::new("home", Home, Some("text_input")),
        KeyBinding::new("end", End, Some("text_input")),
        KeyBinding::new("cmd-v", Paste, Some("text_input")),
        KeyBinding::new("enter", Submit, Some("text_input")),
    ]);
}

#[allow(dead_code)]
fn copy_content(input: &Entity<TextInput>, cx: &mut App) {
    let text = input.read(cx).content.to_string();
    cx.write_to_clipboard(ClipboardItem::new_string(text));
}
