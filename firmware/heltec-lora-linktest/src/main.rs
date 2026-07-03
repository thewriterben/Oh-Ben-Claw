//! Phase B — Heltec WiFi LoRa 32 V3 as an OBC **spine node** over LoRa.
//!
//! Stage 1 of the spine-over-LoRa integration: prove the LoRa link carries
//! structured OBC spine frames (`[src][seq][ttl][payload]`, see `spine.rs`), not
//! just raw pings. Each board emits a JSON heartbeat spine frame every ~2 s and
//! logs every frame it receives — de-duplicated by `(src, seq)` (the seed of mesh
//! relay). Later stages add a UART bridge so the payload comes from a real node
//! (the XIAO) instead of a synthetic heartbeat.
//!
//! Region: 915 MHz (US ISM). Modulation: SF7 / BW 125 kHz / CR 4-5.
//! Heltec V3 pins: NSS=8 SCK=9 MOSI=10 MISO=11 · RST=12 BUSY=13 DIO1=14.

mod spine;
mod sx1262;

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::spi::config::{Config as SpiConfig, DriverConfig};
use esp_idf_svc::hal::spi::SpiDeviceDriver;
use esp_idf_svc::hal::units::Hertz;
use log::info;

use spine::{SeenSet, SpineFrame};
use sx1262::Sx1262;

/// US ISM band.
const FREQ_HZ: u64 = 915_000_000;
const PIN_RST: i32 = 12;
const PIN_BUSY: i32 = 13;
const PIN_DIO1: i32 = 14;
/// Hop budget for flood-relay (mesh). 0 for now — 2 nodes don't relay; bump when
/// a 3rd node is added and stage-2 relay is enabled.
const HEARTBEAT_TTL: u8 = 0;

fn now_ms() -> u64 {
    (unsafe { esp_idf_svc::sys::esp_timer_get_time() } / 1000) as u64
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let pins = peripherals.pins;

    info!("──────────────────────────────────────────────");
    info!("Heltec V3 OBC spine node — 915 MHz, SF7/BW125/CR4-5");

    let spi = SpiDeviceDriver::new_single(
        peripherals.spi2,
        pins.gpio9,
        pins.gpio10,
        Some(pins.gpio11),
        Some(pins.gpio8),
        &DriverConfig::new(),
        &SpiConfig::new().baudrate(Hertz(8_000_000)),
    )?;

    let mut radio = Sx1262::new(spi, PIN_RST, PIN_BUSY, PIN_DIO1);
    let sync = radio.init(FREQ_HZ)?;
    let status = radio.status()?;
    sx1262::log_selftest(status, sync);

    let mut mac = [0u8; 6];
    // SAFETY: reads the factory MAC from efuse into a 6-byte buffer.
    unsafe { esp_idf_svc::sys::esp_efuse_mac_get_default(mac.as_mut_ptr()); }
    let node = mac[5];
    info!("Spine node {node:02X} — heartbeat + listen.");
    info!("──────────────────────────────────────────────");

    let mut seen = SeenSet::new();
    let mut seq: u8 = 0;
    let mut buf: Vec<u8> = Vec::new();

    loop {
        // ── Emit a heartbeat spine frame. ──
        seq = seq.wrapping_add(1);
        // Record our own (src, seq) so we never re-process an echo of it.
        seen.seen_or_insert(node, seq);
        let payload = format!(
            "{{\"node_id\":\"heltec-{node:02X}\",\"type\":\"heartbeat\",\"seq\":{seq},\"uptime_ms\":{}}}",
            now_ms()
        );
        SpineFrame { src: node, seq, ttl: HEARTBEAT_TTL, payload: payload.as_bytes() }
            .encode(&mut buf);
        match radio.transmit(&buf) {
            Ok(()) => info!("SPINE ► seq={seq} ({} B) {payload}", buf.len()),
            Err(e) => info!("SPINE TX error: {e:#}"),
        }

        // ── Listen for spine frames from other nodes (~1.8 s). ──
        match radio.receive(1_800) {
            Ok(Some(rx)) => match SpineFrame::decode(&rx.data) {
                Some(f) => {
                    if seen.seen_or_insert(f.src, f.seq) {
                        info!("SPINE ◄ dup src={:02X} seq={} — dropped", f.src, f.seq);
                    } else {
                        info!(
                            "SPINE ◄ src={:02X} seq={} ttl={} rssi={} dBm : {}",
                            f.src,
                            f.seq,
                            f.ttl,
                            rx.rssi_dbm,
                            String::from_utf8_lossy(f.payload)
                        );
                    }
                }
                None => info!("SPINE ◄ malformed frame ({} B)", rx.data.len()),
            },
            Ok(None) => {}
            Err(e) => info!("SPINE RX error: {e:#}"),
        }

        FreeRtos::delay_ms(200);
    }
}
