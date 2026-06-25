use std::{collections::HashMap, fs, io::Write, path::{Path, PathBuf}, process::{Child, Command, Stdio}, sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError}, time::{Duration, Instant, SystemTime}};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local};
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{backend::CrosstermBackend, layout::{Constraint, Direction, Layout, Rect}, style::{Color, Modifier, Style}, text::{Line, Span}, widgets::{Block, Borders, Paragraph}, Frame, Terminal};
use similar::TextDiff;
use walkdir::WalkDir;

use crate::{control::{ControlCommand, ControlServer}, diff::{DiffEngine, LineKind}, highlight::{default_code_style, Highlighter}, model::{Selection, Tab, TabManager, TextPoint}, watcher::{FileWatcher, IgnorePolicy, WatchEvent}};

use arboard::Clipboard;

const MAX_TABS: usize = 10;
const BATCH_WINDOW: Duration = Duration::from_millis(120);
const MOUSE_SCROLL_LINES: usize = 5;
const FALLBACK_SCAN_INTERVAL: Duration = Duration::from_millis(750);

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
    remote_highlight: Option<(PathBuf, usize)>,
    tab_area: Rect,
    code_area: Rect,
    mouse_selecting: bool,
    last_change: Option<SystemTime>,
    copy_notice_until: Option<Instant>,
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
            last_change: None,
            copy_notice_until: None,
        };
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
        loop {
            terminal.draw(|f| self.render(f))?;
            self.drain_file_changes()?;
            self.scan_for_missed_changes()?;
            self.drain_control_commands()?;
            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    TermEvent::Key(key) if self.handle_key(key) => break,
                    TermEvent::Mouse(mouse) => self.handle_mouse(mouse),
                    TermEvent::Resize(_, _) => {}
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn drain_control_commands(&mut self) -> Result<()> {
        loop {
            match self.command_rx.try_recv() {
                Ok(command) => self.handle_control_command(command)?,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        Ok(())
    }

    fn handle_control_command(&mut self, command: ControlCommand) -> Result<()> {
        match command {
            ControlCommand::Open { path, line } => self.open_path(path, line)?,
            ControlCommand::Highlight { path, line } => self.highlight_path(path, line)?,
            ControlCommand::Line(line) => self.focus_current_line(line),
            ControlCommand::TabNext => self.tabs.next(),
            ControlCommand::TabPrev => self.tabs.prev(),
            ControlCommand::Recenter => if let Some(tab) = self.tabs.current_mut() { tab.auto_center = true; },
        }
        Ok(())
    }

    fn drain_file_changes(&mut self) -> Result<()> {
        let first = match self.rx.try_recv() { Ok(c) => c, Err(_) => return Ok(()) };
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
        Ok(())
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
        let first_change = diff.iter().position(|l| l.kind != LineKind::Unchanged);
        self.snapshots.entry(path.clone()).or_insert_with(|| content.clone());
        let focus_line = line.map(|line| line.saturating_sub(1)).or(first_change);
        let tab = Tab { path, content, highlighted_lines, diff, first_change, focus_line, scroll: 0, auto_center: true, selection: None, last_edit: SystemTime::now() };
        self.last_change = Some(SystemTime::now());
        self.tabs.add_or_bring_to_front(tab);
        Ok(())
    }

    fn highlight_path(&mut self, path: PathBuf, line: usize) -> Result<()> {
        let path = if path.is_absolute() { path } else { self.root.join(path) };
        let path = path.canonicalize().with_context(|| format!("highlight target does not exist: {}", path.display()))?;
        self.remote_highlight = Some((path.clone(), line.saturating_sub(1)));
        self.open_path(path, Some(line))
    }

    fn focus_current_line(&mut self, line: usize) {
        if let Some(tab) = self.tabs.current_mut() {
            tab.focus_line = Some(line.saturating_sub(1));
            tab.auto_center = true;
            tab.selection = None;
        }
    }

    fn scan_for_missed_changes(&mut self) -> Result<()> {
        if self.last_fallback_scan.elapsed() < FALLBACK_SCAN_INTERVAL { return Ok(()); }
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

        for path in removed { self.remove_file(&path, SystemTime::now()); }
        for (path, at) in changed { self.load_change(path, at)?; }
        Ok(())
    }

    fn load_change(&mut self, path: PathBuf, at: SystemTime) -> Result<()> {
        let content = fs::read_to_string(&path).unwrap_or_else(|_| "<binary or unreadable file>".into());
        let old_snapshot = self.snapshots.get(&path).cloned().unwrap_or_else(|| content.clone());
        let diff = self.diff_for_path(&path, &content);
        let highlighted_lines = self.highlighter.highlight_lines(&path, &content);
        let first_change = diff.iter().position(|l| l.kind != LineKind::Unchanged);
        let focus_line = latest_snapshot_change_line(&old_snapshot, &content).or_else(|| diff.iter().rposition(|l| l.kind != LineKind::Unchanged));
        self.snapshots.insert(path.clone(), content.clone());
        self.seen_mtimes.insert(path.clone(), at);
        let tab = Tab { path, content, highlighted_lines, diff, first_change, focus_line, scroll: 0, auto_center: true, selection: None, last_edit: at };
        self.last_change = Some(at);
        self.tabs.add_or_bring_to_front(tab);
        Ok(())
    }

    fn diff_for_path(&self, path: &Path, content: &str) -> Vec<crate::diff::DiffLine> {
        if let Some(lines) = self.git_diff_lines(path, content) {
            return lines;
        }
        let old = self.snapshots.get(path).map(String::as_str).unwrap_or("");
        DiffEngine::diff(old, content)
    }

    fn git_diff_lines(&self, path: &Path, content: &str) -> Option<Vec<crate::diff::DiffLine>> {
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        let output = Command::new("git")
            .arg("-C").arg(&self.root)
            .args(["diff", "--unified=0", "--no-color", "--", &rel.to_string_lossy()])
            .output()
            .ok()?;
        if !output.status.success() { return None; }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.trim().is_empty() {
            return Some(content.lines().map(|line| crate::diff::DiffLine { kind: crate::diff::LineKind::Unchanged, text: line.to_string() }).collect());
        }
        let mut lines = Vec::new();
        for (idx, text) in content.lines().enumerate() {
            let kind = match self.git_line_kind(&stdout, idx + 1) {
                Some(kind) => kind,
                None => crate::diff::LineKind::Unchanged,
            };
            lines.push(crate::diff::DiffLine { kind, text: text.to_string() });
        }
        Some(lines)
    }

    fn git_line_kind(&self, diff: &str, line_no: usize) -> Option<crate::diff::LineKind> {
        let mut changed = false;
        for hunk in diff.lines().filter(|l| l.starts_with("@@ ")) {
            let (old_range, new_range) = parse_hunk_header_ranges(hunk)?;
            let (_old_start, old_count) = parse_hunk_range(old_range, '-')?;
            let (new_start, new_count) = parse_hunk_range(new_range, '+')?;
            let end = new_start + new_count.saturating_sub(1);
            if (new_start..=end).contains(&line_no) {
                if new_count > old_count { return Some(crate::diff::LineKind::Added); }
                changed = true;
            }
        }
        if changed { Some(crate::diff::LineKind::Modified) } else { None }
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
                    if let Some(index) = self.tabs.tab_at_column(mouse.column.saturating_sub(self.tab_area.x)) { self.tabs.select(index); }
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

    fn set_scroll(&mut self, n: usize) { if let Some(t) = self.tabs.current_mut() { t.scroll = n; t.auto_center = false; } }
    fn scroll_up(&mut self, n: usize) { if let Some(t) = self.tabs.current_mut() { t.scroll = t.scroll.saturating_sub(n); t.auto_center = false; } }
    fn scroll_down(&mut self, n: usize) { if let Some(t) = self.tabs.current_mut() { t.scroll = (t.scroll + n).min(t.diff.len().saturating_sub(1)); t.auto_center = false; } }

    fn render(&mut self, f: &mut Frame) {
        let chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).split(f.area());
        self.tab_area = chunks[0];
        self.code_area = chunks[1];
        self.render_tabs(f, chunks[0]);
        self.render_code(f, chunks[1]);
        self.render_status(f, chunks[2]);
    }

    fn render_tabs(&self, f: &mut Frame, area: Rect) {
        let spans = self.tabs.tabs.iter().enumerate().flat_map(|(i, t)| {
            let name = t.path.file_name().and_then(|s| s.to_str()).unwrap_or("?").to_string();
            let style = if i == self.tabs.active { Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Gray) };
            vec![Span::raw(" "), Span::styled(name, style), Span::raw(" ")]
        }).collect::<Vec<_>>();
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_code(&mut self, f: &mut Frame, area: Rect) {
        let render_height = area.height.saturating_sub(2) as usize;
        if let Some(t) = self.tabs.current_mut() { center_tab(t, render_height); }
        let Some(tab) = self.tabs.current() else {
            f.render_widget(Paragraph::new("No source files found yet. Waiting for changes...").block(Block::default().borders(Borders::ALL)), area);
            return;
        };
        let height = area.height.saturating_sub(2) as usize;
        let lines = tab.diff.iter().enumerate().skip(tab.scroll).take(height).map(|(idx, dl)| {
            let line_highlighted = self.remote_highlight.as_ref().is_some_and(|(path, line)| path == &tab.path && *line == idx);
            let mark = match dl.kind { LineKind::Added => "+", LineKind::Modified => "~", LineKind::Unchanged => " " };
            let mark_style = match dl.kind { LineKind::Added => Style::default().fg(Color::Green), LineKind::Modified => Style::default().fg(Color::Yellow), LineKind::Unchanged => Style::default().fg(Color::DarkGray) };
            let mut spans = vec![Span::styled(format!("{:>4} ", idx + 1), Style::default().fg(Color::DarkGray)), Span::styled(mark, mark_style), Span::raw(" ")];
            let base = tab.highlighted_lines.get(idx).cloned().unwrap_or_else(|| vec![Span::styled(dl.text.clone(), default_code_style())]);
            let mut code_spans = apply_selection(&base, selection_range_for_line(tab.selection, idx, &dl.text));
            if line_highlighted {
                code_spans = apply_style_range(&code_spans, leading_whitespace_chars(&dl.text), dl.text.chars().count(), highlight_line_style());
            }
            spans.extend(code_spans);
            Line::from(spans)
        }).collect::<Vec<_>>();
        f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL)), area);
    }

    fn render_status(&self, f: &mut Frame, area: Rect) {
        let copied = self.copy_notice_until.is_some_and(|until| until > Instant::now());
        let text = if let Some(tab) = self.tabs.current() {
            let rel = tab.path.strip_prefix(&self.root).unwrap_or(&tab.path).display();
            let changes = tab.diff.iter().filter(|l| l.kind != LineKind::Unchanged).count();
            let ts: DateTime<Local> = tab.last_edit.into();
            format!("{} | {} lines | {} changes | tab {}/{} | last edit {} | {}{}{}", rel, tab.content.lines().count(), changes, self.tabs.active + 1, self.tabs.len(), ts.format("%H:%M:%S"), if self.last_change.is_some() { "idle" } else { "waiting" }, if copied { " | copied" } else { "" }, match self.remote_highlight.as_ref() { Some((path, line)) if path == &tab.path => format!(" | hl {}", line + 1), _ => String::new(), })
        } else { format!("watching {} | idle{}", self.root.display(), if copied { " | copied" } else { "" }) };
        f.render_widget(Paragraph::new(text), area);
    }
}

fn center_tab(tab: &mut Tab, height: usize) {
    if tab.auto_center {
        if let Some(line) = tab.focus_line.or(tab.first_change) { tab.scroll = line.saturating_sub(height / 2); }
    }
}

fn rect_contains(rect: Rect, column: u16, row: u16) -> bool {
    column >= rect.x && row >= rect.y && column < rect.x.saturating_add(rect.width) && row < rect.y.saturating_add(rect.height)
}

fn code_prefix_width() -> usize { 7 }

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
fn highlight_line_style() -> Style { Style::default().bg(Color::Rgb(78, 72, 110)) }

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

fn parse_hunk_header_ranges(hunk: &str) -> Option<(&str, &str)> {
    let header = hunk.strip_prefix("@@ ")?;
    let (ranges, _) = header.split_once(" @@").unwrap_or((header, ""));
    let mut parts = ranges.split_whitespace();
    Some((parts.next()?, parts.next()?))
}

fn parse_hunk_range(range: &str, sign: char) -> Option<(usize, usize)> {
    let range = range.trim();
    let range = range.strip_prefix(sign)?;
    let (start, count) = match range.split_once(',') {
        Some((start, count)) => (start.parse().ok()?, count.parse().ok()?),
        None => (range.parse().ok()?, 1usize),
    };
    Some((start, count))
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
    fn diff_replace_maps_first_old_lines_as_modified_and_extras_as_added() {
        let lines = DiffEngine::diff("old comment\n", "haiku line one\nhaiku line two\nhaiku line three\n");
        assert_eq!(lines[0].kind, LineKind::Modified);
        assert_eq!(lines[1].kind, LineKind::Added);
        assert_eq!(lines[2].kind, LineKind::Added);
    }

    #[test]
    fn tab_manager_promotes_existing_and_evicts_lru() {
        let mut tm = TabManager::new(2);
        let mk = |p: &str| Tab { path: PathBuf::from(p), content: String::new(), highlighted_lines: vec![], diff: vec![], first_change: None, focus_line: None, scroll: 0, auto_center: true, selection: None, last_edit: SystemTime::now() };
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
        let mk = |p: &str| Tab { path: PathBuf::from(p), content: String::new(), highlighted_lines: vec![], diff: vec![], first_change: None, focus_line: None, scroll: 0, auto_center: true, selection: None, last_edit: SystemTime::now() };
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
        let mk = |p: &str| Tab { path: PathBuf::from(p), content: String::new(), highlighted_lines: vec![], diff: vec![], first_change: None, focus_line: None, scroll: 0, auto_center: true, selection: None, last_edit: SystemTime::now() };
        tm.add_or_bring_to_front(mk("alpha.rs"));
        tm.add_or_bring_to_front(mk("beta.ts"));
        assert_eq!(tm.tab_at_column(1), Some(0));
        assert_eq!(tm.tab_at_column(11), Some(1));
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
}
