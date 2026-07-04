use std::{
    fs,
    time::{Duration, SystemTime},
};

use piv::{diff::LineKind, file_intake::FileIntake, highlight::Highlighter};
use tempfile::tempdir;

#[test]
fn remote_open_preserves_existing_snapshot_and_focuses_requested_new_line() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let path = root.join("main.rs");
    let old = "one\nold\nthree\n";
    let new = "one\ntwo\nthree\n";
    fs::write(&path, old).unwrap();

    let highlighter = Highlighter::new().unwrap();
    let mut intake = FileIntake::new(root);
    intake
        .load_changed(path.clone(), SystemTime::UNIX_EPOCH, &highlighter, None)
        .unwrap();

    fs::write(&path, new).unwrap();
    let tab = intake
        .load_remote_open(
            path.clone(),
            Some(2),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            &highlighter,
            None,
        )
        .unwrap();

    assert_eq!(intake.snapshot(&path), Some(old));
    assert_eq!(tab.focus_line, Some(2));
    assert_eq!(tab.diff[2].new_line_no, Some(2));
    assert_eq!(tab.content, new);
}

#[test]
fn changed_file_updates_snapshot_mtime_and_focuses_latest_snapshot_delta() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let path = root.join("main.rs");
    let old = "a\nb\nc\nd\n";
    let new = "A\nb\nc\nD\n";
    fs::write(&path, old).unwrap();

    let highlighter = Highlighter::new().unwrap();
    let mut intake = FileIntake::new(root);
    intake
        .load_changed(path.clone(), SystemTime::UNIX_EPOCH, &highlighter, None)
        .unwrap();

    let changed_at = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
    fs::write(&path, new).unwrap();
    let tab = intake
        .load_changed(path.clone(), changed_at, &highlighter, None)
        .unwrap();

    assert_eq!(intake.snapshot(&path), Some(new));
    assert_eq!(intake.seen_mtime(&path), Some(changed_at));
    let focus = tab
        .focus_line
        .expect("changed file should focus a diff row");
    assert_eq!(tab.diff[focus].new_line_no, Some(4));
    assert_ne!(tab.diff[focus].kind, LineKind::Unchanged);
}

#[test]
fn fallback_scan_reports_allowed_creates_and_removes_but_ignores_ignored_paths() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let removed = root.join("old.rs");
    let ignored_dir = root.join("target");
    let ignored = ignored_dir.join("generated.rs");
    fs::create_dir_all(&ignored_dir).unwrap();
    fs::write(&removed, "old\n").unwrap();
    fs::write(&ignored, "ignored\n").unwrap();

    let mut intake = FileIntake::new(root.clone());
    intake.seed_seen_mtimes().unwrap();

    fs::remove_file(&removed).unwrap();
    let created = root.join("new.rs");
    fs::write(&created, "new\n").unwrap();
    fs::write(&ignored, "ignored changed\n").unwrap();

    let scan = intake
        .scan_for_missed_changes(Duration::from_secs(10), Duration::ZERO, Duration::ZERO)
        .unwrap();

    assert!(scan.changed.iter().any(|(path, _)| path == &created));
    assert!(scan.removed.iter().any(|path| path == &removed));
    assert!(!scan.changed.iter().any(|(path, _)| path == &ignored));
    assert!(!scan.removed.iter().any(|path| path == &ignored));
}
