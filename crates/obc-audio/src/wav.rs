//! WAV in and out, for the on-device speech path (plan P5).
//!
//! A Wyoming server takes and returns raw PCM with its format stated beside it;
//! `audio_transcribe` is handed a file and `text_to_speech` writes one. This is
//! the conversion between the two: RIFF/WAVE holding integer PCM, nothing else.
//! Compressed audio (MP3, Opus, M4A) is not decoded here. A file that is not a
//! PCM WAV goes to the cloud path, which decodes those itself.

use anyhow::{bail, Result};

/// Interleaved little-endian PCM and the format a Wyoming `audio-start` states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcm {
    /// Samples per second, per channel.
    pub rate: u32,
    /// Bytes per sample (2 for 16-bit).
    pub width: u16,
    pub channels: u16,
    pub data: Vec<u8>,
}

impl Pcm {
    /// Bytes in one frame: one sample for every channel.
    pub fn frame_bytes(&self) -> usize {
        usize::from(self.width) * usize::from(self.channels)
    }
}

/// `true` when `bytes` starts with a RIFF/WAVE header. Says nothing about
/// whether [`parse`] will take the rest.
pub fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Read integer PCM out of a WAV file. Refuses float, A-law, μ-law and
/// compressed formats, and a `data` chunk that comes before `fmt `.
///
/// A streaming writer may leave the `data` size at 0 or `0xFFFFFFFF`; what is
/// there is taken, trimmed to whole frames.
pub fn parse(bytes: &[u8]) -> Result<Pcm> {
    if !is_wav(bytes) {
        bail!("not a RIFF/WAVE file");
    }
    let mut format: Option<(u32, u16, u16)> = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32_at(bytes, at + 4) as usize;
        let body = at + 8;
        let end = body.saturating_add(size);
        match id {
            b"fmt " => {
                if size < 16 || end > bytes.len() {
                    bail!("truncated fmt chunk");
                }
                let b = &bytes[body..end];
                let tag = u16_at(b, 0);
                let channels = u16_at(b, 2);
                let rate = u32_at(b, 4);
                let bits = u16_at(b, 14);
                // WAVE_FORMAT_EXTENSIBLE names the real format in the first two
                // bytes of its sub-format GUID.
                let pcm = tag == 1 || (tag == 0xFFFE && size >= 26 && u16_at(b, 24) == 1);
                if !pcm {
                    bail!("WAV format tag {tag:#06x} is not integer PCM");
                }
                if bits == 0 || !bits.is_multiple_of(8) || bits > 32 {
                    bail!("{bits}-bit samples are not supported");
                }
                if channels == 0 || rate == 0 {
                    bail!("WAV header names {channels} channels at {rate} Hz");
                }
                format = Some((rate, bits / 8, channels));
            }
            b"data" => {
                let Some((rate, width, channels)) = format else {
                    bail!("data chunk before fmt chunk");
                };
                let frame = usize::from(width) * usize::from(channels);
                let available = end.min(bytes.len()) - body;
                let data = bytes[body..body + available / frame * frame].to_vec();
                return Ok(Pcm {
                    rate,
                    width,
                    channels,
                    data,
                });
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        at = end.saturating_add(size & 1);
    }
    bail!("no data chunk")
}

/// Write `pcm` as a canonical 44-byte-header WAV file.
pub fn encode(pcm: &Pcm) -> Result<Vec<u8>> {
    let Ok(data_len) = u32::try_from(pcm.data.len()) else {
        bail!(
            "{} bytes of audio is too long for one WAV file",
            pcm.data.len()
        );
    };
    let Some(riff_len) = data_len.checked_add(36) else {
        bail!("{data_len} bytes of audio is too long for one WAV file");
    };
    let block_align = pcm.width * pcm.channels;
    let byte_rate = pcm.rate * u32::from(block_align);
    let mut out = Vec::with_capacity(44 + pcm.data.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_len.to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&pcm.channels.to_le_bytes());
    out.extend_from_slice(&pcm.rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&(pcm.width * 8).to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&pcm.data);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mono16(samples: &[i16]) -> Pcm {
        Pcm {
            rate: 16_000,
            width: 2,
            channels: 1,
            data: samples.iter().flat_map(|s| s.to_le_bytes()).collect(),
        }
    }

    #[test]
    fn encode_then_parse_round_trips() {
        let pcm = mono16(&[0, 1, -1, i16::MAX, i16::MIN]);
        let bytes = encode(&pcm).unwrap();
        assert_eq!(bytes.len(), 44 + 10);
        assert!(is_wav(&bytes));
        assert_eq!(parse(&bytes).unwrap(), pcm);
    }

    #[test]
    fn parse_skips_unknown_chunks_and_their_padding() {
        let pcm = mono16(&[7, 8]);
        let plain = encode(&pcm).unwrap();
        // RIFF header + fmt, then an odd-sized LIST chunk (padded), then data.
        let mut bytes = plain[..36].to_vec();
        bytes.extend_from_slice(b"LIST");
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(b"abc\0");
        bytes.extend_from_slice(&plain[36..]);
        assert_eq!(parse(&bytes).unwrap(), pcm);
    }

    #[test]
    fn parse_takes_a_streamed_data_size_up_to_whole_frames() {
        let mut bytes = encode(&mono16(&[1, 2, 3])).unwrap();
        bytes[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes.push(0xAA); // half a frame on the end
        assert_eq!(parse(&bytes).unwrap().data.len(), 6);
    }

    #[test]
    fn parse_refuses_what_is_not_integer_pcm() {
        assert!(parse(b"ID3\x03not a wav at all").is_err());
        let mut float = encode(&mono16(&[1])).unwrap();
        float[20..22].copy_from_slice(&3u16.to_le_bytes()); // IEEE float
        assert!(parse(&float)
            .unwrap_err()
            .to_string()
            .contains("not integer PCM"));
        let mut no_fmt = encode(&mono16(&[1])).unwrap();
        no_fmt[12..16].copy_from_slice(b"junk");
        assert!(parse(&no_fmt).is_err());
    }
}
