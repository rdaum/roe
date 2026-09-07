// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use crate::Buffer;

fn plan() -> SyntaxPlan {
    SyntaxPlan::compile(
        "markdown",
        "(atx_heading) @heading (fenced_code_block) @code",
        &[],
    )
    .unwrap()
    .with_injection(
        "markdown_inline",
        "[(inline) (pipe_table_cell)] @content",
        "(emphasis) @emphasis (strong_emphasis) @strong (code_span) @code",
    )
    .unwrap()
}

#[test]
fn markdown_inline_regions_respect_blocks_unicode_and_incremental_edits() {
    let plan = plan();
    let mut buffers = slotmap::SlotMap::with_key();
    let buffer = buffers.insert(Buffer::named("notes", crate::buffer::BufferKind::Ordinary));
    let mut syntax = SyntaxService::default();
    for (revision, source) in [
        "# λ **bold**\n\n*one*\n\n```rust\n*not emphasis*\n```\n",
        "# 🙂 **bold**\r\n\r\n*one*\r\n\r\n> *multi\r\n> line*\r\n",
        "*not closed\n\nnot opened*\n",
        "- **list**\n\n| λ | **cell** |\n|---|---|\n| a | b |\n",
        "```\n*now code*\n```\n\n*now prose*",
        "*now code*\n\n*now prose*",
    ]
    .into_iter()
    .enumerate()
    {
        let rope = Rope::from_str(source);
        let spans = syntax
            .highlights(buffer, revision as u64, &rope, &plan)
            .unwrap();
        let fresh = SyntaxService::default()
            .highlights(buffer, revision as u64, &rope, &plan)
            .unwrap();
        assert_eq!(spans, fresh, "{source:?}");
        assert!(spans.windows(2).all(|pair| pair[0].end <= pair[1].start));
        assert!(spans.iter().all(|span| span.end <= rope.len_chars()));
        if source.starts_with("*not closed") {
            assert!(
                spans.is_empty(),
                "separate paragraphs must not form emphasis: {spans:?}"
            );
        }
        if source.contains("*not emphasis*") {
            let at = source[..source.find("not emphasis").unwrap()]
                .chars()
                .count();
            assert!(
                spans
                    .iter()
                    .any(|span| span.start <= at && span.end > at && span.capture == "code")
            );
        }
        if source.contains("> *multi") {
            let marker = source[..source.rfind('>').unwrap()].chars().count();
            assert!(
                !spans
                    .iter()
                    .any(|span| span.start <= marker && span.end > marker),
                "inline capture must exclude the quote continuation marker"
            );
            let at = source[..source.find("line*").unwrap()].chars().count();
            assert!(
                spans
                    .iter()
                    .any(|span| span.start <= at && span.end > at && span.capture == "emphasis")
            );
        }
    }
}

#[test]
fn markdown_injections_validate_operands_and_bound_regions() {
    for (grammar, regions, highlights) in [
        ("unavailable", "(inline) @content", ""),
        ("markdown_inline", "(inline) @wrong", ""),
        ("markdown_inline", "(inline) @content", "(not_a_node) @code"),
    ] {
        assert!(
            SyntaxPlan::compile("markdown", "", &[])
                .unwrap()
                .with_injection(grammar, regions, highlights)
                .is_err()
        );
    }
    let mut buffers = slotmap::SlotMap::with_key();
    let buffer = buffers.insert(Buffer::named("notes", crate::buffer::BufferKind::Ordinary));
    let source = Rope::from_str(&"*a*\n\n".repeat(MAX_INJECTION_REGIONS + 1));
    assert!(
        SyntaxService::default()
            .highlights(buffer, 0, &source, &plan())
            .is_err()
    );
}
