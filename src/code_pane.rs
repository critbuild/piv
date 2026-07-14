use ratatui::{style::{Color, Modifier, Style}, text::{Line, Span}};

use crate::{diff::{DiffLine, LineKind}, highlight::default_code_style, model::{PreparedRow, Selection, Tab, ViewportCache}, search::Match as SearchMatch};

const CODE_PREFIX_WIDTH: usize = 9;
const SEARCH_MATCH_BG: (u8, u8, u8) = (96, 74, 30);
const SEARCH_CURRENT_BG: (u8, u8, u8) = (180, 138, 40);

#[derive(Clone, Copy, Debug)]
pub struct RemoteHighlightOverlay {
    /// Zero-based new-file line range, inclusive.
    pub start_line: usize,
    pub end_line: usize,
    pub style: Style,
}

#[derive(Clone, Copy, Debug)]
pub struct SearchOverlay<'a> {
    pub matches: &'a [SearchMatch],
    pub current: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CodePaneOverlays<'a> {
    pub selection: Option<Selection>,
    pub remote_highlight: Option<RemoteHighlightOverlay>,
    pub search: Option<SearchOverlay<'a>>,
}

impl CodePaneOverlays<'_> {
    fn is_static(self) -> bool {
        self.selection.is_none() && self.remote_highlight.is_none() && self.search.is_none()
    }
}

pub fn code_prefix_width() -> usize { CODE_PREFIX_WIDTH }

pub fn prepare_rows(diff: &[DiffLine], highlighted_lines: &[Vec<Span<'static>>]) -> Vec<PreparedRow> {
    diff.iter().enumerate().map(|(idx, dl)| {
        let kind = dl.kind;
        let line_no = dl.new_line_no.or(dl.old_line_no).unwrap_or(idx + 1);
        let leading_ws = leading_whitespace_chars(&dl.text);
        let text_len = dl.text.chars().count();
        let base_spans = dl.new_line_no
            .and_then(|line| highlighted_lines.get(line.saturating_sub(1)).cloned())
            .unwrap_or_else(|| vec![Span::styled(dl.text.clone(), default_code_style())]);
        let static_code_spans = base_code_spans(&base_spans, &kind, leading_ws, text_len);
        PreparedRow {
            kind,
            line_no,
            new_line_no: dl.new_line_no,
            leading_ws,
            text_len,
            base_spans,
            static_line: Line::from(row_spans(line_no, &dl.kind, static_code_spans)),
        }
    }).collect()
}

pub fn render_code_pane(tab: &mut Tab, height: usize, overlays: CodePaneOverlays<'_>) -> Vec<Line<'static>> {
    if overlays.is_static() {
        return cached_static_viewport_lines(tab, height);
    }

    let current_match = overlays.search.and_then(|search| search.matches.get(search.current));
    let visible_len = tab
        .diff
        .len()
        .min(tab.prepared_rows.len())
        .saturating_sub(tab.scroll)
        .min(height);
    let visible_search_matches = overlays.search.map(|search| {
        group_visible_search_matches(search.matches, tab.scroll, visible_len)
    });
    tab.diff.iter().zip(tab.prepared_rows.iter()).enumerate().skip(tab.scroll).take(height).enumerate().map(|(offset, (idx, (dl, row)))| {
        let line_highlighted = overlays.remote_highlight.is_some_and(|highlight| {
            row.new_line_no.is_some_and(|line| ((highlight.start_line + 1)..=(highlight.end_line + 1)).contains(&line))
        });
        let line_matches = visible_search_matches
            .as_ref()
            .and_then(|matches| matches.get(offset))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let selection_range = selection_range_for_line(overlays.selection, idx, &dl.text)
            .filter(|(start, end)| start < end);

        if selection_range.is_none() && !line_highlighted && line_matches.is_empty() {
            return row.static_line.clone();
        }

        // Overlay order is product-visible: syntax → removed-line → remote highlight → selection → search/current search.
        let mut code_spans = row.base_spans.clone();
        if row.kind == LineKind::Removed {
            code_spans = apply_style_range(&code_spans, row.leading_ws, row.text_len, removed_line_style());
        }
        if line_highlighted {
            if let Some(highlight) = overlays.remote_highlight {
                code_spans = apply_style_range(&code_spans, row.leading_ws, row.text_len, highlight.style);
            }
        }
        if let Some(range) = selection_range {
            code_spans = apply_selection(&code_spans, Some(range));
        }
        if !line_matches.is_empty() {
            code_spans = apply_search_matches(&code_spans, line_matches, current_match);
        }

        Line::from(row_spans(row.line_no, &row.kind, code_spans))
    }).collect()
}

fn group_visible_search_matches(matches: &[SearchMatch], scroll: usize, visible_len: usize) -> Vec<Vec<&SearchMatch>> {
    let mut grouped = (0..visible_len).map(|_| Vec::new()).collect::<Vec<_>>();
    for search_match in matches {
        let Some(offset) = search_match.line.checked_sub(scroll) else { continue; };
        if let Some(line_matches) = grouped.get_mut(offset) {
            line_matches.push(search_match);
        }
    }
    grouped
}

fn cached_static_viewport_lines(tab: &mut Tab, height: usize) -> Vec<Line<'static>> {
    if let Some(cache) = &mut tab.viewport_cache {
        if cache.height == height {
            if cache.scroll == tab.scroll {
                return cache.lines.clone();
            }
            if cache.scroll + 1 == tab.scroll {
                if !cache.lines.is_empty() { cache.lines.remove(0); }
                let next_idx = tab.scroll + height.saturating_sub(1);
                if let Some(row) = tab.prepared_rows.get(next_idx) { cache.lines.push(row.static_line.clone()); }
                cache.scroll = tab.scroll;
                return cache.lines.clone();
            }
            if tab.scroll + 1 == cache.scroll {
                let _ = cache.lines.pop();
                if let Some(row) = tab.prepared_rows.get(tab.scroll) { cache.lines.insert(0, row.static_line.clone()); }
                cache.scroll = tab.scroll;
                return cache.lines.clone();
            }
        }
    }

    let lines = tab.prepared_rows.iter().skip(tab.scroll).take(height).map(|row| row.static_line.clone()).collect::<Vec<_>>();
    tab.viewport_cache = Some(ViewportCache { scroll: tab.scroll, height, lines: lines.clone() });
    lines
}

fn base_code_spans(base_spans: &[Span<'static>], kind: &LineKind, leading_ws: usize, text_len: usize) -> Vec<Span<'static>> {
    if *kind == LineKind::Removed {
        apply_style_range(base_spans, leading_ws, text_len, removed_line_style())
    } else {
        base_spans.to_vec()
    }
}

fn row_spans(line_no: usize, kind: &LineKind, code_spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    let mark = match kind { LineKind::Added => "+", LineKind::Removed => "-", LineKind::Unchanged => " " };
    let mark_style = match kind { LineKind::Added => Style::default().fg(Color::Green), LineKind::Removed => Style::default().fg(Color::Red), LineKind::Unchanged => Style::default().fg(Color::DarkGray) };
    let mut spans = vec![
        Span::styled(format!("{:>4} ", line_no), Style::default().fg(Color::DarkGray)),
        Span::styled(mark, mark_style),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
    ];
    spans.extend(code_spans);
    spans
}

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
    apply_style_range(spans, start, end, selection_style())
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
fn removed_line_style() -> Style { Style::default().bg(Color::Rgb(92, 48, 48)) }
fn search_match_style() -> Style { Style::default().bg(Color::Rgb(SEARCH_MATCH_BG.0, SEARCH_MATCH_BG.1, SEARCH_MATCH_BG.2)) }
fn search_current_style() -> Style { Style::default().bg(Color::Rgb(SEARCH_CURRENT_BG.0, SEARCH_CURRENT_BG.1, SEARCH_CURRENT_BG.2)).add_modifier(Modifier::BOLD) }

fn apply_search_matches(spans: &[Span<'static>], line_matches: &[&SearchMatch], current: Option<&SearchMatch>) -> Vec<Span<'static>> {
    // Apply right-to-left so earlier character offsets stay valid as we split spans.
    let mut out = spans.to_vec();
    for m in line_matches.iter().rev() {
        let is_current = current.is_some_and(|c| c.line == m.line && c.column == m.column);
        let style = if is_current { search_current_style() } else { search_match_style() };
        out = apply_style_range(&out, m.column, m.end, style);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::SystemTime};

    use super::*;
    use crate::{diff::DiffLine, model::{TextPoint, ViewportCache}};

    fn text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn line(kind: LineKind, old_line_no: Option<usize>, new_line_no: Option<usize>, text: &str) -> DiffLine {
        DiffLine { kind, old_line_no, new_line_no, text: text.to_string() }
    }

    fn tab_with(diff: Vec<DiffLine>) -> Tab {
        let highlighted_lines = diff.iter()
            .filter_map(|dl| dl.new_line_no.map(|_| vec![Span::styled(dl.text.clone(), default_code_style())]))
            .collect::<Vec<_>>();
        let prepared_rows = prepare_rows(&diff, &highlighted_lines);
        Tab {
            path: PathBuf::from("main.rs"),
            content: String::new(),
            highlighted_lines,
            diff,
            prepared_rows,
            viewport_cache: None,
            first_change: None,
            focus_line: None,
            center_diff: None,
            scroll: 0,
            auto_center: false,
            selection: None,
            last_edit: SystemTime::now(),
        }
    }

    #[test]
    fn prepared_rows_use_flat_gutter_separator() {
        let rows = prepare_rows(&[line(LineKind::Added, None, Some(1), "let x = 1;")], &[vec![Span::styled("let x = 1;", default_code_style())]]);
        assert_eq!(text(&rows[0].static_line), "   1 + │ let x = 1;");
        assert_eq!(code_prefix_width(), 9);
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
    fn render_code_pane_applies_selection_without_changing_text() {
        let mut tab = tab_with(vec![line(LineKind::Unchanged, Some(1), Some(1), "hello world")]);
        let lines = render_code_pane(&mut tab, 1, CodePaneOverlays {
            selection: Some(Selection { anchor: TextPoint { line: 0, column: 3 }, focus: TextPoint { line: 0, column: 8 } }),
            ..CodePaneOverlays::default()
        });
        assert_eq!(text(&lines[0]), "   1   │ hello world");
        assert!(lines[0].spans.iter().any(|span| span.content.as_ref() == "lo wo" && span.style.bg == selection_style().bg));
    }

    #[test]
    fn render_code_pane_keeps_rows_outside_selection_static() {
        let mut tab = tab_with(vec![
            line(LineKind::Unchanged, Some(1), Some(1), "selected"),
            line(LineKind::Removed, Some(2), None, "outside"),
        ]);
        let expected = tab.prepared_rows[1].static_line.clone();

        let lines = render_code_pane(&mut tab, 2, CodePaneOverlays {
            selection: Some(Selection { anchor: TextPoint { line: 0, column: 0 }, focus: TextPoint { line: 0, column: 4 } }),
            ..CodePaneOverlays::default()
        });

        assert_eq!(lines[1], expected);
    }

    #[test]
    fn render_code_pane_layers_search_current_after_selection() {
        let mut tab = tab_with(vec![line(LineKind::Unchanged, Some(1), Some(1), "hello hello")]);
        let matches = vec![SearchMatch { line: 0, column: 0, end: 5 }, SearchMatch { line: 0, column: 6, end: 11 }];
        let lines = render_code_pane(&mut tab, 1, CodePaneOverlays {
            selection: Some(Selection { anchor: TextPoint { line: 0, column: 0 }, focus: TextPoint { line: 0, column: 11 } }),
            search: Some(SearchOverlay { matches: &matches, current: 1 }),
            ..CodePaneOverlays::default()
        });
        assert_eq!(text(&lines[0]), "   1   │ hello hello");
        assert!(lines[0].spans.iter().any(|span| span.content.as_ref() == "hello" && span.style.bg == search_match_style().bg));
        assert!(lines[0].spans.iter().any(|span| span.content.as_ref() == "hello" && span.style.bg == search_current_style().bg && span.style.add_modifier.contains(Modifier::BOLD)));
    }

    #[test]
    fn render_code_pane_groups_visible_search_matches_once() {
        let mut tab = tab_with(vec![
            line(LineKind::Unchanged, Some(1), Some(1), "offscreen"),
            line(LineKind::Unchanged, Some(2), Some(2), "first hit"),
            line(LineKind::Unchanged, Some(3), Some(3), "hit then hit"),
        ]);
        tab.scroll = 1;
        let matches = vec![
            SearchMatch { line: 2, column: 9, end: 12 },
            SearchMatch { line: 0, column: 0, end: 3 },
            SearchMatch { line: 1, column: 6, end: 9 },
            SearchMatch { line: 2, column: 0, end: 3 },
        ];

        let lines = render_code_pane(&mut tab, 2, CodePaneOverlays {
            search: Some(SearchOverlay { matches: &matches, current: 2 }),
            ..CodePaneOverlays::default()
        });

        assert_eq!(lines.iter().map(text).collect::<Vec<_>>(), vec![
            "   2   │ first hit",
            "   3   │ hit then hit",
        ]);
        assert!(lines[0].spans.iter().any(|span| {
            span.content.as_ref() == "hit" && span.style.bg == search_current_style().bg
        }));
        assert_eq!(lines[1].spans.iter().filter(|span| {
            span.content.as_ref() == "hit" && span.style.bg == search_match_style().bg
        }).count(), 2);
    }

    #[test]
    fn render_code_pane_applies_remote_highlight_to_code_only() {
        let mut tab = tab_with(vec![line(LineKind::Unchanged, Some(1), Some(1), "    indented")]);
        let highlight = Style::default().bg(Color::Rgb(1, 2, 3));
        let lines = render_code_pane(&mut tab, 1, CodePaneOverlays {
            remote_highlight: Some(RemoteHighlightOverlay { start_line: 0, end_line: 0, style: highlight }),
            ..CodePaneOverlays::default()
        });
        assert_eq!(text(&lines[0]), "   1   │     indented");
        assert!(lines[0].spans.iter().any(|span| span.content.as_ref() == "indented" && span.style.bg == highlight.bg));
        assert!(lines[0].spans.iter().any(|span| span.content.as_ref() == "    " && span.style.bg != highlight.bg));
    }

    #[test]
    fn render_code_pane_layers_selection_after_remote_highlight() {
        let mut tab = tab_with(vec![line(LineKind::Unchanged, Some(1), Some(1), "hello world")]);
        let highlight = Style::default().bg(Color::Rgb(1, 2, 3));
        let lines = render_code_pane(&mut tab, 1, CodePaneOverlays {
            selection: Some(Selection { anchor: TextPoint { line: 0, column: 0 }, focus: TextPoint { line: 0, column: 5 } }),
            remote_highlight: Some(RemoteHighlightOverlay { start_line: 0, end_line: 0, style: highlight }),
            ..CodePaneOverlays::default()
        });
        assert!(lines[0].spans.iter().any(|span| span.content.as_ref() == "hello" && span.style.bg == selection_style().bg));
        assert!(lines[0].spans.iter().any(|span| span.content.as_ref() == " world" && span.style.bg == highlight.bg));
    }

    #[test]
    fn render_code_pane_uses_static_viewport_cache_without_overlays() {
        let mut tab = tab_with(vec![
            line(LineKind::Unchanged, Some(1), Some(1), "one"),
            line(LineKind::Unchanged, Some(2), Some(2), "two"),
            line(LineKind::Unchanged, Some(3), Some(3), "three"),
        ]);
        let first = render_code_pane(&mut tab, 2, CodePaneOverlays::default());
        assert_eq!(first.iter().map(text).collect::<Vec<_>>(), vec!["   1   │ one", "   2   │ two"]);
        assert!(matches!(tab.viewport_cache, Some(ViewportCache { scroll: 0, height: 2, .. })));

        tab.scroll = 1;
        let second = render_code_pane(&mut tab, 2, CodePaneOverlays::default());
        assert_eq!(second.iter().map(text).collect::<Vec<_>>(), vec!["   2   │ two", "   3   │ three"]);
        assert!(matches!(tab.viewport_cache, Some(ViewportCache { scroll: 1, height: 2, .. })));
    }
}
