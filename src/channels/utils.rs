//! Shared utilities for Oh-Ben-Claw communication channel adapters.

/// Split `text` into chunks of at most `max` bytes, respecting UTF-8 character
/// boundaries to avoid splitting multi-byte code points.
///
/// Used to comply with platform-specific message length limits (e.g. Telegram
/// 4096 chars, Discord 2000 chars, Slack 40 000 chars).
pub(super) fn chunk_text(text: &str, max: usize) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let candidate = (start + max).min(text.len());
        // Walk back to the nearest valid UTF-8 char boundary.
        let end = (0..=candidate)
            .rev()
            .find(|&i| text.is_char_boundary(i))
            .unwrap_or(start);
        if end == start {
            break;
        }
        chunks.push(&text[start..end]);
        start = end;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_text_short_string() {
        assert_eq!(chunk_text("hello world", 4096), vec!["hello world"]);
    }

    #[test]
    fn chunk_text_splits_at_boundary() {
        let long = "a".repeat(5000);
        let chunks = chunk_text(&long, 4000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 4000);
        assert_eq!(chunks[1].len(), 1000);
    }

    #[test]
    fn chunk_text_empty_input() {
        assert!(chunk_text("", 4000).is_empty());
    }

    #[test]
    fn chunk_text_respects_multibyte() {
        // Each '©' is 2 UTF-8 bytes.  A chunk of 3 bytes must not cut inside it.
        let text = "©©©©©"; // 10 bytes
        let chunks = chunk_text(text, 3);
        for chunk in &chunks {
            // Every chunk must be valid UTF-8 (i.e. no panic when accessed).
            assert!(!chunk.is_empty());
        }
        let reassembled: String = chunks.concat();
        assert_eq!(reassembled, text);
    }
}
