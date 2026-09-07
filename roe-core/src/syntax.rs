// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Bounded syntax mechanisms. Languages, queries, faces, and indentation rules
//! are selected by Mica. No file extensions or editor commands live here.

mod injection;
use injection::inject_highlights;

use crate::BufferId;
use crate::syntax_highlighting::{HighlightSpan, MAX_HIGHLIGHT_SOURCE_BYTES, MAX_HIGHLIGHT_SPANS};
use ropey::Rope;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::ControlFlow;
use std::time::{Duration, Instant};
use tree_sitter::{
    InputEdit, Language, Node, ParseOptions, Parser, Point, Query, QueryCursor, QueryCursorOptions,
    StreamingIterator, Tree,
};

const MAX_CACHED_BUFFERS: usize = 16;
const MAX_QUERY_BYTES: usize = 65_536;
const MAX_CAPTURE_NAME_BYTES: usize = 128;
const MAX_INDENT_RULES: usize = 64;
const MAX_INDENT_COLUMNS: usize = 256;
const WORK_TIME: Duration = Duration::from_millis(50);
const MAX_INJECTION_REGIONS: usize = 1024;
const MAX_INJECTION_RANGES: usize = 16_384;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct IndentRule {
    pub query: String,
    pub anchor: String,
    pub offset: i64,
    pub precedence: i64,
}

struct CompiledIndentRule {
    query: Query,
    anchor: Anchor,
    offset: i64,
    precedence: i64,
}

#[derive(Clone, Copy)]
enum Anchor {
    Line,
    Column,
    FirstChild,
    Preserve,
    Zero,
}

pub(crate) struct SyntaxPlan {
    grammar: String,
    highlights: Option<Query>,
    indentation: Vec<CompiledIndentRule>,
    injection: Option<InjectionPlan>,
}

struct InjectionPlan {
    language: Language,
    regions: Query,
    highlights: Query,
}

fn language(name: &str) -> Result<Language, String> {
    match name {
        "rust" => Ok(tree_sitter_rust::LANGUAGE.into()),
        "markdown" => Ok(tree_sitter_md::LANGUAGE.into()),
        "markdown_inline" => Ok(tree_sitter_md::INLINE_LANGUAGE.into()),
        _ => Err(format!("syntax grammar is unavailable: {name}")),
    }
}

fn compile_query(language: &Language, source: &str) -> Result<Query, String> {
    if source.len() > MAX_QUERY_BYTES {
        return Err("syntax query exceeds the 65536-byte limit".into());
    }
    let query =
        Query::new(language, source).map_err(|error| format!("invalid syntax query: {error}"))?;
    if query.pattern_count() > 256 || query.capture_names().len() > 256 {
        return Err("syntax query exceeds the 256-pattern/capture limit".into());
    }
    if query
        .capture_names()
        .iter()
        .any(|name| name.len() > MAX_CAPTURE_NAME_BYTES)
    {
        return Err("syntax capture names exceed the 128-byte limit".into());
    }
    for pattern in 0..query.pattern_count() {
        if !query.general_predicates(pattern).is_empty()
            || !query.property_predicates(pattern).is_empty()
            || !query.property_settings(pattern).is_empty()
        {
            return Err("unsupported syntax query predicate or property".into());
        }
    }
    Ok(query)
}

impl SyntaxPlan {
    pub fn compile(grammar: &str, highlights: &str, rules: &[IndentRule]) -> Result<Self, String> {
        if grammar == "mica" {
            if !highlights.is_empty() || !rules.is_empty() {
                return Err("the Mica lexical provider does not accept tree queries".into());
            }
            return Ok(Self {
                grammar: grammar.into(),
                highlights: None,
                indentation: Vec::new(),
                injection: None,
            });
        }
        if rules.len() > MAX_INDENT_RULES
            || rules.iter().map(|rule| rule.query.len()).sum::<usize>() > MAX_QUERY_BYTES
        {
            return Err("indentation policy exceeds its rule or query-byte limit".into());
        }
        let language = language(grammar)?;
        let mut indentation = Vec::new();
        for rule in rules {
            let anchor = match rule.anchor.as_str() {
                "line" => Anchor::Line,
                "column" => Anchor::Column,
                "first_child" => Anchor::FirstChild,
                "preserve" => Anchor::Preserve,
                "zero" => Anchor::Zero,
                _ => return Err(format!("unsupported indentation anchor: {}", rule.anchor)),
            };
            if !(-16..=16).contains(&rule.offset) {
                return Err("indentation offset must be between -16 and 16 units".into());
            }
            let query = compile_query(&language, &rule.query)?;
            if !query.capture_names().contains(&"scope")
                || query
                    .capture_names()
                    .iter()
                    .any(|name| !["scope", "anchor", "target"].contains(name))
            {
                return Err("indentation queries require @scope and accept only @scope, @anchor, and @target".into());
            }
            indentation.push(CompiledIndentRule {
                query,
                anchor,
                offset: rule.offset,
                precedence: rule.precedence,
            });
        }
        Ok(Self {
            grammar: grammar.into(),
            highlights: Some(compile_query(&language, highlights)?),
            indentation,
            injection: None,
        })
    }

    /// One nonrecursive layer. Mica selects both the regions and their grammar.
    pub fn with_injection(
        mut self,
        grammar: &str,
        regions: &str,
        highlights: &str,
    ) -> Result<Self, String> {
        let regions = compile_query(&language(&self.grammar)?, regions)?;
        if regions.capture_names() != ["content"] {
            return Err("syntax injection queries require only @content".into());
        }
        let language = language(grammar)?;
        let highlights = compile_query(&language, highlights)?;
        self.injection = Some(InjectionPlan {
            language,
            regions,
            highlights,
        });
        Ok(self)
    }
}

struct ParsedBuffer {
    revision: u64,
    grammar: String,
    rope: Rope,
    parser: Parser,
    tree: Tree,
    used: u64,
}

#[derive(Default)]
pub(crate) struct SyntaxService {
    buffers: HashMap<BufferId, ParsedBuffer>,
    clock: u64,
}

impl SyntaxService {
    pub fn retain_live(&mut self, live: &HashSet<BufferId>) {
        self.buffers.retain(|buffer, _| live.contains(buffer));
    }

    fn observe(
        &mut self,
        buffer: BufferId,
        revision: u64,
        rope: &Rope,
        plan: &SyntaxPlan,
    ) -> Result<&mut ParsedBuffer, String> {
        if rope.len_bytes() > MAX_HIGHLIGHT_SOURCE_BYTES {
            self.buffers.remove(&buffer);
            return Err("syntax source exceeds the 1 MiB limit".into());
        }
        self.clock = self.clock.saturating_add(1);
        if self
            .buffers
            .get(&buffer)
            .is_some_and(|cached| cached.grammar != plan.grammar)
        {
            self.buffers.remove(&buffer);
        }
        if !self.buffers.contains_key(&buffer) {
            if self.buffers.len() == MAX_CACHED_BUFFERS {
                let oldest = *self
                    .buffers
                    .iter()
                    .min_by_key(|(_, cached)| cached.used)
                    .unwrap()
                    .0;
                self.buffers.remove(&oldest);
            }
            let mut parser = Parser::new();
            parser
                .set_language(&language(&plan.grammar)?)
                .map_err(|error| error.to_string())?;
            let tree = parse(&mut parser, rope, None)?;
            self.buffers.insert(
                buffer,
                ParsedBuffer {
                    revision,
                    grammar: plan.grammar.clone(),
                    rope: rope.clone(),
                    parser,
                    tree,
                    used: self.clock,
                },
            );
        }
        let cached = self.buffers.get_mut(&buffer).unwrap();
        cached.used = self.clock;
        if cached.revision != revision {
            let mut old = cached.tree.clone();
            edit_tree(&mut old, &cached.rope, rope);
            let tree = parse(&mut cached.parser, rope, Some(&old))?;
            cached.tree = tree;
            cached.rope = rope.clone();
            cached.revision = revision;
        }
        Ok(cached)
    }

    pub fn highlights(
        &mut self,
        buffer: BufferId,
        revision: u64,
        rope: &Rope,
        plan: &SyntaxPlan,
    ) -> Result<Vec<HighlightSpan>, String> {
        let Some(query) = &plan.highlights else {
            self.buffers.remove(&buffer);
            if rope.len_bytes() > MAX_HIGHLIGHT_SOURCE_BYTES {
                return Err("syntax source exceeds the 1 MiB limit".into());
            }
            return Ok(crate::syntax_highlighting::mica_highlights(
                &rope.to_string(),
            ));
        };
        let cached = self.observe(buffer, revision, rope, plan)?;
        let source = rope.to_string();
        let mut spans = Vec::new();
        visit_matches(query, &cached.tree, &source, None, |matched| {
            for capture in matched.captures {
                if spans.len() == MAX_HIGHLIGHT_SPANS {
                    return Err("syntax highlights exceed the capture limit".into());
                }
                spans.push((
                    capture.node.start_byte(),
                    capture.node.end_byte(),
                    matched.pattern_index,
                    query.capture_names()[capture.index as usize],
                ));
            }
            Ok(())
        })?;
        if let Some(injection) = &plan.injection {
            inject_highlights(
                injection,
                &cached.tree,
                &source,
                query.pattern_count(),
                &mut spans,
            )?;
        }
        // Query captures can nest and overlap. Normalize in byte coordinates
        // before one ordered Unicode conversion; later query patterns win.
        let mut events = Vec::with_capacity(spans.len() * 2);
        for (index, &(start, end, _, _)) in spans.iter().enumerate() {
            if start < end {
                events.push((start, true, index));
                events.push((end, false, index));
            }
        }
        events.sort_unstable();
        let mut active: BTreeSet<(usize, std::cmp::Reverse<usize>, usize)> = BTreeSet::new();
        let mut previous = 0;
        let mut byte_cursor = 0;
        let mut char_cursor = 0;
        let mut result: Vec<HighlightSpan> = Vec::new();
        for (position, opening, index) in events {
            if position > previous
                && let Some(&(_, _, winner)) = active.last()
            {
                let (_, _, _, name) = spans[winner];
                char_cursor += source[byte_cursor..previous].chars().count();
                let start = char_cursor;
                char_cursor += source[previous..position].chars().count();
                byte_cursor = position;
                if let Some(last) = result.last_mut()
                    && last.end == start
                    && last.capture == name
                {
                    last.end = char_cursor;
                } else {
                    if result.len() == MAX_HIGHLIGHT_SPANS {
                        return Err("normalized syntax spans exceed the limit".into());
                    }
                    result.push(HighlightSpan {
                        start,
                        end: char_cursor,
                        capture: name.into(),
                    });
                }
            }
            let (start, end, pattern, _) = spans[index];
            let priority = (pattern, std::cmp::Reverse(end - start), index);
            if opening {
                active.insert(priority);
            } else {
                active.remove(&priority);
            }
            previous = position;
        }
        Ok(result)
    }

    pub fn indent_edit(
        &mut self,
        buffer: BufferId,
        revision: u64,
        rope: &Rope,
        plan: &SyntaxPlan,
        request: IndentRequest,
    ) -> Result<TextEdit, String> {
        let IndentRequest {
            cursor,
            newline,
            width,
            tab_width,
        } = request;
        if cursor > rope.len_chars() || !(1..=16).contains(&width) || !(1..=16).contains(&tab_width)
        {
            return Err("invalid indentation position or width (expected 1..16)".into());
        }
        let cached = self.observe(buffer, revision, rope, plan)?;
        let mut virtual_rope = rope.clone();
        let position = if newline {
            virtual_rope.insert(cursor, "\n");
            cursor + 1
        } else {
            cursor
        };
        if virtual_rope.len_bytes() > MAX_HIGHLIGHT_SOURCE_BYTES {
            return Err("syntax source exceeds the 1 MiB limit".into());
        }
        let mut tree = cached.tree.clone();
        if newline {
            edit_tree(&mut tree, rope, &virtual_rope);
            tree = parse(&mut cached.parser, &virtual_rope, Some(&tree))?;
        }
        let line = virtual_rope.char_to_line(position);
        let start = virtual_rope.line_to_char(line);
        let whitespace = virtual_rope
            .line(line)
            .chars()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .count();
        let source = virtual_rope.to_string();
        let byte = virtual_rope.char_to_byte(start + whitespace);
        let columns = indentation(plan, &tree, &virtual_rope, &source, byte, width, tab_width)?;
        let text = match columns {
            Some(Some(columns)) => " ".repeat(columns),
            None if newline => {
                // Preserve the preceding line's indentation when syntax is incomplete.
                rope.line(rope.char_to_line(cursor))
                    .chars()
                    .take_while(|c| matches!(c, ' ' | '\t'))
                    .collect()
            }
            _ => virtual_rope.slice(start..start + whitespace).to_string(),
        };
        if text.len() > MAX_INDENT_COLUMNS {
            return Err("indentation exceeds the 256-column limit".into());
        }
        if newline {
            let text = format!("\n{text}");
            Ok(TextEdit {
                start: cursor,
                end: cursor + whitespace,
                cursor: cursor + text.chars().count(),
                text,
            })
        } else {
            let next_cursor = if cursor <= start + whitespace {
                start + text.len()
            } else {
                cursor - whitespace + text.len()
            };
            Ok(TextEdit {
                start,
                end: start + whitespace,
                text,
                cursor: next_cursor,
            })
        }
    }
}

pub(crate) struct IndentRequest {
    pub cursor: usize,
    pub newline: bool,
    pub width: usize,
    pub tab_width: usize,
}

#[cfg(test)]
mod markdown_tests;

#[derive(Debug)]
pub(crate) struct TextEdit {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub cursor: usize,
}

fn parse(parser: &mut Parser, rope: &Rope, old: Option<&Tree>) -> Result<Tree, String> {
    let deadline = Instant::now() + WORK_TIME;
    let mut progress = |_: &tree_sitter::ParseState| {
        if Instant::now() >= deadline {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    parser
        .parse_with_options(
            &mut |byte, _| {
                if byte >= rope.len_bytes() {
                    return &[][..];
                }
                let (chunk, start, _, _) = rope.chunk_at_byte(byte);
                &chunk.as_bytes()[byte - start..]
            },
            old,
            Some(ParseOptions::new().progress_callback(&mut progress)),
        )
        .ok_or_else(|| {
            parser.reset();
            "syntax parsing exceeded its time budget".into()
        })
}

fn point(rope: &Rope, byte: usize) -> Point {
    // Tree-sitter counts LF bytes. Rope also recognizes other line separators.
    let mut point = Point::new(0, 0);
    for byte in rope.bytes().take(byte) {
        if byte == b'\n' {
            point.row += 1;
            point.column = 0;
        } else {
            point.column += 1;
        }
    }
    point
}

/// Derive one exact edit from coherent Rope snapshots. This also covers undo,
/// redo, external reloads, and native requests without an unbounded edit journal.
fn edit_tree(tree: &mut Tree, old: &Rope, new: &Rope) {
    let prefix = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = old
        .slice(prefix..)
        .chars()
        .reversed()
        .zip(new.slice(prefix..).chars().reversed())
        .take_while(|(a, b)| a == b)
        .count();
    let start = old.char_to_byte(prefix);
    let old_end = old.char_to_byte(old.len_chars() - suffix);
    let new_end = new.char_to_byte(new.len_chars() - suffix);
    tree.edit(&InputEdit {
        start_byte: start,
        old_end_byte: old_end,
        new_end_byte: new_end,
        start_position: point(old, start),
        old_end_position: point(old, old_end),
        new_end_position: point(new, new_end),
    });
}

fn visit_matches(
    query: &Query,
    tree: &Tree,
    source: &str,
    range: Option<std::ops::Range<usize>>,
    visit: impl FnMut(&tree_sitter::QueryMatch<'_, '_>) -> Result<(), String>,
) -> Result<(), String> {
    visit_matches_until(
        query,
        tree,
        source,
        range,
        Instant::now() + WORK_TIME,
        visit,
    )
}

fn visit_matches_until(
    query: &Query,
    tree: &Tree,
    source: &str,
    range: Option<std::ops::Range<usize>>,
    deadline: Instant,
    mut visit: impl FnMut(&tree_sitter::QueryMatch<'_, '_>) -> Result<(), String>,
) -> Result<(), String> {
    let mut cursor = QueryCursor::new();
    cursor.set_match_limit(256);
    if let Some(range) = range {
        cursor.set_byte_range(range);
    }
    let mut cancelled = false;
    let mut progress = |_: &tree_sitter::QueryCursorState| {
        cancelled = Instant::now() >= deadline;
        if cancelled {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    {
        let mut matches = cursor.matches_with_options(
            query,
            tree.root_node(),
            source.as_bytes(),
            QueryCursorOptions::new().progress_callback(&mut progress),
        );
        let mut count = 0;
        while let Some(matched) = matches.next() {
            count += 1;
            if count > MAX_HIGHLIGHT_SPANS || Instant::now() >= deadline {
                return Err("syntax query exceeded its work budget".into());
            }
            visit(matched)?;
        }
    }
    if cancelled || cursor.did_exceed_match_limit() {
        return Err("syntax query exceeded its work budget".into());
    }
    Ok(())
}

fn column(
    source: &str,
    rope: &Rope,
    byte: usize,
    tab_width: usize,
    indentation_only: bool,
) -> usize {
    let start = rope.line_to_byte(rope.byte_to_line(byte));
    let mut column = 0;
    for ch in source[start..byte].chars() {
        if indentation_only && ch != ' ' && ch != '\t' {
            break;
        }
        column += if ch == '\t' {
            tab_width - column % tab_width
        } else {
            1
        };
    }
    column
}

fn indentation(
    plan: &SyntaxPlan,
    tree: &Tree,
    rope: &Rope,
    source: &str,
    byte: usize,
    width: usize,
    tab_width: usize,
) -> Result<Option<Option<usize>>, String> {
    let line = rope.byte_to_line(byte);
    let mut selected: Option<(i64, usize, Option<usize>)> = None;
    let mut ambiguous = false;
    let deadline = Instant::now() + WORK_TIME;
    for rule in &plan.indentation {
        if Instant::now() >= deadline {
            return Err("indentation exceeded its time budget".into());
        }
        visit_matches(&rule.query, tree, source, None, |matched| {
            let capture = |name| -> Option<Node<'_>> {
                matched
                    .captures
                    .iter()
                    .find(|capture| rule.query.capture_names()[capture.index as usize] == name)
                    .map(|capture| capture.node)
            };
            let Some(scope) = capture("scope") else {
                return Ok(());
            };
            let extends_error = scope.has_error()
                && scope.end_byte() <= byte
                && source[scope.end_byte()..byte].trim().is_empty();
            if byte < scope.start_byte() || (byte >= scope.end_byte() && !extends_error) {
                return Ok(());
            }
            if let Some(target) = capture("target")
                && target.start_byte() != byte
            {
                return Ok(());
            }
            let anchor = capture("anchor").unwrap_or(scope);
            if !matches!(rule.anchor, Anchor::Preserve | Anchor::Zero)
                && rope.byte_to_line(anchor.start_byte()) >= line
            {
                return Ok(());
            }
            if matches!(rule.anchor, Anchor::FirstChild)
                && rope.byte_to_line(anchor.start_byte()) != rope.byte_to_line(scope.start_byte())
            {
                return Ok(());
            }
            let base = match rule.anchor {
                Anchor::Preserve => None,
                Anchor::Zero => Some(0),
                Anchor::Line => Some(column(source, rope, anchor.start_byte(), tab_width, true)),
                Anchor::Column | Anchor::FirstChild => {
                    Some(column(source, rope, anchor.start_byte(), tab_width, false))
                }
            };
            let value = base.map(|base| (base as i64 + rule.offset * width as i64).max(0) as usize);
            let candidate = (
                rule.precedence,
                scope.end_byte() - scope.start_byte(),
                value,
            );
            match selected {
                Some((precedence, size, previous))
                    if precedence == candidate.0 && size == candidate.1 && previous != value =>
                {
                    ambiguous = true;
                }
                Some((precedence, size, _))
                    if precedence > candidate.0
                        || (precedence == candidate.0 && size < candidate.1) => {}
                Some((precedence, size, _)) if precedence == candidate.0 && size == candidate.1 => {
                }
                _ => {
                    selected = Some(candidate);
                    ambiguous = false;
                }
            }
            Ok(())
        })?;
    }
    if ambiguous {
        return Err("ambiguous indentation rules at equal precedence".into());
    }
    let value = selected.map(|(_, _, value)| value);
    if value
        .flatten()
        .is_some_and(|value| value > MAX_INDENT_COLUMNS)
    {
        return Err("indentation exceeds the 256-column limit".into());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Buffer;
    use slotmap::SlotMap;

    fn ids() -> (SlotMap<BufferId, ()>, BufferId) {
        let mut ids = SlotMap::with_key();
        let buffer = ids.insert(());
        (ids, buffer)
    }

    #[test]
    fn incremental_trees_match_fresh_parsing_after_unicode_edits_undo_and_reload() {
        let (_, id) = ids();
        let buffer = Buffer::new();
        buffer.load_str("fn λ() { let s = \"🙂\u{2028}line\"; }\r\n");
        let plan = SyntaxPlan::compile("rust", "(identifier) @name\n(string_literal) @string", &[])
            .unwrap();
        let mut service = SyntaxService::default();
        for step in 0..5 {
            match step {
                1 => {
                    buffer.insert_pos("// é\n".into(), 0);
                }
                2 => {
                    buffer.undo();
                }
                3 => {
                    buffer.redo();
                }
                4 => {
                    buffer.load_str("fn other() {\n    let s = r###\"λ🙂\"###;\n}\n");
                }
                _ => {}
            }
            let (revision, rope) =
                buffer.with_read(|inner| (inner.text_revision, inner.buffer.clone()));
            let spans = service.highlights(id, revision, &rope, &plan).unwrap();
            let mut fresh = SyntaxService::default();
            assert_eq!(spans, fresh.highlights(id, revision, &rope, &plan).unwrap());
            assert_eq!(
                service.buffers[&id].tree.root_node().to_sexp(),
                fresh.buffers[&id].tree.root_node().to_sexp()
            );
            assert_eq!(
                service.buffers[&id].tree.root_node().end_position(),
                fresh.buffers[&id].tree.root_node().end_position()
            );
        }
    }

    #[test]
    fn overlapping_captures_are_normalized_before_unicode_conversion() {
        let (_, id) = ids();
        let source = "// λ🙂\nfn main() {}";
        let rope = Rope::from_str(source);
        let plan = SyntaxPlan::compile(
            "rust",
            "(source_file) @base\n\"fn\" @keyword\n(identifier) @name",
            &[],
        )
        .unwrap();
        let spans = SyntaxService::default()
            .highlights(id, 1, &rope, &plan)
            .unwrap();
        assert!(spans.windows(2).all(|pair| pair[0].end <= pair[1].start));
        assert!(
            spans
                .iter()
                .any(|span| span.capture == "keyword" && rope.slice(span.start..span.end) == "fn")
        );
        assert!(
            spans
                .iter()
                .any(|span| span.capture == "name" && rope.slice(span.start..span.end) == "main")
        );
    }

    #[test]
    fn syntax_limits_and_cache_eviction_are_explicit() {
        let (mut ids, first) = ids();
        let plan = SyntaxPlan::compile("rust", "(identifier) @name", &[]).unwrap();
        let rope = Rope::from_str("fn main() {}");
        let mut service = SyntaxService::default();
        service.highlights(first, 1, &rope, &plan).unwrap();
        for _ in 0..MAX_CACHED_BUFFERS {
            service.highlights(ids.insert(()), 1, &rope, &plan).unwrap();
        }
        assert_eq!(service.buffers.len(), MAX_CACHED_BUFFERS);
        assert!(!service.buffers.contains_key(&first));
        service.retain_live(&HashSet::new());
        assert!(service.buffers.is_empty());
        assert!(
            service
                .highlights(
                    first,
                    2,
                    &Rope::from_str(&"x".repeat(MAX_HIGHLIGHT_SOURCE_BYTES + 1)),
                    &plan
                )
                .is_err()
        );
        assert!(SyntaxPlan::compile("unavailable", "", &[]).is_err());
        assert!(SyntaxPlan::compile("rust", "(not_a_rust_node) @bad", &[]).is_err());
        assert!(SyntaxPlan::compile("rust", "((identifier) @x (#unknown? @x))", &[]).is_err());
        assert!(SyntaxPlan::compile("rust", &" ".repeat(MAX_QUERY_BYTES + 1), &[]).is_err());
        assert!(
            SyntaxPlan::compile(
                "rust",
                &format!("(identifier) @{}", "x".repeat(MAX_CAPTURE_NAME_BYTES + 1)),
                &[]
            )
            .is_err()
        );
        assert!(
            SyntaxPlan::compile(
                "rust",
                "",
                &[IndentRule {
                    query: "(block) @scope".into(),
                    anchor: "line".into(),
                    offset: 17,
                    precedence: 0
                }]
            )
            .is_err()
        );
    }

    #[test]
    fn effective_indentation_precedence_is_independent_of_rule_iteration_order() {
        let (_, id) = ids();
        let rope = Rope::from_str("fn f() {\nlet x = 1;\n}");
        let rule = |offset, precedence| IndentRule {
            query: "(block) @scope".into(),
            anchor: "line".into(),
            offset,
            precedence,
        };
        let mut rules = vec![rule(1, 100), rule(2, 100), rule(0, 200)];
        for _ in 0..3 {
            let plan = SyntaxPlan::compile("rust", "", &rules).unwrap();
            let edit = SyntaxService::default()
                .indent_edit(
                    id,
                    1,
                    &rope,
                    &plan,
                    IndentRequest {
                        cursor: 9,
                        newline: false,
                        width: 4,
                        tab_width: 4,
                    },
                )
                .unwrap();
            assert_eq!(edit.text, "");
            rules.rotate_left(1);
        }
        let plan = SyntaxPlan::compile("rust", "", &[rule(1, 100), rule(2, 100)]).unwrap();
        assert!(
            SyntaxService::default()
                .indent_edit(
                    id,
                    1,
                    &rope,
                    &plan,
                    IndentRequest {
                        cursor: 9,
                        newline: false,
                        width: 4,
                        tab_width: 4
                    }
                )
                .unwrap_err()
                .contains("ambiguous")
        );
    }
}
