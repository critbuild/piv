use piv::tracker::{IssueStatus, PlanIssueInput, PrdInput, PrdStatus, TrackerStore};

#[test]
fn upsert_plan_creates_ordered_issues_and_blockers_atomically() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &["/tmp/piv"]).unwrap();

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
                    status: IssueStatus::Open,
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
            ],
        )
        .unwrap();

    let prds = store.list_prds("piv").unwrap();
    assert_eq!(prds.len(), 1);
    assert_eq!(prds[0].key, "prd-tracker");
    assert_eq!(prds[0].status, PrdStatus::InProgress);

    let issues = store.list_issues("piv", "prd-tracker").unwrap();
    assert_eq!(
        issues
            .iter()
            .map(|issue| issue.key.as_str())
            .collect::<Vec<_>>(),
        vec!["sqlite-registry", "socket-api"]
    );
    assert!(!issues[0].blocked);
    assert!(issues[1].blocked);
    assert_eq!(issues[1].blocked_by_keys, vec!["sqlite-registry"]);
}

#[test]
fn get_prd_returns_body_source_progress_and_linked_issues() {
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
            vec![PlanIssueInput {
                key: "prd-read-api".into(),
                title: "Expose PRD read API".into(),
                status: IssueStatus::Complete,
                body: Some("Return PRD body and linked issues".into()),
                position: 1,
                depends_on: vec![],
            }],
        )
        .unwrap();

    let details = store.get_prd("piv", "tracker-reading").unwrap();

    assert_eq!(details.prd.key, "tracker-reading");
    assert_eq!(details.prd.body.as_deref(), Some("# Tracker reading\n\nOriginal PRD body"));
    assert_eq!(details.prd.source_uri.as_deref(), Some("docs/prd/tracker-reading.md"));
    assert_eq!(details.prd.completed_issue_count, 1);
    assert_eq!(details.prd.issue_count, 1);
    assert_eq!(details.issues.len(), 1);
    assert_eq!(details.issues[0].key, "prd-read-api");
    assert_eq!(details.issues[0].body.as_deref(), Some("Return PRD body and linked issues"));
}

#[test]
fn get_issue_returns_body_order_blockers_and_linked_prds() {
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
                    body: Some("Return original PRD markdown".into()),
                    position: 1,
                    depends_on: vec![],
                },
                PlanIssueInput {
                    key: "issue-read-api".into(),
                    title: "Expose Issue read API".into(),
                    status: IssueStatus::Open,
                    body: Some("Return Issue body, blockers, and linked PRDs".into()),
                    position: 2,
                    depends_on: vec!["prd-read-api".into()],
                },
            ],
        )
        .unwrap();

    let details = store.get_issue("piv", "issue-read-api").unwrap();

    assert_eq!(details.issue.key, "issue-read-api");
    assert_eq!(details.issue.body.as_deref(), Some("Return Issue body, blockers, and linked PRDs"));
    assert_eq!(details.issue.position, 2);
    assert!(details.issue.blocked);
    assert_eq!(details.issue.blocked_by_keys, vec!["prd-read-api"]);
    assert_eq!(details.prds.len(), 1);
    assert_eq!(details.prds[0].key, "tracker-reading");
}

#[test]
fn upsert_plan_rejects_unknown_blockers_without_partial_save() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &[]).unwrap();

    let result = store.upsert_plan(
        "piv",
        PrdInput {
            key: "bad-plan".into(),
            title: "Bad plan".into(),
            status: PrdStatus::Draft,
            body: None,
            source_uri: None,
        },
        vec![PlanIssueInput {
            key: "a".into(),
            title: "A".into(),
            status: IssueStatus::Open,
            body: None,
            position: 1,
            depends_on: vec!["missing".into()],
        }],
    );

    assert!(result.is_err());
    assert!(store.list_prds("piv").unwrap().is_empty());
}

#[test]
fn upsert_plan_rejects_cycles_without_partial_save() {
    let mut store = TrackerStore::open_in_memory().unwrap();
    store.create_project("piv", "piv", &[]).unwrap();

    let result = store.upsert_plan(
        "piv",
        PrdInput {
            key: "cyclic-plan".into(),
            title: "Cyclic plan".into(),
            status: PrdStatus::Draft,
            body: None,
            source_uri: None,
        },
        vec![
            PlanIssueInput {
                key: "a".into(),
                title: "A".into(),
                status: IssueStatus::Open,
                body: None,
                position: 1,
                depends_on: vec!["b".into()],
            },
            PlanIssueInput {
                key: "b".into(),
                title: "B".into(),
                status: IssueStatus::Open,
                body: None,
                position: 2,
                depends_on: vec!["a".into()],
            },
        ],
    );

    assert!(result.is_err());
    assert!(store.list_prds("piv").unwrap().is_empty());
}

#[test]
fn issue_next_skips_completed_and_blocked_issues_by_prd_order() {
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

    assert_eq!(
        store.issue_next("piv", "prd-tracker").unwrap().unwrap().key,
        "socket-api"
    );

    store
        .set_issue_status("piv", "socket-api", IssueStatus::Complete)
        .unwrap();

    assert_eq!(
        store.issue_next("piv", "prd-tracker").unwrap().unwrap().key,
        "prd-ui"
    );
}
