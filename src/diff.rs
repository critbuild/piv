use similar::{ChangeTag, DiffTag, TextDiff};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineKind { Added, Removed, Unchanged }

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_line_no: Option<usize>,
    pub new_line_no: Option<usize>,
    pub text: String,
}

pub struct DiffEngine;

impl DiffEngine {
    pub fn diff(old: &str, new: &str) -> Vec<DiffLine> {
        let diff = TextDiff::from_lines(old, new);
        let mut lines = Vec::new();
        for op in diff.ops() {
            let mut old_line = op.old_range().start + 1;
            let mut new_line = op.new_range().start + 1;
            for change in diff.iter_changes(op) {
                match change.tag() {
                    ChangeTag::Equal => {
                        lines.push(DiffLine {
                            kind: LineKind::Unchanged,
                            old_line_no: Some(old_line),
                            new_line_no: Some(new_line),
                            text: change.value().trim_end_matches('\n').to_string(),
                        });
                        old_line += 1;
                        new_line += 1;
                    }
                    ChangeTag::Delete => {
                        lines.push(DiffLine {
                            kind: LineKind::Removed,
                            old_line_no: Some(old_line),
                            new_line_no: None,
                            text: change.value().trim_end_matches('\n').to_string(),
                        });
                        old_line += 1;
                    }
                    ChangeTag::Insert => {
                        lines.push(DiffLine {
                            kind: LineKind::Added,
                            old_line_no: None,
                            new_line_no: Some(new_line),
                            text: change.value().trim_end_matches('\n').to_string(),
                        });
                        new_line += 1;
                    }
                }
            }
            match op.tag() {
                DiffTag::Equal | DiffTag::Insert | DiffTag::Delete | DiffTag::Replace => {}
            }
        }
        lines
    }
}
