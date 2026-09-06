// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn mica_replacement_failure_and_recovery_remain_live() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_mica_client_with_clock(
            test_editor(),
            CapabilityGrants::editor_default(),
            Arc::new(FixedNativeClock(42)),
        )
        .unwrap();
        let original = include_str!("../../../../mica/roe-first-wave.mica");
        let replacement = original.replace(
            "let text = string_concat(to_literal(clock[:value]), \"\\n\")",
            "let text = string_concat(\"v2:\", to_literal(clock[:value]), \"\\n\")",
        );
        assert_ne!(replacement, original);
        session
            .workspace.replace_mica_first_wave(replacement.clone())
            .await
            .unwrap();
        assert!(session
            .workspace.export_mica_unit("roe/first-wave")
            .await
            .unwrap()
            .contains("v2:"));

        let replaced = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
            .await
            .unwrap();
        assert_eq!(snapshot(&replaced).views[0].visible_text, "hellov2:42\n");

        assert!(
            session
                .workspace.replace_mica_first_wave("verb this is malformed".to_owned())
                .await
                .is_err()
        );
        let retained = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
            .await
            .unwrap();
        assert_eq!(
            snapshot(&retained).views[0].visible_text,
            "hellov2:42\nv2:42\n"
        );

        session
            .workspace.set_mica_package_enabled("roe/core_package", false)
            .unwrap();
        let disabled = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
            .await
            .unwrap();
        assert_eq!(
            snapshot(&disabled).views[0].visible_text,
            "hellov2:42\nv2:42\n"
        );
        assert_eq!(snapshot(&disabled).echo_area, "F12 is undefined");
        session
            .workspace.set_mica_package_enabled("roe/core_package", true)
            .unwrap();

        let without_yellow = replacement.replace(
            "assert roe/FaceAttribute(#roe/isearch_current_face, :background, \"#ffff00\")\n",
            "",
        );
        session
            .workspace.replace_mica_first_wave(without_yellow)
            .await
            .unwrap();
        session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(1)])))
            .await
            .unwrap();
        assert!(!session.workspace.policy.faces["isearch-current"].contains_key("background"));

        let start = original.find("verb roe/insert_current_time").unwrap();
        let end = start + original[start..].find("\nend\n").unwrap() + "\nend\n".len();
        let failing = format!(
            "{}verb roe/insert_current_time(actor, session)\n  raise E_TEST, \"intentional command failure\"\nend\n{}",
            &original[..start],
            &original[end..]
        );
        session.workspace.replace_mica_first_wave(failing).await.unwrap();
        let failed = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
            .await
            .unwrap();
        assert!(failed.lifecycle.iter().any(|event| matches!(
            event,
            LifecycleEvent::Error(message) if message.contains("intentional command failure")
        )));
        assert!(snapshot(&failed)
            .echo_area
            .contains("intentional command failure"));
        assert!(failed.lifecycle.iter().any(|event| matches!(
            event,
            LifecycleEvent::Error(message)
                if message.contains("selector=roe/dispatch_key")
        )));

        session.workspace.restore_mica_first_wave().await.unwrap();
        let recovered = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
            .await
            .unwrap();
        assert_eq!(
            snapshot(&recovered).views[0].visible_text,
            "hellov2:42\nv2:42\n42\n"
        );
        session
            .terminate_workspace()
            .await
            .unwrap();
    });
}

#[test]
fn native_recovery_surface_operates_before_user_policy_load() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_mica_client_with_clock(
            test_editor(),
            CapabilityGrants::editor_default(),
            Arc::new(FixedNativeClock(42)),
        )
        .unwrap();

        let rejected = session
            .dispatch(
                session.envelope(InputEvent::Recovery(RecoveryOperation::CheckSource {
                    source: "verb this is malformed".to_owned(),
                })),
            )
            .await
            .unwrap();
        assert!(
            rejected.lifecycle.iter().any(|event| matches!(
                event,
                LifecycleEvent::RecoveryResult { result: Err(_), .. }
            ))
        );
        assert!(!session.workspace.terminated);

        let recovery_dir = std::env::temp_dir().join(format!(
            "roe-recovery-{}-{}",
            std::process::id(),
            session.epoch().0
        ));
        std::fs::create_dir(&recovery_dir).unwrap();
        let export_path = recovery_dir.join("first-wave.mica");
        let reports = session
            .workspace
            .execute_startup_recovery(&[
                StartupRecoveryOperation::Inspect,
                StartupRecoveryOperation::ExportUnit {
                    unit: "roe/first-wave".to_owned(),
                    path: export_path.clone(),
                },
            ])
            .await
            .unwrap();
        assert!(reports[0].contains("endpoint="));
        assert!(
            std::fs::read_to_string(&export_path)
                .unwrap()
                .contains("insert_current_time")
        );

        let installed = session
            .dispatch(
                session.envelope(InputEvent::Recovery(RecoveryOperation::ReplaceUnit {
                    unit: "roe/first-wave".to_owned(),
                    source: include_str!("../../../../mica/roe-first-wave.mica").to_owned(),
                })),
            )
            .await
            .unwrap();
        assert!(installed.lifecycle.iter().any(|event| matches!(
            event,
            LifecycleEvent::RecoveryResult {
                result: Ok(None),
                ..
            }
        )));

        let command = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
            .await
            .unwrap();
        assert_eq!(snapshot(&command).views[0].visible_text, "hello42\n");

        let exported = session
            .dispatch(
                session.envelope(InputEvent::Recovery(RecoveryOperation::ExportUnit {
                    unit: "roe/first-wave".to_owned(),
                })),
            )
            .await
            .unwrap();
        assert!(exported.lifecycle.iter().any(|event| matches!(
            event,
            LifecycleEvent::RecoveryResult { result: Ok(Some(source)), .. }
                if source.contains("insert_current_time")
        )));

        let reports = session
            .workspace
            .execute_startup_recovery(&[
                StartupRecoveryOperation::Inspect,
                StartupRecoveryOperation::ExportUnit {
                    unit: "roe/first-wave".to_owned(),
                    path: export_path.clone(),
                },
            ])
            .await
            .unwrap();
        assert!(reports[0].contains("endpoint="));
        assert!(
            std::fs::read_to_string(&export_path)
                .unwrap()
                .contains("insert_current_time")
        );
        std::fs::remove_file(export_path).unwrap();
        std::fs::remove_dir(recovery_dir).unwrap();

        session.terminate_workspace().await.unwrap();
    });
}

#[test]
fn mica_close_cancels_pending_request() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_mica_client_with_clock(
            test_editor(),
            CapabilityGrants::editor_default(),
            Arc::new(FixedNativeClock(7)),
        )
        .unwrap();
        let pending = session
            .workspace
            .mica
            .as_mut()
            .unwrap()
            .start_pending_test_request()
            .await
            .unwrap();
        let close = session.terminate_workspace().await.unwrap();
        assert!(
            close
                .lifecycle
                .contains(&LifecycleEvent::MicaTaskCancelled { task_id: pending })
        );
        assert!(
            close
                .lifecycle
                .contains(&LifecycleEvent::WorkspaceTerminated)
        );
    });
}
