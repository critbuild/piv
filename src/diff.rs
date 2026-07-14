use similar::{ChangeTag, DiffTag, TextDiff};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
        let capacity: usize = diff.ops().iter().map(|op| match op.tag() {
            DiffTag::Equal | DiffTag::Delete => op.old_range().len(),
            DiffTag::Insert => op.new_range().len(),
            DiffTag::Replace => op.old_range().len() + op.new_range().len(),
        }).sum();
        let mut lines = Vec::with_capacity(capacity);
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
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_diff_preserves_row_order_numbers_and_text() {
        let rows = DiffEngine::diff("keep\nold\nremove\n", "keep\nnew\nadd\n");
        let actual = rows.iter().map(|row| {
            (row.kind, row.old_line_no, row.new_line_no, row.text.as_str())
        }).collect::<Vec<_>>();

        assert_eq!(actual, vec![
            (LineKind::Unchanged, Some(1), Some(1), "keep"),
            (LineKind::Removed, Some(2), None, "old"),
            (LineKind::Removed, Some(3), None, "remove"),
            (LineKind::Added, None, Some(2), "new"),
            (LineKind::Added, None, Some(3), "add"),
        ]);
    }
}
