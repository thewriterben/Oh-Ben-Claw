//! Shared by the fixture tests: where the camera-bench fixture is, and how
//! its JPEGs become the greyscale the thresholds were measured on.

use std::path::{Path, PathBuf};

pub fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/vision-bench-2026-09-16")
}

/// Every frame of one labelled set, in capture order, frame 000 included.
pub fn frame_files(label: &str) -> Vec<PathBuf> {
    let dir = fixture().join(label);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "jpg"))
        .collect();
    files.sort();
    files
}

/// A fixture JPEG as grey, through the library's own decoder.
pub fn grey(path: &Path) -> (Vec<u8>, usize, usize) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let f = obc_vision::jpeg::decode_grey(&bytes)
        .unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
    (f.pixels, f.width, f.height)
}
