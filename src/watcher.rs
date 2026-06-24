use std::{path::{Path, PathBuf}, sync::mpsc, time::SystemTime};

use anyhow::Result;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

#[derive(Clone, Debug)]
pub enum WatchEvent {
    Changed { path: PathBuf, at: SystemTime },
    Removed { path: PathBuf, at: SystemTime },
}

pub struct FileWatcher {
    _watcher: RecommendedWatcher,
}

impl FileWatcher {
    pub fn start(root: PathBuf, tx: mpsc::Sender<WatchEvent>) -> Result<Self> {
        let filter = IgnorePolicy::new(&root);
        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<Event>| {
                if let Ok(event) = res {
                    let at = SystemTime::now();
                    let removed = is_remove_event(&event.kind);
                    if !removed && !is_interesting_event(&event.kind) { return; }
                    for path in event.paths {
                        if removed {
                            if filter.allows(&path) { let _ = tx.send(WatchEvent::Removed { path, at }); }
                        } else if path.is_file() && filter.allows(&path) {
                            let _ = tx.send(WatchEvent::Changed { path, at });
                        }
                    }
                }
            },
            Config::default(),
        )?;
        watcher.watch(&root, RecursiveMode::Recursive)?;
        Ok(Self { _watcher: watcher })
    }
}

fn is_interesting_event(kind: &EventKind) -> bool {
    use notify::event::{CreateKind, ModifyKind, RenameMode};
    matches!(
        kind,
        EventKind::Create(CreateKind::File | CreateKind::Any)
            | EventKind::Modify(ModifyKind::Data(_))
            | EventKind::Modify(ModifyKind::Name(RenameMode::To | RenameMode::Both | RenameMode::Any))
            | EventKind::Modify(ModifyKind::Any)
    )
}

fn is_remove_event(kind: &EventKind) -> bool {
    use notify::event::RemoveKind;
    matches!(kind, EventKind::Remove(RemoveKind::File | RemoveKind::Any))
}

#[derive(Clone)]
pub struct IgnorePolicy {
    root: PathBuf,
    gitignore: Option<Gitignore>,
}

impl IgnorePolicy {
    pub fn new(root: &Path) -> Self {
        let gitignore_path = root.join(".gitignore");
        let gitignore = if gitignore_path.exists() {
            let mut builder = GitignoreBuilder::new(root);
            let _ = builder.add(gitignore_path);
            builder.build().ok()
        } else {
            None
        };
        Self { root: root.to_path_buf(), gitignore }
    }

    pub fn allows(&self, path: &Path) -> bool {
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        if rel.components().any(|c| {
            let s = c.as_os_str().to_string_lossy();
            matches!(s.as_ref(), ".git" | "target" | "node_modules" | "__pycache__" | ".idea" | ".vscode")
        }) {
            return false;
        }
        if let Some(gi) = &self.gitignore {
            if gi.matched(path, path.is_dir()).is_ignore() { return false; }
        }
        true
    }
}
