//! Phase B — Heltec WiFi LoRa 32 V3 as an OBC **spine LoRa gateway**.
//!
//! Stage 2: a real bridge between a wired compute node and the LoRa spine.
//!   - Reads newline-delimited OBC messages from **UART1** (the compute uplink,
//!     e.g. a XIAO node) → wraps each in a spine frame → transmits over LoRa.
//!   - On LoRa receive → de-dups, logs on the console, and forwards the payload
//!     back out UART1 to the local compute node.
//!   - Emits a slow gateway keepalive so the link stays observable even when no
//!     compute node is wired yet.
//!
//! UART1 (compute uplink): TX=GPIO4, RX=GPIO2, 115200 8N1. (UART0/GPIO43-44 is the
//! CP2102 USB console — left alone.) Wire the compute node's TX → GPIO2, GND↔GND.
//!
//! Region: 915 MHz (US ISM). Modulation: SF7 / BW 125 kHz / CR 4-5.
//! SX1262: NSS=8 SCK=9 MOSI=10 MISO=11 · RST=12 BUSY=13 DIO1=14.

mod spine;
mod sx1262;

use esp_idf_svc::hal::delay::TickType;
use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::spi::config::{Config as SpiConfig, DriverConfig};
use esp_idf_svc::hal::spi::SpiDeviceDriver;
use esp_idf_svc::hal::uart::config::Config as UartConfig;
use esp_idf_svc::hal::uart::UartDriver;
use esp_idf_svc::hal::units::Hertz;
use log::info;

use spine::{SeenSet, SpineFrame};
use sx1262::Sx1262;

const FREQ_HZ: u64 = 915_000_000;
const PIN_RST: i32 = 12;
const PIN_BUSY: i32 = 13;
const PIN_DIO1: i32 = 14;
const KEEPALIVE_MS: u64 = 5_000;
/// Hop budget for flood-relay. A node that hears a *new* frame rebroadcasts it with
/// ttl-1 until it reaches 0; the `SeenSet` de-dup stops it looping. 2 lets a frame
/// reach nodes two hops out. (With two radios you'll see the rebroadcast and the
/// echo being dropped as a dup; a true 3rd hop needs a node out of direct range.)
const SPINE_TTL: u8 = 2;

fn now_ms() -> u64 {
    (unsafe { esp_idf_svc::sys::esp_timer_get_time() } / 1000) as u64
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let pins = peripherals.pins;

    info!("──────────────────────────────────────────────");
    info!("Heltec V3 OBC spine gateway — LoRa 915 MHz ⇄ UART1 (compute uplink)");

    // UART1 to the compute node: TX=GPIO4, RX=GPIO2.
    let mut uart = UartDriver::new(
        peripherals.uart1,
        pins.gpio4,
        pins.gpio2,
        Option::<AnyIOPin>::None,
        Option::<AnyIOPin>::None,
        &UartConfig::new().baudrate(Hertz(115_200)),
    )?;

    // SX1262 on SPI2.
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
    // SAFETY: reads the factory MAC into a 6-byte buffer.
    unsafe { esp_idf_svc::sys::esp_efuse_mac_get_default(mac.as_mut_ptr()); }
    let node = mac[5];
    info!("Gateway {node:02X} — UART1(TX=4,RX=2) ⇄ LoRa. Wire compute TX→GPIO2, GND↔GND.");
    info!("──────────────────────────────────────────────");

    let mut seen = SeenSet::new();
    let mut seq: u8 = 0;
    let mut buf: Vec<u8> = Vec::new();
    let mut line: Vec<u8> = Vec::new();
    let mut last_keepalive = now_ms();
    let uart_read_timeout = TickType::new_millis(20).ticks();

    // TX one spine frame originated by this node; advances + records seq.
    macro_rules! send_spine {
        ($radio:expr, $seen:expr, $seq:expr, $buf:expr, $payload:expr) => {{
            $seq = $seq.wrapping_add(1);
            $seen.seen_or_insert(node, $seq);
            SpineFrame { src: node, seq: $seq, ttl: SPINE_TTL, payload: $payload }.encode(&mut $buf);
            $radio.transmit(&$buf)
        }};
    }

    loop {
        // ── 1. Drain UART1 into complete lines; each → LoRa spine frame. ──
        let mut byte = [0u8; 1];
        for _ in 0..512 {
            match uart.read(&mut byte, uart_read_timeout) {
                Ok(1) => {
                    let b = byte[0];
                    if b == b'\n' || b == b'\r' {
                        if !line.is_empty() {
                            let txt = String::from_utf8_lossy(&line).to_string();
                            match send_spine!(radio, seen, seq, buf, &line) {
                                Ok(()) => info!("SPINE ► (uart) seq={seq} ({} B) {txt}", buf.len()),
                                Err(e) => info!("SPINE TX error: {e:#}"),
                            }
                            line.clear();
                        }
                    } else if line.len() < spine::MAX_PAYLOAD {
                        line.push(b);
                    }
                }
                _ => break, // timeout / no more bytes ready
            }
        }

        // ── 2. Keepalive so the link is visible without a compute node wired. ──
        if now_ms() - last_keepalive >= KEEPALIVE_MS {
            last_keepalive = now_ms();
            let hb = format!("{{\"node_id\":\"gw-{node:02X}\",\"type\":\"gw_keepalive\",\"seq\":{}}}", seq.wrapping_add(1));
            match send_spine!(radio, seen, seq, buf, hb.as_bytes()) {
                Ok(()) => info!("SPINE ► (keepalive) seq={seq}"),
                Err(e) => info!("SPINE TX error: {e:#}"),
            }
        }

        // ── 3. Listen for spine frames; log + forward to the local compute node. ──
        match radio.receive(600) {
            Ok(Some(rx)) => match SpineFrame::decode(&rx.data) {
                Some(f) => {
                    if seen.seen_or_insert(f.src, f.seq) {
                        // already handled (dup/relay) — ignore
                    } else {
                        let txt = String::from_utf8_lossy(f.payload);
                        info!("SPINE ◄ src={:02X} seq={} rssi={} dBm : {}", f.src, f.seq, rx.rssi_dbm, txt);
                        // Forward the payload to the wired compute node.
                        let _ = uart.write(f.payload);
                        let _ = uart.write(b"\n");
                        // Flood-relay onward if hops remain. Keep the ORIGINAL src/seq
                        // so every node de-dups it identically — that's what stops loops.
                        if f.ttl > 0 {
                            SpineFrame { src: f.src, seq: f.seq, ttl: f.ttl - 1, payload: f.payload }
                                .encode(&mut buf);
                            match radio.transmit(&buf) {
                                Ok(()) => info!("SPINE ⇒ relay src={:02X} seq={} ttl={}", f.src, f.seq, f.ttl - 1),
                                Err(e) => info!("relay TX error: {e:#}"),
                            }
                        }
                    }
                }
                None => info!("SPINE ◄ malformed frame ({} B)", rx.data.len()),
            },
            Ok(None) => {}
            Err(e) => info!("SPINE RX error: {e:#}"),
        }
    }
}
