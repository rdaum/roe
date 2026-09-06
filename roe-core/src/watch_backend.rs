// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Bounded notification delivery and transactional watch ownership.
//! Buffer synchronization and native resource authority belong to callers.

use crate::native_services::FrontendWake;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, RwLock};

pub(crate) const EVENT_QUEUE_CAPACITY: usize = 256;
const MAX_REGISTRATIONS: usize = 1024;

#[derive(Debug)]
pub(crate) struct WatchHint<Owner> {
    pub owner: Owner,
    pub path: PathBuf,
}

pub(crate) struct WatchBackend<Owner> {
    watcher: Option<RecommendedWatcher>,
    owners: HashMap<Owner, PathBuf>,
    registrations: Arc<RwLock<HashMap<PathBuf, HashSet<Owner>>>>,
    parents: HashMap<PathBuf, usize>,
    // Revocation cannot retain an owner. Failed cleanup retains only a backend path.
    orphaned_parents: HashSet<PathBuf>,
    event_tx: SyncSender<WatchHint<Owner>>,
    event_rx: Receiver<WatchHint<Owner>>,
    backend_error: Arc<Mutex<Option<String>>>,
    wake: Arc<RwLock<Option<Arc<dyn FrontendWake>>>>,
}

impl<Owner: Copy + Eq + Hash + Send + Sync + Debug + 'static> WatchBackend<Owner> {
    pub(crate) fn new() -> Self {
        let (event_tx, event_rx) = sync_channel(EVENT_QUEUE_CAPACITY);
        Self {
            watcher: None,
            owners: HashMap::new(),
            registrations: Arc::new(RwLock::new(HashMap::new())),
            parents: HashMap::new(),
            orphaned_parents: HashSet::new(),
            event_tx,
            event_rx,
            backend_error: Arc::new(Mutex::new(None)),
            wake: Arc::new(RwLock::new(None)),
        }
    }

    pub(crate) fn init(&mut self) -> Result<(), notify::Error> {
        if self.watcher.is_some() {
            return Ok(());
        }
        let registrations = self.registrations.clone();
        let tx = self.event_tx.clone();
        let backend_error = self.backend_error.clone();
        let wake = self.wake.clone();
        self.watcher = Some(notify::recommended_watcher(
            move |result: Result<notify::Event, notify::Error>| {
                match result {
                    Err(error) => {
                        tracing::warn!(%error, "watch backend error");
                        *backend_error.lock().expect("watch error lock poisoned") =
                            Some(error.to_string());
                    }
                    Ok(event)
                        if matches!(
                            event.kind,
                            EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                        ) =>
                    {
                        for path in event.paths {
                            let path = path.canonicalize().unwrap_or(path);
                            let owners = registrations
                                .read()
                                .expect("watch registration lock poisoned")
                                .get(&path)
                                .cloned()
                                .unwrap_or_default();
                            for owner in owners {
                                match tx.try_send(WatchHint {
                                    owner,
                                    path: path.clone(),
                                }) {
                                    Ok(()) => {}
                                    Err(TrySendError::Full(_)) => {
                                        // One replaceable diagnostic, not one retained error per hint.
                                        *backend_error.lock().expect("watch error lock poisoned") =
                                            Some(format!(
                                                "watch hint queue reached its {EVENT_QUEUE_CAPACITY}-event limit"
                                            ));
                                    }
                                    Err(TrySendError::Disconnected(_)) => return,
                                }
                            }
                        }
                    }
                    Ok(_) => return,
                }
                let wake = wake.read().expect("watch wake lock poisoned").clone();
                if let Some(wake) = wake {
                    wake.wake();
                }
            },
        )?);
        Ok(())
    }

    pub(crate) fn register(&mut self, owner: Owner, path: &Path) -> Result<PathBuf, notify::Error> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(current) = self.owners.get(&owner) {
            if *current == path {
                return Ok(path);
            }
            return Err(notify::Error::generic("a live watch cannot be rebound")
                .add_path(current.clone())
                .add_path(path));
        }
        let new_parent = path.parent().is_some_and(|parent| {
            !self.parents.contains_key(parent) && !self.orphaned_parents.contains(parent)
        });
        if self.owners.len() >= MAX_REGISTRATIONS
            || (new_parent && self.parents.len() + self.orphaned_parents.len() >= MAX_REGISTRATIONS)
        {
            return Err(notify::Error::generic("watch registration limit reached"));
        }
        self.init()?;
        if let Some(parent) = path.parent() {
            if !self.parents.contains_key(parent) {
                self.watcher
                    .as_mut()
                    .expect("watcher initialized")
                    .watch(parent, RecursiveMode::NonRecursive)?;
                self.orphaned_parents.remove(parent);
            }
            *self.parents.entry(parent.to_path_buf()).or_default() += 1;
        }
        self.registrations
            .write()
            .expect("watch registration lock poisoned")
            .entry(path.clone())
            .or_default()
            .insert(owner);
        self.owners.insert(owner, path.clone());
        Ok(path)
    }

    pub(crate) fn unregister(&mut self, owner: Owner) -> Result<(), notify::Error> {
        let Some(path) = self.owners.get(&owner) else {
            return Ok(());
        };
        if let Some(parent) = path.parent()
            && self.parents.get(parent) == Some(&1)
            && let Some(watcher) = self.watcher.as_mut()
        {
            // Keep all ownership intact until the last fallible step succeeds.
            watcher.unwatch(parent)?;
        }
        self.remove_owner(owner, false);
        Ok(())
    }

    /// Revoke a dead owner after failed cleanup. No subsequent hint can authorize it.
    pub(crate) fn forget(&mut self, owner: Owner) {
        self.remove_owner(owner, true);
    }

    fn remove_owner(&mut self, owner: Owner, retain_cleanup: bool) {
        let Some(path) = self.owners.remove(&owner) else {
            return;
        };
        let mut registrations = self
            .registrations
            .write()
            .expect("watch registration lock poisoned");
        if let Some(owners) = registrations.get_mut(&path) {
            owners.remove(&owner);
            if owners.is_empty() {
                registrations.remove(&path);
            }
        }
        if let Some(parent) = path.parent()
            && let Some(count) = self.parents.get_mut(parent)
        {
            *count -= 1;
            if *count == 0 {
                self.parents.remove(parent);
                if retain_cleanup {
                    self.orphaned_parents.insert(parent.to_path_buf());
                }
            }
        }
    }

    pub(crate) fn poll(&self) -> Vec<WatchHint<Owner>> {
        // A producer can keep sending during a drain. Bound work as well as storage.
        self.event_rx
            .try_iter()
            .take(EVENT_QUEUE_CAPACITY)
            .collect()
    }

    pub(crate) fn take_error(&self) -> Option<String> {
        self.backend_error
            .lock()
            .expect("watch error lock poisoned")
            .take()
    }

    pub(crate) fn set_wake(&mut self, wake: Option<Arc<dyn FrontendWake>>) {
        *self.wake.write().expect("watch wake lock poisoned") = wake;
    }

    pub(crate) fn shutdown(&mut self) -> Vec<String> {
        self.set_wake(None);
        let mut errors = Vec::new();
        if let Some(watcher) = self.watcher.as_mut() {
            for path in self.parents.keys().chain(self.orphaned_parents.iter()) {
                if let Err(error) = watcher.unwatch(path) {
                    errors.push(error.to_string());
                }
            }
        }
        self.watcher = None;
        self.owners.clear();
        self.parents.clear();
        self.orphaned_parents.clear();
        self.registrations
            .write()
            .expect("watch registration lock poisoned")
            .clear();
        self.poll();
        errors
    }

    pub(crate) fn is_initialized(&self) -> bool {
        self.watcher.is_some()
    }

    pub(crate) fn paths(&self) -> Vec<PathBuf> {
        self.registrations
            .read()
            .expect("watch registration lock poisoned")
            .keys()
            .cloned()
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn parent_count(&self, parent: &Path) -> Option<usize> {
        self.parents.get(parent).copied()
    }
    #[cfg(test)]
    pub(crate) fn force_unwatch(&mut self, parent: &Path) -> Result<(), notify::Error> {
        self.watcher
            .as_mut()
            .expect("test watcher initialized")
            .unwatch(parent)
    }
    #[cfg(test)]
    pub(crate) fn inject(
        &self,
        owner: Owner,
        path: PathBuf,
    ) -> Result<(), TrySendError<WatchHint<Owner>>> {
        self.event_tx.try_send(WatchHint { owner, path })
    }
    #[cfg(test)]
    pub(crate) fn inject_error(&self, message: &str) {
        *self.backend_error.lock().unwrap() = Some(message.to_owned());
    }
}
