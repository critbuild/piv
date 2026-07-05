use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use ratatui::{
    style::{Color, Modifier, Style},
    text::Line,
};

use crate::tracker::{Issue, IssueStatus, PrdSnapshot, ProjectSnapshot, TrackerSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CockpitScopeMode {
    Auto,
    AllProjects,
}

impl Default for CockpitScopeMode {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueCockpitState {
    pub open: bool,
    pub scope_mode: CockpitScopeMode,
    pub selected_ref: usize,
}

impl Default for IssueCockpitState {
    fn default() -> Self {
        Self {
            open: false,
            scope_mode: CockpitScopeMode::Auto,
            selected_ref: 0,
        }
    }
}

impl IssueCockpitState {
    pub fn toggle_open(&mut self) {
        self.open = !self.open;
        if !self.open {
            self.selected_ref = 0;
        }
    }

    pub fn toggle_scope(&mut self) {
        self.scope_mode = match self.scope_mode {
            CockpitScopeMode::Auto => CockpitScopeMode::AllProjects,
            CockpitScopeMode::AllProjects => CockpitScopeMode::Auto,
        };
        self.selected_ref = 0;
    }

    pub fn select_next_ref(&mut self, ref_count: usize) {
        if ref_count == 0 {
            self.selected_ref = 0;
        } else {
            self.selected_ref = (self.selected_ref + 1) % ref_count;
        }
    }

    pub fn select_prev_ref(&mut self, ref_count: usize) {
        if ref_count == 0 {
            self.selected_ref = 0;
        } else {
            self.selected_ref = (self.selected_ref + ref_count - 1) % ref_count;
        }
    }

    pub fn clamp_selected_ref(&mut self, ref_count: usize) {
        if ref_count == 0 {
            self.selected_ref = 0;
        } else if self.selected_ref >= ref_count {
            self.selected_ref = ref_count - 1;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectRootMatch {
    pub project_key: String,
    pub project_name: String,
    pub root: PathBuf,
    pub matched_len: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectiveScope {
    Project {
        project_key: String,
        project_name: String,
    },
    AllProjects {
        fallback_no_match: bool,
        matched_project_key: Option<String>,
    },
}

impl EffectiveScope {
    pub fn status_label(&self) -> String {
        match self {
            EffectiveScope::Project { project_key, .. } => project_key.clone(),
            EffectiveScope::AllProjects {
                fallback_no_match: true,
                ..
            } => "all (no root match)".to_string(),
            EffectiveScope::AllProjects { .. } => "all".to_string(),
        }
    }

    pub fn drawer_label(&self) -> String {
        match self {
            EffectiveScope::Project {
                project_key,
                project_name,
            } => format!("current project {project_name} ({project_key})"),
            EffectiveScope::AllProjects {
                fallback_no_match: true,
                ..
            } => "all projects — no registered root matched".to_string(),
            EffectiveScope::AllProjects { .. } => "all projects".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueContext {
    pub project_key: String,
    pub project_name: String,
    pub project_roots: Vec<String>,
    pub prd_key: String,
    pub prd_title: String,
    pub prd_body: Option<String>,
    pub issue_key: String,
    pub issue_title: String,
    pub issue_body: Option<String>,
    pub status: IssueStatus,
    pub position: i64,
    pub blocked: bool,
    pub blocked_by_keys: Vec<String>,
}

impl IssueContext {
    pub fn status_label(&self) -> &'static str {
        issue_status_label(self.blocked, &self.status)
    }

    pub fn is_current(&self) -> bool {
        self.status == IssueStatus::InProgress
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileReference {
    pub path: PathBuf,
    pub display: String,
    pub start_line: usize,
    pub end_line: usize,
}

impl FileReference {
    pub fn is_range(&self) -> bool {
        self.start_line != self.end_line
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueCockpitView {
    pub scope: EffectiveScope,
    pub fallback_notice: Option<String>,
    pub focus: Option<IssueContext>,
    pub siblings: Vec<IssueContext>,
    pub refs: Vec<FileReference>,
    pub project_count: usize,
    pub prd_count: usize,
    pub issue_count: usize,
}

pub fn match_project_for_root(
    snapshot: &TrackerSnapshot,
    watched_root: &Path,
) -> Option<ProjectRootMatch> {
    let watched_root = normalize_path(watched_root);
    let root_match = snapshot
        .projects
        .iter()
        .flat_map(|project| {
            project.project.roots.iter().filter_map(|root| {
                let root = normalize_project_root(root)?;
                if !watched_root.starts_with(&root) {
                    return None;
                }
                Some(ProjectRootMatch {
                    project_key: project.project.key.clone(),
                    project_name: project.project.name.clone(),
                    matched_len: root.components().count(),
                    root,
                })
            })
        })
        .max_by(|a, b| {
            a.matched_len
                .cmp(&b.matched_len)
                .then_with(|| a.project_key.cmp(&b.project_key).reverse())
        });
    root_match.or_else(|| fuzzy_project_match(snapshot, &watched_root))
}

fn fuzzy_project_match(
    snapshot: &TrackerSnapshot,
    watched_root: &Path,
) -> Option<ProjectRootMatch> {
    let components = watched_root
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .map(|component| component.to_ascii_lowercase())
        .collect::<Vec<_>>();
    snapshot
        .projects
        .iter()
        .filter_map(|project| {
            let key = project.project.key.to_ascii_lowercase();
            let name = project.project.name.to_ascii_lowercase();
            let matched_len = components
                .iter()
                .rposition(|component| component == &key || component == &name)?;
            Some(ProjectRootMatch {
                project_key: project.project.key.clone(),
                project_name: project.project.name.clone(),
                root: watched_root.to_path_buf(),
                matched_len,
            })
        })
        .max_by(|a, b| {
            a.matched_len
                .cmp(&b.matched_len)
                .then_with(|| a.project_key.cmp(&b.project_key).reverse())
        })
}

pub fn build_issue_cockpit_view(
    snapshot: &TrackerSnapshot,
    watched_root: &Path,
    state: &IssueCockpitState,
) -> IssueCockpitView {
    let matched = match_project_for_root(snapshot, watched_root);
    let scope = match (&state.scope_mode, matched.as_ref()) {
        (CockpitScopeMode::Auto, Some(matched)) => EffectiveScope::Project {
            project_key: matched.project_key.clone(),
            project_name: matched.project_name.clone(),
        },
        (CockpitScopeMode::Auto, None) => EffectiveScope::AllProjects {
            fallback_no_match: true,
            matched_project_key: None,
        },
        (CockpitScopeMode::AllProjects, matched) => EffectiveScope::AllProjects {
            fallback_no_match: false,
            matched_project_key: matched.map(|matched| matched.project_key.clone()),
        },
    };

    let fallback_notice = match &scope {
        EffectiveScope::AllProjects {
            fallback_no_match: true,
            ..
        } => Some(format!(
            "No registered Project root matched {}; showing all projects.",
            watched_root.display()
        )),
        _ => None,
    };

    let project_count = scoped_projects(snapshot, &scope).len();
    let prd_count = scoped_projects(snapshot, &scope)
        .iter()
        .map(|project| project.prds.len())
        .sum();
    let contexts = issue_contexts(snapshot, &scope);
    let issue_count = contexts.len();
    let focus = choose_focus_issue(&contexts).cloned();
    let siblings = focus
        .as_ref()
        .map(|focus| {
            contexts
                .iter()
                .filter(|context| {
                    context.project_key == focus.project_key && context.prd_key == focus.prd_key
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let refs = focus
        .as_ref()
        .map(|focus| references_for_issue_context(focus, watched_root))
        .unwrap_or_default();

    IssueCockpitView {
        scope,
        fallback_notice,
        focus,
        siblings,
        refs,
        project_count,
        prd_count,
        issue_count,
    }
}

pub fn tracker_status_fragment(
    snapshot: &TrackerSnapshot,
    watched_root: &Path,
    state: &IssueCockpitState,
) -> String {
    let view = build_issue_cockpit_view(snapshot, watched_root, state);
    match view.focus.as_ref() {
        Some(issue) => format!(
            "tracker {}: {} {} — {}",
            view.scope.status_label(),
            issue.status_label(),
            issue.issue_key,
            issue.issue_title
        ),
        None if view.project_count == 0 => "tracker no projects".to_string(),
        None if view.issue_count == 0 => {
            format!("tracker {}: no issues", view.scope.status_label())
        }
        None => format!("tracker {}: no open issues", view.scope.status_label()),
    }
}

pub fn render_issue_cockpit(
    snapshot: &TrackerSnapshot,
    watched_root: &Path,
    state: &IssueCockpitState,
    width: usize,
    height: usize,
) -> Vec<String> {
    let view = build_issue_cockpit_view(snapshot, watched_root, state);
    render_issue_cockpit_view(&view, state, width, height)
}

pub fn render_issue_cockpit_lines(
    snapshot: &TrackerSnapshot,
    watched_root: &Path,
    state: &IssueCockpitState,
    width: usize,
    height: usize,
) -> Vec<Line<'static>> {
    render_issue_cockpit(snapshot, watched_root, state, width, height)
        .into_iter()
        .map(styled_cockpit_line)
        .collect()
}

pub fn render_issue_cockpit_view(
    view: &IssueCockpitView,
    state: &IssueCockpitState,
    width: usize,
    height: usize,
) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    rows.push(fit_width(
        format!("Issue Cockpit — {}", view.scope.drawer_label()),
        width,
    ));
    rows.push("─".repeat(width));
    rows.push(fit_width(
        "keys  i close · a scope · r/R refs · Enter open ref".to_string(),
        width,
    ));
    if let Some(notice) = &view.fallback_notice {
        rows.push(fit_width(format!("◆ {notice}"), width));
    }

    match &view.focus {
        Some(issue) => {
            let label = if issue.is_current() {
                "Current"
            } else {
                "Next"
            };
            rows.push(fit_width(
                format!(
                    "{label}  {} {} — {}",
                    issue.status_label(),
                    issue.issue_key,
                    issue.issue_title
                ),
                width,
            ));
            rows.push(fit_width(
                format!("PRD      {} / {}", issue.project_key, issue.prd_title),
                width,
            ));
            if issue.blocked {
                rows.push(fit_width(
                    format!("Blocked  by {}", issue.blocked_by_keys.join(",")),
                    width,
                ));
            } else if !issue.blocked_by_keys.is_empty() {
                rows.push(fit_width(
                    format!("Depends  on {}", issue.blocked_by_keys.join(",")),
                    width,
                ));
            }
            if let Some(preview) = body_preview(issue.issue_body.as_deref(), 2) {
                rows.push(fit_width(format!("Notes    {preview}"), width));
            }
            rows.push(String::new());
            rows.push("Issues".to_string());
            for sibling in &view.siblings {
                let marker = if sibling.issue_key == issue.issue_key {
                    "→"
                } else {
                    " "
                };
                let blockers = if sibling.blocked {
                    format!(" blocked by {}", sibling.blocked_by_keys.join(","))
                } else {
                    String::new()
                };
                rows.push(fit_width(
                    format!(
                        "{marker} {}. {} {} — {}{}",
                        sibling.position,
                        sibling.status_label(),
                        sibling.issue_key,
                        sibling.issue_title,
                        blockers
                    ),
                    width,
                ));
            }
            if !view.refs.is_empty() {
                rows.push(String::new());
                rows.push("Refs".to_string());
                for (index, reference) in view.refs.iter().enumerate() {
                    let marker = if index == state.selected_ref {
                        "›"
                    } else {
                        " "
                    };
                    rows.push(fit_width(
                        format!("{marker} {}. {}", index + 1, reference.display),
                        width,
                    ));
                }
            }
        }
        None if view.project_count == 0 => {
            rows.push("No tracker projects yet.".to_string());
            rows.push("Ask an AI agent to call project.create or prd.upsert_plan.".to_string());
        }
        None if view.issue_count == 0 => {
            rows.push(format!(
                "No Issues in {} PRDs across {} Projects.",
                view.prd_count, view.project_count
            ));
        }
        None => {
            rows.push("No open or in-progress Issues.".to_string());
            rows.push(format!(
                "{} Issues across {} PRDs are complete, canceled, or blocked.",
                view.issue_count, view.prd_count
            ));
        }
    }

    rows.truncate(height);
    while rows.len() < height {
        rows.push(String::new());
    }
    rows
}

pub fn extract_file_references(text: &str, watched_root: &Path) -> Vec<FileReference> {
    let mut refs = Vec::new();
    let mut seen = BTreeSet::new();
    for token in text.split_whitespace() {
        let token = trim_reference_token(token);
        let Some(reference) = parse_reference_token(token, watched_root) else {
            continue;
        };
        let key = (
            reference.path.clone(),
            reference.start_line,
            reference.end_line,
        );
        if seen.insert(key) {
            refs.push(reference);
        }
    }
    refs
}

pub fn references_for_issue_context(
    context: &IssueContext,
    watched_root: &Path,
) -> Vec<FileReference> {
    let roots = reference_roots(context, watched_root);
    let mut refs = Vec::new();
    let mut seen = BTreeSet::new();
    for body in [context.issue_body.as_deref(), context.prd_body.as_deref()]
        .into_iter()
        .flatten()
    {
        for root in &roots {
            for reference in extract_file_references(body, root) {
                let key = (
                    reference.path.clone(),
                    reference.start_line,
                    reference.end_line,
                );
                if seen.insert(key) {
                    refs.push(reference);
                }
            }
        }
    }
    refs
}

fn reference_roots(context: &IssueContext, watched_root: &Path) -> Vec<PathBuf> {
    let roots = context
        .project_roots
        .iter()
        .filter_map(|root| normalize_project_root(root))
        .collect::<Vec<_>>();
    if roots.is_empty() {
        vec![normalize_path(watched_root)]
    } else {
        roots
    }
}

fn scoped_projects<'a>(
    snapshot: &'a TrackerSnapshot,
    scope: &EffectiveScope,
) -> Vec<&'a ProjectSnapshot> {
    match scope {
        EffectiveScope::Project { project_key, .. } => snapshot
            .projects
            .iter()
            .filter(|project| &project.project.key == project_key)
            .collect(),
        EffectiveScope::AllProjects { .. } => snapshot.projects.iter().collect(),
    }
}

fn issue_contexts(snapshot: &TrackerSnapshot, scope: &EffectiveScope) -> Vec<IssueContext> {
    scoped_projects(snapshot, scope)
        .into_iter()
        .flat_map(|project| {
            project
                .prds
                .iter()
                .flat_map(move |prd| contexts_for_prd(project, prd))
        })
        .collect()
}

fn contexts_for_prd(project: &ProjectSnapshot, prd: &PrdSnapshot) -> Vec<IssueContext> {
    prd.issues
        .iter()
        .map(|issue| context_for_issue(project, prd, issue))
        .collect()
}

fn context_for_issue(project: &ProjectSnapshot, prd: &PrdSnapshot, issue: &Issue) -> IssueContext {
    IssueContext {
        project_key: project.project.key.clone(),
        project_name: project.project.name.clone(),
        project_roots: project.project.roots.clone(),
        prd_key: prd.prd.key.clone(),
        prd_title: prd.prd.title.clone(),
        prd_body: prd.prd.body.clone(),
        issue_key: issue.key.clone(),
        issue_title: issue.title.clone(),
        issue_body: issue.body.clone(),
        status: issue.status.clone(),
        position: issue.position,
        blocked: issue.blocked,
        blocked_by_keys: issue.blocked_by_keys.clone(),
    }
}

fn choose_focus_issue(contexts: &[IssueContext]) -> Option<&IssueContext> {
    contexts
        .iter()
        .find(|issue| issue.status == IssueStatus::InProgress)
        .or_else(|| {
            contexts
                .iter()
                .find(|issue| !issue.status.is_terminal() && !issue.blocked)
        })
}

fn issue_status_label(blocked: bool, status: &IssueStatus) -> &'static str {
    if blocked {
        return "◆ blocked";
    }
    match status {
        IssueStatus::Open => "◌ open",
        IssueStatus::InProgress => "● in progress",
        IssueStatus::Complete => "✓ complete",
        IssueStatus::Canceled => "− canceled",
    }
}

fn body_preview(body: Option<&str>, max_lines: usize) -> Option<String> {
    let lines = body?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with("## Parent") && !line.starts_with("## Blocked by"))
        .take(max_lines)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        None
    } else {
        Some(lines.join(" / "))
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn normalize_project_root(root: &str) -> Option<PathBuf> {
    if root.trim().is_empty() || root.contains("://") {
        return None;
    }
    Some(normalize_path(Path::new(root)))
}

fn trim_reference_token(token: &str) -> &str {
    fn trim_wrappers(token: &str) -> &str {
        token.trim_matches(|c: char| {
            matches!(
                c,
                '`' | '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | ';'
            )
        })
    }

    trim_wrappers(trim_wrappers(token).trim_end_matches('.'))
}

fn parse_reference_token(token: &str, watched_root: &Path) -> Option<FileReference> {
    let (path_part, line_part) = token.rsplit_once(':')?;
    if path_part.is_empty() || line_part.is_empty() {
        return None;
    }
    let (start_line, end_line) = parse_line_range(line_part)?;
    let path_part = path_part.strip_prefix("file://").unwrap_or(path_part);
    let candidate = Path::new(path_part);
    let raw_path = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        watched_root.join(candidate)
    };
    let path = raw_path.canonicalize().ok()?;
    if !path.is_file() {
        return None;
    }
    let root = watched_root.canonicalize().ok()?;
    if !path.starts_with(&root) {
        return None;
    }
    let rel_display = path
        .strip_prefix(&root)
        .unwrap_or(&path)
        .display()
        .to_string();
    let range = if start_line == end_line {
        start_line.to_string()
    } else {
        format!("{start_line}-{end_line}")
    };
    Some(FileReference {
        path,
        display: format!("{rel_display}:{range}"),
        start_line,
        end_line,
    })
}

fn parse_line_range(value: &str) -> Option<(usize, usize)> {
    if let Some((start, end)) = value.split_once('-') {
        let start = parse_line_number(start)?;
        let end = parse_line_number(end)?;
        if end < start {
            return None;
        }
        Some((start, end))
    } else {
        let line = parse_line_number(value)?;
        Some((line, line))
    }
}

fn parse_line_number(value: &str) -> Option<usize> {
    if value.is_empty() || !value.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let line = value.parse::<usize>().ok()?;
    (line > 0).then_some(line)
}

fn styled_cockpit_line(row: String) -> Line<'static> {
    let trimmed = row.trim_start();
    let style = if trimmed.starts_with("Issue Cockpit") || trimmed == "Issues" || trimmed == "Refs"
    {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if trimmed.chars().all(|c| c == '─' || c.is_whitespace()) {
        Style::default().fg(Color::DarkGray)
    } else if trimmed.starts_with('›') {
        Style::default()
            .fg(Color::Cyan)
            .bg(Color::Rgb(23, 35, 39))
            .add_modifier(Modifier::BOLD)
    } else if row.contains('◆') || row.contains("Blocked") || row.contains("blocked by") {
        Style::default()
            .fg(Color::Rgb(230, 170, 70))
            .add_modifier(Modifier::BOLD)
    } else if row.contains('✓') || row.contains("complete") {
        Style::default().fg(Color::Green)
    } else if row.contains('●') || row.contains("in progress") {
        Style::default().fg(Color::Cyan)
    } else if row.contains('−') || row.contains("canceled") {
        Style::default().fg(Color::DarkGray)
    } else if row.contains('◌') || row.contains("open") {
        Style::default().fg(Color::Gray)
    } else {
        Style::default().fg(Color::White)
    };
    Line::styled(row, style)
}

fn fit_width(text: String, width: usize) -> String {
    if display_width(&text) <= width {
        return text;
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    for ch in text.chars() {
        if display_width(&out) + 1 >= width {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

fn display_width(text: &str) -> usize {
    text.chars().count()
}
