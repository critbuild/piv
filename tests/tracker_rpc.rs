use piv::{tracker::TrackerStore, tracker_rpc::handle_json_rpc};
use serde_json::{Value, json};

fn call(store: &mut TrackerStore, request: Value) -> Value {
    let response = handle_json_rpc(store, &request.to_string()).unwrap();
    serde_json::from_str(&response).unwrap()
}

#[test]
fn json_rpc_gets_prd_body_source_and_linked_issues() {
    let mut store = TrackerStore::open_in_memory().unwrap();

    call(
        &mut store,
        json!({
            "jsonrpc": "2.0",
            "id": "1",
            "method": "project.create",
            "params": { "key": "piv", "name": "piv" }
        }),
    );
    call(
        &mut store,
        json!({
            "jsonrpc": "2.0",
            "id": "2",
            "method": "prd.upsert_plan",
            "params": {
                "project_key": "piv",
                "prd": {
                    "key": "tracker-reading",
                    "title": "Tracker reading",
                    "status": "in_progress",
                    "body": "# Tracker reading\n\nOriginal PRD body",
                    "source_uri": "docs/prd/tracker-reading.md"
                },
                "issues": [
                    { "key": "prd-read-api", "title": "Expose PRD read API", "status": "complete", "position": 1 }
                ]
            }
        }),
    );

    let response = call(
        &mut store,
        json!({
            "jsonrpc": "2.0",
            "id": "3",
            "method": "prd.get",
            "params": { "project_key": "piv", "prd_key": "tracker-reading" }
        }),
    );

    assert_eq!(response["result"]["prd"]["key"], "tracker-reading");
    assert_eq!(response["result"]["prd"]["body"], "# Tracker reading\n\nOriginal PRD body");
    assert_eq!(response["result"]["prd"]["source_uri"], "docs/prd/tracker-reading.md");
    assert_eq!(response["result"]["prd"]["completed_issue_count"], 1);
    assert_eq!(response["result"]["issues"][0]["key"], "prd-read-api");
}

#[test]
fn json_rpc_gets_issue_details_blockers_and_linked_prds() {
    let mut store = TrackerStore::open_in_memory().unwrap();

    call(
        &mut store,
        json!({
            "jsonrpc": "2.0",
            "id": "1",
            "method": "project.create",
            "params": { "key": "piv", "name": "piv" }
        }),
    );
    call(
        &mut store,
        json!({
            "jsonrpc": "2.0",
            "id": "2",
            "method": "prd.upsert_plan",
            "params": {
                "project_key": "piv",
                "prd": { "key": "tracker-reading", "title": "Tracker reading" },
                "issues": [
                    { "key": "prd-read-api", "title": "Expose PRD read API", "position": 1 },
                    {
                        "key": "issue-read-api",
                        "title": "Expose Issue read API",
                        "body": "Return Issue body, blockers, and linked PRDs",
                        "position": 2,
                        "depends_on": ["prd-read-api"]
                    }
                ]
            }
        }),
    );

    let response = call(
        &mut store,
        json!({
            "jsonrpc": "2.0",
            "id": "3",
            "method": "issue.get",
            "params": { "project_key": "piv", "issue_key": "issue-read-api" }
        }),
    );

    assert_eq!(response["result"]["issue"]["key"], "issue-read-api");
    assert_eq!(response["result"]["issue"]["body"], "Return Issue body, blockers, and linked PRDs");
    assert_eq!(response["result"]["issue"]["position"], 2);
    assert_eq!(response["result"]["issue"]["blocked"], true);
    assert_eq!(response["result"]["issue"]["blocked_by_keys"], json!(["prd-read-api"]));
    assert_eq!(response["result"]["prds"][0]["key"], "tracker-reading");
}

#[test]
fn json_rpc_registers_plan_and_queries_next_issue() {
    let mut store = TrackerStore::open_in_memory().unwrap();

    let response = call(
        &mut store,
        json!({
            "jsonrpc": "2.0",
            "id": "1",
            "method": "project.create",
            "params": { "key": "piv", "name": "piv", "roots": ["/tmp/piv"] }
        }),
    );
    assert_eq!(response["result"]["key"], "piv");

    let response = call(
        &mut store,
        json!({
            "jsonrpc": "2.0",
            "id": "2",
            "method": "prd.upsert_plan",
            "params": {
                "project_key": "piv",
                "prd": { "key": "prd-tracker", "title": "PRD tracker", "status": "in_progress" },
                "issues": [
                    { "key": "sqlite-registry", "title": "Create SQLite registry", "status": "complete", "position": 1 },
                    { "key": "socket-api", "title": "Add tracker socket", "position": 2, "depends_on": ["sqlite-registry"] }
                ]
            }
        }),
    );
    assert_eq!(response["result"]["key"], "prd-tracker");

    let response = call(
        &mut store,
        json!({
            "jsonrpc": "2.0",
            "id": "3",
            "method": "issue.next",
            "params": { "project_key": "piv", "prd_key": "prd-tracker" }
        }),
    );
    assert_eq!(response["result"]["key"], "socket-api");
    assert_eq!(response["result"]["blocked"], false);
}
