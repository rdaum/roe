use roe_core::native_kernel::{ResourceId, TextSelection, ViewId};
use roe_core::session::{
    Invalidation, PresentationDelta, PresentationSnapshot, PresentationUpdate, PresentedView,
    Revision, SessionEpoch, ViewGeometry, ViewScroll,
};
use roe_terminal::TerminalRenderer;
use roe_vello::VelloRenderer;

fn snapshot(revision: u64, text: &str) -> PresentationSnapshot {
    PresentationSnapshot {
        epoch: SessionEpoch(41),
        revision: Revision(revision),
        columns: 80,
        rows: 23,
        active_view: ViewId(1),
        views: vec![PresentedView {
            id: ViewId(1),
            resource: ResourceId {
                slot: 0,
                generation: 1,
            },
            name: "*session*".to_string(),
            visible_text: text.to_string(),
            visible_start_char: 0,
            visible_end_char: text.chars().count(),
            total_lines: 1,
            max_line_chars: text.chars().count(),
            cursor: text.chars().count(),
            selection: Some(TextSelection {
                anchor: 0,
                active: text.chars().count(),
            }),
            geometry: ViewGeometry {
                x: 0,
                y: 0,
                columns: 80,
                rows: 23,
            },
            scroll: ViewScroll {
                start_line: 0,
                start_column: 0,
            },
            active: true,
            command_view: false,
            show_gutter: false,
            modeline: "*session* (fundamental) 1:1".to_string(),
            styled_ranges: Vec::new(),
        }],
        styles: Vec::new(),
        echo_area: String::new(),
    }
}

#[test]
fn terminal_and_vello_consume_the_same_revisioned_session_stream() {
    let full = PresentationUpdate::Full(snapshot(1, "one"));
    let delta = PresentationUpdate::Delta(PresentationDelta {
        epoch: SessionEpoch(41),
        base_revision: Revision(1),
        revision: Revision(2),
        invalidations: vec![Invalidation::Full],
        snapshot: snapshot(2, "two"),
    });

    let mut terminal = TerminalRenderer::new(Vec::new());
    let mut vello = VelloRenderer::new();
    terminal.apply_session_presentation(&full).unwrap();
    vello.apply_session_presentation(&full).unwrap();
    terminal.apply_session_presentation(&delta).unwrap();
    vello.apply_session_presentation(&delta).unwrap();

    assert_eq!(
        terminal.session_presentation().current(),
        vello.session_presentation().current()
    );

    let gap = PresentationUpdate::Delta(PresentationDelta {
        epoch: SessionEpoch(41),
        base_revision: Revision(4),
        revision: Revision(5),
        invalidations: vec![Invalidation::Full],
        snapshot: snapshot(5, "gap"),
    });
    assert!(terminal.apply_session_presentation(&gap).is_err());
    assert!(vello.apply_session_presentation(&gap).is_err());
}
