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
use log::{info, warn};

use spine::{Framed, LineFramer, SeenSet, SpineFrame};
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
    let uart = UartDriver::new(
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
    info!("Console origin: type/send a JSON command line here to transmit it over the mesh.");
    info!("──────────────────────────────────────────────");

    // Host-command origin (Phase B outbound). A background thread reads newline/CR-
    // delimited lines from the USB console (UART0 stdin) and hands each to the radio
    // loop, which frames it onto LoRa. It only *reads* stdin — it never installs a
    // UART0 driver or reconfigures the console, so EspLogger output is untouched. This
    // lets a host originate node commands with no extra wiring on this board.
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<String>();
    std::thread::Builder::new()
        .stack_size(4096)
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut lock = stdin.lock();
            // Bulk read, not byte-at-a-time. The original took one byte per `read`
            // and slept 20 ms whenever stdin was momentarily empty. A host writing
            // two commands back to back (176 B, no gap — 2026-07-19 18:51:16)
            // outran it and bytes were lost mid-burst: both commands reached the
            // radio as mid-string fragments, 50 B and 31 B. That is worse than
            // dropping them, because a malformed frame still transmits, still
            // costs airtime, and still has to be parsed and rejected downstream.
            let mut chunk = [0u8; 256];
            let mut framer = LineFramer::new();
            loop {
                match std::io::Read::read(&mut lock, &mut chunk) {
                    Ok(0) => std::thread::sleep(std::time::Duration::from_millis(20)),
                    Ok(n) => {
                        for &c in &chunk[..n] {
                            match framer.push(c) {
                                // Same rule as the uplink: a stray shell command
                                // pasted into the wrong window is not a mesh message.
                                Framed::Line(l) if !spine::is_spine_payload(l) => warn!(
                                    "console: ignored non-OBC line ({} B) {}",
                                    l.len(),
                                    String::from_utf8_lossy(l)
                                ),
                                Framed::Line(l) => {
                                    if let Ok(s) = std::str::from_utf8(l) {
                                        let _ = cmd_tx.send(s.trim().to_string());
                                    }
                                }
                                Framed::Overflow => warn!(
                                    "console: command longer than {} B — discarded",
                                    spine::MAX_PAYLOAD
                                ),
                                Framed::Pending => {}
                            }
                        }
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
                }
            }
        })
        .ok();

    let mut seen = SeenSet::new();
    let mut seq: u8 = 0;
    let mut buf: Vec<u8> = Vec::new();
    let mut last_keepalive = now_ms();
    let uart_read_timeout = TickType::new_millis(20).ticks();
    // Same framer as the console origin: complete lines only, oversized ones
    // discarded whole rather than transmitted as a prefix.
    let mut uart_framer = LineFramer::new();

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
                Ok(1) => match uart_framer.push(byte[0]) {
                    // Not every byte on this wire is a message. GPIO43 doubles as the
                    // node's ROM UART, so a reset dumps the bootloader log down the
                    // uplink before the app owns the pin. Drop it here rather than
                    // spend airtime on it — logged locally, so it is visible without
                    // being transmitted.
                    Framed::Line(l) if !spine::is_spine_payload(l) => {
                        info!("uart: dropped non-OBC line ({} B) {}", l.len(), String::from_utf8_lossy(l))
                    }
                    Framed::Line(l) => {
                        let txt = String::from_utf8_lossy(l).to_string();
                        match send_spine!(radio, seen, seq, buf, l) {
                            Ok(()) => info!("SPINE ► (uart) seq={seq} ({} B) {txt}", buf.len()),
                            Err(e) => info!("SPINE TX error: {e:#}"),
                        }
                    }
                    Framed::Overflow => warn!(
                        "SPINE ► (uart) line longer than {} B — discarded",
                        spine::MAX_PAYLOAD
                    ),
                    Framed::Pending => {}
                },
                _ => break, // timeout / no more bytes ready
            }
        }

        // ── 1b. Drain host console commands → LoRa (base-station origin). ──
        // Each JSON line the console thread captured is framed onto the mesh, so a host
        // plugged into this Heltec can command a node reachable only over LoRa.
        while let Ok(cmd) = cmd_rx.try_recv() {
            if cmd.is_empty() {
                continue;
            }
            match send_spine!(radio, seen, seq, buf, cmd.as_bytes()) {
                Ok(()) => info!("SPINE ► (console) seq={seq} ({} B) {cmd}", buf.len()),
                Err(e) => info!("SPINE TX error: {e:#}"),
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
                        // SNR alongside RSSI: together they separate the two opposite RF
                        // faults that look identical from one number. Weak-and-clean
                        // (low RSSI, positive SNR) is range. Strong-and-dirty (high
                        // RSSI, collapsed SNR) is an overdriven receiver — radios too
                        // close, which on 2026-07-17 destroyed every 205-byte frame
                        // while letting keepalives through.
                        //
                        // Safe to add: the host parser reads these by key
                        // (`field_after(rest, "rssi=")`), not by position, and splits
                        // the payload on " : " which still follows.
                        info!(
                            "SPINE ◄ src={:02X} seq={} rssi={} dBm snr={} dB : {}",
                            f.src, f.seq, rx.rssi_dbm, rx.snr_db, txt
                        );
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
