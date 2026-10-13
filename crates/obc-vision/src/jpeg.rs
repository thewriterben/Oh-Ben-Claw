//! JPEG to the greyscale the motion thresholds were measured on.
//!
//! `scripts/vision/classify.py` measured every threshold on Pillow's
//! `convert("L")`: decode to RGB, then ITU-R 601 luma in 16-bit fixed point.
//! This is the same conversion over zune-jpeg's RGB. The two decoders differ
//! by up to 0.0002 in `frac` and 0.0017 in `edge` on the bench fixture, and
//! one recorded frame sits closer than that to a threshold
//! (`tests/motion_fixture.rs` has the whole story).

use anyhow::{Context, Result};

/// A decoded frame: 8-bit grey, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreyFrame {
    pub pixels: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// Decode `jpeg` and convert it to grey exactly as Pillow's `convert("L")`
/// does from RGB: `(R*19595 + G*38470 + B*7471 + 0x8000) >> 16`.
pub fn decode_grey(jpeg: &[u8]) -> Result<GreyFrame> {
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;
    use zune_jpeg::JpegDecoder;

    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut dec = JpegDecoder::new_with_options(std::io::Cursor::new(jpeg), opts);
    let rgb = dec
        .decode()
        .map_err(|e| anyhow::anyhow!("{e:?}"))
        .context("JPEG decode failed")?;
    let info = dec.info().context("JPEG decoded without headers")?;
    let (width, height) = (usize::from(info.width), usize::from(info.height));
    anyhow::ensure!(
        rgb.len() == width * height * 3,
        "JPEG decoded to {} bytes, expected {width}x{height}x3",
        rgb.len()
    );
    let pixels = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .map(|&[r, g, b]| {
            let (r, g, b) = (u32::from(r), u32::from(g), u32::from(b));
            ((r * 19595 + g * 38470 + b * 7471 + 0x8000) >> 16) as u8
        })
        .collect();
    Ok(GreyFrame {
        pixels,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_is_an_error_not_a_frame() {
        assert!(decode_grey(b"\xFF\xD8 not really").is_err());
        assert!(decode_grey(&[]).is_err());
    }
}
