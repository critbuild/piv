use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    thread,
};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::tracker::{IssueStatus, PlanIssueInput, PrdInput, PrdStatus, TrackerStore};

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Deserialize)]
struct ProjectCreateParams {
    key: String,
    name: String,
    #[serde(default)]
    roots: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct UpsertPlanParams {
    project_key: String,
    prd: PrdInput,
    #[serde(default)]
    issues: Vec<PlanIssueInput>,
}

#[derive(Debug, Deserialize)]
struct PrdRefParams {
    project_key: String,
    prd_key: String,
}

#[derive(Debug, Deserialize)]
struct IssueRefParams {
    project_key: String,
    issue_key: String,
}

#[derive(Debug, Deserialize)]
struct ProjectRefParams {
    project_key: String,
}

#[derive(Debug, Deserialize)]
struct IssueStatusParams {
    project_key: String,
    issue_key: String,
    status: IssueStatus,
}

#[derive(Debug, Deserialize)]
struct PrdStatusParams {
    project_key: String,
    prd_key: String,
    status: PrdStatus,
}

#[derive(Debug, Deserialize)]
struct BackfillParams {
    project_key: String,
    path: PathBuf,
}

pub fn handle_json_rpc(store: &mut TrackerStore, request_json: &str) -> Result<String> {
    let parsed: std::result::Result<JsonRpcRequest, _> = serde_json::from_str(request_json);
    let request = match parsed {
        Ok(request) => request,
        Err(error) => {
            return Ok(error_response(
                None,
                -32700,
                &format!("parse error: {error}"),
            ));
        }
    };
    let id = request.id.clone();
    let result = dispatch(store, request);
    Ok(match result {
        Ok(result) => success_response(id, result),
        Err(error) => error_response(id, -32000, &error.to_string()),
    })
}

fn dispatch(store: &mut TrackerStore, request: JsonRpcRequest) -> Result<Value> {
    match request.method.as_str() {
        "project.create" => {
            let params: ProjectCreateParams = serde_json::from_value(request.params)?;
            let root_refs = params.roots.iter().map(String::as_str).collect::<Vec<_>>();
            Ok(serde_json::to_value(store.create_project(
                &params.key,
                &params.name,
                &root_refs,
            )?)?)
        }
        "project.list" => Ok(serde_json::to_value(store.list_projects()?)?),
        "prd.upsert_plan" => {
            let params: UpsertPlanParams = serde_json::from_value(request.params)?;
            Ok(serde_json::to_value(store.upsert_plan(
                &params.project_key,
                params.prd,
                params.issues,
            )?)?)
        }
        "prd.list" => {
            let params: ProjectRefParams = serde_json::from_value(request.params)?;
            Ok(serde_json::to_value(store.list_prds(&params.project_key)?)?)
        }
        "prd.get" => {
            let params: PrdRefParams = serde_json::from_value(request.params)?;
            Ok(serde_json::to_value(store.get_prd(
                &params.project_key,
                &params.prd_key,
            )?)?)
        }
        "prd.set_status" => {
            let params: PrdStatusParams = serde_json::from_value(request.params)?;
            store.set_prd_status(&params.project_key, &params.prd_key, params.status)?;
            Ok(json!({ "ok": true }))
        }
        "issue.list" => {
            let params: PrdRefParams = serde_json::from_value(request.params)?;
            Ok(serde_json::to_value(
                store.list_issues(&params.project_key, &params.prd_key)?,
            )?)
        }
        "issue.get" => {
            let params: IssueRefParams = serde_json::from_value(request.params)?;
            Ok(serde_json::to_value(store.get_issue(
                &params.project_key,
                &params.issue_key,
            )?)?)
        }
        "issue.next" => {
            let params: PrdRefParams = serde_json::from_value(request.params)?;
            Ok(serde_json::to_value(
                store.issue_next(&params.project_key, &params.prd_key)?,
            )?)
        }
        "issue.set_status" => {
            let params: IssueStatusParams = serde_json::from_value(request.params)?;
            store.set_issue_status(&params.project_key, &params.issue_key, params.status)?;
            Ok(json!({ "ok": true }))
        }
        "tracker.snapshot" => Ok(serde_json::to_value(store.snapshot()?)?),
        "import.backfill" => {
            let params: BackfillParams = serde_json::from_value(request.params)?;
            let count = store.backfill_prds_from_dir(&params.project_key, &params.path)?;
            Ok(json!({ "imported": count }))
        }
        other => anyhow::bail!("unknown tracker RPC method: {other}"),
    }
}

fn success_response(id: Option<Value>, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error_response(id: Option<Value>, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

pub fn tracker_socket_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    home.hash(&mut hasher);
    std::env::temp_dir().join(format!("piv-tracker-{:016x}.sock", hasher.finish()))
}

pub fn send_tracker_rpc_request(socket_path: &Path, request_json: &str) -> Result<String> {
    let mut stream = UnixStream::connect(socket_path)
        .with_context(|| format!("no running piv tracker at {}", socket_path.display()))?;
    stream.write_all(request_json.as_bytes())?;
    stream.write_all(b"\n")?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    Ok(response.trim_end().to_string())
}

pub struct TrackerRpcServer {
    path: PathBuf,
    _thread: thread::JoinHandle<()>,
}

impl TrackerRpcServer {
    pub fn start(socket_path: &Path, db_path: &Path) -> Result<Self> {
        if socket_path.exists() {
            let _ = fs::remove_file(socket_path);
        }
        let listener = UnixListener::bind(socket_path)
            .with_context(|| format!("failed to bind tracker socket {}", socket_path.display()))?;
        let mut store = TrackerStore::open(db_path)?;
        let path = socket_path.to_path_buf();
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let _ = handle_stream(&mut store, stream);
            }
        });
        Ok(Self {
            path,
            _thread: thread,
        })
    }
}

impl Drop for TrackerRpcServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn serve_tracker_rpc(socket_path: &Path, db_path: &Path) -> Result<()> {
    if socket_path.exists() {
        let _ = fs::remove_file(socket_path);
    }
    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("failed to bind tracker socket {}", socket_path.display()))?;
    let mut store = TrackerStore::open(db_path)?;
    for stream in listener.incoming() {
        let stream = stream?;
        handle_stream(&mut store, stream)?;
    }
    Ok(())
}

fn handle_stream(store: &mut TrackerStore, mut stream: UnixStream) -> Result<()> {
    let reader_stream = stream.try_clone()?;
    let reader = BufReader::new(reader_stream);
    for line in reader.lines() {
        let line = line?;
        let response = handle_json_rpc(store, &line)?;
        stream.write_all(response.as_bytes())?;
        stream.write_all(b"\n")?;
    }
    Ok(())
}
