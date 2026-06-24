use similar::{DiffTag, TextDiff};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineKind { Added, Modified, Unchanged }

#[derive(Clone, Debug)]
pub struct DiffLine { pub kind: LineKind, pub text: String }

pub struct DiffEngine;

impl DiffEngine {
    pub fn diff(old: &str, new: &str) -> Vec<DiffLine> {
        let diff = TextDiff::from_lines(old, new);
        let mut lines = Vec::new();
        for op in diff.ops() {
            match op.tag() {
                DiffTag::Equal => {
                    for change in diff.iter_changes(op) {
                        if change.tag() == similar::ChangeTag::Delete { continue; }
                        lines.push(DiffLine { kind: LineKind::Unchanged, text: change.value().trim_end_matches('\n').to_string() });
                    }
                }
                DiffTag::Insert => {
                    for change in diff.iter_changes(op) {
                        if change.tag() == similar::ChangeTag::Delete { continue; }
                        lines.push(DiffLine { kind: LineKind::Added, text: change.value().trim_end_matches('\n').to_string() });
                    }
                }
                DiffTag::Replace => {
                    let mut new_count = 0usize;
                    for change in diff.iter_changes(op) {
                        if change.tag() == similar::ChangeTag::Delete { continue; }
                        new_count += 1;
                    }
                    let old_len = op.old_range().len();
                    let modified_count = old_len.min(new_count);
                    let mut emitted = 0usize;
                    for change in diff.iter_changes(op) {
                        if change.tag() == similar::ChangeTag::Delete { continue; }
                        let kind = if emitted < modified_count { LineKind::Modified } else { LineKind::Added };
                        emitted += 1;
                        lines.push(DiffLine { kind, text: change.value().trim_end_matches('\n').to_string() });
                    }
                }
                DiffTag::Delete => {}
            }
        }
        lines
    }
}
