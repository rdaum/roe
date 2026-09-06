// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Live-buffer source overlay. Driver lifecycle and effect dispatch stay in the host.

use super::{ROE_BUFFER_SOURCE_PROVIDER, sym};
use crate::Editor;
use mica_driver::{
    Identity, ListRequest, ProviderResult, ReadRequest, SourceCapabilities, SourceDocument,
    SourceEntry, SourceFailure, SourceProvider, SourceProviderKey, Symbol, Value,
};
use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

#[derive(Default)]
pub(super) struct RoeSourceBuffers {
    by_path: HashMap<PathBuf, crate::Buffer>,
}

pub(super) struct RoeBufferSourceProvider {
    pub(super) root: PathBuf,
    pub(super) buffers: Arc<RwLock<RoeSourceBuffers>>,
}

impl SourceProvider for RoeBufferSourceProvider {
    fn key(&self) -> SourceProviderKey {
        SourceProviderKey::new(ROE_BUFFER_SOURCE_PROVIDER)
    }

    fn name(&self) -> &str {
        "live Roe buffers"
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::READ.union(SourceCapabilities::LIST)
    }

    fn supports_revision_kind(&self, kind: &str) -> bool {
        kind == "worktree"
    }

    fn read(&self, request: &ReadRequest) -> ProviderResult<SourceDocument> {
        if request.revision_kind != "worktree" {
            return ProviderResult::Absent;
        }
        if request.root != self.root {
            return ProviderResult::Absent;
        }
        let Some(path) = source_path(&self.root, &request.relative_path) else {
            return ProviderResult::Absent;
        };
        let buffer = self
            .buffers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .by_path
            .get(&path)
            .cloned();
        let Some(buffer) = buffer else {
            return ProviderResult::Absent;
        };
        buffer.with_read(|inner| {
            if inner.buffer.len_bytes() > request.max_bytes {
                return ProviderResult::Failed(SourceFailure::new(format!(
                    "live Roe buffer exceeds the {} byte source bound",
                    request.max_bytes
                )));
            }
            ProviderResult::Found(SourceDocument::from_text(
                inner.content(),
                format!("roe-buffer:{}", inner.text_revision),
            ))
        })
    }

    fn list(&self, request: &ListRequest) -> ProviderResult<Vec<SourceEntry>> {
        if request.limit == 0 {
            return ProviderResult::Found(Vec::new());
        }
        if request.revision_kind != "worktree" || request.root != self.root {
            return ProviderResult::Absent;
        }
        let Some(directory) = source_path(&self.root, &request.relative_path) else {
            return ProviderResult::Absent;
        };
        let buffers = self
            .buffers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut entries = BTreeMap::new();
        for path in buffers.by_path.keys() {
            let Ok(relative) = path.strip_prefix(&directory) else {
                continue;
            };
            let mut components = relative.components();
            let Some(Component::Normal(first)) = components.next() else {
                continue;
            };
            let name = first.to_string_lossy().into_owned();
            let is_directory = components.next().is_some();
            let child = directory.join(first);
            let Ok(child) = child.strip_prefix(&self.root) else {
                continue;
            };
            let relative_path = child
                .components()
                .filter_map(|component| match component {
                    Component::Normal(value) => value.to_str(),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("/");
            entries.entry(relative_path.clone()).or_insert_with(|| {
                SourceEntry::new(
                    relative_path,
                    if is_directory { "directory" } else { "file" },
                    name,
                )
            });
            if entries.len() >= request.limit {
                break;
            }
        }
        if entries.is_empty() {
            ProviderResult::Absent
        } else {
            ProviderResult::Found(entries.into_values().collect())
        }
    }
}

pub(super) fn source_path(root: &Path, relative: &str) -> Option<PathBuf> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    Some(root.join(relative))
}

pub(super) fn source_relative_path(root: &Path, path: &Path) -> Option<String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let absolute = absolute.canonicalize().unwrap_or(absolute);
    let relative = absolute.strip_prefix(root).ok()?;
    let components: Option<Vec<_>> = relative
        .components()
        .map(|component| match component {
            Component::Normal(value) => value.to_str().map(str::to_owned),
            Component::CurDir => Some(String::new()),
            _ => None,
        })
        .collect();
    let path = components?
        .into_iter()
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    (!path.is_empty()).then_some(path)
}

pub(super) fn synchronize_source_buffers(
    root: &Path,
    state: &Arc<RwLock<RoeSourceBuffers>>,
    editor: &Editor,
) {
    let by_path = editor
        .buffers
        .iter()
        .filter(|(buffer, _)| !editor.is_command_buffer(*buffer))
        .filter_map(|(_, buffer)| {
            let relative = source_relative_path(root, &buffer.visited_file()?)?;
            Some((source_path(root, &relative)?, buffer.clone()))
        })
        .collect();
    state
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .by_path = by_path;
}

pub(super) fn source_context_facts(
    repository: Identity,
    revision: Identity,
    root: &Path,
) -> Vec<(Symbol, mica_driver::Tuple)> {
    vec![
        (
            sym("source/Repository"),
            [Value::identity(repository)].into(),
        ),
        (
            sym("source/RepositoryName"),
            [Value::identity(repository), Value::string("workspace")].into(),
        ),
        (
            sym("source/RepositoryRoot"),
            [
                Value::identity(repository),
                Value::string(root.to_string_lossy()),
            ]
            .into(),
        ),
        (sym("source/Revision"), [Value::identity(revision)].into()),
        (
            sym("source/RevisionOf"),
            [Value::identity(revision), Value::identity(repository)].into(),
        ),
        (
            sym("source/RevisionKind"),
            [Value::identity(revision), Value::string("worktree")].into(),
        ),
        (
            sym("source/RevisionLabel"),
            [Value::identity(revision), Value::string("live worktree")].into(),
        ),
    ]
}
