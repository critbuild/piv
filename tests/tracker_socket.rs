use piv::tracker_rpc::{TrackerRpcServer, send_tracker_rpc_request};
use serde_json::{Value, json};
use tempfile::tempdir;

fn rpc(socket: &std::path::Path, request: Value) -> Value {
    let response = send_tracker_rpc_request(socket, &request.to_string()).unwrap();
    serde_json::from_str(&response).unwrap()
}

#[test]
fn tracker_socket_registers_plan_without_tui_running() {
    let dir = tempdir().unwrap();
    let socket = dir.path().join("tracker.sock");
    let db = dir.path().join("tracker.sqlite");
    let _server = TrackerRpcServer::start(&socket, &db).unwrap();

    rpc(
        &socket,
        json!({
            "jsonrpc":"2.0",
            "id":"1",
            "method":"project.create",
            "params":{"key":"piv","name":"piv"}
        }),
    );
    rpc(
        &socket,
        json!({
            "jsonrpc":"2.0",
            "id":"2",
            "method":"prd.upsert_plan",
            "params":{
                "project_key":"piv",
                "prd":{"key":"prd-tracker","title":"PRD tracker"},
                "issues":[{"key":"sqlite-registry","title":"Create SQLite registry","position":1}]
            }
        }),
    );

    let response = rpc(
        &socket,
        json!({
            "jsonrpc":"2.0",
            "id":"3",
            "method":"issue.next",
            "params":{"project_key":"piv","prd_key":"prd-tracker"}
        }),
    );

    assert_eq!(response["result"]["key"], "sqlite-registry");
}
