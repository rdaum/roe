// Copyright (C) 2025 Ryan Daum <ryan.daum@gmail.com> This program is free
// software: you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation, version
// 3.

//! Renderer- and policy-neutral native editor mechanisms.
//!
//! The kernel owns ephemeral resources and enforces mechanical invariants. It
//! deliberately does not know command names, keymaps, modes, completion, or
//! hooks. Its public identifiers and messages are owned serde values so the
//! same contract can later cross a process boundary without exposing SlotMap
//! keys, Rust references, or renderer objects.

use crate::Buffer;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Generation-checked identity for an ephemeral native resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceId {
    pub slot: u32,
    pub generation: u32,
}

/// Transport identity for a logical view. This is not a renderer handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ViewId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Capability {
    TextRead,
    TextWrite,
    Layout,
    FileRead,
    FileWrite,
    ClipboardRead,
    ClipboardWrite,
    ClockRead,
    ProcessSpawn,
    Watch,
}

/// Explicit authority supplied when a session is opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityGrants {
    grants: BTreeSet<Capability>,
}

impl CapabilityGrants {
    pub fn new(grants: impl IntoIterator<Item = Capability>) -> Self {
        Self {
            grants: grants.into_iter().collect(),
        }
    }

    pub fn editor_default() -> Self {
        Self::new([
            Capability::TextRead,
            Capability::TextWrite,
            Capability::Layout,
            Capability::FileRead,
            Capability::FileWrite,
            Capability::ClipboardRead,
            Capability::ClipboardWrite,
            Capability::ClockRead,
            Capability::ProcessSpawn,
            Capability::Watch,
        ])
    }

    pub fn contains(&self, capability: Capability) -> bool {
        self.grants.contains(&capability)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSelection {
    pub anchor: usize,
    pub active: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSnapshot {
    pub resource: ResourceId,
    pub name: String,
    pub text: String,
    pub character_len: usize,
    pub line_count: usize,
    pub selection: Option<TextSelection>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LayoutNode {
    View(ViewId),
    Split {
        axis: SplitAxis,
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogicalLayout {
    pub columns: u16,
    pub rows: u16,
    pub active: ViewId,
    pub root: LayoutNode,
}

/// Native operations are mechanism requests, never editor commands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NativeOperation {
    CreateText {
        name: String,
        initial: String,
    },
    CloseResource {
        resource: ResourceId,
    },
    Snapshot {
        resource: ResourceId,
    },
    Insert {
        resource: ResourceId,
        at: usize,
        text: String,
    },
    Delete {
        resource: ResourceId,
        start: usize,
        end: usize,
    },
    Replace {
        resource: ResourceId,
        start: usize,
        end: usize,
        text: String,
    },
    SetSelection {
        resource: ResourceId,
        selection: Option<TextSelection>,
    },
    Undo {
        resource: ResourceId,
    },
    Redo {
        resource: ResourceId,
    },
    ValidateLayout {
        layout: LogicalLayout,
    },
    ReadFile {
        path: PathBuf,
    },
    WriteFile {
        path: PathBuf,
        contents: String,
    },
    ReadClipboard,
    WriteClipboard {
        contents: String,
    },
    ReadClockMillis,
    SpawnProcess {
        program: String,
        args: Vec<String>,
    },
    RegisterWatch {
        resource: ResourceId,
        path: PathBuf,
    },
    UnregisterWatch {
        resource: ResourceId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeResult {
    ResourceCreated(ResourceId),
    ResourceClosed,
    Snapshot(TextSnapshot),
    TextChanged(TextSnapshot),
    LayoutValidated,
    FileContents(String),
    FileWritten,
    ClipboardContents(String),
    ClipboardWritten,
    ClockMillis(u64),
    ProcessOutput {
        status: Option<i32>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    WatchRegistered,
    WatchUnregistered,
}

#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("native capability {0:?} was not granted")]
    CapabilityDenied(Capability),
    #[error("native resource {0:?} is stale or unknown")]
    StaleResource(ResourceId),
    #[error("text range {start}..{end} is outside resource length {len}")]
    InvalidRange {
        start: usize,
        end: usize,
        len: usize,
    },
    #[error("invalid logical layout: {0}")]
    InvalidLayout(String),
    #[error("native I/O operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("clipboard operation failed: {0}")]
    Clipboard(String),
}

struct TextResource {
    buffer: Buffer,
    selection: Option<TextSelection>,
    watched_path: Option<PathBuf>,
}

struct ResourceSlot {
    generation: u32,
    resource: Option<TextResource>,
}

/// In-process implementation of the native kernel contract.
pub struct NativeKernel {
    grants: CapabilityGrants,
    slots: Vec<ResourceSlot>,
    free: Vec<u32>,
}

impl NativeKernel {
    pub fn new(grants: CapabilityGrants) -> Self {
        Self {
            grants,
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    pub fn grants(&self) -> &CapabilityGrants {
        &self.grants
    }

    /// Register an existing Roe buffer without copying its Rope storage.
    pub fn register_buffer(&mut self, buffer: Buffer) -> ResourceId {
        self.allocate(TextResource {
            buffer,
            selection: None,
            watched_path: None,
        })
    }

    pub fn execute(&mut self, operation: NativeOperation) -> Result<NativeResult, KernelError> {
        match operation {
            NativeOperation::CreateText { name, initial } => {
                self.require(Capability::TextWrite)?;
                let buffer = Buffer::new(&[]);
                buffer.set_object(name);
                buffer.load_str(&initial);
                Ok(NativeResult::ResourceCreated(self.register_buffer(buffer)))
            }
            NativeOperation::CloseResource { resource } => {
                self.require(Capability::TextWrite)?;
                self.close(resource)?;
                Ok(NativeResult::ResourceClosed)
            }
            NativeOperation::Snapshot { resource } => {
                self.require(Capability::TextRead)?;
                Ok(NativeResult::Snapshot(self.snapshot(resource)?))
            }
            NativeOperation::Insert { resource, at, text } => {
                self.require(Capability::TextWrite)?;
                let entry = self.resource_mut(resource)?;
                let len = entry.buffer.buffer_len_chars();
                validate_range(at, at, len)?;
                entry.buffer.insert_pos(text, at);
                Ok(NativeResult::TextChanged(snapshot_entry(resource, entry)))
            }
            NativeOperation::Delete {
                resource,
                start,
                end,
            } => {
                self.require(Capability::TextWrite)?;
                let entry = self.resource_mut(resource)?;
                let len = entry.buffer.buffer_len_chars();
                validate_range(start, end, len)?;
                entry.buffer.delete_pos(start, (end - start) as isize);
                clamp_selection(entry);
                Ok(NativeResult::TextChanged(snapshot_entry(resource, entry)))
            }
            NativeOperation::Replace {
                resource,
                start,
                end,
                text,
            } => {
                self.require(Capability::TextWrite)?;
                let entry = self.resource_mut(resource)?;
                let len = entry.buffer.buffer_len_chars();
                validate_range(start, end, len)?;
                entry.buffer.begin_undo_group();
                entry.buffer.delete_pos(start, (end - start) as isize);
                entry.buffer.insert_pos(text, start);
                entry.buffer.end_undo_group();
                clamp_selection(entry);
                Ok(NativeResult::TextChanged(snapshot_entry(resource, entry)))
            }
            NativeOperation::SetSelection {
                resource,
                selection,
            } => {
                self.require(Capability::TextWrite)?;
                let entry = self.resource_mut(resource)?;
                if let Some(selection) = selection {
                    let len = entry.buffer.buffer_len_chars();
                    validate_range(selection.anchor, selection.anchor, len)?;
                    validate_range(selection.active, selection.active, len)?;
                }
                entry.selection = selection;
                Ok(NativeResult::TextChanged(snapshot_entry(resource, entry)))
            }
            NativeOperation::Undo { resource } => {
                self.require(Capability::TextWrite)?;
                let entry = self.resource_mut(resource)?;
                entry.buffer.undo();
                clamp_selection(entry);
                Ok(NativeResult::TextChanged(snapshot_entry(resource, entry)))
            }
            NativeOperation::Redo { resource } => {
                self.require(Capability::TextWrite)?;
                let entry = self.resource_mut(resource)?;
                entry.buffer.redo();
                clamp_selection(entry);
                Ok(NativeResult::TextChanged(snapshot_entry(resource, entry)))
            }
            NativeOperation::ValidateLayout { layout } => {
                self.require(Capability::Layout)?;
                validate_layout(&layout)?;
                Ok(NativeResult::LayoutValidated)
            }
            NativeOperation::ReadFile { path } => {
                self.require(Capability::FileRead)?;
                Ok(NativeResult::FileContents(std::fs::read_to_string(path)?))
            }
            NativeOperation::WriteFile { path, contents } => {
                self.require(Capability::FileWrite)?;
                std::fs::write(path, contents)?;
                Ok(NativeResult::FileWritten)
            }
            NativeOperation::ReadClipboard => {
                self.require(Capability::ClipboardRead)?;
                let mut clipboard = arboard::Clipboard::new()
                    .map_err(|error| KernelError::Clipboard(error.to_string()))?;
                let contents = clipboard
                    .get_text()
                    .map_err(|error| KernelError::Clipboard(error.to_string()))?;
                Ok(NativeResult::ClipboardContents(contents))
            }
            NativeOperation::WriteClipboard { contents } => {
                self.require(Capability::ClipboardWrite)?;
                let mut clipboard = arboard::Clipboard::new()
                    .map_err(|error| KernelError::Clipboard(error.to_string()))?;
                clipboard
                    .set_text(contents)
                    .map_err(|error| KernelError::Clipboard(error.to_string()))?;
                Ok(NativeResult::ClipboardWritten)
            }
            NativeOperation::ReadClockMillis => {
                self.require(Capability::ClockRead)?;
                let millis = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64;
                Ok(NativeResult::ClockMillis(millis))
            }
            NativeOperation::SpawnProcess { program, args } => {
                self.require(Capability::ProcessSpawn)?;
                let output = std::process::Command::new(program).args(args).output()?;
                Ok(NativeResult::ProcessOutput {
                    status: output.status.code(),
                    stdout: output.stdout,
                    stderr: output.stderr,
                })
            }
            NativeOperation::RegisterWatch { resource, path } => {
                self.require(Capability::Watch)?;
                self.resource_mut(resource)?.watched_path = Some(path);
                Ok(NativeResult::WatchRegistered)
            }
            NativeOperation::UnregisterWatch { resource } => {
                self.require(Capability::Watch)?;
                self.resource_mut(resource)?.watched_path = None;
                Ok(NativeResult::WatchUnregistered)
            }
        }
    }

    pub fn snapshot(&self, resource: ResourceId) -> Result<TextSnapshot, KernelError> {
        let entry = self.resource(resource)?;
        Ok(snapshot_entry(resource, entry))
    }

    fn require(&self, capability: Capability) -> Result<(), KernelError> {
        if self.grants.contains(capability) {
            Ok(())
        } else {
            Err(KernelError::CapabilityDenied(capability))
        }
    }

    fn allocate(&mut self, resource: TextResource) -> ResourceId {
        if let Some(slot) = self.free.pop() {
            let entry = &mut self.slots[slot as usize];
            debug_assert!(entry.resource.is_none());
            entry.resource = Some(resource);
            ResourceId {
                slot,
                generation: entry.generation,
            }
        } else {
            let slot = self.slots.len() as u32;
            self.slots.push(ResourceSlot {
                generation: 1,
                resource: Some(resource),
            });
            ResourceId {
                slot,
                generation: 1,
            }
        }
    }

    fn close(&mut self, id: ResourceId) -> Result<(), KernelError> {
        let Some(slot) = self.slots.get_mut(id.slot as usize) else {
            return Err(KernelError::StaleResource(id));
        };
        if slot.generation != id.generation || slot.resource.is_none() {
            return Err(KernelError::StaleResource(id));
        }
        slot.resource = None;
        slot.generation = slot.generation.wrapping_add(1).max(1);
        self.free.push(id.slot);
        Ok(())
    }

    fn resource(&self, id: ResourceId) -> Result<&TextResource, KernelError> {
        self.slots
            .get(id.slot as usize)
            .filter(|slot| slot.generation == id.generation)
            .and_then(|slot| slot.resource.as_ref())
            .ok_or(KernelError::StaleResource(id))
    }

    fn resource_mut(&mut self, id: ResourceId) -> Result<&mut TextResource, KernelError> {
        self.slots
            .get_mut(id.slot as usize)
            .filter(|slot| slot.generation == id.generation)
            .and_then(|slot| slot.resource.as_mut())
            .ok_or(KernelError::StaleResource(id))
    }
}

fn validate_range(start: usize, end: usize, len: usize) -> Result<(), KernelError> {
    if start <= end && end <= len {
        Ok(())
    } else {
        Err(KernelError::InvalidRange { start, end, len })
    }
}

fn snapshot_entry(resource: ResourceId, entry: &TextResource) -> TextSnapshot {
    TextSnapshot {
        resource,
        name: entry.buffer.object(),
        text: entry.buffer.content(),
        character_len: entry.buffer.buffer_len_chars(),
        line_count: entry.buffer.buffer_len_lines(),
        selection: entry.selection,
    }
}

fn clamp_selection(entry: &mut TextResource) {
    let len = entry.buffer.buffer_len_chars();
    if let Some(selection) = &mut entry.selection {
        selection.anchor = selection.anchor.min(len);
        selection.active = selection.active.min(len);
    }
}

pub fn validate_layout(layout: &LogicalLayout) -> Result<(), KernelError> {
    if layout.columns == 0 || layout.rows == 0 {
        return Err(KernelError::InvalidLayout(
            "frame dimensions must be non-zero".to_string(),
        ));
    }
    let mut views = HashSet::new();
    validate_layout_node(&layout.root, &mut views)?;
    if !views.contains(&layout.active) {
        return Err(KernelError::InvalidLayout(
            "active view is not present in the layout".to_string(),
        ));
    }
    Ok(())
}

fn validate_layout_node(node: &LayoutNode, views: &mut HashSet<ViewId>) -> Result<(), KernelError> {
    match node {
        LayoutNode::View(view) => {
            if views.insert(*view) {
                Ok(())
            } else {
                Err(KernelError::InvalidLayout(
                    "a view appears more than once".to_string(),
                ))
            }
        }
        LayoutNode::Split {
            ratio,
            first,
            second,
            ..
        } => {
            if !ratio.is_finite() || *ratio <= 0.0 || *ratio >= 1.0 {
                return Err(KernelError::InvalidLayout(
                    "split ratio must be finite and strictly between zero and one".to_string(),
                ));
            }
            validate_layout_node(first, views)?;
            validate_layout_node(second, views)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_kernel() -> NativeKernel {
        NativeKernel::new(CapabilityGrants::new([
            Capability::TextRead,
            Capability::TextWrite,
            Capability::Layout,
        ]))
    }

    fn created_id(result: NativeResult) -> ResourceId {
        match result {
            NativeResult::ResourceCreated(id) => id,
            other => panic!("expected resource identity, got {other:?}"),
        }
    }

    #[test]
    fn stale_generation_cannot_access_reused_slot() {
        let mut kernel = text_kernel();
        let first = created_id(
            kernel
                .execute(NativeOperation::CreateText {
                    name: "first".to_string(),
                    initial: "one".to_string(),
                })
                .unwrap(),
        );
        kernel
            .execute(NativeOperation::CloseResource { resource: first })
            .unwrap();
        let second = created_id(
            kernel
                .execute(NativeOperation::CreateText {
                    name: "second".to_string(),
                    initial: "two".to_string(),
                })
                .unwrap(),
        );
        assert_eq!(first.slot, second.slot);
        assert_ne!(first.generation, second.generation);
        assert!(matches!(
            kernel.execute(NativeOperation::Snapshot { resource: first }),
            Err(KernelError::StaleResource(id)) if id == first
        ));
    }

    #[test]
    fn unicode_mutations_use_character_ranges_and_round_trip_undo() {
        let mut kernel = text_kernel();
        let id = created_id(
            kernel
                .execute(NativeOperation::CreateText {
                    name: "unicode".to_string(),
                    initial: "aéz".to_string(),
                })
                .unwrap(),
        );
        kernel
            .execute(NativeOperation::Replace {
                resource: id,
                start: 1,
                end: 2,
                text: "🦀".to_string(),
            })
            .unwrap();
        assert_eq!(kernel.snapshot(id).unwrap().text, "a🦀z");
        kernel
            .execute(NativeOperation::Undo { resource: id })
            .unwrap();
        assert_eq!(kernel.snapshot(id).unwrap().text, "aéz");
        kernel
            .execute(NativeOperation::Redo { resource: id })
            .unwrap();
        assert_eq!(kernel.snapshot(id).unwrap().text, "a🦀z");
    }

    #[test]
    fn capability_is_checked_before_resource_disclosure() {
        let mut kernel = NativeKernel::new(CapabilityGrants::new([Capability::TextWrite]));
        let id = created_id(
            kernel
                .execute(NativeOperation::CreateText {
                    name: "secret".to_string(),
                    initial: "classified".to_string(),
                })
                .unwrap(),
        );
        assert!(matches!(
            kernel.execute(NativeOperation::Snapshot { resource: id }),
            Err(KernelError::CapabilityDenied(Capability::TextRead))
        ));
    }

    #[test]
    fn layout_validation_rejects_duplicate_and_missing_active_views() {
        let duplicate = LogicalLayout {
            columns: 80,
            rows: 24,
            active: ViewId(1),
            root: LayoutNode::Split {
                axis: SplitAxis::Horizontal,
                ratio: 0.5,
                first: Box::new(LayoutNode::View(ViewId(1))),
                second: Box::new(LayoutNode::View(ViewId(1))),
            },
        };
        assert!(validate_layout(&duplicate).is_err());

        let missing = LogicalLayout {
            columns: 80,
            rows: 24,
            active: ViewId(2),
            root: LayoutNode::View(ViewId(1)),
        };
        assert!(validate_layout(&missing).is_err());
    }

    #[test]
    fn out_of_range_mutation_is_atomic() {
        let mut kernel = text_kernel();
        let id = created_id(
            kernel
                .execute(NativeOperation::CreateText {
                    name: "range".to_string(),
                    initial: "abc".to_string(),
                })
                .unwrap(),
        );
        assert!(matches!(
            kernel.execute(NativeOperation::Delete {
                resource: id,
                start: 2,
                end: 9,
            }),
            Err(KernelError::InvalidRange { .. })
        ));
        assert_eq!(kernel.snapshot(id).unwrap().text, "abc");
    }
}
