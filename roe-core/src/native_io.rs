// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Owned, bounded external I/O. Admission requires native authorization.
//! No operation retains a kernel mutex or moves a Compio runtime between threads.

use crate::native_kernel::{
    Capability, KernelError, NativeDirectoryEntry, NativeOperation, NativeResult,
};
use compio::buf::BufResult;
use compio::io::{AsyncRead, AsyncReadAt, AsyncWriteAtExt};
use futures_util::future::{Either, select, try_join3};
use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::time::Duration;

/// The only asynchronous kernel entry. Authorization and admission finish before await.
pub(crate) async fn execute(
    kernel: &Arc<Mutex<crate::native_kernel::NativeKernel>>,
    operation: NativeOperation,
    cancellation: impl Future<Output = ()>,
) -> Result<NativeResult, KernelError> {
    if capability(&operation).is_none() {
        return kernel.lock().unwrap().execute(operation);
    }
    let request = kernel.lock().unwrap().prepare_io(operation)?;
    request.run(cancellation).await
}

pub const MAX_IO_REQUESTS: usize = 16;
pub const MAX_IO_BYTES: usize = 1_048_576;
const MAX_ARGUMENTS: usize = 256;
use crate::native_kernel::MAX_DIRECTORY_ENTRIES;
const MAX_DIRECTORY_SCAN: usize = 65_536;
const IO_TIMEOUT: Duration = Duration::from_secs(30);
const CHUNK_BYTES: usize = 8192;

#[derive(Default)]
struct Cancellation {
    cancelled: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl Cancellation {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let waker = self.waker.lock().unwrap().take();
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    async fn wait(&self) {
        std::future::poll_fn(|context| {
            let mut waker = self.waker.lock().unwrap();
            if self.cancelled.load(Ordering::Acquire) {
                return Poll::Ready(());
            }
            *waker = Some(context.waker().clone());
            Poll::Pending
        })
        .await
    }
}

#[derive(Default)]
struct OwnerState {
    closed: bool,
    next_id: u64,
    active: HashMap<u64, Arc<Cancellation>>,
    closed_waker: Option<Waker>,
}

#[derive(Clone, Default)]
pub(crate) struct IoOwner(Arc<Mutex<OwnerState>>);

struct Lease {
    owner: IoOwner,
    id: u64,
    cancellation: Arc<Cancellation>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut state = self.owner.0.lock().unwrap();
        state.active.remove(&self.id);
        let waker = state.closed_waker.take();
        drop(state);
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// Cancelling a caller also cancels cooperative blocking work that still owns a lease.
struct CancelOnDrop(Arc<Cancellation>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl IoOwner {
    fn admit(&self) -> Result<Arc<Lease>, KernelError> {
        let mut state = self.0.lock().unwrap();
        if state.closed {
            return Err(cancelled());
        }
        if state.active.len() >= MAX_IO_REQUESTS {
            return Err(KernelError::IoLimit(
                "concurrent I/O request limit reached".into(),
            ));
        }
        let id = state.next_id;
        state.next_id = state
            .next_id
            .checked_add(1)
            .ok_or_else(|| KernelError::IoLimit("I/O request identity exhausted".into()))?;
        let cancellation = Arc::new(Cancellation::default());
        state.active.insert(id, cancellation.clone());
        Ok(Arc::new(Lease {
            owner: self.clone(),
            id,
            cancellation,
        }))
    }

    pub(crate) fn cancel_all(&self) {
        let cancellations: Vec<_> = {
            let mut state = self.0.lock().unwrap();
            state.closed = true;
            state.active.values().cloned().collect()
        };
        for cancellation in cancellations {
            cancellation.cancel();
        }
    }

    pub(crate) async fn close(&self) {
        self.cancel_all();
        // Blocking directory work retains its lease until its last OS call returns.
        // Shutdown waits for that work instead of detaching it.
        std::future::poll_fn(|context| {
            let mut state = self.0.lock().unwrap();
            if state.active.is_empty() {
                return Poll::Ready(());
            }
            state.closed_waker = Some(context.waker().clone());
            Poll::Pending
        })
        .await
    }
}

pub(crate) struct IoRequest {
    operation: NativeOperation,
    lease: Arc<Lease>,
}

pub(crate) fn capability(operation: &NativeOperation) -> Option<Capability> {
    match operation {
        NativeOperation::ReadFile { .. } | NativeOperation::ListDirectory { .. } => {
            Some(Capability::FileRead)
        }
        NativeOperation::WriteFile { .. } => Some(Capability::FileWrite),
        NativeOperation::SpawnProcess { .. } => Some(Capability::ProcessSpawn),
        _ => None,
    }
}

impl IoRequest {
    pub(crate) fn prepare(
        owner: &IoOwner,
        operation: NativeOperation,
    ) -> Result<Self, KernelError> {
        match &operation {
            NativeOperation::ReadFile { path }
            | NativeOperation::ListDirectory { path }
            | NativeOperation::WriteFile { path, .. }
                if path.as_os_str().len() > 65_536 =>
            {
                return Err(KernelError::IoLimit("I/O path exceeds 65,536 bytes".into()));
            }
            NativeOperation::WriteFile { contents, .. } if contents.len() > MAX_IO_BYTES => {
                return Err(KernelError::IoLimit(
                    "file write exceeds the byte limit".into(),
                ));
            }
            NativeOperation::SpawnProcess { program, args }
                if args.len() > MAX_ARGUMENTS
                    || args.iter().fold(program.len(), |total, argument| {
                        total.saturating_add(argument.len())
                    }) > 65_536 =>
            {
                return Err(KernelError::IoLimit(
                    "process arguments exceed their count or byte limit".into(),
                ));
            }
            _ => {}
        }
        Ok(Self {
            operation,
            lease: owner.admit()?,
        })
    }

    pub(crate) async fn run(
        self,
        external_cancellation: impl Future<Output = ()>,
    ) -> Result<NativeResult, KernelError> {
        let Self { operation, lease } = self;
        let _cancel_on_drop = CancelOnDrop(lease.cancellation.clone());
        let cancellation = async {
            let external_or_timeout = select(
                Box::pin(external_cancellation),
                Box::pin(compio::time::sleep(IO_TIMEOUT)),
            );
            select(
                Box::pin(lease.cancellation.wait()),
                Box::pin(external_or_timeout),
            )
            .await;
        };
        if let NativeOperation::SpawnProcess { program, args } = operation {
            return run_process(program, args, cancellation).await;
        }
        let work = async {
            match operation {
                NativeOperation::ReadFile { path } => read_text(&path, MAX_IO_BYTES)
                    .await
                    .map(NativeResult::FileContents),
                NativeOperation::WriteFile { path, contents } => {
                    write_text(&path, contents, MAX_IO_BYTES).await?;
                    Ok(NativeResult::FileWritten)
                }
                NativeOperation::ListDirectory { path } => {
                    let lease = lease.clone();
                    // This task never owns a runtime or kernel. Its retained result and
                    // scan are bounded, and shutdown waits for its ownership lease.
                    compio::runtime::spawn_blocking(move || list_directory(path, &lease))
                        .await
                        .map_err(|_| {
                            KernelError::Io(std::io::Error::other("directory worker panicked"))
                        })?
                }
                _ => Err(KernelError::IoRequired),
            }
        };
        match select(Box::pin(cancellation), Box::pin(work)).await {
            Either::Left(_) => Err(cancelled()),
            Either::Right((result, _)) => result,
        }
    }
}

pub(crate) async fn read_text(path: &Path, limit: usize) -> Result<String, KernelError> {
    match select(
        Box::pin(compio::time::sleep(IO_TIMEOUT)),
        Box::pin(read_text_chunks(path, limit)),
    )
    .await
    {
        Either::Left(_) => Err(cancelled()),
        Either::Right((result, _)) => result,
    }
}

async fn read_text_chunks(path: &Path, limit: usize) -> Result<String, KernelError> {
    let file = compio::fs::File::open(path).await?;
    let mut contents = Vec::new();
    loop {
        let remaining = limit.saturating_sub(contents.len());
        let chunk = vec![0; CHUNK_BYTES.min(remaining.saturating_add(1))];
        let BufResult(result, chunk) = file.read_at(chunk, contents.len() as u64).await;
        let count = result?;
        if count == 0 {
            break;
        }
        if count > remaining {
            return Err(KernelError::IoLimit(
                "file read exceeds the byte limit".into(),
            ));
        }
        contents.extend_from_slice(&chunk[..count]);
    }
    String::from_utf8(contents).map_err(|error| {
        KernelError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })
}

pub(crate) async fn write_text(
    path: &Path,
    contents: String,
    limit: usize,
) -> Result<(), KernelError> {
    if contents.len() > limit {
        return Err(KernelError::IoLimit(
            "file write exceeds the byte limit".into(),
        ));
    }
    let write = async {
        let mut file = compio::fs::File::create(path).await?;
        let BufResult(result, _) = file.write_all_at(contents.into_bytes(), 0).await;
        result?;
        Ok(())
    };
    match select(Box::pin(compio::time::sleep(IO_TIMEOUT)), Box::pin(write)).await {
        Either::Left(_) => Err(cancelled()),
        Either::Right((result, _)) => result,
    }
}

fn list_directory(path: PathBuf, lease: &Lease) -> Result<NativeResult, KernelError> {
    let directory = std::fs::canonicalize(path)?;
    let mut paths = BTreeSet::new();
    let mut bytes = directory.as_os_str().len();
    for (index, entry) in std::fs::read_dir(&directory)?.enumerate() {
        if lease.cancellation.cancelled.load(Ordering::Acquire) {
            return Err(cancelled());
        }
        if index >= MAX_DIRECTORY_SCAN {
            return Err(KernelError::IoLimit(
                "directory scan exceeds 65,536 entries".into(),
            ));
        }
        let path = entry?.path();
        bytes = bytes.saturating_add(path.as_os_str().len());
        paths.insert(path);
        if paths.len() > MAX_DIRECTORY_ENTRIES
            && let Some(path) = paths.pop_last()
        {
            bytes = bytes.saturating_sub(path.as_os_str().len());
        }
        if bytes > MAX_IO_BYTES {
            return Err(KernelError::IoLimit(
                "directory paths exceed the byte limit".into(),
            ));
        }
    }
    let mut entries = Vec::with_capacity(paths.len());
    let mut bytes = directory.as_os_str().len();
    for path in paths {
        if lease.cancellation.cancelled.load(Ordering::Acquire) {
            return Err(cancelled());
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        bytes = bytes
            .saturating_add(path.as_os_str().len())
            .saturating_add(name.len());
        if bytes > MAX_IO_BYTES {
            return Err(KernelError::IoLimit(
                "directory result exceeds the byte limit".into(),
            ));
        }
        entries.push(NativeDirectoryEntry {
            name,
            is_directory: path.is_dir(),
            path,
        });
    }
    Ok(NativeResult::DirectoryEntries { directory, entries })
}

struct ProcessGuard(Option<std::process::Child>);
impl ProcessGuard {
    async fn terminate(&mut self) -> Result<(), KernelError> {
        if let Some(child) = self.0.as_mut() {
            if child.try_wait()?.is_none() {
                child.kill()?;
                while child.try_wait()?.is_none() {
                    compio::time::sleep(Duration::from_millis(2)).await;
                }
            }
            self.0 = None;
        }
        Ok(())
    }
}
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        // Unwinding or dropping the calling future must not leave a live child or
        // zombie. Normal completion and explicit cancellation use async cleanup.
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

async fn read_pipe(pipe: &mut impl AsyncRead, total: &AtomicUsize) -> Result<Vec<u8>, KernelError> {
    let mut contents = Vec::new();
    loop {
        let BufResult(result, chunk) = pipe.read(vec![0; CHUNK_BYTES]).await;
        let count = result?;
        if count == 0 {
            return Ok(contents);
        }
        let previous = total.fetch_add(count, Ordering::AcqRel);
        if previous.saturating_add(count) > MAX_IO_BYTES {
            return Err(KernelError::IoLimit(
                "process output exceeds the combined byte limit".into(),
            ));
        }
        contents.extend_from_slice(&chunk[..count]);
    }
}

async fn run_process(
    program: String,
    args: Vec<String>,
    cancellation: impl Future<Output = ()>,
) -> Result<NativeResult, KernelError> {
    // Check cancellation before the first externally observable action.
    let spawn = async {
        std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
    };
    let mut cancellation = Box::pin(cancellation);
    let child = match select(cancellation.as_mut(), Box::pin(spawn)).await {
        Either::Left(_) => return Err(cancelled()),
        Either::Right((child, _)) => child?,
    };
    let mut guard = ProcessGuard(Some(child));
    let child = guard.0.as_mut().expect("new process guard owns its child");
    let mut stdout = compio::fs::AsyncFd::new(child.stdout.take().expect("stdout was piped"))?;
    let mut stderr = compio::fs::AsyncFd::new(child.stderr.take().expect("stderr was piped"))?;
    let total = AtomicUsize::new(0);
    let result = {
        let wait = async {
            loop {
                if let Some(status) = child.try_wait()? {
                    return Ok::<_, KernelError>(status.code());
                }
                compio::time::sleep(Duration::from_millis(2)).await;
            }
        };
        let work = try_join3(
            read_pipe(&mut stdout, &total),
            read_pipe(&mut stderr, &total),
            wait,
        );
        match select(cancellation, Box::pin(work)).await {
            Either::Left(_) => Err(cancelled()),
            Either::Right((result, _)) => result,
        }
    };
    // Release pipe reads before killing and reaping the process.
    drop(stdout);
    drop(stderr);
    let cleanup = guard.terminate().await;
    match result {
        Ok((stdout, stderr, status)) => {
            cleanup?;
            Ok(NativeResult::ProcessOutput {
                status,
                stdout,
                stderr,
            })
        }
        Err(error) => {
            if let Err(cleanup) = cleanup {
                tracing::warn!(%cleanup, "process cleanup failed");
            }
            Err(error)
        }
    }
}

fn cancelled() -> KernelError {
    KernelError::Io(std::io::Error::new(
        std::io::ErrorKind::Interrupted,
        "native I/O was cancelled or exceeded its 30-second deadline",
    ))
}

#[cfg(test)]
mod tests;
