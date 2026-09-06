// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Read-only workspace projection. Mutable access is limited to caches and display state.

use super::policy::PolicyProjection;
use super::presentation_cache::PresentationCache;
use super::protocol::*;
use super::{TypeoutState, typeout_body_rows, typeout_text_lines};
use crate::editor::WindowType;
use crate::native_kernel::{ResourceId, TextSelection, ViewId};
use crate::syntax_highlighting::HighlightSpan;
use crate::{BufferId, Editor, WindowId};
use std::collections::{HashMap, HashSet};

pub(super) struct ProjectionInput<'a> {
    pub editor: &'a Editor,
    pub policy: &'a PolicyProjection,
    pub buffer_resources: &'a HashMap<BufferId, ResourceId>,
    pub view_ids: &'a HashMap<WindowId, ViewId>,
    pub typeout: Option<&'a TypeoutState>,
    pub search_ranges: &'a HashMap<WindowId, Vec<(usize, usize, String)>>,
    pub styled_lines: &'a HashMap<WindowId, Vec<(usize, String)>>,
}

pub(super) struct ProjectionAttachment<'a> {
    pub epoch: SessionEpoch,
    pub revision: Revision,
    pub viewport: AttachmentViewport,
    pub view_scroll: &'a mut HashMap<WindowId, ViewScroll>,
    pub presented_cursors: &'a mut HashMap<WindowId, usize>,
    pub typeout_page: &'a mut Option<(TypeoutId, usize)>,
}

#[derive(Default)]
pub(super) struct PresentationProjector {
    buffers: PresentationCache,
    highlights: HashMap<BufferId, MicaHighlightCache>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MicaHighlightCache {
    text_revision: u64,
    policy_revision: u64,
    spans: Vec<HighlightSpan>,
}

impl PresentationProjector {
    pub(super) fn scroll_limits(&mut self, id: BufferId, buffer: &crate::Buffer) -> (usize, usize) {
        self.buffers.scroll_limits(id, buffer)
    }

    pub(super) fn retain_live(&mut self, buffers: &HashSet<BufferId>) {
        self.highlights.retain(|buffer, _| buffers.contains(buffer));
    }

    #[cfg(test)]
    pub(super) fn highlight_revision(&self, buffer: BufferId) -> Option<u64> {
        self.highlights
            .get(&buffer)
            .map(|cache| cache.text_revision)
    }

    fn refresh_highlights(
        &mut self,
        input: &ProjectionInput<'_>,
        syntax: &mut crate::syntax::SyntaxService,
    ) {
        syntax.retain_live(&input.editor.buffers.keys().collect());
        let policy_revision = input.policy.revision;
        let visible_buffers: HashSet<_> = input
            .editor
            .windows
            .values()
            .map(|window| window.active_buffer)
            .take(MAX_SESSION_VIEWS)
            .collect();
        self.highlights
            .retain(|buffer, _| visible_buffers.contains(buffer));
        let candidates: Vec<_> = visible_buffers
            .into_iter()
            .filter_map(|buffer| {
                input
                    .editor
                    .buffers
                    .get(buffer)
                    .map(|value| (buffer, value))
            })
            .filter(|(buffer, _)| input.policy.highlights.contains_key(buffer))
            .map(|(buffer, value)| (buffer, value.clone()))
            .collect();

        for (buffer, value) in candidates {
            let observed = self.buffers.buffer(buffer, &value);
            let text_revision = observed.revision;
            let current = self.highlights.get(&buffer);
            if current.is_some_and(|cached| {
                cached.text_revision == text_revision && cached.policy_revision == policy_revision
            }) {
                continue;
            }
            self.highlights.insert(
                buffer,
                MicaHighlightCache {
                    text_revision,
                    policy_revision,
                    spans: match input
                        .policy
                        .modes
                        .get(&buffer)
                        .and_then(|mode| input.policy.parsers.get(mode))
                    {
                        Some(plan) => syntax
                            .highlights(buffer, text_revision, &observed.rope, plan)
                            .unwrap_or_else(|error| {
                                tracing::warn!(%error, "syntax highlighting unavailable");
                                Vec::new()
                            }),
                        None => Vec::new(),
                    },
                },
            );
        }
    }

    pub(super) fn capture(
        &mut self,
        input: ProjectionInput<'_>,
        attachment: ProjectionAttachment<'_>,
        syntax: &mut crate::syntax::SyntaxService,
    ) -> PresentationSnapshot {
        self.buffers.begin_frame();
        self.refresh_highlights(&input, syntax);
        let mut styles = Vec::new();
        let mut style_by_name = HashMap::new();
        let mut views = Vec::new();
        let live_windows: HashSet<_> = input.editor.windows.keys().collect();
        attachment
            .view_scroll
            .retain(|window, _| live_windows.contains(window));
        attachment
            .presented_cursors
            .retain(|window, _| live_windows.contains(window));

        for (window_id, window) in &input.editor.windows {
            let Some(buffer) = input.editor.buffers.get(window.active_buffer) else {
                continue;
            };
            let resource = input.buffer_resources[&window.active_buffer];
            let id = input.view_ids[&window_id];
            let frame = self.buffers.buffer(window.active_buffer, buffer);
            let total_lines = frame.rope.len_lines();
            let (cursor_column, cursor_line) = frame.cursor_coordinates(window.cursor);
            let scroll = attachment
                .view_scroll
                .entry(window_id)
                .or_insert(ViewScroll {
                    start_line: 0,
                    start_column: 0,
                });
            if attachment.presented_cursors.get(&window_id) != Some(&window.cursor) {
                ensure_cursor_visible(
                    scroll,
                    cursor_column,
                    cursor_line,
                    window.width_chars.saturating_sub(4),
                    window.height_chars.saturating_sub(3),
                );
                attachment
                    .presented_cursors
                    .insert(window_id, window.cursor);
            }
            let scroll = *scroll;
            let start_line = scroll.start_line.min(total_lines.saturating_sub(1));
            let visible_lines = usize::from(window.height_chars.saturating_sub(2)).max(1);
            let end_line = (start_line + visible_lines).min(total_lines);
            let (visible_start_char, visible_end_char, visible_text) =
                self.buffers
                    .visible_text(window.active_buffer, &frame, start_line, end_line);

            let mut styled_ranges: Vec<_> = self
                .highlights
                .get(&window.active_buffer)
                .filter(|cache| {
                    cache.text_revision == frame.revision
                        && cache.policy_revision == input.policy.revision
                })
                .into_iter()
                .flat_map(|cache| &cache.spans)
                .filter(|span| span.end > visible_start_char && span.start < visible_end_char)
                .filter_map(|span| {
                    let face =
                        mica_highlight_face(input.policy, window.active_buffer, &span.capture)?
                            .to_owned();
                    Some(StyledRange {
                        start: span.start,
                        end: span.end,
                        style: presentation_style(
                            &face,
                            &input.policy.faces,
                            &mut styles,
                            &mut style_by_name,
                        ),
                    })
                })
                .collect();
            styled_ranges.extend(
                input
                    .search_ranges
                    .get(&window_id)
                    .into_iter()
                    .flatten()
                    .filter(|(start, end, _)| {
                        *end > visible_start_char && *start < visible_end_char
                    })
                    .map(|(start, end, name)| {
                        let style = presentation_style(
                            name,
                            &input.policy.faces,
                            &mut styles,
                            &mut style_by_name,
                        );
                        StyledRange {
                            start: *start,
                            end: *end,
                            style,
                        }
                    }),
            );
            let styled_lines = input
                .styled_lines
                .get(&window_id)
                .into_iter()
                .flatten()
                .filter(|(line, _)| *line >= start_line && *line < end_line)
                .map(|(line, name)| StyledLine {
                    line: *line,
                    style: presentation_style(
                        name,
                        &input.policy.faces,
                        &mut styles,
                        &mut style_by_name,
                    ),
                })
                .collect();

            let selection = frame
                .mark
                .filter(|anchor| *anchor != window.cursor)
                .map(|anchor| TextSelection {
                    anchor: anchor.min(window.cursor).min(frame.rope.len_chars()),
                    active: anchor.max(window.cursor).min(frame.rope.len_chars()),
                });
            let (column, line) = (cursor_column, cursor_line);
            let mode = input
                .policy
                .modes
                .get(&window.active_buffer)
                .cloned()
                .unwrap_or_else(|| "unpublished".to_string());
            let status = if frame.read_only {
                "%"
            } else if frame.modified() {
                "*"
            } else {
                "-"
            };
            let modeline = format!(
                "{status} {} ({mode}) {}:{}",
                frame.name,
                line.saturating_add(1),
                column.saturating_add(1)
            );
            let typeout = presented_typeout(
                input.typeout,
                input.editor,
                attachment.typeout_page,
                window_id,
                window.height_chars,
            );
            views.push(PresentedView {
                id,
                resource,
                name: frame.name.clone(),
                buffer_kind: frame.kind.as_str().to_owned(),
                visited_file: frame.path.clone(),
                text_revision: frame.revision,
                last_saved_revision: frame.saved_revision,
                modified: frame.modified(),
                read_only: frame.read_only,
                visible_text,
                visible_start_char,
                visible_end_char,
                total_lines,
                max_line_chars: frame.max_line_chars,
                cursor: window.cursor,
                selection,
                geometry: ViewGeometry {
                    x: window.x,
                    y: window.y,
                    columns: window.width_chars,
                    rows: window.height_chars,
                },
                scroll: ViewScroll {
                    start_line: scroll.start_line,
                    start_column: scroll.start_column,
                },
                active: window_id == input.editor.active_window,
                command_view: matches!(window.window_type, WindowType::Command { .. }),
                show_gutter: frame.show_gutter,
                modeline,
                styled_ranges,
                styled_lines,
                typeout,
            });
        }
        self.buffers.finish_frame();
        views.sort_by_key(|view| view.id.0);

        PresentationSnapshot {
            epoch: attachment.epoch,
            revision: attachment.revision,
            columns: attachment.viewport.columns,
            rows: attachment.viewport.rows,
            active_view: input.view_ids[&input.editor.active_window],
            views,
            styles,
            echo_area: input.editor.echo_message.clone(),
        }
    }
}

fn mica_highlight_face<'a>(
    policy: &'a PolicyProjection,
    buffer: BufferId,
    capture: &str,
) -> Option<&'a str> {
    let rules = policy
        .highlights
        .get(&buffer)?
        .iter()
        .filter(|rule| rule.capture == capture);
    let selected = rules.clone().max_by_key(|rule| rule.precedence)?;
    if rules
        .filter(|rule| rule.precedence == selected.precedence)
        .any(|rule| rule.face != selected.face)
    {
        return None;
    }
    Some(selected.face.as_str())
}

fn presented_typeout(
    typeout: Option<&TypeoutState>,
    editor: &Editor,
    page: &mut Option<(TypeoutId, usize)>,
    window: WindowId,
    height_rows: u16,
) -> Option<PresentedTypeout> {
    let typeout = typeout?;
    if typeout.view != window || editor.windows.get(window)?.active_buffer != typeout.origin_buffer
    {
        return None;
    }
    let page_rows = typeout_body_rows(height_rows);
    let lines = typeout_text_lines(&typeout.text);
    let total_lines = lines.len();
    if page.map(|(id, _)| id) != Some(typeout.id) {
        *page = Some((typeout.id, 0));
    }
    let (_, first_line) = page.as_mut().expect("typeout page was initialized above");
    let first_line = (*first_line).min(total_lines.saturating_sub(1));
    let end_line = first_line.saturating_add(page_rows).min(total_lines);
    Some(PresentedTypeout {
        id: typeout.id,
        kind: typeout.kind.clone(),
        title: typeout.title.clone(),
        visible_text: lines[first_line..end_line].join("\n"),
        first_visible_line: first_line,
        total_lines,
        more_before: first_line > 0,
        more_after: end_line < total_lines,
        complete: true,
    })
}

fn ensure_cursor_visible(
    scroll: &mut ViewScroll,
    cursor_column: usize,
    cursor_line: usize,
    content_columns: u16,
    content_rows: u16,
) {
    let content_columns = usize::from(content_columns.max(1));
    let content_rows = usize::from(content_rows.max(1));
    if cursor_line >= scroll.start_line.saturating_add(content_rows) {
        scroll.start_line = cursor_line.saturating_sub(content_rows.saturating_sub(1));
    } else if cursor_line < scroll.start_line {
        scroll.start_line = cursor_line;
    }
    if cursor_column >= scroll.start_column.saturating_add(content_columns) {
        scroll.start_column = cursor_column.saturating_sub(content_columns.saturating_sub(1));
    } else if cursor_column < scroll.start_column {
        scroll.start_column = cursor_column;
    }
}

fn presentation_color_hex(value: &str) -> Option<PresentationColor> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    Some(PresentationColor::Rgb {
        r: u8::from_str_radix(&hex[0..2], 16).ok()?,
        g: u8::from_str_radix(&hex[2..4], 16).ok()?,
        b: u8::from_str_radix(&hex[4..6], 16).ok()?,
    })
}

fn presentation_style(
    name: &str,
    faces: &HashMap<String, HashMap<String, String>>,
    styles: &mut Vec<StyleDefinition>,
    style_by_name: &mut HashMap<String, StyleRef>,
) -> StyleRef {
    if let Some(style) = style_by_name.get(name) {
        return *style;
    }
    let id = StyleRef(styles.len() as u32 + 1);
    let attributes = faces.get(name);
    styles.push(StyleDefinition {
        id,
        name: name.to_owned(),
        foreground: attributes
            .and_then(|values| values.get("foreground"))
            .and_then(|value| presentation_color_hex(value)),
        background: attributes
            .and_then(|values| values.get("background"))
            .and_then(|value| presentation_color_hex(value)),
        bold: attributes
            .and_then(|values| values.get("weight"))
            .is_some_and(|value| value == "bold"),
        italic: attributes
            .and_then(|values| values.get("slant"))
            .is_some_and(|value| value == "italic"),
        underline: attributes
            .and_then(|values| values.get("underline"))
            .is_some_and(|value| value == "true"),
        strikethrough: false,
    });
    style_by_name.insert(name.to_owned(), id);
    id
}
