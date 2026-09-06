# Typeout windows for Roe

Status: initial plain-text implementation complete

This note records the design and current implementation of transient command output in Roe. It is
not an architectural decision record; promotion, streaming, and structured output remain future
work.

## Summary

A typeout window presents temporary command output over the view that produced it. It is neither an
editable buffer nor a node in the logical split tree. Writing output exposes it; paging or dismissal
reveals the unchanged buffer presentation beneath it. Useful output can be promoted into a normal
buffer explicitly.

Roe should model typeout as a transient logical child of a `ViewId`. Mica owns when output goes to
the echo area, a typeout, or a buffer, along with typeout key behavior and lifecycle policy. Rust
owns bounded output storage, validated paging, and presentation transport. Terminal and Vello
consume the same logical typeout presentation but realize it differently.

## Historical context

"Typeout" was already TECO and ITS vocabulary for printed terminal output, including `--MORE--`
paging. The distinctive overlay discussed here came from the Lisp Machine window system rather than
from the universal Emacs buffer model.

The 1983 _Lisp Machine Window System Manual_ describes a typeout window as an inferior child of a
window whose normal display reflects a persistent data source. The child exposes itself when output
is directed to it. This was a general window-system service available to editor and scroll windows,
not a Zmacs-only invention.

Zmacs used that service at the top of an editor window. Typeout temporarily covered as much buffer
display as it needed, supported `More` paging, and disappeared without changing the buffer or editor
layout. This explains why it feels native in Zmacs: the surrounding platform already understood
transient output as a child presentation.

Hemlock, the Common Lisp Emacs from CMU, retained a closely related design. Its pop-up window
overlaid displayed text, paged long output, saved output in a "random typeout buffer," and could
promote it to an ordinary window. Hemlock supported terminal and CLX frontends, demonstrating that
the semantics do not require a bitmap compositor.

GNU Emacs mostly took another path: help and command output live in temporary buffers displayed in
ordinary windows. Its `momentary-string-display` is a small relative—it displays text without
modifying the buffer until the next input—but it is not a pageable child window.

Primary references:

- [Lisp Machine Window System Manual, edition 1.1, August 1983](http://bitsavers.informatik.uni-stuttgart.de/pdf/mit/cadr/LISPM_WindowSystem_Aug83.pdf)
- [Symbolics text editing and Zmacs manual, March 1985](https://bitsavers.computerhistory.org/pdf/symbolics/software/release_6/996035_Text_Editing_and_Processing_Mar85.pdf)
- [Hemlock User's Manual](https://cmucl.common-lisp.dev/docs/hem/user/hemlock-user.pdf)
- [GNU Emacs Lisp Reference: Temporary Displays](https://www.gnu.org/software/emacs/manual/html_node/elisp/Temporary-Displays.html)

## Design principles

1. **A typeout is not a buffer.** It has no file identity, point, mark, undo history, major mode, or
   place in the buffer list.
2. **A typeout is not a logical window.** Opening it must not split, resize, or replace nodes in
   Roe's view tree.
3. **A typeout belongs to a view.** The same buffer may appear in multiple views; output should
   emerge from the particular view that invoked the command.
4. **Exposure follows output.** A command selects a typeout destination and writes to it; it should
   not need to manage a renderer popup manually.
5. **Promotion is explicit.** Output becomes an editable or retainable results buffer only when Mica
   policy or the user requests it.
6. **The semantics are shared.** Vello may animate and composite the surface, but terminal Roe must
   receive and present the same logical object.

## Proposed Roe model

```text
Mica editor session
└── logical view
    ├── persistent buffer presentation
    ├── modeline
    └── optional active typeout
        └── bounded Rust output resource

attachment
└── typeout page/scroll position
```

The first implementation should permit one active typeout per editor session. Its association with
the originating view and buffer is session-volatile. A later design may permit one per view, but it
must not introduce an unbounded surface collection or output queue.

Candidate Mica relations are:

```mica
roe/Typeout(typeout)
roe/SessionTypeout(session, typeout)       // functional by session
roe/TypeoutView(typeout, view)             // functional by typeout
roe/TypeoutOriginBuffer(typeout, buffer)   // provenance, not ownership
roe/TypeoutKind(typeout, kind)             // help, evaluation, diagnostic, inspection
roe/TypeoutTitle(typeout, title)
```

The schemas are durable policy; individual typeout facts and identities are volatile. Mica never
receives the native output-resource handle. Rust associates that resource with the logical typeout
through the existing authorized host boundary.

The logical association and output content have different lifetimes:

| State                                               | Owner                    | Lifetime                        |
| --------------------------------------------------- | ------------------------ | ------------------------------- |
| Output destination, kind, title, active association | Mica                     | Editor session                  |
| Bounded text and text revision                      | Rust workspace mechanism | Active typeout                  |
| Page offset for a particular viewport               | Attachment               | Attachment or typeout dismissal |
| Visible slice and paging flags                      | Presentation snapshot    | Presentation revision           |
| Pixel inset, alpha, shadow, animation               | Vello                    | Render realization              |

Deleting the parent view must close its typeout. Switching that view to another buffer should
normally dismiss it because the visible output no longer has the same provenance; this remains Mica
policy rather than a native invariant.

## Presentation protocol

Nest typeout presentation under its parent rather than adding another `PresentedView`:

```rust
pub struct PresentedView {
    // existing fields
    pub typeout: Option<PresentedTypeout>,
}

pub struct PresentedTypeout {
    pub id: TypeoutId,
    pub kind: String,
    pub title: String,
    pub visible_text: String,
    pub first_visible_line: usize,
    pub total_lines: usize,
    pub more_before: bool,
    pub more_after: bool,
    pub complete: bool,
}
```

`TypeoutId` is an ephemeral transport identity, not native authority. The protocol communicates a
top-attached child and its visible contents; it does not encode Vello pixels. Add a typeout-specific
view invalidation so exposure, paging, and dismissal repaint only the owning view. Full snapshots
must be self-contained and include any active typeout.

## Behavior and input

While a typeout is exposed, a Mica-owned transient keymap takes precedence:

- `Space` advances one page, or dismisses the final page.
- `Backspace` or `Delete` moves to the previous page.
- `C-g` or `Esc` dismisses immediately.
- Another editor command dismisses the typeout and then executes against the underlying view,
  exactly once within the accepted input transaction.
- A future `keep-typeout` command copies the complete output into a results buffer and displays it
  through ordinary Mica window policy.

Rust performs bounded page movement; Mica selects the action and bindings. Pointer-sensitive items
and structured output should be deferred. When added, a frontend may report which realized surface
was hit, but Mica must still decide what the interaction means.

## Output routing

Typeout fills the gap between the echo area and a permanent results buffer:

| Output                                     | Default destination                            |
| ------------------------------------------ | ---------------------------------------------- |
| Brief status or scalar result              | Echo area, optionally mirrored to `*Messages*` |
| Multi-line help, evaluation, or inspection | Typeout                                        |
| Output explicitly retained by the user     | Results buffer                                 |
| Durable or substantial diagnostics         | Typeout plus diagnostics buffer                |

Region-evaluation success and failure now pass through the functional Mica `roe/OutputRoute`
relation. Shipped policy routes both to typeout while Rust continues to enforce the size and title
bounds. Other output producers still use their existing destinations and can migrate deliberately.

## Renderer realization

### Vello

The surface descends from the top of the active view's content area. It should be inset horizontally
so the parent remains visible at both sides, making the relationship clear. A slightly translucent
background, opaque text, subtle border, and downward shadow can reinforce that it is temporary.
Animation is renderer-local and keyed by typeout identity/revision; it does not delay logical
exposure or dismissal.

Initial visual parameters worth trying are a 12–16 px horizontal inset, a 92–96% opaque background,
and a maximum height of roughly two-thirds of the parent content area. Backdrop blur is unnecessary
for the first implementation.

### Terminal

The terminal composites the same child over rows at the top of the parent view, with a one-cell
inset or simple border. It appears and disappears immediately. Dismissal repaints the underlying
view from the current presentation snapshot rather than attempting to preserve terminal cells. A
small terminal may give the typeout the entire content area while retaining the modeline.

## Bounds and failure behavior

The first implementation should make all resource policy explicit:

- one active typeout per editor session;
- at most 65,536 text characters and 256 title characters;
- no hidden queue or unbounded history;
- replacement or explicit overload when another typeout is already active;
- a bounded visible slice in every presentation snapshot;
- recoverable diagnostics for malformed, unauthorized, oversized, or stale-view updates; and
- deterministic cleanup on view deletion, endpoint close, and workspace shutdown.

The producer should initially submit complete output atomically. Streaming output and true producer
backpressure at `More` are valuable later features, but they require explicit task, cancellation,
and queue ownership.

## Implementation status

The first implementation includes:

1. Volatile Mica ontology, functional evaluation-output routing, bounded Rust host state, nested
   presentation, targeted invalidation, attachment-local paging, and cleanup.
2. Plain-text terminal and Vello realization, including repaint of the underlying view and a
   headless Vello scene test.
3. Production routing for Mica region-evaluation success and failure, plus Space, Backspace/Delete,
   C-g/Esc, dismiss-and-redispatch keyboard behavior, and click dismissal.

The next slices are promotion to a results buffer and configurable Vello appearance, followed only
then by streaming, mouse-sensitive items, styled spans, or inspector objects.

## Open questions

The first slice replaces an existing typeout, makes dismissal session-wide, and keeps paging
attachment-local. Those choices can be revisited if multiple simultaneous producers become useful.
Remaining questions are:

- Which output classes are mirrored to `*Messages*`, and which remain only in their typeout
  resource?
- Should opacity and maximum extent be Mica configuration realized by the renderer, or frontend
  preferences outside editor policy?
- When a command is typed while typeout is visible, which commands should consume that input rather
  than dismiss-and-redispatch it?
