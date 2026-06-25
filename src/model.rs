use std::{collections::VecDeque, path::{Path, PathBuf}, time::SystemTime};

use ratatui::text::{Line, Span};

use crate::{diff::{DiffLine, LineKind}};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPoint {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: TextPoint,
    pub focus: TextPoint,
}

pub struct PreparedRow {
    pub kind: LineKind,
    pub line_no: usize,
    pub new_line_no: Option<usize>,
    pub leading_ws: usize,
    pub text_len: usize,
    pub base_spans: Vec<Span<'static>>,
    pub static_line: Line<'static>,
}

pub struct ViewportCache {
    pub scroll: usize,
    pub height: usize,
    pub lines: Vec<Line<'static>>,
}

pub struct Tab {
    pub path: PathBuf,
    pub content: String,
    pub highlighted_lines: Vec<Vec<Span<'static>>>,
    pub diff: Vec<DiffLine>,
    pub prepared_rows: Vec<PreparedRow>,
    pub viewport_cache: Option<ViewportCache>,
    pub first_change: Option<usize>,
    pub focus_line: Option<usize>,
    pub scroll: usize,
    pub auto_center: bool,
    pub selection: Option<Selection>,
    pub last_edit: SystemTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabHit {
    Select(usize),
    Close(usize),
}

pub struct TabManager {
    pub tabs: VecDeque<Tab>,
    pub active: usize,
    pub cap: usize,
}

impl TabManager {
    pub fn new(cap: usize) -> Self { Self { tabs: VecDeque::new(), active: 0, cap } }
    pub fn current(&self) -> Option<&Tab> { self.tabs.get(self.active) }
    pub fn current_mut(&mut self) -> Option<&mut Tab> { self.tabs.get_mut(self.active) }
    pub fn len(&self) -> usize { self.tabs.len() }
    pub fn next(&mut self) { if !self.tabs.is_empty() { self.active = (self.active + 1) % self.tabs.len(); } }
    pub fn prev(&mut self) { if !self.tabs.is_empty() { self.active = (self.active + self.tabs.len() - 1) % self.tabs.len(); } }

    pub fn add_or_bring_to_front(&mut self, tab: Tab) {
        if let Some(pos) = self.tabs.iter().position(|t| t.path == tab.path) { self.tabs.remove(pos); }
        self.tabs.push_front(tab);
        while self.tabs.len() > self.cap { self.tabs.pop_back(); }
        self.active = 0;
    }

    pub fn select(&mut self, index: usize) {
        if index < self.tabs.len() { self.active = index; }
    }

    pub fn tab_hit_at_column(&self, column: u16) -> Option<TabHit> {
        let mut x = 0u16;
        for (index, tab) in self.tabs.iter().enumerate() {
            let name = tab.path.file_name().and_then(|s| s.to_str()).unwrap_or("?");
            let name_width = name.chars().count() as u16;
            let divider_width = if index > 0 { 3 } else { 0 };
            let total_width = divider_width + name_width + 3;
            if column >= x && column < x.saturating_add(total_width) {
                if divider_width > 0 && column < x + divider_width { return None; }
                let content_x = x + divider_width;
                let close_x = content_x + 1 + name_width + 1;
                return Some(if column == close_x { TabHit::Close(index) } else { TabHit::Select(index) });
            }
            x = x.saturating_add(total_width);
        }
        None
    }

    pub fn remove(&mut self, path: &Path) {
        if let Some(pos) = self.tabs.iter().position(|t| t.path == path) {
            self.remove_at(pos);
        }
    }

    pub fn remove_at(&mut self, index: usize) {
        if index >= self.tabs.len() { return; }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.active = 0;
        } else if self.active > index {
            self.active -= 1;
        } else if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
    }
}
