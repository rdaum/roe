// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Bounded, nonrecursive parsing of Mica-selected source regions.

use super::*;

pub(super) fn inject_highlights<'a>(
    plan: &'a InjectionPlan,
    tree: &Tree,
    source: &str,
    priority: usize,
    spans: &mut Vec<(usize, usize, usize, &'a str)>,
) -> Result<(), String> {
    let deadline = Instant::now() + WORK_TIME;
    let mut regions = Vec::new();
    let mut range_count = 0;
    visit_matches_until(&plan.regions, tree, source, None, deadline, |matched| {
        for capture in matched.captures {
            if regions.len() == MAX_INJECTION_REGIONS {
                return Err("syntax injections exceed the 1024-region limit".into());
            }
            // Exclude grammar-recognized continuation markers (for example,
            // quote prefixes) from inline parsing, but keep absolute offsets.
            let node = capture.node;
            let mut start_byte = node.start_byte();
            let mut start_point = node.start_position();
            let mut ranges = Vec::new();
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if Instant::now() >= deadline {
                    return Err("syntax injections exceeded their time budget".into());
                }
                if start_byte < child.start_byte() {
                    range_count += 1;
                    if range_count > MAX_INJECTION_RANGES {
                        return Err("syntax injections exceed their range limit".into());
                    }
                    ranges.push(tree_sitter::Range {
                        start_byte,
                        end_byte: child.start_byte(),
                        start_point,
                        end_point: child.start_position(),
                    });
                }
                start_byte = child.end_byte();
                start_point = child.end_position();
            }
            if start_byte < node.end_byte() {
                range_count += 1;
                if range_count > MAX_INJECTION_RANGES {
                    return Err("syntax injections exceed their range limit".into());
                }
                ranges.push(tree_sitter::Range {
                    start_byte,
                    end_byte: node.end_byte(),
                    start_point,
                    end_point: node.end_position(),
                });
            }
            regions.push(ranges);
        }
        Ok(())
    })?;
    let mut parser = Parser::new();
    parser
        .set_language(&plan.language)
        .map_err(|error| error.to_string())?;
    for ranges in regions {
        if Instant::now() >= deadline {
            return Err("syntax injections exceeded their time budget".into());
        }
        if ranges.is_empty() {
            continue;
        }
        parser
            .set_included_ranges(&ranges)
            .map_err(|error| error.to_string())?;
        let mut progress = |_: &tree_sitter::ParseState| {
            if Instant::now() >= deadline {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let inline = parser
            .parse_with_options(
                &mut |byte, _| &source.as_bytes()[byte.min(source.len())..],
                None,
                Some(ParseOptions::new().progress_callback(&mut progress)),
            )
            .ok_or("syntax injections exceeded their time budget")?;
        visit_matches_until(
            &plan.highlights,
            &inline,
            source,
            None,
            deadline,
            |matched| {
                if Instant::now() >= deadline {
                    return Err("syntax injections exceeded their time budget".into());
                }
                for capture in matched.captures {
                    // Captures can cross excluded ranges. Never style those gaps.
                    for range in &ranges {
                        if Instant::now() >= deadline {
                            return Err("syntax injections exceeded their time budget".into());
                        }
                        let start = capture.node.start_byte().max(range.start_byte);
                        let end = capture.node.end_byte().min(range.end_byte);
                        if start >= end {
                            continue;
                        }
                        if spans.len() == MAX_HIGHLIGHT_SPANS {
                            return Err("syntax highlights exceed the capture limit".into());
                        }
                        spans.push((
                            start,
                            end,
                            priority + matched.pattern_index,
                            plan.highlights.capture_names()[capture.index as usize],
                        ));
                    }
                }
                Ok(())
            },
        )?;
    }
    Ok(())
}
