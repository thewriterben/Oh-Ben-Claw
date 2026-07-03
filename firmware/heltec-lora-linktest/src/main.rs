//! Phase B, step 1 — Heltec WiFi LoRa 32 V3 board bring-up.
//!
//! Deliberately minimal: prove the board flashes with our Rust/ESP-IDF toolchain,
//! boots, logs over its CP2102 USB-UART, and blinks the onboard LED (GPIO35).
//! Only once this is confirmed do we add the SX1262 LoRa driver + ping/listen.
//!
//! Board facts (from the community `heltec_esp32_lora_v3` library), for the next
//! step:
//!   LED = GPIO35 · PRG button = GPIO0 · VEXT = GPIO36
//!   SX1262 (SPI): NSS=8 SCK=9 MOSI=10 MISO=11 · RST=12 BUSY=13 DIO1=14
//!   TCXO is on the SX1262's DIO3 (~1.8 V); RF switch on DIO2 (chip-controlled).

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::PinDriver;
use esp_idf_svc::hal::peripherals::Peripherals;
use log::info;

fn main() -> anyhow::Result<()> {
    // Required ESP-IDF Rust runtime setup.
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    // Onboard user LED (active-high) on GPIO35.
    let mut led = PinDriver::output(peripherals.pins.gpio35)?;

    info!("──────────────────────────────────────────────");
    info!("Heltec WiFi LoRa 32 V3 — link-test boot firmware");
    info!("Board + Rust/ESP-IDF toolchain OK. Blinking LED (GPIO35).");
    info!("Next: SX1262 driver + LoRa ping/listen.");
    info!("──────────────────────────────────────────────");

    let mut n: u32 = 0;
    loop {
        led.set_high()?;
        FreeRtos::delay_ms(200);
        led.set_low()?;
        FreeRtos::delay_ms(800);
        n = n.wrapping_add(1);
        info!("heartbeat {n} (LED blink on GPIO35)");
    }
}
