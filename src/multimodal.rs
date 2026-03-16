//! Enhanced multimodal message handling.
//!
//! This module provides utilities for detecting and parsing image markers
//! embedded in text messages, and types for configuring multimodal behaviour.

use thiserror::Error;

/// Prefix used to identify image markers embedded in text.
pub const IMAGE_MARKER_PREFIX: &str = "[IMAGE:";

/// MIME types accepted for inline images.
pub const ALLOWED_IMAGE_MIME_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "image/bmp",
];

/// Error variants for multimodal operations.
#[derive(Debug, Error)]
pub enum MultimodalError {
    #[error("Too many images: {count} exceeds the maximum of {max}")]
    TooManyImages { count: usize, max: usize },

    #[error("Image too large: {size} bytes exceeds the maximum of {max} bytes")]
    ImageTooLarge { size: usize, max: usize },

    #[error("Unsupported MIME type: {mime}")]
    UnsupportedMime { mime: String },

    #[error("Remote image fetching is disabled")]
    RemoteFetchDisabled,

    #[error("Image source not found: {path}")]
    ImageSourceNotFound { path: String },

    #[error("Invalid image marker: {marker}")]
    InvalidMarker { marker: String },

    #[error("Failed to fetch remote image: {reason}")]
    RemoteFetchFailed { reason: String },

    #[error("Failed to read local image: {reason}")]
    LocalReadFailed { reason: String },
}

/// A prepared set of chat messages, tagged with whether any images are present.
#[derive(Debug, Clone)]
pub struct PreparedMessages {
    /// The prepared messages, ready to send to the LLM provider.
    pub messages: Vec<crate::providers::ChatMessage>,
    /// Whether any of the messages contain image data.
    pub contains_images: bool,
}

/// Parse `[IMAGE:path]` markers from a text string.
///
/// Returns the cleaned text (with all markers removed) and a list of image
/// references (paths or URLs) in the order they appeared.
///
/// # Example
///
/// ```
/// use oh_ben_claw::multimodal::parse_image_markers;
///
/// let (text, refs) = parse_image_markers("Hello [IMAGE:/tmp/photo.png] world");
/// assert_eq!(text.trim(), "Hello  world".trim());
/// assert_eq!(refs, vec!["/tmp/photo.png"]);
/// ```
pub fn parse_image_markers(content: &str) -> (String, Vec<String>) {
    let mut refs = Vec::new();
    let mut cleaned = String::with_capacity(content.len());
    let mut remaining = content;

    while let Some(start) = remaining.find(IMAGE_MARKER_PREFIX) {
        // Append everything before the marker
        cleaned.push_str(&remaining[..start]);

        let after_prefix = &remaining[start + IMAGE_MARKER_PREFIX.len()..];
        if let Some(end) = after_prefix.find(']') {
            let path = after_prefix[..end].trim().to_string();
            if !path.is_empty() {
                refs.push(path);
            }
            remaining = &after_prefix[end + 1..];
        } else {
            // Malformed marker — treat as plain text
            cleaned.push_str(IMAGE_MARKER_PREFIX);
            remaining = after_prefix;
        }
    }

    cleaned.push_str(remaining);
    (cleaned, refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_markers_returns_original() {
        let (text, refs) = parse_image_markers("Hello world");
        assert_eq!(text, "Hello world");
        assert!(refs.is_empty());
    }

    #[test]
    fn single_marker_extracted() {
        let (text, refs) = parse_image_markers("See [IMAGE:/tmp/photo.png] here");
        assert!(!text.contains(IMAGE_MARKER_PREFIX));
        assert_eq!(refs, vec!["/tmp/photo.png"]);
    }

    #[test]
    fn multiple_markers_extracted() {
        let input = "[IMAGE:a.png] and [IMAGE:b.png]";
        let (text, refs) = parse_image_markers(input);
        assert_eq!(refs, vec!["a.png", "b.png"]);
        assert!(!text.contains(IMAGE_MARKER_PREFIX));
    }

    #[test]
    fn malformed_marker_kept_as_text() {
        let (text, refs) = parse_image_markers("bad [IMAGE: no close bracket");
        assert!(refs.is_empty());
        assert!(text.contains(IMAGE_MARKER_PREFIX));
    }

    #[test]
    fn empty_marker_ignored() {
        let (text, refs) = parse_image_markers("hello [IMAGE:] world");
        assert!(refs.is_empty());
        assert!(!text.contains(IMAGE_MARKER_PREFIX));
    }
}
