// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Typed layout mutation, validation, and geometry. No Mica, attachment, or I/O access.

use crate::editor::{BorderInfo, SplitDirection, WindowNode};
use crate::native_kernel::{LayoutNode, LogicalLayout, SplitAxis, ViewId, validate_layout};
use crate::{Editor, WindowId};
use std::collections::HashMap;

pub(super) enum LayoutChange {
    Split {
        view: WindowId,
        direction: SplitDirection,
    },
    Delete {
        view: WindowId,
        others: bool,
    },
}

impl LayoutChange {
    fn apply(self, editor: &mut Editor) -> bool {
        match self {
            Self::Split { view, direction } => {
                if !editor.windows.contains_key(view) {
                    return false;
                }
                editor.active_window = view;
                match direction {
                    SplitDirection::Horizontal => editor.split_horizontal(),
                    SplitDirection::Vertical => editor.split_vertical(),
                };
                true
            }
            Self::Delete { view, others } => {
                if !editor.windows.contains_key(view) {
                    return false;
                }
                editor.active_window = view;
                if others {
                    editor.delete_other_windows()
                } else {
                    editor.delete_window()
                }
            }
        }
    }
}

pub(super) fn realize(
    editor: &mut Editor,
    view_ids: &mut HashMap<WindowId, ViewId>,
    next_view_id: &mut u64,
    change: LayoutChange,
) -> Result<bool, String> {
    let previous_tree = editor.window_tree.clone();
    let previous_windows = editor.windows.clone();
    let previous_active = editor.active_window;
    let previous_prior = editor.previous_active_window;
    let previous_view_ids = view_ids.clone();
    let previous_next_view_id = *next_view_id;
    if !change.apply(editor) {
        editor.active_window = previous_active;
        return Ok(false);
    }
    for window in editor.windows.keys() {
        view_ids.entry(window).or_insert_with(|| {
            let id = ViewId(*next_view_id);
            *next_view_id += 1;
            id
        });
    }
    let validation = logical_layout(&editor.window_tree, editor, view_ids)
        .and_then(|layout| validate_layout(&layout).map_err(|error| error.to_string()));
    if let Err(error) = validation {
        editor.window_tree = previous_tree;
        editor.windows = previous_windows;
        editor.active_window = previous_active;
        editor.previous_active_window = previous_prior;
        *view_ids = previous_view_ids;
        *next_view_id = previous_next_view_id;
        Err(error)
    } else {
        Ok(true)
    }
}

pub(super) fn detect_border(editor: &Editor, x: u16, y: u16) -> Option<(BorderInfo, WindowId)> {
    for (window_id, window) in &editor.windows {
        let right = window
            .x
            .saturating_add(window.width_chars.saturating_sub(1));
        let bottom = window
            .y
            .saturating_add(window.height_chars.saturating_sub(1));
        if (x == window.x || x == right)
            && y >= window.y
            && y <= bottom
            && let Some((path, ratio)) = find_split_for_border(editor, window_id, x, true)
        {
            return Some((
                BorderInfo {
                    is_vertical: true,
                    split_node_path: path,
                    original_ratio: ratio,
                },
                window_id,
            ));
        }
        if (y == window.y || y == bottom)
            && x >= window.x
            && x <= right
            && let Some((path, ratio)) = find_split_for_border(editor, window_id, y, false)
        {
            return Some((
                BorderInfo {
                    is_vertical: false,
                    split_node_path: path,
                    original_ratio: ratio,
                },
                window_id,
            ));
        }
    }
    None
}

pub(super) fn find_split_for_border(
    editor: &Editor,
    window_id: WindowId,
    coordinate: u16,
    vertical: bool,
) -> Option<(Vec<usize>, f32)> {
    let window = editor.windows.get(window_id)?;
    let (leading, trailing) = if vertical {
        (
            window.x,
            window
                .x
                .saturating_add(window.width_chars.saturating_sub(1)),
        )
    } else {
        (
            window.y,
            window
                .y
                .saturating_add(window.height_chars.saturating_sub(1)),
        )
    };
    let required_branch = if coordinate == leading {
        1
    } else if coordinate == trailing {
        0
    } else {
        return None;
    };
    let direction = if vertical {
        SplitDirection::Vertical
    } else {
        SplitDirection::Horizontal
    };
    find_split_path(&editor.window_tree, window_id, direction, required_branch)
}

pub(super) fn find_split_path(
    tree: &WindowNode,
    window_id: WindowId,
    direction: SplitDirection,
    required_branch: usize,
) -> Option<(Vec<usize>, f32)> {
    fn leaf_path(node: &WindowNode, target: WindowId, path: &mut Vec<usize>) -> bool {
        match node {
            WindowNode::Leaf { window_id } => *window_id == target,
            WindowNode::Split { first, second, .. } => {
                path.push(0);
                if leaf_path(first, target, path) {
                    return true;
                }
                path.pop();
                path.push(1);
                if leaf_path(second, target, path) {
                    return true;
                }
                path.pop();
                false
            }
        }
    }

    let mut leaf = Vec::new();
    if !leaf_path(tree, window_id, &mut leaf) {
        return None;
    }
    let mut node = tree;
    let mut node_path = Vec::new();
    let mut candidate = None;
    for branch in leaf {
        let WindowNode::Split {
            direction: node_direction,
            ratio,
            first,
            second,
        } = node
        else {
            return None;
        };
        if *node_direction == direction && branch == required_branch {
            candidate = Some((node_path.clone(), *ratio));
        }
        node = if branch == 0 { first } else { second };
        node_path.push(branch);
    }
    candidate
}

pub(super) fn update_layout_drag(editor: &mut Editor, border: &BorderInfo, dx: i32, dy: i32) {
    const SENSITIVITY: f32 = 0.005;
    let change = if border.is_vertical {
        dx as f32 * SENSITIVITY
    } else {
        dy as f32 * SENSITIVITY
    };
    if change == 0.0 {
        return;
    }
    adjust_ratio_at_path(&mut editor.window_tree, &border.split_node_path, change);
    editor.calculate_window_layout();
}

pub(super) fn ratio_at_path(node: &WindowNode, path: &[usize]) -> Option<f32> {
    if path.is_empty() {
        return match node {
            WindowNode::Split { ratio, .. } => Some(*ratio),
            WindowNode::Leaf { .. } => None,
        };
    }
    match node {
        WindowNode::Leaf { .. } => None,
        WindowNode::Split { first, second, .. } => match path[0] {
            0 => ratio_at_path(first, &path[1..]),
            1 => ratio_at_path(second, &path[1..]),
            _ => None,
        },
    }
}

pub(super) fn set_ratio_at_path(node: &mut WindowNode, path: &[usize], ratio: f32) -> bool {
    if path.is_empty() {
        if let WindowNode::Split { ratio: current, .. } = node {
            *current = ratio;
            return true;
        }
        return false;
    }
    match node {
        WindowNode::Leaf { .. } => false,
        WindowNode::Split { first, second, .. } => match path[0] {
            0 => set_ratio_at_path(first, &path[1..], ratio),
            1 => set_ratio_at_path(second, &path[1..], ratio),
            _ => false,
        },
    }
}

pub(super) fn logical_layout(
    tree: &WindowNode,
    editor: &Editor,
    view_ids: &HashMap<WindowId, ViewId>,
) -> Result<LogicalLayout, String> {
    fn convert(
        node: &WindowNode,
        view_ids: &HashMap<WindowId, ViewId>,
    ) -> Result<LayoutNode, String> {
        match node {
            WindowNode::Leaf { window_id } => view_ids
                .get(window_id)
                .copied()
                .map(LayoutNode::View)
                .ok_or_else(|| "layout leaf has no transport view identity".to_owned()),
            WindowNode::Split {
                direction,
                ratio,
                first,
                second,
            } => Ok(LayoutNode::Split {
                axis: match direction {
                    SplitDirection::Horizontal => SplitAxis::Horizontal,
                    SplitDirection::Vertical => SplitAxis::Vertical,
                },
                ratio: *ratio,
                first: Box::new(convert(first, view_ids)?),
                second: Box::new(convert(second, view_ids)?),
            }),
        }
    }

    Ok(LogicalLayout {
        columns: editor.frame.available_columns,
        rows: editor.frame.available_lines,
        active: *view_ids
            .get(&editor.active_window)
            .ok_or_else(|| "active window has no transport view identity".to_owned())?,
        root: convert(tree, view_ids)?,
    })
}

pub(super) fn adjust_ratio_at_path(node: &mut WindowNode, path: &[usize], change: f32) {
    if path.is_empty() {
        if let WindowNode::Split { ratio, .. } = node {
            *ratio = (*ratio + change).clamp(0.15, 0.85);
        }
        return;
    }
    match node {
        WindowNode::Leaf { .. } => {}
        WindowNode::Split { first, second, .. } => match path[0] {
            0 => adjust_ratio_at_path(first, &path[1..], change),
            1 => adjust_ratio_at_path(second, &path[1..], change),
            _ => {}
        },
    }
}
