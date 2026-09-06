// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn mica_file_prompt_descends_directories_before_opening_a_file() {
    let _guard = MICA_TEST_LOCK.lock().unwrap();
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory_name = format!("roe-picker-{}-{unique}", std::process::id());
        let directory = PathBuf::from(&directory_name);
        let nested = directory.join("nested");
        let file = nested.join("inside.txt");
        std::fs::create_dir(&directory).unwrap();
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(&file, "nested contents").unwrap();

        let mut session =
            test_mica_client(test_editor(), CapabilityGrants::editor_default()).unwrap();
        let opened_prompt = session
            .dispatch(session.envelope(InputEvent::Keys(vec![
                control(),
                LogicalKey::AlphaNumeric('x'),
                control(),
                LogicalKey::AlphaNumeric('f'),
            ])))
            .await
            .unwrap();
        let prompt_view = snapshot(&opened_prompt)
            .views
            .iter()
            .find(|view| view.command_view)
            .unwrap_or_else(|| panic!("{opened_prompt:#?}"));
        assert!(
            prompt_view
                .visible_text
                .contains(&format!("{directory_name}/"))
        );
        let initial_header = prompt_view.visible_text.lines().next().unwrap().to_owned();

        session
            .dispatch(session.envelope(InputEvent::Text(directory_name.clone())))
            .await
            .unwrap();
        let descended = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
            .await
            .unwrap();
        let descended_prompt = snapshot(&descended)
            .views
            .iter()
            .find(|view| view.command_view)
            .unwrap();
        assert!(descended_prompt.visible_text.contains("nested/"));
        assert!(descended_prompt.visible_text.contains("../"));
        let descended_header = descended_prompt
            .visible_text
            .lines()
            .next()
            .unwrap()
            .to_owned();

        let ascended = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
            .await
            .unwrap();
        let ascended_header = snapshot(&ascended)
            .views
            .iter()
            .find(|view| view.command_view)
            .unwrap()
            .visible_text
            .lines()
            .next()
            .unwrap();
        assert_eq!(ascended_header, initial_header);

        session
            .dispatch(session.envelope(InputEvent::Text(directory_name.clone())))
            .await
            .unwrap();
        let redescended = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
            .await
            .unwrap();
        let redescended_header = snapshot(&redescended)
            .views
            .iter()
            .find(|view| view.command_view)
            .unwrap()
            .visible_text
            .lines()
            .next()
            .unwrap();
        assert_eq!(redescended_header, descended_header);

        session
            .dispatch(session.envelope(InputEvent::Text("nested".to_owned())))
            .await
            .unwrap();
        let nested_descent = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
            .await
            .unwrap();
        assert!(
            snapshot(&nested_descent)
                .views
                .iter()
                .find(|view| view.command_view)
                .unwrap()
                .visible_text
                .contains("inside.txt")
        );

        session
            .dispatch(session.envelope(InputEvent::Text("inside".to_owned())))
            .await
            .unwrap();
        let opened_file = session
            .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Enter])))
            .await
            .unwrap();
        assert!(
            snapshot(&opened_file)
                .views
                .iter()
                .any(|view| !view.command_view && view.visible_text == "nested contents")
        );
        assert!(opened_file.lifecycle.iter().all(|event| !matches!(
            event,
            LifecycleEvent::Error(message) if message.contains("Is a directory")
        )));

        session.terminate_workspace().await.unwrap();
        std::fs::remove_file(file).unwrap();
        std::fs::remove_dir(nested).unwrap();
        std::fs::remove_dir(directory).unwrap();
    });
}
