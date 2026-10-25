use helix_core::{
    syntax::{Loader, QueryMatchIterEvent},
    unicode::width::UnicodeWidthStr,
};
use helix_view::{graphics::Rect, Document, Theme, View};
use tui::buffer::Buffer as Surface;

const SEPARATOR: &str = " › ";
const SEPARATOR_WIDTH: usize = 3;

/// Returns the names of the definitions (functions, classes, modules, ...) enclosing the
/// primary cursor, outermost first.
///
/// Definitions come from the language's `tags.scm` query. The query is restricted to the
/// cursor position so only nodes along the path from the root to the cursor are visited.
pub fn enclosing_symbols(doc: &Document, view: &View, loader: &Loader) -> Vec<String> {
    let Some(syntax) = doc.syntax() else {
        return Vec::new();
    };
    let text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(text);
    let cursor_byte = text.char_to_byte(cursor) as u32;

    let mut symbols: Vec<(u32, String)> = Vec::new();
    let mut matches = syntax.tags(text, loader, cursor_byte..cursor_byte + 1);
    while let Some(event) = matches.next() {
        let QueryMatchIterEvent::Match(mat) = event else {
            continue;
        };
        let Some(query) = loader.tag_query(matches.current_language()) else {
            continue;
        };
        let query = &query.query;
        let name_capture = query.get_capture("name");

        let mut definition = None;
        let mut name = None;
        for node in mat.nodes.iter() {
            if query.capture_name(node.capture).starts_with("definition.") {
                definition = Some(node.node.byte_range());
            } else if name_capture == Some(node.capture) {
                name = Some(node.node.byte_range());
            }
        }

        // The query cursor may return matches that merely touch the range.
        let Some(definition) = definition.filter(|def| def.contains(&cursor_byte)) else {
            continue;
        };
        let name = name.unwrap_or_else(|| definition.clone());
        let name = text
            .byte_slice(name.start as usize..name.end as usize)
            .to_string();
        // Only the first line of multi-line names (e.g. `impl<T> Foo for Bar<T>` types).
        let name = name.lines().next().unwrap_or_default().trim().to_string();
        symbols.push((definition.start, name));
    }

    // Matches across injection layers aren't necessarily ordered by position.
    symbols.sort_by_key(|(start, _)| *start);
    symbols.dedup();
    symbols.into_iter().map(|(_, name)| name).collect()
}

/// Render the breadcrumbs header (`file › outer › inner`) into the first row of `area`.
pub fn render(
    doc: &Document,
    view: &View,
    loader: &Loader,
    theme: &Theme,
    area: Rect,
    surface: &mut Surface,
) {
    let base_style = theme
        .try_get("ui.breadcrumbs")
        .unwrap_or_else(|| theme.get("ui.text"));
    let separator_style = theme
        .try_get_exact("ui.breadcrumbs.separator")
        .unwrap_or_else(|| theme.get("ui.virtual"));
    let symbol_style = theme
        .try_get_exact("ui.breadcrumbs.symbol")
        .unwrap_or(base_style);

    let row = area.with_height(1);
    surface.set_style(row, base_style);

    let mut crumbs = vec![(doc.display_name().into_owned(), base_style)];
    crumbs.extend(
        enclosing_symbols(doc, view, loader)
            .into_iter()
            .map(|name| (name, symbol_style)),
    );

    // Leave a one column margin on both sides.
    let width = row.width.saturating_sub(2) as usize;
    let crumb_width = |(name, _): &(String, _)| name.width() + SEPARATOR_WIDTH;
    // Drop the outermost crumbs first: the innermost context is the most useful.
    let mut total: usize = crumbs.iter().map(crumb_width).sum::<usize>() - SEPARATOR_WIDTH;
    let mut first = 0;
    while total > width && first + 1 < crumbs.len() {
        total -= crumb_width(&crumbs[first]);
        first += 1;
    }

    let mut x = row.x + 1;
    let right = row.x + row.width.saturating_sub(1);
    for (i, (name, style)) in crumbs[first..].iter().enumerate() {
        if i > 0 || first > 0 {
            let sep = if i == 0 { "… › " } else { SEPARATOR };
            x = surface
                .set_stringn(
                    x,
                    row.y,
                    sep,
                    right.saturating_sub(x) as usize,
                    separator_style,
                )
                .0;
        }
        x = surface
            .set_stringn(x, row.y, name, right.saturating_sub(x) as usize, *style)
            .0;
    }
}
