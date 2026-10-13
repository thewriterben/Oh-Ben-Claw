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

/// Decode a JPEG to 8-bit greyscale exactly as Pillow's `convert("L")` does
/// from RGB: `(R*19595 + G*38470 + B*7471 + 0x8000) >> 16`.
pub fn grey(path: &Path) -> (Vec<u8>, usize, usize) {
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;
    use zune_jpeg::JpegDecoder;

    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut dec = JpegDecoder::new_with_options(std::io::Cursor::new(bytes), opts);
    let rgb = dec
        .decode()
        .unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
    let info = dec.info().expect("headers decoded");
    let (w, h) = (usize::from(info.width), usize::from(info.height));
    assert_eq!(rgb.len(), w * h * 3, "{}", path.display());
    let luma = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .map(|&[r, g, b]| {
            let (r, g, b) = (u32::from(r), u32::from(g), u32::from(b));
            ((r * 19595 + g * 38470 + b * 7471 + 0x8000) >> 16) as u8
        })
        .collect();
    (luma, w, h)
}
