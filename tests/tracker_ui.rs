use piv::{
    tracker::{IssueStatus, PlanIssueInput, PrdInput, PrdStatus, TrackerStore},
    tracker_ui::{
        TrackerViewState, max_tracker_detail_scroll, render_tracker_rows, render_tracker_view,
        render_tracker_view_lines, render_tracker_viewport,
    },
};
use ratatui::{style::{Color, Modifier}, text::Line};

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join("")
}

fn char_index(text: &str, needle: &str) -> Option<usize> {
    text.find(needle).map(|byte| text[..byte].chars().count())
}

#[test]
fn tracker_rows_render_project_prd_and_ordered_issues_with_blocker_glyphs() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &[]).unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "prd-tracker".into(),
                title: "PRD tracker".into(),
                status: PrdStatus::InProgress,
                body: None,
                source_uri: None,
            },
            vec![
                PlanIssueInput {
                    key: "sqlite-registry".into(),
                    title: "Create SQLite registry".into(),
                    status: IssueStatus::Complete,
                    body: None,
                    position: 1,
                    depends_on: vec![],
                },
                PlanIssueInput {
                    key: "socket-api".into(),
                    title: "Add tracker socket".into(),
                    status: IssueStatus::Open,
                    body: None,
                    position: 2,
                    depends_on: vec!["sqlite-registry".into()],
                },
                PlanIssueInput {
                    key: "prd-ui".into(),
                    title: "Add tracker UI".into(),
                    status: IssueStatus::Open,
                    body: None,
                    position: 3,
                    depends_on: vec!["socket-api".into()],
                },
            ],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let mut state = TrackerViewState::default();
    state.expand("project:piv");
    state.expand("prd:piv/prd-tracker");

    let rows = render_tracker_rows(&snapshot, &state, 100);

    assert_eq!(rows[0], "PRD Tracker");
    assert!(rows.iter().all(|row| !row.contains(":prd")));
    assert!(rows.iter().any(|row| row.contains("▾ piv")));
    assert!(rows.iter().any(|row| row.contains("▾ PRD tracker")
        && row.contains("● in progress")
        && row.contains("1/3")));
    assert!(
        rows.iter()
            .any(|row| row.contains("1. Create SQLite registry") && row.contains("✓ complete"))
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("2. Add tracker socket") && row.contains("◌ open"))
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("3. Add tracker UI")
                && row.contains("◆ blocked")
                && row.contains("blocked by socket-api"))
    );
}

#[test]
fn tracker_rows_render_all_prd_and_issue_status_badges() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &[]).unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "status-prd".into(),
                title: "Status PRD".into(),
                status: PrdStatus::Archived,
                body: None,
                source_uri: None,
            },
            vec![
                PlanIssueInput {
                    key: "active".into(),
                    title: "Active issue".into(),
                    status: IssueStatus::InProgress,
                    body: None,
                    position: 1,
                    depends_on: vec![],
                },
                PlanIssueInput {
                    key: "canceled".into(),
                    title: "Canceled issue".into(),
                    status: IssueStatus::Canceled,
                    body: None,
                    position: 2,
                    depends_on: vec![],
                },
            ],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let mut state = TrackerViewState::default();
    state.expand("project:piv");
    state.expand("prd:piv/status-prd");

    let rendered = render_tracker_rows(&snapshot, &state, 100).join("\n");

    assert!(rendered.contains("◇ archived"));
    assert!(rendered.contains("1. Active issue") && rendered.contains("● in progress"));
    assert!(rendered.contains("2. Canceled issue") && rendered.contains("− canceled"));
}

#[test]
fn tracker_rows_align_issue_prd_and_project_status_columns() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &[]).unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "alignment".into(),
                title: "Alignment".into(),
                status: PrdStatus::Complete,
                body: None,
                source_uri: None,
            },
            vec![PlanIssueInput {
                key: "finished".into(),
                title: "Finished issue".into(),
                status: IssueStatus::Complete,
                body: None,
                position: 1,
                depends_on: vec![],
            }],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let mut state = TrackerViewState::default();
    state.expand("project:piv");
    state.expand("prd:piv/alignment");

    let rows = render_tracker_rows(&snapshot, &state, 100);
    let project = rows.iter().find(|row| row.contains("▾ piv")).unwrap();
    let prd = rows.iter().find(|row| row.contains("▾ Alignment")).unwrap();
    let issue = rows.iter().find(|row| row.contains("1. Finished issue")).unwrap();

    assert_eq!(char_index(prd, "✓ complete"), char_index(issue, "✓ complete"));
    assert_eq!(char_index(project, "1 PRDs"), char_index(prd, "1/1"));
}

#[test]
fn tracker_view_lines_style_selection_headings_and_status_badges() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &[]).unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "detail-polish".into(),
                title: "Detail polish".into(),
                status: PrdStatus::InProgress,
                body: None,
                source_uri: None,
            },
            vec![
                PlanIssueInput {
                    key: "base".into(),
                    title: "Base issue".into(),
                    status: IssueStatus::Open,
                    body: None,
                    position: 1,
                    depends_on: vec![],
                },
                PlanIssueInput {
                    key: "blocked".into(),
                    title: "Blocked issue".into(),
                    status: IssueStatus::Open,
                    body: None,
                    position: 2,
                    depends_on: vec!["base".into()],
                },
            ],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let mut state = TrackerViewState::default();
    state.expand("project:piv");
    state.expand("prd:piv/detail-polish");
    state.selected = 1;

    let lines = render_tracker_view_lines(&snapshot, &state, 120, 20);
    let heading = lines
        .iter()
        .find(|line| line_text(line).contains("Project / PRD / Issue tree"))
        .unwrap();
    let expanded_project = lines
        .iter()
        .find(|line| line_text(line).contains("  ▾ piv"))
        .unwrap();
    let selected_prd = lines
        .iter()
        .find(|line| line_text(line).contains("›   ▾ Detail polish"))
        .unwrap();
    let blocked_issue = lines
        .iter()
        .find(|line| line_text(line).contains("Blocked issue") && line_text(line).contains("◆ blocked"))
        .unwrap();

    assert_eq!(heading.style.fg, Some(Color::Cyan));
    assert_eq!(expanded_project.style.fg, Some(Color::Cyan));
    assert!(selected_prd.style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(selected_prd.style.fg, Some(Color::Cyan));
    assert_eq!(blocked_issue.style.fg, Some(Color::Rgb(230, 170, 70)));
}

#[test]
fn wide_tracker_view_shows_selected_prd_detail_body_and_linked_issues() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &["/tmp/piv"]).unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "tracker-reading".into(),
                title: "Tracker reading".into(),
                status: PrdStatus::InProgress,
                body: Some("# Tracker reading\n\nOriginal PRD body".into()),
                source_uri: Some("docs/prd/tracker-reading.md".into()),
            },
            vec![
                PlanIssueInput {
                    key: "prd-read-api".into(),
                    title: "Expose PRD read API".into(),
                    status: IssueStatus::Open,
                    body: Some("Return PRD body".into()),
                    position: 1,
                    depends_on: vec![],
                },
                PlanIssueInput {
                    key: "issue-read-api".into(),
                    title: "Expose Issue read API".into(),
                    status: IssueStatus::Open,
                    body: Some("Return Issue body and blockers".into()),
                    position: 2,
                    depends_on: vec!["prd-read-api".into()],
                },
            ],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let mut state = TrackerViewState::default();
    state.expand("project:piv");
    state.selected = 1;

    let rows = render_tracker_view(&snapshot, &state, 120);
    let rendered = rows.join("\n");

    assert!(rendered.contains("Project / PRD / Issue tree"));
    assert!(rendered.contains("Details"));
    assert!(rendered.contains("PRD: Tracker reading"));
    assert!(rendered.contains("source  docs/prd/tracker-reading.md"));
    assert!(rendered.contains("Original PRD body"));
    assert!(rendered.contains("Issues"));
    assert!(rendered.contains("1. ◌ Expose PRD read API"));
    assert!(rendered.contains("status  ◌ open"));
    assert!(rendered.contains("notes   Return PRD body"));
    assert!(rendered.contains("2. ◆ Expose Issue read API"));
    assert!(rendered.contains("blocked by prd-read-api"));
    assert!(rendered.contains("notes   Return Issue body and blockers"));
}

#[test]
fn tracker_detail_renders_markdown_headings_and_bullets_with_style() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &[]).unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "markdown-polish".into(),
                title: "Markdown polish".into(),
                status: PrdStatus::InProgress,
                body: Some("# Product Title\n\n## Problem\n- first bullet".into()),
                source_uri: None,
            },
            vec![],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let mut state = TrackerViewState::default();
    state.expand("project:piv");
    state.selected = 1;

    let lines = render_tracker_view_lines(&snapshot, &state, 120, 24);
    let title = lines
        .iter()
        .find(|line| line_text(line).contains("Product Title"))
        .unwrap();
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(!rendered.contains("# Product Title"));
    assert!(rendered.contains("▌ Product Title"));
    assert!(rendered.contains("▸ Problem"));
    assert!(rendered.contains("• first bullet"));
    assert_eq!(title.style.fg, Some(Color::Yellow));
    assert!(title.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn wide_tracker_view_wraps_long_prd_body_text_instead_of_truncating() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &[]).unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "detail-polish".into(),
                title: "Detail polish".into(),
                status: PrdStatus::InProgress,
                body: Some("This long PRD paragraph should wrap inside the detail pane so the final readable phrase is still visible.".into()),
                source_uri: None,
            },
            vec![],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let mut state = TrackerViewState::default();
    state.expand("project:piv");
    state.selected = 1;

    let rendered = render_tracker_view(&snapshot, &state, 120).join("\n");

    assert!(rendered.contains("final readable phrase"));
}

#[test]
fn wide_tracker_viewport_scrolls_detail_pane_with_footer() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &[]).unwrap();
    let body = (1..=20)
        .map(|line| format!("long detail line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "detail-polish".into(),
                title: "Detail polish".into(),
                status: PrdStatus::InProgress,
                body: Some(body),
                source_uri: None,
            },
            vec![],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let mut state = TrackerViewState::default();
    state.expand("project:piv");
    state.selected = 1;
    state.detail_scroll = 13;

    let rows = render_tracker_viewport(&snapshot, &state, 120, 10);
    let rendered = rows.join("\n");

    assert!(rendered.contains("long detail line 9"));
    assert!(!rows.iter().any(|row| row.trim_end().ends_with("long detail line 1")));
    assert!(rendered.contains("PgUp/PgDn scroll"));
}

#[test]
fn wide_tracker_view_uses_equal_width_horizontal_borders() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &[]).unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "detail-polish".into(),
                title: "Detail polish".into(),
                status: PrdStatus::InProgress,
                body: Some("Readable details".into()),
                source_uri: None,
            },
            vec![],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let mut state = TrackerViewState::default();
    state.expand("project:piv");
    state.selected = 1;

    let rows = render_tracker_viewport(&snapshot, &state, 120, 20);
    let border_widths = rows
        .iter()
        .filter(|row| {
            let trimmed = row.trim();
            !trimmed.is_empty() && trimmed.chars().all(|ch| ch == '─')
        })
        .map(|row| row.trim().chars().count())
        .collect::<Vec<_>>();

    assert!(border_widths.len() >= 2);
    assert!(border_widths.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn wide_tracker_view_has_horizontal_separator_and_full_width_padded_detail_text() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &[]).unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "detail-polish".into(),
                title: "Detail polish".into(),
                status: PrdStatus::InProgress,
                body: Some("Readable details should use almost the full terminal width with padding so this final phrase stays on one line.".into()),
                source_uri: None,
            },
            vec![],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let mut state = TrackerViewState::default();
    state.expand("project:piv");
    state.selected = 1;

    let rows = render_tracker_viewport(&snapshot, &state, 120, 20);

    assert!(rows.iter().any(|row| row.trim().chars().all(|ch| ch == '─')));
    assert!(rows.iter().any(|row| row.starts_with("  PRD: Detail polish")));
    assert!(rows.iter().any(|row| row.starts_with("  Readable details") && row.contains("final phrase stays on one line")));
    assert!(rows.iter().all(|row| !row.contains(" │ ")));
}

#[test]
fn clamped_detail_scroll_reverses_immediately_after_overscroll() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &[]).unwrap();
    let body = (1..=30)
        .map(|line| format!("long detail line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "detail-polish".into(),
                title: "Detail polish".into(),
                status: PrdStatus::InProgress,
                body: Some(body),
                source_uri: None,
            },
            vec![],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let mut state = TrackerViewState::default();
    state.expand("project:piv");
    state.selected = 1;
    state.detail_scroll = usize::MAX;

    let max_scroll = max_tracker_detail_scroll(&snapshot, &state, 120, 12);
    state.clamp_detail_scroll(max_scroll);
    state.scroll_detail_up(5);

    assert!(state.detail_scroll < max_scroll);
}

#[test]
fn tracker_selection_changes_reset_detail_scroll() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &[]).unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "detail-polish".into(),
                title: "Detail polish".into(),
                status: PrdStatus::InProgress,
                body: None,
                source_uri: None,
            },
            vec![],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let mut state = TrackerViewState::default();
    state.expand("project:piv");
    state.detail_scroll = 12;

    state.move_down(&snapshot);

    assert_eq!(state.selected, 1);
    assert_eq!(state.detail_scroll, 0);
}

#[test]
fn wide_tracker_view_includes_full_selected_issue_body() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &[]).unwrap();
    let body = (1..=10)
        .map(|line| format!("issue body line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "detail-polish".into(),
                title: "Detail polish".into(),
                status: PrdStatus::InProgress,
                body: None,
                source_uri: None,
            },
            vec![PlanIssueInput {
                key: "full-issue-body".into(),
                title: "Full issue body".into(),
                status: IssueStatus::Open,
                body: Some(body),
                position: 1,
                depends_on: vec![],
            }],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let mut state = TrackerViewState::default();
    state.expand("project:piv");
    state.expand("prd:piv/detail-polish");
    state.selected = 2;

    let rendered = render_tracker_view(&snapshot, &state, 120).join("\n");

    assert!(rendered.contains("issue body line 10"));
}

#[test]
fn wide_tracker_view_shows_selected_issue_detail_body_blockers_and_linked_prds() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &["/tmp/piv"]).unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "tracker-reading".into(),
                title: "Tracker reading".into(),
                status: PrdStatus::InProgress,
                body: Some("# Tracker reading".into()),
                source_uri: Some("docs/prd/tracker-reading.md".into()),
            },
            vec![
                PlanIssueInput {
                    key: "prd-read-api".into(),
                    title: "Expose PRD read API".into(),
                    status: IssueStatus::Open,
                    body: Some("Return PRD body".into()),
                    position: 1,
                    depends_on: vec![],
                },
                PlanIssueInput {
                    key: "issue-read-api".into(),
                    title: "Expose Issue read API".into(),
                    status: IssueStatus::Open,
                    body: Some("Return Issue body and blockers".into()),
                    position: 2,
                    depends_on: vec!["prd-read-api".into()],
                },
            ],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let mut state = TrackerViewState::default();
    state.expand("project:piv");
    state.expand("prd:piv/tracker-reading");
    state.selected = 3;

    let rows = render_tracker_view(&snapshot, &state, 120);
    let rendered = rows.join("\n");

    assert!(rendered.contains("Issue: Expose Issue read API"));
    assert!(rendered.contains("status  ◆ blocked"));
    assert!(rendered.contains("blocked by prd-read-api"));
    assert!(rendered.contains("Return Issue body and blockers"));
    assert!(rendered.contains("PRDs"));
    assert!(rendered.contains("Tracker reading"));
}

#[test]
fn wide_tracker_view_shows_selected_project_detail_roots_and_prd_count() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &["/tmp/piv"]).unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "tracker-reading".into(),
                title: "Tracker reading".into(),
                status: PrdStatus::Draft,
                body: None,
                source_uri: None,
            },
            vec![],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let state = TrackerViewState::default();

    let rows = render_tracker_view(&snapshot, &state, 120);
    let rendered = rows.join("\n");

    assert!(rendered.contains("Project: piv"));
    assert!(rendered.contains("key     piv"));
    assert!(rendered.contains("PRDs    1"));
    assert!(rendered.contains("Roots"));
    assert!(rendered.contains("/tmp/piv"));
}

#[test]
fn tracker_rows_align_prd_status_and_progress_columns() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &[]).unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "active-prd".into(),
                title: "Active PRD".into(),
                status: PrdStatus::InProgress,
                body: None,
                source_uri: None,
            },
            vec![PlanIssueInput {
                key: "active-issue".into(),
                title: "Active issue".into(),
                status: IssueStatus::Open,
                body: None,
                position: 1,
                depends_on: vec![],
            }],
        )
        .unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "complete-prd".into(),
                title: "Complete PRD".into(),
                status: PrdStatus::Complete,
                body: None,
                source_uri: None,
            },
            vec![PlanIssueInput {
                key: "complete-issue".into(),
                title: "Complete issue".into(),
                status: IssueStatus::Complete,
                body: None,
                position: 1,
                depends_on: vec![],
            }],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let mut state = TrackerViewState::default();
    state.expand("project:piv");

    let rows = render_tracker_rows(&snapshot, &state, 90);
    let active = rows.iter().find(|row| row.contains("Active PRD")).unwrap();
    let complete = rows.iter().find(|row| row.contains("Complete PRD")).unwrap();

    assert_eq!(active.find("● in progress"), complete.find("✓ complete"));
    assert_eq!(active.find("0/1"), complete.find("1/1"));
}

#[test]
fn tracker_rows_hide_zero_of_zero_progress_for_prds_without_issues() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &[]).unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "piv-mvp".into(),
                title: "piv MVP".into(),
                status: PrdStatus::Complete,
                body: None,
                source_uri: None,
            },
            vec![],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let mut state = TrackerViewState::default();
    state.expand("project:piv");

    let rows = render_tracker_rows(&snapshot, &state, 80);
    let prd_row = rows.iter().find(|row| row.contains("piv MVP")).unwrap();

    assert!(prd_row.contains("complete"));
    assert!(!prd_row.contains("0/0"));
}
