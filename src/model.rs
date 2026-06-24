use std::{collections::VecDeque, path::{Path, PathBuf}, time::SystemTime};

use ratatui::text::Span;

use crate::diff::DiffLine;

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

pub struct Tab {
    pub path: PathBuf,
    pub content: String,
    pub highlighted_lines: Vec<Vec<Span<'static>>>,
    pub diff: Vec<DiffLine>,
    pub first_change: Option<usize>,
    pub focus_line: Option<usize>,
    pub scroll: usize,
    pub auto_center: bool,
    pub selection: Option<Selection>,
    pub last_edit: SystemTime,
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

    pub fn tab_at_column(&self, column: u16) -> Option<usize> {
        let mut x = 0u16;
        for (index, tab) in self.tabs.iter().enumerate() {
            let name = tab.path.file_name().and_then(|s| s.to_str()).unwrap_or("?");
            let width = (name.chars().count() + 2) as u16;
            if column >= x && column < x.saturating_add(width) { return Some(index); }
            x = x.saturating_add(width);
        }
        None
    }

    pub fn remove(&mut self, path: &Path) {
        if let Some(pos) = self.tabs.iter().position(|t| t.path == path) {
            self.tabs.remove(pos);
            if self.active >= self.tabs.len() && !self.tabs.is_empty() { self.active = self.tabs.len() - 1; }
        }
    }
}
