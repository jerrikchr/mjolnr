/// Bound UTF-8 output without slicing in the middle of a code point.
#[must_use]
pub(crate) fn truncate(text: String, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }

    let mut end = max_bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    let mut bounded = text.get(..end).unwrap_or_default().to_owned();
    bounded.push_str("\n[… output truncated …]");
    (bounded, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_preserves_utf8_and_discloses_itself() {
        let (text, truncated) = truncate("ééé".to_owned(), 3);
        assert!(truncated);
        assert!(text.starts_with('é'));
        assert!(text.contains("truncated"));
    }
}
