/// 将文本截断到 max_words 个词，超出时在词边界截断并追加 "..."
pub fn cap_to_words(text: &str, max_words: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() <= max_words {
        return text.to_string();
    }
    let truncated = words[..max_words].join(" ");
    format!("{truncated}...")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cap_short_text_unchanged() {
        let text = "hello world";
        assert_eq!(cap_to_words(text, 100), "hello world");
    }

    #[test]
    fn test_cap_truncates_at_word_boundary() {
        let words: Vec<&str> = (0..50).map(|_| "word").collect();
        let text = words.join(" ");
        let result = cap_to_words(&text, 10);
        let result_word_count = result.split_whitespace().count();
        assert!(result_word_count <= 10);
    }

    #[test]
    fn test_cap_empty_string() {
        assert_eq!(cap_to_words("", 100), "");
    }

    #[test]
    fn test_cap_adds_ellipsis_when_truncated() {
        let text = (0..100).map(|i| format!("word{i}")).collect::<Vec<_>>().join(" ");
        let result = cap_to_words(&text, 10);
        assert!(result.ends_with("..."));
    }
}
