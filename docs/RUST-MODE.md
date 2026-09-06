# Rust mode

Roe provides Rust syntax highlighting and line indentation in both frontends.
The mode does not require Cargo, rust-analyzer, or rustfmt.
It works with incomplete source and buffers without files.

## Use the mode

Open an `.rs` file to select Rust mode automatically.
For another buffer, run `M-x rust-mode`.

| Input | Action |
| --- | --- |
| `Tab` | Reindent the current line. A second Tab does not add indentation. |
| `Enter` | Insert a newline and indent it in one undo group. |
| `M-x indent-line` | Reindent the current line with its mode rules. |
| `M-x newline-and-indent` | Insert a newline with its mode rules. |
| `C-/` | Undo the last edit. |

Rust mode uses four spaces per indentation unit.
The indentation width is separate from the tab display width.
Indentation changes leading whitespace and preserves the cursor position relative to the code.

The shipped rules cover blocks, declarations, argument lists, arrays, match arms, and common continuation expressions.
Rules preserve existing whitespace inside multiline strings, block comments, and macro token trees.
Enter does not add indentation inside these protected regions.
The mode does not expand macros or promise rustfmt-equivalent formatting.

## Change the policy

The package is `roe/rust_package`.
Its source unit is `roe/rust`, in [`mica/roe-rust.mica`](../mica/roe-rust.mica).
Mica owns mode selection, key bindings, highlight queries, face mappings, and indentation rules.

To export the current unit, run:

```bash
./scripts/run.sh --mica-export roe/rust /tmp/roe-rust.mica
```

Edit the exported source.
For a two-space indentation width, change this fact:

```mica
assert roe/Configuration(#roe/rust_mode, :indent_width, 2)
```

To load the replacement, run:

```bash
./scripts/run.sh --mica-replace roe/rust /tmp/roe-rust.mica example.rs
```

The Vello frontend accepts the same recovery arguments.
The session recovery interface also supports replacement in a live workspace.
Mica rejects invalid source before replacement.
Native query validation rolls back a candidate with invalid grammar, queries, or indentation operands.
The last complete presentation policy remains available after a rejected publication.

To disable the Rust package, run:

```bash
./scripts/run.sh --mica-disable-package roe/rust_package example.rs
```

To restore the shipped policy units, run:

```bash
./scripts/run.sh --mica-restore-first-wave example.rs
```

Durable user configuration is not enabled.
An exported unit provides an explicit source file for later replacement.

## Highlight queries

`roe/SyntaxParser(mode, grammar, query)` selects the native syntax provider and query text.
The Rust provider uses the locked Tree-sitter Rust grammar.
The Mica provider retains its compiler lexer.

Tree-sitter queries produce named captures.
`roe/SyntaxHighlightRule(mode, capture, face, precedence)` maps each capture to a Mica face.
Effective major-mode and minor-mode rules compose for each buffer.
Later query patterns win for overlapping captures.
Equal-precedence face conflicts produce no face for that capture.

The native service converts byte ranges into character ranges after overlap resolution.
Both frontends receive the same styled ranges through the revisioned presentation protocol.
Neither frontend selects a language or interprets syntax.

## Indentation rules

Each rule has this form:

```mica
assert roe/IndentationRule(mode, query, anchor, offset, precedence, enabled)
```

The query identifies a syntax scope and optional anchor and target nodes.
The rule offset uses indentation-width units.
The highest precedence wins.
The smallest matching scope breaks equal-precedence ties.
Conflicting results at an equal precedence and scope size produce a diagnostic.

| Capture | Meaning |
| --- | --- |
| `@scope` | The syntax node that contains the line. Every rule requires this capture. |
| `@anchor` | The reference node. The scope is the default anchor. |
| `@target` | A token that must start at the first non-whitespace position. |

| Anchor | Base indentation |
| --- | --- |
| `:line` | The indentation of the anchor line. |
| `:column` | The column of the anchor node. |
| `:first_child` | The anchor column, only if the anchor starts on the scope's first line. |
| `:preserve` | The existing whitespace, without an offset. |
| `:zero` | Column zero. |

For example, these rules indent block contents and align a closing brace:

```mica
assert roe/IndentationRule(#roe/rust_mode, "(block) @scope", :line, 1, 100, true)
assert roe/IndentationRule(#roe/rust_mode, "(block \"}\" @target) @scope", :line, 0, 200, true)
```

These examples describe existing rules, not additional facts for the shipped unit.

## Native ownership and bounds

The workspace owns one syntax service, shared by highlighting and indentation.
The service retains at most 16 buffer trees and evicts the least recently used tree.
It removes trees for dead buffers and invalidates trees after a grammar change.
It derives incremental edits from coherent Rope snapshots, including undo, redo, and file reloads.
It does not retain an edit journal or publish syntax nodes as Mica facts.

Each source has a 1 MiB limit.
Each highlight query has a 64 KiB limit and at most 256 patterns and captures.
Each capture name has a 128-byte limit.
Each mode has at most 64 indentation rules and 64 KiB of indentation query text.
The query cursor permits 256 in-progress matches and at most 16,384 results.
The final highlight output also permits at most 16,384 spans.
Parsing and query work use cooperative 50 ms deadlines.
An indentation operation also checks a shared deadline across its rules.

Indentation widths and tab widths range from 1 through 16.
Offsets range from -16 through 16 units.
The result cannot exceed 256 columns.
An indentation error leaves the text unchanged and reports a lifecycle diagnostic.
An unavailable highlight result omits syntax styles for that revision and records a warning in tracing.

The endpoint validates buffer and service associations.
The kernel validates capabilities, resource generations, revisions, ranges, and read-only state.
The final replacement holds one buffer write lock and records one undo group.

## Verification

Run the focused checks:

```bash
cargo test -p roe-core rust_mode -- --test-threads=1
cargo test -p roe-core syntax::tests -- --test-threads=1
cargo test -p roe-core checked_native -- --test-threads=1
cargo test -p roe-vello --test session_conformance
cargo test -p roe-vello production_rust_mode_builds_a_vello_scene_without_a_display
./scripts/test-phase0-terminal-workflows.sh
```

Run the production-path Rust measurement:

```bash
cargo run --release -p roe-terminal --example phase0_baseline -- --rust
```

The default measurement still uses the original fundamental-mode fixture.
The Rust measurement includes Mica dispatch, incremental parsing, highlighting, and presentation updates.
Headless scene tests do not replace a display-host GPU smoke test.
