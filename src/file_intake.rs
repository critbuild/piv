use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result};
use similar::TextDiff;
use walkdir::WalkDir;

use crate::{
    code_pane::prepare_rows,
    diff::{DiffEngine, DiffLine, LineKind},
    highlight::Highlighter,
    model::Tab,
    watcher::IgnorePolicy,
};

pub const UNREADABLE_FILE_TEXT: &str = "<binary or unreadable file>";

#[derive(Debug, Default)]
pub struct MissedChanges {
    pub changed: Vec<(PathBuf, SystemTime)>,
    pub removed: Vec<PathBuf>,
}

impl MissedChanges {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.removed.is_empty()
    }
}

pub struct FileIntake {
    root: PathBuf,
    snapshots: HashMap<PathBuf, String>,
    seen_mtimes: HashMap<PathBuf, SystemTime>,
    last_fallback_scan: Instant,
}

impl FileIntake {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            snapshots: HashMap::new(),
            seen_mtimes: HashMap::new(),
            last_fallback_scan: Instant::now(),
        }
    }

    pub fn snapshot(&self, path: &Path) -> Option<&str> {
        self.snapshots.get(path).map(String::as_str)
    }

    pub fn seen_mtime(&self, path: &Path) -> Option<SystemTime> {
        self.seen_mtimes.get(path).copied()
    }

    pub fn seed_seen_mtimes(&mut self) -> Result<()> {
        let policy = IgnorePolicy::new(&self.root);
        self.seen_mtimes = WalkDir::new(&self.root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file() && policy.allows(entry.path()))
            .filter_map(|entry| {
                entry.metadata().ok().and_then(|metadata| {
                    metadata
                        .modified()
                        .ok()
                        .map(|modified| (entry.path().to_path_buf(), modified))
                })
            })
            .collect();
        self.last_fallback_scan = Instant::now();
        Ok(())
    }

    pub fn newest_allowed_file(&self) -> Option<(PathBuf, SystemTime)> {
        let policy = IgnorePolicy::new(&self.root);
        WalkDir::new(&self.root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file() && policy.allows(entry.path()))
            .filter_map(|entry| {
                entry.metadata().ok().and_then(|metadata| {
                    metadata
                        .modified()
                        .ok()
                        .map(|modified| (entry.path().to_path_buf(), modified))
                })
            })
            .max_by_key(|(_, modified)| *modified)
    }

    pub fn resolve_existing_path(&self, path: PathBuf, context: &str) -> Result<PathBuf> {
        let path = if path.is_absolute() {
            path
        } else {
            self.root.join(path)
        };
        path.canonicalize()
            .with_context(|| format!("{context}: {}", path.display()))
    }

    pub fn load_remote_open(
        &mut self,
        path: PathBuf,
        line: Option<usize>,
        at: SystemTime,
        highlighter: &Highlighter,
        reference_content: Option<String>,
    ) -> Result<Tab> {
        let content = read_file_content(&path);
        let diff = self.diff_for_content(&path, &content, reference_content.as_deref());
        let first_change = first_changed_row(&diff);
        let focus_line = line
            .and_then(|line| row_index_for_new_line(&diff, line.saturating_sub(1)))
            .or(first_change);
        self.snapshots
            .entry(path.clone())
            .or_insert_with(|| content.clone());
        Ok(build_tab(
            path,
            content,
            diff,
            highlighter,
            first_change,
            focus_line,
            at,
        ))
    }

    pub fn load_changed(
        &mut self,
        path: PathBuf,
        at: SystemTime,
        highlighter: &Highlighter,
        reference_content: Option<String>,
    ) -> Result<Tab> {
        let content = read_file_content(&path);
        let old_snapshot = self
            .snapshots
            .get(&path)
            .map(String::as_str)
            .unwrap_or(content.as_str());
        let diff = self.diff_for_content(&path, &content, reference_content.as_deref());
        let first_change = first_changed_row(&diff);
        let focus_line = latest_snapshot_change_line(old_snapshot, &content)
            .and_then(|line| row_index_for_new_line(&diff, line))
            .or_else(|| {
                diff.iter()
                    .rposition(|line| line.kind != LineKind::Unchanged)
            });
        self.snapshots.insert(path.clone(), content.clone());
        self.seen_mtimes.insert(path.clone(), at);
        Ok(build_tab(
            path,
            content,
            diff,
            highlighter,
            first_change,
            focus_line,
            at,
        ))
    }

    pub fn remove(&mut self, path: &Path) {
        self.snapshots.remove(path);
        self.seen_mtimes.remove(path);
    }

    pub fn scan_for_missed_changes(
        &mut self,
        idle_for: Duration,
        interval: Duration,
        idle_delay: Duration,
    ) -> Result<MissedChanges> {
        if self.last_fallback_scan.elapsed() < interval || idle_for < idle_delay {
            return Ok(MissedChanges::default());
        }
        self.last_fallback_scan = Instant::now();

        let policy = IgnorePolicy::new(&self.root);
        let mut current = HashMap::new();
        let mut changed = Vec::new();
        for entry in WalkDir::new(&self.root).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() || !policy.allows(entry.path()) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            let path = entry.path().to_path_buf();
            current.insert(path.clone(), modified);
            if self
                .seen_mtimes
                .get(&path)
                .is_none_or(|old| *old < modified)
            {
                changed.push((path, modified));
            }
        }

        let removed = self
            .seen_mtimes
            .keys()
            .filter(|path| !current.contains_key(*path))
            .cloned()
            .collect::<Vec<_>>();
        self.seen_mtimes = current;
        Ok(MissedChanges { changed, removed })
    }

    #[cfg(test)]
    fn force_fallback_scan_due(&mut self, interval: Duration) {
        self.last_fallback_scan = Instant::now() - interval;
    }

    fn diff_for_content(
        &self,
        path: &Path,
        content: &str,
        reference_content: Option<&str>,
    ) -> Vec<DiffLine> {
        if let Some(old) = reference_content {
            return DiffEngine::diff(old, content);
        }
        let old = self.snapshots.get(path).map(String::as_str).unwrap_or("");
        DiffEngine::diff(old, content)
    }
}

fn read_file_content(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|_| UNREADABLE_FILE_TEXT.into())
}

fn build_tab(
    path: PathBuf,
    content: String,
    diff: Vec<DiffLine>,
    highlighter: &Highlighter,
    first_change: Option<usize>,
    focus_line: Option<usize>,
    last_edit: SystemTime,
) -> Tab {
    let highlighted_lines = highlighter.highlight_lines(&path, &content);
    let prepared_rows = prepare_rows(&diff, &highlighted_lines);
    Tab {
        path,
        content,
        highlighted_lines,
        diff,
        prepared_rows,
        viewport_cache: None,
        first_change,
        focus_line,
        center_diff: None,
        scroll: 0,
        auto_center: true,
        selection: None,
        last_edit,
    }
}

fn first_changed_row(diff: &[DiffLine]) -> Option<usize> {
    diff.iter()
        .position(|line| line.kind != LineKind::Unchanged)
}

pub fn row_index_for_new_line(diff: &[DiffLine], line: usize) -> Option<usize> {
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
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn fallback_scan_waits_until_idle_and_interval_elapsed() {
        let dir = tempdir().unwrap();
        let mut intake = FileIntake::new(dir.path().to_path_buf());
        intake.force_fallback_scan_due(Duration::from_secs(5));

        let scan = intake
            .scan_for_missed_changes(
                Duration::from_secs(0),
                Duration::from_secs(5),
                Duration::from_secs(1),
            )
            .unwrap();

        assert!(scan.is_empty());
    }

    #[test]
    fn changed_file_uses_snapshot_delta_for_focus_with_reference_diff() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let path = root.join("main.rs");
        let snapshot = "zero\none\ntwo\nthree\n";
        let reference = "reference\none\ntwo\nthree\n";
        let changed = "zero\none\ntwo\nTHREE\n";
        fs::write(&path, snapshot).unwrap();

        let highlighter = Highlighter::new().unwrap();
        let mut intake = FileIntake::new(root);
        intake
            .load_changed(path.clone(), SystemTime::UNIX_EPOCH, &highlighter, None)
            .unwrap();

        fs::write(&path, changed).unwrap();
        let tab = intake
            .load_changed(
                path.clone(),
                SystemTime::UNIX_EPOCH,
                &highlighter,
                Some(reference.to_string()),
            )
            .unwrap();

        assert!(tab.diff.iter().any(|row| {
            row.kind == LineKind::Added && row.new_line_no == Some(1)
        }));
        let focus = tab.focus_line.expect("snapshot delta should select a row");
        assert_eq!(tab.diff[focus].new_line_no, Some(4));
        assert_eq!(intake.snapshot(&path), Some(changed));
    }
}
