use crate::read_model::AssetView;

pub fn truncate(value: &str, max: usize) -> String {
    let mut characters = value.chars();
    let truncated: String = characters.by_ref().take(max).collect();
    if characters.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

pub fn first_line(value: &str, max: usize) -> String {
    truncate(value.lines().next().unwrap_or_default(), max)
}

pub fn wrap_content(content: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    for paragraph in content.lines() {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut start = 0;
        let chars: Vec<char> = paragraph.chars().collect();
        while start < chars.len() {
            let end = (start + width).min(chars.len());
            let mut slice_end = end;
            if end < chars.len() {
                while slice_end > start && !chars[slice_end - 1].is_whitespace() {
                    slice_end -= 1;
                }
                if slice_end == start {
                    slice_end = end;
                }
            }
            lines.push(chars[start..slice_end].iter().collect());
            start = slice_end;
            while start < chars.len() && chars[start].is_whitespace() {
                start += 1;
            }
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

pub fn parse_ic_query(query: &str) -> Option<i64> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    if let Some(value) = query
        .strip_prefix("[IC:")
        .and_then(|value| value.strip_suffix(']'))
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
    {
        return Some(value);
    }
    if let Some(value) = query
        .strip_prefix("IC:")
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
    {
        return Some(value);
    }
    if query.chars().all(|ch| ch.is_ascii_digit()) {
        return query.parse::<i64>().ok().filter(|value| *value > 0);
    }
    None
}

pub fn asset_status_label(asset: &AssetView) -> &'static str {
    if asset.exists_locally {
        "✓ local"
    } else if asset.relative_path.is_some() {
        "× missing"
    } else {
        "– not in export"
    }
}

#[cfg(test)]
mod tests {
    use super::{first_line, parse_ic_query, truncate, wrap_content};
    use crate::read_model::AssetView;

    #[test]
    fn truncates_on_character_boundaries() {
        assert_eq!(truncate("A🦀B", 2), "A🦀…");
        assert_eq!(first_line("one\ntwo", 10), "one");
    }

    #[test]
    fn wraps_long_lines_and_preserves_paragraphs() {
        let wrapped = wrap_content("hello world again", 5);
        assert_eq!(wrapped, vec!["hello", "world", "again"]);
        let multiline = wrap_content("line one\nline two", 20);
        assert_eq!(multiline, vec!["line one", "line two"]);
    }

    #[test]
    fn parse_ic_query_accepts_common_forms() {
        assert_eq!(parse_ic_query("42"), Some(42));
        assert_eq!(parse_ic_query("IC:42"), Some(42));
        assert_eq!(parse_ic_query("[IC:42]"), Some(42));
        assert_eq!(parse_ic_query("hello"), None);
        assert_eq!(parse_ic_query("0"), None);
    }

    #[test]
    fn asset_status_labels_are_distinct() {
        let local = AssetView {
            id: "1".into(),
            name: "a".into(),
            mime_type: None,
            exists_locally: true,
            relative_path: Some("assets/a".into()),
        };
        let missing = AssetView {
            id: "2".into(),
            name: "b".into(),
            mime_type: None,
            exists_locally: false,
            relative_path: Some("assets/b".into()),
        };
        let absent = AssetView {
            id: "3".into(),
            name: "c".into(),
            mime_type: None,
            exists_locally: false,
            relative_path: None,
        };
        assert_eq!(super::asset_status_label(&local), "✓ local");
        assert_eq!(super::asset_status_label(&missing), "× missing");
        assert_eq!(super::asset_status_label(&absent), "– not in export");
    }
}
