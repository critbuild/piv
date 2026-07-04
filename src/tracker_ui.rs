use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use ratatui::{
    style::{Color, Modifier, Style},
    text::Line,
};

use crate::tracker::{Issue, IssueStatus, Prd, Project, TrackerSnapshot};

const DETAIL_HORIZONTAL_PADDING: usize = 2;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerViewState {
    pub selected: usize,
    pub detail_scroll: usize,
    expanded: BTreeSet<String>,
}

impl TrackerViewState {
    pub fn expand(&mut self, id: &str) {
        self.expanded.insert(id.to_string());
    }

    pub fn collapse(&mut self, id: &str) {
        self.expanded.remove(id);
    }

    pub fn toggle(&mut self, id: &str) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.to_string());
        }
    }

    pub fn is_expanded(&self, id: &str) -> bool {
        self.expanded.contains(id)
    }

    pub fn move_down(&mut self, snapshot: &TrackerSnapshot) {
        let before = self.selected;
        let len = visible_items(snapshot, self).len();
        if len > 0 {
            self.selected = (self.selected + 1).min(len - 1);
        }
        if self.selected != before {
            self.detail_scroll = 0;
        }
    }

    pub fn move_up(&mut self) {
        let before = self.selected;
        self.selected = self.selected.saturating_sub(1);
        if self.selected != before {
            self.detail_scroll = 0;
        }
    }

    pub fn scroll_detail_down(&mut self, amount: usize) {
        self.detail_scroll = self.detail_scroll.saturating_add(amount);
    }

    pub fn scroll_detail_up(&mut self, amount: usize) {
        self.detail_scroll = self.detail_scroll.saturating_sub(amount);
    }

    pub fn reset_detail_scroll(&mut self) {
        self.detail_scroll = 0;
    }

    pub fn clamp_detail_scroll(&mut self, max_scroll: usize) {
        self.detail_scroll = self.detail_scroll.min(max_scroll);
    }

    pub fn selected_item(&self, snapshot: &TrackerSnapshot) -> Option<TrackerItemRef> {
        visible_items(snapshot, self)
            .get(self.selected)
            .map(|row| row.item.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackerItemRef {
    Project {
        project_key: String,
    },
    Prd {
        project_key: String,
        prd_key: String,
    },
    Issue {
        project_key: String,
        prd_key: String,
        issue_key: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackerRow {
    pub id: String,
    pub item: TrackerItemRef,
    pub text: String,
    pub status: Option<String>,
    pub progress: Option<String>,
}

pub fn render_tracker_rows(
    snapshot: &TrackerSnapshot,
    state: &TrackerViewState,
    width: usize,
) -> Vec<String> {
    let mut rows = vec![fit_width("PRD Tracker".to_string(), width), String::new()];
    let items = visible_items(snapshot, state);
    if items.is_empty() {
        rows.push(
            "No tracker projects yet. Ask an AI agent to call project.create or prd.upsert_plan."
                .to_string(),
        );
        return rows;
    }
    rows.extend(items.into_iter().enumerate().map(|(index, row)| {
        let prefix = if index == state.selected {
            "› "
        } else {
            "  "
        };
        format_tracker_row(prefix, &row, width)
    }));
    rows
}

pub fn render_tracker_view(
    snapshot: &TrackerSnapshot,
    state: &TrackerViewState,
    width: usize,
) -> Vec<String> {
    render_tracker_view_with_height(snapshot, state, width, None)
}

pub fn render_tracker_viewport(
    snapshot: &TrackerSnapshot,
    state: &TrackerViewState,
    width: usize,
    height: usize,
) -> Vec<String> {
    let mut rows = render_tracker_view_with_height(snapshot, state, width, Some(height));
    rows.truncate(height);
    while rows.len() < height {
        rows.push(String::new());
    }
    rows
}

pub fn render_tracker_view_lines(
    snapshot: &TrackerSnapshot,
    state: &TrackerViewState,
    width: usize,
    height: usize,
) -> Vec<Line<'static>> {
    render_tracker_viewport(snapshot, state, width, height)
        .into_iter()
        .map(styled_tracker_line)
        .collect()
}

fn render_tracker_view_with_height(
    snapshot: &TrackerSnapshot,
    state: &TrackerViewState,
    width: usize,
    height: Option<usize>,
) -> Vec<String> {
    let detail_width = detail_text_width(width);
    let mut tree = render_tracker_rows(snapshot, state, width);
    if !tree.is_empty() {
        tree[0] = fit_width("Project / PRD / Issue tree".to_string(), width);
    }
    if tree.len() > 1 {
        tree[1] = "─".repeat(width);
    }

    let details = wrap_lines(render_tracker_detail(snapshot, state, detail_width), detail_width);
    match height {
        Some(height) => {
            if height == 0 {
                return Vec::new();
            }
            let menu_height = tree
                .len()
                .min(menu_height_limit(height));
            let mut rows = tree_menu_viewport(tree, state.selected + 2, menu_height);
            if rows.len() < height {
                rows.push("─".repeat(width));
            }
            let detail_height = height.saturating_sub(rows.len());
            let detail_rows = detail_viewport(details, state.detail_scroll, detail_width, detail_height);
            rows.extend(pad_detail_rows(detail_rows, DETAIL_HORIZONTAL_PADDING, width));
            rows.truncate(height);
            rows
        }
        None => {
            let mut rows = tree;
            rows.push("─".repeat(width));
            rows.extend(pad_detail_rows(details, DETAIL_HORIZONTAL_PADDING, width));
            rows
        }
    }
}

pub fn max_tracker_detail_scroll(
    snapshot: &TrackerSnapshot,
    state: &TrackerViewState,
    width: usize,
    height: usize,
) -> usize {
    let detail_width = detail_text_width(width);
    let tree_len = render_tracker_rows(snapshot, state, width).len();
    let menu_height = tree_len.min(menu_height_limit(height));
    let detail_height = height.saturating_sub(menu_height.saturating_add(1));
    let detail_len = wrap_lines(render_tracker_detail(snapshot, state, detail_width), detail_width).len();
    max_detail_scroll_for_len(detail_len, detail_height)
}

fn detail_text_width(width: usize) -> usize {
    width.saturating_sub(DETAIL_HORIZONTAL_PADDING * 2).max(1)
}

fn menu_height_limit(height: usize) -> usize {
    height.saturating_sub(3).max(1).min((height / 3).max(4))
}

pub fn visible_items(snapshot: &TrackerSnapshot, state: &TrackerViewState) -> Vec<TrackerRow> {
    let mut rows = Vec::new();
    for project_snapshot in &snapshot.projects {
        let project = &project_snapshot.project;
        let project_id = project_row_id(project);
        let expanded = state.is_expanded(&project_id);
        rows.push(TrackerRow {
            id: project_id.clone(),
            item: TrackerItemRef::Project {
                project_key: project.key.clone(),
            },
            text: format!("{} {}", if expanded { "▾" } else { "▸" }, project.name),
            status: None,
            progress: Some(format!("{} PRDs", project_snapshot.prds.len())),
        });
        if !expanded {
            continue;
        }
        for prd_snapshot in &project_snapshot.prds {
            let prd = &prd_snapshot.prd;
            let prd_id = prd_row_id(project, prd);
            let expanded = state.is_expanded(&prd_id);
            rows.push(TrackerRow {
                id: prd_id.clone(),
                item: TrackerItemRef::Prd {
                    project_key: project.key.clone(),
                    prd_key: prd.key.clone(),
                },
                text: format!("  {} {}", if expanded { "▾" } else { "▸" }, prd.title),
                status: Some(prd_status_label(&prd.status).to_string()),
                progress: (prd.issue_count > 0)
                    .then(|| format!("{}/{}", prd.completed_issue_count, prd.issue_count)),
            });
            if !expanded {
                continue;
            }
            for issue in &prd_snapshot.issues {
                rows.push(TrackerRow {
                    id: issue_row_id(project, prd, issue),
                    item: TrackerItemRef::Issue {
                        project_key: project.key.clone(),
                        prd_key: prd.key.clone(),
                        issue_key: issue.key.clone(),
                    },
                    text: issue_row_text(issue),
                    status: Some(issue_status_label(issue).to_string()),
                    progress: issue
                        .blocked
                        .then(|| format!("blocked by {}", issue.blocked_by_keys.join(","))),
                });
            }
        }
    }
    rows
}

pub fn project_row_id(project: &Project) -> String {
    format!("project:{}", project.key)
}

pub fn prd_row_id(project: &Project, prd: &Prd) -> String {
    format!("prd:{}/{}", project.key, prd.key)
}

pub fn issue_row_id(project: &Project, prd: &Prd, issue: &Issue) -> String {
    format!("issue:{}/{}/{}", project.key, prd.key, issue.key)
}

fn issue_row_text(issue: &Issue) -> String {
    format!("    {}. {}", issue.position, issue.title)
}

fn prd_status_label(status: &crate::tracker::PrdStatus) -> &'static str {
    match status {
        crate::tracker::PrdStatus::Draft => "◌ draft",
        crate::tracker::PrdStatus::InProgress => "● in progress",
        crate::tracker::PrdStatus::Complete => "✓ complete",
        crate::tracker::PrdStatus::Archived => "◇ archived",
    }
}

fn issue_glyph(issue: &Issue) -> &'static str {
    if issue.blocked {
        return "◆";
    }
    match issue.status {
        IssueStatus::Open => "◌",
        IssueStatus::InProgress => "●",
        IssueStatus::Complete => "✓",
        IssueStatus::Canceled => "−",
    }
}

fn issue_status_label(issue: &Issue) -> &'static str {
    if issue.blocked {
        return "◆ blocked";
    }
    match issue.status {
        IssueStatus::Open => "◌ open",
        IssueStatus::InProgress => "● in progress",
        IssueStatus::Complete => "✓ complete",
        IssueStatus::Canceled => "− canceled",
    }
}

fn render_tracker_detail(
    snapshot: &TrackerSnapshot,
    state: &TrackerViewState,
    width: usize,
) -> Vec<String> {
    let Some(item) = state.selected_item(snapshot) else {
        return vec!["Details".to_string()];
    };
    match item {
        TrackerItemRef::Project { project_key } => snapshot
            .projects
            .iter()
            .find(|project| project.project.key == project_key)
            .map(|project| {
                let mut lines = vec![
                    "Details".to_string(),
                    "─".repeat(width),
                    format!("Project: {}", project.project.name),
                    format!("key     {}", project.project.key),
                    format!("PRDs    {}", project.prds.len()),
                    String::new(),
                    "Roots".to_string(),
                ];
                if project.project.roots.is_empty() {
                    lines.push("<none>".to_string());
                } else {
                    lines.extend(project.project.roots.iter().cloned());
                }
                lines
            })
            .unwrap_or_else(|| vec!["Details".to_string(), "Project not found".to_string()]),
        TrackerItemRef::Prd {
            project_key,
            prd_key,
        } => snapshot
            .projects
            .iter()
            .find(|project| project.project.key == project_key)
            .and_then(|project| project.prds.iter().find(|prd| prd.prd.key == prd_key))
            .map(|prd_snapshot| {
                let prd = &prd_snapshot.prd;
                let mut lines = vec![
                    "Details".to_string(),
                    "─".repeat(width),
                    format!("PRD: {}", prd.title),
                    format!("status  {}", prd_status_label(&prd.status)),
                ];
                if let Some(source) = &prd.source_uri {
                    lines.push(format!("source  {source}"));
                } else {
                    lines.push("source  <none>".to_string());
                }
                if prd.issue_count > 0 {
                    lines.push(format!(
                        "progress {}/{}",
                        prd.completed_issue_count, prd.issue_count
                    ));
                }
                lines.push(String::new());
                lines.push("Body".to_string());
                match prd.body.as_deref().filter(|body| !body.trim().is_empty()) {
                    Some(body) => lines.extend(render_markdown_text(body)),
                    None => lines.push("<no PRD body stored>".to_string()),
                }
                lines.push(String::new());
                lines.push("Issues".to_string());
                if prd_snapshot.issues.is_empty() {
                    lines.push("<no linked issues>".to_string());
                } else {
                    for issue in &prd_snapshot.issues {
                        lines.push(format!(
                            "{}. {} {}",
                            issue.position,
                            issue_glyph(issue),
                            issue.title
                        ));
                        lines.push(format!("   status  {}", issue_status_label(issue)));
                        if issue.blocked {
                            lines.push(format!(
                                "   blocked by {}",
                                issue.blocked_by_keys.join(",")
                            ));
                        } else if !issue.blocked_by_keys.is_empty() {
                            lines.push(format!(
                                "   depends on {}",
                                issue.blocked_by_keys.join(",")
                            ));
                        }
                        if let Some(body) = issue.body.as_deref().filter(|body| !body.trim().is_empty()) {
                            if let Some(first_line) = body.lines().next() {
                                lines.push(format!("   notes   {first_line}"));
                            }
                        }
                    }
                }
                lines
            })
            .unwrap_or_else(|| vec!["Details".to_string(), "PRD not found".to_string()]),
        TrackerItemRef::Issue {
            project_key,
            issue_key,
            ..
        } => snapshot
            .projects
            .iter()
            .find(|project| project.project.key == project_key)
            .and_then(|project| {
                let linked_prds = project
                    .prds
                    .iter()
                    .filter(|prd| prd.issues.iter().any(|issue| issue.key == issue_key))
                    .collect::<Vec<_>>();
                let issue = linked_prds
                    .iter()
                    .flat_map(|prd| prd.issues.iter())
                    .find(|issue| issue.key == issue_key)?;
                let mut lines = vec![
                    "Details".to_string(),
                    "─".repeat(width),
                    format!("Issue: {}", issue.title),
                    format!("key     {}", issue.key),
                    format!("status  {}", issue_status_label(issue)),
                    format!("order   {}", issue.position),
                ];
                if issue.blocked {
                    lines.push(format!("blocked by {}", issue.blocked_by_keys.join(",")));
                } else if !issue.blocked_by_keys.is_empty() {
                    lines.push(format!("depends on {}", issue.blocked_by_keys.join(",")));
                }
                lines.push(String::new());
                lines.push("Body".to_string());
                match issue.body.as_deref().filter(|body| !body.trim().is_empty()) {
                    Some(body) => lines.extend(render_markdown_text(body)),
                    None => lines.push("<no issue body stored>".to_string()),
                }
                lines.push(String::new());
                lines.push("PRDs".to_string());
                lines.extend(linked_prds.iter().map(|prd| prd.prd.title.clone()));
                Some(lines)
            })
            .unwrap_or_else(|| vec!["Details".to_string(), "Issue not found".to_string()]),
    }
}

fn render_markdown_text(markdown: &str) -> Vec<String> {
    markdown.lines().map(render_markdown_line).collect()
}

fn render_markdown_line(line: &str) -> String {
    let trimmed = line.trim_start();
    let indent = &line[..line.len().saturating_sub(trimmed.len())];

    if let Some(rest) = trimmed.strip_prefix("- [ ] ") {
        return format!("{indent}☐ {rest}");
    }
    if let Some(rest) = trimmed
        .strip_prefix("- [x] ")
        .or_else(|| trimmed.strip_prefix("- [X] "))
    {
        return format!("{indent}☑ {rest}");
    }
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return format!("{indent}• {rest}");
        }
    }
    if let Some(rest) = trimmed.strip_prefix("> ") {
        return format!("{indent}┃ {rest}");
    }

    let heading_level = trimmed.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&heading_level)
        && trimmed.chars().nth(heading_level).is_some_and(char::is_whitespace)
    {
        let title = trimmed[heading_level..].trim();
        if heading_level == 1 {
            return format!("{indent}▌ {title}");
        }
        return format!("{indent}▸ {title}");
    }

    line.to_string()
}

fn styled_tracker_line(row: String) -> Line<'static> {
    let trimmed = row.trim_start();
    let style = if trimmed.starts_with('›') {
        Style::default()
            .fg(Color::Cyan)
            .bg(Color::Rgb(23, 35, 39))
            .add_modifier(Modifier::BOLD)
    } else if trimmed.starts_with("▌ ") {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else if trimmed.starts_with("▸ ") {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if trimmed.starts_with("┃ ") {
        Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC)
    } else if trimmed.starts_with("Project /")
        || trimmed.starts_with("Details")
        || trimmed.starts_with("PRD:")
        || trimmed.starts_with("Issue:")
        || trimmed.starts_with("Project:")
        || trimmed == "Body"
        || trimmed == "Issues"
        || trimmed == "Roots"
        || trimmed == "PRDs"
    {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if trimmed.chars().all(|c| c == '─' || c.is_whitespace()) {
        Style::default().fg(Color::DarkGray)
    } else if row.contains('◆') || row.contains("blocked by") {
        Style::default()
            .fg(Color::Rgb(230, 170, 70))
            .add_modifier(Modifier::BOLD)
    } else if row.contains('✓') || row.contains("complete") {
        Style::default().fg(Color::Green)
    } else if row.contains('●') || row.contains("in progress") {
        Style::default().fg(Color::Cyan)
    } else if row.contains('−') || row.contains("canceled") || row.contains('◇') {
        Style::default().fg(Color::DarkGray)
    } else if row.contains('◌') || row.contains("open") {
        Style::default().fg(Color::Gray)
    } else {
        Style::default().fg(Color::White)
    };
    Line::styled(row, style)
}

fn format_tracker_row(prefix: &str, row: &TrackerRow, width: usize) -> String {
    const STATUS_COLUMN_WIDTH: usize = 13;
    const PROGRESS_COLUMN_WIDTH: usize = 22;
    const RIGHT_GAP: usize = 2;
    const COLUMN_GAP: usize = 2;

    let left = format!("{prefix}{}", row.text);
    if row.status.is_none() && row.progress.is_none() {
        return fit_width(left, width);
    }

    let status = row.status.as_deref().unwrap_or("");
    let progress = row.progress.as_deref().unwrap_or("");
    let right_width = STATUS_COLUMN_WIDTH + COLUMN_GAP + PROGRESS_COLUMN_WIDTH;
    let right = format!(
        "{status:<STATUS_COLUMN_WIDTH$}{}{progress:<PROGRESS_COLUMN_WIDTH$}",
        " ".repeat(COLUMN_GAP)
    );

    if width <= right_width + RIGHT_GAP {
        return fit_width(format!("{left}  {right}"), width);
    }

    let left_room = width.saturating_sub(right_width + RIGHT_GAP);
    let left = fit_width(left, left_room);
    let left_width = display_width(&left);
    let gap = width.saturating_sub(left_width + right_width);
    format!("{left}{}{right}", " ".repeat(gap))
}

fn tree_menu_viewport(tree: Vec<String>, selected_row: usize, height: usize) -> Vec<String> {
    if height == 0 || tree.len() <= height {
        return tree.into_iter().take(height).collect();
    }

    let sticky_count = tree.len().min(2).min(height);
    let body_height = height.saturating_sub(sticky_count);
    if body_height == 0 {
        return tree.into_iter().take(height).collect();
    }
    let body_start = sticky_count;
    let body = &tree[body_start..];
    let selected = selected_row.saturating_sub(body_start).min(body.len().saturating_sub(1));
    let start = selected.saturating_sub(body_height / 2).min(body.len().saturating_sub(body_height));
    let end = (start + body_height).min(body.len());

    let mut rows = tree[..sticky_count].to_vec();
    rows.extend(body[start..end].iter().cloned());
    rows
}

fn pad_detail_rows(rows: Vec<String>, padding: usize, width: usize) -> Vec<String> {
    let prefix = " ".repeat(padding);
    let text_width = width.saturating_sub(padding);
    rows.into_iter()
        .map(|row| {
            let trimmed = row.trim();
            if !trimmed.is_empty() && trimmed.chars().all(|ch| ch == '─') {
                "─".repeat(width)
            } else {
                fit_width(format!("{prefix}{row}"), text_width)
            }
        })
        .collect()
}

fn detail_viewport(
    details: Vec<String>,
    scroll: usize,
    width: usize,
    height: usize,
) -> Vec<String> {
    if height == 0 || details.len() <= height {
        return details;
    }

    let sticky_count = details.len().min(2).min(height.saturating_sub(1));
    let footer_count = 1;
    let body_height = height.saturating_sub(sticky_count + footer_count).max(1);
    let body = &details[sticky_count..];
    let max_scroll = body.len().saturating_sub(body_height);
    let scroll = scroll.min(max_scroll);
    let end = (scroll + body_height).min(body.len());

    let mut rows = details[..sticky_count].to_vec();
    rows.extend(body[scroll..end].iter().cloned());
    rows.push(fit_width(
        format!(
            "Details {}-{}/{}  PgUp/PgDn scroll",
            scroll + 1,
            end,
            body.len()
        ),
        width,
    ));
    rows
}

fn max_detail_scroll_for_len(detail_len: usize, height: usize) -> usize {
    if height == 0 || detail_len <= height {
        return 0;
    }
    let sticky_count = detail_len.min(2).min(height.saturating_sub(1));
    let footer_count = 1;
    let body_height = height.saturating_sub(sticky_count + footer_count).max(1);
    detail_len.saturating_sub(sticky_count).saturating_sub(body_height)
}

fn wrap_lines(lines: Vec<String>, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new(); lines.len()];
    }
    lines
        .into_iter()
        .flat_map(|line| wrap_line(&line, width))
        .collect()
}

fn wrap_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    if display_width(line) <= width {
        return vec![line.to_string()];
    }

    let indent = line
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect::<String>();
    let continuation_indent = if indent.is_empty() {
        String::new()
    } else {
        indent.clone()
    };
    let mut rows = Vec::new();
    let mut current = String::new();

    for word in line.split_whitespace() {
        let separator = if current.trim().is_empty() { "" } else { " " };
        if display_width(&current) + separator.len() + display_width(word) <= width {
            current.push_str(separator);
            current.push_str(word);
        } else {
            if !current.is_empty() {
                rows.push(current);
            }
            current = continuation_indent.clone();
            if display_width(&current) + display_width(word) <= width {
                current.push_str(word);
            } else {
                let available = width.saturating_sub(display_width(&current)).max(1);
                let mut rest = word;
                while display_width(rest) > available {
                    let chunk = rest.chars().take(available).collect::<String>();
                    rows.push(format!("{current}{chunk}"));
                    rest = &rest[chunk.len()..];
                }
                current = format!("{continuation_indent}{rest}");
            }
        }
    }
    if !current.is_empty() {
        rows.push(current);
    }
    rows
}

fn display_width(text: &str) -> usize {
    text.chars().count()
}

fn fit_width(mut text: String, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if text.chars().count() <= width {
        return text;
    }
    text = text.chars().take(width.saturating_sub(1)).collect();
    text.push('…');
    text
}
