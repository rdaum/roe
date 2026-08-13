//! Public-driver-only Mica embedding for Roe's session host.
//!
//! Mica owns command/keymap policy. This module translates volatile Roe
//! identities, bounded native requests, and committed effects at the host
//! boundary; it does not expose renderer or Rust policy objects to Mica.

use crate::editor::{SplitDirection, WindowNode};
use crate::native_kernel::{NativeKernel, NativeOperation, NativeResult, ResourceId};
use crate::{BufferId, Editor, WindowId};
use mica_driver::{
    CompioTaskDriver, DriverError, DriverEvent, DriverResources, ExternalRequestContext,
    ExternalRequestFuture, ExternalRequestHandler, FileinMode, Identity, RelationAcceleration,
    Symbol, TaskId, TaskLimits, Value,
};
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
const EVENT_QUEUE_CAPACITY: usize = 256;
const EXTERNAL_REQUEST_CAPACITY: usize = 16;
const SUBSCRIPTION_QUEUE_BUDGET: usize = 64;
const MAX_PROMPT_CANDIDATES: usize = 256;
const MAX_SEARCH_MATCHES: usize = 1_024;
const MAX_POLICY_FACTS: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum MicaHostError {
    #[error("Mica driver failed: {0}")]
    Driver(#[from] DriverError),
    #[error("Mica session has no logical identity for the active Rust object")]
    MissingIdentity,
    #[error("Mica session host is already closed")]
    Closed,
    #[error("Mica editor policy rejected the operation: {0}")]
    Policy(String),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MicaPromptTarget {
    Selector(String),
    Buffer(BufferId),
    Path(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicaPromptUpdate {
    pub kind: String,
    pub query: String,
    pub selected: usize,
    pub candidates: Vec<(String, MicaPromptTarget)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MicaHostAction {
    pub name: String,
    pub buffer: Option<BufferId>,
    pub view: Option<WindowId>,
    pub path: Option<String>,
    pub position: Option<usize>,
    pub anchor: Option<usize>,
    pub phase: Option<String>,
    pub line: Option<u16>,
    pub column: Option<u16>,
    pub split_path: Option<Vec<usize>>,
    pub ratio: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicaNativeAction {
    pub name: String,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicaPolicyFact {
    pub kind: String,
    pub subject: Option<BufferId>,
    pub name: String,
    pub attribute: Option<String>,
    pub value: String,
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

#[derive(Debug, Default)]
pub struct MicaEventBatch {
    pub effects: Vec<MicaPresentationEffect>,
    pub host_actions: Vec<MicaHostAction>,
    pub native_actions: Vec<MicaNativeAction>,
    pub policy_reset: bool,
    pub policy_facts: Vec<MicaPolicyFact>,
    pub prompt_updates: Vec<MicaPromptUpdate>,
    pub prompt_close: bool,
    pub search_updates: Vec<MicaSearchUpdate>,
    pub search_finishes: Vec<MicaSearchFinish>,
    pub errors: Vec<String>,
    pub cancelled_tasks: Vec<TaskId>,
    pub ready_subscriptions: Vec<u64>,
}

impl MicaEventBatch {
    fn extend(&mut self, mut other: Self) {
        self.effects.append(&mut other.effects);
        self.host_actions.append(&mut other.host_actions);
        self.native_actions.append(&mut other.native_actions);
        self.policy_reset |= other.policy_reset;
        for policy in other.policy_facts.drain(..) {
            self.push_policy(policy);
        }
        self.prompt_updates.append(&mut other.prompt_updates);
        self.prompt_close |= other.prompt_close;
        self.search_updates.append(&mut other.search_updates);
        self.search_finishes.append(&mut other.search_finishes);
        self.errors.append(&mut other.errors);
        self.cancelled_tasks.append(&mut other.cancelled_tasks);
        self.ready_subscriptions
            .append(&mut other.ready_subscriptions);
    }

    fn push_policy(&mut self, policy: MicaPolicyFact) {
        if self.policy_facts.len() < MAX_POLICY_FACTS {
            self.policy_facts.push(policy);
        } else if !self
            .errors
            .iter()
            .any(|error| error.contains("policy fact limit"))
        {
            self.errors.push(format!(
                "Mica policy fact limit of {MAX_POLICY_FACTS} exceeded"
            ));
        }
    }
}

#[derive(Debug)]
pub struct MicaDispatchResult {
    pub key: MicaKeyResult,
    pub events: MicaEventBatch,
}

struct NativeBridge {
    kernel: Arc<Mutex<NativeKernel>>,
    state: Mutex<NativeBridgeState>,
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

    fn handle(&self, context: ExternalRequestContext, service: Symbol, payload: Value) -> Value {
        if context.cancellation.is_cancelled() {
            return native_error("request cancelled before native admission");
        }
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

        let operation = if service == sym("clock_millis") {
            NativeOperation::ReadClockMillis
        } else if service == sym("list_directory") {
            let path = map_value(&payload, "path")
                .and_then(|value| value.with_str(str::to_owned))
                .unwrap_or_else(|| ".".to_owned());
            NativeOperation::ListDirectory { path: path.into() }
        } else if service == sym("text_search") {
            let Some(buffer) = map_value(&payload, "buffer").and_then(|value| value.as_identity())
            else {
                return native_error("text_search requires an identity buffer");
            };
            let Some(resource) = state.resources.get(&buffer).copied() else {
                return native_error("text_search buffer is not authorized for this endpoint");
            };
            NativeOperation::Snapshot { resource }
        } else {
            let Some(buffer) = map_value(&payload, "buffer").and_then(|value| value.as_identity())
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
        };
        drop(state);

        if context.cancellation.is_cancelled() {
            return native_error("request cancelled before native execution");
        }
        match self.kernel.lock().unwrap().execute(operation) {
            Ok(NativeResult::ClockMillis(value)) => native_ok(
                Value::int(i64::try_from(value).unwrap_or(i64::MAX))
                    .unwrap_or_else(|_| Value::string(value.to_string())),
            ),
            Ok(NativeResult::DirectoryEntries(entries)) => native_ok(Value::list(
                entries
                    .into_iter()
                    .take(256)
                    .map(|path| Value::string(path.to_string_lossy())),
            )),
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
    driver: CompioTaskDriver,
    bridge: Arc<NativeBridge>,
    endpoint: Identity,
    actor: Identity,
    session: Identity,
    frame: Identity,
    editor_role: Identity,
    fundamental_mode: Identity,
    global_map: Identity,
    buffer_ids: HashMap<BufferId, Identity>,
    buffer_names: HashMap<BufferId, String>,
    native_ids: HashMap<BufferId, Identity>,
    resource_ids: HashMap<BufferId, ResourceId>,
    view_ids: HashMap<WindowId, Identity>,
    view_buffers: HashMap<WindowId, BufferId>,
    view_cursors: HashMap<WindowId, usize>,
    layout_nodes: HashMap<Vec<usize>, Identity>,
    layout_tuples: Vec<LayoutFact>,
    disabled_packages: HashSet<Identity>,
    active_view: WindowId,
    pending_key_prefix: Option<String>,
    prompt_active: bool,
    closed: bool,
}

impl MicaHost {
    pub fn open(
        editor: &Editor,
        kernel: Arc<Mutex<NativeKernel>>,
        resource_ids: &HashMap<BufferId, ResourceId>,
    ) -> Result<Self, MicaHostError> {
        let bridge = Arc::new(NativeBridge::new(kernel));
        let handler_bridge = Arc::clone(&bridge);
        let external_handler: ExternalRequestHandler = Arc::new(move |context, request| {
            let bridge = Arc::clone(&handler_bridge);
            Box::pin(async move {
                if request.service == sym("test_pending") {
                    context.cancellation.cancelled().await;
                    return native_error("request cancelled");
                }
                bridge.handle(context, request.service, request.payload)
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
        resources.relation_acceleration = RelationAcceleration::Disabled;

        let driver = CompioTaskDriver::builder(resources)
            .initial_filein_unit(sym("roe/core"), CORE_SOURCE, FileinMode::Add, None)
            .initial_filein_unit(
                sym("roe/first-wave"),
                FIRST_WAVE_SOURCE,
                FileinMode::Add,
                None,
            )
            .external_request_handler(external_handler)
            .build()?;

        let endpoint = driver.allocate_ephemeral_identity()?;
        let actor = driver.allocate_ephemeral_identity()?;
        let session = driver.allocate_ephemeral_identity()?;
        let frame = driver.allocate_ephemeral_identity()?;
        let editor_role = driver.named_identity(sym("roe/editor_role"))?;
        let fundamental_mode = driver.named_identity(sym("roe/fundamental_mode"))?;
        let global_map = driver.named_identity(sym("roe/global_map"))?;

        let mut buffer_ids = HashMap::new();
        let mut buffer_names = HashMap::new();
        let mut native_ids = HashMap::new();
        let mut bridge_resources = HashMap::new();
        for (buffer_id, _buffer) in &editor.buffers {
            let buffer = driver.allocate_ephemeral_identity()?;
            let native = driver.allocate_ephemeral_identity()?;
            let resource = *resource_ids
                .get(&buffer_id)
                .ok_or(MicaHostError::MissingIdentity)?;
            buffer_ids.insert(buffer_id, buffer);
            buffer_names.insert(buffer_id, _buffer.object());
            native_ids.insert(buffer_id, native);
            bridge_resources.insert(buffer, resource);
        }

        let mut view_ids = HashMap::new();
        let mut view_buffers = HashMap::new();
        let mut view_cursors = HashMap::new();
        for (window_id, window) in &editor.windows {
            view_ids.insert(window_id, driver.allocate_ephemeral_identity()?);
            view_buffers.insert(window_id, window.active_buffer);
            view_cursors.insert(window_id, window.cursor);
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
        for (buffer_id, buffer) in &editor.buffers {
            let logical = buffer_ids[&buffer_id];
            let native = native_ids[&buffer_id];
            let resource = resource_ids[&buffer_id];
            tuples.push((sym("roe/LogicalBuffer"), [Value::identity(logical)].into()));
            tuples.push((
                sym("roe/BufferName"),
                [Value::identity(logical), Value::string(buffer.object())].into(),
            ));
            tuples.push((
                sym("roe/BufferMajorMode"),
                [Value::identity(logical), Value::identity(fundamental_mode)].into(),
            ));
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
        }
        let mut layout_nodes = HashMap::new();
        let layout_tuples = build_layout_tuples(
            &driver,
            frame,
            &editor.window_tree,
            &view_ids,
            &mut layout_nodes,
        )?;
        tuples.extend(layout_named_tuples!(&layout_tuples));
        driver.open_endpoint_with_context_and_volatile_tuples_named(
            endpoint,
            None,
            Some(actor),
            sym("roe/session-v1"),
            tuples,
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
            driver,
            bridge,
            endpoint,
            actor,
            session,
            frame,
            editor_role,
            fundamental_mode,
            global_map,
            buffer_ids,
            buffer_names,
            native_ids,
            resource_ids: resource_ids.clone(),
            view_ids,
            view_buffers,
            view_cursors,
            layout_nodes,
            layout_tuples,
            disabled_packages: HashSet::new(),
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
        let submitted = self
            .driver
            .submit_invocation_for_endpoint(
                self.endpoint,
                selector,
                vec![
                    (sym("actor"), Value::identity(self.actor)),
                    (sym("session"), Value::identity(self.session)),
                    (sym("sequence"), Value::string(&sequence)),
                ],
            )
            .await?;
        let mut result = self.wait_for_task(submitted.task_id).await?;
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
        line: u16,
        column: u16,
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
                (sym("line"), int_value(line as usize)),
                (sym("column"), int_value(column as usize)),
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
        let submitted = self
            .driver
            .submit_invocation_for_endpoint(self.endpoint, sym(selector), arguments)
            .await?;
        let result = self.wait_for_task(submitted.task_id).await?;
        match result.key {
            MicaKeyResult::Failed(message) => Err(MicaHostError::Policy(message)),
            _ => Ok(result.events),
        }
    }

    pub fn drain_background_events(&mut self) -> MicaEventBatch {
        let mut batch = MicaEventBatch::default();
        for event in self.driver.drain_events() {
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
        self.synchronize_context(editor, resource_ids)?;
        let submitted = self
            .driver
            .submit_invocation_for_endpoint(
                self.endpoint,
                sym("roe/publish_policy"),
                vec![
                    (sym("actor"), Value::identity(self.actor)),
                    (sym("session"), Value::identity(self.session)),
                ],
            )
            .await?;
        Ok(self.wait_for_task(submitted.task_id).await?.events)
    }

    pub async fn check_source(&self, source: String) -> Result<(), MicaHostError> {
        self.driver.check_filein(source, None).await?;
        Ok(())
    }

    pub async fn replace_unit(&self, unit: &str, source: String) -> Result<(), MicaHostError> {
        self.driver.check_filein(source.clone(), None).await?;
        self.driver
            .filein_unit(sym(unit), source, FileinMode::Replace, None)
            .await?;
        Ok(())
    }

    pub async fn export_unit(&self, unit: &str) -> Result<String, MicaHostError> {
        Ok(self.driver.fileout_unit(sym(unit)).await?)
    }

    pub async fn restore_first_wave(&self) -> Result<(), MicaHostError> {
        self.replace_unit("roe/first-wave", FIRST_WAVE_SOURCE.to_owned())
            .await
    }

    pub fn set_package_enabled(
        &mut self,
        package: &str,
        enabled: bool,
    ) -> Result<(), MicaHostError> {
        let package = self.driver.named_identity(sym(package))?;
        let tuple = vec![(
            sym("roe/PackageDisabled"),
            [Value::identity(package)].into(),
        )];
        if enabled {
            self.driver.retract_volatile_tuples_named(tuple)?;
            self.disabled_packages.remove(&package);
        } else if self.disabled_packages.insert(package) {
            self.driver.assert_volatile_tuples_named(tuple)?;
        }
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
    pub async fn start_pending_test_request(&self) -> Result<TaskId, MicaHostError> {
        const SOURCE: &str = r#"
assert RoleCanInvoke(#roe/editor_role, :roe/test_pending)
verb roe/test_pending(actor, session)
  roe/SessionActor(session, actor) || return :not_session_actor
  return external_request(:test_pending, nothing, 60)
end
"#;
        self.driver
            .filein_unit(
                sym("roe/test-pending"),
                SOURCE.to_owned(),
                FileinMode::Add,
                None,
            )
            .await?;
        let submitted = self
            .driver
            .submit_invocation_for_endpoint(
                self.endpoint,
                sym("roe/test_pending"),
                vec![
                    (sym("actor"), Value::identity(self.actor)),
                    (sym("session"), Value::identity(self.session)),
                ],
            )
            .await?;
        self.driver.drain_events();
        Ok(submitted.task_id)
    }

    #[cfg(test)]
    pub async fn start_background_test_task(&self) -> Result<TaskId, MicaHostError> {
        const SOURCE: &str = r#"
assert RoleCanInvoke(#roe/editor_role, :roe/test_background)
verb roe/test_background(actor, session)
  roe/SessionActor(session, actor) || return :not_session_actor
  let view = one roe/ActiveView(session, ?view)
  let buffer = one roe/ViewBuffer(view, ?buffer)
  let cursor = one roe/ViewCursor(view, ?cursor)
  suspend(0.02)
  emit(session, {:kind -> :presentation_invalidated, :view -> view, :buffer -> buffer, :cursor -> cursor})
  return :done
end
"#;
        self.driver
            .filein_unit(
                sym("roe/test-background"),
                SOURCE.to_owned(),
                FileinMode::Add,
                None,
            )
            .await?;
        let submitted = self
            .driver
            .submit_invocation_for_endpoint(
                self.endpoint,
                sym("roe/test_background"),
                vec![
                    (sym("actor"), Value::identity(self.actor)),
                    (sym("session"), Value::identity(self.session)),
                ],
            )
            .await?;
        Ok(submitted.task_id)
    }

    #[cfg(test)]
    async fn fill_event_queue_for_test(&self) -> Result<(), MicaHostError> {
        for value in 0..EVENT_QUEUE_CAPACITY {
            self.driver
                .submit_root_source_report(format!("return {value}"))
                .await?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub async fn verify_event_backpressure_and_refill_for_test(
        &self,
    ) -> Result<bool, MicaHostError> {
        self.fill_event_queue_for_test().await?;
        let producer_driver = self.driver.clone();
        let producer = compio::runtime::spawn(async move {
            producer_driver
                .submit_root_source_report("return 999".to_owned())
                .await
        });
        compio::time::sleep(Duration::from_millis(10)).await;
        let was_backpressured = !producer.is_finished();

        self.driver.drain_events();
        producer
            .await
            .map_err(|_| DriverError::Join("event producer task panicked".to_owned()))??;
        self.driver.drain_events();
        self.fill_event_queue_for_test().await?;
        Ok(was_backpressured)
    }

    fn synchronize_context(
        &mut self,
        editor: &Editor,
        resource_ids: &HashMap<BufferId, ResourceId>,
    ) -> Result<(), MicaHostError> {
        let live_buffers: HashSet<_> = editor.buffers.keys().collect();
        let live_views: HashSet<_> = editor.windows.keys().collect();
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
        let mut stale_tuples = Vec::new();
        for window_id in &stale_views {
            let view = self.view_ids[window_id];
            if *window_id == self.active_view {
                stale_tuples.push((
                    sym("roe/ActiveView"),
                    [Value::identity(self.session), Value::identity(view)].into(),
                ));
            }
            stale_tuples.push((sym("roe/View"), [Value::identity(view)].into()));
            if let Some(buffer_id) = self.view_buffers.get(window_id) {
                stale_tuples.push((
                    sym("roe/ViewBuffer"),
                    [
                        Value::identity(view),
                        Value::identity(self.buffer_ids[buffer_id]),
                    ]
                    .into(),
                ));
            }
            if let Some(cursor) = self.view_cursors.get(window_id) {
                stale_tuples.push((
                    sym("roe/ViewCursor"),
                    [Value::identity(view), int_value(*cursor)].into(),
                ));
            }
        }
        for buffer_id in &stale_buffers {
            let logical = self.buffer_ids[buffer_id];
            let native = self.native_ids[buffer_id];
            let resource = self.resource_ids[buffer_id];
            stale_tuples.extend([
                (sym("roe/LogicalBuffer"), [Value::identity(logical)].into()),
                (
                    sym("roe/BufferName"),
                    [
                        Value::identity(logical),
                        Value::string(&self.buffer_names[buffer_id]),
                    ]
                    .into(),
                ),
                (
                    sym("roe/BufferMajorMode"),
                    [
                        Value::identity(logical),
                        Value::identity(self.fundamental_mode),
                    ]
                    .into(),
                ),
                (
                    sym("roe/NativeTextResource"),
                    [Value::identity(logical), Value::identity(native)].into(),
                ),
                (
                    sym("roe/NativeResourceGeneration"),
                    [Value::identity(native), int_value(resource.generation)].into(),
                ),
                (
                    sym("roe/CanUseBuffer"),
                    [Value::identity(self.actor), Value::identity(logical)].into(),
                ),
            ]);
        }
        self.driver.retract_volatile_tuples_named(stale_tuples)?;
        for window_id in stale_views {
            self.view_ids.remove(&window_id);
            self.view_buffers.remove(&window_id);
            self.view_cursors.remove(&window_id);
        }
        for buffer_id in stale_buffers {
            let logical = self.buffer_ids.remove(&buffer_id).unwrap();
            self.bridge.remove_resource(logical);
            self.buffer_names.remove(&buffer_id);
            self.native_ids.remove(&buffer_id);
            self.resource_ids.remove(&buffer_id);
        }

        let active = editor.active_window;
        let window = editor
            .windows
            .get(active)
            .ok_or(MicaHostError::MissingIdentity)?;
        let mut retract = Vec::new();
        let mut assert = Vec::new();

        if !self.buffer_ids.contains_key(&window.active_buffer) {
            let logical = self.driver.allocate_ephemeral_identity()?;
            let native = self.driver.allocate_ephemeral_identity()?;
            let resource = *resource_ids
                .get(&window.active_buffer)
                .ok_or(MicaHostError::MissingIdentity)?;
            let buffer = editor
                .buffers
                .get(window.active_buffer)
                .ok_or(MicaHostError::MissingIdentity)?;
            self.buffer_ids.insert(window.active_buffer, logical);
            self.buffer_names
                .insert(window.active_buffer, buffer.object());
            self.native_ids.insert(window.active_buffer, native);
            self.resource_ids.insert(window.active_buffer, resource);
            self.bridge.add_resource(logical, resource);
            assert.extend([
                (sym("roe/LogicalBuffer"), [Value::identity(logical)].into()),
                (
                    sym("roe/BufferName"),
                    [Value::identity(logical), Value::string(buffer.object())].into(),
                ),
                (
                    sym("roe/BufferMajorMode"),
                    [
                        Value::identity(logical),
                        Value::identity(self.fundamental_mode),
                    ]
                    .into(),
                ),
                (
                    sym("roe/NativeTextResource"),
                    [Value::identity(logical), Value::identity(native)].into(),
                ),
                (
                    sym("roe/NativeResourceGeneration"),
                    [Value::identity(native), int_value(resource.generation)].into(),
                ),
                (
                    sym("roe/CanUseBuffer"),
                    [Value::identity(self.actor), Value::identity(logical)].into(),
                ),
            ]);
        }
        let buffer = self.buffer_ids[&window.active_buffer];

        for (window_id, candidate) in &editor.windows {
            if self.view_ids.contains_key(&window_id) {
                continue;
            }
            let Some(logical_buffer) = self.buffer_ids.get(&candidate.active_buffer).copied()
            else {
                continue;
            };
            let logical_view = self.driver.allocate_ephemeral_identity()?;
            self.view_ids.insert(window_id, logical_view);
            self.view_buffers.insert(window_id, candidate.active_buffer);
            self.view_cursors.insert(window_id, candidate.cursor);
            assert.extend([
                (sym("roe/View"), [Value::identity(logical_view)].into()),
                (
                    sym("roe/ViewBuffer"),
                    [
                        Value::identity(logical_view),
                        Value::identity(logical_buffer),
                    ]
                    .into(),
                ),
                (
                    sym("roe/ViewCursor"),
                    [Value::identity(logical_view), int_value(candidate.cursor)].into(),
                ),
            ]);
        }

        if let std::collections::hash_map::Entry::Vacant(entry) = self.view_ids.entry(active) {
            let view = self.driver.allocate_ephemeral_identity()?;
            entry.insert(view);
            self.view_buffers.insert(active, window.active_buffer);
            self.view_cursors.insert(active, window.cursor);
            assert.push((sym("roe/View"), [Value::identity(view)].into()));
        }
        let view = self.view_ids[&active];
        if self.active_view != active
            && let Some(previous_active) = self.view_ids.get(&self.active_view).copied()
        {
            retract.push((
                sym("roe/ActiveView"),
                [
                    Value::identity(self.session),
                    Value::identity(previous_active),
                ]
                .into(),
            ));
        }
        if self.view_buffers.get(&active).copied() != Some(window.active_buffer)
            && let Some(previous) = self.view_buffers.get(&active).copied()
        {
            retract.push((
                sym("roe/ViewBuffer"),
                [
                    Value::identity(view),
                    Value::identity(self.buffer_ids[&previous]),
                ]
                .into(),
            ));
        }
        if self.view_cursors.get(&active).copied() != Some(window.cursor)
            && let Some(previous) = self.view_cursors.get(&active).copied()
        {
            retract.push((
                sym("roe/ViewCursor"),
                [Value::identity(view), int_value(previous)].into(),
            ));
        }
        assert.extend([
            (
                sym("roe/ActiveView"),
                [Value::identity(self.session), Value::identity(view)].into(),
            ),
            (
                sym("roe/ViewBuffer"),
                [Value::identity(view), Value::identity(buffer)].into(),
            ),
            (
                sym("roe/ViewCursor"),
                [Value::identity(view), int_value(window.cursor)].into(),
            ),
        ]);
        self.driver.retract_volatile_tuples_named(retract)?;
        self.driver.assert_volatile_tuples_named(assert)?;
        self.active_view = active;
        self.view_buffers.insert(active, window.active_buffer);
        self.view_cursors.insert(active, window.cursor);
        self.synchronize_layout(editor)?;
        Ok(())
    }

    fn synchronize_layout(&mut self, editor: &Editor) -> Result<(), MicaHostError> {
        self.driver
            .retract_volatile_tuples_named(layout_named_tuples!(&self.layout_tuples))?;
        self.layout_tuples = build_layout_tuples(
            &self.driver,
            self.frame,
            &editor.window_tree,
            &self.view_ids,
            &mut self.layout_nodes,
        )?;
        self.driver
            .assert_volatile_tuples_named(layout_named_tuples!(&self.layout_tuples))?;
        Ok(())
    }

    async fn wait_for_task(
        &mut self,
        task_id: TaskId,
    ) -> Result<MicaDispatchResult, MicaHostError> {
        let mut batch = MicaEventBatch::default();
        loop {
            let events = {
                let ready = self.driver.drain_events();
                if ready.is_empty() {
                    self.driver.wait_events().await
                } else {
                    ready
                }
            };
            let mut key = None;
            for event in events {
                match event {
                    DriverEvent::Effect(effect) => {
                        if let Some(effect) = self.presentation_effect(effect.target, &effect.value)
                        {
                            batch.effects.push(effect);
                        } else if let Some(update) =
                            self.prompt_update(effect.target, &effect.value)
                        {
                            self.prompt_active = true;
                            batch.prompt_updates.push(update);
                        } else if self.prompt_closed(effect.target, &effect.value) {
                            self.prompt_active = false;
                            batch.prompt_close = true;
                        } else if let Some(update) =
                            self.search_update(effect.target, &effect.value)
                        {
                            batch.search_updates.push(update);
                        } else if let Some(finish) =
                            self.search_finish(effect.target, &effect.value)
                        {
                            batch.search_finishes.push(finish);
                        } else if self.policy_reset(effect.target, &effect.value) {
                            batch.policy_reset = true;
                        } else if let Some(policy) = self.policy_fact(effect.target, &effect.value)
                        {
                            batch.push_policy(policy);
                        } else if let Some(action) =
                            self.native_action(effect.target, &effect.value)
                        {
                            batch.native_actions.push(action);
                        } else if let Some(action) = self.host_action(effect.target, &effect.value)
                        {
                            batch.host_actions.push(action);
                        }
                    }
                    DriverEvent::TaskCompleted {
                        task_id: completed,
                        value,
                    } if completed == task_id => {
                        if value.as_symbol() == Some(sym("unbound")) {
                            key = Some(MicaKeyResult::Unbound);
                        } else if value.as_symbol() == Some(sym("prefix")) {
                            key = Some(MicaKeyResult::Prefix);
                        } else {
                            key = Some(MicaKeyResult::Handled);
                        }
                    }
                    DriverEvent::TaskAborted {
                        task_id: aborted,
                        error,
                    } if aborted == task_id => {
                        key = Some(MicaKeyResult::Failed(format!(
                            "Mica command aborted: {}",
                            self.driver.format_value(&error)
                        )));
                    }
                    DriverEvent::TaskFailed {
                        task_id: failed,
                        error,
                    } if failed == task_id => {
                        key = Some(MicaKeyResult::Failed(format!(
                            "Mica command failed: {error}"
                        )));
                    }
                    DriverEvent::TaskCancelled {
                        task_id: cancelled,
                        reason,
                    } if cancelled == task_id => {
                        key = Some(MicaKeyResult::Failed(format!(
                            "Mica command cancelled: {reason:?}"
                        )));
                    }
                    event => self.record_background_event(event, &mut batch),
                }
            }
            if let Some(key) = key {
                return Ok(MicaDispatchResult { key, events: batch });
            }
        }
    }

    fn record_background_event(&mut self, event: DriverEvent, batch: &mut MicaEventBatch) {
        match event {
            DriverEvent::Effect(effect) => {
                if let Some(effect) = self.presentation_effect(effect.target, &effect.value) {
                    batch.effects.push(effect);
                } else if let Some(update) = self.prompt_update(effect.target, &effect.value) {
                    self.prompt_active = true;
                    batch.prompt_updates.push(update);
                } else if self.prompt_closed(effect.target, &effect.value) {
                    self.prompt_active = false;
                    batch.prompt_close = true;
                } else if let Some(update) = self.search_update(effect.target, &effect.value) {
                    batch.search_updates.push(update);
                } else if let Some(finish) = self.search_finish(effect.target, &effect.value) {
                    batch.search_finishes.push(finish);
                } else if self.policy_reset(effect.target, &effect.value) {
                    batch.policy_reset = true;
                } else if let Some(policy) = self.policy_fact(effect.target, &effect.value) {
                    batch.push_policy(policy);
                } else if let Some(action) = self.native_action(effect.target, &effect.value) {
                    batch.native_actions.push(action);
                } else if let Some(action) = self.host_action(effect.target, &effect.value) {
                    batch.host_actions.push(action);
                }
            }
            DriverEvent::TaskAborted { task_id, error } => batch.errors.push(format!(
                "Mica background task {task_id} aborted: {}",
                self.driver.format_value(&error)
            )),
            DriverEvent::TaskFailed { task_id, error } => batch
                .errors
                .push(format!("Mica background task {task_id} failed: {error}")),
            DriverEvent::TaskCancelled { task_id, .. } => {
                batch.cancelled_tasks.push(task_id);
            }
            DriverEvent::SubscriptionReady { mailbox } => {
                batch.ready_subscriptions.push(mailbox);
            }
            DriverEvent::TaskCompleted { .. } | DriverEvent::TaskSuspended { .. } => {}
        }
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

    fn native_action(&self, target: Identity, value: &Value) -> Option<MicaNativeAction> {
        if target != self.session || map_value(value, "kind")?.as_symbol()? != sym("native_action")
        {
            return None;
        }
        let action = map_value(value, "action")?.as_symbol()?;
        Some(MicaNativeAction {
            name: action.name()?.to_owned(),
            text: map_value(value, "text").and_then(|value| value.with_str(str::to_owned)),
        })
    }

    fn policy_fact(&self, target: Identity, value: &Value) -> Option<MicaPolicyFact> {
        if target != self.session {
            return None;
        }
        let kind = map_value(value, "kind")?.as_symbol()?.name()?.to_owned();
        if !matches!(
            kind.as_str(),
            "mode_policy" | "face_policy" | "syntax_policy" | "configuration_policy"
        ) {
            return None;
        }
        let subject = map_value(value, "buffer")
            .and_then(|value| value.as_identity())
            .and_then(|logical| {
                self.buffer_ids
                    .iter()
                    .find_map(|(buffer, identity)| (*identity == logical).then_some(*buffer))
            });
        let name = if kind == "face_policy" {
            map_value(value, "face")?.with_str(str::to_owned)?
        } else if kind == "mode_policy" {
            map_value(value, "name")?.with_str(str::to_owned)?
        } else {
            map_value(value, "syntax_kind")
                .or_else(|| map_value(value, "key"))?
                .as_symbol()?
                .name()?
                .to_owned()
        };
        let attribute = map_value(value, "attribute")
            .and_then(|value| value.as_symbol())
            .and_then(Symbol::name)
            .map(str::to_owned);
        let raw = map_value(value, "value").or_else(|| map_value(value, "pattern"));
        let value = raw
            .clone()
            .and_then(|value| value.with_str(str::to_owned))
            .or_else(|| raw.map(|value| self.driver.format_value(&value)))
            .unwrap_or_default();
        Some(MicaPolicyFact {
            kind,
            subject,
            name,
            attribute,
            value,
        })
    }

    fn policy_reset(&self, target: Identity, value: &Value) -> bool {
        target == self.session
            && map_value(value, "kind").and_then(|value| value.as_symbol())
                == Some(sym("policy_reset"))
    }

    fn host_action(&self, target: Identity, value: &Value) -> Option<MicaHostAction> {
        if target != self.session || map_value(value, "kind")?.as_symbol()? != sym("host_action") {
            return None;
        }
        let name = map_value(value, "action")?
            .as_symbol()?
            .name()
            .map(str::to_owned)?;
        let buffer = map_value(value, "buffer")
            .and_then(|value| value.as_identity())
            .and_then(|logical| {
                self.buffer_ids
                    .iter()
                    .find_map(|(buffer, identity)| (*identity == logical).then_some(*buffer))
            });
        let path = map_value(value, "path").and_then(|value| value.with_str(str::to_owned));
        let view = map_value(value, "view")
            .and_then(|value| value.as_identity())
            .and_then(|logical| {
                self.view_ids
                    .iter()
                    .find_map(|(view, identity)| (*identity == logical).then_some(*view))
            });
        let position = map_value(value, "position")
            .and_then(|value| value.as_int())
            .and_then(|value| usize::try_from(value).ok());
        let anchor = map_value(value, "anchor")
            .and_then(|value| value.as_int())
            .and_then(|value| usize::try_from(value).ok());
        let phase = map_value(value, "phase")
            .and_then(|value| value.as_symbol())
            .and_then(|value| value.name().map(str::to_owned));
        let line = map_value(value, "line")
            .and_then(|value| value.as_int())
            .and_then(|value| u16::try_from(value).ok());
        let column = map_value(value, "column")
            .and_then(|value| value.as_int())
            .and_then(|value| u16::try_from(value).ok());
        let split_path = map_value(value, "node")
            .and_then(|value| value.as_identity())
            .and_then(|node| {
                self.layout_nodes
                    .iter()
                    .find_map(|(path, identity)| (*identity == node).then_some(path.clone()))
            });
        let ratio = map_value(value, "ratio").and_then(|value| value.as_float());
        Some(MicaHostAction {
            name,
            buffer,
            view,
            path,
            position,
            anchor,
            phase,
            line,
            column,
            split_path,
            ratio,
        })
    }

    fn prompt_closed(&self, target: Identity, value: &Value) -> bool {
        target == self.session
            && map_value(value, "kind").and_then(|value| value.as_symbol())
                == Some(sym("prompt_close"))
    }

    fn prompt_update(&self, target: Identity, value: &Value) -> Option<MicaPromptUpdate> {
        if target != self.session || map_value(value, "kind")?.as_symbol()? != sym("prompt_update")
        {
            return None;
        }
        let kind = map_value(value, "prompt_kind")?
            .as_symbol()?
            .name()?
            .to_owned();
        let query = map_value(value, "query")?.with_str(str::to_owned)?;
        let selected = usize::try_from(map_value(value, "selected")?.as_int()?).ok()?;
        let values = map_value(value, "candidates")?;
        let mut candidates = Vec::new();
        for index in 0..values.list_len()?.min(MAX_PROMPT_CANDIDATES) {
            let row = values.list_get(index)?;
            let name = row.list_get(0)?.with_str(str::to_owned)?;
            let raw = row.list_get(1)?;
            let target = if kind == "command" {
                MicaPromptTarget::Selector(raw.as_symbol()?.name()?.to_owned())
            } else if kind == "switch_buffer" || kind == "kill_buffer" {
                let logical = raw.as_identity()?;
                let buffer = self
                    .buffer_ids
                    .iter()
                    .find_map(|(buffer, identity)| (*identity == logical).then_some(*buffer))?;
                MicaPromptTarget::Buffer(buffer)
            } else {
                MicaPromptTarget::Path(raw.with_str(str::to_owned)?)
            };
            candidates.push((name, target));
        }
        Some(MicaPromptUpdate {
            kind,
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
        for index in 0..raw.list_len()?.min(MAX_SEARCH_MATCHES) {
            let row = raw.list_get(index)?;
            let start = usize::try_from(row.list_get(0)?.as_int()?).ok()?;
            let end = usize::try_from(row.list_get(1)?.as_int()?).ok()?;
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

    pub async fn close(&mut self) -> Result<MicaEventBatch, MicaHostError> {
        if self.closed {
            return Ok(MicaEventBatch::default());
        }
        self.closed = true;
        let mut events = self.drain_background_events();
        let mut tuples = vec![
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
        ];
        if let Some(active) = self.view_ids.get(&self.active_view).copied() {
            tuples.push((
                sym("roe/ActiveView"),
                [Value::identity(self.session), Value::identity(active)].into(),
            ));
        }
        for (buffer_id, logical) in &self.buffer_ids {
            let native = self.native_ids[buffer_id];
            let resource = self.resource_ids[buffer_id];
            tuples.push((sym("roe/LogicalBuffer"), [Value::identity(*logical)].into()));
            tuples.push((
                sym("roe/BufferName"),
                [
                    Value::identity(*logical),
                    Value::string(&self.buffer_names[buffer_id]),
                ]
                .into(),
            ));
            tuples.push((
                sym("roe/BufferMajorMode"),
                [
                    Value::identity(*logical),
                    Value::identity(self.fundamental_mode),
                ]
                .into(),
            ));
            tuples.push((
                sym("roe/NativeTextResource"),
                [Value::identity(*logical), Value::identity(native)].into(),
            ));
            tuples.push((
                sym("roe/NativeResourceGeneration"),
                [Value::identity(native), int_value(resource.generation)].into(),
            ));
            tuples.push((
                sym("roe/CanUseBuffer"),
                [Value::identity(self.actor), Value::identity(*logical)].into(),
            ));
        }
        for (window_id, view) in &self.view_ids {
            let Some(buffer_id) = self.view_buffers.get(window_id) else {
                continue;
            };
            let Some(cursor) = self.view_cursors.get(window_id) else {
                continue;
            };
            tuples.push((sym("roe/View"), [Value::identity(*view)].into()));
            tuples.push((
                sym("roe/ViewBuffer"),
                [
                    Value::identity(*view),
                    Value::identity(self.buffer_ids[buffer_id]),
                ]
                .into(),
            ));
            tuples.push((
                sym("roe/ViewCursor"),
                [Value::identity(*view), int_value(*cursor)].into(),
            ));
        }
        let close_driver = self.driver.clone();
        let endpoint = self.endpoint;
        let close = compio::runtime::spawn(async move {
            close_driver
                .close_endpoint_and_retract_volatile_tuples_named(endpoint, tuples)
                .await
        });
        while !close.is_finished() {
            events.extend(self.drain_background_events());
            compio::time::sleep(Duration::from_millis(1)).await;
        }
        let close_result = close
            .await
            .map_err(|_| DriverError::Join("endpoint close task panicked".to_owned()))?;
        events.extend(self.drain_background_events());

        let shutdown_driver = self.driver.clone();
        let shutdown = compio::runtime::spawn(async move { shutdown_driver.shutdown().await });
        while !shutdown.is_finished() {
            events.extend(self.drain_background_events());
            compio::time::sleep(Duration::from_millis(1)).await;
        }
        let shutdown_result = shutdown
            .await
            .map_err(|_| DriverError::Join("driver shutdown task panicked".to_owned()))?;
        events.extend(self.drain_background_events());

        let report = close_result?;
        shutdown_result?;
        events.cancelled_tasks.extend(report.cancelled_tasks);
        events.cancelled_tasks.sort_unstable();
        events.cancelled_tasks.dedup();
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
    driver: &CompioTaskDriver,
    frame: Identity,
    root: &WindowNode,
    views: &HashMap<WindowId, Identity>,
    nodes: &mut HashMap<Vec<usize>, Identity>,
) -> Result<Vec<LayoutFact>, MicaHostError> {
    fn visit(
        driver: &CompioTaskDriver,
        node: &WindowNode,
        path: &mut Vec<usize>,
        views: &HashMap<WindowId, Identity>,
        nodes: &mut HashMap<Vec<usize>, Identity>,
        tuples: &mut Vec<LayoutFact>,
        leaves: &mut Vec<Identity>,
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
                let identity = if let Some(identity) = nodes.get(path).copied() {
                    identity
                } else {
                    let identity = driver.allocate_ephemeral_identity()?;
                    nodes.insert(path.clone(), identity);
                    identity
                };
                tuples.push(LayoutFact::View(identity));
                path.push(0);
                let first = visit(driver, first, path, views, nodes, tuples, leaves)?;
                path.pop();
                path.push(1);
                let second = visit(driver, second, path, views, nodes, tuples, leaves)?;
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
    let root = visit(
        driver,
        root,
        &mut Vec::new(),
        views,
        nodes,
        &mut tuples,
        &mut leaves,
    )?;
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
    use super::normalized_key_sequence;
    use crate::keys::{KeyModifier, LogicalKey, Side};

    #[test]
    fn normalized_keys_use_the_mica_keymap_spelling() {
        assert_eq!(normalized_key_sequence(&[LogicalKey::Function(12)]), "F12");
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
