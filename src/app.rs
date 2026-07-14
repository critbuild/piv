use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError},
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Result, bail};
use chrono::{DateTime, Local};
use crossterm::event::{
    self, Event as TermEvent, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{
    code_pane::{
        CodePaneOverlays, RemoteHighlightOverlay, SearchOverlay, code_prefix_width, prepare_rows,
        render_code_pane,
    },
    control::{ControlCommand, ControlServer},
    diff::{DiffEngine, LineKind},
    file_intake::{FileIntake, row_index_for_new_line},
    highlight::Highlighter,
    issue_cockpit::{
        CockpitScopeMode, IssueCockpitState, build_issue_cockpit_view, match_project_for_root,
        render_issue_cockpit_lines, tracker_status_fragment,
    },
    model::{Selection, Tab, TabHit, TabManager, TextPoint},
    search::{Match as SearchMatch, SearchQuery},
    tracker::TrackerStore,
    tracker_ui::{
        TrackerItemRef, TrackerViewState, max_tracker_detail_scroll, max_tracker_menu_scroll,
        render_tracker_detail_lines, render_tracker_menu_lines, tracker_menu_item_at_row,
        visible_items,
    },
    watcher::{FileWatcher, WatchEvent},
};

use arboard::Clipboard;

const MAX_TABS: usize = 10;
const BATCH_WINDOW: Duration = Duration::from_millis(120);
const MOUSE_SCROLL_LINES: usize = 5;
const TRACKER_MIN_PANE_WIDTH: u16 = 24;
const TRACKER_WIDE_MIN_WIDTH: u16 = TRACKER_MIN_PANE_WIDTH * 2 + 1;
const FALLBACK_SCAN_INTERVAL: Duration = Duration::from_millis(750);
const FALLBACK_SCAN_IDLE_DELAY: Duration = Duration::from_millis(1200);
const GIT_REF_REFRESH_INTERVAL: Duration = Duration::from_millis(750);
const TRACKER_REFRESH_INTERVAL: Duration = Duration::from_millis(750);
const HIGHLIGHT_FADE_DURATION: Duration = Duration::from_secs(10);
const AUTO_FOCUS_TOP_PADDING: usize = 2;
const ASSUMED_EDITOR_BG: (u8, u8, u8) = (0x17, 0x23, 0x27);
const AI_HIGHLIGHT_BG: (u8, u8, u8) = (78, 72, 110);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffBase {
    Head,
    OriginMain,
}

impl DiffBase {
    fn label(self) -> &'static str {
        match self {
            Self::Head => "HEAD",
            Self::OriginMain => "origin/main",
        }
    }

    fn toggle(self) -> Self {
        match self {
            Self::Head => Self::OriginMain,
            Self::OriginMain => Self::Head,
        }
    }
}

pub struct App {
    root: PathBuf,
    rx: Receiver<WatchEvent>,
    command_rx: Receiver<ControlCommand>,
    _watcher: FileWatcher,
    _control_server: ControlServer,
    tabs: TabManager,
    intake: FileIntake,
    highlighter: Highlighter,
    clipboard: Option<Clipboard>,
    clipboard_process: Option<Child>,
    remote_highlight: Option<(PathBuf, usize, usize, Instant)>,
    tab_area: Rect,
    code_area: Rect,
    mouse_selecting: bool,
    last_input_at: Instant,
    last_change: Option<SystemTime>,
    copy_notice_until: Option<Instant>,
    diff_base: DiffBase,
    last_git_ref_probe: Instant,
    last_seen_diff_base_rev: Option<String>,
    search_input: Option<String>,
    last_search: Option<CommittedSearch>,
    mode: InteractionMode,
    tracker: Option<TrackerStore>,
    tracker_snapshot: Option<crate::tracker::TrackerSnapshot>,
    last_tracker_refresh: Instant,
    tracker_view: TrackerViewState,
    tracker_scope_mode: CockpitScopeMode,
    tracker_notice: Option<String>,
    tracker_area: Rect,
    tracker_menu_area: Rect,
    tracker_divider_area: Rect,
    tracker_detail_area: Rect,
    tracker_menu_width: Option<u16>,
    tracker_divider_dragging: bool,
    issue_cockpit: IssueCockpitState,
    issue_cockpit_area: Rect,
}

struct CommittedSearch {
    path: PathBuf,
    matches: Vec<SearchMatch>,
    current: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum InteractionMode {
    Code,
    Command(String),
    Tracker,
}

impl App {
    pub fn new(root: PathBuf) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let watcher = FileWatcher::start(root.clone(), tx)?;
        let (command_tx, command_rx) = mpsc::channel();
        let control_server = ControlServer::start(&root, command_tx)?;
        let mut app = Self {
            root: root.clone(),
            rx,
            command_rx,
            _watcher: watcher,
            _control_server: control_server,
            tabs: TabManager::new(MAX_TABS),
            intake: FileIntake::new(root.clone()),
            highlighter: Highlighter::new()?,
            clipboard: Clipboard::new().ok(),
            clipboard_process: None,
            remote_highlight: None,
            tab_area: Rect::default(),
            code_area: Rect::default(),
            mouse_selecting: false,
            last_input_at: Instant::now(),
            last_change: None,
            copy_notice_until: None,
            diff_base: DiffBase::Head,
            last_git_ref_probe: Instant::now(),
            last_seen_diff_base_rev: None,
            search_input: None,
            last_search: None,
            mode: InteractionMode::Code,
            tracker: None,
            tracker_snapshot: None,
            last_tracker_refresh: Instant::now() - TRACKER_REFRESH_INTERVAL,
            tracker_view: TrackerViewState::default(),
            tracker_scope_mode: CockpitScopeMode::Auto,
            tracker_notice: None,
            tracker_area: Rect::default(),
            tracker_menu_area: Rect::default(),
            tracker_divider_area: Rect::default(),
            tracker_detail_area: Rect::default(),
            tracker_menu_width: None,
            tracker_divider_dragging: false,
            issue_cockpit: IssueCockpitState::default(),
            issue_cockpit_area: Rect::default(),
        };
        app.last_seen_diff_base_rev = app.current_diff_base_rev();
        app.seed_seen_mtimes()?;
        app.open_initial_file()?;
        Ok(app)
    }

    fn open_initial_file(&mut self) -> Result<()> {
        if let Some((path, at)) = self.intake.newest_allowed_file() {
            self.load_change(path, at)?;
        }
        Ok(())
    }

    fn seed_seen_mtimes(&mut self) -> Result<()> {
        self.intake.seed_seen_mtimes()
    }

    pub fn run(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ) -> Result<()> {
        let mut should_draw = true;
        loop {
            if self.refresh_tracker_context_if_due() {
                should_draw = true;
            }
            if should_draw {
                terminal.draw(|f| self.render(f))?;
                should_draw = false;
            }

            if self.drain_file_changes()? {
                should_draw = true;
            }
            if self.scan_for_missed_changes()? {
                should_draw = true;
            }
            if self.scan_for_git_ref_changes() {
                should_draw = true;
            }
            if self.drain_control_commands()? {
                should_draw = true;
            }

            let timeout = if self.remote_highlight.is_some() {
                Duration::from_millis(33)
            } else {
                Duration::from_millis(250)
            };
            if event::poll(timeout)? {
                should_draw = true;
                self.last_input_at = Instant::now();
                match event::read()? {
                    TermEvent::Key(key) if self.handle_key(key) => break,
                    TermEvent::Mouse(mouse) => self.handle_mouse(mouse),
                    TermEvent::Resize(_, _) => {}
                    _ => {}
                }
            } else if self.remote_highlight.is_some() {
                should_draw = true;
            }
        }
        Ok(())
    }

    fn drain_control_commands(&mut self) -> Result<bool> {
        let mut changed = false;
        loop {
            match self.command_rx.try_recv() {
                Ok(command) => {
                    self.handle_control_command(command)?;
                    changed = true;
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        Ok(changed)
    }

    fn handle_control_command(&mut self, command: ControlCommand) -> Result<()> {
        match command {
            ControlCommand::Open { path, line } => self.open_path(path, line)?,
            ControlCommand::Highlight {
                path,
                start_line,
                end_line,
            } => self.highlight_path(path, start_line, end_line)?,
            ControlCommand::Line(line) => self.focus_current_line(line),
            ControlCommand::TabNext => {
                self.tabs.next();
                self.invalidate_search();
            }
            ControlCommand::TabPrev => {
                self.tabs.prev();
                self.invalidate_search();
            }
            ControlCommand::Recenter => {
                if let Some(tab) = self.tabs.current_mut() {
                    tab.auto_center = true;
                }
            }
        }
        Ok(())
    }

    fn drain_file_changes(&mut self) -> Result<bool> {
        let first = match self.rx.try_recv() {
            Ok(c) => c,
            Err(_) => return Ok(false),
        };
        let mut changed: HashMap<PathBuf, SystemTime> = HashMap::new();
        let mut removed: Vec<(PathBuf, SystemTime)> = Vec::new();
        match first {
            WatchEvent::Changed { path, at } => {
                changed.insert(path, at);
            }
            WatchEvent::Removed { path, at } => {
                removed.push((path, at));
            }
        }
        let deadline = Instant::now() + BATCH_WINDOW;
        while Instant::now() < deadline {
            match self.rx.recv_timeout(Duration::from_millis(20)) {
                Ok(WatchEvent::Changed { path, at }) => {
                    changed.insert(path, at);
                }
                Ok(WatchEvent::Removed { path, at }) => {
                    removed.push((path, at));
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
            }
        }
        for (path, at) in removed {
            self.remove_file(&path, at);
        }
        for (path, at) in changed {
            self.load_change(path, at)?;
        }
        Ok(true)
    }

    fn remove_file(&mut self, path: &Path, at: SystemTime) {
        self.invalidate_search();
        self.intake.remove(path);
        self.tabs.remove(path);
        self.last_change = Some(at);
    }

    fn open_path(&mut self, path: PathBuf, line: Option<usize>) -> Result<()> {
        self.invalidate_search();
        let path = self
            .intake
            .resolve_existing_path(path, "open target does not exist")?;
        let reference_content = self.reference_content(&path);
        let at = SystemTime::now();
        let tab =
            self.intake
                .load_remote_open(path, line, at, &self.highlighter, reference_content)?;
        self.last_change = Some(at);
        self.tabs.add_or_bring_to_front(tab);
        Ok(())
    }

    fn highlight_path(&mut self, path: PathBuf, start_line: usize, end_line: usize) -> Result<()> {
        let path = self
            .intake
            .resolve_existing_path(path, "highlight target does not exist")?;
        let (start_line, end_line) = if start_line <= end_line {
            (start_line, end_line)
        } else {
            (end_line, start_line)
        };
        self.remote_highlight = Some((
            path.clone(),
            start_line.saturating_sub(1),
            end_line.saturating_sub(1),
            Instant::now(),
        ));
        self.open_path(path, Some(start_line))
    }

    fn focus_current_line(&mut self, line: usize) {
        if let Some(tab) = self.tabs.current_mut() {
            tab.focus_line = row_index_for_new_line(&tab.diff, line.saturating_sub(1));
            tab.auto_center = true;
            tab.selection = None;
        }
    }

    fn scan_for_missed_changes(&mut self) -> Result<bool> {
        let missed = self.intake.scan_for_missed_changes(
            self.last_input_at.elapsed(),
            FALLBACK_SCAN_INTERVAL,
            FALLBACK_SCAN_IDLE_DELAY,
        )?;
        let had_changes = !missed.is_empty();
        for path in missed.removed {
            self.remove_file(&path, SystemTime::now());
        }
        for (path, at) in missed.changed {
            self.load_change(path, at)?;
        }
        Ok(had_changes)
    }

    fn load_change(&mut self, path: PathBuf, at: SystemTime) -> Result<()> {
        self.invalidate_search();
        let reference_content = self.reference_content(&path);
        let tab = self
            .intake
            .load_changed(path, at, &self.highlighter, reference_content)?;
        self.last_change = Some(at);
        self.tabs.add_or_bring_to_front(tab);
        Ok(())
    }

    fn reference_content(&self, path: &Path) -> Option<String> {
        match self.diff_base {
            DiffBase::Head => self.git_ref_content("HEAD", path),
            DiffBase::OriginMain => {
                if self.git_ref_exists("origin/main") {
                    Some(
                        self.git_ref_content("origin/main", path)
                            .unwrap_or_default(),
                    )
                } else {
                    self.git_ref_content("HEAD", path)
                }
            }
        }
    }

    fn git_ref_exists(&self, git_ref: &str) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["rev-parse", "--verify", "--quiet", git_ref])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn git_ref_content(&self, git_ref: &str, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["show", &format!("{}:{}", git_ref, rel.display())])
            .output()
            .ok()?;
        if output.status.success() {
            return Some(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        None
    }

    fn current_diff_base_rev(&self) -> Option<String> {
        let git_ref = match self.diff_base {
            DiffBase::Head => Some("HEAD"),
            DiffBase::OriginMain => {
                if self.git_ref_exists("origin/main") {
                    Some("origin/main")
                } else {
                    Some("HEAD")
                }
            }
        }?;
        self.git_ref_oid(git_ref)
    }

    fn git_ref_oid(&self, git_ref: &str) -> Option<String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["rev-parse", "--verify", "--quiet", git_ref])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let oid = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!oid.is_empty()).then_some(oid)
    }

    fn scan_for_git_ref_changes(&mut self) -> bool {
        if self.last_git_ref_probe.elapsed() < GIT_REF_REFRESH_INTERVAL {
            return false;
        }
        self.last_git_ref_probe = Instant::now();
        let current = self.current_diff_base_rev();
        if current == self.last_seen_diff_base_rev {
            return false;
        }
        self.last_seen_diff_base_rev = current;
        self.refresh_open_tab_diffs();
        true
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if matches!(
            (key.code, key.modifiers),
            (KeyCode::Char('c'), KeyModifiers::CONTROL)
        ) {
            return true;
        }
        match &self.mode {
            InteractionMode::Tracker => {
                self.handle_tracker_key(key);
                return false;
            }
            InteractionMode::Command(_) => {
                self.handle_command_input(key);
                return false;
            }
            InteractionMode::Code => {}
        }
        if self.search_input.is_some() {
            // While the `/` prompt is open, keys feed the search buffer.
            // Ctrl-C / q still quit so the user is never trapped.
            if matches!(
                (key.code, key.modifiers),
                (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Char('q'), _)
            ) {
                return true;
            }
            self.handle_search_input(key);
            return false;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => return true,
            (KeyCode::Char('i'), _) => self.toggle_issue_cockpit(),
            (KeyCode::Char('a'), _) if self.issue_cockpit.open => self.toggle_issue_cockpit_scope(),
            (KeyCode::Char('r'), _) if self.issue_cockpit.open => {
                self.select_next_issue_cockpit_ref()
            }
            (KeyCode::Char('R'), KeyModifiers::SHIFT) if self.issue_cockpit.open => {
                self.select_prev_issue_cockpit_ref()
            }
            (KeyCode::Enter, _) if self.issue_cockpit.open => {
                self.open_selected_issue_cockpit_ref()
            }
            (KeyCode::Char(':'), _) | (KeyCode::Char(';'), KeyModifiers::SHIFT) => {
                self.begin_command()
            }
            (KeyCode::Char('/'), _) => self.begin_search(),
            (KeyCode::Char('n'), _) => self.cycle_search(true),
            (KeyCode::Char('N'), KeyModifiers::SHIFT) => self.cycle_search(false),
            (KeyCode::Tab, _) => {
                self.tabs.next();
                self.invalidate_search();
            }
            (KeyCode::BackTab, _) => {
                self.tabs.prev();
                self.invalidate_search();
            }
            (KeyCode::Up, _) => self.scroll_up(1),
            (KeyCode::Down, _) => self.scroll_down(1),
            (KeyCode::PageUp, _) => self.scroll_up(20),
            (KeyCode::PageDown, _) => self.scroll_down(20),
            (KeyCode::Home, _) => self.set_scroll(0),
            (KeyCode::End, _) => {
                if let Some(t) = self.tabs.current() {
                    self.set_scroll(t.diff.len().saturating_sub(1));
                }
            }
            (KeyCode::Char('['), _) => self.jump_to_diff(false),
            (KeyCode::Char(']'), _) => self.jump_to_diff(true),
            (KeyCode::Char('\\'), _) => self.toggle_diff_base(),
            (KeyCode::Char('c'), _) => {
                if let Some(t) = self.tabs.current_mut() {
                    t.auto_center = true;
                }
            }
            _ => {}
        }
        false
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
            self.tracker_divider_dragging = false;
        }
        if self.in_tracker_mode() {
            self.handle_tracker_mouse(mouse);
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_up(MOUSE_SCROLL_LINES),
            MouseEventKind::ScrollDown => self.scroll_down(MOUSE_SCROLL_LINES),
            MouseEventKind::Down(MouseButton::Left) => {
                if rect_contains(self.tab_area, mouse.column, mouse.row) {
                    let before = self.tabs.active;
                    let before_len = self.tabs.len();
                    match self
                        .tabs
                        .tab_hit_at_column(mouse.column.saturating_sub(self.tab_area.x))
                    {
                        Some(TabHit::Select(index)) => self.tabs.select(index),
                        Some(TabHit::Close(index)) => self.tabs.remove_at(index),
                        None => {}
                    }
                    if self.tabs.active != before || self.tabs.len() != before_len {
                        self.invalidate_search();
                    }
                    self.mouse_selecting = false;
                } else if let Some(point) = self.mouse_point_to_text_point(mouse.column, mouse.row)
                {
                    if let Some(tab) = self.tabs.current_mut() {
                        tab.selection = Some(Selection {
                            anchor: point,
                            focus: point,
                        });
                        tab.auto_center = false;
                    }
                    self.mouse_selecting = true;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.mouse_selecting {
                    if let Some(point) = self.mouse_point_to_text_point(mouse.column, mouse.row) {
                        if let Some(tab) = self.tabs.current_mut() {
                            if let Some(selection) = &mut tab.selection {
                                selection.focus = point;
                            }
                        }
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.mouse_selecting = false;
                self.copy_selection_to_clipboard();
            }
            _ => {}
        }
    }

    fn mouse_point_to_text_point(&self, column: u16, row: u16) -> Option<TextPoint> {
        let inner_x = self.code_area.x;
        let inner_y = self.code_area.y;
        let inner_width = self.code_area.width;
        let inner_height = self.code_area.height;
        if column < inner_x
            || row < inner_y
            || column >= inner_x.saturating_add(inner_width)
            || row >= inner_y.saturating_add(inner_height)
        {
            return None;
        }
        let line = self.tabs.current()?.scroll + usize::from(row.saturating_sub(inner_y));
        let text = self.tabs.current()?.diff.get(line)?.text.as_str();
        let visible_column = usize::from(column.saturating_sub(inner_x));
        let text_column = visible_column.saturating_sub(code_prefix_width());
        Some(TextPoint {
            line,
            column: text_column.min(text.chars().count()),
        })
    }

    fn viewport_height(&self) -> usize {
        self.code_area.height as usize
    }
    fn set_scroll(&mut self, n: usize) {
        let h = self.viewport_height();
        if let Some(t) = self.tabs.current_mut() {
            t.scroll = n;
            t.auto_center = false;
            clamp_tab_scroll(t, h);
        }
    }
    fn scroll_up(&mut self, n: usize) {
        let h = self.viewport_height();
        if let Some(t) = self.tabs.current_mut() {
            t.scroll = t.scroll.saturating_sub(n);
            t.auto_center = false;
            clamp_tab_scroll(t, h);
        }
    }
    fn scroll_down(&mut self, n: usize) {
        let h = self.viewport_height();
        if let Some(t) = self.tabs.current_mut() {
            t.scroll = (t.scroll + n).min(t.diff.len().saturating_sub(1));
            t.auto_center = false;
            clamp_tab_scroll(t, h);
        }
    }

    fn jump_to_diff(&mut self, forward: bool) {
        let height = self.code_area.height.saturating_sub(2) as usize;
        let target = self.tabs.current().and_then(|tab| {
            let current = visible_diff_center(&tab.diff, tab.scroll, height).or(tab.center_diff);
            next_diff_center(&tab.diff, current, forward)
        });
        if let Some(target) = target {
            if let Some(tab) = self.tabs.current_mut() {
                tab.center_diff = Some(target);
                tab.focus_line = Some(target);
                tab.auto_center = true;
                tab.selection = None;
            }
        }
    }

    fn render(&mut self, f: &mut Frame) {
        if self
            .remote_highlight
            .as_ref()
            .is_some_and(|(_, _, _, at)| at.elapsed() >= HIGHLIGHT_FADE_DURATION)
        {
            self.remote_highlight = None;
        }
        if self.in_tracker_mode() {
            self.render_tracker(f, f.area());
            return;
        }
        let prompting =
            self.search_input.is_some() || matches!(self.mode, InteractionMode::Command(_));
        let cockpit_height = self
            .issue_cockpit
            .open
            .then(|| issue_cockpit_height(f.area().height, prompting));
        let mut constraints = vec![Constraint::Length(1), Constraint::Min(1)];
        if let Some(height) = cockpit_height {
            constraints.push(Constraint::Length(height));
        }
        if prompting {
            constraints.push(Constraint::Length(1));
        }
        constraints.push(Constraint::Length(1));
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(f.area());
        self.tab_area = chunks[0];
        self.code_area = chunks[1];
        self.render_tabs(f, chunks[0]);
        self.render_code(f, chunks[1]);
        let mut next_idx = 2;
        if let Some(height) = cockpit_height {
            self.issue_cockpit_area = chunks[next_idx];
            self.render_issue_cockpit(f, chunks[next_idx], usize::from(height));
            next_idx += 1;
        } else {
            self.issue_cockpit_area = Rect::default();
        }
        if self.search_input.is_some() {
            self.render_search_prompt(f, chunks[next_idx]);
            next_idx += 1;
        } else if matches!(self.mode, InteractionMode::Command(_)) {
            self.render_command_prompt(f, chunks[next_idx]);
            next_idx += 1;
        }
        self.render_status(f, chunks[next_idx]);
    }

    fn render_tabs(&self, f: &mut Frame, area: Rect) {
        let divider_style = Style::default().fg(Color::DarkGray);
        let mut spans = vec![Span::raw(" ")];
        spans.extend(self.tabs.tabs.iter().enumerate().flat_map(|(i, t)| {
            let name = t
                .path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("?")
                .to_string();
            let active = i == self.tabs.active;
            let accent_style = if active {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                divider_style
            };
            let label_style = if active {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            let close_style = if active {
                Style::default()
                    .fg(Color::Rgb(220, 80, 80))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Rgb(180, 40, 40))
            };
            let mut parts = Vec::new();
            if i > 0 {
                parts.push(Span::styled(" │ ", divider_style));
            }
            parts.push(Span::styled(if active { "▌" } else { " " }, accent_style));
            parts.push(Span::styled(name, label_style));
            parts.push(Span::raw(" "));
            parts.push(Span::styled("×", close_style));
            parts
        }));
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_code(&mut self, f: &mut Frame, area: Rect) {
        let height = area.height as usize;
        let Some(tab) = self.tabs.current_mut() else {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "No source files found yet. Waiting for changes...",
                    Style::default().fg(Color::DarkGray),
                )),
                area,
            );
            return;
        };
        center_tab(tab, height);
        if let Some(center) = visible_diff_center(&tab.diff, tab.scroll, height) {
            tab.center_diff = Some(center);
        }

        let remote_highlight =
            self.remote_highlight
                .as_ref()
                .and_then(|(path, start_line, end_line, at)| {
                    if path == &tab.path {
                        highlight_line_style(*at).map(|style| RemoteHighlightOverlay {
                            start_line: *start_line,
                            end_line: *end_line,
                            style,
                        })
                    } else {
                        None
                    }
                });
        let search = self
            .last_search
            .as_ref()
            .filter(|search| search.path == tab.path)
            .map(|search| SearchOverlay {
                matches: search.matches.as_slice(),
                current: search.current,
            });
        let selection = tab.selection;
        let lines = render_code_pane(
            tab,
            height,
            CodePaneOverlays {
                selection,
                remote_highlight,
                search,
            },
        );
        f.render_widget(Paragraph::new(lines), area);
    }

    fn render_search_prompt(&self, f: &mut Frame, area: Rect) {
        let Some(buffer) = &self.search_input else {
            return;
        };
        let query = SearchQuery::new(buffer);
        let count = if query.is_empty() {
            None
        } else {
            Some(self.current_search_matches(&query).len())
        };
        let label = match count {
            Some(n) => format!("/{buffer} [{n}]"),
            None => format!("/{buffer}"),
        };
        f.render_widget(
            Paragraph::new(Span::styled(label, Style::default().fg(Color::Yellow))),
            area,
        );
    }

    fn render_command_prompt(&self, f: &mut Frame, area: Rect) {
        let InteractionMode::Command(buffer) = &self.mode else {
            return;
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                format!(":{buffer}"),
                Style::default().fg(Color::Yellow),
            )),
            area,
        );
    }

    fn render_issue_cockpit(&mut self, f: &mut Frame, area: Rect, height: usize) {
        let rows = match self.tracker_snapshot.as_ref() {
            Some(snapshot) => {
                let ref_count = self.current_issue_cockpit_ref_count();
                self.issue_cockpit.clamp_selected_ref(ref_count);
                render_issue_cockpit_lines(
                    snapshot,
                    &self.root,
                    &self.issue_cockpit,
                    area.width as usize,
                    height,
                )
            }
            None => vec![
                Line::raw("Issue Cockpit"),
                Line::raw("─".repeat(area.width as usize)),
                Line::raw(
                    self.tracker_notice
                        .clone()
                        .unwrap_or_else(|| "Tracker database unavailable.".to_string()),
                ),
            ],
        };
        f.render_widget(Paragraph::new(rows), area);
    }

    fn render_tracker(&mut self, f: &mut Frame, area: Rect) {
        self.set_tracker_layout(area);
        let snapshot = self
            .tracker
            .as_ref()
            .and_then(|tracker| tracker.snapshot().ok());

        let menu_rows = match snapshot {
            Some(raw_snapshot) => {
                let header = self.tracker_scope_header(&raw_snapshot);
                let snapshot = self.scope_tracker_snapshot(raw_snapshot);
                self.clamp_tracker_offsets(&snapshot);
                let mut rows = render_tracker_menu_lines(
                    &snapshot,
                    &self.tracker_view,
                    self.tracker_menu_area.width as usize,
                    self.tracker_menu_area.height as usize,
                );
                if let Some(first) = rows.first_mut() {
                    *first = Line::styled(
                        header,
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    );
                }
                if self.tracker_detail_area.width > 0 {
                    let detail_rows = render_tracker_detail_lines(
                        &snapshot,
                        &self.tracker_view,
                        self.tracker_detail_area.width as usize,
                        self.tracker_detail_area.height as usize,
                    );
                    f.render_widget(Paragraph::new(detail_rows), self.tracker_detail_area);
                }
                rows
            }
            None => vec![
                Line::raw("PRD Tracker                                      :prd"),
                Line::raw(""),
                Line::raw(
                    self.tracker_notice
                        .clone()
                        .unwrap_or_else(|| "Tracker database unavailable.".to_string()),
                ),
            ],
        };
        f.render_widget(Paragraph::new(menu_rows), self.tracker_menu_area);
        if self.tracker_divider_area.width > 0 {
            let divider = (0..self.tracker_divider_area.height)
                .map(|_| Line::styled("│", Style::default().fg(Color::DarkGray)))
                .collect::<Vec<_>>();
            f.render_widget(Paragraph::new(divider), self.tracker_divider_area);
        }
    }

    fn render_status(&self, f: &mut Frame, area: Rect) {
        let copied = self
            .copy_notice_until
            .is_some_and(|until| until > Instant::now());
        let tracker_suffix = self.tracker_status_suffix();
        let text = if let Some(tab) = self.tabs.current() {
            let rel = tab
                .path
                .strip_prefix(&self.root)
                .unwrap_or(&tab.path)
                .display();
            let changes = tab
                .diff
                .iter()
                .filter(|l| l.kind != LineKind::Unchanged)
                .count();
            let ts: DateTime<Local> = tab.last_edit.into();
            format!(
                "{} | diff {} | {} lines | {} changes | tab {}/{} | last edit {} | {}{}{}{}",
                rel,
                self.diff_base.label(),
                tab.content.lines().count(),
                changes,
                self.tabs.active + 1,
                self.tabs.len(),
                ts.format("%H:%M:%S"),
                if self.last_change.is_some() {
                    "idle"
                } else {
                    "waiting"
                },
                if copied { " | copied" } else { "" },
                match self.remote_highlight.as_ref() {
                    Some((path, start_line, end_line, at))
                        if path == &tab.path && at.elapsed() < HIGHLIGHT_FADE_DURATION =>
                        if start_line == end_line {
                            format!(" | hl {}", start_line + 1)
                        } else {
                            format!(" | hl {}-{}", start_line + 1, end_line + 1)
                        },
                    _ => String::new(),
                },
                tracker_suffix
            )
        } else {
            format!(
                "watching {} | diff {} | idle{}{}",
                self.root.display(),
                self.diff_base.label(),
                if copied { " | copied" } else { "" },
                tracker_suffix
            )
        };
        f.render_widget(Paragraph::new(text), area);
    }
}

impl App {
    fn toggle_diff_base(&mut self) {
        self.diff_base = self.diff_base.toggle();
        self.last_seen_diff_base_rev = self.current_diff_base_rev();
        self.refresh_open_tab_diffs();
    }

    fn refresh_open_tab_diffs(&mut self) {
        self.invalidate_search();
        let updates = self
            .tabs
            .tabs
            .iter()
            .map(|tab| {
                let diff = match self.reference_content(&tab.path) {
                    Some(old) => DiffEngine::diff(&old, &tab.content),
                    None => {
                        let old = self.intake.snapshot(&tab.path).unwrap_or("");
                        DiffEngine::diff(old, &tab.content)
                    }
                };
                let first_change = diff.iter().position(|l| l.kind != LineKind::Unchanged);
                let prepared_rows = prepare_rows(&diff, &tab.highlighted_lines);
                (diff, prepared_rows, first_change)
            })
            .collect::<Vec<_>>();

        for (tab, (diff, prepared_rows, first_change)) in self.tabs.tabs.iter_mut().zip(updates) {
            tab.prepared_rows = prepared_rows;
            tab.diff = diff;
            tab.first_change = first_change;
            tab.focus_line = first_change;
            tab.center_diff = None;
            tab.scroll = 0;
            tab.auto_center = true;
            tab.selection = None;
            tab.viewport_cache = None;
        }
    }
}

fn center_tab(tab: &mut Tab, height: usize) {
    if tab.auto_center {
        if let Some(line) = tab.focus_line.or(tab.first_change) {
            tab.scroll = line.saturating_sub(AUTO_FOCUS_TOP_PADDING.min(height.saturating_sub(1)));
        }
    }
    // Clamp last so the viewport stays aligned with EOF: when the focus line or
    // a manual scroll would land past the last full page, pin the scroll so the
    // final page fills the viewport instead of leaving blank filler underneath.
    clamp_tab_scroll(tab, height);
}

fn clamp_tab_scroll(tab: &mut Tab, height: usize) {
    let max_scroll = tab.diff.len().saturating_sub(height);
    tab.scroll = tab.scroll.min(max_scroll);
}

fn diff_hunks(diff: &[crate::diff::DiffLine]) -> Vec<(usize, usize)> {
    let mut hunks = Vec::new();
    let mut start = None;
    for (idx, line) in diff.iter().enumerate() {
        if line.kind != LineKind::Unchanged {
            start.get_or_insert(idx);
        } else if let Some(hunk_start) = start.take() {
            hunks.push((hunk_start, idx - 1));
        }
    }
    if let Some(hunk_start) = start {
        hunks.push((hunk_start, diff.len().saturating_sub(1)));
    }
    hunks
}

fn hunk_anchor(start: usize, _end: usize) -> usize {
    start
}

fn visible_diff_center(
    diff: &[crate::diff::DiffLine],
    scroll: usize,
    height: usize,
) -> Option<usize> {
    if height == 0 {
        return None;
    }
    let viewport_end = scroll + height.saturating_sub(1);
    let viewport_target = scroll + AUTO_FOCUS_TOP_PADDING.min(height.saturating_sub(1));
    diff_hunks(diff)
        .into_iter()
        .filter(|(start, end)| *start <= viewport_end && *end >= scroll)
        .map(|(start, end)| hunk_anchor(start, end))
        .min_by_key(|anchor| anchor.abs_diff(viewport_target))
}

fn next_diff_center(
    diff: &[crate::diff::DiffLine],
    current: Option<usize>,
    forward: bool,
) -> Option<usize> {
    let anchors = diff_hunks(diff)
        .into_iter()
        .map(|(start, end)| hunk_anchor(start, end))
        .collect::<Vec<_>>();
    match (current, forward) {
        (Some(current), true) => anchors.into_iter().find(|anchor| *anchor > current),
        (Some(current), false) => anchors.into_iter().rev().find(|anchor| *anchor < current),
        (None, true) => anchors.into_iter().next(),
        (None, false) => anchors.into_iter().last(),
    }
}

fn issue_cockpit_height(total_height: u16, prompting: bool) -> u16 {
    let reserved = 2 + u16::from(prompting);
    let available = total_height.saturating_sub(reserved + 1);
    let preferred = (total_height / 3).clamp(6, 12);
    preferred.min(available).max(1)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TrackerPaneLayout {
    menu: Rect,
    divider: Rect,
    detail: Rect,
}

fn tracker_pane_layout(area: Rect, requested_menu_width: Option<u16>) -> TrackerPaneLayout {
    if area.width < TRACKER_WIDE_MIN_WIDTH || area.height == 0 {
        return TrackerPaneLayout {
            menu: area,
            ..TrackerPaneLayout::default()
        };
    }

    let max_menu_width = area.width - TRACKER_MIN_PANE_WIDTH - 1;
    let default_menu_width = area.width.saturating_mul(2) / 5;
    let menu_width = requested_menu_width
        .unwrap_or(default_menu_width)
        .clamp(TRACKER_MIN_PANE_WIDTH, max_menu_width);
    let divider_x = area.x.saturating_add(menu_width);
    TrackerPaneLayout {
        menu: Rect::new(area.x, area.y, menu_width, area.height),
        divider: Rect::new(divider_x, area.y, 1, area.height),
        detail: Rect::new(
            divider_x.saturating_add(1),
            area.y,
            area.width - menu_width - 1,
            area.height,
        ),
    }
}

fn rect_contains(rect: Rect, column: u16, row: u16) -> bool {
    column >= rect.x
        && row >= rect.y
        && column < rect.x.saturating_add(rect.width)
        && row < rect.y.saturating_add(rect.height)
}

fn highlight_line_style(started_at: Instant) -> Option<Style> {
    let elapsed = started_at.elapsed();
    if elapsed >= HIGHLIGHT_FADE_DURATION {
        return None;
    }
    let remain = 1.0 - (elapsed.as_secs_f32() / HIGHLIGHT_FADE_DURATION.as_secs_f32());
    let blend = |from: u8, to: u8| {
        ((to as f32) + ((from as f32) - (to as f32)) * remain)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Some(Style::default().bg(Color::Rgb(
        blend(AI_HIGHLIGHT_BG.0, ASSUMED_EDITOR_BG.0),
        blend(AI_HIGHLIGHT_BG.1, ASSUMED_EDITOR_BG.1),
        blend(AI_HIGHLIGHT_BG.2, ASSUMED_EDITOR_BG.2),
    )))
}
impl App {
    fn begin_command(&mut self) {
        self.mode = InteractionMode::Command(String::new());
    }

    fn handle_command_input(&mut self, key: KeyEvent) {
        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => {
                let command = match std::mem::replace(&mut self.mode, InteractionMode::Code) {
                    InteractionMode::Command(command) => command,
                    other => {
                        self.mode = other;
                        return;
                    }
                };
                self.commit_command(command);
            }
            (KeyCode::Esc, _) => {
                self.mode = InteractionMode::Code;
            }
            (KeyCode::Backspace, _) => {
                if let InteractionMode::Command(buffer) = &mut self.mode {
                    buffer.pop();
                }
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                if let InteractionMode::Command(buffer) = &mut self.mode {
                    buffer.push(c);
                }
            }
            _ => {}
        }
    }

    fn commit_command(&mut self, command: String) {
        match command.trim() {
            "prd" => self.enter_tracker_mode(),
            "" => {
                self.mode = InteractionMode::Code;
            }
            other => {
                self.tracker_notice = Some(format!("unknown command: {other}"));
                self.mode = InteractionMode::Code;
            }
        }
    }

    fn enter_tracker_mode(&mut self) {
        self.ensure_tracker_open();
        self.refresh_tracker_context();
        self.mode = InteractionMode::Tracker;
    }

    fn ensure_tracker_open(&mut self) -> bool {
        if self.tracker.is_some() {
            return true;
        }
        match TrackerStore::open_default() {
            Ok(store) => {
                self.tracker = Some(store);
                self.tracker_notice = None;
                true
            }
            Err(error) => {
                self.tracker_notice = Some(format!("tracker unavailable: {error}"));
                false
            }
        }
    }

    fn refresh_tracker_context_if_due(&mut self) -> bool {
        if self.last_tracker_refresh.elapsed() < TRACKER_REFRESH_INTERVAL {
            return false;
        }
        self.refresh_tracker_context()
    }

    fn refresh_tracker_context(&mut self) -> bool {
        self.last_tracker_refresh = Instant::now();
        if !self.ensure_tracker_open() {
            self.tracker_snapshot = None;
            return false;
        }
        let Some(tracker) = self.tracker.as_ref() else {
            return false;
        };
        match tracker.snapshot() {
            Ok(snapshot) => {
                let changed = self.tracker_snapshot.as_ref() != Some(&snapshot);
                self.tracker_snapshot = Some(snapshot);
                self.tracker_notice = None;
                changed
            }
            Err(error) => {
                self.tracker_notice = Some(format!("tracker unavailable: {error}"));
                self.tracker_snapshot = None;
                false
            }
        }
    }

    fn tracker_status_suffix(&self) -> String {
        if let Some(snapshot) = &self.tracker_snapshot {
            return format!(
                " | {}",
                tracker_status_fragment(snapshot, &self.root, &self.issue_cockpit)
            );
        }
        self.tracker_notice
            .as_ref()
            .map(|notice| format!(" | {notice}"))
            .unwrap_or_default()
    }

    fn toggle_issue_cockpit(&mut self) {
        self.issue_cockpit.toggle_open();
        self.refresh_tracker_context();
    }

    fn toggle_issue_cockpit_scope(&mut self) {
        let Some(snapshot) = &self.tracker_snapshot else {
            return;
        };
        if match_project_for_root(snapshot, &self.root).is_none() {
            return;
        }
        self.issue_cockpit.toggle_scope();
        self.refresh_tracker_context();
    }

    fn current_issue_cockpit_ref_count(&self) -> usize {
        self.tracker_snapshot
            .as_ref()
            .map(|snapshot| {
                build_issue_cockpit_view(snapshot, &self.root, &self.issue_cockpit)
                    .refs
                    .len()
            })
            .unwrap_or(0)
    }

    fn select_next_issue_cockpit_ref(&mut self) {
        let count = self.current_issue_cockpit_ref_count();
        self.issue_cockpit.select_next_ref(count);
    }

    fn select_prev_issue_cockpit_ref(&mut self) {
        let count = self.current_issue_cockpit_ref_count();
        self.issue_cockpit.select_prev_ref(count);
    }

    fn open_selected_issue_cockpit_ref(&mut self) {
        if self.tracker_snapshot.is_none() {
            self.refresh_tracker_context();
        }
        let Some(reference) = self.tracker_snapshot.as_ref().and_then(|snapshot| {
            let view = build_issue_cockpit_view(snapshot, &self.root, &self.issue_cockpit);
            view.refs.get(self.issue_cockpit.selected_ref).cloned()
        }) else {
            return;
        };
        let result = if reference.is_range() {
            self.highlight_path(reference.path, reference.start_line, reference.end_line)
        } else {
            self.open_path(reference.path, Some(reference.start_line))
        };
        if let Err(error) = result {
            self.tracker_notice = Some(format!("file reference unavailable: {error}"));
        }
    }

    fn in_tracker_mode(&self) -> bool {
        matches!(self.mode, InteractionMode::Tracker)
    }

    fn set_tracker_layout(&mut self, area: Rect) {
        self.tracker_area = area;
        let layout = tracker_pane_layout(area, self.tracker_menu_width);
        self.tracker_menu_area = layout.menu;
        self.tracker_divider_area = layout.divider;
        self.tracker_detail_area = layout.detail;
    }

    fn clamp_tracker_offsets(&mut self, snapshot: &crate::tracker::TrackerSnapshot) {
        let item_count = visible_items(snapshot, &self.tracker_view).len();
        self.tracker_view.clamp_selection(item_count);
        let max_menu_scroll = max_tracker_menu_scroll(
            snapshot,
            &self.tracker_view,
            self.tracker_menu_area.height as usize,
        );
        self.tracker_view.clamp_menu_scroll(max_menu_scroll);
        let max_detail_scroll = if self.tracker_detail_area.width == 0 {
            0
        } else {
            max_tracker_detail_scroll(
                snapshot,
                &self.tracker_view,
                self.tracker_detail_area.width as usize,
                self.tracker_detail_area.height as usize,
            )
        };
        self.tracker_view.clamp_detail_scroll(max_detail_scroll);
    }

    fn handle_tracker_key(&mut self, key: KeyEvent) {
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => {
                self.tracker_divider_dragging = false;
                self.mode = InteractionMode::Code;
            }
            (KeyCode::Char('j'), _) | (KeyCode::Down, _) => {
                if let Some(snapshot) = self.tracker_snapshot() {
                    self.tracker_view.move_down(&snapshot);
                    self.tracker_view
                        .reveal_selected(self.tracker_menu_area.height as usize);
                }
            }
            (KeyCode::Char('k'), _) | (KeyCode::Up, _) => {
                self.tracker_view.move_up();
                self.tracker_view
                    .reveal_selected(self.tracker_menu_area.height as usize);
            }
            (KeyCode::PageDown, _) | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                self.tracker_view.scroll_detail_down(10)
            }
            (KeyCode::PageUp, _) | (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.tracker_view.scroll_detail_up(10)
            }
            (KeyCode::Home, _) => self.tracker_view.reset_detail_scroll(),
            (KeyCode::End, _) => self.tracker_view.scroll_detail_down(usize::MAX / 2),
            (KeyCode::Char('l'), _) | (KeyCode::Enter, _) => self.toggle_selected_tracker_row(),
            (KeyCode::Char('h'), _) => self.collapse_selected_tracker_row(),
            (KeyCode::Char('a'), _) => self.toggle_tracker_scope(),
            (KeyCode::Char(' '), _) => self.cycle_selected_issue_status(),
            (KeyCode::Char('p'), _) => self.cycle_selected_prd_status(),
            _ => {}
        }
        if self.tracker_area.width > 0 {
            if let Some(snapshot) = self.tracker_snapshot() {
                self.clamp_tracker_offsets(&snapshot);
            }
        }
    }

    fn handle_tracker_mouse(&mut self, mouse: MouseEvent) {
        if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
            self.tracker_divider_dragging = false;
            return;
        }

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                if rect_contains(self.tracker_menu_area, mouse.column, mouse.row) {
                    self.tracker_view.scroll_menu_up(MOUSE_SCROLL_LINES);
                } else if rect_contains(self.tracker_detail_area, mouse.column, mouse.row) {
                    self.tracker_view.scroll_detail_up(MOUSE_SCROLL_LINES);
                } else {
                    return;
                }
            }
            MouseEventKind::ScrollDown => {
                if rect_contains(self.tracker_menu_area, mouse.column, mouse.row) {
                    self.tracker_view.scroll_menu_down(MOUSE_SCROLL_LINES);
                } else if rect_contains(self.tracker_detail_area, mouse.column, mouse.row) {
                    self.tracker_view.scroll_detail_down(MOUSE_SCROLL_LINES);
                } else {
                    return;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if rect_contains(self.tracker_divider_area, mouse.column, mouse.row) {
                    self.tracker_divider_dragging = true;
                    return;
                }
                if !rect_contains(self.tracker_menu_area, mouse.column, mouse.row) {
                    return;
                }
                let Some(snapshot) = self.tracker_snapshot() else {
                    return;
                };
                let local_row = usize::from(mouse.row - self.tracker_menu_area.y);
                let Some(index) = tracker_menu_item_at_row(
                    &snapshot,
                    &self.tracker_view,
                    local_row,
                    self.tracker_menu_area.height as usize,
                ) else {
                    return;
                };
                let item = visible_items(&snapshot, &self.tracker_view)
                    .get(index)
                    .map(|row| row.item.clone());
                self.tracker_view.select(index);
                match item {
                    Some(TrackerItemRef::Project { project_key }) => {
                        self.tracker_view.toggle(&format!("project:{project_key}"))
                    }
                    Some(TrackerItemRef::Prd {
                        project_key,
                        prd_key,
                    }) => self
                        .tracker_view
                        .toggle(&format!("prd:{project_key}/{prd_key}")),
                    Some(TrackerItemRef::Issue { .. }) | None => {}
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if !self.tracker_divider_dragging
                    || self.tracker_area.width < TRACKER_WIDE_MIN_WIDTH
                {
                    return;
                }
                let desired = mouse.column.saturating_sub(self.tracker_area.x);
                let max_width = self.tracker_area.width - TRACKER_MIN_PANE_WIDTH - 1;
                self.tracker_menu_width = Some(desired.clamp(TRACKER_MIN_PANE_WIDTH, max_width));
                self.set_tracker_layout(self.tracker_area);
            }
            _ => return,
        }

        if let Some(snapshot) = self.tracker_snapshot() {
            self.clamp_tracker_offsets(&snapshot);
        }
    }

    fn tracker_scope_header(&self, snapshot: &crate::tracker::TrackerSnapshot) -> String {
        match match_project_for_root(snapshot, &self.root) {
            Some(matched) if self.tracker_scope_mode == CockpitScopeMode::Auto => format!(
                "Project / PRD / Issue tree   scope {}   a: show all",
                matched.project_key
            ),
            Some(_) => "Project / PRD / Issue tree   scope all   a: current project".to_string(),
            None => "Project / PRD / Issue tree   scope all (no root match)".to_string(),
        }
    }

    fn tracker_snapshot(&self) -> Option<crate::tracker::TrackerSnapshot> {
        self.tracker
            .as_ref()
            .and_then(|tracker| tracker.snapshot().ok())
            .map(|snapshot| self.scope_tracker_snapshot(snapshot))
    }

    fn scope_tracker_snapshot(
        &self,
        snapshot: crate::tracker::TrackerSnapshot,
    ) -> crate::tracker::TrackerSnapshot {
        if self.tracker_scope_mode == CockpitScopeMode::AllProjects {
            return snapshot;
        }
        let Some(matched) = match_project_for_root(&snapshot, &self.root) else {
            return snapshot;
        };
        crate::tracker::TrackerSnapshot {
            projects: snapshot
                .projects
                .into_iter()
                .filter(|project| project.project.key == matched.project_key)
                .collect(),
        }
    }

    fn toggle_tracker_scope(&mut self) {
        let Some(snapshot) = self
            .tracker
            .as_ref()
            .and_then(|tracker| tracker.snapshot().ok())
        else {
            return;
        };
        if match_project_for_root(&snapshot, &self.root).is_none() {
            return;
        }
        self.tracker_scope_mode = match self.tracker_scope_mode {
            CockpitScopeMode::Auto => CockpitScopeMode::AllProjects,
            CockpitScopeMode::AllProjects => CockpitScopeMode::Auto,
        };
        self.tracker_view.select(0);
        self.tracker_view.menu_scroll = 0;
        self.tracker_view.reset_detail_scroll();
    }

    fn selected_tracker_item(&self) -> Option<TrackerItemRef> {
        let snapshot = self.tracker_snapshot()?;
        self.tracker_view.selected_item(&snapshot)
    }

    fn toggle_selected_tracker_row(&mut self) {
        match self.selected_tracker_item() {
            Some(TrackerItemRef::Project { project_key }) => {
                self.tracker_view.toggle(&format!("project:{project_key}"))
            }
            Some(TrackerItemRef::Prd {
                project_key,
                prd_key,
            }) => self
                .tracker_view
                .toggle(&format!("prd:{project_key}/{prd_key}")),
            Some(TrackerItemRef::Issue { .. }) | None => {}
        }
    }

    fn collapse_selected_tracker_row(&mut self) {
        match self.selected_tracker_item() {
            Some(TrackerItemRef::Project { project_key }) => self
                .tracker_view
                .collapse(&format!("project:{project_key}")),
            Some(TrackerItemRef::Prd {
                project_key,
                prd_key,
            }) => self
                .tracker_view
                .collapse(&format!("prd:{project_key}/{prd_key}")),
            Some(TrackerItemRef::Issue { .. }) | None => {}
        }
    }

    fn cycle_selected_issue_status(&mut self) {
        let Some(TrackerItemRef::Issue {
            project_key,
            issue_key,
            ..
        }) = self.selected_tracker_item()
        else {
            return;
        };
        let Some(snapshot) = self.tracker_snapshot() else {
            return;
        };
        let Some(status) = snapshot
            .projects
            .iter()
            .find(|project| project.project.key == project_key)
            .and_then(|project| {
                project
                    .prds
                    .iter()
                    .flat_map(|prd| prd.issues.iter())
                    .find(|issue| issue.key == issue_key)
            })
            .map(|issue| issue.status.cycle())
        else {
            return;
        };
        if let Some(tracker) = self.tracker.as_mut() {
            match tracker.set_issue_status(&project_key, &issue_key, status) {
                Ok(()) => {
                    self.refresh_tracker_context();
                }
                Err(error) => {
                    self.tracker_notice = Some(error.to_string());
                }
            }
        }
    }

    fn cycle_selected_prd_status(&mut self) {
        let Some(TrackerItemRef::Prd {
            project_key,
            prd_key,
        }) = self.selected_tracker_item()
        else {
            return;
        };
        let Some(snapshot) = self.tracker_snapshot() else {
            return;
        };
        let Some(status) = snapshot
            .projects
            .iter()
            .find(|project| project.project.key == project_key)
            .and_then(|project| project.prds.iter().find(|prd| prd.prd.key == prd_key))
            .map(|prd| prd.prd.status.cycle())
        else {
            return;
        };
        if let Some(tracker) = self.tracker.as_mut() {
            match tracker.set_prd_status(&project_key, &prd_key, status) {
                Ok(()) => {
                    self.refresh_tracker_context();
                }
                Err(error) => {
                    self.tracker_notice = Some(error.to_string());
                }
            }
        }
    }

    fn begin_search(&mut self) {
        self.search_input = Some(String::new());
    }

    fn invalidate_search(&mut self) {
        // Matches index into a tab's diff by line; any tab identity/content
        // change makes them stale, so drop the state rather than mis-highlight.
        self.last_search = None;
        self.search_input = None;
    }

    fn handle_search_input(&mut self, key: KeyEvent) {
        let Some(buffer) = self.search_input.as_mut() else {
            return;
        };
        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => {
                let query_text = std::mem::take(buffer);
                self.commit_search(query_text);
            }
            (KeyCode::Esc, _) => {
                self.search_input = None;
            }
            (KeyCode::Backspace, _) => {
                buffer.pop();
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                buffer.push(c);
            }
            _ => {}
        }
    }

    fn commit_search(&mut self, text: String) {
        self.search_input = None;
        let query = SearchQuery::new(&text);
        if query.is_empty() {
            self.last_search = None;
            return;
        }
        let Some(tab) = self.tabs.current() else {
            self.last_search = None;
            return;
        };
        let path = tab.path.clone();
        let matches = self.current_search_matches(&query);
        if matches.is_empty() {
            self.last_search = None;
            return;
        }
        let current = self.starting_match_index(&matches);
        if let Some(tab) = self.tabs.current_mut() {
            tab.selection = None;
        }
        self.last_search = Some(CommittedSearch {
            path,
            matches,
            current,
        });
        self.jump_to_current_match();
    }

    fn cycle_search(&mut self, forward: bool) {
        let Some(search) = self.last_search.as_mut() else {
            return;
        };
        if search.matches.is_empty() {
            return;
        }
        search.current = if forward {
            crate::search::next_match(&search.matches, Some(search.current)).unwrap_or(0)
        } else {
            crate::search::prev_match(&search.matches, Some(search.current)).unwrap_or(0)
        };
        self.jump_to_current_match();
    }

    fn jump_to_current_match(&mut self) {
        let Some(search) = self.last_search.as_ref() else {
            return;
        };
        let Some(m) = search.matches.get(search.current) else {
            return;
        };
        if let Some(tab) = self.tabs.current_mut() {
            tab.focus_line = Some(m.line);
            tab.auto_center = true;
        }
    }

    fn current_search_matches(&self, query: &SearchQuery) -> Vec<SearchMatch> {
        let Some(tab) = self.tabs.current() else {
            return Vec::new();
        };
        query.find(
            tab.diff
                .iter()
                .enumerate()
                .map(|(i, dl)| (i, dl.text.as_str())),
        )
    }

    /// Start at the first match at or after the viewport center, else the first match.
    fn starting_match_index(&self, matches: &[SearchMatch]) -> usize {
        let height = self.viewport_height();
        let Some(tab) = self.tabs.current() else {
            return 0;
        };
        let from = tab.scroll + AUTO_FOCUS_TOP_PADDING.min(height.saturating_sub(1));
        for (i, m) in matches.iter().enumerate() {
            if m.line >= from {
                return i;
            }
        }
        0
    }

    fn copy_selection_to_clipboard(&mut self) {
        let Some(tab) = self.tabs.current() else {
            return;
        };
        let Some(selection) = tab.selection else {
            return;
        };
        let Some(text) = selected_text(&tab.diff, selection) else {
            return;
        };
        if self.write_clipboard(&text).is_ok() {
            if let Some(tab) = self.tabs.current_mut() {
                tab.selection = None;
            }
            self.copy_notice_until = Some(Instant::now() + Duration::from_millis(900));
        }
    }

    fn write_clipboard(&mut self, text: &str) -> Result<()> {
        if let Ok(mut child) = Command::new("wl-copy").stdin(Stdio::piped()).spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(text.as_bytes())?;
            }
            self.clipboard_process = Some(child);
            self.clipboard = None;
            return Ok(());
        }

        for (cmd, args) in [
            ("xclip", vec!["-selection", "clipboard"]),
            ("xsel", vec!["--clipboard", "--input"]),
            ("pbcopy", vec![]),
        ] {
            let mut child = match Command::new(cmd).args(&args).stdin(Stdio::piped()).spawn() {
                Ok(child) => child,
                Err(_) => continue,
            };
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(text.as_bytes())?;
            }
            let status = child.wait()?;
            if status.success() {
                self.clipboard_process = None;
                self.clipboard = None;
                return Ok(());
            }
        }

        if let Some(clipboard) = &mut self.clipboard {
            if clipboard.set_text(text.to_string()).is_ok() {
                self.clipboard_process = None;
                return Ok(());
            }
        }

        if let Ok(mut clipboard) = Clipboard::new() {
            clipboard.set_text(text.to_string())?;
            self.clipboard = Some(clipboard);
            self.clipboard_process = None;
            return Ok(());
        }

        bail!("no clipboard backend succeeded")
    }
}

fn selected_text(lines: &[crate::diff::DiffLine], selection: Selection) -> Option<String> {
    let (start, end) = if selection.anchor <= selection.focus {
        (selection.anchor, selection.focus)
    } else {
        (selection.focus, selection.anchor)
    };
    let mut out = String::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx < start.line || idx > end.line {
            continue;
        }
        let chars: Vec<char> = line.text.chars().collect();
        let line_start = if idx == start.line {
            start.column.min(chars.len())
        } else {
            0
        };
        let line_end = if idx == end.line {
            end.column.min(chars.len())
        } else {
            chars.len()
        };
        if line_start < line_end {
            out.push_str(&chars[line_start..line_end].iter().collect::<String>());
        }
        if idx != end.line {
            out.push('\n');
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watcher::IgnorePolicy;
    use std::{path::Path, time::SystemTime};

    #[test]
    fn diff_marks_added_lines() {
        let lines = DiffEngine::diff("a\n", "a\nb\n");
        assert_eq!(lines[1].kind, LineKind::Added);
    }

    #[test]
    fn diff_replace_renders_removed_then_added_lines() {
        let lines = DiffEngine::diff(
            "old comment\n",
            "haiku line one\nhaiku line two\nhaiku line three\n",
        );
        assert_eq!(lines[0].kind, LineKind::Removed);
        assert_eq!(lines[1].kind, LineKind::Added);
        assert_eq!(lines[2].kind, LineKind::Added);
        assert_eq!(lines[3].kind, LineKind::Added);
    }

    #[test]
    fn visible_diff_center_tracks_hunk_near_top_of_screen() {
        let diff = vec![
            crate::diff::DiffLine {
                kind: LineKind::Unchanged,
                old_line_no: Some(1),
                new_line_no: Some(1),
                text: "a".into(),
            },
            crate::diff::DiffLine {
                kind: LineKind::Added,
                old_line_no: None,
                new_line_no: Some(2),
                text: "b".into(),
            },
            crate::diff::DiffLine {
                kind: LineKind::Added,
                old_line_no: None,
                new_line_no: Some(3),
                text: "c".into(),
            },
            crate::diff::DiffLine {
                kind: LineKind::Unchanged,
                old_line_no: Some(4),
                new_line_no: Some(4),
                text: "d".into(),
            },
            crate::diff::DiffLine {
                kind: LineKind::Unchanged,
                old_line_no: Some(5),
                new_line_no: Some(5),
                text: "e".into(),
            },
            crate::diff::DiffLine {
                kind: LineKind::Removed,
                old_line_no: Some(6),
                new_line_no: None,
                text: "f".into(),
            },
            crate::diff::DiffLine {
                kind: LineKind::Added,
                old_line_no: None,
                new_line_no: Some(6),
                text: "F".into(),
            },
            crate::diff::DiffLine {
                kind: LineKind::Unchanged,
                old_line_no: Some(7),
                new_line_no: Some(7),
                text: "g".into(),
            },
            crate::diff::DiffLine {
                kind: LineKind::Added,
                old_line_no: None,
                new_line_no: Some(8),
                text: "h".into(),
            },
        ];
        assert_eq!(visible_diff_center(&diff, 0, 4), Some(1));
        assert_eq!(visible_diff_center(&diff, 4, 3), Some(5));
        assert_eq!(visible_diff_center(&diff, 6, 3), Some(8));
    }

    #[test]
    fn next_diff_center_moves_between_hunk_starts() {
        let diff = vec![
            crate::diff::DiffLine {
                kind: LineKind::Unchanged,
                old_line_no: Some(1),
                new_line_no: Some(1),
                text: "a".into(),
            },
            crate::diff::DiffLine {
                kind: LineKind::Added,
                old_line_no: None,
                new_line_no: Some(2),
                text: "b".into(),
            },
            crate::diff::DiffLine {
                kind: LineKind::Added,
                old_line_no: None,
                new_line_no: Some(3),
                text: "c".into(),
            },
            crate::diff::DiffLine {
                kind: LineKind::Unchanged,
                old_line_no: Some(4),
                new_line_no: Some(4),
                text: "d".into(),
            },
            crate::diff::DiffLine {
                kind: LineKind::Unchanged,
                old_line_no: Some(5),
                new_line_no: Some(5),
                text: "e".into(),
            },
            crate::diff::DiffLine {
                kind: LineKind::Removed,
                old_line_no: Some(6),
                new_line_no: None,
                text: "f".into(),
            },
            crate::diff::DiffLine {
                kind: LineKind::Added,
                old_line_no: None,
                new_line_no: Some(6),
                text: "F".into(),
            },
            crate::diff::DiffLine {
                kind: LineKind::Unchanged,
                old_line_no: Some(7),
                new_line_no: Some(7),
                text: "g".into(),
            },
            crate::diff::DiffLine {
                kind: LineKind::Added,
                old_line_no: None,
                new_line_no: Some(8),
                text: "h".into(),
            },
        ];
        assert_eq!(next_diff_center(&diff, None, true), Some(1));
        assert_eq!(next_diff_center(&diff, Some(1), true), Some(5));
        assert_eq!(next_diff_center(&diff, Some(5), true), Some(8));
        assert_eq!(next_diff_center(&diff, Some(8), true), None);
        assert_eq!(next_diff_center(&diff, Some(8), false), Some(5));
        assert_eq!(next_diff_center(&diff, Some(5), false), Some(1));
    }

    #[test]
    fn colon_prd_command_enters_tracker_mode() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();

        let mut app = App::new(root.to_path_buf()).unwrap();
        assert!(!app.in_tracker_mode());

        app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(app.in_tracker_mode());
    }

    #[test]
    fn shifted_semicolon_prd_command_enters_tracker_mode() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();

        let mut app = App::new(root.to_path_buf()).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char(';'), KeyModifiers::SHIFT));
        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(app.in_tracker_mode());
    }

    #[test]
    fn ctrl_c_quits_from_tracker_mode() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();

        let mut app = App::new(root.to_path_buf()).unwrap();
        app.enter_tracker_mode();

        assert!(app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn prd_tracker_scopes_to_project_matching_watched_root_by_default() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let piv_root = dir.path().join("piv");
        let fairy_root = dir.path().join("fairy");
        fs::create_dir_all(&piv_root).unwrap();
        fs::create_dir_all(&fairy_root).unwrap();
        fs::write(piv_root.join("main.rs"), "fn main() {}\n").unwrap();

        let mut store = TrackerStore::open_in_memory().unwrap();
        store
            .create_project("piv", "piv", &[piv_root.to_str().unwrap()])
            .unwrap();
        store
            .create_project("fairy", "Fairy", &[fairy_root.to_str().unwrap()])
            .unwrap();
        store
            .upsert_plan(
                "piv",
                crate::tracker::PrdInput {
                    key: "piv-prd".into(),
                    title: "piv PRD".into(),
                    status: crate::tracker::PrdStatus::InProgress,
                    body: None,
                    source_uri: None,
                },
                vec![],
            )
            .unwrap();
        store
            .upsert_plan(
                "fairy",
                crate::tracker::PrdInput {
                    key: "fairy-prd".into(),
                    title: "Fairy PRD".into(),
                    status: crate::tracker::PrdStatus::InProgress,
                    body: None,
                    source_uri: None,
                },
                vec![],
            )
            .unwrap();

        let mut app = App::new(piv_root.clone()).unwrap();
        app.tracker = Some(store);

        let raw = app.tracker.as_ref().unwrap().snapshot().unwrap();
        assert!(app.tracker_scope_header(&raw).contains("scope piv"));
        assert!(app.tracker_scope_header(&raw).contains("a: show all"));

        let scoped = app.tracker_snapshot().unwrap();
        assert_eq!(scoped.projects.len(), 1);
        assert_eq!(scoped.projects[0].project.key, "piv");

        app.handle_tracker_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(app.tracker_scope_header(&raw).contains("scope all"));
        assert!(
            app.tracker_scope_header(&raw)
                .contains("a: current project")
        );
        let all = app.tracker_snapshot().unwrap();
        assert_eq!(all.projects.len(), 2);
    }

    #[test]
    fn i_toggles_issue_cockpit_without_entering_full_tracker_mode() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();

        let mut app = App::new(root.to_path_buf()).unwrap();
        app.tracker = Some(TrackerStore::open_in_memory().unwrap());
        assert!(!app.issue_cockpit.open);
        assert!(!app.in_tracker_mode());

        app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        assert!(app.issue_cockpit.open);
        assert!(!app.in_tracker_mode());

        app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        assert!(!app.issue_cockpit.open);
        assert!(!app.in_tracker_mode());
    }

    #[test]
    fn tracker_context_refresh_updates_status_and_unblocks_next_issue() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();

        let mut store = TrackerStore::open_in_memory().unwrap();
        store
            .create_project("piv", "piv", &[root.to_str().unwrap()])
            .unwrap();
        store
            .upsert_plan(
                "piv",
                crate::tracker::PrdInput {
                    key: "cockpit".into(),
                    title: "Cockpit".into(),
                    status: crate::tracker::PrdStatus::InProgress,
                    body: None,
                    source_uri: None,
                },
                vec![
                    crate::tracker::PlanIssueInput {
                        key: "first".into(),
                        title: "First issue".into(),
                        status: crate::tracker::IssueStatus::Open,
                        body: None,
                        position: 1,
                        depends_on: vec![],
                    },
                    crate::tracker::PlanIssueInput {
                        key: "second".into(),
                        title: "Second issue".into(),
                        status: crate::tracker::IssueStatus::Open,
                        body: None,
                        position: 2,
                        depends_on: vec!["first".into()],
                    },
                ],
            )
            .unwrap();

        let mut app = App::new(root.to_path_buf()).unwrap();
        app.tracker = Some(store);
        app.refresh_tracker_context();
        assert!(app.tracker_status_suffix().contains("◌ open first"));

        app.tracker
            .as_mut()
            .unwrap()
            .set_issue_status("piv", "first", crate::tracker::IssueStatus::InProgress)
            .unwrap();
        app.refresh_tracker_context();
        assert!(app.tracker_status_suffix().contains("● in progress first"));

        app.tracker
            .as_mut()
            .unwrap()
            .set_issue_status("piv", "first", crate::tracker::IssueStatus::Complete)
            .unwrap();
        app.refresh_tracker_context();
        assert!(app.tracker_status_suffix().contains("◌ open second"));
    }

    #[test]
    fn issue_cockpit_enter_opens_selected_line_reference() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let root = dir.path();
        let path = root.join("main.rs");
        fs::write(&path, "one\ntwo\nthree\n").unwrap();

        let mut store = TrackerStore::open_in_memory().unwrap();
        store
            .create_project("piv", "piv", &[root.to_str().unwrap()])
            .unwrap();
        store
            .upsert_plan(
                "piv",
                crate::tracker::PrdInput {
                    key: "refs".into(),
                    title: "Refs".into(),
                    status: crate::tracker::PrdStatus::InProgress,
                    body: None,
                    source_uri: None,
                },
                vec![crate::tracker::PlanIssueInput {
                    key: "line-ref".into(),
                    title: "Line ref".into(),
                    status: crate::tracker::IssueStatus::Open,
                    body: Some("Open main.rs:2".into()),
                    position: 1,
                    depends_on: vec![],
                }],
            )
            .unwrap();

        let mut app = App::new(root.to_path_buf()).unwrap();
        app.tracker = Some(store);
        app.issue_cockpit.open = true;
        app.refresh_tracker_context();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let tab = app.tabs.current().unwrap();
        assert_eq!(tab.path, path.canonicalize().unwrap());
        assert_eq!(tab.focus_line, Some(1));
        assert!(app.remote_highlight.is_none());
    }

    #[test]
    fn issue_cockpit_enter_highlights_selected_range_reference() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let root = dir.path();
        let path = root.join("main.rs");
        fs::write(&path, "one\ntwo\nthree\n").unwrap();

        let mut store = TrackerStore::open_in_memory().unwrap();
        store
            .create_project("piv", "piv", &[root.to_str().unwrap()])
            .unwrap();
        store
            .upsert_plan(
                "piv",
                crate::tracker::PrdInput {
                    key: "refs".into(),
                    title: "Refs".into(),
                    status: crate::tracker::PrdStatus::InProgress,
                    body: None,
                    source_uri: None,
                },
                vec![crate::tracker::PlanIssueInput {
                    key: "range-ref".into(),
                    title: "Range ref".into(),
                    status: crate::tracker::IssueStatus::Open,
                    body: Some("Open main.rs:2-3".into()),
                    position: 1,
                    depends_on: vec![],
                }],
            )
            .unwrap();

        let mut app = App::new(root.to_path_buf()).unwrap();
        app.tracker = Some(store);
        app.issue_cockpit.open = true;
        app.refresh_tracker_context();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let (highlight_path, start, end, _) = app.remote_highlight.as_ref().unwrap();
        assert_eq!(highlight_path, &path.canonicalize().unwrap());
        assert_eq!((*start, *end), (1, 2));
    }

    #[test]
    fn tracker_page_keys_scroll_detail_pane() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();

        let mut app = App::new(root.to_path_buf()).unwrap();
        app.enter_tracker_mode();

        app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert!(app.tracker_view.detail_scroll > 0);

        app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(app.tracker_view.detail_scroll, 0);
    }

    fn tracker_mouse_test_app() -> (tempfile::TempDir, App) {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
        let mut store = TrackerStore::open_in_memory().unwrap();
        store.create_project("piv", "piv", &[]).unwrap();
        store
            .upsert_plan(
                "piv",
                crate::tracker::PrdInput {
                    key: "mouse".into(),
                    title: "Mouse interactions".into(),
                    status: crate::tracker::PrdStatus::InProgress,
                    body: Some(
                        (1..=40)
                            .map(|line| format!("long detail line {line}"))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ),
                    source_uri: None,
                },
                (1..=12)
                    .map(|position| crate::tracker::PlanIssueInput {
                        key: format!("issue-{position}"),
                        title: format!("Issue {position}"),
                        status: crate::tracker::IssueStatus::Open,
                        body: None,
                        position,
                        depends_on: vec![],
                    })
                    .collect(),
            )
            .unwrap();

        let mut app = App::new(dir.path().to_path_buf()).unwrap();
        app.mode = InteractionMode::Tracker;
        app.tracker = Some(store);
        app.tracker_view.expand("project:piv");
        app.tracker_view.expand("prd:piv/mouse");
        app.tracker_view.selected = 1;
        app.set_tracker_layout(Rect::new(10, 5, 100, 10));
        (dir, app)
    }

    fn tracker_mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn tracker_mouse_wheels_are_pane_local_and_ignore_divider_and_outside() {
        let (_dir, mut app) = tracker_mouse_test_app();
        let selected = app.tracker_view.selected;

        app.handle_tracker_mouse(tracker_mouse(MouseEventKind::ScrollDown, 11, 8));
        assert_eq!(app.tracker_view.menu_scroll, MOUSE_SCROLL_LINES);
        assert_eq!(app.tracker_view.detail_scroll, 0);
        assert_eq!(app.tracker_view.selected, selected);

        app.handle_tracker_mouse(tracker_mouse(MouseEventKind::ScrollDown, 52, 8));
        assert_eq!(app.tracker_view.menu_scroll, MOUSE_SCROLL_LINES);
        assert_eq!(app.tracker_view.detail_scroll, MOUSE_SCROLL_LINES);
        assert_eq!(app.tracker_view.selected, selected);

        let before = app.tracker_view.clone();
        app.handle_tracker_mouse(tracker_mouse(MouseEventKind::ScrollDown, 50, 8));
        app.handle_tracker_mouse(tracker_mouse(MouseEventKind::ScrollUp, 2, 2));
        assert_eq!(app.tracker_view, before);
    }

    #[test]
    fn tracker_mouse_row_clicks_map_after_scroll_and_only_branches_toggle() {
        let (_dir, mut app) = tracker_mouse_test_app();
        app.tracker_view.detail_scroll = 9;

        app.handle_tracker_mouse(tracker_mouse(
            MouseEventKind::Down(MouseButton::Left),
            12,
            9,
        ));
        assert_eq!(app.tracker_view.selected, 2);
        assert_eq!(app.tracker_view.detail_scroll, 0);
        assert!(app.tracker_view.is_expanded("project:piv"));
        assert!(app.tracker_view.is_expanded("prd:piv/mouse"));

        app.tracker_view.menu_scroll = 5;
        app.handle_tracker_mouse(tracker_mouse(
            MouseEventKind::Down(MouseButton::Left),
            12,
            7,
        ));
        assert_eq!(app.tracker_view.selected, 5);

        app.tracker_view.menu_scroll = 0;
        app.handle_tracker_mouse(tracker_mouse(
            MouseEventKind::Down(MouseButton::Left),
            12,
            7,
        ));
        assert_eq!(app.tracker_view.selected, 0);
        assert!(!app.tracker_view.is_expanded("project:piv"));
        app.handle_tracker_mouse(tracker_mouse(
            MouseEventKind::Down(MouseButton::Left),
            12,
            7,
        ));
        assert!(app.tracker_view.is_expanded("project:piv"));
        app.handle_tracker_mouse(tracker_mouse(
            MouseEventKind::Down(MouseButton::Left),
            12,
            8,
        ));
        assert!(!app.tracker_view.is_expanded("prd:piv/mouse"));
    }

    #[test]
    fn tracker_mouse_non_menu_clicks_and_blank_rows_do_not_activate_items() {
        let (_dir, mut app) = tracker_mouse_test_app();
        app.tracker_view.collapse("project:piv");
        app.tracker_view.selected = 0;
        let selected = app.tracker_view.selected;
        let expanded = app.tracker_view.is_expanded("project:piv");

        for (column, row) in [(12, 5), (12, 6), (12, 10), (52, 8), (2, 2)] {
            app.handle_tracker_mouse(tracker_mouse(
                MouseEventKind::Down(MouseButton::Left),
                column,
                row,
            ));
        }
        app.handle_tracker_mouse(tracker_mouse(
            MouseEventKind::Down(MouseButton::Left),
            50,
            8,
        ));
        app.handle_tracker_mouse(tracker_mouse(MouseEventKind::Up(MouseButton::Left), 2, 2));

        assert_eq!(app.tracker_view.selected, selected);
        assert_eq!(app.tracker_view.is_expanded("project:piv"), expanded);
    }

    #[test]
    fn tracker_divider_drag_resizes_clamps_and_release_stops_dragging() {
        let (_dir, mut app) = tracker_mouse_test_app();
        app.handle_tracker_mouse(tracker_mouse(
            MouseEventKind::Down(MouseButton::Left),
            50,
            8,
        ));
        assert!(app.tracker_divider_dragging);

        app.handle_tracker_mouse(tracker_mouse(
            MouseEventKind::Drag(MouseButton::Left),
            45,
            30,
        ));
        assert_eq!(app.tracker_menu_area.width, 35);
        app.handle_tracker_mouse(tracker_mouse(
            MouseEventKind::Drag(MouseButton::Left),
            u16::MAX,
            0,
        ));
        assert_eq!(app.tracker_menu_area.width, 75);
        assert_eq!(app.tracker_detail_area.width, TRACKER_MIN_PANE_WIDTH);

        app.tracker_view.menu_scroll = usize::MAX;
        app.tracker_view.detail_scroll = usize::MAX;
        app.handle_tracker_mouse(tracker_mouse(MouseEventKind::Drag(MouseButton::Left), 0, 0));
        assert_eq!(app.tracker_menu_area.width, TRACKER_MIN_PANE_WIDTH);
        let snapshot = app.tracker_snapshot().unwrap();
        assert_eq!(
            app.tracker_view.menu_scroll,
            max_tracker_menu_scroll(
                &snapshot,
                &app.tracker_view,
                app.tracker_menu_area.height as usize
            )
        );
        assert_eq!(
            app.tracker_view.detail_scroll,
            max_tracker_detail_scroll(
                &snapshot,
                &app.tracker_view,
                app.tracker_detail_area.width as usize,
                app.tracker_detail_area.height as usize
            )
        );

        app.handle_tracker_mouse(tracker_mouse(
            MouseEventKind::Drag(MouseButton::Left),
            u16::MAX,
            0,
        ));
        assert_eq!(app.tracker_menu_area.width, 75);
        app.handle_tracker_mouse(tracker_mouse(MouseEventKind::Up(MouseButton::Left), 0, 0));
        assert!(!app.tracker_divider_dragging);
        app.handle_tracker_mouse(tracker_mouse(
            MouseEventKind::Drag(MouseButton::Left),
            50,
            8,
        ));
        assert_eq!(app.tracker_menu_area.width, 75);
    }

    #[test]
    fn tracker_layout_has_safe_narrow_fallback_and_non_zero_origins() {
        let narrow = tracker_pane_layout(Rect::new(7, 3, 48, 9), Some(30));
        assert_eq!(narrow.menu, Rect::new(7, 3, 48, 9));
        assert_eq!(narrow.divider, Rect::default());
        assert_eq!(narrow.detail, Rect::default());

        let wide = tracker_pane_layout(Rect::new(7, 3, 80, 9), Some(30));
        assert_eq!(wide.menu, Rect::new(7, 3, 30, 9));
        assert_eq!(wide.divider, Rect::new(37, 3, 1, 9));
        assert_eq!(wide.detail, Rect::new(38, 3, 49, 9));
    }

    #[test]
    fn tracker_terminal_resize_during_divider_drag_remains_safe_until_release() {
        let (_dir, mut app) = tracker_mouse_test_app();
        app.handle_tracker_mouse(tracker_mouse(
            MouseEventKind::Down(MouseButton::Left),
            50,
            8,
        ));
        assert!(app.tracker_divider_dragging);

        app.set_tracker_layout(Rect::new(7, 3, 40, 9));
        app.handle_tracker_mouse(tracker_mouse(
            MouseEventKind::Drag(MouseButton::Left),
            20,
            5,
        ));
        assert_eq!(app.tracker_menu_area, Rect::new(7, 3, 40, 9));
        assert_eq!(app.tracker_divider_area, Rect::default());
        assert_eq!(app.tracker_detail_area, Rect::default());

        app.handle_tracker_mouse(tracker_mouse(
            MouseEventKind::Up(MouseButton::Left),
            200,
            200,
        ));
        assert!(!app.tracker_divider_dragging);
        app.set_tracker_layout(Rect::new(7, 3, 80, 9));
        assert!(app.tracker_divider_area.width > 0);
        assert!(app.tracker_detail_area.width >= TRACKER_MIN_PANE_WIDTH);
    }

    #[test]
    fn tracker_q_and_escape_disarm_an_active_divider_drag() {
        for exit_key in [KeyCode::Char('q'), KeyCode::Esc] {
            let (_dir, mut app) = tracker_mouse_test_app();
            app.handle_mouse(tracker_mouse(
                MouseEventKind::Down(MouseButton::Left),
                50,
                8,
            ));
            assert!(app.tracker_divider_dragging);

            app.handle_tracker_key(KeyEvent::new(exit_key, KeyModifiers::NONE));

            assert_eq!(app.mode, InteractionMode::Code);
            assert!(!app.tracker_divider_dragging);
        }
    }

    #[test]
    fn tracker_release_in_code_mode_disarms_resize_before_reentry() {
        let (_dir, mut app) = tracker_mouse_test_app();
        app.handle_mouse(tracker_mouse(
            MouseEventKind::Down(MouseButton::Left),
            50,
            8,
        ));
        assert!(app.tracker_divider_dragging);

        app.mode = InteractionMode::Code;
        app.handle_mouse(tracker_mouse(MouseEventKind::Up(MouseButton::Left), 2, 2));
        assert!(!app.tracker_divider_dragging);

        app.enter_tracker_mode();
        let width = app.tracker_menu_area.width;
        app.handle_mouse(tracker_mouse(
            MouseEventKind::Drag(MouseButton::Left),
            45,
            8,
        ));
        assert_eq!(app.tracker_menu_area.width, width);
    }

    #[test]
    fn search_commits_and_cycles_through_matches() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let root = dir.path();
        let path = root.join("main.rs");
        fs::write(&path, "alpha beta\nbeta gamma\ngamma delta\n").unwrap();

        let mut app = App::new(root.to_path_buf()).unwrap();
        app.commit_search("beta".to_string());
        let search = app.last_search.as_ref().expect("search committed");
        assert_eq!(search.matches.len(), 2);
        assert_eq!(search.matches[0].line, 0);
        assert_eq!(search.matches[1].line, 1);
        // first jump goes to the match at/after the top center
        assert_eq!(app.tabs.current().unwrap().focus_line, Some(0));

        app.cycle_search(true);
        assert_eq!(app.last_search.as_ref().unwrap().current, 1);
        assert_eq!(app.tabs.current().unwrap().focus_line, Some(1));

        app.cycle_search(true);
        assert_eq!(app.last_search.as_ref().unwrap().current, 0);
    }

    #[test]
    fn search_is_case_insensitive_for_lowercase_query() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let root = dir.path();
        let path = root.join("main.rs");
        fs::write(&path, "Hello hello HELLO\n").unwrap();

        let mut app = App::new(root.to_path_buf()).unwrap();
        app.commit_search("hello".to_string());
        assert_eq!(app.last_search.as_ref().unwrap().matches.len(), 3);
    }

    #[test]
    fn center_tab_places_focus_near_top() {
        // 40-line diff so the focus line sits within range and clamp does not pin it.
        let diff: Vec<crate::diff::DiffLine> = (0..40)
            .map(|i| crate::diff::DiffLine {
                kind: LineKind::Unchanged,
                old_line_no: Some(i + 1),
                new_line_no: Some(i + 1),
                text: format!("line {i}"),
            })
            .collect();
        let mut tab = Tab {
            path: PathBuf::from("a"),
            content: String::new(),
            highlighted_lines: vec![],
            diff,
            prepared_rows: vec![],
            viewport_cache: None,
            first_change: None,
            focus_line: Some(20),
            center_diff: None,
            scroll: 0,
            auto_center: true,
            selection: None,
            last_edit: SystemTime::now(),
        };
        center_tab(&mut tab, 20);
        assert_eq!(tab.scroll, 18);
    }

    #[test]
    fn center_tab_clamps_scroll_near_eof() {
        // 30-line file, 10-row viewport, focus on the last line: the last page
        // fills the viewport rather than leaving blank filler underneath.
        let diff: Vec<crate::diff::DiffLine> = (0..30)
            .map(|i| crate::diff::DiffLine {
                kind: LineKind::Unchanged,
                old_line_no: Some(i + 1),
                new_line_no: Some(i + 1),
                text: format!("line {i}"),
            })
            .collect();
        let mut tab = Tab {
            path: PathBuf::from("a"),
            content: String::new(),
            highlighted_lines: vec![],
            diff,
            prepared_rows: vec![],
            viewport_cache: None,
            first_change: None,
            focus_line: Some(29),
            center_diff: None,
            scroll: 0,
            auto_center: true,
            selection: None,
            last_edit: SystemTime::now(),
        };
        center_tab(&mut tab, 10);
        assert_eq!(tab.scroll, 20);
    }

    #[test]
    fn clamp_tab_scroll_pins_short_file_to_top() {
        // Shorter-than-viewport file: scroll collapses to 0 so EOF sits flush.
        let diff: Vec<crate::diff::DiffLine> = (0..5)
            .map(|i| crate::diff::DiffLine {
                kind: LineKind::Unchanged,
                old_line_no: Some(i + 1),
                new_line_no: Some(i + 1),
                text: format!("line {i}"),
            })
            .collect();
        let mut tab = Tab {
            path: PathBuf::from("a"),
            content: String::new(),
            highlighted_lines: vec![],
            diff,
            prepared_rows: vec![],
            viewport_cache: None,
            first_change: None,
            focus_line: Some(4),
            center_diff: None,
            scroll: 100,
            auto_center: false,
            selection: None,
            last_edit: SystemTime::now(),
        };
        clamp_tab_scroll(&mut tab, 10);
        assert_eq!(tab.scroll, 0);
    }

    #[test]
    fn tab_manager_promotes_existing_and_evicts_lru() {
        let mut tm = TabManager::new(2);
        let mk = |p: &str| Tab {
            path: PathBuf::from(p),
            content: String::new(),
            highlighted_lines: vec![],
            diff: vec![],
            prepared_rows: vec![],
            viewport_cache: None,
            first_change: None,
            focus_line: None,
            center_diff: None,
            scroll: 0,
            auto_center: true,
            selection: None,
            last_edit: SystemTime::now(),
        };
        tm.add_or_bring_to_front(mk("a"));
        tm.add_or_bring_to_front(mk("b"));
        tm.add_or_bring_to_front(mk("a"));
        assert_eq!(tm.tabs[0].path, PathBuf::from("a"));
        tm.add_or_bring_to_front(mk("c"));
        assert_eq!(tm.len(), 2);
        assert!(tm.tabs.iter().all(|t| t.path != PathBuf::from("b")));
    }

    #[test]
    fn tab_manager_removes_path_and_adjusts_active() {
        let mut tm = TabManager::new(3);
        let mk = |p: &str| Tab {
            path: PathBuf::from(p),
            content: String::new(),
            highlighted_lines: vec![],
            diff: vec![],
            prepared_rows: vec![],
            viewport_cache: None,
            first_change: None,
            focus_line: None,
            center_diff: None,
            scroll: 0,
            auto_center: true,
            selection: None,
            last_edit: SystemTime::now(),
        };
        tm.add_or_bring_to_front(mk("a"));
        tm.add_or_bring_to_front(mk("b"));
        tm.add_or_bring_to_front(mk("c"));
        tm.active = 2;
        tm.remove(Path::new("b"));
        assert_eq!(tm.len(), 2);
        assert!(tm.tabs.iter().all(|t| t.path != PathBuf::from("b")));
        assert_eq!(tm.active, 1);
    }

    #[test]
    fn tab_manager_maps_click_column_to_tab() {
        let mut tm = TabManager::new(3);
        let mk = |p: &str| Tab {
            path: PathBuf::from(p),
            content: String::new(),
            highlighted_lines: vec![],
            diff: vec![],
            prepared_rows: vec![],
            viewport_cache: None,
            first_change: None,
            focus_line: None,
            center_diff: None,
            scroll: 0,
            auto_center: true,
            selection: None,
            last_edit: SystemTime::now(),
        };
        tm.add_or_bring_to_front(mk("alpha.rs"));
        tm.add_or_bring_to_front(mk("beta.ts"));
        assert_eq!(tm.tab_hit_at_column(2), Some(TabHit::Select(0)));
        assert_eq!(tm.tab_hit_at_column(10), Some(TabHit::Close(0)));
        assert_eq!(tm.tab_hit_at_column(15), Some(TabHit::Select(1)));
    }

    #[test]
    fn borderless_code_pane_mouse_coordinates_start_at_area_origin() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("main.rs"), "abcdef\n").unwrap();

        let mut app = App::new(root.to_path_buf()).unwrap();
        app.code_area = Rect {
            x: 10,
            y: 5,
            width: 80,
            height: 20,
        };

        let point = app
            .mouse_point_to_text_point(10 + code_prefix_width() as u16 + 2, 5)
            .unwrap();
        assert_eq!(point, TextPoint { line: 0, column: 2 });
        assert!(
            app.mouse_point_to_text_point(10 + code_prefix_width() as u16 + 2, 4)
                .is_none()
        );
    }

    #[test]
    fn highlighter_returns_styled_text_not_ansi_escapes() {
        let h = Highlighter::new().unwrap();
        let lines = h.highlight_lines(Path::new("main.rs"), "fn main() {}\n");
        let rendered = lines
            .into_iter()
            .flatten()
            .map(|s| s.content.into_owned())
            .collect::<String>();
        assert_eq!(rendered, "fn main() {}");
        assert!(!rendered.contains('\u{1b}'));
    }

    #[test]
    fn ignore_policy_filters_default_dirs() {
        let root = PathBuf::from("/tmp/project");
        let policy = IgnorePolicy::new(&root);
        assert!(!policy.allows(&root.join("target/debug/app")));
        assert!(policy.allows(&root.join("src/main.rs")));
    }

    #[test]
    fn tracked_git_file_shows_added_line_after_change() {
        use std::{fs, process::Command};
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let root = dir.path();
        Command::new("git").arg("init").arg(root).status().unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["config", "user.email", "test@example.com"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["config", "user.name", "Test User"])
            .status()
            .unwrap();

        let path = root.join("main.rs");
        fs::write(
            &path,
            (1..=21).map(|i| format!("line {i}\n")).collect::<String>(),
        )
        .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["add", "."])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "-m", "init"])
            .status()
            .unwrap();

        let mut app = App::new(root.to_path_buf()).unwrap();
        fs::write(
            &path,
            (1..=22).map(|i| format!("line {i}\n")).collect::<String>(),
        )
        .unwrap();
        app.load_change(path.clone(), SystemTime::now()).unwrap();

        let tab = app.tabs.current().unwrap();
        assert_eq!(tab.path, path.canonicalize().unwrap());
        assert_eq!(tab.diff[21].kind, LineKind::Added);
    }

    #[test]
    fn origin_main_diff_base_shows_worktree_change_against_remote_main() {
        use std::{fs, process::Command};
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let remote_dir = tempdir().unwrap();
        let root = dir.path();
        let remote = remote_dir.path().join("remote.git");

        Command::new("git")
            .arg("init")
            .arg("-b")
            .arg("main")
            .arg(root)
            .status()
            .unwrap();
        Command::new("git")
            .arg("init")
            .arg("--bare")
            .arg(&remote)
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["config", "user.email", "test@example.com"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["config", "user.name", "Test User"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["remote", "add", "origin", remote.to_str().unwrap()])
            .status()
            .unwrap();

        let path = root.join("main.rs");
        fs::write(&path, "fn main() {}\n").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["add", "."])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "-m", "init"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["push", "-u", "origin", "main"])
            .status()
            .unwrap();

        let mut app = App::new(root.to_path_buf()).unwrap();
        app.toggle_diff_base();
        fs::write(&path, "fn main() {}\nprintln!(\"hi\");\n").unwrap();
        app.load_change(path.clone(), SystemTime::now()).unwrap();

        let tab = app.tabs.current().unwrap();
        assert_eq!(tab.path, path.canonicalize().unwrap());
        assert_eq!(tab.diff[1].kind, LineKind::Added);
    }

    #[test]
    fn origin_main_push_refreshes_open_diff_without_file_change() {
        use std::{fs, process::Command};
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let remote_dir = tempdir().unwrap();
        let root = dir.path();
        let remote = remote_dir.path().join("remote.git");

        Command::new("git")
            .arg("init")
            .arg("-b")
            .arg("main")
            .arg(root)
            .status()
            .unwrap();
        Command::new("git")
            .arg("init")
            .arg("--bare")
            .arg(&remote)
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["config", "user.email", "test@example.com"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["config", "user.name", "Test User"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["remote", "add", "origin", remote.to_str().unwrap()])
            .status()
            .unwrap();

        let path = root.join("main.rs");
        fs::write(&path, "fn main() {}\n").unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["add", "."])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "-m", "init"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["push", "-u", "origin", "main"])
            .status()
            .unwrap();

        let mut app = App::new(root.to_path_buf()).unwrap();
        app.toggle_diff_base();
        fs::write(&path, "fn main() {}\nprintln!(\"hi\");\n").unwrap();
        app.load_change(path.clone(), SystemTime::now()).unwrap();
        assert_eq!(
            app.tabs
                .current()
                .unwrap()
                .diff
                .iter()
                .filter(|l| l.kind != LineKind::Unchanged)
                .count(),
            1
        );

        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["add", "."])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "-m", "second"])
            .status()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["push", "origin", "main"])
            .status()
            .unwrap();

        app.last_git_ref_probe = Instant::now() - GIT_REF_REFRESH_INTERVAL;
        assert!(app.scan_for_git_ref_changes());
        assert_eq!(
            app.tabs
                .current()
                .unwrap()
                .diff
                .iter()
                .filter(|l| l.kind != LineKind::Unchanged)
                .count(),
            0
        );
    }
}
