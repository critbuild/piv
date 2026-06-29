use std::{collections::HashMap, fs, io::Write, path::{Path, PathBuf}, process::{Child, Command, Stdio}, sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError}, time::{Duration, Instant, SystemTime}};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local};
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{backend::CrosstermBackend, layout::{Constraint, Direction, Layout, Rect}, style::{Color, Modifier, Style}, text::{Line, Span}, widgets::{Block, Borders, Paragraph}, Frame, Terminal};
use similar::TextDiff;
use walkdir::WalkDir;

use crate::{control::{ControlCommand, ControlServer}, diff::{DiffEngine, LineKind}, highlight::{default_code_style, Highlighter}, model::{PreparedRow, Selection, Tab, TabHit, TabManager, TextPoint}, watcher::{FileWatcher, IgnorePolicy, WatchEvent}};

use arboard::Clipboard;

const MAX_TABS: usize = 10;
const BATCH_WINDOW: Duration = Duration::from_millis(120);
const MOUSE_SCROLL_LINES: usize = 5;
const FALLBACK_SCAN_INTERVAL: Duration = Duration::from_millis(750);
const FALLBACK_SCAN_IDLE_DELAY: Duration = Duration::from_millis(1200);
const GIT_REF_REFRESH_INTERVAL: Duration = Duration::from_millis(750);
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
    snapshots: HashMap<PathBuf, String>,
    seen_mtimes: HashMap<PathBuf, SystemTime>,
    last_fallback_scan: Instant,
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
}

impl App {
    pub fn new(root: PathBuf) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let watcher = FileWatcher::start(root.clone(), tx)?;
        let (command_tx, command_rx) = mpsc::channel();
        let control_server = ControlServer::start(&root, command_tx)?;
        let mut app = Self {
            root,
            rx,
            command_rx,
            _watcher: watcher,
            _control_server: control_server,
            tabs: TabManager::new(MAX_TABS),
            snapshots: HashMap::new(),
            seen_mtimes: HashMap::new(),
            last_fallback_scan: Instant::now(),
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
        };
        app.last_seen_diff_base_rev = app.current_diff_base_rev();
        app.seed_seen_mtimes()?;
        app.open_initial_file()?;
        Ok(app)
    }

    fn open_initial_file(&mut self) -> Result<()> {
        let policy = IgnorePolicy::new(&self.root);
        let newest = WalkDir::new(&self.root).into_iter().filter_map(Result::ok)
            .filter(|e| e.file_type().is_file() && policy.allows(e.path()))
            .filter_map(|e| e.metadata().ok().and_then(|m| m.modified().ok().map(|t| (e.path().to_path_buf(), t))))
            .max_by_key(|(_, t)| *t);
        if let Some((path, at)) = newest { self.load_change(path, at)?; }
        Ok(())
    }

    fn seed_seen_mtimes(&mut self) -> Result<()> {
        let policy = IgnorePolicy::new(&self.root);
        self.seen_mtimes = WalkDir::new(&self.root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file() && policy.allows(e.path()))
            .filter_map(|e| e.metadata().ok().and_then(|m| m.modified().ok().map(|t| (e.path().to_path_buf(), t))))
            .collect();
        self.last_fallback_scan = Instant::now();
        Ok(())
    }

    pub fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) -> Result<()> {
        let mut should_draw = true;
        loop {
            if should_draw {
                terminal.draw(|f| self.render(f))?;
                should_draw = false;
            }

            if self.drain_file_changes()? { should_draw = true; }
            if self.scan_for_missed_changes()? { should_draw = true; }
            if self.scan_for_git_ref_changes() { should_draw = true; }
            if self.drain_control_commands()? { should_draw = true; }

            let timeout = if self.remote_highlight.is_some() { Duration::from_millis(33) } else { Duration::from_millis(250) };
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
            ControlCommand::Highlight { path, start_line, end_line } => self.highlight_path(path, start_line, end_line)?,
            ControlCommand::Line(line) => self.focus_current_line(line),
            ControlCommand::TabNext => self.tabs.next(),
            ControlCommand::TabPrev => self.tabs.prev(),
            ControlCommand::Recenter => if let Some(tab) = self.tabs.current_mut() { tab.auto_center = true; },
        }
        Ok(())
    }

    fn drain_file_changes(&mut self) -> Result<bool> {
        let first = match self.rx.try_recv() { Ok(c) => c, Err(_) => return Ok(false) };
        let mut changed: HashMap<PathBuf, SystemTime> = HashMap::new();
        let mut removed: Vec<(PathBuf, SystemTime)> = Vec::new();
        match first {
            WatchEvent::Changed { path, at } => { changed.insert(path, at); }
            WatchEvent::Removed { path, at } => { removed.push((path, at)); }
        }
        let deadline = Instant::now() + BATCH_WINDOW;
        while Instant::now() < deadline {
            match self.rx.recv_timeout(Duration::from_millis(20)) {
                Ok(WatchEvent::Changed { path, at }) => { changed.insert(path, at); }
                Ok(WatchEvent::Removed { path, at }) => { removed.push((path, at)); }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
            }
        }
        for (path, at) in removed { self.remove_file(&path, at); }
        for (path, at) in changed { self.load_change(path, at)?; }
        Ok(true)
    }

    fn remove_file(&mut self, path: &Path, at: SystemTime) {
        self.snapshots.remove(path);
        self.seen_mtimes.remove(path);
        self.tabs.remove(path);
        self.last_change = Some(at);
    }

    fn open_path(&mut self, path: PathBuf, line: Option<usize>) -> Result<()> {
        let path = if path.is_absolute() { path } else { self.root.join(path) };
        let path = path.canonicalize().with_context(|| format!("open target does not exist: {}", path.display()))?;
        let content = fs::read_to_string(&path).unwrap_or_else(|_| "<binary or unreadable file>".into());
        let diff = self.diff_for_path(&path, &content);
        let highlighted_lines = self.highlighter.highlight_lines(&path, &content);
        let prepared_rows = prepare_rows(&diff, &highlighted_lines);
        let first_change = diff.iter().position(|l| l.kind != LineKind::Unchanged);
        self.snapshots.entry(path.clone()).or_insert_with(|| content.clone());
        let focus_line = line.and_then(|line| row_index_for_new_line(&diff, line.saturating_sub(1))).or(first_change);
        let tab = Tab { path, content, highlighted_lines, diff, prepared_rows, viewport_cache: None, first_change, focus_line, center_diff: None, scroll: 0, auto_center: true, selection: None, last_edit: SystemTime::now() };
        self.last_change = Some(SystemTime::now());
        self.tabs.add_or_bring_to_front(tab);
        Ok(())
    }

    fn highlight_path(&mut self, path: PathBuf, start_line: usize, end_line: usize) -> Result<()> {
        let path = if path.is_absolute() { path } else { self.root.join(path) };
        let path = path.canonicalize().with_context(|| format!("highlight target does not exist: {}", path.display()))?;
        let (start_line, end_line) = if start_line <= end_line { (start_line, end_line) } else { (end_line, start_line) };
        self.remote_highlight = Some((path.clone(), start_line.saturating_sub(1), end_line.saturating_sub(1), Instant::now()));
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
        if self.last_fallback_scan.elapsed() < FALLBACK_SCAN_INTERVAL { return Ok(false); }
        if self.last_input_at.elapsed() < FALLBACK_SCAN_IDLE_DELAY { return Ok(false); }
        self.last_fallback_scan = Instant::now();

        let policy = IgnorePolicy::new(&self.root);
        let mut current = HashMap::new();
        let mut changed = Vec::new();
        for entry in WalkDir::new(&self.root).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() || !policy.allows(entry.path()) { continue; }
            let Ok(metadata) = entry.metadata() else { continue; };
            let Ok(modified) = metadata.modified() else { continue; };
            let path = entry.path().to_path_buf();
            current.insert(path.clone(), modified);
            if self.seen_mtimes.get(&path).is_none_or(|old| *old < modified) {
                changed.push((path, modified));
            }
        }

        let removed = self.seen_mtimes.keys().filter(|path| !current.contains_key(*path)).cloned().collect::<Vec<_>>();
        self.seen_mtimes = current;

        let had_changes = !removed.is_empty() || !changed.is_empty();
        for path in removed { self.remove_file(&path, SystemTime::now()); }
        for (path, at) in changed { self.load_change(path, at)?; }
        Ok(had_changes)
    }

    fn load_change(&mut self, path: PathBuf, at: SystemTime) -> Result<()> {
        let content = fs::read_to_string(&path).unwrap_or_else(|_| "<binary or unreadable file>".into());
        let old_snapshot = self.snapshots.get(&path).cloned().unwrap_or_else(|| content.clone());
        let diff = self.diff_for_path(&path, &content);
        let highlighted_lines = self.highlighter.highlight_lines(&path, &content);
        let prepared_rows = prepare_rows(&diff, &highlighted_lines);
        let first_change = diff.iter().position(|l| l.kind != LineKind::Unchanged);
        let focus_line = latest_snapshot_change_line(&old_snapshot, &content)
            .and_then(|line| row_index_for_new_line(&diff, line))
            .or_else(|| diff.iter().rposition(|l| l.kind != LineKind::Unchanged));
        self.snapshots.insert(path.clone(), content.clone());
        self.seen_mtimes.insert(path.clone(), at);
        let tab = Tab { path, content, highlighted_lines, diff, prepared_rows, viewport_cache: None, first_change, focus_line, center_diff: None, scroll: 0, auto_center: true, selection: None, last_edit: at };
        self.last_change = Some(at);
        self.tabs.add_or_bring_to_front(tab);
        Ok(())
    }

    fn diff_for_path(&self, path: &Path, content: &str) -> Vec<crate::diff::DiffLine> {
        if let Some(old) = self.reference_content(path) {
            return DiffEngine::diff(&old, content);
        }
        let old = self.snapshots.get(path).map(String::as_str).unwrap_or("");
        DiffEngine::diff(old, content)
    }

    fn reference_content(&self, path: &Path) -> Option<String> {
        match self.diff_base {
            DiffBase::Head => self.git_ref_content("HEAD", path),
            DiffBase::OriginMain => {
                if self.git_ref_exists("origin/main") {
                    Some(self.git_ref_content("origin/main", path).unwrap_or_default())
                } else {
                    self.git_ref_content("HEAD", path)
                }
            }
        }
    }

    fn git_ref_exists(&self, git_ref: &str) -> bool {
        Command::new("git")
            .arg("-C").arg(&self.root)
            .args(["rev-parse", "--verify", "--quiet", git_ref])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn git_ref_content(&self, git_ref: &str, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        let output = Command::new("git")
            .arg("-C").arg(&self.root)
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
            DiffBase::OriginMain => if self.git_ref_exists("origin/main") { Some("origin/main") } else { Some("HEAD") },
        }?;
        self.git_ref_oid(git_ref)
    }

    fn git_ref_oid(&self, git_ref: &str) -> Option<String> {
        let output = Command::new("git")
            .arg("-C").arg(&self.root)
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
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => return true,
            (KeyCode::Tab, _) => self.tabs.next(),
            (KeyCode::BackTab, _) => self.tabs.prev(),
            (KeyCode::Up, _) => self.scroll_up(1),
            (KeyCode::Down, _) => self.scroll_down(1),
            (KeyCode::PageUp, _) => self.scroll_up(20),
            (KeyCode::PageDown, _) => self.scroll_down(20),
            (KeyCode::Home, _) => self.set_scroll(0),
            (KeyCode::End, _) => if let Some(t) = self.tabs.current() { self.set_scroll(t.diff.len().saturating_sub(1)); },
            (KeyCode::Char('['), _) => self.jump_to_diff(false),
            (KeyCode::Char(']'), _) => self.jump_to_diff(true),
            (KeyCode::Char('\\'), _) => self.toggle_diff_base(),
            (KeyCode::Char('c'), _) => if let Some(t) = self.tabs.current_mut() { t.auto_center = true; },
            _ => {}
        }
        false
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_up(MOUSE_SCROLL_LINES),
            MouseEventKind::ScrollDown => self.scroll_down(MOUSE_SCROLL_LINES),
            MouseEventKind::Down(MouseButton::Left) => {
                if rect_contains(self.tab_area, mouse.column, mouse.row) {
                    match self.tabs.tab_hit_at_column(mouse.column.saturating_sub(self.tab_area.x)) {
                        Some(TabHit::Select(index)) => self.tabs.select(index),
                        Some(TabHit::Close(index)) => self.tabs.remove_at(index),
                        None => {}
                    }
                    self.mouse_selecting = false;
                } else if let Some(point) = self.mouse_point_to_text_point(mouse.column, mouse.row) {
                    if let Some(tab) = self.tabs.current_mut() {
                        tab.selection = Some(Selection { anchor: point, focus: point });
                        tab.auto_center = false;
                    }
                    self.mouse_selecting = true;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.mouse_selecting {
                    if let Some(point) = self.mouse_point_to_text_point(mouse.column, mouse.row) {
                        if let Some(tab) = self.tabs.current_mut() {
                            if let Some(selection) = &mut tab.selection { selection.focus = point; }
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
        let inner_x = self.code_area.x.saturating_add(1);
        let inner_y = self.code_area.y.saturating_add(1);
        let inner_width = self.code_area.width.saturating_sub(2);
        let inner_height = self.code_area.height.saturating_sub(2);
        if column < inner_x || row < inner_y || column >= inner_x.saturating_add(inner_width) || row >= inner_y.saturating_add(inner_height) { return None; }
        let line = self.tabs.current()?.scroll + usize::from(row.saturating_sub(inner_y));
        let text = self.tabs.current()?.diff.get(line)?.text.as_str();
        let visible_column = usize::from(column.saturating_sub(inner_x));
        let text_column = visible_column.saturating_sub(code_prefix_width());
        Some(TextPoint { line, column: text_column.min(text.chars().count()) })
    }

    fn viewport_height(&self) -> usize { self.code_area.height.saturating_sub(2) as usize }
    fn set_scroll(&mut self, n: usize) { let h = self.viewport_height(); if let Some(t) = self.tabs.current_mut() { t.scroll = n; t.auto_center = false; clamp_tab_scroll(t, h); } }
    fn scroll_up(&mut self, n: usize) { let h = self.viewport_height(); if let Some(t) = self.tabs.current_mut() { t.scroll = t.scroll.saturating_sub(n); t.auto_center = false; clamp_tab_scroll(t, h); } }
    fn scroll_down(&mut self, n: usize) { let h = self.viewport_height(); if let Some(t) = self.tabs.current_mut() { t.scroll = (t.scroll + n).min(t.diff.len().saturating_sub(1)); t.auto_center = false; clamp_tab_scroll(t, h); } }

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
        if self.remote_highlight.as_ref().is_some_and(|(_, _, _, at)| at.elapsed() >= HIGHLIGHT_FADE_DURATION) {
            self.remote_highlight = None;
        }
        let chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).split(f.area());
        self.tab_area = chunks[0];
        self.code_area = chunks[1];
        self.render_tabs(f, chunks[0]);
        self.render_code(f, chunks[1]);
        self.render_status(f, chunks[2]);
    }

    fn render_tabs(&self, f: &mut Frame, area: Rect) {
        let divider_style = Style::default().fg(Color::DarkGray);
        let mut spans = vec![Span::raw(" ")];
        spans.extend(self.tabs.tabs.iter().enumerate().flat_map(|(i, t)| {
            let name = t.path.file_name().and_then(|s| s.to_str()).unwrap_or("?").to_string();
            let active = i == self.tabs.active;
            let accent_style = if active { Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD) } else { divider_style };
            let label_style = if active { Style::default().fg(Color::White).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Gray) };
            let close_style = if active { Style::default().fg(Color::Rgb(220, 80, 80)).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Rgb(180, 40, 40)) };
            let mut parts = Vec::new();
            if i > 0 { parts.push(Span::styled(" │ ", divider_style)); }
            parts.push(Span::styled(if active { "▌" } else { " " }, accent_style));
            parts.push(Span::styled(name, label_style));
            parts.push(Span::raw(" "));
            parts.push(Span::styled("×", close_style));
            parts
        }));
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_code(&mut self, f: &mut Frame, area: Rect) {
        let render_height = area.height.saturating_sub(2) as usize;
        let Some(tab) = self.tabs.current_mut() else {
            f.render_widget(Paragraph::new("No source files found yet. Waiting for changes...").block(Block::default().borders(Borders::ALL)), area);
            return;
        };
        center_tab(tab, render_height);
        if let Some(center) = visible_diff_center(&tab.diff, tab.scroll, render_height) {
            tab.center_diff = Some(center);
        }

        let height = area.height.saturating_sub(2) as usize;
        let highlight_style = self.remote_highlight.as_ref().and_then(|(_, _, _, at)| highlight_line_style(*at));
        let selection_active = tab.selection.is_some();
        let static_only = !selection_active && highlight_style.is_none();
        let lines = if static_only {
            cached_static_viewport_lines(tab, height)
        } else {
            tab.diff.iter().zip(tab.prepared_rows.iter()).enumerate().skip(tab.scroll).take(height).map(|(idx, (dl, row))| {
                let line_highlighted = self.remote_highlight.as_ref().is_some_and(|(path, start_line, end_line, _)| {
                    path == &tab.path
                        && row.new_line_no.is_some_and(|line| ((start_line + 1)..=(end_line + 1)).contains(&line))
                });
                if !selection_active && !line_highlighted {
                    return row.static_line.clone();
                }
                let mark = match row.kind { LineKind::Added => "+", LineKind::Removed => "-", LineKind::Unchanged => " " };
                let mark_style = match row.kind { LineKind::Added => Style::default().fg(Color::Green), LineKind::Removed => Style::default().fg(Color::Red), LineKind::Unchanged => Style::default().fg(Color::DarkGray) };
                let mut spans = vec![Span::styled(format!("{:>4} ", row.line_no), Style::default().fg(Color::DarkGray)), Span::styled(mark, mark_style), Span::raw(" ")];
                let mut code_spans = if selection_active {
                    apply_selection(&row.base_spans, selection_range_for_line(tab.selection, idx, &dl.text))
                } else {
                    row.base_spans.clone()
                };
                if row.kind == LineKind::Removed {
                    code_spans = apply_style_range(&code_spans, row.leading_ws, row.text_len, removed_line_style());
                }
                if line_highlighted {
                    if let Some(style) = highlight_style {
                        code_spans = apply_style_range(&code_spans, row.leading_ws, row.text_len, style);
                    }
                }
                spans.extend(code_spans);
                Line::from(spans)
            }).collect::<Vec<_>>()
        };
        f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL)), area);
    }

    fn render_status(&self, f: &mut Frame, area: Rect) {
        let copied = self.copy_notice_until.is_some_and(|until| until > Instant::now());
        let text = if let Some(tab) = self.tabs.current() {
            let rel = tab.path.strip_prefix(&self.root).unwrap_or(&tab.path).display();
            let changes = tab.diff.iter().filter(|l| l.kind != LineKind::Unchanged).count();
            let ts: DateTime<Local> = tab.last_edit.into();
            format!("{} | diff {} | {} lines | {} changes | tab {}/{} | last edit {} | {}{}{}", rel, self.diff_base.label(), tab.content.lines().count(), changes, self.tabs.active + 1, self.tabs.len(), ts.format("%H:%M:%S"), if self.last_change.is_some() { "idle" } else { "waiting" }, if copied { " | copied" } else { "" }, match self.remote_highlight.as_ref() { Some((path, start_line, end_line, at)) if path == &tab.path && at.elapsed() < HIGHLIGHT_FADE_DURATION => if start_line == end_line { format!(" | hl {}", start_line + 1) } else { format!(" | hl {}-{}", start_line + 1, end_line + 1) }, _ => String::new(), })
        } else { format!("watching {} | diff {} | idle{}", self.root.display(), self.diff_base.label(), if copied { " | copied" } else { "" }) };
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
        let updates = self.tabs.tabs.iter().map(|tab| {
            let diff = match self.reference_content(&tab.path) {
                Some(old) => DiffEngine::diff(&old, &tab.content),
                None => {
                    let old = self.snapshots.get(&tab.path).map(String::as_str).unwrap_or("");
                    DiffEngine::diff(old, &tab.content)
                }
            };
            let first_change = diff.iter().position(|l| l.kind != LineKind::Unchanged);
            let prepared_rows = prepare_rows(&diff, &tab.highlighted_lines);
            (diff, prepared_rows, first_change)
        }).collect::<Vec<_>>();

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

fn hunk_anchor(start: usize, _end: usize) -> usize { start }

fn visible_diff_center(diff: &[crate::diff::DiffLine], scroll: usize, height: usize) -> Option<usize> {
    if height == 0 { return None; }
    let viewport_end = scroll + height.saturating_sub(1);
    let viewport_target = scroll + AUTO_FOCUS_TOP_PADDING.min(height.saturating_sub(1));
    diff_hunks(diff)
        .into_iter()
        .filter(|(start, end)| *start <= viewport_end && *end >= scroll)
        .map(|(start, end)| hunk_anchor(start, end))
        .min_by_key(|anchor| anchor.abs_diff(viewport_target))
}

fn next_diff_center(diff: &[crate::diff::DiffLine], current: Option<usize>, forward: bool) -> Option<usize> {
    let anchors = diff_hunks(diff).into_iter().map(|(start, end)| hunk_anchor(start, end)).collect::<Vec<_>>();
    match (current, forward) {
        (Some(current), true) => anchors.into_iter().find(|anchor| *anchor > current),
        (Some(current), false) => anchors.into_iter().rev().find(|anchor| *anchor < current),
        (None, true) => anchors.into_iter().next(),
        (None, false) => anchors.into_iter().last(),
    }
}

fn rect_contains(rect: Rect, column: u16, row: u16) -> bool {
    column >= rect.x && row >= rect.y && column < rect.x.saturating_add(rect.width) && row < rect.y.saturating_add(rect.height)
}

fn code_prefix_width() -> usize { 7 }

fn cached_static_viewport_lines(tab: &mut Tab, height: usize) -> Vec<Line<'static>> {
    if let Some(cache) = &mut tab.viewport_cache {
        if cache.height == height {
            if cache.scroll == tab.scroll {
                return cache.lines.clone();
            }
            if cache.scroll + 1 == tab.scroll {
                if !cache.lines.is_empty() { cache.lines.remove(0); }
                let next_idx = tab.scroll + height.saturating_sub(1);
                if let Some(row) = tab.prepared_rows.get(next_idx) { cache.lines.push(row.static_line.clone()); }
                cache.scroll = tab.scroll;
                return cache.lines.clone();
            }
            if tab.scroll + 1 == cache.scroll {
                let _ = cache.lines.pop();
                if let Some(row) = tab.prepared_rows.get(tab.scroll) { cache.lines.insert(0, row.static_line.clone()); }
                cache.scroll = tab.scroll;
                return cache.lines.clone();
            }
        }
    }

    let lines = tab.prepared_rows.iter().skip(tab.scroll).take(height).map(|row| row.static_line.clone()).collect::<Vec<_>>();
    tab.viewport_cache = Some(crate::model::ViewportCache { scroll: tab.scroll, height, lines: lines.clone() });
    lines
}

fn prepare_rows(diff: &[crate::diff::DiffLine], highlighted_lines: &[Vec<Span<'static>>]) -> Vec<PreparedRow> {
    diff.iter().enumerate().map(|(idx, dl)| {
        let kind = dl.kind.clone();
        let line_no = dl.new_line_no.or(dl.old_line_no).unwrap_or(idx + 1);
        let leading_ws = leading_whitespace_chars(&dl.text);
        let text_len = dl.text.chars().count();
        let base_spans = dl.new_line_no
            .and_then(|line| highlighted_lines.get(line.saturating_sub(1)).cloned())
            .unwrap_or_else(|| vec![Span::styled(dl.text.clone(), default_code_style())]);
        let mut static_code_spans = base_spans.clone();
        if kind == LineKind::Removed {
            static_code_spans = apply_style_range(&static_code_spans, leading_ws, text_len, removed_line_style());
        }
        let mark = match kind { LineKind::Added => "+", LineKind::Removed => "-", LineKind::Unchanged => " " };
        let mark_style = match kind { LineKind::Added => Style::default().fg(Color::Green), LineKind::Removed => Style::default().fg(Color::Red), LineKind::Unchanged => Style::default().fg(Color::DarkGray) };
        let mut spans = vec![
            Span::styled(format!("{:>4} ", line_no), Style::default().fg(Color::DarkGray)),
            Span::styled(mark, mark_style),
            Span::raw(" "),
        ];
        spans.extend(static_code_spans);
        PreparedRow {
            kind,
            line_no,
            new_line_no: dl.new_line_no,
            leading_ws,
            text_len,
            base_spans,
            static_line: Line::from(spans),
        }
    }).collect()
}

fn selection_range_for_line(selection: Option<Selection>, line: usize, text: &str) -> Option<(usize, usize)> {
    let selection = selection?;
    let (start, end) = if selection.anchor <= selection.focus { (selection.anchor, selection.focus) } else { (selection.focus, selection.anchor) };
    if line < start.line || line > end.line { return None; }
    let line_len = text.chars().count();
    if start.line == end.line { return Some((start.column.min(line_len), end.column.min(line_len))); }
    if line == start.line { return Some((start.column.min(line_len), line_len)); }
    if line == end.line { return Some((0, end.column.min(line_len))); }
    Some((0, line_len))
}

fn apply_selection(spans: &[Span<'static>], range: Option<(usize, usize)>) -> Vec<Span<'static>> {
    let Some((start, end)) = range else { return spans.to_vec(); };
    if start >= end { return spans.to_vec(); }
    let mut out = Vec::new();
    let mut cursor = 0usize;
    for span in spans {
        let len = span.content.chars().count();
        let span_start = cursor;
        let span_end = cursor + len;
        let overlap_start = start.max(span_start);
        let overlap_end = end.min(span_end);
        if overlap_start >= overlap_end {
            out.push(span.clone());
        } else {
            if overlap_start > span_start { out.push(Span::styled(slice_chars(span.content.as_ref(), 0, overlap_start - span_start), span.style)); }
            out.push(Span::styled(slice_chars(span.content.as_ref(), overlap_start - span_start, overlap_end - span_start), span.style.patch(selection_style())));
            if overlap_end < span_end { out.push(Span::styled(slice_chars(span.content.as_ref(), overlap_end - span_start, len), span.style)); }
        }
        cursor = span_end;
    }
    out
}

fn slice_chars(text: &str, start: usize, end: usize) -> String {
    text.chars().skip(start).take(end.saturating_sub(start)).collect()
}

fn leading_whitespace_chars(text: &str) -> usize {
    text.chars().take_while(|c| c.is_whitespace()).count()
}

fn apply_style_range(spans: &[Span<'static>], start: usize, end: usize, style: Style) -> Vec<Span<'static>> {
    if start >= end { return spans.to_vec(); }
    let mut out = Vec::new();
    let mut cursor = 0usize;
    for span in spans {
        let len = span.content.chars().count();
        let span_start = cursor;
        let span_end = cursor + len;
        let overlap_start = start.max(span_start);
        let overlap_end = end.min(span_end);
        if overlap_start >= overlap_end {
            out.push(span.clone());
        } else {
            if overlap_start > span_start { out.push(Span::styled(slice_chars(span.content.as_ref(), 0, overlap_start - span_start), span.style)); }
            out.push(Span::styled(slice_chars(span.content.as_ref(), overlap_start - span_start, overlap_end - span_start), span.style.patch(style)));
            if overlap_end < span_end { out.push(Span::styled(slice_chars(span.content.as_ref(), overlap_end - span_start, len), span.style)); }
        }
        cursor = span_end;
    }
    out
}

fn selection_style() -> Style { Style::default().bg(Color::Rgb(62, 84, 122)) }
fn highlight_line_style(started_at: Instant) -> Option<Style> {
    let elapsed = started_at.elapsed();
    if elapsed >= HIGHLIGHT_FADE_DURATION { return None; }
    let remain = 1.0 - (elapsed.as_secs_f32() / HIGHLIGHT_FADE_DURATION.as_secs_f32());
    let blend = |from: u8, to: u8| ((to as f32) + ((from as f32) - (to as f32)) * remain).round().clamp(0.0, 255.0) as u8;
    Some(Style::default().bg(Color::Rgb(
        blend(AI_HIGHLIGHT_BG.0, ASSUMED_EDITOR_BG.0),
        blend(AI_HIGHLIGHT_BG.1, ASSUMED_EDITOR_BG.1),
        blend(AI_HIGHLIGHT_BG.2, ASSUMED_EDITOR_BG.2),
    )))
}
fn removed_line_style() -> Style { Style::default().bg(Color::Rgb(92, 48, 48)) }

impl App {
    fn copy_selection_to_clipboard(&mut self) {
        let Some(tab) = self.tabs.current() else { return; };
        let Some(selection) = tab.selection else { return; };
        let Some(text) = selected_text(&tab.diff, selection) else { return; };
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
    let (start, end) = if selection.anchor <= selection.focus { (selection.anchor, selection.focus) } else { (selection.focus, selection.anchor) };
    let mut out = String::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx < start.line || idx > end.line { continue; }
        let chars: Vec<char> = line.text.chars().collect();
        let line_start = if idx == start.line { start.column.min(chars.len()) } else { 0 };
        let line_end = if idx == end.line { end.column.min(chars.len()) } else { chars.len() };
        if line_start < line_end {
            out.push_str(&chars[line_start..line_end].iter().collect::<String>());
        }
        if idx != end.line { out.push('\n'); }
    }
    Some(out)
}

fn row_index_for_new_line(diff: &[crate::diff::DiffLine], line: usize) -> Option<usize> {
    diff.iter().position(|dl| dl.new_line_no == Some(line + 1))
}

fn latest_snapshot_change_line(old: &str, new: &str) -> Option<usize> {
    let diff = TextDiff::from_lines(old, new);
    for op in diff.ops().iter().rev() {
        match op.tag() {
            similar::DiffTag::Insert | similar::DiffTag::Replace => {
                if op.new_range().len() > 0 {
                    return Some(op.new_range().end.saturating_sub(1));
                }
            }
            similar::DiffTag::Delete => {
                return Some(op.new_range().start.saturating_sub(1));
            }
            similar::DiffTag::Equal => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::{path::Path, time::SystemTime};
    use ratatui::text::Span;
    use super::*;
    use crate::watcher::IgnorePolicy;

    #[test]
    fn diff_marks_added_lines() {
        let lines = DiffEngine::diff("a\n", "a\nb\n");
        assert_eq!(lines[1].kind, LineKind::Added);
    }

    #[test]
    fn diff_replace_renders_removed_then_added_lines() {
        let lines = DiffEngine::diff("old comment\n", "haiku line one\nhaiku line two\nhaiku line three\n");
        assert_eq!(lines[0].kind, LineKind::Removed);
        assert_eq!(lines[1].kind, LineKind::Added);
        assert_eq!(lines[2].kind, LineKind::Added);
        assert_eq!(lines[3].kind, LineKind::Added);
    }

    #[test]
    fn visible_diff_center_tracks_hunk_near_top_of_screen() {
        let diff = vec![
            crate::diff::DiffLine { kind: LineKind::Unchanged, old_line_no: Some(1), new_line_no: Some(1), text: "a".into() },
            crate::diff::DiffLine { kind: LineKind::Added, old_line_no: None, new_line_no: Some(2), text: "b".into() },
            crate::diff::DiffLine { kind: LineKind::Added, old_line_no: None, new_line_no: Some(3), text: "c".into() },
            crate::diff::DiffLine { kind: LineKind::Unchanged, old_line_no: Some(4), new_line_no: Some(4), text: "d".into() },
            crate::diff::DiffLine { kind: LineKind::Unchanged, old_line_no: Some(5), new_line_no: Some(5), text: "e".into() },
            crate::diff::DiffLine { kind: LineKind::Removed, old_line_no: Some(6), new_line_no: None, text: "f".into() },
            crate::diff::DiffLine { kind: LineKind::Added, old_line_no: None, new_line_no: Some(6), text: "F".into() },
            crate::diff::DiffLine { kind: LineKind::Unchanged, old_line_no: Some(7), new_line_no: Some(7), text: "g".into() },
            crate::diff::DiffLine { kind: LineKind::Added, old_line_no: None, new_line_no: Some(8), text: "h".into() },
        ];
        assert_eq!(visible_diff_center(&diff, 0, 4), Some(1));
        assert_eq!(visible_diff_center(&diff, 4, 3), Some(5));
        assert_eq!(visible_diff_center(&diff, 6, 3), Some(8));
    }

    #[test]
    fn next_diff_center_moves_between_hunk_starts() {
        let diff = vec![
            crate::diff::DiffLine { kind: LineKind::Unchanged, old_line_no: Some(1), new_line_no: Some(1), text: "a".into() },
            crate::diff::DiffLine { kind: LineKind::Added, old_line_no: None, new_line_no: Some(2), text: "b".into() },
            crate::diff::DiffLine { kind: LineKind::Added, old_line_no: None, new_line_no: Some(3), text: "c".into() },
            crate::diff::DiffLine { kind: LineKind::Unchanged, old_line_no: Some(4), new_line_no: Some(4), text: "d".into() },
            crate::diff::DiffLine { kind: LineKind::Unchanged, old_line_no: Some(5), new_line_no: Some(5), text: "e".into() },
            crate::diff::DiffLine { kind: LineKind::Removed, old_line_no: Some(6), new_line_no: None, text: "f".into() },
            crate::diff::DiffLine { kind: LineKind::Added, old_line_no: None, new_line_no: Some(6), text: "F".into() },
            crate::diff::DiffLine { kind: LineKind::Unchanged, old_line_no: Some(7), new_line_no: Some(7), text: "g".into() },
            crate::diff::DiffLine { kind: LineKind::Added, old_line_no: None, new_line_no: Some(8), text: "h".into() },
        ];
        assert_eq!(next_diff_center(&diff, None, true), Some(1));
        assert_eq!(next_diff_center(&diff, Some(1), true), Some(5));
        assert_eq!(next_diff_center(&diff, Some(5), true), Some(8));
        assert_eq!(next_diff_center(&diff, Some(8), true), None);
        assert_eq!(next_diff_center(&diff, Some(8), false), Some(5));
        assert_eq!(next_diff_center(&diff, Some(5), false), Some(1));
    }

    #[test]
    fn center_tab_places_focus_near_top() {
        // 40-line diff so the focus line sits within range and clamp does not pin it.
        let diff: Vec<crate::diff::DiffLine> = (0..40)
            .map(|i| crate::diff::DiffLine { kind: LineKind::Unchanged, old_line_no: Some(i + 1), new_line_no: Some(i + 1), text: format!("line {i}") })
            .collect();
        let mut tab = Tab { path: PathBuf::from("a"), content: String::new(), highlighted_lines: vec![], diff, prepared_rows: vec![], viewport_cache: None, first_change: None, focus_line: Some(20), center_diff: None, scroll: 0, auto_center: true, selection: None, last_edit: SystemTime::now() };
        center_tab(&mut tab, 20);
        assert_eq!(tab.scroll, 18);
    }

    #[test]
    fn center_tab_clamps_scroll_near_eof() {
        // 30-line file, 10-row viewport, focus on the last line: the last page
        // fills the viewport rather than leaving blank filler underneath.
        let diff: Vec<crate::diff::DiffLine> = (0..30)
            .map(|i| crate::diff::DiffLine { kind: LineKind::Unchanged, old_line_no: Some(i + 1), new_line_no: Some(i + 1), text: format!("line {i}") })
            .collect();
        let mut tab = Tab { path: PathBuf::from("a"), content: String::new(), highlighted_lines: vec![], diff, prepared_rows: vec![], viewport_cache: None, first_change: None, focus_line: Some(29), center_diff: None, scroll: 0, auto_center: true, selection: None, last_edit: SystemTime::now() };
        center_tab(&mut tab, 10);
        assert_eq!(tab.scroll, 20);
    }

    #[test]
    fn clamp_tab_scroll_pins_short_file_to_top() {
        // Shorter-than-viewport file: scroll collapses to 0 so EOF sits flush.
        let diff: Vec<crate::diff::DiffLine> = (0..5)
            .map(|i| crate::diff::DiffLine { kind: LineKind::Unchanged, old_line_no: Some(i + 1), new_line_no: Some(i + 1), text: format!("line {i}") })
            .collect();
        let mut tab = Tab { path: PathBuf::from("a"), content: String::new(), highlighted_lines: vec![], diff, prepared_rows: vec![], viewport_cache: None, first_change: None, focus_line: Some(4), center_diff: None, scroll: 100, auto_center: false, selection: None, last_edit: SystemTime::now() };
        clamp_tab_scroll(&mut tab, 10);
        assert_eq!(tab.scroll, 0);
    }

    #[test]
    fn tab_manager_promotes_existing_and_evicts_lru() {
        let mut tm = TabManager::new(2);
        let mk = |p: &str| Tab { path: PathBuf::from(p), content: String::new(), highlighted_lines: vec![], diff: vec![], prepared_rows: vec![], viewport_cache: None, first_change: None, focus_line: None, center_diff: None, scroll: 0, auto_center: true, selection: None, last_edit: SystemTime::now() };
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
        let mk = |p: &str| Tab { path: PathBuf::from(p), content: String::new(), highlighted_lines: vec![], diff: vec![], prepared_rows: vec![], viewport_cache: None, first_change: None, focus_line: None, center_diff: None, scroll: 0, auto_center: true, selection: None, last_edit: SystemTime::now() };
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
        let mk = |p: &str| Tab { path: PathBuf::from(p), content: String::new(), highlighted_lines: vec![], diff: vec![], prepared_rows: vec![], viewport_cache: None, first_change: None, focus_line: None, center_diff: None, scroll: 0, auto_center: true, selection: None, last_edit: SystemTime::now() };
        tm.add_or_bring_to_front(mk("alpha.rs"));
        tm.add_or_bring_to_front(mk("beta.ts"));
        assert_eq!(tm.tab_hit_at_column(2), Some(TabHit::Select(0)));
        assert_eq!(tm.tab_hit_at_column(10), Some(TabHit::Close(0)));
        assert_eq!(tm.tab_hit_at_column(15), Some(TabHit::Select(1)));
    }

    #[test]
    fn selection_range_handles_multiline_drag() {
        let selection = Selection { anchor: TextPoint { line: 3, column: 5 }, focus: TextPoint { line: 5, column: 2 } };
        assert_eq!(selection_range_for_line(Some(selection), 2, "abcd"), None);
        assert_eq!(selection_range_for_line(Some(selection), 3, "abcdefgh"), Some((5, 8)));
        assert_eq!(selection_range_for_line(Some(selection), 4, "abc"), Some((0, 3)));
        assert_eq!(selection_range_for_line(Some(selection), 5, "abcdef"), Some((0, 2)));
    }

    #[test]
    fn apply_selection_preserves_text() {
        let spans = vec![Span::styled("hello world".to_string(), default_code_style())];
        let selected = apply_selection(&spans, Some((3, 8)));
        let rendered = selected.into_iter().map(|s| s.content.into_owned()).collect::<String>();
        assert_eq!(rendered, "hello world");
    }

    #[test]
    fn highlighter_returns_styled_text_not_ansi_escapes() {
        let h = Highlighter::new().unwrap();
        let lines = h.highlight_lines(Path::new("main.rs"), "fn main() {}\n");
        let rendered = lines.into_iter().flatten().map(|s| s.content.into_owned()).collect::<String>();
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
        Command::new("git").arg("-C").arg(root).args(["config", "user.email", "test@example.com"]).status().unwrap();
        Command::new("git").arg("-C").arg(root).args(["config", "user.name", "Test User"]).status().unwrap();

        let path = root.join("main.rs");
        fs::write(&path, (1..=21).map(|i| format!("line {i}\n")).collect::<String>()).unwrap();
        Command::new("git").arg("-C").arg(root).args(["add", "."]).status().unwrap();
        Command::new("git").arg("-C").arg(root).args(["commit", "-m", "init"]).status().unwrap();

        let mut app = App::new(root.to_path_buf()).unwrap();
        fs::write(&path, (1..=22).map(|i| format!("line {i}\n")).collect::<String>()).unwrap();
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

        Command::new("git").arg("init").arg("-b").arg("main").arg(root).status().unwrap();
        Command::new("git").arg("init").arg("--bare").arg(&remote).status().unwrap();
        Command::new("git").arg("-C").arg(root).args(["config", "user.email", "test@example.com"]).status().unwrap();
        Command::new("git").arg("-C").arg(root).args(["config", "user.name", "Test User"]).status().unwrap();
        Command::new("git").arg("-C").arg(root).args(["remote", "add", "origin", remote.to_str().unwrap()]).status().unwrap();

        let path = root.join("main.rs");
        fs::write(&path, "fn main() {}\n").unwrap();
        Command::new("git").arg("-C").arg(root).args(["add", "."]).status().unwrap();
        Command::new("git").arg("-C").arg(root).args(["commit", "-m", "init"]).status().unwrap();
        Command::new("git").arg("-C").arg(root).args(["push", "-u", "origin", "main"]).status().unwrap();

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

        Command::new("git").arg("init").arg("-b").arg("main").arg(root).status().unwrap();
        Command::new("git").arg("init").arg("--bare").arg(&remote).status().unwrap();
        Command::new("git").arg("-C").arg(root).args(["config", "user.email", "test@example.com"]).status().unwrap();
        Command::new("git").arg("-C").arg(root).args(["config", "user.name", "Test User"]).status().unwrap();
        Command::new("git").arg("-C").arg(root).args(["remote", "add", "origin", remote.to_str().unwrap()]).status().unwrap();

        let path = root.join("main.rs");
        fs::write(&path, "fn main() {}\n").unwrap();
        Command::new("git").arg("-C").arg(root).args(["add", "."]).status().unwrap();
        Command::new("git").arg("-C").arg(root).args(["commit", "-m", "init"]).status().unwrap();
        Command::new("git").arg("-C").arg(root).args(["push", "-u", "origin", "main"]).status().unwrap();

        let mut app = App::new(root.to_path_buf()).unwrap();
        app.toggle_diff_base();
        fs::write(&path, "fn main() {}\nprintln!(\"hi\");\n").unwrap();
        app.load_change(path.clone(), SystemTime::now()).unwrap();
        assert_eq!(app.tabs.current().unwrap().diff.iter().filter(|l| l.kind != LineKind::Unchanged).count(), 1);

        Command::new("git").arg("-C").arg(root).args(["add", "."]).status().unwrap();
        Command::new("git").arg("-C").arg(root).args(["commit", "-m", "second"]).status().unwrap();
        Command::new("git").arg("-C").arg(root).args(["push", "origin", "main"]).status().unwrap();

        app.last_git_ref_probe = Instant::now() - GIT_REF_REFRESH_INTERVAL;
        assert!(app.scan_for_git_ref_changes());
        assert_eq!(app.tabs.current().unwrap().diff.iter().filter(|l| l.kind != LineKind::Unchanged).count(), 0);
    }
}
