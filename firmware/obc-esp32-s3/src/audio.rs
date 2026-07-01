//! I2S microphone driver — replaces the `audio_sample` RMS placeholder.
//!
//! Samples an I2S MEMS microphone (INMP441 / SPH0645 — 24-bit data in 32-bit
//! slots) and returns a normalised RMS level in `0.0..=1.0`, the same shape the
//! stub returned, so the ClawCam audio-ingest path and any loudness reflex get a
//! real signal. Falls back to the stub when no mic initialised.
//!
//! The RMS math (read 32-bit slots → take the 24-bit sample → mean-square → root
//! → normalise) is straightforward and correct; the **I2S driver construction is
//! the part most likely to need an API tweak** for your exact `esp-idf-hal`
//! version — verify against the compiler and `BRINGUP.md`.
//!
//! **Untested on metal by its author.**

use anyhow::Context;
use esp_idf_svc::hal::delay::BLOCK;
use esp_idf_svc::hal::i2s::{I2sDriver, I2sRx};

/// Sample rate — a loudness estimate doesn't need speech bandwidth.
pub const SAMPLE_RATE_HZ: u32 = 16_000;
/// I2S slot width in bytes (INMP441/SPH0645 present 24-bit data in a 32-bit slot).
const BYTES_PER_SAMPLE: usize = 4;
/// 24-bit full scale (2^23), for normalising RMS into 0.0..=1.0.
const FULL_SCALE_24BIT: f64 = 8_388_608.0;

/// Owns the I2S RX driver and samples loudness.
pub struct AudioMic {
    i2s: I2sDriver<'static, I2sRx>,
}

impl AudioMic {
    pub fn new(i2s: I2sDriver<'static, I2sRx>) -> Self {
        Self { i2s }
    }

    /// Sample ~`duration_ms` of audio and return the normalised RMS level
    /// (`0.0..=1.0`). A quiet room reads near zero; louder sound approaches 1.
    pub fn rms(&mut self, duration_ms: u64) -> anyhow::Result<f64> {
        // Enable the RX channel (idempotent; ignore an already-enabled error).
        let _ = self.i2s.rx_enable();

        let target = (SAMPLE_RATE_HZ as u64 * duration_ms / 1000).max(1);
        let mut buf = [0u8; 512];
        let mut sum_sq = 0.0f64;
        let mut n: u64 = 0;

        while n < target {
            let read = self.i2s.read(&mut buf, BLOCK).context("I2S read")?;
            if read == 0 {
                break;
            }
            for slot in buf[..read].chunks_exact(BYTES_PER_SAMPLE) {
                let raw = i32::from_le_bytes([slot[0], slot[1], slot[2], slot[3]]);
                // The 24-bit sample sits in the high bits of the 32-bit slot.
                let sample = (raw >> 8) as f64;
                sum_sq += sample * sample;
                n += 1;
            }
        }

        if n == 0 {
            anyhow::bail!("no audio samples read");
        }
        let rms = (sum_sq / n as f64).sqrt();
        Ok((rms / FULL_SCALE_24BIT).clamp(0.0, 1.0))
    }
}
