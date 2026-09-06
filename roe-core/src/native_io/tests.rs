// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use crate::native_kernel::{CapabilityGrants, NativeKernel};
use std::future::pending;

fn kernel(capabilities: impl IntoIterator<Item = Capability>) -> Arc<Mutex<NativeKernel>> {
    Arc::new(Mutex::new(NativeKernel::new(CapabilityGrants::new(
        capabilities,
    ))))
}

#[test]
fn admission_is_authorized_bounded_and_released() {
    let denied = kernel([]);
    assert!(matches!(
        denied
            .lock()
            .unwrap()
            .prepare_io(NativeOperation::ReadFile {
                path: "nonexistent".into(),
            }),
        Err(KernelError::CapabilityDenied(Capability::FileRead))
    ));
    let granted = kernel([Capability::FileRead]);
    let mut requests = Vec::new();
    for _ in 0..MAX_IO_REQUESTS {
        requests.push(
            granted
                .lock()
                .unwrap()
                .prepare_io(NativeOperation::ReadFile {
                    path: "nonexistent".into(),
                })
                .unwrap(),
        );
    }
    assert!(matches!(
        granted
            .lock()
            .unwrap()
            .prepare_io(NativeOperation::ReadFile {
                path: "nonexistent".into(),
            }),
        Err(KernelError::IoLimit(_))
    ));
    requests.clear();
    assert!(
        granted
            .lock()
            .unwrap()
            .prepare_io(NativeOperation::ReadFile {
                path: "nonexistent".into(),
            })
            .is_ok()
    );
}

#[test]
fn file_limits_and_pre_cancel_preserve_the_destination() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("contents");
        std::fs::write(&path, "keep").unwrap();
        let kernel = kernel([Capability::FileRead, Capability::FileWrite]);
        assert!(matches!(execute(&kernel, NativeOperation::WriteFile {
            path: path.clone(), contents: "x".repeat(MAX_IO_BYTES + 1),
        }, pending()).await, Err(KernelError::IoLimit(_))));
        assert!(execute(&kernel, NativeOperation::WriteFile {
            path: path.clone(), contents: "replace".into(),
        }, std::future::ready(())).await.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep");
        std::fs::write(&path, vec![b'x'; MAX_IO_BYTES]).unwrap();
        assert!(matches!(execute(&kernel, NativeOperation::ReadFile { path: path.clone() }, pending()).await,
            Ok(NativeResult::FileContents(text)) if text.len() == MAX_IO_BYTES));
        std::fs::write(&path, vec![b'x'; MAX_IO_BYTES + 1]).unwrap();
        assert!(matches!(execute(&kernel, NativeOperation::ReadFile { path }, pending()).await,
            Err(KernelError::IoLimit(_))));
        let owner = kernel.lock().unwrap().io_owner();
        owner.close().await;
    });
}

#[test]
fn directory_results_are_sorted_and_bounded() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        for index in (0..MAX_DIRECTORY_ENTRIES + 4).rev() {
            std::fs::write(directory.path().join(format!("{index:04}")), "").unwrap();
        }
        let kernel = kernel([Capability::FileRead]);
        let result = execute(
            &kernel,
            NativeOperation::ListDirectory {
                path: directory.path().into(),
            },
            pending(),
        )
        .await
        .unwrap();
        let NativeResult::DirectoryEntries { entries, .. } = result else {
            panic!("wrong result")
        };
        assert_eq!(entries.len(), MAX_DIRECTORY_ENTRIES);
        assert_eq!(entries.first().unwrap().name, "0000");
        assert_eq!(entries.last().unwrap().name, "0255");
        assert!(entries.windows(2).all(|pair| pair[0].path < pair[1].path));
    });
}

#[test]
fn close_waits_for_owned_blocking_work_and_rejects_new_admission() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let owner = IoOwner::default();
        let lease = owner.admit().unwrap();
        let worker_finished = Arc::new(AtomicBool::new(false));
        let finished = worker_finished.clone();
        let worker = compio::runtime::spawn_blocking(move || {
            while !lease.cancellation.cancelled.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            std::thread::sleep(Duration::from_millis(20));
            finished.store(true, Ordering::Release);
            drop(lease);
        });
        owner.close().await;
        assert!(worker_finished.load(Ordering::Acquire));
        assert!(owner.admit().is_err());
        worker.await.unwrap();
    });
}

#[cfg(unix)]
#[test]
fn process_streams_share_a_limit_and_report_exit_status() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let kernel = kernel([Capability::ProcessSpawn]);
        let result = execute(
            &kernel,
            NativeOperation::SpawnProcess {
                program: "sh".into(),
                args: vec!["-c".into(), "printf out; printf err >&2; exit 7".into()],
            },
            pending(),
        )
        .await
        .unwrap();
        assert_eq!(
            result,
            NativeResult::ProcessOutput {
                status: Some(7),
                stdout: b"out".to_vec(),
                stderr: b"err".to_vec()
            }
        );
        let result = execute(
            &kernel,
            NativeOperation::SpawnProcess {
                program: "sh".into(),
                args: vec![
                    "-c".into(),
                    "head -c 600000 /dev/zero; head -c 600000 /dev/zero >&2".into(),
                ],
            },
            pending(),
        )
        .await;
        assert!(matches!(result, Err(KernelError::IoLimit(_))), "{result:?}");
    });
}

#[cfg(target_os = "linux")]
#[test]
fn process_cancellation_reaps_child_without_holding_kernel_lock() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let pid_path = directory.path().join("pid");
        let kernel = kernel([Capability::ProcessSpawn]);
        let cancellation = async {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while !pid_path.exists() {
                assert!(kernel.try_lock().is_ok(), "external I/O retained the kernel lock");
                assert!(std::time::Instant::now() < deadline, "child did not start");
                compio::time::sleep(Duration::from_millis(2)).await;
            }
        };
        let result = execute(&kernel, NativeOperation::SpawnProcess {
            program: "sh".into(), args: vec!["-c".into(), "echo $$ > \"$1\"; exec sleep 60".into(), "roe-test".into(), pid_path.to_string_lossy().into_owned()],
        }, cancellation).await;
        assert!(matches!(result, Err(KernelError::Io(ref error)) if error.kind() == std::io::ErrorKind::Interrupted));
        let pid = std::fs::read_to_string(pid_path).unwrap();
        assert!(!Path::new("/proc").join(pid.trim()).exists(), "cancelled child was not reaped");
        let owner = kernel.lock().unwrap().io_owner();
        owner.close().await;
    });
}

#[cfg(target_os = "linux")]
#[test]
fn dropping_a_process_future_reaps_its_child_and_releases_admission() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let pid_path = directory.path().join("pid");
        let kernel = kernel([Capability::ProcessSpawn]);
        let owner = kernel.lock().unwrap().io_owner();
        let started = async {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while !pid_path.exists() {
                assert!(std::time::Instant::now() < deadline, "child did not start");
                compio::time::sleep(Duration::from_millis(2)).await;
            }
        };
        let request = execute(
            &kernel,
            NativeOperation::SpawnProcess {
                program: "sh".into(),
                args: vec![
                    "-c".into(),
                    "echo $$ > \"$1\"; exec sleep 60".into(),
                    "roe-test".into(),
                    pid_path.to_string_lossy().into_owned(),
                ],
            },
            pending(),
        );
        match select(Box::pin(started), Box::pin(request)).await {
            Either::Left(((), request)) => drop(request),
            Either::Right((result, _)) => {
                panic!("process completed before cancellation: {result:?}")
            }
        }
        let pid = std::fs::read_to_string(pid_path).unwrap();
        assert!(!Path::new("/proc").join(pid.trim()).exists());
        assert!(owner.0.lock().unwrap().active.is_empty());
        owner.close().await;
    });
}

#[cfg(unix)]
#[test]
fn workspace_io_close_cancels_an_active_process() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let kernel = kernel([Capability::ProcessSpawn]);
        let owner = kernel.lock().unwrap().io_owner();
        let request = execute(&kernel, NativeOperation::SpawnProcess {
            program: "sleep".into(), args: vec!["60".into()],
        }, pending());
        let close = async {
            compio::time::sleep(Duration::from_millis(10)).await;
            assert_eq!(owner.0.lock().unwrap().active.len(), 1);
            owner.close().await;
        };
        let (result, ()) = futures_util::future::join(request, close).await;
        assert!(matches!(result, Err(KernelError::Io(ref error)) if error.kind() == std::io::ErrorKind::Interrupted));
        assert!(owner.0.lock().unwrap().active.is_empty());
    });
}
