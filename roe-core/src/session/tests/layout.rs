// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use crate::session::layout::{LayoutChange, adjust_ratio_at_path, find_split_path, realize};

#[test]
fn nested_layout_drag_changes_only_the_identified_split() {
    let mut ids: SlotMap<WindowId, ()> = SlotMap::with_key();
    let left = ids.insert(());
    let middle = ids.insert(());
    let bottom = ids.insert(());
    let mut tree = WindowNode::new_split(
        SplitDirection::Horizontal,
        0.5,
        WindowNode::new_split(
            SplitDirection::Vertical,
            0.4,
            WindowNode::new_leaf(left),
            WindowNode::new_leaf(middle),
        ),
        WindowNode::new_leaf(bottom),
    );
    assert_eq!(
        find_split_path(&tree, middle, SplitDirection::Vertical, 1),
        Some((vec![0], 0.4))
    );
    adjust_ratio_at_path(&mut tree, &[0], 0.1);
    let WindowNode::Split {
        ratio: root_ratio,
        first,
        ..
    } = tree
    else {
        unreachable!();
    };
    let WindowNode::Split {
        ratio: nested_ratio,
        ..
    } = *first
    else {
        unreachable!();
    };
    assert_eq!(root_ratio, 0.5);
    assert!((nested_ratio - 0.5).abs() < f32::EPSILON);
}

#[test]
fn layout_validation_failure_restores_native_tree_and_presentation_identities() {
    let mut editor = test_editor();
    editor.frame.available_columns = 0;
    let initial_window = editor.active_window;
    let mut ids = HashMap::from([(initial_window, ViewId(1))]);
    let initial_ids = ids.clone();
    let mut next_id = 2;
    assert!(
        realize(
            &mut editor,
            &mut ids,
            &mut next_id,
            LayoutChange::Split {
                view: initial_window,
                direction: SplitDirection::Horizontal
            }
        )
        .is_err()
    );
    assert_eq!(editor.windows.len(), 1);
    assert_eq!(editor.active_window, initial_window);
    assert!(
        matches!(editor.window_tree, WindowNode::Leaf { window_id } if window_id == initial_window)
    );
    assert_eq!(ids, initial_ids);
    assert_eq!(next_id, 2);
    assert_eq!(editor.active_buffer().content(), "hello");
}
