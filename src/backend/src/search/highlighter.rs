//! Text highlighting for search results

/// Highlight search matches in text.
pub fn highlight_matches(
    text: &str,
    query: &str,
    highlight_start: &str,
    highlight_end: &str,
) -> String {
    let haystack = text.to_ascii_lowercase();
    let mut matches = Vec::<(usize, usize)>::new();

    for term in query.split_whitespace() {
        let needle = term.to_ascii_lowercase();
        if needle.is_empty() {
            continue;
        }

        let mut offset = 0;
        while let Some(relative) = haystack[offset..].find(&needle) {
            let start = offset + relative;
            let end = start + needle.len();
            matches.push((start, end));
            offset = end;
        }
    }

    if matches.is_empty() {
        return text.to_string();
    }

    // Fold overlapping or adjacent hits so a marker pair never nests.
    matches.sort_unstable();
    let mut merged = Vec::<(usize, usize)>::with_capacity(matches.len());
    for (start, end) in matches {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }

    let mut result = String::with_capacity(text.len() + query.len() * 2);
    let mut last_end = 0;
    for (start, end) in merged {
        result.push_str(&text[last_end..start]);
        result.push_str(highlight_start);
        result.push_str(&text[start..end]);
        result.push_str(highlight_end);
        last_end = end;
    }
    result.push_str(&text[last_end..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highlight_matches() {
        let text = "Hello world, Rust is great!";
        let result = highlight_matches(text, "rust great", "<mark>", "</mark>");
        assert!(result.contains("<mark>Rust</mark>"));
        assert!(result.contains("<mark>great</mark>"));
    }

    #[test]
    fn test_highlight_empty_query() {
        let text = "Hello world";
        let result = highlight_matches(text, "", "<mark>", "</mark>");
        assert_eq!(result, text);
    }

    #[test]
    fn test_highlight_merges_overlapping_terms() {
        let result = highlight_matches("rustlang", "rust rustlang", "<mark>", "</mark>");
        assert_eq!(result, "<mark>rustlang</mark>");
    }
}
