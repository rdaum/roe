// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com> This program is free
// software: you can redistribute it and/or modify it under the terms of the GNU
// General Public License as published by the Free Software Foundation, version
// 3.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.

use mica_compiler::{SyntaxKind, Token, lex};

pub(crate) const MAX_HIGHLIGHT_SOURCE_BYTES: usize = 1_048_576;
pub(crate) const MAX_HIGHLIGHT_SPANS: usize = 16_384;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HighlightSpan {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) capture: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ByteHighlightSpan {
    start: usize,
    end: usize,
    capture: &'static str,
}

/// Lex Mica source into bounded, character-indexed semantic captures.
///
/// `mica-compiler` reports byte spans while Roe's native and presentation
/// boundaries use character indices. Conversion walks the ordered spans and
/// source once, so Unicode does not turn highlighting into quadratic work.
pub(crate) fn mica_highlights(source: &str) -> Vec<HighlightSpan> {
    if source.len() > MAX_HIGHLIGHT_SOURCE_BYTES {
        tracing::warn!(
            source_bytes = source.len(),
            limit = MAX_HIGHLIGHT_SOURCE_BYTES,
            "Mica syntax highlighting skipped for oversized buffer"
        );
        return Vec::new();
    }

    let tokens = lex(source);
    let byte_spans = mica_byte_highlights(&tokens);
    byte_spans_to_chars(source, byte_spans)
}

fn mica_byte_highlights(tokens: &[Token]) -> Vec<ByteHighlightSpan> {
    let mut highlights = Vec::new();
    let mut index = 0;
    while let Some(token) = tokens.get(index) {
        if highlights.len() == MAX_HIGHLIGHT_SPANS {
            tracing::warn!(
                limit = MAX_HIGHLIGHT_SPANS,
                "Mica syntax highlights truncated at span limit"
            );
            break;
        }

        if let Some(capture) = prefixed_capture(token.kind)
            && let Some(first) = tokens.get(index + 1)
            && first.kind == SyntaxKind::Ident
            && token.span.end == first.span.start
        {
            let (end, next_index) = qualified_identifier_end(tokens, index + 1);
            highlights.push(ByteHighlightSpan {
                start: token.span.start,
                end,
                capture,
            });
            index = next_index;
            continue;
        }

        if token.kind == SyntaxKind::Ident {
            let (end, next_index) = qualified_identifier_end(tokens, index);
            highlights.push(ByteHighlightSpan {
                start: token.span.start,
                end,
                capture: "identifier",
            });
            index = next_index;
            continue;
        }

        if let Some(capture) = token_capture(token.kind) {
            highlights.push(ByteHighlightSpan {
                start: token.span.start,
                end: token.span.end,
                capture,
            });
        }
        index += 1;
    }
    highlights
}

fn qualified_identifier_end(tokens: &[Token], index: usize) -> (usize, usize) {
    let mut end = tokens[index].span.end;
    let mut next_index = index + 1;
    while let (Some(slash), Some(next)) = (tokens.get(next_index), tokens.get(next_index + 1)) {
        if slash.kind != SyntaxKind::Slash
            || next.kind != SyntaxKind::Ident
            || slash.span.start != end
            || slash.span.end != next.span.start
        {
            break;
        }
        end = next.span.end;
        next_index += 2;
    }
    (end, next_index)
}

fn prefixed_capture(kind: SyntaxKind) -> Option<&'static str> {
    match kind {
        SyntaxKind::Colon => Some("symbol"),
        SyntaxKind::Hash => Some("identity"),
        SyntaxKind::Question => Some("variable"),
        _ => None,
    }
}

fn token_capture(kind: SyntaxKind) -> Option<&'static str> {
    match kind {
        SyntaxKind::LineComment => Some("comment"),
        SyntaxKind::String | SyntaxKind::Bytes => Some("string"),
        SyntaxKind::Int | SyntaxKind::Float => Some("number"),
        SyntaxKind::LetKw
        | SyntaxKind::ConstKw
        | SyntaxKind::IfKw
        | SyntaxKind::MatchKw
        | SyntaxKind::CaseKw
        | SyntaxKind::ExactlyKw
        | SyntaxKind::ElseIfKw
        | SyntaxKind::ElseKw
        | SyntaxKind::EndKw
        | SyntaxKind::BeginKw
        | SyntaxKind::ForKw
        | SyntaxKind::InKw
        | SyntaxKind::WhileKw
        | SyntaxKind::ReturnKw
        | SyntaxKind::RaiseKw
        | SyntaxKind::RecoverKw
        | SyntaxKind::SpawnKw
        | SyntaxKind::AfterKw
        | SyntaxKind::NotKw
        | SyntaxKind::BreakKw
        | SyntaxKind::ContinueKw
        | SyntaxKind::TryKw
        | SyntaxKind::CatchKw
        | SyntaxKind::AsKw
        | SyntaxKind::FinallyKw
        | SyntaxKind::FnKw
        | SyntaxKind::MethodKw
        | SyntaxKind::VerbKw
        | SyntaxKind::DoKw
        | SyntaxKind::AssertKw
        | SyntaxKind::RetractKw
        | SyntaxKind::RequireKw
        | SyntaxKind::TrueKw
        | SyntaxKind::FalseKw => Some("keyword"),
        SyntaxKind::Eq
        | SyntaxKind::EqEq
        | SyntaxKind::BangEq
        | SyntaxKind::Lt
        | SyntaxKind::LtEq
        | SyntaxKind::Gt
        | SyntaxKind::GtEq
        | SyntaxKind::Plus
        | SyntaxKind::Minus
        | SyntaxKind::Star
        | SyntaxKind::Slash
        | SyntaxKind::Percent
        | SyntaxKind::AmpAmp
        | SyntaxKind::PipePipe
        | SyntaxKind::Pipe
        | SyntaxKind::Membership
        | SyntaxKind::Bang
        | SyntaxKind::Arrow
        | SyntaxKind::FatArrow
        | SyntaxKind::ColonDash => Some("operator"),
        SyntaxKind::Error | SyntaxKind::ErrorCode => Some("error"),
        SyntaxKind::Underscore => Some("identifier"),
        _ => None,
    }
}

fn byte_spans_to_chars(source: &str, spans: Vec<ByteHighlightSpan>) -> Vec<HighlightSpan> {
    let mut byte_cursor = 0;
    let mut char_cursor = 0;
    let mut converted = Vec::with_capacity(spans.len());
    for span in spans {
        if span.start < byte_cursor
            || span.end < span.start
            || span.end > source.len()
            || !source.is_char_boundary(span.start)
            || !source.is_char_boundary(span.end)
        {
            continue;
        }
        char_cursor += source[byte_cursor..span.start].chars().count();
        let start = char_cursor;
        char_cursor += source[span.start..span.end].chars().count();
        converted.push(HighlightSpan {
            start,
            end: char_cursor,
            capture: span.capture,
        });
        byte_cursor = span.end;
    }
    converted
}

#[cfg(test)]
mod tests {
    use super::{HighlightSpan, MAX_HIGHLIGHT_SOURCE_BYTES, mica_highlights};

    #[test]
    fn classifies_mica_tokens_and_qualified_atoms() {
        let source = "// hi\nverb roe/demo(?name, :ok, #roe/item)\n  let n = 42\nend";
        let spans = mica_highlights(source);
        let captured: Vec<_> = spans
            .iter()
            .map(|span| {
                (
                    &source[char_to_byte(source, span.start)..char_to_byte(source, span.end)],
                    span.capture,
                )
            })
            .collect();
        assert!(captured.contains(&("// hi", "comment")));
        assert!(captured.contains(&("verb", "keyword")));
        assert!(captured.contains(&("roe/demo", "identifier")));
        assert!(captured.contains(&("?name", "variable")));
        assert!(captured.contains(&(":ok", "symbol")));
        assert!(captured.contains(&("#roe/item", "identity")));
        assert!(captured.contains(&("42", "number")));
    }

    #[test]
    fn converts_compiler_byte_spans_to_character_indices() {
        let spans = mica_highlights("\u{3bb}\nlet value = \"\u{1f642}\"");
        assert!(spans.contains(&HighlightSpan {
            start: 2,
            end: 5,
            capture: "keyword",
        }));
        assert!(spans.contains(&HighlightSpan {
            start: 14,
            end: 17,
            capture: "string",
        }));
    }

    #[test]
    fn malformed_and_oversized_sources_are_recoverable() {
        assert!(
            mica_highlights("let text = \"unterminated")
                .iter()
                .any(|span| span.capture == "string")
        );
        let oversized = "x".repeat(MAX_HIGHLIGHT_SOURCE_BYTES + 1);
        assert!(mica_highlights(&oversized).is_empty());
    }

    fn char_to_byte(source: &str, target: usize) -> usize {
        source
            .char_indices()
            .nth(target)
            .map_or(source.len(), |(byte, _)| byte)
    }
}
