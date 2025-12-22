// Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free
// software: you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation, version
// 3.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along with
// this program. If not, see <https://www.gnu.org/licenses/>.
//

//! File watching and CRDT-lite merge system
//!
//! This module provides:
//! - File system watching for external changes
//! - Base version tracking per buffer
//! - Line-based diff and merge with conflict detection
//! - Integration with undo system for safety

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use similar::{ChangeTag, TextDiff};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::BufferId;

/// Represents a change to a specific line range
#[derive(Debug, Clone)]
pub struct LineChange {
    /// Starting line (0-indexed)
    pub start_line: usize,
    /// Ending line (exclusive, 0-indexed)
    pub end_line: usize,
    /// The new content for these lines (empty vec = deletion)
    pub new_lines: Vec<String>,
}

/// Result of attempting to merge external changes with local changes
#[derive(Debug)]
pub enum MergeResult {
    /// No local changes, just reload the file
    CleanReload(String),
    /// Changes merged successfully without conflicts
    Merged { content: String, message: String },
    /// Changes merged but with conflicts marked inline
    MergedWithConflicts {
        content: String,
        conflict_count: usize,
    },
    /// No changes needed (file unchanged or only whitespace)
    NoChange,
    /// Local changes preserved, external changes ignored
    /// Contains the new external content to update the base (what's on disk now)
    LocalPreserved { new_base: String, message: String },
}

/// Tracks the base version and state for a single buffer
#[derive(Debug, Clone)]
pub struct BufferSyncState {
    /// The file path this buffer is associated with
    pub file_path: PathBuf,
    /// The base content (last known sync point with disk)
    pub base_content: String,
    /// Timestamp of when base was last updated
    pub base_timestamp: Instant,
    /// Whether we're currently ignoring changes (e.g., during our own save)
    pub ignore_until: Option<Instant>,
}

impl BufferSyncState {
    pub fn new(file_path: PathBuf, content: String) -> Self {
        Self {
            file_path,
            base_content: content,
            base_timestamp: Instant::now(),
            ignore_until: None,
        }
    }

    /// Update the base content after a successful merge or save
    pub fn update_base(&mut self, content: String) {
        self.base_content = content;
        self.base_timestamp = Instant::now();
    }

    /// Temporarily ignore file changes (call before saving)
    pub fn ignore_for(&mut self, duration: Duration) {
        self.ignore_until = Some(Instant::now() + duration);
    }

    /// Check if we should ignore current changes
    pub fn should_ignore(&self) -> bool {
        self.ignore_until
            .map(|until| Instant::now() < until)
            .unwrap_or(false)
    }
}

/// Event sent when an external file change is detected
#[derive(Debug, Clone)]
pub struct FileChangeEvent {
    pub buffer_id: BufferId,
    pub file_path: PathBuf,
}

/// Manages file watching for all open buffers
pub struct FileWatcher {
    /// The notify watcher instance
    watcher: Option<RecommendedWatcher>,
    /// Sender for file change events
    event_tx: Sender<FileChangeEvent>,
    /// Receiver for file change events (polled by editor)
    event_rx: Receiver<FileChangeEvent>,
    /// Map of file paths to buffer IDs (Arc for sharing with callback)
    path_to_buffer: Arc<RwLock<HashMap<PathBuf, BufferId>>>,
    /// Sync state per buffer
    sync_states: HashMap<BufferId, BufferSyncState>,
}

impl FileWatcher {
    pub fn new() -> Self {
        let (event_tx, event_rx) = channel();
        Self {
            watcher: None,
            event_tx,
            event_rx,
            path_to_buffer: Arc::new(RwLock::new(HashMap::new())),
            sync_states: HashMap::new(),
        }
    }

    /// Initialize the file watcher
    pub fn init(&mut self) -> Result<(), notify::Error> {
        let tx = self.event_tx.clone();
        let path_to_buffer = self.path_to_buffer.clone();

        let watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            let Ok(event) = res else { return };

            // Only care about modify/create/remove events
            if !matches!(
                event.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            ) {
                return;
            }

            for path in &event.paths {
                let Ok(canonical) = path.canonicalize() else {
                    continue;
                };
                let Ok(map) = path_to_buffer.read() else {
                    continue;
                };
                let Some(buffer_id) = map.get(&canonical) else {
                    continue;
                };

                let _ = tx.send(FileChangeEvent {
                    buffer_id: *buffer_id,
                    file_path: canonical,
                });
            }
        })?;

        self.watcher = Some(watcher);
        Ok(())
    }

    /// Start watching a file for a buffer
    pub fn watch_file(
        &mut self,
        buffer_id: BufferId,
        file_path: &Path,
        initial_content: String,
    ) -> Result<(), notify::Error> {
        // Initialize watcher if needed
        if self.watcher.is_none() {
            self.init()?;
        }

        let canonical = file_path
            .canonicalize()
            .unwrap_or_else(|_| file_path.to_path_buf());

        // Add to our tracking maps (using write lock for Arc)
        if let Ok(mut map) = self.path_to_buffer.write() {
            map.insert(canonical.clone(), buffer_id);
        }
        self.sync_states.insert(
            buffer_id,
            BufferSyncState::new(canonical.clone(), initial_content),
        );

        // Start watching the parent directory
        if let Some(ref mut watcher) = self.watcher {
            if let Some(parent) = canonical.parent() {
                watcher.watch(parent, RecursiveMode::NonRecursive)?;
            }
        }

        Ok(())
    }

    /// Stop watching a file
    pub fn unwatch_file(&mut self, buffer_id: BufferId) {
        if let Some(state) = self.sync_states.remove(&buffer_id) {
            // Remove from path_to_buffer using write lock
            if let Ok(mut map) = self.path_to_buffer.write() {
                map.remove(&state.file_path);
            }

            if let Some(ref mut watcher) = self.watcher {
                if let Some(parent) = state.file_path.parent() {
                    let _ = watcher.unwatch(parent);
                }
            }
        }
    }

    /// Poll for file change events (non-blocking)
    pub fn poll_events(&self) -> Vec<FileChangeEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            // Check if we should ignore this event
            if let Some(state) = self.sync_states.get(&event.buffer_id) {
                if !state.should_ignore() {
                    events.push(event);
                }
            }
        }
        events
    }

    /// Get sync state for a buffer
    pub fn get_sync_state(&self, buffer_id: BufferId) -> Option<&BufferSyncState> {
        self.sync_states.get(&buffer_id)
    }

    /// Get mutable sync state for a buffer
    pub fn get_sync_state_mut(&mut self, buffer_id: BufferId) -> Option<&mut BufferSyncState> {
        self.sync_states.get_mut(&buffer_id)
    }

    /// Update base content after save or merge
    pub fn update_base(&mut self, buffer_id: BufferId, content: String) {
        if let Some(state) = self.sync_states.get_mut(&buffer_id) {
            state.update_base(content);
        }
    }

    /// Mark that we're about to save, so ignore imminent file change events
    pub fn mark_saving(&mut self, buffer_id: BufferId) {
        if let Some(state) = self.sync_states.get_mut(&buffer_id) {
            state.ignore_for(Duration::from_millis(500));
        }
    }

    /// Get diagnostic info about the file watcher state
    pub fn status(&self) -> String {
        let watcher_status = if self.watcher.is_some() {
            "active"
        } else {
            "inactive"
        };

        let watched_files: Vec<String> = if let Ok(map) = self.path_to_buffer.read() {
            map.keys()
                .map(|p| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                })
                .collect()
        } else {
            vec![]
        };

        format!(
            "FileWatcher {}: {} file(s) watched: {}",
            watcher_status,
            watched_files.len(),
            watched_files.join(", ")
        )
    }
}

impl Default for FileWatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute line-based diff between two strings
pub fn compute_line_diff(old: &str, new: &str) -> Vec<LineChange> {
    let diff = TextDiff::from_lines(old, new);
    let mut changes = Vec::new();
    let mut current_change: Option<LineChange> = None;

    for change in diff.iter_all_changes() {
        let line_no = change
            .old_index()
            .unwrap_or(change.new_index().unwrap_or(0));

        match change.tag() {
            ChangeTag::Equal => {
                // Flush any pending change
                if let Some(c) = current_change.take() {
                    changes.push(c);
                }
            }
            ChangeTag::Delete => {
                if let Some(ref mut c) = current_change {
                    c.end_line = line_no + 1;
                } else {
                    current_change = Some(LineChange {
                        start_line: line_no,
                        end_line: line_no + 1,
                        new_lines: Vec::new(),
                    });
                }
            }
            ChangeTag::Insert => {
                if let Some(ref mut c) = current_change {
                    c.new_lines.push(change.value().to_string());
                } else {
                    current_change = Some(LineChange {
                        start_line: line_no,
                        end_line: line_no,
                        new_lines: vec![change.value().to_string()],
                    });
                }
            }
        }
    }

    // Flush final change
    if let Some(c) = current_change {
        changes.push(c);
    }

    changes
}

/// Check if two line changes overlap
pub fn changes_overlap(a: &LineChange, b: &LineChange) -> bool {
    // Changes overlap if their line ranges intersect
    let a_range = a.start_line..a.end_line.max(a.start_line + a.new_lines.len());
    let b_range = b.start_line..b.end_line.max(b.start_line + b.new_lines.len());

    a_range.start < b_range.end && b_range.start < a_range.end
}

/// Apply non-overlapping changes from both local and external to base
/// This performs a true 3-way merge when changes don't conflict
fn merge_non_overlapping(
    base: &str,
    local_changes: &[LineChange],
    external_changes: &[LineChange],
) -> String {
    // Combine all changes and sort by start_line descending
    // Applying from bottom-up ensures line numbers remain valid
    let mut all_changes: Vec<&LineChange> = local_changes
        .iter()
        .chain(external_changes.iter())
        .collect();
    all_changes.sort_by(|a, b| b.start_line.cmp(&a.start_line));

    let mut lines: Vec<String> = base.lines().map(|s| s.to_string()).collect();

    // Track if base ended with newline
    let had_trailing_newline = base.ends_with('\n');

    for change in all_changes {
        // Remove old lines (the range being replaced)
        let end = change.end_line.min(lines.len());
        if change.start_line < end {
            lines.drain(change.start_line..end);
        }

        // Insert new lines at the start position
        for (i, new_line) in change.new_lines.iter().enumerate() {
            let insert_pos = (change.start_line + i).min(lines.len());
            lines.insert(insert_pos, new_line.trim_end_matches('\n').to_string());
        }
    }

    let mut result = lines.join("\n");
    if had_trailing_newline && !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Attempt to merge external changes with local changes
pub fn merge_changes(base: &str, local: &str, external: &str) -> MergeResult {
    // If local hasn't changed from base, just take external
    if local == base {
        if external == base {
            return MergeResult::NoChange;
        }
        return MergeResult::CleanReload(external.to_string());
    }

    // If external hasn't changed from base, keep local
    if external == base {
        return MergeResult::NoChange;
    }

    // If local and external are the same, no merge needed
    if local == external {
        return MergeResult::NoChange;
    }

    // Compute diffs
    let local_changes = compute_line_diff(base, local);
    let external_changes = compute_line_diff(base, external);

    // Check for overlapping changes
    let mut has_conflicts = false;
    for lc in &local_changes {
        for ec in &external_changes {
            if changes_overlap(lc, ec) {
                has_conflicts = true;
                break;
            }
        }
        if has_conflicts {
            break;
        }
    }

    if !has_conflicts {
        // No conflicts - perform true 3-way merge
        let merged = merge_non_overlapping(base, &local_changes, &external_changes);
        MergeResult::Merged {
            content: merged,
            message: format!(
                "Merged {} local + {} external change(s)",
                local_changes.len(),
                external_changes.len()
            ),
        }
    } else {
        // Has conflicts - merge what we can and mark conflicts
        let (merged, conflict_count) = merge_with_conflicts(base, local, external);
        MergeResult::MergedWithConflicts {
            content: merged,
            conflict_count,
        }
    }
}

/// Merge with conflict markers for overlapping changes
fn merge_with_conflicts(_base: &str, local: &str, external: &str) -> (String, usize) {
    // Note: base is unused in this simple implementation but would be needed
    // for a proper 3-way merge that shows the original version in conflicts

    let mut result = Vec::new();
    let mut conflict_count = 0;

    // Use similar's unified diff to identify conflicts
    let diff = TextDiff::from_lines(local, external);

    let mut in_conflict = false;
    let mut local_section = Vec::new();
    let mut external_section = Vec::new();

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                // Flush any conflict
                if in_conflict {
                    result.push("<<<<<<< LOCAL".to_string());
                    result.extend(local_section.drain(..));
                    result.push("=======".to_string());
                    result.extend(external_section.drain(..));
                    result.push(">>>>>>> EXTERNAL".to_string());
                    conflict_count += 1;
                    in_conflict = false;
                }
                result.push(change.value().trim_end_matches('\n').to_string());
            }
            ChangeTag::Delete => {
                // This is local content not in external
                in_conflict = true;
                local_section.push(change.value().trim_end_matches('\n').to_string());
            }
            ChangeTag::Insert => {
                // This is external content not in local
                in_conflict = true;
                external_section.push(change.value().trim_end_matches('\n').to_string());
            }
        }
    }

    // Flush final conflict
    if in_conflict {
        result.push("<<<<<<< LOCAL".to_string());
        result.extend(local_section);
        result.push("=======".to_string());
        result.extend(external_section);
        result.push(">>>>>>> EXTERNAL".to_string());
        conflict_count += 1;
    }

    (result.join("\n"), conflict_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_changes() {
        let base = "line1\nline2\nline3";
        let result = merge_changes(base, base, base);
        assert!(matches!(result, MergeResult::NoChange));
    }

    #[test]
    fn test_clean_reload() {
        let base = "line1\nline2";
        let local = "line1\nline2"; // unchanged
        let external = "line1\nline2\nline3"; // added line

        let result = merge_changes(base, local, external);
        assert!(matches!(result, MergeResult::CleanReload(_)));
    }

    #[test]
    fn test_local_only_changes() {
        let base = "line1\nline2";
        let local = "line1\nmodified\nline2";
        let external = "line1\nline2"; // unchanged

        let result = merge_changes(base, local, external);
        assert!(matches!(result, MergeResult::NoChange));
    }

    #[test]
    fn test_non_overlapping_merge() {
        let base = "line1\nline2\nline3\nline4";
        let local = "modified1\nline2\nline3\nline4"; // changed line 1
        let external = "line1\nline2\nline3\nmodified4"; // changed line 4

        let result = merge_changes(base, local, external);
        // These don't overlap, should merge
        assert!(matches!(result, MergeResult::Merged { .. }));
    }

    #[test]
    fn test_conflict_detection() {
        let base = "line1\nline2\nline3";
        let local = "line1\nlocal_change\nline3";
        let external = "line1\nexternal_change\nline3";

        let result = merge_changes(base, local, external);
        assert!(matches!(result, MergeResult::MergedWithConflicts { .. }));
    }

    // ==================== NEW COMPREHENSIVE TESTS ====================

    #[test]
    fn test_compute_line_diff_simple_change() {
        let old = "line1\nline2\nline3";
        let new = "line1\nMODIFIED\nline3";
        let changes = compute_line_diff(old, new);

        eprintln!("Changes: {:?}", changes);
        assert!(!changes.is_empty(), "Should detect a change");
    }

    #[test]
    fn test_compute_line_diff_addition() {
        let old = "line1\nline2";
        let new = "line1\nline2\nline3";
        let changes = compute_line_diff(old, new);

        eprintln!("Addition changes: {:?}", changes);
        assert!(!changes.is_empty(), "Should detect addition");
    }

    #[test]
    fn test_non_overlapping_merge_content_verification() {
        // This is the critical test - verify the ACTUAL merged content
        let base = "line1\nline2\nline3\nline4";
        let local = "LOCAL1\nline2\nline3\nline4"; // changed line 1
        let external = "line1\nline2\nline3\nEXTERNAL4"; // changed line 4

        let result = merge_changes(base, local, external);

        match result {
            MergeResult::Merged { content, message } => {
                eprintln!("Merged message: {}", message);
                eprintln!("Merged content:\n{}", content);

                // The merged content MUST contain BOTH changes
                assert!(
                    content.contains("LOCAL1"),
                    "Merged content must contain local change 'LOCAL1'. Got:\n{}",
                    content
                );
                assert!(
                    content.contains("EXTERNAL4"),
                    "Merged content must contain external change 'EXTERNAL4'. Got:\n{}",
                    content
                );

                // Verify unchanged lines are preserved
                assert!(content.contains("line2"), "Should preserve line2");
                assert!(content.contains("line3"), "Should preserve line3");
            }
            other => {
                panic!(
                    "Expected MergeResult::Merged, got {:?}",
                    std::mem::discriminant(&other)
                );
            }
        }
    }

    #[test]
    fn test_merge_non_overlapping_direct() {
        // Test the merge_non_overlapping function directly
        let base = "line1\nline2\nline3\nline4";

        // local changes line 0 (line1 -> LOCAL1)
        let local_changes = vec![LineChange {
            start_line: 0,
            end_line: 1,
            new_lines: vec!["LOCAL1\n".to_string()],
        }];

        // external changes line 3 (line4 -> EXTERNAL4)
        let external_changes = vec![LineChange {
            start_line: 3,
            end_line: 4,
            new_lines: vec!["EXTERNAL4\n".to_string()],
        }];

        let merged = merge_non_overlapping(base, &local_changes, &external_changes);
        eprintln!("Direct merge result:\n{}", merged);

        assert!(
            merged.contains("LOCAL1"),
            "Must contain LOCAL1. Got:\n{}",
            merged
        );
        assert!(
            merged.contains("EXTERNAL4"),
            "Must contain EXTERNAL4. Got:\n{}",
            merged
        );
    }

    #[test]
    fn test_line_diff_then_merge_roundtrip() {
        // This tests the full roundtrip: diff -> merge
        let base = "aaa\nbbb\nccc\nddd";
        let local = "XXX\nbbb\nccc\nddd"; // changed first line
        let external = "aaa\nbbb\nccc\nYYY"; // changed last line

        let local_changes = compute_line_diff(base, local);
        let external_changes = compute_line_diff(base, external);

        eprintln!("Local changes: {:?}", local_changes);
        eprintln!("External changes: {:?}", external_changes);

        // Verify no overlap
        for lc in &local_changes {
            for ec in &external_changes {
                assert!(
                    !changes_overlap(lc, ec),
                    "Changes should not overlap: {:?} vs {:?}",
                    lc,
                    ec
                );
            }
        }

        let merged = merge_non_overlapping(base, &local_changes, &external_changes);
        eprintln!("Roundtrip merge result:\n{}", merged);

        assert!(merged.contains("XXX"), "Must contain XXX. Got:\n{}", merged);
        assert!(merged.contains("YYY"), "Must contain YYY. Got:\n{}", merged);
        assert!(merged.contains("bbb"), "Must preserve bbb");
        assert!(merged.contains("ccc"), "Must preserve ccc");
    }

    #[test]
    fn test_external_only_changes_triggers_reload() {
        let base = "line1\nline2";
        let local = "line1\nline2"; // unchanged from base
        let external = "line1\nEXTERNAL\nline2"; // external added a line

        let result = merge_changes(base, local, external);

        match result {
            MergeResult::CleanReload(content) => {
                assert!(
                    content.contains("EXTERNAL"),
                    "CleanReload should have external content"
                );
            }
            other => {
                panic!(
                    "Expected CleanReload, got {:?}",
                    std::mem::discriminant(&other)
                );
            }
        }
    }
}
