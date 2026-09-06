// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn mica_background_completion_is_pumped_by_idle_timer() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let mut session = test_mica_client_with_clock(
            test_editor(),
            CapabilityGrants::editor_default(),
            Arc::new(FixedNativeClock(42)),
        )
        .unwrap();
        let task = session
            .workspace
            .mica
            .as_mut()
            .unwrap()
            .start_background_test_task()
            .await
            .unwrap();
        let next_input = session.next_sequence();
        compio::time::sleep(std::time::Duration::from_millis(40)).await;

        let idle = session.poll_output().await.unwrap().unwrap();
        assert_eq!(idle.acknowledged_input, None);
        assert_eq!(session.next_sequence(), next_input);
        assert!(idle.presentation.is_some());
        assert_eq!(snapshot(&idle).views[0].visible_text, "hello");
        assert!(idle.lifecycle.iter().all(|event| !matches!(
            event,
            LifecycleEvent::Error(message) if message.contains(&task.to_string())
        )));

        session.terminate_workspace().await.unwrap();
    });
}
