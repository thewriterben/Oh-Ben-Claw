//! Phase B, step 1 — Heltec WiFi LoRa 32 V3 SX1262 point-to-point link test.
//!
//! Flash the *same* firmware to both Heltec V3s. Each board transmits a labelled
//! counter packet, then listens ~1.8 s; on receive it logs the payload + RSSI/SNR.
//! Because LoRa is half-duplex, a board never hears its own transmission — so any
//! received frame is proof the two radios are talking.
//!
//! Startup runs an SPI self-test (reset + read the sync-word register back). If
//! that reads 0x1424 the SPI/BUSY plumbing is good; only then does RF matter.
//!
//! Region: 915 MHz (US ISM). Modulation: SF7 / BW 125 kHz / CR 4-5.
//! Heltec V3 pins: NSS=8 SCK=9 MOSI=10 MISO=11 · RST=12 BUSY=13 DIO1=14.

mod sx1262;

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::spi::config::{Config as SpiConfig, DriverConfig};
use esp_idf_svc::hal::spi::SpiDeviceDriver;
use esp_idf_svc::hal::units::Hertz;
use log::info;

use sx1262::Sx1262;

/// US ISM band.
const FREQ_HZ: u64 = 915_000_000;
// SX1262 control lines (raw GPIO numbers).
const PIN_RST: i32 = 12;
const PIN_BUSY: i32 = 13;
const PIN_DIO1: i32 = 14;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let pins = peripherals.pins;

    info!("──────────────────────────────────────────────");
    info!("Heltec V3 SX1262 link test — 915 MHz, SF7/BW125/CR4-5");

    // SPI bus to the SX1262: SCK=9, MOSI(sdo)=10, MISO(sdi)=11, NSS(cs)=8, 8 MHz.
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

    // Bring the radio up and self-test the SPI link.
    let sync = radio.init(FREQ_HZ)?;
    let status = radio.status()?;
    sx1262::log_selftest(status, sync);

    // Distinguish the two boards in logs by the factory MAC's low byte.
    let mut mac = [0u8; 6];
    // SAFETY: reads the factory MAC from efuse into a 6-byte buffer.
    unsafe { esp_idf_svc::sys::esp_efuse_mac_get_default(mac.as_mut_ptr()); }
    let node = mac[5];
    info!("Node {node:02X} — starting ping/listen loop.");
    info!("──────────────────────────────────────────────");

    let mut counter: u32 = 0;
    loop {
        counter += 1;
        let msg = format!("HELTEC-{node:02X} ping #{counter}");
        match radio.transmit(msg.as_bytes()) {
            Ok(()) => info!("TX ► #{counter} ({} bytes)", msg.len()),
            Err(e) => info!("TX error: {e:#}"),
        }

        match radio.receive(1_800) {
            Ok(Some(rx)) => info!(
                "RX ◄ \"{}\"  rssi={} dBm  snr={} dB",
                String::from_utf8_lossy(&rx.data),
                rx.rssi_dbm,
                rx.snr_db
            ),
            Ok(None) => info!("RX ◄ (nothing this window)"),
            Err(e) => info!("RX error: {e:#}"),
        }

        FreeRtos::delay_ms(200);
    }
}
