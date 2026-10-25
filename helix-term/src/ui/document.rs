use std::cmp::min;

use helix_core::doc_formatter::{DocumentFormatter, FormattedGrapheme, GraphemeSource, TextFormat};
use helix_core::graphemes::Grapheme;
use helix_core::str_utils::char_to_byte_idx;
use helix_core::syntax::{self, HighlightEvent, Highlighter, OverlayHighlights};
use helix_core::text_annotations::TextAnnotations;
use helix_core::{visual_offset_from_block, Position, RopeSlice};
use helix_stdx::rope::RopeSliceExt;
use helix_view::editor::{WhitespaceConfig, WhitespaceRenderValue};
use helix_view::graphics::{Color, Rect};
use helix_view::theme::Style;
use helix_view::view::ViewPosition;
use helix_view::{Document, Theme};
use tui::buffer::Buffer as Surface;

use crate::ui::text_decorations::DecorationManager;

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub struct LinePos {
    /// Indicates whether the given visual line
    /// is the first visual line of the given document line
    pub first_visual_line: bool,
    /// The line index of the document line that contains the given visual line
    pub doc_line: usize,
    /// Vertical offset from the top of the inner view area
    pub visual_line: u16,
}

#[allow(clippy::too_many_arguments)]
pub fn render_document(
    surface: &mut Surface,
    viewport: Rect,
    doc: &Document,
    offset: ViewPosition,
    doc_annotations: &TextAnnotations,
    syntax_highlighter: Option<Highlighter<'_>>,
    overlay_highlights: Vec<syntax::OverlayHighlights>,
    theme: &Theme,
    decorations: DecorationManager,
    trailing_whitespace_exempt_line: Option<usize>,
) {
    let mut renderer = TextRenderer::new(
        surface,
        doc,
        theme,
        Position::new(offset.vertical_offset, offset.horizontal_offset),
        viewport,
    );
    renderer.trailing_whitespace_exempt_line = trailing_whitespace_exempt_line;
    render_text(
        &mut renderer,
        doc.text().slice(..),
        offset.anchor,
        &doc.text_format(viewport.width, Some(theme)),
        doc_annotations,
        syntax_highlighter,
        overlay_highlights,
        theme,
        decorations,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_text(
    renderer: &mut TextRenderer,
    text: RopeSlice<'_>,
    anchor: usize,
    text_fmt: &TextFormat,
    text_annotations: &TextAnnotations,
    syntax_highlighter: Option<Highlighter<'_>>,
    overlay_highlights: Vec<syntax::OverlayHighlights>,
    theme: &Theme,
    mut decorations: DecorationManager,
) {
    let row_off = visual_offset_from_block(text, anchor, anchor, text_fmt, text_annotations)
        .0
        .row;

    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text, text_fmt, text_annotations, anchor);
    let mut syntax_highlighter =
        SyntaxHighlighter::new(syntax_highlighter, text, theme, renderer.text_style);
    let mut overlay_highlighter = OverlayHighlighter::new(overlay_highlights, theme);

    let mut last_line_pos = LinePos {
        first_visual_line: false,
        doc_line: usize::MAX,
        visual_line: u16::MAX,
    };
    let mut last_line_end = 0;
    let mut is_in_indent_area = true;
    let mut last_line_indent_level = 0;
    let mut reached_view_top = false;
    // Char index where the trailing whitespace of `trailing_whitespace_line` starts.
    let mut trailing_whitespace_line = usize::MAX;
    let mut trailing_whitespace_start = usize::MAX;

    loop {
        let Some(mut grapheme) = formatter.next() else {
            break;
        };

        // skip any graphemes on visual lines before the block start
        if grapheme.visual_pos.row < row_off {
            continue;
        }
        grapheme.visual_pos.row -= row_off;
        if !reached_view_top {
            decorations.prepare_for_rendering(grapheme.char_idx);
            reached_view_top = true;
        }

        // if the end of the viewport is reached stop rendering
        if grapheme.visual_pos.row as u16 >= renderer.viewport.height + renderer.offset.row as u16 {
            break;
        }

        // apply decorations before rendering a new line
        if grapheme.visual_pos.row as u16 != last_line_pos.visual_line {
            // we initiate doc_line with usize::MAX because no file
            // can reach that size (memory allocations are limited to isize::MAX)
            // initially there is no "previous" line (so doc_line is set to usize::MAX)
            // in that case we don't need to draw indent guides/virtual text
            if last_line_pos.doc_line != usize::MAX {
                // draw indent guides for the last line
                renderer.draw_indent_guides(last_line_indent_level, last_line_pos.visual_line);
                is_in_indent_area = true;
                decorations.render_virtual_lines(renderer, last_line_pos, last_line_end)
            }
            last_line_pos = LinePos {
                first_visual_line: grapheme.line_idx != last_line_pos.doc_line,
                doc_line: grapheme.line_idx,
                visual_line: grapheme.visual_pos.row as u16,
            };
            decorations.decorate_line(renderer, last_line_pos);
        }

        // acquire the correct grapheme style
        while grapheme.char_idx >= syntax_highlighter.pos {
            syntax_highlighter.advance();
        }
        while grapheme.char_idx >= overlay_highlighter.pos {
            overlay_highlighter.advance();
        }

        let grapheme_style = if let GraphemeSource::VirtualText { highlight } = grapheme.source {
            let mut style = renderer.text_style;
            if let Some(highlight) = highlight {
                style = style.patch(theme.highlight(highlight));
            }
            GraphemeStyle {
                syntax_style: style,
                overlay_style: Style::default(),
            }
        } else {
            GraphemeStyle {
                syntax_style: syntax_highlighter.style,
                overlay_style: overlay_highlighter.style,
            }
        };
        decorations.decorate_grapheme(renderer, &grapheme);

        let virt = grapheme.is_virtual();
        if renderer.trailing_whitespace && !virt && grapheme.line_idx != trailing_whitespace_line {
            trailing_whitespace_line = grapheme.line_idx;
            trailing_whitespace_start =
                if renderer.trailing_whitespace_exempt_line == Some(grapheme.line_idx) {
                    usize::MAX
                } else {
                    trailing_whitespace_start_of_line(text, grapheme.line_idx)
                };
        }
        let is_trailing_whitespace = !virt
            && grapheme.char_idx >= trailing_whitespace_start
            && grapheme.is_whitespace()
            && !matches!(grapheme.raw, Grapheme::Newline);
        let grapheme_width = renderer.draw_grapheme(
            &grapheme,
            grapheme_style,
            virt,
            is_trailing_whitespace,
            &mut last_line_indent_level,
            &mut is_in_indent_area,
            grapheme.visual_pos,
        );
        last_line_end = grapheme.visual_pos.col + grapheme_width;
    }

    renderer.draw_indent_guides(last_line_indent_level, last_line_pos.visual_line);
    decorations.render_virtual_lines(renderer, last_line_pos, last_line_end)
}

/// Returns the char index at which the run of whitespace at the end of `line` starts, or
/// `usize::MAX` if the line doesn't end in whitespace.
fn trailing_whitespace_start_of_line(text: RopeSlice, line: usize) -> usize {
    let line_end = helix_core::line_ending::line_end_char_index(&text, line);
    let line_start = text.line_to_char(line);
    let mut start = line_end;
    for ch in text
        .slice(line_start..line_end)
        .chars_at(line_end - line_start)
        .reversed()
    {
        if !ch.is_whitespace() {
            break;
        }
        start -= 1;
    }
    if start == line_end {
        usize::MAX
    } else {
        start
    }
}

#[derive(Debug)]
pub struct TextRenderer<'a> {
    surface: &'a mut Surface,
    pub text_style: Style,
    pub whitespace_style: Style,
    pub indent_guide_char: String,
    pub indent_guide_style: Style,
    pub newline: String,
    pub nbsp: String,
    pub nnbsp: String,
    pub space: String,
    pub tab: String,
    pub virtual_tab: String,
    /// Render whitespace at the end of lines with `trailing_whitespace_style`.
    pub trailing_whitespace: bool,
    pub trailing_whitespace_style: Style,
    /// Document line whose trailing whitespace is not highlighted (the line being typed on).
    pub trailing_whitespace_exempt_line: Option<usize>,
    /// Whitespace symbols used for trailing whitespace, independent of `render`.
    pub trailing_space: String,
    pub trailing_tab: String,
    pub trailing_nbsp: String,
    pub trailing_nnbsp: String,
    /// Style for rendered non-breaking spaces, which are usually unintended.
    pub nbsp_style: Style,
    pub indent_width: u16,
    pub starting_indent: usize,
    pub draw_indent_guides: bool,
    pub viewport: Rect,
    pub offset: Position,
}

pub struct GraphemeStyle {
    syntax_style: Style,
    overlay_style: Style,
}

impl<'a> TextRenderer<'a> {
    pub fn new(
        surface: &'a mut Surface,
        doc: &Document,
        theme: &Theme,
        offset: Position,
        viewport: Rect,
    ) -> TextRenderer<'a> {
        let editor_config = doc.config.load();
        let WhitespaceConfig {
            render: ws_render,
            characters: ws_chars,
        } = &editor_config.whitespace;

        let tab_width = doc.tab_width();
        let visible_tab: String = std::iter::once(ws_chars.tab)
            .chain(std::iter::repeat_n(ws_chars.tabpad, tab_width - 1))
            .collect();
        let tab = if ws_render.tab() == WhitespaceRenderValue::All {
            visible_tab.clone()
        } else {
            " ".repeat(tab_width)
        };
        let virtual_tab = " ".repeat(tab_width);
        let newline = if ws_render.newline() == WhitespaceRenderValue::All {
            ws_chars.newline.into()
        } else {
            " ".to_owned()
        };

        let space = if ws_render.space() == WhitespaceRenderValue::All {
            ws_chars.space.into()
        } else {
            " ".to_owned()
        };
        let nbsp = if ws_render.nbsp() == WhitespaceRenderValue::All {
            ws_chars.nbsp.into()
        } else {
            " ".to_owned()
        };
        let nnbsp = if ws_render.nnbsp() == WhitespaceRenderValue::All {
            ws_chars.nnbsp.into()
        } else {
            " ".to_owned()
        };

        let text_style = theme.get("ui.text");

        let indent_width = doc.indent_style.indent_width(tab_width) as u16;

        let trailing_whitespace_style = theme
            .try_get_exact("ui.virtual.trailing-whitespace")
            .or_else(|| theme.try_get_exact("error"))
            .unwrap_or_else(|| Style::default().fg(Color::Red));
        let nbsp_style = theme
            .try_get_exact("ui.virtual.nbsp")
            .or_else(|| theme.try_get_exact("warning"))
            .unwrap_or_else(|| Style::default().fg(Color::Yellow));

        TextRenderer {
            surface,
            trailing_whitespace: ws_render.trailing() == WhitespaceRenderValue::All,
            trailing_whitespace_style,
            trailing_whitespace_exempt_line: None,
            trailing_space: ws_chars.space.into(),
            trailing_tab: visible_tab,
            trailing_nbsp: ws_chars.nbsp.into(),
            trailing_nnbsp: ws_chars.nnbsp.into(),
            nbsp_style,
            indent_guide_char: editor_config.indent_guides.character.into(),
            newline,
            nbsp,
            nnbsp,
            space,
            tab,
            virtual_tab,
            whitespace_style: theme.get("ui.virtual.whitespace"),
            indent_width,
            starting_indent: offset.col / indent_width as usize
                + !offset.col.is_multiple_of(indent_width as usize) as usize
                + editor_config.indent_guides.skip_levels as usize,
            indent_guide_style: text_style.patch(
                theme
                    .try_get("ui.virtual.indent-guide")
                    .unwrap_or_else(|| theme.get("ui.virtual.whitespace")),
            ),
            text_style,
            draw_indent_guides: editor_config.indent_guides.render,
            viewport,
            offset,
        }
    }
    /// Draws a single `grapheme` at the current render position with a specified `style`.
    pub fn draw_decoration_grapheme(
        &mut self,
        grapheme: Grapheme,
        mut style: Style,
        mut row: u16,
        col: u16,
    ) -> bool {
        if (row as usize) < self.offset.row
            || row >= self.viewport.height
            || col >= self.viewport.width
        {
            return false;
        }
        row -= self.offset.row as u16;
        // TODO is it correct to apply the whitspace style to all unicode white spaces?
        if grapheme.is_whitespace() {
            style = style.patch(self.whitespace_style);
        }

        let grapheme = match grapheme {
            Grapheme::Tab { width } => {
                let grapheme_tab_width = char_to_byte_idx(&self.virtual_tab, width);
                &self.virtual_tab[..grapheme_tab_width]
            }
            Grapheme::Other { ref g } if g == "\u{00A0}" => " ",
            Grapheme::Other { ref g } => g,
            Grapheme::Newline => " ",
        };

        self.surface.set_string(
            self.viewport.x + col,
            self.viewport.y + row,
            grapheme,
            style,
        );
        true
    }

    /// Draws a single `grapheme` at the current render position with a specified `style`.
    pub fn draw_grapheme(
        &mut self,
        grapheme: &FormattedGrapheme,
        grapheme_style: GraphemeStyle,
        is_virtual: bool,
        is_trailing_whitespace: bool,
        last_indent_level: &mut usize,
        is_in_indent_area: &mut bool,
        mut position: Position,
    ) -> usize {
        if position.row < self.offset.row {
            return 0;
        }
        position.row -= self.offset.row;
        let cut_off_start = self.offset.col.saturating_sub(position.col);
        let is_whitespace = grapheme.is_whitespace();

        // TODO is it correct to apply the whitespace style to all unicode white spaces?
        let mut style = grapheme_style.syntax_style;
        if is_whitespace {
            style = style.patch(self.whitespace_style);
        }
        let is_nbsp =
            matches!(grapheme.raw, Grapheme::Other { ref g } if g == "\u{00A0}" || g == "\u{202F}");
        if is_trailing_whitespace {
            style = style.patch(self.trailing_whitespace_style);
        } else if is_nbsp && !is_virtual && self.nbsp != " " {
            style = style.patch(self.nbsp_style);
        }
        style = style.patch(grapheme_style.overlay_style);

        let width = grapheme.width();
        let mut is_tab = false;
        let (space, nbsp, nnbsp, tab) = if is_virtual {
            (" ", " ", " ", self.virtual_tab.as_str())
        } else if is_trailing_whitespace {
            (
                self.trailing_space.as_str(),
                self.trailing_nbsp.as_str(),
                self.trailing_nnbsp.as_str(),
                self.trailing_tab.as_str(),
            )
        } else {
            (
                self.space.as_str(),
                self.nbsp.as_str(),
                self.nnbsp.as_str(),
                self.tab.as_str(),
            )
        };
        let grapheme = match grapheme.raw {
            Grapheme::Tab { width } => {
                is_tab = true;
                let grapheme_tab_width = char_to_byte_idx(tab, width);
                &tab[..grapheme_tab_width]
            }
            // TODO special rendering for other whitespaces?
            Grapheme::Other { ref g } if g == " " && !grapheme.source.is_eof() => space,
            Grapheme::Other { ref g } if g == "\u{00A0}" => nbsp,
            Grapheme::Other { ref g } if g == "\u{202F}" => nnbsp,
            Grapheme::Other { ref g } => g,
            Grapheme::Newline => &self.newline,
        };

        let in_bounds = self.column_in_bounds(position.col, width);

        if in_bounds {
            let x = self.viewport.x + (position.col - self.offset.col) as u16;
            let y = self.viewport.y + position.row as u16;
            if is_tab {
                // A tab expands to `width` single-column cells; writing them
                // individually keeps background styles (selection, cursorline)
                // across the whole tab and avoids the redraw diff clipping
                // `render-whitespace` pads. A single `set_grapheme` would pack
                // them into one wide cell and leave the rest unstyled.
                self.surface.set_tab(x, y, grapheme, style);
            } else {
                self.surface.set_grapheme(x, y, grapheme, width, style);
            }
        } else if cut_off_start != 0 && cut_off_start < width {
            // partially on screen
            let rect = Rect::new(
                self.viewport.x,
                self.viewport.y + position.row as u16,
                (width - cut_off_start) as u16,
                1,
            );
            self.surface.set_style(rect, style);
        }
        if *is_in_indent_area && !is_whitespace {
            *last_indent_level = position.col;
            *is_in_indent_area = false;
        }

        width
    }

    pub fn column_in_bounds(&self, colum: usize, width: usize) -> bool {
        self.offset.col <= colum && colum + width <= self.offset.col + self.viewport.width as usize
    }

    /// Overlay indentation guides ontop of a rendered line
    /// The indentation level is computed in `draw_lines`.
    /// Therefore this function must always be called afterwards.
    pub fn draw_indent_guides(&mut self, indent_level: usize, mut row: u16) {
        if !self.draw_indent_guides || self.offset.row > row as usize {
            return;
        }
        row -= self.offset.row as u16;

        // Don't draw indent guides outside of view
        let end_indent = min(
            indent_level,
            // Add indent_width - 1 to round up, since the first visible
            // indent might be a bit after offset.col
            self.offset.col + self.viewport.width as usize + (self.indent_width as usize - 1),
        ) / self.indent_width as usize;

        for i in self.starting_indent..end_indent {
            let x = (self.viewport.x as usize + (i * self.indent_width as usize) - self.offset.col)
                as u16;
            let y = self.viewport.y + row;
            debug_assert!(self.surface.in_bounds(x, y));
            self.surface
                .set_string(x, y, &self.indent_guide_char, self.indent_guide_style);
        }
    }

    pub fn set_string(&mut self, x: u16, y: u16, string: &str, style: Style) {
        if (y as usize) < self.offset.row {
            return;
        }
        let y = y - self.offset.row as u16;
        self.surface
            .set_string(x, y + self.viewport.y, string, style)
    }

    pub fn set_stringn(&mut self, x: u16, y: u16, string: &str, width: usize, style: Style) {
        if (y as usize) < self.offset.row {
            return;
        }
        let y = y - self.offset.row as u16;
        self.surface
            .set_stringn(x, y + self.viewport.y, string, width, style);
    }

    /// Sets the style of an area **within the text viewport* this accounts
    /// both for the renderers vertical offset and its viewport
    pub fn set_style(&mut self, mut area: Rect, style: Style) {
        let offset = self.offset.row as u16;
        if area.y < offset {
            area.height = area.height.saturating_sub(offset - area.y);
            area.y = offset;
        }
        area.y = area.y - offset + self.viewport.y;
        self.surface.set_style(area, style);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_string_truncated(
        &mut self,
        x: u16,
        y: u16,
        string: &str,
        width: usize,
        style: impl Fn(usize) -> Style, // Map a grapheme's string offset to a style
        ellipsis: bool,
        truncate_start: bool,
    ) -> (u16, u16) {
        if (y as usize) < self.offset.row {
            return (x, y);
        }
        let y = y - self.offset.row as u16;
        self.surface.set_string_truncated(
            x,
            y + self.viewport.y,
            string,
            width,
            style,
            ellipsis,
            truncate_start,
        )
    }
}

struct SyntaxHighlighter<'h, 'r, 't> {
    inner: Option<Highlighter<'h>>,
    text: RopeSlice<'r>,
    /// The character index of the next highlight event, or `usize::MAX` if the highlighter is
    /// finished.
    pos: usize,
    theme: &'t Theme,
    text_style: Style,
    style: Style,
}

impl<'h, 'r, 't> SyntaxHighlighter<'h, 'r, 't> {
    fn new(
        inner: Option<Highlighter<'h>>,
        text: RopeSlice<'r>,
        theme: &'t Theme,
        text_style: Style,
    ) -> Self {
        let mut highlighter = Self {
            inner,
            text,
            pos: 0,
            theme,
            style: text_style,
            text_style,
        };
        highlighter.update_pos();
        highlighter
    }

    fn update_pos(&mut self) {
        self.pos = self
            .inner
            .as_ref()
            .and_then(|highlighter| {
                let next_byte_idx = highlighter.next_event_offset();
                (next_byte_idx != u32::MAX).then(|| {
                    // Move the byte index to the nearest character boundary (rounding up) and
                    // convert it to a character index.
                    self.text
                        .byte_to_char(self.text.ceil_char_boundary(next_byte_idx as usize))
                })
            })
            .unwrap_or(usize::MAX);
    }

    fn advance(&mut self) {
        let Some(highlighter) = self.inner.as_mut() else {
            return;
        };

        let (event, highlights) = highlighter.advance();
        let base = match event {
            HighlightEvent::Refresh => self.text_style,
            HighlightEvent::Push => self.style,
        };

        self.style = highlights.fold(base, |acc, highlight| {
            acc.patch(self.theme.highlight(highlight))
        });
        self.update_pos();
    }
}

struct OverlayHighlighter<'t> {
    inner: syntax::OverlayHighlighter,
    pos: usize,
    theme: &'t Theme,
    style: Style,
}

impl<'t> OverlayHighlighter<'t> {
    fn new(overlays: Vec<OverlayHighlights>, theme: &'t Theme) -> Self {
        let inner = syntax::OverlayHighlighter::new(overlays);
        let mut highlighter = Self {
            inner,
            pos: 0,
            theme,
            style: Style::default(),
        };
        highlighter.update_pos();
        highlighter
    }

    fn update_pos(&mut self) {
        self.pos = self.inner.next_event_offset();
    }

    fn advance(&mut self) {
        let (event, highlights) = self.inner.advance();
        let base = match event {
            HighlightEvent::Refresh => Style::default(),
            HighlightEvent::Push => self.style,
        };

        self.style = highlights.fold(base, |acc, highlight| {
            acc.patch(self.theme.highlight(highlight))
        });
        self.update_pos();
    }
}

#[cfg(test)]
mod tests {
    use helix_core::Rope;

    use super::trailing_whitespace_start_of_line;

    #[test]
    fn trailing_whitespace_start() {
        let text = Rope::from("no trailing\nab  \n\t \u{00A0}\n\nend \t");
        let text = text.slice(..);
        assert_eq!(usize::MAX, trailing_whitespace_start_of_line(text, 0));
        // "ab  " starts at char 12
        assert_eq!(14, trailing_whitespace_start_of_line(text, 1));
        // whitespace-only line
        assert_eq!(17, trailing_whitespace_start_of_line(text, 2));
        // empty line
        assert_eq!(usize::MAX, trailing_whitespace_start_of_line(text, 3));
        // last line without a line ending
        assert_eq!(25, trailing_whitespace_start_of_line(text, 4));
    }
}
