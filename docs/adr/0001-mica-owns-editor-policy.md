# ADR 0001: Mica owns logical editor policy

Status: accepted for Phase 3

## Context

Phase 2 placed terminal and Vello behind one session boundary, but `HostSession` still delegates
policy to Rust's `Editor`, `CommandRegistry`, modes, key bindings, and `ChromeAction`. Keeping a
second policy model in Rust would make Mica ornamental and would prevent live replacement.

## Decision

Mica owns editor sessions, actors, logical buffers, frames, view trees, active views, cursors,
marks, selections, commands, interactive argument acquisition, command discovery, keymaps, modes,
hooks, faces, syntax rules, configuration, packages, and named units.

Rust owns Rope storage, validated text mutations, undo primitives, file/process/clipboard/clock and
watch mechanisms, logical geometry validation, presentation realization, GPU/terminal resources, and
the process/session transport. Rust may enforce mechanical invariants but may not choose a command,
binding, mode, hook, completion candidate, or package.

The checked prototype is [`mica/roe-model.mica`](../../mica/roe-model.mica). It is intentionally
ordinary Mica source with no Roe-specific runtime builtin.

## State classes

| Class                 | Examples                                                                                                                    | Creation and removal                                                                                 | Persistence                                 |
| --------------------- | --------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- | ------------------------------------------- |
| Durable description   | commands, selectors, summaries, keymaps/bindings, modes, hooks, faces, syntax rules, configuration, package-to-unit mapping | named unit filein/add/replace/disable                                                                | fileout-able after persistence is enabled   |
| Session-volatile      | editor session, actor, frame/view tree, active view, buffer/view association, cursor, mark, selection, active keymaps       | asserted with endpoint open; retracted with endpoint close                                           | never copied to a durable unit              |
| Derived               | effective binding, visible buffers, effective mode/hook/face composition                                                    | relation rules; recomputed from authoritative facts                                                  | never persisted as authority                |
| Native-cached         | buffer/presentation revision observations                                                                                   | host assertion after native completion/effect                                                        | disposable and reconstructible              |
| Ephemeral association | endpoint identity, native text resource and generation                                                                      | host volatile tuples; invalidated on close, resource removal, driver failure, or generation mismatch | prohibited from fileout and durable storage |

## Consequences

The Mica model may describe chrome content, but terminal/Vello still render chrome. A Mica identity
may name a logical buffer without being a native capability. Every native request re-resolves the
volatile association and checks endpoint authority. Phase 5 must delete each replaced Rust policy
path rather than synchronizing two sources of truth.
