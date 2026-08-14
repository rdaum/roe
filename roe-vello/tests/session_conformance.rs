use roe_core::editor::{WindowNode, WindowType};
use roe_core::file_watcher::FileWatcher;
use roe_core::keys::LogicalKey;
use roe_core::kill_ring::KillRing;
use roe_core::native_kernel::CapabilityGrants;
use roe_core::native_services::SystemClock;
use roe_core::session::{
    AttachmentConfiguration, DirectSessionClient, InputEvent, PresentationUpdate, SessionClient,
    WorkspaceHost,
};
use roe_core::{Buffer, BufferId, Editor, Frame, Window, WindowId};
use roe_terminal::TerminalRenderer;
use roe_vello::VelloRenderer;
use slotmap::SlotMap;
use std::sync::Arc;

fn editor_fixture() -> Editor {
    let mut buffers: SlotMap<BufferId, Buffer> = SlotMap::default();
    let buffer = Buffer::named("*conformance*", roe_core::buffer::BufferKind::Ordinary);
    buffer.load_str("one λ");
    let buffer_id = buffers.insert(buffer);
    let mut windows: SlotMap<WindowId, Window> = SlotMap::default();
    let window_id = windows.insert(Window {
        x: 0,
        y: 0,
        width_chars: 80,
        height_chars: 23,
        active_buffer: buffer_id,
        cursor: 5,
        window_type: WindowType::Normal,
    });
    Editor {
        frame: Frame::new(80, 23),
        buffers,
        windows,
        active_window: window_id,
        window_tree: WindowNode::new_leaf(window_id),
        kill_ring: KillRing::with_capacity(60),
        previous_active_window: None,
        buffer_history: vec![buffer_id],
        echo_message: String::new(),
        echo_message_time: None,
        clock: Arc::new(SystemClock),
        mouse_drag_state: None,
        messages_buffer_id: None,
        file_watcher: FileWatcher::new(),
    }
}

#[test]
fn terminal_and_vello_consume_the_same_production_mica_session_stream() {
    compio::runtime::Runtime::new().unwrap().block_on(async {
        let workspace =
            WorkspaceHost::open_with_mica(editor_fixture(), CapabilityGrants::editor_default())
                .unwrap();
        let mut session =
            DirectSessionClient::new(workspace, AttachmentConfiguration::headless(80, 23));
        let mut outputs = vec![session.initial_output().await];
        outputs.push(
            session
                .dispatch(session.envelope(InputEvent::Text("x".to_owned())))
                .await
                .unwrap(),
        );
        outputs.push(
            session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Backspace])))
                .await
                .unwrap(),
        );
        outputs.push(
            session
                .dispatch(session.envelope(InputEvent::Keys(vec![LogicalKey::Function(12)])))
                .await
                .unwrap(),
        );

        assert!(matches!(
            outputs[0].presentation,
            Some(PresentationUpdate::Full(_))
        ));
        assert!(
            outputs[1..]
                .iter()
                .all(|output| matches!(output.presentation, Some(PresentationUpdate::Delta(_))))
        );

        let mut terminal = TerminalRenderer::new(Vec::new());
        let mut vello = VelloRenderer::new();
        for output in outputs {
            let update = output.presentation.as_ref().unwrap();
            terminal.apply_session_presentation(update).unwrap();
            terminal.render_session().unwrap();
            vello.apply_session_presentation(update).unwrap();
            assert_eq!(
                terminal.session_presentation().current(),
                vello.session_presentation().current()
            );
        }

        let current = terminal.session_presentation().current().unwrap();
        assert!(current.views[0].visible_text.starts_with("one λ"));
        assert!(current.views[0].visible_text.len() > "one λ".len());
        session.terminate_workspace().await.unwrap();
    });
}
