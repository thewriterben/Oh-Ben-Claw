//! Enhanced multimodal message handling.
//!
//! This module provides utilities for detecting and parsing image markers
//! embedded in text messages, fetching and validating image data, and types
//! for configuring multimodal behaviour.

use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as B64, Engine};
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

/// Represents the origin of an image — either a local file or a remote URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSource {
    Local(PathBuf),
    Remote(String),
}

/// Holds fetched and validated image data ready for inclusion in a prompt.
#[derive(Debug, Clone)]
pub struct ImageData {
    pub source: ImageSource,
    pub mime_type: String,
    pub data: Vec<u8>,
    pub base64: String,
}

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

/// Determine whether a path string refers to a remote URL or a local file.
pub fn resolve_image_source(path: &str) -> ImageSource {
    if path.starts_with("http://") || path.starts_with("https://") {
        ImageSource::Remote(path.to_string())
    } else {
        ImageSource::Local(PathBuf::from(path))
    }
}

/// Validate that the given MIME type is in [`ALLOWED_IMAGE_MIME_TYPES`].
pub fn validate_mime_type(mime: &str) -> Result<(), MultimodalError> {
    if ALLOWED_IMAGE_MIME_TYPES.contains(&mime) {
        Ok(())
    } else {
        Err(MultimodalError::UnsupportedMime {
            mime: mime.to_string(),
        })
    }
}

/// Validate that an image's byte size does not exceed `max_bytes`.
pub fn validate_image_size(size: usize, max_bytes: usize) -> Result<(), MultimodalError> {
    if size > max_bytes {
        Err(MultimodalError::ImageTooLarge {
            size,
            max: max_bytes,
        })
    } else {
        Ok(())
    }
}

/// Infer a MIME type from a file extension. Returns `None` for unknown extensions.
fn mime_from_extension(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("webp") => Some("image/webp"),
        Some("gif") => Some("image/gif"),
        Some("bmp") => Some("image/bmp"),
        _ => None,
    }
}

/// Read a local image file, infer its MIME type, and validate it.
pub fn fetch_local_image(path: &Path) -> Result<ImageData, MultimodalError> {
    let mime = mime_from_extension(path).ok_or_else(|| MultimodalError::UnsupportedMime {
        mime: path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("unknown")
            .to_string(),
    })?;

    validate_mime_type(mime)?;

    let data = std::fs::read(path).map_err(|e| MultimodalError::LocalReadFailed {
        reason: e.to_string(),
    })?;

    let encoded = B64.encode(&data);

    Ok(ImageData {
        source: ImageSource::Local(path.to_path_buf()),
        mime_type: mime.to_string(),
        data,
        base64: encoded,
    })
}

/// Fetch a remote image by URL.
///
/// Currently returns [`MultimodalError::RemoteFetchDisabled`] because full
/// HTTP fetching is handled at a higher layer. This keeps the module free of
/// async runtime concerns.
pub fn fetch_remote_image(_url: &str) -> Result<ImageData, MultimodalError> {
    Err(MultimodalError::RemoteFetchDisabled)
}

/// Resolve, fetch, and validate a batch of image references.
///
/// Enforces the maximum image count (`max_images`) and the per-image byte
/// limit (`max_bytes`). Each reference is resolved via
/// [`resolve_image_source`], fetched with the appropriate local/remote
/// handler, and then size-validated.
pub fn prepare_images(
    refs: &[String],
    max_images: usize,
    max_bytes: usize,
) -> Result<Vec<ImageData>, MultimodalError> {
    if refs.len() > max_images {
        return Err(MultimodalError::TooManyImages {
            count: refs.len(),
            max: max_images,
        });
    }

    let mut images = Vec::with_capacity(refs.len());
    for r in refs {
        let source = resolve_image_source(r);
        let img = match &source {
            ImageSource::Local(p) => fetch_local_image(p)?,
            ImageSource::Remote(url) => fetch_remote_image(url)?,
        };
        validate_image_size(img.data.len(), max_bytes)?;
        images.push(img);
    }
    Ok(images)
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

    // ── resolve_image_source ──────────────────────────────────────────────────

    #[test]
    fn resolve_http_url() {
        let src = resolve_image_source("http://example.com/img.png");
        assert_eq!(
            src,
            ImageSource::Remote("http://example.com/img.png".into())
        );
    }

    #[test]
    fn resolve_https_url() {
        let src = resolve_image_source("https://cdn.example.com/photo.jpg");
        assert_eq!(
            src,
            ImageSource::Remote("https://cdn.example.com/photo.jpg".into())
        );
    }

    #[test]
    fn resolve_local_path() {
        let src = resolve_image_source("/home/user/photo.png");
        assert_eq!(
            src,
            ImageSource::Local(std::path::PathBuf::from("/home/user/photo.png"))
        );
    }

    #[test]
    fn resolve_relative_path() {
        let src = resolve_image_source("images/cat.jpg");
        assert_eq!(
            src,
            ImageSource::Local(std::path::PathBuf::from("images/cat.jpg"))
        );
    }

    // ── validate_mime_type ────────────────────────────────────────────────────

    #[test]
    fn valid_mime_types_accepted() {
        for mime in ALLOWED_IMAGE_MIME_TYPES {
            assert!(
                validate_mime_type(mime).is_ok(),
                "expected {mime} to be valid"
            );
        }
    }

    #[test]
    fn invalid_mime_type_rejected() {
        let err = validate_mime_type("application/pdf").unwrap_err();
        match err {
            MultimodalError::UnsupportedMime { mime } => {
                assert_eq!(mime, "application/pdf");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    // ── validate_image_size ──────────────────────────────────────────────────

    #[test]
    fn size_within_limit_ok() {
        assert!(validate_image_size(1024, 2048).is_ok());
    }

    #[test]
    fn size_at_limit_ok() {
        assert!(validate_image_size(2048, 2048).is_ok());
    }

    #[test]
    fn size_exceeding_limit_rejected() {
        let err = validate_image_size(3000, 2048).unwrap_err();
        match err {
            MultimodalError::ImageTooLarge { size, max } => {
                assert_eq!(size, 3000);
                assert_eq!(max, 2048);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    // ── fetch_local_image ────────────────────────────────────────────────────

    #[test]
    fn fetch_local_image_reads_file() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let img_path = dir.path().join("test.png");
        let payload = b"\x89PNG\r\n\x1a\nfake image data";
        std::fs::write(&img_path, payload).unwrap();

        let img = fetch_local_image(&img_path).expect("should succeed");
        assert_eq!(img.mime_type, "image/png");
        assert_eq!(img.data, payload);
        assert_eq!(img.base64, B64.encode(payload));
        assert_eq!(img.source, ImageSource::Local(img_path));
    }

    #[test]
    fn fetch_local_image_unknown_ext_fails() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let img_path = dir.path().join("test.xyz");
        std::fs::write(&img_path, b"data").unwrap();

        let err = fetch_local_image(&img_path).unwrap_err();
        matches!(err, MultimodalError::UnsupportedMime { .. });
    }

    #[test]
    fn fetch_local_image_missing_file_fails() {
        let err = fetch_local_image(std::path::Path::new("/nonexistent/image.png")).unwrap_err();
        matches!(err, MultimodalError::LocalReadFailed { .. });
    }

    // ── fetch_remote_image ───────────────────────────────────────────────────

    #[test]
    fn fetch_remote_image_returns_disabled() {
        let err = fetch_remote_image("https://example.com/img.png").unwrap_err();
        matches!(err, MultimodalError::RemoteFetchDisabled);
    }

    // ── prepare_images ───────────────────────────────────────────────────────

    #[test]
    fn prepare_images_too_many() {
        let refs: Vec<String> = (0..4).map(|i| format!("img{i}.png")).collect();
        let err = prepare_images(&refs, 3, 1024 * 1024).unwrap_err();
        match err {
            MultimodalError::TooManyImages { count, max } => {
                assert_eq!(count, 4);
                assert_eq!(max, 3);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn prepare_images_success() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let p1 = dir.path().join("a.png");
        let p2 = dir.path().join("b.jpeg");
        std::fs::write(&p1, b"png-data").unwrap();
        std::fs::write(&p2, b"jpeg-data").unwrap();

        let refs = vec![
            p1.to_string_lossy().to_string(),
            p2.to_string_lossy().to_string(),
        ];
        let images = prepare_images(&refs, 5, 1024 * 1024).expect("should succeed");
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].mime_type, "image/png");
        assert_eq!(images[1].mime_type, "image/jpeg");
    }

    #[test]
    fn prepare_images_size_exceeded() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let p = dir.path().join("big.png");
        std::fs::write(&p, vec![0u8; 2000]).unwrap();

        let refs = vec![p.to_string_lossy().to_string()];
        let err = prepare_images(&refs, 5, 1000).unwrap_err();
        matches!(err, MultimodalError::ImageTooLarge { .. });
    }
}
