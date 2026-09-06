// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Atomic, read-only projection of effective Mica policy.
//! This component has no editor, attachment, kernel, or driver access.

use super::{MicaHighlightRule, MicaSyntaxRule};
use crate::BufferId;
use crate::mica_host::MicaPolicyFact;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

#[derive(Default)]
pub(super) struct PolicyProjection {
    pub(super) modes: HashMap<BufferId, String>,
    pub(super) faces: HashMap<String, HashMap<String, String>>,
    pub(super) configuration: HashMap<String, String>,
    pub(super) syntax: HashMap<BufferId, Vec<MicaSyntaxRule>>,
    pub(super) highlights: HashMap<String, Vec<MicaHighlightRule>>,
    pub(super) revision: u64,
    facts: HashSet<MicaPolicyFact>,
}

fn insert_unique<K: Eq + Hash>(
    map: &mut HashMap<K, String>,
    key: K,
    value: &str,
) -> Result<(), String> {
    if let Some(previous) = map.insert(key, value.to_owned())
        && previous != value
    {
        return Err("Mica policy snapshot contains conflicting functional facts".into());
    }
    Ok(())
}

impl PolicyProjection {
    /// Build and validate the complete replacement before publishing any field.
    pub(super) fn replace(&mut self, facts: Vec<MicaPolicyFact>) -> Result<bool, String> {
        let facts: HashSet<_> = facts.into_iter().collect();
        if facts == self.facts {
            return Ok(false);
        }
        let mut next = Self::default();
        for fact in &facts {
            match fact {
                MicaPolicyFact::Mode { buffer, name } => {
                    insert_unique(&mut next.modes, *buffer, name)?
                }
                MicaPolicyFact::Face {
                    name,
                    attribute,
                    value,
                } => insert_unique(
                    next.faces.entry(name.clone()).or_default(),
                    attribute.clone(),
                    value,
                )?,
                MicaPolicyFact::Configuration { key, value } => {
                    insert_unique(&mut next.configuration, key.clone(), value)?
                }
                MicaPolicyFact::Syntax {
                    buffer,
                    kind,
                    pattern,
                    precedence,
                } => {
                    next.syntax
                        .entry(*buffer)
                        .or_default()
                        .push(MicaSyntaxRule {
                            kind: kind.clone(),
                            pattern: pattern.clone(),
                            precedence: *precedence,
                        });
                }
                MicaPolicyFact::Highlight {
                    mode,
                    capture,
                    face,
                    precedence,
                } => {
                    next.highlights
                        .entry(mode.clone())
                        .or_default()
                        .push(MicaHighlightRule {
                            capture: capture.clone(),
                            face: face.clone(),
                            precedence: *precedence,
                        });
                }
            }
        }
        next.facts = facts;
        next.revision = self
            .revision
            .checked_add(1)
            .ok_or("Mica policy revision exhausted")?;
        *self = next;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(value: &str) -> MicaPolicyFact {
        MicaPolicyFact::Configuration {
            key: "tab-width".into(),
            value: value.into(),
        }
    }

    #[test]
    fn equal_publications_ignore_order_and_duplicates() {
        let mut projection = PolicyProjection::default();
        let face = MicaPolicyFact::Face {
            name: "default".into(),
            attribute: "foreground".into(),
            value: "white".into(),
        };
        assert!(
            projection
                .replace(vec![setting("4"), face.clone()])
                .unwrap()
        );
        assert_eq!(projection.revision, 1);
        assert!(
            !projection
                .replace(vec![face, setting("4"), setting("4")])
                .unwrap()
        );
        assert_eq!(projection.revision, 1);
        assert!(projection.replace(vec![]).unwrap());
        assert!(projection.configuration.is_empty());
        assert!(projection.faces.is_empty());
        assert_eq!(projection.revision, 2);
    }

    #[test]
    fn conflicting_publication_preserves_every_previous_field() {
        let mut projection = PolicyProjection::default();
        projection.replace(vec![setting("4")]).unwrap();
        let old_facts = projection.facts.clone();
        assert!(
            projection
                .replace(vec![setting("2"), setting("8")])
                .is_err()
        );
        assert_eq!(projection.revision, 1);
        assert_eq!(projection.configuration["tab-width"], "4");
        assert_eq!(projection.facts, old_facts);
    }
}
