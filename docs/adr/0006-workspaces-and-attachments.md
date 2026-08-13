# ADR 0006: separate persistent workspaces from frontend attachments

Status: accepted and implemented

## Context

The original in-process session combined the durable editor and Mica state with one frontend's
viewport, ordering, presentation revision, and close behavior. That shape could not represent a
persistent editor surviving a dropped terminal, a compositor frontend, or a remote attachment
without making transport loss terminate the editor. Clipboard access was also incorrectly located
with backend-native services even though a remote workspace must use the attaching machine's
clipboard.

## Decision

`WorkspaceHost` owns editor buffers and logical windows, Mica, native resources, files, watchers,
and processes. `Attachment` owns viewport and focus, input epoch and sequence, presentation
revision, pointer and scroll state, and attachment-local service capabilities.

Frontends use the `SessionClient` interface. `DirectSessionClient` is the in-process implementation
and invokes `WorkspaceHost` directly. A remote client and server must implement this protocol
directly; there is no retained `HostSession`, compatibility wrapper, or parallel frontend API.
Protocol values are owned and Serde-compatible so CBOR can be used without exposing Rust resource
or renderer handles.

Attach, detach, resume, close-attachment, and terminate-workspace are distinct operations. Resume
allocates a new epoch and resets input ordering while retaining workspace text and attachment
scroll. Closing an attachment is permanent but leaves the workspace available for a new
attachment. Terminating a workspace closes Mica and invalidates native resources.

Client-caused output acknowledges the accepted input sequence. Server-originated output uses no
input acknowledgement and is polled independently in the direct implementation; a message
transport may push it. Background Mica work, watcher changes, diagnostics, and frontend-service
completions therefore never manufacture client input.

Files, watches, and processes execute where `WorkspaceHost` runs. Clipboard and notifications are
attachment-local and use a bounded, correlated server-to-client request/result vocabulary. The
workspace kill ring has no platform clipboard dependency. At most 16 frontend requests may be
outstanding and frontend text payloads are limited to 65,536 characters.

## Consequences

- A dropped connection can detach without destroying buffers or Mica work.
- Embedded tests retain a zero-serialization direct path.
- A ZeroMQ/CBOR transport can be added by implementing `SessionClient` and a server dispatcher,
  without changing editor ownership or preserving an older API.
- Each attachment has independent ordering, revision, viewport, focus, pointer, scroll, and local
  service authority.
- Backend code cannot reach the process-global clipboard; terminal and Vello fulfill requests on
  their own side of the attachment boundary.

This ADR supersedes ADR 0002 only where that ADR coupled endpoint closure to session/workspace
shutdown. Its single-consumer, bounded-driver, and orderly Mica shutdown decisions remain in force.
