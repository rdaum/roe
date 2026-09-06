// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Atomic, read-only projection of effective Mica policy.
//! This component has no editor, attachment, kernel, or driver access.

use super::{MicaHighlightRule, MicaSyntaxRule};
use crate::BufferId;
use crate::mica_host::MicaPolicyFact;
use crate::syntax::{IndentRule, SyntaxPlan};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::Arc;

#[derive(Default)]
pub(super) struct PolicyProjection {
    pub(super) modes: HashMap<BufferId, String>,
    pub(super) faces: HashMap<String, HashMap<String, String>>,
    pub(super) configuration: HashMap<String, String>,
    pub(super) syntax: HashMap<BufferId, Vec<MicaSyntaxRule>>,
    pub(super) highlights: HashMap<BufferId, Vec<MicaHighlightRule>>,
    pub(super) parsers: HashMap<String, Arc<SyntaxPlan>>,
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
        let mut grammars = HashMap::new();
        let mut queries = HashMap::new();
        let mut indentation: HashMap<String, Vec<IndentRule>> = HashMap::new();
        for fact in &facts {
            match fact {
                MicaPolicyFact::Parser {
                    mode,
                    grammar,
                    query,
                } => {
                    insert_unique(&mut grammars, mode.clone(), grammar)?;
                    insert_unique(&mut queries, mode.clone(), query)?;
                }
                MicaPolicyFact::Indentation {
                    mode,
                    query,
                    anchor,
                    offset,
                    precedence,
                } => {
                    indentation
                        .entry(mode.clone())
                        .or_default()
                        .push(IndentRule {
                            query: query.clone(),
                            anchor: anchor.clone(),
                            offset: *offset,
                            precedence: *precedence,
                        });
                }
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
                MicaPolicyFact::Highlights { buffer, rules } => {
                    for (capture, face, precedence) in rules {
                        next.highlights
                            .entry(*buffer)
                            .or_default()
                            .push(MicaHighlightRule {
                                capture: capture.clone(),
                                face: face.clone(),
                                precedence: *precedence,
                            });
                    }
                }
            }
        }
        for (mode, grammar) in grammars {
            let rules = indentation.remove(&mode).unwrap_or_default();
            // A changed face or buffer association does not recompile unchanged queries.
            let parser_facts = |set: &HashSet<MicaPolicyFact>| {
                set.iter().filter(|fact| matches!(fact,
                MicaPolicyFact::Parser { mode: found, .. } | MicaPolicyFact::Indentation { mode: found, .. } if found == &mode)).cloned().collect::<HashSet<_>>()
            };
            let plan = if parser_facts(&facts) == parser_facts(&self.facts) {
                self.parsers.get(&mode).cloned()
            } else {
                None
            };
            next.parsers.insert(
                mode.clone(),
                match plan {
                    Some(plan) => plan,
                    None => Arc::new(SyntaxPlan::compile(&grammar, &queries[&mode], &rules)?),
                },
            );
        }
        if !indentation.is_empty() {
            return Err("indentation rules require a syntax parser policy".into());
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
