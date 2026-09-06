//! Public-driver-only Mica embedding for Roe's session host.
//!
//! Mica owns command/keymap policy. This module translates volatile Roe
//! identities, bounded native requests, and committed effects at the host
//! boundary; it does not expose renderer or Rust policy objects to Mica.

mod decode;
mod events;
mod source_provider;
mod syntax_policy;
pub use events::{MicaEvent, MicaEventBatch, MicaHostAction, MicaNativeAction, MicaPolicyFact};
#[cfg(test)]
use source_provider::source_path;
use source_provider::{
    RoeBufferSourceProvider, RoeSourceBuffers, source_context_facts, source_relative_path,
    synchronize_source_buffers,
};

use crate::editor::{SplitDirection, WindowNode};
use crate::native_kernel::{KernelError, NativeKernel, NativeOperation, NativeResult, ResourceId};
use crate::native_services::FrontendWake;
use crate::{BufferId, Editor, WindowId};
#[cfg(test)]
use mica_driver::TaskId;
use mica_driver::{
    DriverAdministrator, DriverClient, DriverError, DriverEvent, DriverEventPump, DriverOwner,
    DriverResources, EndpointConfiguration, EndpointSession, ExternalRequestContext,
    ExternalRequestFuture, ExternalRequestHandler, ExternalStreamRequestHandler, FileinMode,
    Identity, InvocationHandle, InvocationOutcome, RelationAcceleration, SourceConfig, Symbol,
    TaskLimits, Value,
};
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Debug, Clone)]
enum LayoutFact {
    View(Identity),
    Root(Identity, Identity),
    First(Identity, Identity),
    Second(Identity, Identity),
    Axis(Identity, Symbol),
    Ratio(Identity, f32),
    Next(Identity, Identity),
}

macro_rules! layout_named_tuples {
    ($facts:expr) => {{
        let mut tuples = Vec::new();
        for fact in $facts {
            match *fact {
                LayoutFact::View(view) => {
                    tuples.push((sym("roe/View"), [Value::identity(view)].into()))
                }
                LayoutFact::Root(frame, root) => tuples.push((
                    sym("roe/FrameRootView"),
                    [Value::identity(frame), Value::identity(root)].into(),
                )),
                LayoutFact::First(parent, child) => tuples.push((
                    sym("roe/ViewFirstChild"),
                    [Value::identity(parent), Value::identity(child)].into(),
                )),
                LayoutFact::Second(parent, child) => tuples.push((
                    sym("roe/ViewSecondChild"),
                    [Value::identity(parent), Value::identity(child)].into(),
                )),
                LayoutFact::Axis(view, axis) => tuples.push((
                    sym("roe/ViewSplitAxis"),
                    [Value::identity(view), Value::symbol(axis)].into(),
                )),
                LayoutFact::Ratio(view, ratio) => tuples.push((
                    sym("roe/ViewSplitRatio"),
                    [
                        Value::identity(view),
                        Value::float(ratio).expect("finite normalized split ratio"),
                    ]
                    .into(),
                )),
                LayoutFact::Next(current, next) => tuples.push((
                    sym("roe/NextView"),
                    [Value::identity(current), Value::identity(next)].into(),
                )),
            }
        }
        tuples
    }};
}

const CORE_SOURCE: &str = include_str!("../../mica/roe-model.mica");
const FIRST_WAVE_SOURCE: &str = include_str!("../../mica/roe-first-wave.mica");
const RUST_SOURCE: &str = include_str!("../../mica/roe-rust.mica");
const AGENT_SOURCE: &str = include_str!("../../mica/roe-agent.mica");
const EVENT_QUEUE_CAPACITY: usize = 256;
const EXTERNAL_REQUEST_CAPACITY: usize = 16;
const SUBSCRIPTION_QUEUE_BUDGET: usize = 64;
const ACTIVE_TASK_CAPACITY: usize = 128;
const SUSPENDED_TASK_CAPACITY: usize = 64;
const TIMER_CAPACITY: usize = 64;
const TERMINAL_TASK_RETENTION: usize = 256;
const MAX_PROMPT_CANDIDATES: usize = 256;
const MAX_SEARCH_MATCHES: usize = 1_024;
const ROE_BUFFER_SOURCE_PROVIDER: &str = "roe-buffer";

/// Keep each HTTP future inside the driver's bounded external-request slot.
/// Mica runs the producer as an endpoint-owned child while the transcript task
/// receives its mailbox events. Closing the endpoint drops the network future.
fn agent_stream_handler() -> ExternalStreamRequestHandler {
    Arc::new(|_, request, emitter| {
        Box::pin(async move {
            if let Err(message) =
                mica_external_http::perform_external_stream_request(request, &emitter).await
            {
                let _ = emitter
                    .emit(Value::map([
                        (Value::symbol(sym("type")), Value::symbol(sym("error"))),
                        (Value::symbol(sym("message")), Value::string(message)),
                    ]))
                    .await;
            }
            Value::bool(true)
        })
    })
}

#[derive(Debug, thiserror::Error)]
pub enum MicaHostError {
    #[error("Mica driver failed: {0}")]
    Driver(#[from] DriverError),
    #[error("Mica session has no logical identity for the active Rust object")]
    MissingIdentity,
    #[error("Mica session host is already closed")]
    Closed,
    #[error("Roe native kernel failed while opening the Mica session: {0}")]
    Kernel(#[from] KernelError),
    #[error("Mica editor policy rejected the operation: {0}")]
    Policy(String),
    #[error("Mica source-provider configuration failed: {0}")]
    SourceConfiguration(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicaPresentationEffect {
    pub buffer: BufferId,
    pub view: WindowId,
    pub cursor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MicaKeyResult {
    Unbound,
    Prefix,
    Handled,
    Failed(String),
}

/// Display-only prompt data. Candidate identities and interaction state stay in Mica.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicaPromptUpdate {
    pub prefix: String,
    pub query: String,
    pub selected: usize,
    pub candidates: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicaSearchUpdate {
    pub view: WindowId,
    pub matches: Vec<(usize, usize)>,
    pub selected: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicaSearchFinish {
    pub view: WindowId,
    pub original_cursor: usize,
    pub query: String,
    pub accepted: bool,
}

#[derive(Debug)]
pub struct MicaDispatchResult {
    pub key: MicaKeyResult,
    pub events: MicaEventBatch,
}

#[derive(Debug)]
pub struct MicaEvaluationResult {
    pub value: String,
    pub events: MicaEventBatch,
}

struct NativeBridge {
    kernel: Arc<Mutex<NativeKernel>>,
    state: Mutex<NativeBridgeState>,
}

struct MicaFrontendWake(Arc<dyn FrontendWake>);

impl mica_driver::DriverWake for MicaFrontendWake {
    fn wake(&self) {
        self.0.wake();
    }
}

#[derive(Default)]
struct NativeBridgeState {
    actor: Option<Identity>,
    resources: HashMap<Identity, ResourceId>,
    services: HashSet<Symbol>,
}

impl NativeBridge {
    fn new(kernel: Arc<Mutex<NativeKernel>>) -> Self {
        Self {
            kernel,
            state: Mutex::new(NativeBridgeState::default()),
        }
    }

    fn configure(
        &self,
        actor: Identity,
        resources: HashMap<Identity, ResourceId>,
        services: HashSet<Symbol>,
    ) {
        let mut state = self.state.lock().unwrap();
        state.actor = Some(actor);
        state.resources = resources;
        state.services = services;
    }

    fn add_resource(&self, buffer: Identity, resource: ResourceId) {
        self.state
            .lock()
            .unwrap()
            .resources
            .insert(buffer, resource);
    }

    fn remove_resource(&self, buffer: Identity) {
        self.state.lock().unwrap().resources.remove(&buffer);
    }

    #[cfg(test)]
    fn revoke_service(&self, service: Symbol) {
        self.state.lock().unwrap().services.remove(&service);
    }

    async fn handle(
        &self,
        context: ExternalRequestContext,
        service: Symbol,
        payload: Value,
    ) -> Value {
        if context.cancellation.is_cancelled() {
            return native_error("request cancelled before native admission");
        }
        let operation = {
            let state = self.state.lock().unwrap();
            if context.actor != state.actor {
                return native_error("request actor does not own this Roe endpoint");
            }

            let required_service = if service == sym("clock_millis") {
                sym("clock_read")
            } else if service == sym("text_insert") {
                sym("text_write")
            } else if service == sym("text_search") {
                sym("text_read")
            } else if service == sym("list_directory") {
                sym("file_read")
            } else {
                return native_error("unknown Roe native service");
            };
            if !state.services.contains(&required_service) {
                return native_error("request actor lacks the required native service grant");
            }

            if service == sym("clock_millis") {
                NativeOperation::ReadClockMillis
            } else if service == sym("list_directory") {
                let path = map_value(&payload, "path")
                    .and_then(|value| value.with_str(str::to_owned))
                    .unwrap_or_else(|| ".".to_owned());
                NativeOperation::ListDirectory { path: path.into() }
            } else if service == sym("text_search") {
                let Some(buffer) =
                    map_value(&payload, "buffer").and_then(|value| value.as_identity())
                else {
                    return native_error("text_search requires an identity buffer");
                };
                let Some(resource) = state.resources.get(&buffer).copied() else {
                    return native_error("text_search buffer is not authorized for this endpoint");
                };
                NativeOperation::Snapshot { resource }
            } else {
                let Some(buffer) =
                    map_value(&payload, "buffer").and_then(|value| value.as_identity())
                else {
                    return native_error("text_insert requires an identity buffer");
                };
                let Some(resource) = state.resources.get(&buffer).copied() else {
                    return native_error("text_insert buffer is not authorized for this endpoint");
                };
                let Some(at) = map_value(&payload, "at")
                    .and_then(|value| value.as_int())
                    .and_then(|value| usize::try_from(value).ok())
                else {
                    return native_error("text_insert requires a non-negative character offset");
                };
                let Some(text) =
                    map_value(&payload, "text").and_then(|value| value.with_str(str::to_owned))
                else {
                    return native_error("text_insert requires string text");
                };
                NativeOperation::Insert { resource, at, text }
            }
        };

        if context.cancellation.is_cancelled() {
            return native_error("request cancelled before native execution");
        }
        match crate::native_io::execute(&self.kernel, operation, context.cancellation.cancelled())
            .await
        {
            Ok(NativeResult::ClockMillis(value)) => native_ok(
                Value::int(i64::try_from(value).unwrap_or(i64::MAX))
                    .unwrap_or_else(|_| Value::string(value.to_string())),
            ),
            Ok(NativeResult::DirectoryEntries { directory, entries }) => native_ok(Value::map([
                (
                    Value::symbol(sym("directory")),
                    Value::string(directory.to_string_lossy()),
                ),
                (
                    Value::symbol(sym("entries")),
                    Value::list(
                        entries
                            .into_iter()
                            .take(MAX_PROMPT_CANDIDATES.saturating_sub(1))
                            .map(|entry| {
                                Value::map([
                                    (Value::symbol(sym("name")), Value::string(entry.name)),
                                    (
                                        Value::symbol(sym("path")),
                                        Value::string(entry.path.to_string_lossy()),
                                    ),
                                    (
                                        Value::symbol(sym("is_directory")),
                                        Value::bool(entry.is_directory),
                                    ),
                                ])
                            }),
                    ),
                ),
            ])),
            Ok(NativeResult::Snapshot(snapshot)) if service == sym("text_search") => {
                let query = map_value(&payload, "query")
                    .and_then(|value| value.with_str(str::to_owned))
                    .unwrap_or_default();
                if query.is_empty() {
                    native_ok(Value::list([]))
                } else {
                    let haystack: Vec<char> = snapshot.text.chars().collect();
                    let needle: Vec<char> = query.chars().collect();
                    let matches = haystack
                        .windows(needle.len())
                        .enumerate()
                        .filter(|(_, candidate)| *candidate == needle.as_slice())
                        .take(1024)
                        .map(|(start, _)| {
                            Value::list([
                                int_value(start),
                                int_value(start.saturating_add(needle.len())),
                            ])
                        });
                    native_ok(Value::list(matches))
                }
            }
            Ok(NativeResult::TextChanged { .. }) => native_ok(Value::symbol(sym("inserted"))),
            Ok(other) => native_error(&format!("unexpected native result: {other:?}")),
            Err(error) => native_error(&error.to_string()),
        }
    }
}

pub struct MicaHost {
    owner: Option<DriverOwner>,
    client: DriverClient,
    administrator: DriverAdministrator,
    event_pump: Option<DriverEventPump>,
    endpoint_session: Option<EndpointSession>,
    bridge: Arc<NativeBridge>,
    endpoint: Identity,
    actor: Identity,
    session: Identity,
    frame: Identity,
    editor_role: Identity,
    global_map: Identity,
    source_repository: Identity,
    source_revision: Identity,
    source_root: PathBuf,
    source_buffers: Arc<RwLock<RoeSourceBuffers>>,
    buffer_ids: HashMap<BufferId, Identity>,
    buffer_metadata: HashMap<BufferId, MicaBufferMetadata>,
    native_ids: HashMap<BufferId, Identity>,
    resource_ids: HashMap<BufferId, ResourceId>,
    view_ids: HashMap<WindowId, Identity>,
    view_buffers: HashMap<WindowId, BufferId>,
    view_cursors: HashMap<WindowId, usize>,
    view_marks: HashMap<WindowId, Option<usize>>,
    layout_nodes: HashMap<Vec<usize>, Identity>,
    layout_tuples: Vec<LayoutFact>,
    disabled_packages: HashSet<Identity>,
    loaded_units: HashSet<Symbol>,
    first_wave_loaded: bool,
    deferred_events: MicaEventBatch,
    active_view: WindowId,
    pending_key_prefix: Option<String>,
    prompt_active: bool,
    closed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MicaBufferMetadata {
    name: String,
    kind: String,
    visited_file: Option<String>,
    file_extension: Option<String>,
    text_revision: u64,
    last_saved_revision: u64,
    modified: bool,
    read_only: bool,
}

impl MicaBufferMetadata {
    fn from_buffer(buffer: &crate::Buffer) -> Self {
        Self {
            name: buffer.display_name(),
            kind: buffer.kind().as_str().to_owned(),
            visited_file: buffer
                .visited_file()
                .map(|path| path.to_string_lossy().into_owned()),
            file_extension: buffer
                .visited_file()
                .and_then(|path| {
                    path.extension()
                        .and_then(|value| value.to_str())
                        .map(str::to_owned)
                })
                .map(|value| value.to_ascii_lowercase()),
            text_revision: buffer.text_revision(),
            last_saved_revision: buffer.last_saved_revision(),
            modified: buffer.is_modified(),
            read_only: buffer.is_read_only(),
        }
    }
}

impl MicaHost {
    pub fn set_wake_handler(&mut self, handler: Arc<dyn FrontendWake>) {
        if let Some(pump) = self.event_pump.as_mut() {
            pump.set_wake_handler(Arc::new(MicaFrontendWake(handler)));
        }
    }

    fn endpoint_session(&self) -> &EndpointSession {
        self.endpoint_session
            .as_ref()
            .expect("open Mica host retains its endpoint session")
    }

    fn format_value(&self, value: &Value) -> String {
        self.client.format_value(value)
    }

    pub fn recovery_diagnostics(&self) -> String {
        format!(
            "endpoint={} actor={} session={} first_wave_loaded={} buffers={} views={} disabled_packages={} closed={}",
            self.format_value(&Value::identity(self.endpoint)),
            self.format_value(&Value::identity(self.actor)),
            self.format_value(&Value::identity(self.session)),
            self.first_wave_loaded,
            self.buffer_ids.len(),
            self.view_ids.len(),
            self.disabled_packages.len(),
            self.closed
        )
    }

    pub fn open(
        editor: &Editor,
        kernel: Arc<Mutex<NativeKernel>>,
        resource_ids: &HashMap<BufferId, ResourceId>,
    ) -> Result<Self, MicaHostError> {
        Self::open_with_stream_handler(editor, kernel, resource_ids, agent_stream_handler())
    }

    #[cfg(test)]
    pub fn open_with_stream_handler_for_test(
        editor: &Editor,
        kernel: Arc<Mutex<NativeKernel>>,
        resource_ids: &HashMap<BufferId, ResourceId>,
        stream_handler: ExternalStreamRequestHandler,
    ) -> Result<Self, MicaHostError> {
        Self::open_with_stream_handler(editor, kernel, resource_ids, stream_handler)
    }

    fn open_with_stream_handler(
        editor: &Editor,
        kernel: Arc<Mutex<NativeKernel>>,
        resource_ids: &HashMap<BufferId, ResourceId>,
        stream_handler: ExternalStreamRequestHandler,
    ) -> Result<Self, MicaHostError> {
        let source_root = std::env::current_dir()
            .and_then(|path| path.canonicalize())
            .map_err(|error| MicaHostError::SourceConfiguration(error.to_string()))?;
        let source_buffers = Arc::new(RwLock::new(RoeSourceBuffers::default()));
        synchronize_source_buffers(&source_root, &source_buffers, editor);
        let source_config = SourceConfig::new([source_root.clone()]).with_shared_provider(
            Arc::new(RoeBufferSourceProvider {
                root: source_root.clone(),
                buffers: Arc::clone(&source_buffers),
            }),
        );
        let bridge = Arc::new(NativeBridge::new(kernel));
        let handler_bridge = Arc::clone(&bridge);
        let http_handler = mica_external_http::handler();
        let external_handler: ExternalRequestHandler = Arc::new(move |context, request| {
            if matches!(
                request.service.name(),
                Some("http" | "openai" | "openai_responses" | "embedding")
            ) {
                return http_handler(context, request);
            }
            let bridge = Arc::clone(&handler_bridge);
            Box::pin(async move {
                if request.service == sym("test_pending") {
                    context.cancellation.cancelled().await;
                    return native_error("request cancelled");
                }
                bridge
                    .handle(context, request.service, request.payload)
                    .await
            }) as ExternalRequestFuture
        });

        let mut resources = DriverResources::new(NonZeroUsize::new(2).unwrap());
        resources.relation_parallelism = NonZeroUsize::new(1).unwrap();
        resources.task_limits = TaskLimits {
            instruction_budget: 250_000,
            max_retries: 4,
            max_call_depth: 32,
        };
        resources.event_queue_capacity = NonZeroUsize::new(EVENT_QUEUE_CAPACITY).unwrap();
        resources.external_request_capacity = NonZeroUsize::new(EXTERNAL_REQUEST_CAPACITY).unwrap();
        resources.subscription_queue_budget = NonZeroUsize::new(SUBSCRIPTION_QUEUE_BUDGET).unwrap();
        resources.active_task_capacity = NonZeroUsize::new(ACTIVE_TASK_CAPACITY).unwrap();
        resources.suspended_task_capacity = NonZeroUsize::new(SUSPENDED_TASK_CAPACITY).unwrap();
        resources.timer_capacity = NonZeroUsize::new(TIMER_CAPACITY).unwrap();
        resources.terminal_task_retention = NonZeroUsize::new(TERMINAL_TASK_RETENTION).unwrap();
        resources.relation_acceleration = RelationAcceleration::Disabled;

        let mut owner = mica_driver::DriverOwner::builder(resources)
            .source_config(source_config)
            .initial_filein_unit(sym("roe/core"), CORE_SOURCE, FileinMode::Add, None)
            .external_request_handler(external_handler)
            .external_stream_request_handler(stream_handler)
            .build()?;
        let event_pump = owner.take_event_pump()?;
        let client = owner.client();
        let administrator = owner.administrator();

        let endpoint = client.allocate_ephemeral_identity()?;
        let actor = client.allocate_ephemeral_identity()?;
        let session = client.allocate_ephemeral_identity()?;
        let frame = client.allocate_ephemeral_identity()?;
        let editor_role = client.named_identity(sym("roe/editor_role"))?;
        let global_map = client.named_identity(sym("roe/global_map"))?;
        let source_repository = client.named_identity(sym("roe/source_repository"))?;
        let source_revision = client.named_identity(sym("roe/source_worktree"))?;

        let mut buffer_ids = HashMap::new();
        let mut buffer_metadata = HashMap::new();
        let mut native_ids = HashMap::new();
        let mut bridge_resources = HashMap::new();
        for (buffer_id, _buffer) in &editor.buffers {
            let buffer = client.allocate_ephemeral_identity()?;
            let native = client.allocate_ephemeral_identity()?;
            let resource = *resource_ids
                .get(&buffer_id)
                .ok_or(MicaHostError::MissingIdentity)?;
            buffer_ids.insert(buffer_id, buffer);
            buffer_metadata.insert(buffer_id, MicaBufferMetadata::from_buffer(_buffer));
            native_ids.insert(buffer_id, native);
            bridge_resources.insert(buffer, resource);
        }

        let mut view_ids = HashMap::new();
        let mut view_buffers = HashMap::new();
        let mut view_cursors = HashMap::new();
        let mut view_marks = HashMap::new();
        for (window_id, window) in &editor.windows {
            view_ids.insert(window_id, client.allocate_ephemeral_identity()?);
            view_buffers.insert(window_id, window.active_buffer);
            view_cursors.insert(window_id, window.cursor);
            view_marks.insert(window_id, editor.buffers[window.active_buffer].get_mark());
        }
        let active_view = editor.active_window;
        let active_view_identity = *view_ids
            .get(&active_view)
            .ok_or(MicaHostError::MissingIdentity)?;

        let mut tuples = vec![
            (sym("roe/EditorSession"), [Value::identity(session)].into()),
            (
                sym("roe/SessionActor"),
                [Value::identity(session), Value::identity(actor)].into(),
            ),
            (
                sym("roe/SessionEndpoint"),
                [Value::identity(session), Value::identity(endpoint)].into(),
            ),
            (sym("roe/Frame"), [Value::identity(frame)].into()),
            (
                sym("roe/SessionFrame"),
                [Value::identity(session), Value::identity(frame)].into(),
            ),
            (
                sym("roe/ActorRole"),
                [Value::identity(actor), Value::identity(editor_role)].into(),
            ),
            (
                sym("roe/SessionKeymap"),
                [
                    Value::identity(session),
                    Value::identity(global_map),
                    int_value(100),
                ]
                .into(),
            ),
            (
                sym("roe/ActiveView"),
                [
                    Value::identity(session),
                    Value::identity(active_view_identity),
                ]
                .into(),
            ),
        ];
        tuples.extend(source_context_facts(
            source_repository,
            source_revision,
            &source_root,
        ));
        for (buffer_id, _buffer) in &editor.buffers {
            let logical = buffer_ids[&buffer_id];
            let native = native_ids[&buffer_id];
            let resource = resource_ids[&buffer_id];
            tuples.push((sym("roe/LogicalBuffer"), [Value::identity(logical)].into()));
            let metadata = &buffer_metadata[&buffer_id];
            tuples.push((
                sym("roe/BufferName"),
                [Value::identity(logical), Value::string(&metadata.name)].into(),
            ));
            tuples.push((
                sym("roe/BufferKind"),
                [Value::identity(logical), Value::symbol(sym(&metadata.kind))].into(),
            ));
            if let Some(path) = &metadata.visited_file {
                tuples.push((
                    sym("roe/BufferVisitedFile"),
                    [Value::identity(logical), Value::string(path)].into(),
                ));
                if let Some(relative) = source_relative_path(&source_root, Path::new(path)) {
                    tuples.push((
                        sym("roe/BufferSourcePath"),
                        [Value::identity(logical), Value::string(relative)].into(),
                    ));
                }
            }
            if let Some(extension) = &metadata.file_extension {
                tuples.push((
                    sym("roe/BufferFileExtension"),
                    [Value::identity(logical), Value::string(extension)].into(),
                ));
            }
            tuples.extend([
                (
                    sym("roe/NativeBufferRevision"),
                    [Value::identity(logical), int_value(metadata.text_revision)].into(),
                ),
                (
                    sym("roe/BufferLastSavedRevision"),
                    [
                        Value::identity(logical),
                        int_value(metadata.last_saved_revision),
                    ]
                    .into(),
                ),
                (
                    sym("roe/BufferModified"),
                    [Value::identity(logical), Value::bool(metadata.modified)].into(),
                ),
                (
                    sym("roe/BufferReadOnly"),
                    [Value::identity(logical), Value::bool(metadata.read_only)].into(),
                ),
            ]);
            tuples.push((
                sym("roe/NativeTextResource"),
                [Value::identity(logical), Value::identity(native)].into(),
            ));
            tuples.push((
                sym("roe/NativeResourceGeneration"),
                [Value::identity(native), int_value(resource.generation)].into(),
            ));
            tuples.push((
                sym("roe/CanUseBuffer"),
                [Value::identity(actor), Value::identity(logical)].into(),
            ));
        }
        for (window_id, window) in &editor.windows {
            let view = view_ids[&window_id];
            let buffer = buffer_ids[&window.active_buffer];
            tuples.push((sym("roe/View"), [Value::identity(view)].into()));
            tuples.push((
                sym("roe/ViewBuffer"),
                [Value::identity(view), Value::identity(buffer)].into(),
            ));
            tuples.push((
                sym("roe/ViewCursor"),
                [Value::identity(view), int_value(window.cursor)].into(),
            ));
            if let Some(mark) = editor.buffers[window.active_buffer].get_mark() {
                tuples.push((
                    sym("roe/ViewMark"),
                    [Value::identity(view), int_value(mark)].into(),
                ));
                tuples.push((
                    sym("roe/ViewSelection"),
                    [
                        Value::identity(view),
                        int_value(mark),
                        int_value(window.cursor),
                    ]
                    .into(),
                ));
            }
        }
        let mut layout_nodes = HashMap::new();
        let layout_tuples = build_layout_tuples(
            &client,
            frame,
            &editor.window_tree,
            &view_ids,
            &mut layout_nodes,
        )?;
        tuples.extend(layout_named_tuples!(&layout_tuples));
        let endpoint_session = client.open_endpoint(
            EndpointConfiguration::new(sym("roe/session-v1"))
                .endpoint(endpoint)
                .actor(actor)
                .volatile_facts(tuples),
        )?;
        bridge.configure(
            actor,
            bridge_resources,
            [
                sym("clock_read"),
                sym("text_read"),
                sym("text_write"),
                sym("file_read"),
            ]
            .into(),
        );

        Ok(Self {
            owner: Some(owner),
            client,
            administrator,
            event_pump: Some(event_pump),
            endpoint_session: Some(endpoint_session),
            bridge,
            endpoint,
            actor,
            session,
            frame,
            editor_role,
            global_map,
            source_repository,
            source_revision,
            source_root,
            source_buffers,
            buffer_ids,
            buffer_metadata,
            native_ids,
            resource_ids: resource_ids.clone(),
            view_ids,
            view_buffers,
            view_cursors,
            view_marks,
            layout_nodes,
            layout_tuples,
            disabled_packages: HashSet::new(),
            loaded_units: [sym("roe/core")].into_iter().collect(),
            first_wave_loaded: false,
            deferred_events: MicaEventBatch::default(),
            active_view,
            pending_key_prefix: None,
            prompt_active: false,
            closed: false,
        })
    }

    pub async fn dispatch_key(
        &mut self,
        editor: &Editor,
        resource_ids: &HashMap<BufferId, ResourceId>,
        sequence: String,
    ) -> Result<MicaDispatchResult, MicaHostError> {
        if self.closed {
            return Err(MicaHostError::Closed);
        }
        self.ensure_first_wave().await?;
        self.synchronize_context(editor, resource_ids)?;
        let had_prefix = self.pending_key_prefix.is_some();
        let sequence = self
            .pending_key_prefix
            .as_ref()
            .map_or(sequence.clone(), |prefix| format!("{prefix} {sequence}"));
        let selector = if self.prompt_active {
            sym("roe/prompt_key")
        } else {
            sym("roe/dispatch_key")
        };
        let invocation = self
            .endpoint_session()
            .invoke(
                selector,
                vec![
                    (sym("actor"), Value::identity(self.actor)),
                    (sym("session"), Value::identity(self.session)),
                    (sym("sequence"), Value::string(&sequence)),
                ],
            )
            .await?;
        let selector_name = selector.name().unwrap_or("<unnamed-selector>");
        let mut result = self.wait_for_task(&invocation, selector_name).await?;
        if had_prefix && result.key == MicaKeyResult::Unbound {
            result.key = MicaKeyResult::Failed(format!("{sequence} is undefined"));
        }
        if result.key == MicaKeyResult::Prefix {
            self.pending_key_prefix = Some(sequence);
        } else {
            self.pending_key_prefix = None;
        }
        Ok(result)
    }

    pub async fn dispatch_pointer(
        &mut self,
        editor: &Editor,
        resource_ids: &HashMap<BufferId, ResourceId>,
        view: WindowId,
        position: usize,
        phase: &str,
        button: &str,
    ) -> Result<MicaEventBatch, MicaHostError> {
        self.synchronize_context(editor, resource_ids)?;
        let view = *self
            .view_ids
            .get(&view)
            .ok_or(MicaHostError::MissingIdentity)?;
        self.invoke_editor_verb(
            "roe/pointer_event",
            vec![
                (sym("view"), Value::identity(view)),
                (sym("position"), int_value(position)),
                (sym("phase"), Value::symbol(sym(phase))),
                (sym("button"), Value::symbol(sym(button))),
            ],
        )
        .await
    }

    pub async fn set_view_scroll(
        &mut self,
        editor: &Editor,
        resource_ids: &HashMap<BufferId, ResourceId>,
        view: WindowId,
        line: usize,
        column: usize,
    ) -> Result<MicaEventBatch, MicaHostError> {
        self.synchronize_context(editor, resource_ids)?;
        let view = *self
            .view_ids
            .get(&view)
            .ok_or(MicaHostError::MissingIdentity)?;
        self.invoke_editor_verb(
            "roe/set_view_scroll",
            vec![
                (sym("view"), Value::identity(view)),
                (sym("line"), int_value(line)),
                (sym("column"), int_value(column)),
            ],
        )
        .await
    }

    pub async fn set_split_ratio(
        &mut self,
        editor: &Editor,
        resource_ids: &HashMap<BufferId, ResourceId>,
        path: &[usize],
        ratio: f32,
    ) -> Result<MicaEventBatch, MicaHostError> {
        self.synchronize_context(editor, resource_ids)?;
        let node = *self
            .layout_nodes
            .get(path)
            .ok_or(MicaHostError::MissingIdentity)?;
        let ratio = Value::float(ratio).map_err(|_| MicaHostError::MissingIdentity)?;
        self.invoke_editor_verb(
            "roe/set_split_ratio",
            vec![(sym("node"), Value::identity(node)), (sym("ratio"), ratio)],
        )
        .await
    }

    pub async fn present_evaluation_result(
        &mut self,
        editor: &Editor,
        resource_ids: &HashMap<BufferId, ResourceId>,
        view: WindowId,
        buffer: BufferId,
        text: String,
        failed: bool,
    ) -> Result<MicaEventBatch, MicaHostError> {
        self.ensure_first_wave().await?;
        self.synchronize_context(editor, resource_ids)?;
        let view = *self
            .view_ids
            .get(&view)
            .ok_or(MicaHostError::MissingIdentity)?;
        let buffer = *self
            .buffer_ids
            .get(&buffer)
            .ok_or(MicaHostError::MissingIdentity)?;
        self.invoke_editor_verb(
            "roe/present_evaluation_result",
            vec![
                (sym("view"), Value::identity(view)),
                (sym("buffer"), Value::identity(buffer)),
                (sym("text"), Value::string(&text)),
                (sym("failed"), Value::bool(failed)),
            ],
        )
        .await
    }

    pub async fn settle_typeout_closed(
        &mut self,
        editor: &Editor,
        resource_ids: &HashMap<BufferId, ResourceId>,
    ) -> Result<MicaEventBatch, MicaHostError> {
        self.synchronize_context(editor, resource_ids)?;
        self.invoke_editor_verb("roe/typeout_closed", Vec::new())
            .await
    }

    pub async fn start_agent(
        &mut self,
        editor: &Editor,
        resource_ids: &HashMap<BufferId, ResourceId>,
    ) -> Result<MicaEventBatch, MicaHostError> {
        self.ensure_first_wave().await?;
        self.synchronize_context(editor, resource_ids)?;
        self.invoke_editor_verb("roe/agent_start", Vec::new()).await
    }

    pub(crate) async fn initialize_startup(
        &mut self,
        editor: &Editor,
        resource_ids: &HashMap<BufferId, ResourceId>,
        buffers: &[BufferId],
    ) -> Result<MicaEventBatch, MicaHostError> {
        if buffers.len() > crate::session::MAX_SESSION_VIEWS {
            return Err(MicaHostError::Policy(
                "startup buffer limit exceeded".into(),
            ));
        }
        self.ensure_first_wave().await?;
        self.synchronize_context(editor, resource_ids)?;
        let buffers = buffers
            .iter()
            .map(|id| {
                self.buffer_ids
                    .get(id)
                    .copied()
                    .map(Value::identity)
                    .ok_or_else(|| MicaHostError::Policy("startup buffer no longer exists".into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.invoke_editor_verb(
            "roe/initialize_startup",
            vec![(sym("buffers"), Value::list(buffers))],
        )
        .await
    }

    async fn invoke_editor_verb(
        &mut self,
        selector: &str,
        mut arguments: Vec<(Symbol, Value)>,
    ) -> Result<MicaEventBatch, MicaHostError> {
        if self.closed {
            return Err(MicaHostError::Closed);
        }
        arguments.push((sym("actor"), Value::identity(self.actor)));
        arguments.push((sym("session"), Value::identity(self.session)));
        let invocation = self
            .endpoint_session()
            .invoke(sym(selector), arguments)
            .await?;
        let result = self.wait_for_task(&invocation, selector).await?;
        match result.key {
            MicaKeyResult::Failed(message) => Err(MicaHostError::Policy(message)),
            _ => Ok(result.events),
        }
    }

    pub fn drain_background_events(&mut self) -> MicaEventBatch {
        let mut batch = std::mem::take(&mut self.deferred_events);
        let events = self
            .event_pump
            .as_mut()
            .map(DriverEventPump::drain)
            .unwrap_or_default();
        for event in events {
            self.record_background_event(event, &mut batch);
        }
        batch
    }

    pub async fn publish_policy(
        &mut self,
        editor: &Editor,
        resource_ids: &HashMap<BufferId, ResourceId>,
    ) -> Result<MicaEventBatch, MicaHostError> {
        if self.closed {
            return Err(MicaHostError::Closed);
        }
        self.ensure_first_wave().await?;
        self.synchronize_context(editor, resource_ids)?;
        self.invoke_editor_verb("roe/publish_policy", Vec::new())
            .await
    }

    pub async fn check_source(&self, source: String) -> Result<(), MicaHostError> {
        self.administrator.check_filein(source, None).await?;
        Ok(())
    }

    pub async fn replace_unit(&mut self, unit: &str, source: String) -> Result<(), MicaHostError> {
        let unit_symbol = sym(unit);
        let previous = if self.loaded_units.contains(&unit_symbol) {
            Some(self.administrator.fileout_unit(unit_symbol).await?)
        } else {
            None
        };
        let mode = if self.loaded_units.contains(&unit_symbol) {
            // Replace validates in a staged kernel after retracting the old unit.
            // An additive check_filein would reject legitimate functional-key changes
            // against the still-installed unit. The staged replacement commits only
            // after every declaration completes; failure retains the working unit.
            FileinMode::Replace
        } else {
            self.administrator
                .check_filein(source.clone(), None)
                .await?;
            FileinMode::Add
        };
        self.administrator
            .filein_unit(unit_symbol, source, mode, None)
            .await?;
        if let Err(error) = self.validate_syntax_policy().await {
            self.administrator
                .filein_unit(
                    unit_symbol,
                    previous.unwrap_or_default(),
                    FileinMode::Replace,
                    None,
                )
                .await?;
            // An empty rollback unit still exists in the driver. Track it so
            // a later retry uses Replace rather than an additive file-in.
            self.loaded_units.insert(unit_symbol);
            return Err(error);
        }
        self.loaded_units.insert(unit_symbol);
        if unit == "roe/first-wave" {
            self.first_wave_loaded = true;
        }
        Ok(())
    }

    pub async fn export_unit(&mut self, unit: &str) -> Result<String, MicaHostError> {
        if unit == "roe/first-wave" || unit == "roe/rust" {
            self.ensure_first_wave().await?;
        }
        Ok(self.administrator.fileout_unit(sym(unit)).await?)
    }

    pub async fn restore_first_wave(&mut self) -> Result<(), MicaHostError> {
        self.replace_unit("roe/first-wave", FIRST_WAVE_SOURCE.to_owned())
            .await?;
        self.replace_unit("roe/agent", AGENT_SOURCE.to_owned())
            .await?;
        self.replace_unit("roe/rust", RUST_SOURCE.to_owned()).await
    }

    async fn ensure_first_wave(&mut self) -> Result<(), MicaHostError> {
        if !self.first_wave_loaded {
            self.administrator
                .check_filein(FIRST_WAVE_SOURCE.to_owned(), None)
                .await?;
            self.administrator
                .filein_unit(
                    sym("roe/first-wave"),
                    FIRST_WAVE_SOURCE.to_owned(),
                    FileinMode::Add,
                    None,
                )
                .await?;
            self.loaded_units.insert(sym("roe/first-wave"));
            self.first_wave_loaded = true;
        }
        if !self.loaded_units.contains(&sym("roe/rust")) {
            self.administrator
                .check_filein(RUST_SOURCE.to_owned(), None)
                .await?;
            self.administrator
                .filein_unit(
                    sym("roe/rust"),
                    RUST_SOURCE.to_owned(),
                    FileinMode::Add,
                    None,
                )
                .await?;
            self.loaded_units.insert(sym("roe/rust"));
        }
        if !self.loaded_units.contains(&sym("roe/agent")) {
            self.administrator
                .check_filein(AGENT_SOURCE.to_owned(), None)
                .await?;
            self.administrator
                .filein_unit(
                    sym("roe/agent"),
                    AGENT_SOURCE.to_owned(),
                    FileinMode::Add,
                    None,
                )
                .await?;
            self.loaded_units.insert(sym("roe/agent"));
        }
        Ok(())
    }

    pub async fn evaluate_source(
        &mut self,
        editor: &Editor,
        resource_ids: &HashMap<BufferId, ResourceId>,
        source: String,
    ) -> Result<MicaEvaluationResult, MicaHostError> {
        if self.closed {
            return Err(MicaHostError::Closed);
        }
        self.ensure_first_wave().await?;
        self.synchronize_context(editor, resource_ids)?;
        let invocation = self.endpoint_session().evaluate(source).await?;
        let mut events = std::mem::take(&mut self.deferred_events);
        let mut pump = self
            .event_pump
            .take()
            .expect("open Mica host retains its event pump");
        let outcome = pump
            .drive_invocation(&invocation, |event| {
                self.record_background_event(event, &mut events);
            })
            .await;
        self.event_pump = Some(pump);
        match outcome {
            InvocationOutcome::Completed(value) => Ok(MicaEvaluationResult {
                value: self.format_value(&value),
                events,
            }),
            InvocationOutcome::Aborted(error) => Err(MicaHostError::Policy(format!(
                "evaluation aborted: {}",
                self.format_value(&error)
            ))),
            InvocationOutcome::Failed(error) => {
                Err(MicaHostError::Policy(format!("evaluation failed: {error}")))
            }
            InvocationOutcome::Cancelled(reason) => Err(MicaHostError::Policy(format!(
                "evaluation cancelled: {reason:?}"
            ))),
        }
    }

    pub fn set_package_enabled(
        &mut self,
        package: &str,
        enabled: bool,
    ) -> Result<(), MicaHostError> {
        let package = self.client.named_identity(sym(package))?;
        if enabled {
            self.disabled_packages.remove(&package);
        } else {
            self.disabled_packages.insert(package);
        }
        let facts = self
            .disabled_packages
            .iter()
            .map(|package| {
                (
                    sym("roe/PackageDisabled"),
                    [Value::identity(*package)].into(),
                )
            })
            .collect();
        self.endpoint_session
            .as_mut()
            .expect("open Mica host retains its endpoint session")
            .replace_volatile_scope(sym("roe/packages"), facts)?;
        Ok(())
    }

    #[cfg(test)]
    pub fn revoke_service_for_test(&self, service: &str) {
        self.bridge.revoke_service(sym(service));
    }

    #[cfg(test)]
    pub fn identity_counts_for_test(&self) -> (usize, usize) {
        (self.buffer_ids.len(), self.view_ids.len())
    }

    #[cfg(test)]
    pub async fn start_pending_test_request(&mut self) -> Result<TaskId, MicaHostError> {
        const SOURCE: &str = r#"
assert RoleCanInvoke(#roe/editor_role, :roe/test_pending)
verb roe/test_pending(actor, session)
  roe/SessionActor(session, actor) || return :not_session_actor
  return external_request(:test_pending, (), 60)
end
"#;
        self.administrator
            .filein_unit(
                sym("roe/test-pending"),
                SOURCE.to_owned(),
                FileinMode::Add,
                None,
            )
            .await?;
        let invocation = self
            .endpoint_session()
            .invoke(
                sym("roe/test_pending"),
                vec![
                    (sym("actor"), Value::identity(self.actor)),
                    (sym("session"), Value::identity(self.session)),
                ],
            )
            .await?;
        let task_id = invocation.detach()?;
        self.drain_background_events();
        Ok(task_id)
    }

    #[cfg(test)]
    pub async fn start_background_test_task(&mut self) -> Result<TaskId, MicaHostError> {
        const SOURCE: &str = r#"
assert RoleCanInvoke(#roe/editor_role, :roe/test_background)
verb roe/test_background(actor, session)
  roe/SessionActor(session, actor) || return :not_session_actor
  let exactly {:view -> view} = roe/ActiveView(session, ?view)
  let exactly {:buffer -> buffer} = roe/ViewBuffer(view, ?buffer)
  let exactly {:cursor -> cursor} = roe/ViewCursor(view, ?cursor)
  suspend(0.02)
  emit(session, {:kind -> :presentation_invalidated, :view -> view, :buffer -> buffer, :cursor -> cursor})
  return :done
end
"#;
        self.administrator
            .filein_unit(
                sym("roe/test-background"),
                SOURCE.to_owned(),
                FileinMode::Add,
                None,
            )
            .await?;
        let invocation = self
            .endpoint_session()
            .invoke(
                sym("roe/test_background"),
                vec![
                    (sym("actor"), Value::identity(self.actor)),
                    (sym("session"), Value::identity(self.session)),
                ],
            )
            .await?;
        Ok(invocation.detach()?)
    }

    fn synchronize_context(
        &mut self,
        editor: &Editor,
        resource_ids: &HashMap<BufferId, ResourceId>,
    ) -> Result<(), MicaHostError> {
        let live_buffers: HashSet<_> = editor
            .buffers
            .keys()
            .filter(|buffer| !editor.is_command_buffer(*buffer))
            .collect();
        let live_views: HashSet<_> = editor
            .windows
            .iter()
            .filter_map(|(view, window)| {
                (!matches!(
                    window.window_type,
                    crate::editor::WindowType::Command { .. }
                ))
                .then_some(view)
            })
            .collect();
        let stale_buffers: Vec<_> = self
            .buffer_ids
            .keys()
            .copied()
            .filter(|buffer| !live_buffers.contains(buffer))
            .collect();
        let stale_views: Vec<_> = self
            .view_ids
            .keys()
            .copied()
            .filter(|view| !live_views.contains(view))
            .collect();
        for window_id in stale_views {
            self.view_ids.remove(&window_id);
            self.view_buffers.remove(&window_id);
            self.view_cursors.remove(&window_id);
            self.view_marks.remove(&window_id);
        }
        for buffer_id in stale_buffers {
            let logical = self.buffer_ids.remove(&buffer_id).unwrap();
            self.bridge.remove_resource(logical);
            self.buffer_metadata.remove(&buffer_id);
            self.native_ids.remove(&buffer_id);
            self.resource_ids.remove(&buffer_id);
        }

        let active = if matches!(
            editor.windows[editor.active_window].window_type,
            crate::editor::WindowType::Command { .. }
        ) {
            editor
                .previous_active_window
                .filter(|view| live_views.contains(view))
                .ok_or(MicaHostError::MissingIdentity)?
        } else {
            editor.active_window
        };
        let window = editor
            .windows
            .get(active)
            .ok_or(MicaHostError::MissingIdentity)?;
        for buffer_id in &live_buffers {
            if self.buffer_ids.contains_key(buffer_id) {
                continue;
            }
            let logical = self.client.allocate_ephemeral_identity()?;
            let native = self.client.allocate_ephemeral_identity()?;
            let resource = *resource_ids
                .get(buffer_id)
                .ok_or(MicaHostError::MissingIdentity)?;
            let buffer = editor
                .buffers
                .get(*buffer_id)
                .ok_or(MicaHostError::MissingIdentity)?;
            self.buffer_ids.insert(*buffer_id, logical);
            self.buffer_metadata
                .insert(*buffer_id, MicaBufferMetadata::from_buffer(buffer));
            self.native_ids.insert(*buffer_id, native);
            self.resource_ids.insert(*buffer_id, resource);
            self.bridge.add_resource(logical, resource);
        }

        for (window_id, candidate) in &editor.windows {
            if matches!(
                candidate.window_type,
                crate::editor::WindowType::Command { .. }
            ) {
                continue;
            }
            if self.view_ids.contains_key(&window_id) {
                continue;
            }
            if !self.buffer_ids.contains_key(&candidate.active_buffer) {
                continue;
            }
            let logical_view = self.client.allocate_ephemeral_identity()?;
            self.view_ids.insert(window_id, logical_view);
            self.view_buffers.insert(window_id, candidate.active_buffer);
            self.view_cursors.insert(window_id, candidate.cursor);
            self.view_marks.insert(
                window_id,
                editor.buffers[candidate.active_buffer].get_mark(),
            );
        }

        for buffer_id in &live_buffers {
            if let Some(buffer) = editor.buffers.get(*buffer_id) {
                self.buffer_metadata
                    .insert(*buffer_id, MicaBufferMetadata::from_buffer(buffer));
            }
        }

        for (window_id, candidate) in &editor.windows {
            if matches!(
                candidate.window_type,
                crate::editor::WindowType::Command { .. }
            ) {
                continue;
            }
            if !self.view_ids.contains_key(&window_id) {
                continue;
            }
            if !self.buffer_ids.contains_key(&candidate.active_buffer) {
                continue;
            }
            if self.view_buffers.get(&window_id).copied() != Some(candidate.active_buffer) {
                self.view_buffers.insert(window_id, candidate.active_buffer);
            }
            if self.view_cursors.get(&window_id).copied() != Some(candidate.cursor) {
                self.view_cursors.insert(window_id, candidate.cursor);
            }
            self.view_marks.insert(
                window_id,
                editor.buffers[candidate.active_buffer].get_mark(),
            );
        }

        if !self.view_ids.contains_key(&active) {
            let view = self.client.allocate_ephemeral_identity()?;
            self.view_ids.insert(active, view);
            self.view_buffers.insert(active, window.active_buffer);
            self.view_cursors.insert(active, window.cursor);
            self.view_marks
                .insert(active, editor.buffers[window.active_buffer].get_mark());
        }
        self.view_buffers.insert(active, window.active_buffer);
        self.view_cursors.insert(active, window.cursor);
        self.view_marks
            .insert(active, editor.buffers[window.active_buffer].get_mark());
        self.active_view = active;
        self.synchronize_layout(editor)?;
        synchronize_source_buffers(&self.source_root, &self.source_buffers, editor);
        let facts = self.volatile_context_facts();
        self.endpoint_session
            .as_mut()
            .expect("open Mica host retains its endpoint session")
            .replace_volatile_scope(sym("endpoint"), facts)?;
        Ok(())
    }

    fn synchronize_layout(&mut self, editor: &Editor) -> Result<(), MicaHostError> {
        self.layout_tuples = build_layout_tuples(
            &self.client,
            self.frame,
            &editor.window_tree,
            &self.view_ids,
            &mut self.layout_nodes,
        )?;
        Ok(())
    }

    fn volatile_context_facts(&self) -> Vec<(Symbol, mica_driver::Tuple)> {
        let mut facts = vec![
            (
                sym("roe/EditorSession"),
                [Value::identity(self.session)].into(),
            ),
            (
                sym("roe/SessionActor"),
                [Value::identity(self.session), Value::identity(self.actor)].into(),
            ),
            (
                sym("roe/SessionEndpoint"),
                [
                    Value::identity(self.session),
                    Value::identity(self.endpoint),
                ]
                .into(),
            ),
            (sym("roe/Frame"), [Value::identity(self.frame)].into()),
            (
                sym("roe/SessionFrame"),
                [Value::identity(self.session), Value::identity(self.frame)].into(),
            ),
        ];
        facts.extend([
            (
                sym("roe/ActorRole"),
                [
                    Value::identity(self.actor),
                    Value::identity(self.editor_role),
                ]
                .into(),
            ),
            (
                sym("roe/SessionKeymap"),
                [
                    Value::identity(self.session),
                    Value::identity(self.global_map),
                    int_value(100),
                ]
                .into(),
            ),
        ]);
        facts.extend(source_context_facts(
            self.source_repository,
            self.source_revision,
            &self.source_root,
        ));
        if let Some(active) = self.view_ids.get(&self.active_view).copied() {
            facts.push((
                sym("roe/ActiveView"),
                [Value::identity(self.session), Value::identity(active)].into(),
            ));
        }
        for (buffer_id, logical) in &self.buffer_ids {
            let native = self.native_ids[buffer_id];
            let resource = self.resource_ids[buffer_id];
            let metadata = &self.buffer_metadata[buffer_id];
            facts.extend([
                (sym("roe/LogicalBuffer"), [Value::identity(*logical)].into()),
                (
                    sym("roe/BufferName"),
                    [Value::identity(*logical), Value::string(&metadata.name)].into(),
                ),
                (
                    sym("roe/BufferKind"),
                    [
                        Value::identity(*logical),
                        Value::symbol(sym(&metadata.kind)),
                    ]
                    .into(),
                ),
                (
                    sym("roe/NativeBufferRevision"),
                    [Value::identity(*logical), int_value(metadata.text_revision)].into(),
                ),
                (
                    sym("roe/BufferLastSavedRevision"),
                    [
                        Value::identity(*logical),
                        int_value(metadata.last_saved_revision),
                    ]
                    .into(),
                ),
                (
                    sym("roe/BufferModified"),
                    [Value::identity(*logical), Value::bool(metadata.modified)].into(),
                ),
                (
                    sym("roe/BufferReadOnly"),
                    [Value::identity(*logical), Value::bool(metadata.read_only)].into(),
                ),
                (
                    sym("roe/NativeTextResource"),
                    [Value::identity(*logical), Value::identity(native)].into(),
                ),
                (
                    sym("roe/NativeResourceGeneration"),
                    [Value::identity(native), int_value(resource.generation)].into(),
                ),
                (
                    sym("roe/CanUseBuffer"),
                    [Value::identity(self.actor), Value::identity(*logical)].into(),
                ),
            ]);
            if let Some(path) = &metadata.visited_file {
                facts.push((
                    sym("roe/BufferVisitedFile"),
                    [Value::identity(*logical), Value::string(path)].into(),
                ));
                if let Some(relative) = source_relative_path(&self.source_root, Path::new(path)) {
                    facts.push((
                        sym("roe/BufferSourcePath"),
                        [Value::identity(*logical), Value::string(relative)].into(),
                    ));
                }
            }
            if let Some(extension) = &metadata.file_extension {
                facts.push((
                    sym("roe/BufferFileExtension"),
                    [Value::identity(*logical), Value::string(extension)].into(),
                ));
            }
        }
        for (window_id, view) in &self.view_ids {
            let Some(buffer_id) = self.view_buffers.get(window_id) else {
                continue;
            };
            let Some(cursor) = self.view_cursors.get(window_id) else {
                continue;
            };
            facts.extend([
                (sym("roe/View"), [Value::identity(*view)].into()),
                (
                    sym("roe/ViewBuffer"),
                    [
                        Value::identity(*view),
                        Value::identity(self.buffer_ids[buffer_id]),
                    ]
                    .into(),
                ),
                (
                    sym("roe/ViewCursor"),
                    [Value::identity(*view), int_value(*cursor)].into(),
                ),
            ]);
            if let Some(mark) = self.view_marks.get(window_id).copied().flatten() {
                facts.extend([
                    (
                        sym("roe/ViewMark"),
                        [Value::identity(*view), int_value(mark)].into(),
                    ),
                    (
                        sym("roe/ViewSelection"),
                        [Value::identity(*view), int_value(mark), int_value(*cursor)].into(),
                    ),
                ]);
            }
        }
        facts.extend(layout_named_tuples!(&self.layout_tuples));
        facts
    }

    async fn wait_for_task(
        &mut self,
        invocation: &InvocationHandle,
        selector: &str,
    ) -> Result<MicaDispatchResult, MicaHostError> {
        let mut batch = std::mem::take(&mut self.deferred_events);
        let mut pump = self
            .event_pump
            .take()
            .expect("open Mica host retains its event pump");
        let outcome = pump
            .drive_invocation(invocation, |event| {
                self.record_background_event(event, &mut batch);
            })
            .await;
        let task_id = invocation.task_id();
        let endpoint = self.format_value(&Value::identity(self.endpoint));
        let session = self.format_value(&Value::identity(self.session));
        let key = match outcome {
            InvocationOutcome::Completed(value) if value.as_symbol() == Some(sym("unbound")) => {
                MicaKeyResult::Unbound
            }
            InvocationOutcome::Completed(value) if value.as_symbol() == Some(sym("prefix")) => {
                MicaKeyResult::Prefix
            }
            InvocationOutcome::Completed(_) => MicaKeyResult::Handled,
            InvocationOutcome::Aborted(error) => MicaKeyResult::Failed(format!(
                "Mica task {task_id} selector={selector} endpoint={endpoint} session={session} failure_class=aborted: {}",
                self.format_value(&error)
            )),
            InvocationOutcome::Failed(error) => MicaKeyResult::Failed(format!(
                "Mica task {task_id} selector={selector} endpoint={endpoint} session={session} failure_class=failed: {error}"
            )),
            InvocationOutcome::Cancelled(reason) => MicaKeyResult::Failed(format!(
                "Mica task {task_id} selector={selector} endpoint={endpoint} session={session} failure_class=cancelled: {reason:?}"
            )),
        };
        self.event_pump = Some(pump);
        Ok(MicaDispatchResult { key, events: batch })
    }

    fn record_background_event(&mut self, event: DriverEvent, batch: &mut MicaEventBatch) {
        let event = match event {
            DriverEvent::Effect(effect) => self
                .decode_effect(effect.target, &effect.value)
                .unwrap_or_else(MicaEvent::Error),
            DriverEvent::TaskAborted { task_id, error } => MicaEvent::Error(format!(
                "Mica background task {task_id} aborted: {}",
                self.format_value(&error)
            )),
            DriverEvent::TaskFailed { task_id, error } => {
                MicaEvent::Error(format!("Mica background task {task_id} failed: {error}"))
            }
            DriverEvent::TaskCancelled { task_id, .. } => MicaEvent::TaskCancelled(task_id),
            DriverEvent::SubscriptionReady { mailbox } => MicaEvent::SubscriptionReady(mailbox),
            DriverEvent::TaskCompleted { .. } | DriverEvent::TaskSuspended { .. } => return,
        };
        let prompt_active = match &event {
            MicaEvent::Prompt(_) => Some(true),
            MicaEvent::PromptClosed => Some(false),
            _ => None,
        };
        if batch.push(event)
            && let Some(active) = prompt_active
        {
            self.prompt_active = active;
        }
    }

    pub async fn close(&mut self) -> Result<MicaEventBatch, MicaHostError> {
        if self.closed {
            return Ok(MicaEventBatch::default());
        }
        self.closed = true;
        let mut events = self.drain_background_events();
        let endpoint = self
            .endpoint_session
            .take()
            .expect("open Mica host retains its endpoint session");
        let mut pump = self
            .event_pump
            .take()
            .expect("open Mica host retains its event pump");
        let endpoint_result = endpoint
            .close_with_pump(&mut pump, |event| {
                self.record_background_event(event, &mut events);
            })
            .await;
        let mut owner = self
            .owner
            .take()
            .expect("open Mica host retains its driver owner");
        let shutdown_result = owner
            .shutdown(&mut pump, |event| {
                self.record_background_event(event, &mut events)
            })
            .await;
        let report = endpoint_result?;
        shutdown_result?;
        for task_id in report.cancelled_tasks {
            events.push(MicaEvent::TaskCancelled(task_id));
        }
        Ok(events)
    }
}

fn sym(name: &str) -> Symbol {
    Symbol::intern(name)
}

fn int_value(value: impl TryInto<i64>) -> Value {
    Value::int(value.try_into().ok().unwrap_or(i64::MAX)).unwrap()
}

fn map_value(value: &Value, key: &str) -> Option<Value> {
    value.map_get(&Value::symbol(sym(key)))
}

fn native_ok(value: Value) -> Value {
    Value::map([
        (Value::symbol(sym("status")), Value::symbol(sym("ok"))),
        (Value::symbol(sym("value")), value),
    ])
}

fn native_error(message: &str) -> Value {
    Value::map([
        (Value::symbol(sym("status")), Value::symbol(sym("error"))),
        (Value::symbol(sym("error")), Value::string(message)),
    ])
}

fn build_layout_tuples(
    driver: &DriverClient,
    frame: Identity,
    root: &WindowNode,
    views: &HashMap<WindowId, Identity>,
    nodes: &mut HashMap<Vec<usize>, Identity>,
) -> Result<Vec<LayoutFact>, MicaHostError> {
    #[expect(
        clippy::too_many_arguments,
        reason = "recursive layout construction carries explicit bounded accumulators"
    )]
    fn visit(
        driver: &DriverClient,
        node: &WindowNode,
        path: &mut Vec<usize>,
        views: &HashMap<WindowId, Identity>,
        nodes: &mut HashMap<Vec<usize>, Identity>,
        tuples: &mut Vec<LayoutFact>,
        leaves: &mut Vec<Identity>,
        live_paths: &mut HashSet<Vec<usize>>,
    ) -> Result<Identity, MicaHostError> {
        match node {
            WindowNode::Leaf { window_id } => {
                let view = *views.get(window_id).ok_or(MicaHostError::MissingIdentity)?;
                leaves.push(view);
                Ok(view)
            }
            WindowNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                live_paths.insert(path.clone());
                let identity = if let Some(identity) = nodes.get(path).copied() {
                    identity
                } else {
                    let identity = driver.allocate_ephemeral_identity()?;
                    nodes.insert(path.clone(), identity);
                    identity
                };
                tuples.push(LayoutFact::View(identity));
                path.push(0);
                let first = visit(
                    driver, first, path, views, nodes, tuples, leaves, live_paths,
                )?;
                path.pop();
                path.push(1);
                let second = visit(
                    driver, second, path, views, nodes, tuples, leaves, live_paths,
                )?;
                path.pop();
                tuples.extend([
                    LayoutFact::First(identity, first),
                    LayoutFact::Second(identity, second),
                    LayoutFact::Axis(
                        identity,
                        match direction {
                            SplitDirection::Horizontal => sym("horizontal"),
                            SplitDirection::Vertical => sym("vertical"),
                        },
                    ),
                    LayoutFact::Ratio(identity, *ratio),
                ]);
                Ok(identity)
            }
        }
    }

    let mut tuples = Vec::new();
    let mut leaves = Vec::new();
    let mut live_paths = HashSet::new();
    let root = visit(
        driver,
        root,
        &mut Vec::new(),
        views,
        nodes,
        &mut tuples,
        &mut leaves,
        &mut live_paths,
    )?;
    nodes.retain(|path, _| live_paths.contains(path));
    tuples.push(LayoutFact::Root(frame, root));
    if leaves.len() > 1 {
        for index in 0..leaves.len() {
            tuples.push(LayoutFact::Next(
                leaves[index],
                leaves[(index + 1) % leaves.len()],
            ));
        }
    }
    Ok(tuples)
}

pub fn normalized_key_sequence(keys: &[crate::keys::LogicalKey]) -> String {
    let mut result = Vec::new();
    let mut index = 0;
    while index < keys.len() {
        let start = index;
        let mut modifiers = Vec::new();
        while index < keys.len() && matches!(keys[index], crate::keys::LogicalKey::Modifier(_)) {
            modifiers.push(keys[index].as_display_string());
            index += 1;
        }
        if index == keys.len() {
            result.extend(modifiers);
            break;
        }
        let key = keys[index];
        index += 1;
        let key_name = match key {
            crate::keys::LogicalKey::AlphaNumeric(' ') if modifiers.is_empty() => " ".to_owned(),
            crate::keys::LogicalKey::AlphaNumeric(' ') => "Space".to_owned(),
            _ => key.as_display_string(),
        };
        let shift_only_text = index - start == 2
            && modifiers.as_slice() == ["S"]
            && matches!(key, crate::keys::LogicalKey::AlphaNumeric(_));
        if shift_only_text {
            let crate::keys::LogicalKey::AlphaNumeric(character) = key else {
                unreachable!()
            };
            result.push(character.to_uppercase().collect());
        } else if modifiers.is_empty() {
            result.push(key_name);
        } else {
            modifiers.push(key_name);
            result.push(modifiers.join("-"));
        }
    }
    result.join(" ")
}

#[cfg(test)]
mod tests {
    use super::{normalized_key_sequence, source_path};
    use crate::keys::{KeyModifier, LogicalKey, Side};
    use std::path::{Path, PathBuf};

    #[test]
    fn roe_buffer_source_paths_cannot_escape_the_workspace_root() {
        let root = Path::new("/workspace");

        assert_eq!(
            source_path(root, "src/main.rs"),
            Some(PathBuf::from("/workspace/src/main.rs"))
        );
        assert_eq!(source_path(root, "../outside"), None);
        assert_eq!(source_path(root, "/etc/passwd"), None);
    }

    #[test]
    fn normalized_keys_use_the_mica_keymap_spelling() {
        assert_eq!(normalized_key_sequence(&[LogicalKey::Function(12)]), "F12");
        assert_eq!(
            normalized_key_sequence(&[LogicalKey::AlphaNumeric(' ')]),
            " "
        );
        assert_eq!(
            normalized_key_sequence(&[
                LogicalKey::Modifier(KeyModifier::Control(Side::Right)),
                LogicalKey::AlphaNumeric('x'),
            ]),
            "C-x"
        );
        assert_eq!(
            normalized_key_sequence(&[
                LogicalKey::Modifier(KeyModifier::Shift(Side::Left)),
                LogicalKey::AlphaNumeric('Z'),
            ]),
            "Z"
        );
        assert_eq!(
            normalized_key_sequence(&[
                LogicalKey::Modifier(KeyModifier::Control(Side::Left)),
                LogicalKey::Modifier(KeyModifier::Shift(Side::Right)),
                LogicalKey::Left,
            ]),
            "C-S-←"
        );
    }
}
