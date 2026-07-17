use super::{SearchSnippet, SnippetSegment};

/// Marker character used by SQLite `snippet` for start of match
pub const MATCH_START_MARKER: char = '\x01';
/// Marker character used by SQLite `snippet` for end of match
pub const MATCH_END_MARKER: char = '\x02';

pub fn parse_snippet(raw_snippet: &str) -> SearchSnippet {
    let mut segments = Vec::new();
    let mut plain_text = String::with_capacity(raw_snippet.len());

    let mut current_segment = String::new();
    let mut in_match = false;

    for c in raw_snippet.chars() {
        match c {
            MATCH_START_MARKER => {
                if !current_segment.is_empty() {
                    segments.push(SnippetSegment {
                        text: current_segment.clone(),
                        highlighted: in_match,
                    });
                    plain_text.push_str(&current_segment);
                    current_segment.clear();
                }
                in_match = true;
            }
            MATCH_END_MARKER => {
                if !current_segment.is_empty() {
                    segments.push(SnippetSegment {
                        text: current_segment.clone(),
                        highlighted: in_match,
                    });
                    plain_text.push_str(&current_segment);
                    current_segment.clear();
                }
                in_match = false;
            }
            _ => {
                current_segment.push(c);
            }
        }
    }

    if !current_segment.is_empty() {
        segments.push(SnippetSegment {
            text: current_segment.clone(),
            highlighted: in_match,
        });
        plain_text.push_str(&current_segment);
    }

    SearchSnippet {
        text: plain_text,
        segments,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_snippet() {
        let raw = format!(
            "RecallEngine utilise désormais une recherche {}FTS5{}.",
            MATCH_START_MARKER, MATCH_END_MARKER
        );
        let snippet = parse_snippet(&raw);

        assert_eq!(
            snippet.text,
            "RecallEngine utilise désormais une recherche FTS5."
        );
        assert_eq!(snippet.segments.len(), 3);

        assert_eq!(
            snippet.segments[0].text,
            "RecallEngine utilise désormais une recherche "
        );
        assert!(!snippet.segments[0].highlighted);

        assert_eq!(snippet.segments[1].text, "FTS5");
        assert!(snippet.segments[1].highlighted);

        assert_eq!(snippet.segments[2].text, ".");
        assert!(!snippet.segments[2].highlighted);
    }

    #[test]
    fn test_parse_snippet_consecutive() {
        let raw = format!(
            "{}Hello{} {}world{}!",
            MATCH_START_MARKER, MATCH_END_MARKER, MATCH_START_MARKER, MATCH_END_MARKER
        );
        let snippet = parse_snippet(&raw);

        assert_eq!(snippet.text, "Hello world!");
        assert_eq!(snippet.segments.len(), 4);
        assert!(snippet.segments[0].highlighted);
        assert_eq!(snippet.segments[0].text, "Hello");
        assert!(!snippet.segments[1].highlighted);
        assert_eq!(snippet.segments[1].text, " ");
        assert!(snippet.segments[2].highlighted);
        assert_eq!(snippet.segments[2].text, "world");
        assert!(!snippet.segments[3].highlighted);
        assert_eq!(snippet.segments[3].text, "!");
    }
}
