use std::path::Path;

use anyhow::Result;
use ratatui::{style::{Color, Modifier, Style}, text::Span};
use tree_sitter::Language;
use tree_sitter_highlight::{Highlight, HighlightConfiguration, HighlightEvent, Highlighter as TreeSitterHighlighter};

const TREE_SITTER_HIGHLIGHTS: &[&str] = &[
    "attribute", "comment", "constant", "constant.builtin", "constructor", "embedded", "function", "function.builtin",
    "keyword", "module", "number", "operator", "property", "punctuation", "punctuation.bracket",
    "punctuation.delimiter", "string", "string.special", "tag", "type", "type.builtin", "variable",
    "variable.builtin", "variable.member",
];

struct LanguageConfig {
    extensions: &'static [&'static str],
    config: HighlightConfiguration,
}

impl LanguageConfig {
    fn new(extensions: &'static [&'static str], language_name: &'static str, language: Language, highlights_query: &str, injections_query: &str, locals_query: &str) -> Result<Self> {
        let mut config = HighlightConfiguration::new(language, language_name, highlights_query, injections_query, locals_query)?;
        config.configure(TREE_SITTER_HIGHLIGHTS);
        Ok(Self { extensions, config })
    }

    fn matches(&self, path: &Path) -> bool {
        path.extension().and_then(|ext| ext.to_str()).map(|ext| self.extensions.contains(&ext)).unwrap_or(false)
    }
}

pub struct Highlighter {
    configs: Vec<LanguageConfig>,
}

impl Highlighter {
    pub fn new() -> Result<Self> {
        Ok(Self {
            configs: vec![
                LanguageConfig::new(&["rs"], "rust", tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::HIGHLIGHTS_QUERY, tree_sitter_rust::INJECTIONS_QUERY, "")?,
                LanguageConfig::new(&["ts"], "typescript", tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), tree_sitter_typescript::HIGHLIGHTS_QUERY, "", tree_sitter_typescript::LOCALS_QUERY)?,
                LanguageConfig::new(&["tsx"], "tsx", tree_sitter_typescript::LANGUAGE_TSX.into(), tree_sitter_typescript::HIGHLIGHTS_QUERY, "", tree_sitter_typescript::LOCALS_QUERY)?,
                LanguageConfig::new(&["js"], "javascript", tree_sitter_javascript::LANGUAGE.into(), tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_javascript::INJECTIONS_QUERY, tree_sitter_javascript::LOCALS_QUERY)?,
                LanguageConfig::new(&["jsx"], "jsx", tree_sitter_javascript::LANGUAGE.into(), tree_sitter_javascript::JSX_HIGHLIGHT_QUERY, tree_sitter_javascript::INJECTIONS_QUERY, tree_sitter_javascript::LOCALS_QUERY)?,
            ],
        })
    }

    pub fn highlight_lines(&self, path: &Path, source: &str) -> Vec<Vec<Span<'static>>> {
        let Some(language) = self.configs.iter().find(|config| config.matches(path)) else { return plain_lines(source); };
        let mut highlighter = TreeSitterHighlighter::new();
        let Ok(events) = highlighter.highlight(&language.config, source.as_bytes(), None, |_| None) else { return plain_lines(source); };
        render_tree_sitter_lines(source, events)
    }
}

fn render_tree_sitter_lines(source: &str, events: impl Iterator<Item = std::result::Result<HighlightEvent, tree_sitter_highlight::Error>>) -> Vec<Vec<Span<'static>>> {
    let mut lines = vec![Vec::new()];
    let mut stack: Vec<usize> = Vec::new();
    for event in events {
        let Ok(event) = event else { return plain_lines(source); };
        match event {
            HighlightEvent::HighlightStart(Highlight(index)) => stack.push(index),
            HighlightEvent::HighlightEnd => { stack.pop(); }
            HighlightEvent::Source { start, end } => {
                let style = stack.last().copied().map(style_for_highlight).unwrap_or_else(default_code_style);
                push_highlighted_fragment(&mut lines, &source[start..end], style);
            }
        }
    }
    if source.ends_with('\n') && lines.last().is_some_and(|line| line.is_empty()) { lines.pop(); }
    lines
}

fn push_highlighted_fragment(lines: &mut Vec<Vec<Span<'static>>>, fragment: &str, style: Style) {
    for piece in fragment.split_inclusive('\n') {
        let text = piece.strip_suffix('\n').unwrap_or(piece);
        if !text.is_empty() { lines.last_mut().expect("at least one line").push(Span::styled(text.to_string(), style)); }
        if piece.ends_with('\n') { lines.push(Vec::new()); }
    }
}

pub fn plain_lines(source: &str) -> Vec<Vec<Span<'static>>> {
    source.lines().map(|line| vec![Span::styled(line.to_string(), default_code_style())]).collect()
}

pub fn default_code_style() -> Style {
    Style::default().fg(Color::Rgb(171, 178, 191))
}

fn style_for_highlight(index: usize) -> Style {
    let name = TREE_SITTER_HIGHLIGHTS.get(index).copied().unwrap_or("variable");
    match name {
        n if n.starts_with("comment") => Style::default().fg(Color::Rgb(92, 99, 112)).add_modifier(Modifier::ITALIC),
        n if n.starts_with("string") => Style::default().fg(Color::Rgb(152, 195, 121)),
        n if n.starts_with("keyword") => Style::default().fg(Color::Rgb(198, 120, 221)).add_modifier(Modifier::BOLD),
        n if n.starts_with("function") => Style::default().fg(Color::Rgb(97, 175, 239)),
        n if n.starts_with("type") || n.starts_with("constructor") => Style::default().fg(Color::Rgb(229, 192, 123)),
        n if n.starts_with("number") || n.starts_with("constant") => Style::default().fg(Color::Rgb(209, 154, 102)),
        n if n.starts_with("property") || n.starts_with("variable.member") => Style::default().fg(Color::Rgb(86, 182, 194)),
        n if n.starts_with("variable.builtin") || n.starts_with("function.builtin") => Style::default().fg(Color::Rgb(224, 108, 117)),
        n if n.starts_with("operator") || n.starts_with("punctuation") => Style::default().fg(Color::Rgb(171, 178, 191)),
        n if n.starts_with("attribute") || n.starts_with("tag") || n.starts_with("module") => Style::default().fg(Color::Rgb(97, 175, 239)),
        _ => default_code_style(),
    }
}
