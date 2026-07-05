use std::path::Path;

use piv::{
    issue_cockpit::{
        build_issue_cockpit_view, extract_file_references, match_project_for_root,
        render_issue_cockpit, tracker_status_fragment, IssueCockpitState,
    },
    tracker::{IssueStatus, PlanIssueInput, PrdInput, PrdStatus, TrackerStore},
};
use tempfile::tempdir;

fn upsert_plan(
    store: &mut TrackerStore,
    project_key: &str,
    prd_key: &str,
    prd_title: &str,
    issues: Vec<PlanIssueInput>,
) {
    store
        .upsert_plan(
            project_key,
            PrdInput {
                key: prd_key.into(),
                title: prd_title.into(),
                status: PrdStatus::InProgress,
                body: None,
                source_uri: None,
            },
            issues,
        )
        .unwrap();
}

fn issue(key: &str, title: &str, position: i64) -> PlanIssueInput {
    PlanIssueInput {
        key: key.into(),
        title: title.into(),
        status: IssueStatus::Open,
        body: None,
        position,
        depends_on: vec![],
    }
}

#[test]
fn root_matching_uses_watched_root_and_longest_registered_prefix() {
    let dir = tempdir().unwrap();
    let broad = dir.path().join("repo");
    let nested = broad.join("crates/piv");
    let watched = nested.join("src");
    std::fs::create_dir_all(&watched).unwrap();

    let mut store = TrackerStore::open_in_memory().unwrap();
    store
        .create_project("broad", "Broad", &[broad.to_str().unwrap()])
        .unwrap();
    store
        .create_project("nested", "Nested", &[nested.to_str().unwrap()])
        .unwrap();
    upsert_plan(
        &mut store,
        "broad",
        "broad-prd",
        "Broad PRD",
        vec![issue("broad-issue", "Broad issue", 1)],
    );
    upsert_plan(
        &mut store,
        "nested",
        "nested-prd",
        "Nested PRD",
        vec![issue("nested-issue", "Nested issue", 1)],
    );

    let snapshot = store.snapshot().unwrap();
    let matched = match_project_for_root(&snapshot, &watched).unwrap();
    assert_eq!(matched.project_key, "nested");

    let status = tracker_status_fragment(&snapshot, &watched, &IssueCockpitState::default());
    assert!(status.contains("tracker nested"));
    assert!(status.contains("nested-issue"));
    assert!(!status.contains("broad-issue"));
}

#[test]
fn matched_root_can_toggle_between_current_project_and_all_projects() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    let mut store = TrackerStore::open_in_memory().unwrap();
    store
        .create_project("piv", "piv", &[root.to_str().unwrap()])
        .unwrap();
    store.create_project("other", "other", &[]).unwrap();
    upsert_plan(
        &mut store,
        "piv",
        "piv-prd",
        "piv PRD",
        vec![issue("piv-issue", "piv issue", 1)],
    );
    upsert_plan(
        &mut store,
        "other",
        "other-prd",
        "other PRD",
        vec![issue("other-issue", "other issue", 1)],
    );
    let snapshot = store.snapshot().unwrap();

    let mut state = IssueCockpitState::default();
    let scoped = build_issue_cockpit_view(&snapshot, root, &state);
    assert_eq!(scoped.scope.drawer_label(), "current project piv (piv)");
    assert_eq!(scoped.focus.unwrap().issue_key, "piv-issue");

    state.toggle_scope();
    let all = build_issue_cockpit_view(&snapshot, root, &state);
    assert_eq!(all.scope.drawer_label(), "all projects");
    assert_eq!(all.project_count, 2);
}

#[test]
fn root_matching_falls_back_to_project_key_when_no_registered_root_matches() {
    let dir = tempdir().unwrap();
    let watched = dir.path().join("piv");
    std::fs::create_dir_all(&watched).unwrap();

    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &[]).unwrap();
    store.create_project("fairy", "Fairy", &[]).unwrap();
    upsert_plan(
        &mut store,
        "piv",
        "piv-prd",
        "piv PRD",
        vec![issue("piv-issue", "piv issue", 1)],
    );
    upsert_plan(
        &mut store,
        "fairy",
        "fairy-prd",
        "Fairy PRD",
        vec![issue("fairy-issue", "Fairy issue", 1)],
    );

    let snapshot = store.snapshot().unwrap();
    let matched = match_project_for_root(&snapshot, &watched).unwrap();
    assert_eq!(matched.project_key, "piv");

    let view = build_issue_cockpit_view(&snapshot, &watched, &IssueCockpitState::default());
    assert_eq!(view.focus.unwrap().issue_key, "piv-issue");
}

#[test]
fn unmatched_root_falls_back_to_all_projects_with_clear_notice() {
    let dir = tempdir().unwrap();
    let project_root = dir.path().join("repo");
    let watched = dir.path().join("other");
    std::fs::create_dir_all(&project_root).unwrap();
    std::fs::create_dir_all(&watched).unwrap();

    let mut store = TrackerStore::open_in_memory().unwrap();
    store
        .create_project("piv", "piv", &[project_root.to_str().unwrap()])
        .unwrap();
    upsert_plan(
        &mut store,
        "piv",
        "fallback-prd",
        "Fallback PRD",
        vec![issue("fallback-issue", "Fallback issue", 1)],
    );
    let snapshot = store.snapshot().unwrap();

    let view = build_issue_cockpit_view(&snapshot, &watched, &IssueCockpitState::default());
    assert!(view
        .fallback_notice
        .unwrap()
        .contains("showing all projects"));
    assert_eq!(view.focus.unwrap().issue_key, "fallback-issue");

    let rendered =
        render_issue_cockpit(&snapshot, &watched, &IssueCockpitState::default(), 100, 10)
            .join("\n");
    assert!(rendered.contains("all projects — no registered root matched"));
    assert!(rendered.contains("No registered Project root matched"));
}

#[test]
fn status_fragment_reports_empty_tracker_and_empty_matching_project() {
    let dir = tempdir().unwrap();
    let watched = dir.path();
    let state = IssueCockpitState::default();

    let empty = TrackerStore::open_in_memory().unwrap().snapshot().unwrap();
    assert_eq!(
        tracker_status_fragment(&empty, watched, &state),
        "tracker no projects"
    );

    let mut store = TrackerStore::open_in_memory().unwrap();
    store
        .create_project("piv", "piv", &[watched.to_str().unwrap()])
        .unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "empty-prd".into(),
                title: "Empty PRD".into(),
                status: PrdStatus::InProgress,
                body: None,
                source_uri: None,
            },
            vec![],
        )
        .unwrap();
    let snapshot = store.snapshot().unwrap();
    assert_eq!(
        tracker_status_fragment(&snapshot, watched, &state),
        "tracker piv: no issues"
    );
}

#[test]
fn issue_cockpit_renders_current_issue_siblings_and_blockers() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut store = TrackerStore::open_in_memory().unwrap();
    store
        .create_project("piv", "piv", &[root.to_str().unwrap()])
        .unwrap();
    upsert_plan(
        &mut store,
        "piv",
        "cockpit-prd",
        "Cockpit PRD",
        vec![
            PlanIssueInput {
                status: IssueStatus::Complete,
                ..issue("done", "Already done", 1)
            },
            PlanIssueInput {
                status: IssueStatus::InProgress,
                body: Some("Working notes".into()),
                ..issue("active", "Active work", 2)
            },
            PlanIssueInput {
                depends_on: vec!["active".into()],
                ..issue("blocked", "Blocked work", 3)
            },
        ],
    );

    let snapshot = store.snapshot().unwrap();
    let rendered =
        render_issue_cockpit(&snapshot, root, &IssueCockpitState::default(), 120, 16).join("\n");

    assert!(rendered.contains("Current  ● in progress active — Active work"));
    assert!(rendered.contains("Notes    Working notes"));
    assert!(rendered.contains("→ 2. ● in progress active — Active work"));
    assert!(rendered.contains("3. ◆ blocked blocked — Blocked work blocked by active"));
}

#[test]
fn all_project_refs_resolve_against_focused_issue_project_root() {
    let dir = tempdir().unwrap();
    let watched = dir.path().join("unmatched");
    let project_root = dir.path().join("project");
    std::fs::create_dir_all(project_root.join("src")).unwrap();
    std::fs::create_dir_all(&watched).unwrap();
    std::fs::write(project_root.join("src/main.rs"), "one\n").unwrap();

    let mut store = TrackerStore::open_in_memory().unwrap();
    store
        .create_project("piv", "piv", &[project_root.to_str().unwrap()])
        .unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "refs-prd".into(),
                title: "Refs PRD".into(),
                status: PrdStatus::InProgress,
                body: None,
                source_uri: None,
            },
            vec![PlanIssueInput {
                body: Some("Issue mentions src/main.rs:1".into()),
                ..issue("refs", "Refs", 1)
            }],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let view = build_issue_cockpit_view(&snapshot, &watched, &IssueCockpitState::default());

    assert_eq!(view.refs.len(), 1);
    assert_eq!(
        view.refs[0].path,
        project_root.join("src/main.rs").canonicalize().unwrap()
    );
    assert_eq!(view.refs[0].display, "src/main.rs:1");
}

#[test]
fn file_reference_discovery_normalizes_valid_refs_and_ignores_noise() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("secret.rs"), "nope\n").unwrap();

    let text = format!(
        "See `src/main.rs:2`., ({abs}:3-5), missing.rs:1, prose:4, {outside}:1",
        abs = root.join("src/lib.rs").display(),
        outside = outside.path().join("secret.rs").display()
    );
    let refs = extract_file_references(&text, root);

    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].display, "src/main.rs:2");
    assert_eq!(refs[0].start_line, 2);
    assert_eq!(refs[0].end_line, 2);
    assert_eq!(refs[1].display, "src/lib.rs:3-5");
    assert_eq!(refs[1].start_line, 3);
    assert_eq!(refs[1].end_line, 5);
}

#[test]
fn issue_cockpit_shows_refs_from_issue_and_prd_bodies() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "one\ntwo\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "one\ntwo\nthree\n").unwrap();

    let mut store = TrackerStore::open_in_memory().unwrap();
    store
        .create_project("piv", "piv", &[root.to_str().unwrap()])
        .unwrap();
    store
        .upsert_plan(
            "piv",
            PrdInput {
                key: "refs-prd".into(),
                title: "Refs PRD".into(),
                status: PrdStatus::InProgress,
                body: Some("PRD mentions src/lib.rs:3".into()),
                source_uri: None,
            },
            vec![PlanIssueInput {
                body: Some("Issue mentions src/main.rs:2".into()),
                ..issue("refs", "Show refs", 1)
            }],
        )
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let rendered =
        render_issue_cockpit(&snapshot, root, &IssueCockpitState::default(), 100, 16).join("\n");

    assert!(rendered.contains("Refs"));
    assert!(rendered.contains("› 1. src/main.rs:2"));
    assert!(rendered.contains("  2. src/lib.rs:3"));
}

#[test]
fn reference_parser_requires_existing_files_under_watched_root() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "one\n").unwrap();

    let refs = extract_file_references(
        "src/main.rs:1 src/main.rs:0 src/main.rs:3-2 /tmp/nope.rs:1",
        Path::new(root),
    );
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].display, "src/main.rs:1");
}
