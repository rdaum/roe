// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Decode Mica effects before any editor mechanism receives them.

use super::events::{MAX_BATCH_BYTES, MAX_POLICY_FACTS};
use super::*;
use crate::editor::OpenType;

fn field(value: &Value, name: &str) -> Result<Value, String> {
    map_value(value, name).ok_or_else(|| format!("Mica effect requires {name}"))
}

fn string(value: &Value, name: &str) -> Result<String, String> {
    field(value, name)?
        .with_str(str::to_owned)
        .ok_or_else(|| format!("Mica effect {name} must be a string"))
}

fn symbol(value: &Value, name: &str) -> Result<String, String> {
    field(value, name)?
        .as_symbol()
        .and_then(Symbol::name)
        .map(str::to_owned)
        .ok_or_else(|| format!("Mica effect {name} must be a symbol"))
}

fn position(value: &Value, name: &str) -> Result<usize, String> {
    field(value, name)?
        .as_int()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| format!("Mica effect {name} must be a nonnegative integer"))
}

/// Bound both traversal and retained payload before decoding owned strings.
fn admit_value(value: &Value, remaining: &mut usize, depth: usize) -> Result<(), String> {
    if depth > 32 {
        return Err("Mica effect exceeds the nesting limit".into());
    }
    let bytes = value
        .with_str(str::len)
        .or_else(|| value.with_bytes(<[u8]>::len))
        .unwrap_or(size_of::<Value>());
    *remaining = remaining
        .checked_sub(bytes)
        .ok_or("Mica effect exceeds the byte limit")?;
    if let Some(result) = value.with_map(|entries| {
        for (key, value) in entries {
            admit_value(key, remaining, depth + 1)?;
            admit_value(value, remaining, depth + 1)?;
        }
        Ok::<_, String>(())
    }) {
        result?;
    }
    if let Some(len) = value.list_len() {
        for i in 0..len {
            admit_value(
                &value.list_get(i).ok_or("invalid effect list")?,
                remaining,
                depth + 1,
            )?;
        }
    }
    Ok(())
}

impl MicaHost {
    pub(super) fn decode_effect(
        &mut self,
        target: Identity,
        value: &Value,
    ) -> Result<MicaEvent, String> {
        if target != self.session {
            return Err("Mica effect targeted another session".into());
        }
        let mut remaining = MAX_BATCH_BYTES;
        admit_value(value, &mut remaining, 0)?;
        let kind = symbol(value, "kind")?;
        match kind.as_str() {
            "host_action" => self.decode_host_action(value).map(MicaEvent::Host),
            "native_action" => {
                let name = symbol(value, "action")?;
                let text = map_value(value, "text")
                    .map(|_| string(value, "text"))
                    .transpose()?;
                if text.as_ref().is_some_and(|text| {
                    text.chars().count() > crate::session::MAX_TEXT_CHARS_PER_INPUT
                }) {
                    return Err("Mica native text exceeds the character limit".into());
                }
                MicaNativeAction::decode(&name, text).map(MicaEvent::Native)
            }
            "policy_snapshot" => {
                let facts = field(value, "facts")?;
                let len = facts.list_len().ok_or("Mica policy facts must be a list")?;
                if len > MAX_POLICY_FACTS {
                    return Err(format!(
                        "Mica policy fact limit of {MAX_POLICY_FACTS} exceeded"
                    ));
                }
                let mut decoded = Vec::with_capacity(len);
                for i in 0..len {
                    decoded.push(
                        self.decode_policy_fact(&facts.list_get(i).ok_or("invalid policy fact")?)?,
                    );
                }
                Ok(MicaEvent::Policy(decoded))
            }
            "presentation_invalidated" => self
                .presentation_effect(target, value)
                .map(MicaEvent::Presentation)
                .ok_or_else(|| "malformed Mica presentation effect".into()),
            "prompt_update" => {
                let prompt = self
                    .prompt_update(target, value)
                    .ok_or("malformed Mica prompt effect")?;
                Ok(MicaEvent::Prompt(prompt))
            }
            "prompt_close" => Ok(MicaEvent::PromptClosed),
            "search_update" => self
                .search_update(target, value)
                .map(MicaEvent::Search)
                .ok_or_else(|| "malformed Mica search effect".into()),
            "search_finish" => self
                .search_finish(target, value)
                .map(MicaEvent::SearchFinished)
                .ok_or_else(|| "malformed Mica search-finish effect".into()),
            _ => Err(format!("unknown Mica effect kind: {kind}")),
        }
    }

    fn decode_buffer(&self, value: &Value, name: &str) -> Result<BufferId, String> {
        let logical = field(value, name)?
            .as_identity()
            .ok_or("Mica buffer must be an identity")?;
        self.buffer_ids
            .iter()
            .find_map(|(buffer, id)| (*id == logical).then_some(*buffer))
            .ok_or_else(|| format!("Mica effect {name} refers to a stale buffer"))
    }

    fn decode_view(&self, value: &Value) -> Result<WindowId, String> {
        let logical = field(value, "view")?
            .as_identity()
            .ok_or("Mica view must be an identity")?;
        self.view_ids
            .iter()
            .find_map(|(view, id)| (*id == logical).then_some(*view))
            .ok_or_else(|| "Mica effect refers to a stale view".into())
    }

    fn decode_host_action(&self, value: &Value) -> Result<MicaHostAction, String> {
        use MicaHostAction::*;
        let name = symbol(value, "action")?;
        Ok(match name.as_str() {
            "indent_line" => {
                let logical = field(value, "buffer")?
                    .as_identity()
                    .ok_or("indent buffer must be an identity")?;
                let state = self.bridge.state.lock().unwrap();
                if state.actor != Some(self.actor)
                    || !state.resources.contains_key(&logical)
                    || !state.services.contains(&sym("text_read"))
                    || !state.services.contains(&sym("text_write"))
                {
                    return Err(
                        "indentation is not authorized for this endpoint, buffer, or service"
                            .into(),
                    );
                }
                Indent {
                    view: self.decode_view(value)?,
                    buffer: self.decode_buffer(value, "buffer")?,
                    revision: position(value, "revision")? as u64,
                    newline: field(value, "newline")?
                        .as_bool()
                        .ok_or("indent newline must be a boolean")?,
                    width: position(value, "width")?,
                    tab_width: position(value, "tab_width")?,
                }
            }
            "agent_render" => match symbol(value, "phase")?.as_str() {
                "open" => AgentOpen {
                    view: self.decode_view(value)?,
                    name: string(value, "buffer_name")?,
                    text: string(value, "text")?,
                },
                "update" => AgentUpdate {
                    buffer: self.decode_buffer(value, "buffer")?,
                    text: string(value, "text")?,
                },
                _ => return Err("unknown Mica agent-render phase".into()),
            },
            "start_agent" => StartAgent,
            "show_typeout" => ShowTypeout {
                view: self.decode_view(value)?,
                buffer: self.decode_buffer(value, "buffer")?,
                kind: symbol(value, "typeout_kind")?,
                title: string(value, "title")?,
                text: string(value, "text")?,
            },
            "dismiss_typeout" => DismissTypeout,
            "typeout_page_forward" => PageTypeout { forward: true },
            "typeout_page_backward" => PageTypeout { forward: false },
            "echo" => Echo(string(value, "text")?),
            "quit" => Quit,
            "redraw" => Redraw,
            "split_horizontal" | "split_vertical" => Split {
                view: self.decode_view(value)?,
                direction: if name == "split_horizontal" {
                    SplitDirection::Horizontal
                } else {
                    SplitDirection::Vertical
                },
            },
            "other_window" => SelectView {
                view: self.decode_view(value)?,
            },
            "delete_window" => DeleteView {
                view: self.decode_view(value)?,
            },
            "delete_other_windows" => CollapseToView {
                view: self.decode_view(value)?,
            },
            "begin_layout_drag" => BeginLayoutDrag {
                view: self.decode_view(value)?,
            },
            "pointer_selection" => match symbol(value, "phase")?.as_str() {
                "down" => PointerDown {
                    view: self.decode_view(value)?,
                    position: position(value, "position")?,
                    anchor: position(value, "anchor")?,
                },
                "move" => PointerMove {
                    view: self.decode_view(value)?,
                    position: position(value, "position")?,
                    anchor: position(value, "anchor")?,
                },
                "up" => PointerUp,
                _ => return Err("unknown Mica pointer phase".into()),
            },
            "set_view_scroll" => Scroll {
                view: self.decode_view(value)?,
                line: position(value, "line")?,
                column: position(value, "column")?,
            },
            "set_split_ratio" => {
                let node = field(value, "node")?
                    .as_identity()
                    .ok_or("Mica split node must be an identity")?;
                let path = self
                    .layout_nodes
                    .iter()
                    .find_map(|(path, id)| (*id == node).then_some(path.clone()))
                    .ok_or("Mica split node is stale")?;
                let ratio = field(value, "ratio")?
                    .as_float()
                    .ok_or("Mica split ratio must be a float")?;
                if !ratio.is_finite() || !(0.0..=1.0).contains(&ratio) {
                    return Err("invalid Mica split ratio".into());
                }
                SplitRatio { path, ratio }
            }
            "invalidate_syntax" => InvalidateSyntax {
                view: self.decode_view(value)?,
            },
            "save_buffer" => Save {
                buffer: self.decode_buffer(value, "buffer")?,
            },
            "save_buffer_as_selected" => SaveAs {
                buffer: self.decode_buffer(value, "buffer")?,
                path: string(value, "path")?,
            },
            "create_buffer" => CreateBuffer {
                view: self.decode_view(value)?,
                name: string(value, "buffer_name")?,
            },
            "eval_region" => EvalRegion {
                buffer: self.decode_buffer(value, "buffer")?,
                view: self.decode_view(value)?,
            },
            "eval_buffer" => EvalBuffer {
                buffer: self.decode_buffer(value, "buffer")?,
                unit: symbol(value, "unit")?,
            },
            "switch_buffer_selected" => SelectBuffer {
                buffer: self.decode_buffer(value, "buffer")?,
            },
            "kill_buffer_selected" => KillBuffer {
                buffer: self.decode_buffer(value, "buffer")?,
                replacement: self.decode_buffer(value, "replacement")?,
            },
            "find_file_selected" | "visit_file_selected" => OpenFile {
                path: string(value, "path")?,
                kind: if name == "find_file_selected" {
                    OpenType::New
                } else {
                    OpenType::Visit
                },
            },
            _ => return Err(format!("unknown Mica host action: {name}")),
        })
    }

    fn decode_policy_fact(&self, value: &Value) -> Result<MicaPolicyFact, String> {
        use MicaPolicyFact::*;
        let kind = symbol(value, "kind")?;
        let precedence = || {
            field(value, "precedence")?
                .as_int()
                .ok_or_else(|| "Mica precedence must be an integer".to_owned())
        };
        Ok(match kind.as_str() {
            "parser_policy" => Parser {
                mode: string(value, "mode")?,
                grammar: symbol(value, "grammar")?,
                query: string(value, "query")?,
            },
            "injection_policy" => Injection {
                mode: string(value, "mode")?,
                grammar: symbol(value, "grammar")?,
                query: string(value, "query")?,
                highlights: string(value, "highlights")?,
            },
            "indentation_policy" => Indentation {
                mode: string(value, "mode")?,
                query: string(value, "query")?,
                anchor: symbol(value, "anchor")?,
                offset: field(value, "offset")?
                    .as_int()
                    .ok_or("indentation offset must be an integer")?,
                precedence: precedence()?,
            },
            "mode_policy" => Mode {
                buffer: self.decode_buffer(value, "buffer")?,
                name: string(value, "name")?,
            },
            "face_policy" => Face {
                name: string(value, "face")?,
                attribute: symbol(value, "attribute")?,
                value: self.policy_value(value)?,
            },
            "configuration_policy" => Configuration {
                key: symbol(value, "key")?,
                value: self.policy_value(value)?,
            },
            "syntax_policy" => Syntax {
                buffer: self.decode_buffer(value, "buffer")?,
                kind: symbol(value, "syntax_kind")?,
                pattern: string(value, "pattern")?,
                precedence: precedence()?,
            },
            "highlight_policy" => {
                let list = field(value, "rules")?;
                let count = list.list_len().ok_or("highlight rules must be a list")?;
                if count > MAX_POLICY_FACTS {
                    return Err("highlight rules exceed the 256-rule limit".into());
                }
                let mut rules = Vec::with_capacity(count);
                for index in 0..count {
                    let rule = list.list_get(index).ok_or("missing highlight rule")?;
                    rules.push((
                        symbol(&rule, "capture")?,
                        string(&rule, "face")?,
                        field(&rule, "precedence")?
                            .as_int()
                            .ok_or("highlight precedence must be an integer")?,
                    ));
                }
                rules.sort_unstable();
                rules.dedup();
                Highlights {
                    buffer: self.decode_buffer(value, "buffer")?,
                    rules,
                }
            }
            _ => return Err(format!("unknown Mica policy fact: {kind}")),
        })
    }

    fn policy_value(&self, value: &Value) -> Result<String, String> {
        let raw = field(value, "value")?;
        Ok(raw
            .with_str(str::to_owned)
            .unwrap_or_else(|| self.format_value(&raw)))
    }
    fn presentation_effect(
        &mut self,
        target: Identity,
        value: &Value,
    ) -> Option<MicaPresentationEffect> {
        if target != self.session {
            return None;
        }
        if map_value(value, "kind")?.as_symbol()? != sym("presentation_invalidated") {
            return None;
        }
        let logical_buffer = map_value(value, "buffer")?.as_identity()?;
        let logical_view = map_value(value, "view")?.as_identity()?;
        let cursor = usize::try_from(map_value(value, "cursor")?.as_int()?).ok()?;
        let buffer = self
            .buffer_ids
            .iter()
            .find_map(|(buffer, identity)| (*identity == logical_buffer).then_some(*buffer))?;
        let view = self
            .view_ids
            .iter()
            .find_map(|(view, identity)| (*identity == logical_view).then_some(*view))?;
        self.view_cursors.insert(view, cursor);
        Some(MicaPresentationEffect {
            buffer,
            view,
            cursor,
        })
    }

    fn prompt_update(&self, target: Identity, value: &Value) -> Option<MicaPromptUpdate> {
        if target != self.session {
            return None;
        }
        let prefix = string(value, "prefix").ok()?;
        let query = string(value, "query").ok()?;
        let selected = position(value, "selected").ok()?;
        let values = field(value, "candidates").ok()?;
        let len = values.list_len()?;
        if len > MAX_PROMPT_CANDIDATES {
            return None;
        }
        let mut candidates = Vec::with_capacity(len);
        for index in 0..len {
            let row = values.list_get(index)?;
            candidates.push(row.list_get(0)?.with_str(str::to_owned)?);
        }
        // Search prompts have a selection index but no completion candidates.
        if len > 0 && selected >= len {
            return None;
        }
        Some(MicaPromptUpdate {
            prefix,
            query,
            selected,
            candidates,
        })
    }

    fn search_update(&self, target: Identity, value: &Value) -> Option<MicaSearchUpdate> {
        if target != self.session || map_value(value, "kind")?.as_symbol()? != sym("search_update")
        {
            return None;
        }
        let logical = map_value(value, "view")?.as_identity()?;
        let view = self
            .view_ids
            .iter()
            .find_map(|(view, identity)| (*identity == logical).then_some(*view))?;
        let raw = map_value(value, "matches")?;
        let mut matches = Vec::new();
        let len = raw.list_len()?;
        if len > MAX_SEARCH_MATCHES {
            return None;
        }
        for index in 0..len {
            let row = raw.list_get(index)?;
            let start = usize::try_from(row.list_get(0)?.as_int()?).ok()?;
            let end = usize::try_from(row.list_get(1)?.as_int()?).ok()?;
            if end < start {
                return None;
            }
            matches.push((start, end));
        }
        let selected = map_value(value, "selected")
            .and_then(|value| value.as_int())
            .and_then(|value| usize::try_from(value).ok());
        Some(MicaSearchUpdate {
            view,
            matches,
            selected,
        })
    }

    fn search_finish(&self, target: Identity, value: &Value) -> Option<MicaSearchFinish> {
        if target != self.session || map_value(value, "kind")?.as_symbol()? != sym("search_finish")
        {
            return None;
        }
        let logical = map_value(value, "view")?.as_identity()?;
        let view = self
            .view_ids
            .iter()
            .find_map(|(view, identity)| (*identity == logical).then_some(*view))?;
        Some(MicaSearchFinish {
            view,
            original_cursor: usize::try_from(map_value(value, "original")?.as_int()?).ok()?,
            query: map_value(value, "query")?.with_str(str::to_owned)?,
            accepted: map_value(value, "accepted")?.as_bool()?,
        })
    }
}
