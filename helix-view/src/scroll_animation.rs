//! Smooth scrolling: animates a view between two offsets, one visual line per frame.
//!
//! Instead of hooking every command that moves the view, the offset of each view is compared
//! with the offset drawn in the previous frame. If it jumped by more than a line while the
//! document and the view area stayed the same, the view is moved there gradually over the
//! following frames.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use helix_core::doc_formatter::TextFormat;
use helix_core::text_annotations::TextAnnotations;
use helix_core::{
    char_idx_at_visual_offset, visual_offset_from_anchor, RopeSlice, VisualOffsetError,
};

use crate::{graphics::Rect, view::ViewPosition, DocumentId, Editor, ViewId};

/// Jumps further than this many view heights only animate the last screen.
const MAX_ANIMATED_SCREENS: usize = 3;

#[derive(Debug, Default)]
pub struct ScrollAnimations {
    /// What was drawn for each view in the previous frame.
    last_frame: HashMap<ViewId, Frame>,
    active: HashMap<ViewId, Animation>,
}

#[derive(Debug, Clone, Copy)]
struct Frame {
    doc: DocumentId,
    version: i32,
    area: Rect,
    offset: ViewPosition,
}

#[derive(Debug, Clone, Copy)]
struct Animation {
    from: ViewPosition,
    target: ViewPosition,
    /// Visual lines between `from` and `target`, negative when scrolling up.
    rows: isize,
    start: Instant,
    /// The offset set for the last frame. If the view offset is anything else, the view was
    /// moved by something other than the animation.
    shown: ViewPosition,
}

enum Distance {
    Rows(isize),
    /// Further than the maximum number of rows, scrolling down (true) or up (false).
    Far {
        forward: bool,
    },
}

fn same_position(a: ViewPosition, b: ViewPosition) -> bool {
    a.anchor == b.anchor && a.vertical_offset == b.vertical_offset
}

/// Number of visual lines from `from` to `to`.
fn distance(
    text: RopeSlice,
    text_fmt: &TextFormat,
    annotations: &TextAnnotations,
    from: ViewPosition,
    to: ViewPosition,
    max_rows: usize,
) -> Option<Distance> {
    let forward = from.anchor <= to.anchor;
    let (start, end) = if forward { (from, to) } else { (to, from) };
    match visual_offset_from_anchor(
        text,
        start.anchor,
        end.anchor,
        text_fmt,
        annotations,
        max_rows,
    ) {
        Ok((pos, _)) => {
            let rows =
                pos.row as isize + end.vertical_offset as isize - start.vertical_offset as isize;
            Some(Distance::Rows(if forward { rows } else { -rows }))
        }
        Err(VisualOffsetError::PosAfterMaxRow) => Some(Distance::Far { forward }),
        Err(VisualOffsetError::PosBeforeAnchorRow) => None,
    }
}

/// `pos` moved by `rows` visual lines.
fn offset_by(
    text: RopeSlice,
    text_fmt: &TextFormat,
    annotations: &TextAnnotations,
    pos: ViewPosition,
    rows: isize,
) -> ViewPosition {
    let (anchor, vertical_offset) = char_idx_at_visual_offset(
        text,
        pos.anchor,
        pos.vertical_offset as isize + rows,
        0,
        text_fmt,
        annotations,
    );
    ViewPosition {
        anchor,
        horizontal_offset: pos.horizontal_offset,
        vertical_offset,
    }
}

fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

impl Editor {
    /// Starts, advances or finishes the scroll animation of every view. Called once per frame,
    /// before rendering. Returns true while any view is still animating, in which case another
    /// frame should be requested.
    pub fn step_scroll_animations(&mut self) -> bool {
        let config = self.config();
        let duration = Duration::from_millis(config.smooth_scroll.duration);
        let enabled = config.smooth_scroll.enable && !duration.is_zero();
        drop(config);

        let now = Instant::now();
        let tree = &self.tree;
        let anims = &mut self.scroll_animations;
        anims.last_frame.retain(|id, _| tree.contains(*id));
        anims.active.retain(|id, _| tree.contains(*id));

        let mut animating = false;
        for (view, _) in tree.views() {
            let Some(doc) = self.documents.get_mut(&view.doc) else {
                continue;
            };
            let current = doc.view_offset(view.id);
            let area = view.inner_area(doc);
            let version = doc.version();
            let height = area.height as usize;

            let last = anims.last_frame.get(&view.id).copied().filter(|last| {
                last.doc == view.doc && last.version == version && last.area == area
            });

            match (last, anims.active.get(&view.id)) {
                // The text or the layout changed: the old offsets can't be animated from.
                (None, _) => {
                    anims.active.remove(&view.id);
                }
                (Some(_), Some(anim)) if anim.shown == current => {}
                (Some(last), _) => {
                    anims.active.remove(&view.id);
                    if enabled && height > 0 && !same_position(last.offset, current) {
                        let text = doc.text().slice(..);
                        let text_fmt = doc.text_format(area.width, None);
                        let annotations = view.text_annotations(doc, None);
                        let max_rows = height * MAX_ANIMATED_SCREENS;
                        let target = current;
                        let from = ViewPosition {
                            horizontal_offset: target.horizontal_offset,
                            ..last.offset
                        };
                        let start =
                            match distance(text, &text_fmt, &annotations, from, target, max_rows) {
                                Some(Distance::Rows(rows)) => Some((from, rows)),
                                Some(Distance::Far { forward }) => {
                                    // Only animate the last screen of a long jump.
                                    let shift = if forward {
                                        -(height as isize)
                                    } else {
                                        height as isize
                                    };
                                    let from =
                                        offset_by(text, &text_fmt, &annotations, target, shift);
                                    match distance(
                                        text,
                                        &text_fmt,
                                        &annotations,
                                        from,
                                        target,
                                        max_rows,
                                    ) {
                                        Some(Distance::Rows(rows)) => Some((from, rows)),
                                        _ => None,
                                    }
                                }
                                None => None,
                            };
                        if let Some((from, rows)) = start.filter(|(_, rows)| rows.abs() >= 2) {
                            anims.active.insert(
                                view.id,
                                Animation {
                                    from,
                                    target,
                                    rows,
                                    start: now,
                                    shown: from,
                                },
                            );
                        }
                    }
                }
            }

            if let Some(anim) = anims.active.get_mut(&view.id) {
                let t = now.duration_since(anim.start).as_secs_f64() / duration.as_secs_f64();
                let offset = if !enabled || t >= 1.0 {
                    anim.target
                } else {
                    let rows = (anim.rows as f64 * ease_out_cubic(t)).round() as isize;
                    let text = doc.text().slice(..);
                    let text_fmt = doc.text_format(area.width, None);
                    let annotations = view.text_annotations(doc, None);
                    let offset = offset_by(text, &text_fmt, &annotations, anim.from, rows);
                    ViewPosition {
                        horizontal_offset: anim.target.horizontal_offset,
                        ..offset
                    }
                };
                if offset == anim.target {
                    anims.active.remove(&view.id);
                } else {
                    anim.shown = offset;
                    animating = true;
                }
                doc.set_view_offset(view.id, offset);
            }

            anims.last_frame.insert(
                view.id,
                Frame {
                    doc: view.doc,
                    version,
                    area,
                    offset: doc.view_offset(view.id),
                },
            );
        }
        animating
    }

    /// Jumps every animating view to the end of its animation, so that commands operate on the
    /// final view position rather than an intermediate frame.
    pub fn finish_scroll_animations(&mut self) {
        for (view_id, anim) in self.scroll_animations.active.drain() {
            if !self.tree.contains(view_id) {
                continue;
            }
            let doc_id = self.tree.get(view_id).doc;
            if let Some(doc) = self.documents.get_mut(&doc_id) {
                if doc.view_offset(view_id) == anim.shown {
                    doc.set_view_offset(view_id, anim.target);
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use helix_core::Rope;

    fn pos(anchor: usize) -> ViewPosition {
        ViewPosition {
            anchor,
            horizontal_offset: 0,
            vertical_offset: 0,
        }
    }

    fn rows(text: &Rope, text_fmt: &TextFormat, from: usize, to: usize) -> Option<isize> {
        let text = text.slice(..);
        let annotations = TextAnnotations::default();
        let from = pos(text.line_to_char(from));
        let to = pos(text.line_to_char(to));
        match distance(text, text_fmt, &annotations, from, to, 50) {
            Some(Distance::Rows(rows)) => Some(rows),
            _ => None,
        }
    }

    #[test]
    fn distance_between_lines() {
        let text = Rope::from("line\n".repeat(200));
        let text_fmt = TextFormat::default();
        assert_eq!(rows(&text, &text_fmt, 10, 30), Some(20));
        assert_eq!(rows(&text, &text_fmt, 30, 10), Some(-20));
        assert_eq!(rows(&text, &text_fmt, 7, 7), Some(0));
    }

    #[test]
    fn distance_beyond_max_rows() {
        let text = Rope::from("line\n".repeat(200));
        let slice = text.slice(..);
        let text_fmt = TextFormat::default();
        let annotations = TextAnnotations::default();
        let from = pos(0);
        let to = pos(slice.line_to_char(150));
        assert!(matches!(
            distance(slice, &text_fmt, &annotations, from, to, 50),
            Some(Distance::Far { forward: true })
        ));
        assert!(matches!(
            distance(slice, &text_fmt, &annotations, to, from, 50),
            Some(Distance::Far { forward: false })
        ));
    }

    #[test]
    fn distance_counts_soft_wrapped_rows() {
        // Every line wraps into two rows at a width of 10.
        let text = Rope::from(format!("{}\n", "x".repeat(15)).repeat(50));
        let text_fmt = TextFormat {
            soft_wrap: true,
            viewport_width: 10,
            wrap_indicator: Box::from(""),
            ..TextFormat::default()
        };
        assert_eq!(rows(&text, &text_fmt, 0, 5), Some(10));
        assert_eq!(rows(&text, &text_fmt, 5, 0), Some(-10));
    }

    #[test]
    fn offset_by_round_trips() {
        let text = Rope::from("line\n".repeat(200));
        let slice = text.slice(..);
        let text_fmt = TextFormat::default();
        let annotations = TextAnnotations::default();
        let start = pos(slice.line_to_char(40));
        let moved = offset_by(slice, &text_fmt, &annotations, start, -25);
        assert_eq!(moved.anchor, slice.line_to_char(15));
        assert!(matches!(
            distance(slice, &text_fmt, &annotations, moved, start, 50),
            Some(Distance::Rows(25))
        ));
    }

    #[test]
    fn easing_ends_at_target() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert!(ease_out_cubic(0.5) > 0.5);
    }
}
