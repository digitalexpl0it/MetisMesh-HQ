//! Mesh text helpers (mentions). Plain text only — never shell/HTML.

/// True when `haystack` mentions `name` as `@[name]` or `@name` (MeshCore-style).
/// Comparison is case-insensitive; empty `name` never matches.
pub fn text_mentions_name(haystack: &str, name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() {
        return false;
    }
    let needle = name.to_ascii_lowercase();
    let chars: Vec<char> = haystack.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] != '@' {
            i += 1;
            continue;
        }
        if i + 1 < chars.len() && chars[i + 1] == '[' {
            if let Some(end) = chars[i + 2..].iter().position(|ch| *ch == ']') {
                let found: String = chars[i + 2..i + 2 + end].iter().collect();
                if found.trim().eq_ignore_ascii_case(&needle) {
                    return true;
                }
                i += 3 + end;
                continue;
            }
        } else if i + 1 < chars.len() {
            let mut j = i + 1;
            while j < chars.len()
                && (chars[j].is_ascii_alphanumeric() || matches!(chars[j], '_' | '-' | '.'))
            {
                j += 1;
            }
            if j > i + 1 {
                let found: String = chars[i + 1..j].iter().collect();
                if found.eq_ignore_ascii_case(&needle) {
                    return true;
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracket_and_bare_mentions() {
        assert!(text_mentions_name("hi @[Metis HQ] there", "Metis HQ"));
        assert!(text_mentions_name("@metis-hq please", "Metis-HQ"));
        assert!(!text_mentions_name("hi @[Other]", "Metis HQ"));
        assert!(!text_mentions_name("@metis", "Metis HQ"));
        assert!(!text_mentions_name("anything", ""));
    }
}
