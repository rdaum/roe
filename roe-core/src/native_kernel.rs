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
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

const WATCH_EVENT_CAPACITY: usize = 256;
pub const MAX_NATIVE_RESOURCES: usize = 1_024;

/// Wall-clock source for native time requests. Tests inject this boundary so
/// Mica command policy remains deterministic without gaining a clock builtin.
pub trait NativeClock: Send + Sync {
    fn unix_millis(&self) -> u64;
}

#[derive(Default)]
pub struct SystemNativeClock;

impl NativeClock for SystemNativeClock {
    fn unix_millis(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeWatchNotification {
    pub resource: ResourceId,
    pub path: PathBuf,
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
    ListDirectory {
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
    TextChanged {
        resource: ResourceId,
        character_len: usize,
        line_count: usize,
        selection: Option<TextSelection>,
    },
    LayoutValidated,
    FileContents(String),
    DirectoryEntries(Vec<PathBuf>),
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
    #[error("native resource capacity of {capacity} is exhausted")]
    ResourceExhausted { capacity: usize },
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
    #[error("native file-watch operation failed: {0}")]
    Watch(#[from] notify::Error),
    #[error("native resource was revoked, but cleanup failed: {0}")]
    Cleanup(String),
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
    clock: Arc<dyn NativeClock>,
    slots: Vec<ResourceSlot>,
    free: Vec<u32>,
    watch_service: NativeWatchService,
}

struct NativeWatchService {
    watcher: Option<RecommendedWatcher>,
    registrations: Arc<RwLock<HashMap<PathBuf, HashSet<ResourceId>>>>,
    watched_parent_counts: HashMap<PathBuf, usize>,
    event_tx: SyncSender<NativeWatchNotification>,
    event_rx: Receiver<NativeWatchNotification>,
    backend_error: Arc<Mutex<Option<String>>>,
}

impl NativeKernel {
    pub fn new(grants: CapabilityGrants) -> Self {
        Self::with_clock(grants, Arc::new(SystemNativeClock))
    }

    pub fn with_clock(grants: CapabilityGrants, clock: Arc<dyn NativeClock>) -> Self {
        let (event_tx, event_rx) = sync_channel(WATCH_EVENT_CAPACITY);
        Self {
            grants,
            clock,
            slots: Vec::new(),
            free: Vec::new(),
            watch_service: NativeWatchService {
                watcher: None,
                registrations: Arc::new(RwLock::new(HashMap::new())),
                watched_parent_counts: HashMap::new(),
                event_tx,
                event_rx,
                backend_error: Arc::new(Mutex::new(None)),
            },
        }
    }

    pub fn grants(&self) -> &CapabilityGrants {
        &self.grants
    }

    /// Check endpoint-native authority before a host realizes a Mica effect.
    pub fn authorize(&self, capability: Capability) -> Result<(), KernelError> {
        self.require(capability)
    }

    /// Register an existing Roe buffer without copying its Rope storage.
    pub fn register_buffer(&mut self, buffer: Buffer) -> Result<ResourceId, KernelError> {
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
                let buffer = Buffer::new();
                buffer.set_object(name);
                buffer.load_str(&initial);
                Ok(NativeResult::ResourceCreated(self.register_buffer(buffer)?))
            }
            NativeOperation::CloseResource { resource } => {
                self.require(Capability::TextWrite)?;
                if let Some(error) = self.close(resource)? {
                    return Err(KernelError::Cleanup(error));
                }
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
                Ok(text_changed(resource, entry))
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
                Ok(text_changed(resource, entry))
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
                Ok(text_changed(resource, entry))
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
                Ok(text_changed(resource, entry))
            }
            NativeOperation::Undo { resource } => {
                self.require(Capability::TextWrite)?;
                let entry = self.resource_mut(resource)?;
                entry.buffer.undo();
                clamp_selection(entry);
                Ok(text_changed(resource, entry))
            }
            NativeOperation::Redo { resource } => {
                self.require(Capability::TextWrite)?;
                let entry = self.resource_mut(resource)?;
                entry.buffer.redo();
                clamp_selection(entry);
                Ok(text_changed(resource, entry))
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
            NativeOperation::ListDirectory { path } => {
                self.require(Capability::FileRead)?;
                let mut entries = std::fs::read_dir(path)?
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .collect::<Vec<_>>();
                entries.sort();
                Ok(NativeResult::DirectoryEntries(entries))
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
                Ok(NativeResult::ClockMillis(self.clock.unix_millis()))
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
                self.register_watch(resource, path)?;
                Ok(NativeResult::WatchRegistered)
            }
            NativeOperation::UnregisterWatch { resource } => {
                self.require(Capability::Watch)?;
                self.unregister_watch(resource)?;
                Ok(NativeResult::WatchUnregistered)
            }
        }
    }

    /// Drain bounded native file-change hints. Notifications contain only an
    /// ephemeral resource identity and path; the caller rereads authoritative
    /// state and decides policy.
    pub fn poll_watch_notifications(&self) -> Vec<NativeWatchNotification> {
        self.watch_service.event_rx.try_iter().collect()
    }

    pub fn take_watch_error(&self) -> Option<String> {
        self.watch_service
            .backend_error
            .lock()
            .ok()
            .and_then(|mut error| error.take())
    }

    /// Invalidate a host-owned association independently of client grants.
    /// Authority controls client operations; host lifecycle cleanup must
    /// always be able to revoke an ephemeral identity.
    pub(crate) fn invalidate_resource(
        &mut self,
        resource: ResourceId,
    ) -> Result<Option<String>, KernelError> {
        self.close(resource)
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

    fn allocate(&mut self, resource: TextResource) -> Result<ResourceId, KernelError> {
        if let Some(slot) = self.free.pop() {
            let entry = &mut self.slots[slot as usize];
            debug_assert!(entry.resource.is_none());
            entry.resource = Some(resource);
            Ok(ResourceId {
                slot,
                generation: entry.generation,
            })
        } else {
            if self.slots.len() >= MAX_NATIVE_RESOURCES {
                return Err(KernelError::ResourceExhausted {
                    capacity: MAX_NATIVE_RESOURCES,
                });
            }
            let slot = self.slots.len() as u32;
            self.slots.push(ResourceSlot {
                generation: 1,
                resource: Some(resource),
            });
            Ok(ResourceId {
                slot,
                generation: 1,
            })
        }
    }

    fn close(&mut self, id: ResourceId) -> Result<Option<String>, KernelError> {
        // Validate before cleanup, then revoke the generation regardless of a
        // backend-unwatch failure. Native cleanup is fallible; capability
        // revocation is not.
        self.resource(id)?;
        let cleanup_error = match self.unregister_watch(id) {
            Ok(()) => None,
            Err(error) => {
                let message = error.to_string();
                self.forget_watch_registration(id);
                Some(message)
            }
        };
        let Some(slot) = self.slots.get_mut(id.slot as usize) else {
            return Err(KernelError::StaleResource(id));
        };
        if slot.generation != id.generation || slot.resource.is_none() {
            return Err(KernelError::StaleResource(id));
        }
        slot.resource = None;
        slot.generation = slot.generation.wrapping_add(1).max(1);
        self.free.push(id.slot);
        Ok(cleanup_error)
    }

    fn register_watch(&mut self, resource: ResourceId, path: PathBuf) -> Result<(), KernelError> {
        let current = self.resource(resource)?.watched_path.clone();
        let canonical = path.canonicalize().unwrap_or(path);
        if let Some(current) = current {
            if current == canonical {
                return Ok(());
            }
            return Err(KernelError::Watch(
                notify::Error::generic("a live native watch cannot be rebound")
                    .add_path(current)
                    .add_path(canonical),
            ));
        }

        self.watch_service.ensure_watcher()?;
        let parent = canonical.parent().map(std::path::Path::to_path_buf);
        if let Some(parent) = parent.as_ref()
            && !self
                .watch_service
                .watched_parent_counts
                .contains_key(parent)
            && let Some(watcher) = self.watch_service.watcher.as_mut()
        {
            watcher.watch(parent, RecursiveMode::NonRecursive)?;
        }
        if let Some(parent) = parent {
            *self
                .watch_service
                .watched_parent_counts
                .entry(parent)
                .or_insert(0) += 1;
        }
        if let Ok(mut registrations) = self.watch_service.registrations.write() {
            registrations
                .entry(canonical.clone())
                .or_default()
                .insert(resource);
        }
        self.resource_mut(resource)?.watched_path = Some(canonical);
        Ok(())
    }

    fn unregister_watch(&mut self, resource: ResourceId) -> Result<(), KernelError> {
        let Some(path) = self.resource(resource)?.watched_path.clone() else {
            return Ok(());
        };
        let parent = path.parent().map(std::path::Path::to_path_buf);
        let remove_backend = parent.as_ref().is_some_and(|parent| {
            self.watch_service
                .watched_parent_counts
                .get(parent)
                .copied()
                == Some(1)
        });
        if remove_backend
            && let (Some(watcher), Some(parent)) =
                (self.watch_service.watcher.as_mut(), parent.as_ref())
        {
            watcher.unwatch(parent)?;
        }
        if let Ok(mut registrations) = self.watch_service.registrations.write()
            && let Some(resources) = registrations.get_mut(&path)
        {
            resources.remove(&resource);
            if resources.is_empty() {
                registrations.remove(&path);
            }
        }
        if let Some(parent) = parent {
            match self.watch_service.watched_parent_counts.get_mut(&parent) {
                Some(count) if *count > 1 => *count -= 1,
                Some(_) => {
                    self.watch_service.watched_parent_counts.remove(&parent);
                }
                None => {}
            }
        }
        self.resource_mut(resource)?.watched_path = None;
        Ok(())
    }

    fn forget_watch_registration(&mut self, resource: ResourceId) {
        let Ok(entry) = self.resource(resource) else {
            return;
        };
        let Some(path) = entry.watched_path.clone() else {
            return;
        };
        if let Ok(mut registrations) = self.watch_service.registrations.write()
            && let Some(resources) = registrations.get_mut(&path)
        {
            resources.remove(&resource);
            if resources.is_empty() {
                registrations.remove(&path);
            }
        }
        if let Some(parent) = path.parent() {
            match self.watch_service.watched_parent_counts.get_mut(parent) {
                Some(count) if *count > 1 => *count -= 1,
                Some(_) => {
                    self.watch_service.watched_parent_counts.remove(parent);
                }
                None => {}
            }
        }
        if let Ok(entry) = self.resource_mut(resource) {
            entry.watched_path = None;
        }
    }

    #[cfg(test)]
    pub(crate) fn force_backend_unwatch_for_test(
        &mut self,
        path: &std::path::Path,
    ) -> Result<(), notify::Error> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let parent = canonical.parent().unwrap_or(&canonical);
        self.watch_service
            .watcher
            .as_mut()
            .expect("test watch backend is initialized")
            .unwatch(parent)
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

impl NativeWatchService {
    fn ensure_watcher(&mut self) -> Result<(), notify::Error> {
        if self.watcher.is_some() {
            return Ok(());
        }
        let registrations = self.registrations.clone();
        let tx = self.event_tx.clone();
        let backend_error = self.backend_error.clone();
        self.watcher = Some(notify::recommended_watcher(
            move |result: Result<notify::Event, notify::Error>| {
                let event = match result {
                    Ok(event) => event,
                    Err(error) => {
                        tracing::warn!(%error, "native watcher backend error");
                        if let Ok(mut current) = backend_error.lock() {
                            *current = Some(error.to_string());
                        }
                        return;
                    }
                };
                if !matches!(
                    event.kind,
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                ) {
                    return;
                }
                for path in event.paths {
                    let canonical = path.canonicalize().unwrap_or(path);
                    let resources = registrations
                        .read()
                        .ok()
                        .and_then(|entries| entries.get(&canonical).cloned())
                        .unwrap_or_default();
                    for resource in resources {
                        let notification = NativeWatchNotification {
                            resource,
                            path: canonical.clone(),
                        };
                        match tx.try_send(notification) {
                            Ok(()) => {}
                            Err(TrySendError::Full(notification)) => tracing::warn!(
                                resource = ?notification.resource,
                                path = %notification.path.display(),
                                capacity = WATCH_EVENT_CAPACITY,
                                "native watch queue is full; dropping notification hint"
                            ),
                            Err(TrySendError::Disconnected(_)) => return,
                        }
                    }
                }
            },
        )?);
        Ok(())
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

fn text_changed(resource: ResourceId, entry: &TextResource) -> NativeResult {
    NativeResult::TextChanged {
        resource,
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
    use std::time::{Duration, Instant};

    fn text_kernel() -> NativeKernel {
        NativeKernel::new(CapabilityGrants::new([
            Capability::TextRead,
            Capability::TextWrite,
            Capability::Layout,
        ]))
    }

    #[test]
    fn native_resource_capacity_is_explicit_and_slots_are_reused() {
        let mut kernel = text_kernel();
        let mut resources = Vec::with_capacity(MAX_NATIVE_RESOURCES);
        for _ in 0..MAX_NATIVE_RESOURCES {
            resources.push(kernel.register_buffer(Buffer::new()).unwrap());
        }
        assert!(matches!(
            kernel.register_buffer(Buffer::new()),
            Err(KernelError::ResourceExhausted {
                capacity: MAX_NATIVE_RESOURCES
            })
        ));

        let retired = resources[0];
        kernel
            .execute(NativeOperation::CloseResource { resource: retired })
            .unwrap();
        let reused = kernel.register_buffer(Buffer::new()).unwrap();
        assert_eq!(reused.slot, retired.slot);
        assert_ne!(reused.generation, retired.generation);
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

    #[test]
    fn native_watch_registration_delivers_real_bounded_notifications() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("roe-native-watch-{}-{unique}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("watched.txt");
        std::fs::write(&path, "before").unwrap();

        let mut kernel = NativeKernel::new(CapabilityGrants::new([Capability::Watch]));
        let resource = kernel.register_buffer(Buffer::new()).unwrap();
        kernel
            .execute(NativeOperation::RegisterWatch {
                resource,
                path: path.clone(),
            })
            .unwrap();
        assert!(kernel.watch_service.watcher.is_some());

        std::fs::write(&path, "after").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let notification = loop {
            if let Some(notification) = kernel.poll_watch_notifications().into_iter().next() {
                break notification;
            }
            assert!(Instant::now() < deadline, "native watcher did not deliver");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(notification.resource, resource);
        assert_eq!(notification.path, path.canonicalize().unwrap());

        kernel
            .execute(NativeOperation::UnregisterWatch { resource })
            .unwrap();
        assert!(
            kernel
                .watch_service
                .registrations
                .read()
                .unwrap()
                .is_empty()
        );
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn host_invalidation_revokes_generation_when_backend_cleanup_fails() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("roe-native-revoke-{}-{unique}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("watched.txt");
        std::fs::write(&path, "content").unwrap();

        let mut kernel = NativeKernel::new(CapabilityGrants::new([Capability::Watch]));
        let resource = kernel.register_buffer(Buffer::new()).unwrap();
        kernel
            .execute(NativeOperation::RegisterWatch {
                resource,
                path: path.clone(),
            })
            .unwrap();
        kernel.force_backend_unwatch_for_test(&path).unwrap();

        let cleanup_error = kernel.invalidate_resource(resource).unwrap();
        assert!(cleanup_error.is_some());
        assert!(matches!(
            kernel.snapshot(resource),
            Err(KernelError::StaleResource(id)) if id == resource
        ));
        assert!(
            kernel
                .watch_service
                .registrations
                .read()
                .unwrap()
                .is_empty()
        );
        assert!(kernel.watch_service.watched_parent_counts.is_empty());

        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
