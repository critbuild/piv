use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrdStatus {
    Draft,
    InProgress,
    Complete,
    Archived,
}

impl PrdStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::InProgress => "in_progress",
            Self::Complete => "complete",
            Self::Archived => "archived",
        }
    }

    pub fn cycle(&self) -> Self {
        match self {
            Self::Draft => Self::InProgress,
            Self::InProgress => Self::Complete,
            Self::Complete => Self::Draft,
            Self::Archived => Self::Draft,
        }
    }

    fn from_db(value: String) -> Result<Self> {
        match value.as_str() {
            "draft" => Ok(Self::Draft),
            "in_progress" => Ok(Self::InProgress),
            "complete" => Ok(Self::Complete),
            "archived" => Ok(Self::Archived),
            other => bail!("unknown PRD status: {other}"),
        }
    }
}

impl Default for PrdStatus {
    fn default() -> Self {
        Self::Draft
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueStatus {
    Open,
    InProgress,
    Complete,
    Canceled,
}

impl IssueStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::InProgress => "in_progress",
            Self::Complete => "complete",
            Self::Canceled => "canceled",
        }
    }

    pub fn cycle(&self) -> Self {
        match self {
            Self::Open => Self::InProgress,
            Self::InProgress => Self::Complete,
            Self::Complete => Self::Open,
            Self::Canceled => Self::Open,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Complete | Self::Canceled)
    }

    fn from_db(value: String) -> Result<Self> {
        match value.as_str() {
            "open" => Ok(Self::Open),
            "in_progress" => Ok(Self::InProgress),
            "complete" => Ok(Self::Complete),
            "canceled" => Ok(Self::Canceled),
            other => bail!("unknown issue status: {other}"),
        }
    }
}

impl Default for IssueStatus {
    fn default() -> Self {
        Self::Open
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub key: String,
    pub name: String,
    pub roots: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prd {
    pub id: String,
    pub project_id: String,
    pub key: String,
    pub title: String,
    pub status: PrdStatus,
    pub body: Option<String>,
    pub source_uri: Option<String>,
    pub issue_count: usize,
    pub completed_issue_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub id: String,
    pub project_id: String,
    pub key: String,
    pub title: String,
    pub status: IssueStatus,
    pub body: Option<String>,
    pub position: i64,
    pub blocked: bool,
    pub blocked_by_keys: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrdInput {
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub status: PrdStatus,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub source_uri: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanIssueInput {
    pub key: String,
    pub title: String,
    #[serde(default)]
    pub status: IssueStatus,
    #[serde(default)]
    pub body: Option<String>,
    pub position: i64,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerSnapshot {
    pub projects: Vec<ProjectSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSnapshot {
    pub project: Project,
    pub prds: Vec<PrdSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrdSnapshot {
    pub prd: Prd,
    pub issues: Vec<Issue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrdDetails {
    pub prd: Prd,
    pub issues: Vec<Issue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueDetails {
    pub issue: Issue,
    pub prds: Vec<Prd>,
}

pub struct TrackerStore {
    conn: Connection,
}

impl TrackerStore {
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("open in-memory tracker database")?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create tracker data dir {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open tracker database {}", path.display()))?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_default() -> Result<Self> {
        Self::open(default_db_path()?)
    }

    pub fn create_project(&mut self, key: &str, name: &str, roots: &[&str]) -> Result<Project> {
        validate_key("project", key)?;
        let tx = self.conn.transaction()?;
        let project = upsert_project_tx(&tx, key, name, roots)?;
        tx.commit()?;
        Ok(project)
    }

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, key, name FROM projects ORDER BY name, key")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut projects = Vec::new();
        for row in rows {
            let (id, key, name) = row?;
            projects.push(Project {
                roots: roots_for_project(&self.conn, &id)?,
                id,
                key,
                name,
            });
        }
        Ok(projects)
    }

    pub fn upsert_plan(
        &mut self,
        project_key: &str,
        prd: PrdInput,
        issues: Vec<PlanIssueInput>,
    ) -> Result<Prd> {
        validate_key("project", project_key)?;
        validate_key("prd", &prd.key)?;
        validate_plan_issues(&issues)?;
        let tx = self.conn.transaction()?;
        let project_id = project_id_by_key_tx(&tx, project_key)?
            .with_context(|| format!("project not found: {project_key}"))?;
        let prd_id = upsert_prd_tx(&tx, &project_id, &prd)?;

        let mut issue_ids_by_key = HashMap::new();
        for issue in &issues {
            let issue_id = upsert_issue_tx(&tx, &project_id, issue)?;
            issue_ids_by_key.insert(issue.key.clone(), issue_id);
        }

        tx.execute("DELETE FROM prd_issues WHERE prd_id = ?1", params![prd_id])?;
        for issue in &issues {
            let issue_id = issue_ids_by_key
                .get(&issue.key)
                .expect("validated issue key");
            tx.execute(
                "INSERT INTO prd_issues (prd_id, issue_id, position) VALUES (?1, ?2, ?3)",
                params![prd_id, issue_id, issue.position],
            )?;
        }

        for issue in &issues {
            let issue_id = issue_ids_by_key
                .get(&issue.key)
                .expect("validated issue key");
            tx.execute(
                "DELETE FROM issue_blockers WHERE issue_id = ?1",
                params![issue_id],
            )?;
            for blocker_key in &issue.depends_on {
                let Some(blocker_id) = issue_ids_by_key.get(blocker_key) else {
                    bail!("unknown blocker key for {}: {blocker_key}", issue.key);
                };
                tx.execute(
                    "INSERT OR IGNORE INTO issue_blockers (issue_id, blocked_by_issue_id) VALUES (?1, ?2)",
                    params![issue_id, blocker_id],
                )?;
            }
        }

        ensure_no_blocker_cycles_tx(&tx, &issue_ids_by_key)?;
        insert_event_tx(
            &tx,
            "prd.upsert_plan",
            &format!("{project_key}/{}", prd.key),
        )?;
        let stored = prd_by_id_tx(&tx, &prd_id)?;
        tx.commit()?;
        Ok(stored)
    }

    pub fn list_prds(&self, project_key: &str) -> Result<Vec<Prd>> {
        let project_id = self
            .project_id_by_key(project_key)?
            .with_context(|| format!("project not found: {project_key}"))?;
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, key, title, status, body, source_uri
             FROM prds WHERE project_id = ?1 ORDER BY title, key",
        )?;
        let rows = stmt.query_map(params![project_id], |row| prd_from_row(row))?;
        let mut prds = Vec::new();
        for row in rows {
            let mut prd = row??;
            fill_prd_counts(&self.conn, &mut prd)?;
            prds.push(prd);
        }
        Ok(prds)
    }

    pub fn list_issues(&self, project_key: &str, prd_key: &str) -> Result<Vec<Issue>> {
        let (project_id, prd_id) = self.project_and_prd_ids(project_key, prd_key)?;
        let mut stmt = self.conn.prepare(
            "SELECT issues.id, issues.project_id, issues.key, issues.title, issues.status, issues.body, prd_issues.position
             FROM prd_issues
             JOIN issues ON issues.id = prd_issues.issue_id
             WHERE prd_issues.prd_id = ?1 AND issues.project_id = ?2
             ORDER BY prd_issues.position, issues.key",
        )?;
        let rows = stmt.query_map(params![prd_id, project_id], |row| issue_from_row(row))?;
        let mut issues = Vec::new();
        for row in rows {
            let mut issue = row??;
            fill_blocker_state(&self.conn, &mut issue)?;
            issues.push(issue);
        }
        Ok(issues)
    }

    pub fn get_prd(&self, project_key: &str, prd_key: &str) -> Result<PrdDetails> {
        let (_, prd_id) = self.project_and_prd_ids(project_key, prd_key)?;
        let mut prd = self.conn.query_row(
            "SELECT id, project_id, key, title, status, body, source_uri FROM prds WHERE id = ?1",
            params![prd_id],
            prd_from_row,
        )??;
        fill_prd_counts(&self.conn, &mut prd)?;
        let issues = self.list_issues(project_key, prd_key)?;
        Ok(PrdDetails { prd, issues })
    }

    pub fn get_issue(&self, project_key: &str, issue_key: &str) -> Result<IssueDetails> {
        let project_id = self
            .project_id_by_key(project_key)?
            .with_context(|| format!("project not found: {project_key}"))?;
        let mut issue = self
            .conn
            .query_row(
                "SELECT issues.id, issues.project_id, issues.key, issues.title, issues.status, issues.body, MIN(prd_issues.position)
                 FROM issues
                 JOIN prd_issues ON prd_issues.issue_id = issues.id
                 WHERE issues.project_id = ?1 AND issues.key = ?2
                 GROUP BY issues.id, issues.project_id, issues.key, issues.title, issues.status, issues.body",
                params![project_id, issue_key],
                issue_from_row,
            )
            .optional()?
            .with_context(|| format!("issue not found: {project_key}/{issue_key}"))??;
        fill_blocker_state(&self.conn, &mut issue)?;

        let mut stmt = self.conn.prepare(
            "SELECT prds.id, prds.project_id, prds.key, prds.title, prds.status, prds.body, prds.source_uri
             FROM prd_issues
             JOIN prds ON prds.id = prd_issues.prd_id
             WHERE prd_issues.issue_id = ?1
             ORDER BY prd_issues.position, prds.title, prds.key",
        )?;
        let rows = stmt.query_map(params![issue.id], prd_from_row)?;
        let mut prds = Vec::new();
        for row in rows {
            let mut prd = row??;
            fill_prd_counts(&self.conn, &mut prd)?;
            prds.push(prd);
        }
        Ok(IssueDetails { issue, prds })
    }

    pub fn issue_next(&self, project_key: &str, prd_key: &str) -> Result<Option<Issue>> {
        Ok(self
            .list_issues(project_key, prd_key)?
            .into_iter()
            .find(|issue| !issue.status.is_terminal() && !issue.blocked))
    }

    pub fn set_issue_status(
        &mut self,
        project_key: &str,
        issue_key: &str,
        status: IssueStatus,
    ) -> Result<()> {
        let project_id = self
            .project_id_by_key(project_key)?
            .with_context(|| format!("project not found: {project_key}"))?;
        let changed = self.conn.execute(
            "UPDATE issues SET status = ?1, updated_at = ?2 WHERE project_id = ?3 AND key = ?4",
            params![status.as_str(), now_ms(), project_id, issue_key],
        )?;
        if changed == 0 {
            bail!("issue not found: {project_key}/{issue_key}");
        }
        Ok(())
    }

    pub fn set_prd_status(
        &mut self,
        project_key: &str,
        prd_key: &str,
        status: PrdStatus,
    ) -> Result<()> {
        let (project_id, prd_id) = self.project_and_prd_ids(project_key, prd_key)?;
        self.conn.execute(
            "UPDATE prds SET status = ?1, updated_at = ?2 WHERE project_id = ?3 AND id = ?4",
            params![status.as_str(), now_ms(), project_id, prd_id],
        )?;
        Ok(())
    }

    pub fn snapshot(&self) -> Result<TrackerSnapshot> {
        let projects = self
            .list_projects()?
            .into_iter()
            .map(|project| {
                let prds = self
                    .list_prds(&project.key)?
                    .into_iter()
                    .map(|prd| {
                        let issues = self.list_issues(&project.key, &prd.key)?;
                        Ok(PrdSnapshot { prd, issues })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(ProjectSnapshot { project, prds })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(TrackerSnapshot { projects })
    }

    pub fn backfill_prds_from_dir(&mut self, project_key: &str, dir: &Path) -> Result<usize> {
        let mut count = 0;
        for entry in std::fs::read_dir(dir)
            .with_context(|| format!("read backfill dir {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            let title = content
                .lines()
                .find_map(|line| line.strip_prefix("# "))
                .unwrap_or_else(|| {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Untitled PRD")
                });
            let key = slugify(path.file_stem().and_then(|s| s.to_str()).unwrap_or(title));
            self.upsert_plan(
                project_key,
                PrdInput {
                    key,
                    title: title.to_string(),
                    status: PrdStatus::Draft,
                    body: Some(content),
                    source_uri: Some(path.display().to_string()),
                },
                Vec::new(),
            )?;
            count += 1;
        }
        Ok(count)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY,
                key TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS project_roots (
                project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                path TEXT NOT NULL,
                PRIMARY KEY (project_id, path)
             );
             CREATE TABLE IF NOT EXISTS project_sources (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                kind TEXT NOT NULL,
                uri TEXT NOT NULL,
                UNIQUE (project_id, kind, uri)
             );
             CREATE TABLE IF NOT EXISTS prds (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                key TEXT NOT NULL,
                title TEXT NOT NULL,
                status TEXT NOT NULL,
                body TEXT,
                source_uri TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE (project_id, key)
             );
             CREATE TABLE IF NOT EXISTS issues (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                key TEXT NOT NULL,
                title TEXT NOT NULL,
                status TEXT NOT NULL,
                body TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE (project_id, key)
             );
             CREATE TABLE IF NOT EXISTS prd_issues (
                prd_id TEXT NOT NULL REFERENCES prds(id) ON DELETE CASCADE,
                issue_id TEXT NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                position INTEGER NOT NULL,
                PRIMARY KEY (prd_id, issue_id),
                UNIQUE (prd_id, position)
             );
             CREATE TABLE IF NOT EXISTS issue_blockers (
                issue_id TEXT NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                blocked_by_issue_id TEXT NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                PRIMARY KEY (issue_id, blocked_by_issue_id)
             );
             CREATE TABLE IF NOT EXISTS events (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                subject TEXT NOT NULL,
                created_at INTEGER NOT NULL
             );",
        )?;
        Ok(())
    }

    fn project_id_by_key(&self, project_key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT id FROM projects WHERE key = ?1",
                params![project_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    fn project_and_prd_ids(&self, project_key: &str, prd_key: &str) -> Result<(String, String)> {
        let project_id = self
            .project_id_by_key(project_key)?
            .with_context(|| format!("project not found: {project_key}"))?;
        let prd_id = self
            .conn
            .query_row(
                "SELECT id FROM prds WHERE project_id = ?1 AND key = ?2",
                params![project_id, prd_key],
                |row| row.get(0),
            )
            .optional()?
            .with_context(|| format!("PRD not found: {project_key}/{prd_key}"))?;
        Ok((project_id, prd_id))
    }
}

fn upsert_project_tx(
    tx: &Transaction<'_>,
    key: &str,
    name: &str,
    roots: &[&str],
) -> Result<Project> {
    let now = now_ms();
    let existing: Option<String> = tx
        .query_row(
            "SELECT id FROM projects WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?;
    let id = existing.unwrap_or_else(|| new_id("proj"));
    tx.execute(
        "INSERT INTO projects (id, key, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(key) DO UPDATE SET name = excluded.name, updated_at = excluded.updated_at",
        params![id, key, name, now],
    )?;
    tx.execute(
        "DELETE FROM project_roots WHERE project_id = ?1",
        params![id],
    )?;
    for root in roots {
        tx.execute(
            "INSERT OR IGNORE INTO project_roots (project_id, path) VALUES (?1, ?2)",
            params![id, root],
        )?;
    }
    Ok(Project {
        id,
        key: key.into(),
        name: name.into(),
        roots: roots.iter().map(|root| (*root).into()).collect(),
    })
}

fn project_id_by_key_tx(tx: &Transaction<'_>, project_key: &str) -> Result<Option<String>> {
    tx.query_row(
        "SELECT id FROM projects WHERE key = ?1",
        params![project_key],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn upsert_prd_tx(tx: &Transaction<'_>, project_id: &str, prd: &PrdInput) -> Result<String> {
    let now = now_ms();
    let existing: Option<String> = tx
        .query_row(
            "SELECT id FROM prds WHERE project_id = ?1 AND key = ?2",
            params![project_id, prd.key],
            |row| row.get(0),
        )
        .optional()?;
    let id = existing.unwrap_or_else(|| new_id("prd"));
    tx.execute(
        "INSERT INTO prds (id, project_id, key, title, status, body, source_uri, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
         ON CONFLICT(project_id, key) DO UPDATE SET
            title = excluded.title,
            status = excluded.status,
            body = excluded.body,
            source_uri = excluded.source_uri,
            updated_at = excluded.updated_at",
        params![id, project_id, prd.key, prd.title, prd.status.as_str(), prd.body, prd.source_uri, now],
    )?;
    Ok(id)
}

fn upsert_issue_tx(
    tx: &Transaction<'_>,
    project_id: &str,
    issue: &PlanIssueInput,
) -> Result<String> {
    let now = now_ms();
    let existing: Option<String> = tx
        .query_row(
            "SELECT id FROM issues WHERE project_id = ?1 AND key = ?2",
            params![project_id, issue.key],
            |row| row.get(0),
        )
        .optional()?;
    let id = existing.unwrap_or_else(|| new_id("iss"));
    tx.execute(
        "INSERT INTO issues (id, project_id, key, title, status, body, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
         ON CONFLICT(project_id, key) DO UPDATE SET
            title = excluded.title,
            status = excluded.status,
            body = excluded.body,
            updated_at = excluded.updated_at",
        params![
            id,
            project_id,
            issue.key,
            issue.title,
            issue.status.as_str(),
            issue.body,
            now
        ],
    )?;
    Ok(id)
}

fn prd_by_id_tx(tx: &Transaction<'_>, prd_id: &str) -> Result<Prd> {
    let mut prd = tx.query_row(
        "SELECT id, project_id, key, title, status, body, source_uri FROM prds WHERE id = ?1",
        params![prd_id],
        prd_from_row,
    )??;
    fill_prd_counts(tx, &mut prd)?;
    Ok(prd)
}

fn prd_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Prd>> {
    Ok(Ok(Prd {
        id: row.get(0)?,
        project_id: row.get(1)?,
        key: row.get(2)?,
        title: row.get(3)?,
        status: PrdStatus::from_db(row.get(4)?).map_err(to_sql_err)?,
        body: row.get(5)?,
        source_uri: row.get(6)?,
        issue_count: 0,
        completed_issue_count: 0,
    }))
}

fn issue_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Issue>> {
    Ok(Ok(Issue {
        id: row.get(0)?,
        project_id: row.get(1)?,
        key: row.get(2)?,
        title: row.get(3)?,
        status: IssueStatus::from_db(row.get(4)?).map_err(to_sql_err)?,
        body: row.get(5)?,
        position: row.get(6)?,
        blocked: false,
        blocked_by_keys: Vec::new(),
    }))
}

fn fill_prd_counts(conn: &Connection, prd: &mut Prd) -> Result<()> {
    prd.issue_count = conn.query_row(
        "SELECT COUNT(*) FROM prd_issues WHERE prd_id = ?1",
        params![prd.id],
        |row| row.get::<_, i64>(0),
    )? as usize;
    prd.completed_issue_count = conn.query_row(
        "SELECT COUNT(*)
         FROM prd_issues JOIN issues ON issues.id = prd_issues.issue_id
         WHERE prd_issues.prd_id = ?1 AND issues.status = 'complete'",
        params![prd.id],
        |row| row.get::<_, i64>(0),
    )? as usize;
    Ok(())
}

fn fill_blocker_state(conn: &Connection, issue: &mut Issue) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT blocked.key, blocked.status
         FROM issue_blockers
         JOIN issues AS blocked ON blocked.id = issue_blockers.blocked_by_issue_id
         WHERE issue_blockers.issue_id = ?1
         ORDER BY blocked.key",
    )?;
    let rows = stmt.query_map(params![issue.id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut blocked = false;
    let mut keys = Vec::new();
    for row in rows {
        let (key, status) = row?;
        let status = IssueStatus::from_db(status)?;
        if !status.is_terminal() {
            blocked = true;
        }
        keys.push(key);
    }
    issue.blocked = blocked;
    issue.blocked_by_keys = keys;
    Ok(())
}

fn roots_for_project(conn: &Connection, project_id: &str) -> Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT path FROM project_roots WHERE project_id = ?1 ORDER BY path")?;
    let rows = stmt.query_map(params![project_id], |row| row.get(0))?;
    let mut roots = Vec::new();
    for row in rows {
        roots.push(row?);
    }
    Ok(roots)
}

fn ensure_no_blocker_cycles_tx(
    tx: &Transaction<'_>,
    issue_ids: &HashMap<String, String>,
) -> Result<()> {
    for issue_id in issue_ids.values() {
        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();
        if has_cycle_from_tx(tx, issue_id, &mut visiting, &mut visited)? {
            bail!("issue blocker cycle detected");
        }
    }
    Ok(())
}

fn has_cycle_from_tx(
    tx: &Transaction<'_>,
    issue_id: &str,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
) -> Result<bool> {
    if visiting.contains(issue_id) {
        return Ok(true);
    }
    if visited.contains(issue_id) {
        return Ok(false);
    }
    visiting.insert(issue_id.to_string());
    let blockers = blocker_ids_tx(tx, issue_id)?;
    for blocker in blockers {
        if has_cycle_from_tx(tx, &blocker, visiting, visited)? {
            return Ok(true);
        }
    }
    visiting.remove(issue_id);
    visited.insert(issue_id.to_string());
    Ok(false)
}

fn blocker_ids_tx(tx: &Transaction<'_>, issue_id: &str) -> Result<Vec<String>> {
    let mut stmt =
        tx.prepare("SELECT blocked_by_issue_id FROM issue_blockers WHERE issue_id = ?1")?;
    let rows = stmt.query_map(params![issue_id], |row| row.get(0))?;
    let mut blockers = Vec::new();
    for row in rows {
        blockers.push(row?);
    }
    Ok(blockers)
}

fn insert_event_tx(tx: &Transaction<'_>, kind: &str, subject: &str) -> Result<()> {
    tx.execute(
        "INSERT INTO events (id, kind, subject, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![new_id("evt"), kind, subject, now_ms()],
    )?;
    Ok(())
}

fn validate_plan_issues(issues: &[PlanIssueInput]) -> Result<()> {
    let mut keys = BTreeSet::new();
    let mut positions = BTreeSet::new();
    for issue in issues {
        validate_key("issue", &issue.key)?;
        if issue.title.trim().is_empty() {
            bail!("issue title must not be empty: {}", issue.key);
        }
        if !keys.insert(issue.key.as_str()) {
            bail!("duplicate issue key: {}", issue.key);
        }
        if !positions.insert(issue.position) {
            bail!("duplicate issue position: {}", issue.position);
        }
    }
    Ok(())
}

fn validate_key(kind: &str, key: &str) -> Result<()> {
    if key.trim().is_empty() {
        bail!("{kind} key must not be empty");
    }
    if key.chars().any(char::is_whitespace) {
        bail!("{kind} key must not contain whitespace: {key}");
    }
    Ok(())
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for c in value.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

pub fn default_db_path() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        return Ok(PathBuf::from(xdg).join("piv/tracker.sqlite"));
    }
    let home =
        std::env::var("HOME").context("HOME is not set; cannot choose default tracker path")?;
    Ok(PathBuf::from(home).join(".local/share/piv/tracker.sqlite"))
}

fn new_id(prefix: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let seq = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{now:x}_{seq:x}")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn to_sql_err(error: anyhow::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error.to_string(),
        )),
    )
}
