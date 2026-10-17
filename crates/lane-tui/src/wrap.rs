//! Word wrap that keeps an indent on every line, for hints under a check.

use unicode_width::UnicodeWidthStr;

/// Wrap `text` to `width` columns. Every line starts with `indent` spaces.
pub fn wrap(text: &str, width: usize, indent: usize) -> Vec<String> {
    let avail = width.saturating_sub(indent).max(10);
    let pad = " ".repeat(indent);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let needed = if line.is_empty() {
            word.width()
        } else {
            line.width() + 1 + word.width()
        };
        if needed > avail && !line.is_empty() {
            lines.push(format!("{pad}{line}"));
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(format!("{pad}{line}"));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_with_indent() {
        let lines = wrap("one two three four five six", 14, 4);
        assert!(lines.iter().all(|l| l.starts_with("    ")));
        assert!(lines.iter().all(|l| l.width() <= 14), "{lines:?}");
        assert_eq!(lines.join(" ").split_whitespace().count(), 6);
    }

    #[test]
    fn long_words_stay_whole() {
        assert_eq!(wrap("supercalifragilistic", 8, 0), ["supercalifragilistic"]);
    }
}
