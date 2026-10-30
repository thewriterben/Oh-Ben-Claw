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

/// Whether `sender` may talk to the agent through `channel`. An empty list
/// refuses everyone (fail closed); refusals are logged so an operator who
/// forgot to fill the list can see why the bot is silent.
pub(super) fn sender_allowed(channel: &str, allowed: &[String], sender: &str) -> bool {
    if allowed.iter().any(|a| a == sender) {
        return true;
    }
    if allowed.is_empty() {
        tracing::warn!(
            channel,
            sender,
            "message ignored: channels.{channel}.allowed_users is empty, which refuses everyone"
        );
    } else {
        tracing::info!(
            channel,
            sender,
            "message ignored: sender not in allowed_users"
        );
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_allowlist_refuses_everyone() {
        assert!(!sender_allowed("discord", &[], "123"));
        assert!(!sender_allowed("discord", &[], ""));
    }

    #[test]
    fn allowlist_admits_exact_matches_only() {
        let allowed = vec!["@ben:matrix.org".to_string()];
        assert!(sender_allowed("matrix", &allowed, "@ben:matrix.org"));
        assert!(!sender_allowed("matrix", &allowed, "@ben:evil.org"));
        assert!(!sender_allowed("matrix", &allowed, "@BEN:matrix.org"));
    }

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
