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

use crate::session::{
    PresentationSnapshot as SessionPresentationSnapshot, PresentationUpdate, Revision, SessionEpoch,
};
use crate::{BufferId, WindowId};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PresentationStreamError {
    #[error("presentation snapshot metadata does not match its envelope")]
    SnapshotMismatch,
    #[error("presentation delta belongs to epoch {received:?}, expected {expected:?}")]
    EpochGap {
        received: SessionEpoch,
        expected: SessionEpoch,
    },
    #[error("presentation delta starts at {received:?}, expected {expected:?}")]
    RevisionGap {
        received: Revision,
        expected: Revision,
    },
    #[error("presentation revision did not advance by one from {base:?} to {revision:?}")]
    NonMonotonic { base: Revision, revision: Revision },
}

/// Revision gate shared by every renderer. A delta can only be applied to the
/// exact snapshot it names; a full snapshot is the explicit resynchronization
/// mechanism after a gap or reconnect.
#[derive(Debug, Clone, Default)]
pub struct PresentationStreamState {
    current: Option<SessionPresentationSnapshot>,
}

impl PresentationStreamState {
    pub fn apply(&mut self, update: &PresentationUpdate) -> Result<(), PresentationStreamError> {
        let snapshot = match update {
            PresentationUpdate::Full(snapshot) => snapshot.clone(),
            PresentationUpdate::Delta(delta) => {
                if delta.snapshot.epoch != delta.epoch || delta.snapshot.revision != delta.revision
                {
                    return Err(PresentationStreamError::SnapshotMismatch);
                }
                let Some(current) = self.current.as_ref() else {
                    return Err(PresentationStreamError::RevisionGap {
                        received: delta.base_revision,
                        expected: Revision(0),
                    });
                };
                if delta.epoch != current.epoch {
                    return Err(PresentationStreamError::EpochGap {
                        received: delta.epoch,
                        expected: current.epoch,
                    });
                }
                if delta.base_revision != current.revision {
                    return Err(PresentationStreamError::RevisionGap {
                        received: delta.base_revision,
                        expected: current.revision,
                    });
                }
                if delta.revision.0 != delta.base_revision.0.saturating_add(1) {
                    return Err(PresentationStreamError::NonMonotonic {
                        base: delta.base_revision,
                        revision: delta.revision,
                    });
                }
                delta.snapshot.clone()
            }
        };
        self.current = Some(snapshot);
        Ok(())
    }

    pub fn current(&self) -> Option<&SessionPresentationSnapshot> {
        self.current.as_ref()
    }
}

/// Represents a dirty region in logical buffer coordinates
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirtyRegion {
    /// Single line needs redrawing
    Line { buffer_id: BufferId, line: usize },
    /// Range of lines need redrawing
    #[allow(dead_code)]
    LineRange {
        buffer_id: BufferId,
        start_line: usize,
        end_line: usize,
    },
    /// Specific character range (for highlighting, cursors, etc.)
    #[allow(dead_code)]
    CharRange {
        buffer_id: BufferId,
        start_char: usize,
        end_char: usize,
    },
    /// Entire buffer needs redrawing
    Buffer { buffer_id: BufferId },
    /// Window chrome (borders, modeline) needs redrawing
    #[allow(dead_code)]
    WindowChrome { window_id: WindowId },
    /// Specific modeline component needs updating
    Modeline {
        window_id: WindowId,
        component: ModelineComponent,
    },
    /// Entire screen needs redrawing (layout changes, etc.)
    FullScreen,
}

/// Components of the modeline that can be updated independently
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ModelineComponent {
    /// Cursor position (line:col)
    CursorPosition,
    /// Buffer name/object
    #[allow(dead_code)]
    BufferName,
    /// Mode name
    #[allow(dead_code)]
    ModeName,
    /// All components (equivalent to WindowChrome)
    All,
}
