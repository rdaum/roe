//! Public-driver-only Mica embedding for Roe's session host.
//!
//! Mica owns command/keymap policy. This module translates volatile Roe
//! identities, bounded native requests, and committed effects at the host
//! boundary; it does not expose renderer or Rust policy objects to Mica.

use crate::native_kernel::{NativeKernel, NativeOperation, NativeResult, ResourceId};
use crate::{BufferId, Editor, WindowId};
use mica_driver::{
    CompioTaskDriver, DriverError, DriverEvent, DriverResources, ExternalRequestContext,
    ExternalRequestFuture, ExternalRequestHandler, FileinMode, Identity, RelationAcceleration,
    Symbol, TaskId, TaskLimits, Value,
};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const CORE_SOURCE: &str = include_str!("../../mica/roe-model.mica");
const FIRST_WAVE_SOURCE: &str = include_str!("../../mica/roe-first-wave.mica");
const EVENT_QUEUE_CAPACITY: usize = 256;
const EXTERNAL_REQUEST_CAPACITY: usize = 16;
const SUBSCRIPTION_QUEUE_BUDGET: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum MicaHostError {
    #[error("Mica driver failed: {0}")]
    Driver(#[from] DriverError),
    #[error("Mica session has no logical identity for the active Rust object")]
    MissingIdentity,
    #[error("Mica session host is already closed")]
    Closed,
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
    Handled {
        effects: Vec<MicaPresentationEffect>,
    },
    Failed(String),
}

struct NativeBridge {
    kernel: Arc<Mutex<NativeKernel>>,
    state: Mutex<NativeBridgeState>,
}

#[derive(Default)]
struct NativeBridgeState {
    actor: Option<Identity>,
    resources: HashMap<Identity, ResourceId>,
}

impl NativeBridge {
    fn new(kernel: Arc<Mutex<NativeKernel>>) -> Self {
        Self {
            kernel,
            state: Mutex::new(NativeBridgeState::default()),
        }
    }

    fn configure(&self, actor: Identity, resources: HashMap<Identity, ResourceId>) {
        let mut state = self.state.lock().unwrap();
        state.actor = Some(actor);
        state.resources = resources;
    }

    fn add_resource(&self, buffer: Identity, resource: ResourceId) {
        self.state
            .lock()
            .unwrap()
            .resources
            .insert(buffer, resource);
    }

    fn handle(&self, context: ExternalRequestContext, service: Symbol, payload: Value) -> Value {
        if context.cancellation.is_cancelled() {
            return native_error("request cancelled before native admission");
        }
        let state = self.state.lock().unwrap();
        if context.actor != state.actor {
            return native_error("request actor does not own this Roe endpoint");
        }

        let operation = if service == sym("clock_millis") {
            NativeOperation::ReadClockMillis
        } else if service == sym("text_insert") {
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
        } else if service == sym("test_pending") {
            return native_error("test_pending must be handled asynchronously");
        } else {
            return native_error("unknown Roe native service");
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
    active_view: WindowId,
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
        driver.open_endpoint_with_context_and_volatile_tuples_named(
            endpoint,
            None,
            Some(actor),
            sym("roe/session-v1"),
            tuples,
        )?;
        bridge.configure(actor, bridge_resources);

        Ok(Self {
            driver,
            bridge,
            endpoint,
            actor,
            session,
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
            active_view,
            closed: false,
        })
    }

    pub async fn dispatch_key(
        &mut self,
        editor: &Editor,
        resource_ids: &HashMap<BufferId, ResourceId>,
        sequence: String,
    ) -> Result<MicaKeyResult, MicaHostError> {
        if self.closed {
            return Err(MicaHostError::Closed);
        }
        self.synchronize_context(editor, resource_ids)?;
        let submitted = self
            .driver
            .submit_invocation_for_endpoint(
                self.endpoint,
                sym("roe/dispatch_key"),
                vec![
                    (sym("actor"), Value::identity(self.actor)),
                    (sym("session"), Value::identity(self.session)),
                    (sym("sequence"), Value::string(sequence)),
                ],
            )
            .await?;
        self.wait_for_task(submitted.task_id).await
    }

    pub async fn replace_first_wave(&self, source: String) -> Result<(), MicaHostError> {
        self.driver.check_filein(source.clone(), None).await?;
        self.driver
            .filein_unit(sym("roe/first-wave"), source, FileinMode::Replace, None)
            .await?;
        // Filein effects, if a future replacement adds any, remain on the one
        // driver stream and are consumed here rather than by a second reader.
        self.driver.drain_events();
        Ok(())
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

        if let std::collections::hash_map::Entry::Vacant(entry) = self.view_ids.entry(active) {
            let view = self.driver.allocate_ephemeral_identity()?;
            entry.insert(view);
            self.view_buffers.insert(active, window.active_buffer);
            self.view_cursors.insert(active, window.cursor);
            assert.push((sym("roe/View"), [Value::identity(view)].into()));
        }
        let view = self.view_ids[&active];
        if self.active_view != active {
            retract.push((
                sym("roe/ActiveView"),
                [
                    Value::identity(self.session),
                    Value::identity(self.view_ids[&self.active_view]),
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
        Ok(())
    }

    async fn wait_for_task(&mut self, task_id: TaskId) -> Result<MicaKeyResult, MicaHostError> {
        let mut effects = Vec::new();
        loop {
            let events = {
                let ready = self.driver.drain_events();
                if ready.is_empty() {
                    self.driver.wait_events().await
                } else {
                    ready
                }
            };
            for event in events {
                match event {
                    DriverEvent::Effect(effect) => {
                        if let Some(effect) = self.presentation_effect(&effect.value) {
                            effects.push(effect);
                        }
                    }
                    DriverEvent::TaskCompleted {
                        task_id: completed,
                        value,
                    } if completed == task_id => {
                        if value.as_symbol() == Some(sym("unbound")) {
                            return Ok(MicaKeyResult::Unbound);
                        }
                        return Ok(MicaKeyResult::Handled { effects });
                    }
                    DriverEvent::TaskAborted {
                        task_id: aborted,
                        error,
                    } if aborted == task_id => {
                        return Ok(MicaKeyResult::Failed(format!(
                            "Mica command aborted: {}",
                            self.driver.format_value(&error)
                        )));
                    }
                    DriverEvent::TaskFailed {
                        task_id: failed,
                        error,
                    } if failed == task_id => {
                        return Ok(MicaKeyResult::Failed(format!(
                            "Mica command failed: {error}"
                        )));
                    }
                    DriverEvent::TaskCancelled {
                        task_id: cancelled,
                        reason,
                    } if cancelled == task_id => {
                        return Ok(MicaKeyResult::Failed(format!(
                            "Mica command cancelled: {reason:?}"
                        )));
                    }
                    _ => {}
                }
            }
        }
    }

    fn presentation_effect(&mut self, value: &Value) -> Option<MicaPresentationEffect> {
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

    pub async fn close(&mut self) -> Result<Vec<TaskId>, MicaHostError> {
        if self.closed {
            return Ok(Vec::new());
        }
        self.closed = true;
        self.driver.drain_events();
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
            self.driver.drain_events();
            compio::time::sleep(Duration::from_millis(1)).await;
        }
        let report = close
            .await
            .map_err(|_| DriverError::Join("endpoint close task panicked".to_owned()))??;
        self.driver.drain_events();

        let shutdown_driver = self.driver.clone();
        let shutdown = compio::runtime::spawn(async move { shutdown_driver.shutdown().await });
        while !shutdown.is_finished() {
            self.driver.drain_events();
            compio::time::sleep(Duration::from_millis(1)).await;
        }
        shutdown
            .await
            .map_err(|_| DriverError::Join("driver shutdown task panicked".to_owned()))??;
        self.driver.drain_events();
        Ok(report.cancelled_tasks)
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

pub fn normalized_key_sequence(keys: &[crate::keys::LogicalKey]) -> String {
    let mut result = Vec::new();
    let mut index = 0;
    while index < keys.len() {
        if matches!(keys[index], crate::keys::LogicalKey::Modifier(_)) && index + 1 < keys.len() {
            result.push(format!(
                "{}-{}",
                keys[index].as_display_string(),
                keys[index + 1].as_display_string()
            ));
            index += 2;
        } else {
            result.push(keys[index].as_display_string());
            index += 1;
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
    }
}
