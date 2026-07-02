// In-file text search (vim-style `/`).

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub line: usize,
    pub column: usize,
    pub end: usize,
}

#[derive(Clone, Debug)]
pub struct SearchQuery {
    pub text: String,
    pub case_sensitive: bool,
}

impl SearchQuery {
    pub fn new(text: &str) -> Self {
        Self { text: text.to_string(), case_sensitive: text.chars().any(|c| c.is_uppercase()) }
    }

    pub fn is_empty(&self) -> bool { self.text.is_empty() }

    pub fn find<'a>(&self, lines: impl IntoIterator<Item = (usize, &'a str)>) -> Vec<Match> {
        if self.is_empty() { return Vec::new(); }
        let needle = if self.case_sensitive { self.text.clone() } else { self.text.to_lowercase() };
        let mut out = Vec::new();
        for (line, text) in lines {
            let haystack: Vec<char> = text.chars().collect();
            let cmp: Vec<char> = if self.case_sensitive { haystack.clone() } else { haystack.iter().map(|c| c.to_ascii_lowercase()).collect() };
            let needle: Vec<char> = needle.chars().collect();
            if needle.is_empty() { continue; }
            let mut i = 0;
            while i + needle.len() <= cmp.len() {
                if cmp[i..i + needle.len()] == needle[..] {
                    out.push(Match { line, column: i, end: i + needle.len() });
                    i += needle.len();
                } else {
                    i += 1;
                }
            }
        }
        out
    }
}

/// Index of the next match strictly after `from` (line-major), wrapping around.
pub fn next_match(matches: &[Match], from: Option<usize>) -> Option<usize> {
    if matches.is_empty() { return None; }
    match from {
        None => Some(0),
        Some(i) => Some((i + 1) % matches.len()),
    }
}

/// Index of the previous match strictly before `from` (line-major), wrapping around.
pub fn prev_match(matches: &[Match], from: Option<usize>) -> Option<usize> {
    if matches.is_empty() { return None; }
    match from {
        None => Some(matches.len() - 1),
        Some(i) => Some(if i == 0 { matches.len() - 1 } else { i - 1 }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_all<'a>(q: &SearchQuery, lines: &'a [&'a str]) -> Vec<Match> {
        q.find(lines.iter().enumerate().map(|(i, s)| (i, *s)))
    }

    #[test]
    fn empty_query_matches_nothing() {
        let q = SearchQuery::new("");
        assert!(find_all(&q, &["foo", "bar"]).is_empty());
    }

    #[test]
    fn lowercase_query_is_case_insensitive() {
        let q = SearchQuery::new("foo");
        assert!(!q.case_sensitive);
        let m = find_all(&q, &["Foo bar", "baz FOO"]);
        assert_eq!(m, vec![
            Match { line: 0, column: 0, end: 3 },
            Match { line: 1, column: 4, end: 7 },
        ]);
    }

    #[test]
    fn uppercase_in_query_is_case_sensitive() {
        let q = SearchQuery::new("Foo");
        assert!(q.case_sensitive);
        let m = find_all(&q, &["Foo bar", "foo FOO"]);
        assert_eq!(m, vec![Match { line: 0, column: 0, end: 3 }]);
    }

    #[test]
    fn matches_do_not_overlap() {
        let q = SearchQuery::new("aa");
        // "aaaa" -> matches at 0 and 2, not 1 and 3.
        let m = find_all(&q, &["aaaa"]);
        assert_eq!(m, vec![
            Match { line: 0, column: 0, end: 2 },
            Match { line: 0, column: 2, end: 4 },
        ]);
    }

    #[test]
    fn next_match_wraps_around() {
        let matches = vec![
            Match { line: 0, column: 0, end: 1 },
            Match { line: 2, column: 3, end: 4 },
        ];
        assert_eq!(next_match(&matches, None), Some(0));
        assert_eq!(next_match(&matches, Some(0)), Some(1));
        assert_eq!(next_match(&matches, Some(1)), Some(0));
    }

    #[test]
    fn prev_match_wraps_around() {
        let matches = vec![
            Match { line: 0, column: 0, end: 1 },
            Match { line: 2, column: 3, end: 4 },
        ];
        assert_eq!(prev_match(&matches, None), Some(1));
        assert_eq!(prev_match(&matches, Some(0)), Some(1));
        assert_eq!(prev_match(&matches, Some(1)), Some(0));
    }
}