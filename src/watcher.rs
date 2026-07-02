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
            watcher_config(),
        )?;
        watcher.watch(&root, RecursiveMode::Recursive)?;
        Ok(Self { _watcher: watcher })
    }
}

fn watcher_config() -> Config {
    Config::default().with_follow_symlinks(false)
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
        if rel.components().any(|c| is_ignored_component(&c.as_os_str().to_string_lossy())) {
            return false;
        }
        if is_ignored_extension(rel) {
            return false;
        }
        if let Some(gi) = &self.gitignore {
            if gi.matched(path, path.is_dir()).is_ignore() { return false; }
        }
        true
    }
}

fn is_ignored_component(component: &str) -> bool {
    matches!(
        component,
        ".git"
            | "target"
            | "node_modules"
            | "__pycache__"
            | ".idea"
            | ".vscode"
            | "cache"
            | ".cache"
            | ".pytest_cache"
            | ".mypy_cache"
            | ".ruff_cache"
            | "dist"
            | "build"
            | ".venv"
            | "venv"
    ) || component.ends_with(".egg-info")
}

fn is_ignored_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::mpsc, time::Duration};

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn watcher_config_does_not_follow_symlinks() {
        assert!(!watcher_config().follow_symlinks());
    }

    #[test]
    fn watcher_emits_changed_for_file_write() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let path = root.join("main.rs");
        fs::write(&path, "fn main() {}\n").unwrap();

        let (tx, rx) = mpsc::channel();
        let _watcher = FileWatcher::start(root.clone(), tx).unwrap();

        fs::write(&path, "fn main() { println!(\"hi\"); }\n").unwrap();

        let event = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        match event {
            WatchEvent::Changed { path: changed, .. } => assert_eq!(changed, path),
            other => panic!("expected changed event, got {other:?}"),
        }
    }

    #[test]
    fn watcher_emits_changed_for_atomic_replace() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let path = root.join("main.rs");
        let tmp = root.join("main.rs.tmp");
        fs::write(&path, "fn main() {}\n").unwrap();

        let (tx, rx) = mpsc::channel();
        let _watcher = FileWatcher::start(root.clone(), tx).unwrap();

        fs::write(&tmp, "fn main() { println!(\"hi\"); }\n").unwrap();
        fs::rename(&tmp, &path).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if let Ok(WatchEvent::Changed { path: changed, .. }) = rx.recv_timeout(Duration::from_millis(200)) {
                if changed == path { return; }
            }
        }
        panic!("expected changed event for atomic replace");
    }

    #[test]
    fn ignore_policy_filters_common_cache_and_build_dirs() {
        let root = PathBuf::from("/tmp/project");
        let policy = IgnorePolicy::new(&root);
        for rel in [
            "cache/data.json",
            ".cache/data.json",
            ".pytest_cache/state",
            ".mypy_cache/module.json",
            ".ruff_cache/index",
            "dist/app.js",
            "build/output.bin",
            ".venv/bin/python",
            "venv/bin/python",
            "src/tomi.egg-info/SOURCES.txt",
            "package.egg-info/PKG-INFO",
        ] {
            assert!(!policy.allows(&root.join(rel)), "expected {rel} to be ignored");
        }
        assert!(policy.allows(&root.join("src/main.py")));
    }

    #[test]
    fn ignore_policy_filters_pdfs() {
        let root = PathBuf::from("/tmp/project");
        let policy = IgnorePolicy::new(&root);

        assert!(!policy.allows(&root.join("docs/spec.pdf")));
        assert!(!policy.allows(&root.join("docs/spec.PDF")));
        assert!(policy.allows(&root.join("docs/spec.md")));
    }
}
