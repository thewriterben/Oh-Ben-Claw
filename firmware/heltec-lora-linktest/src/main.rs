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

mod auth;
mod spine;
mod sx1262;

use std::collections::BTreeMap;

use esp_idf_svc::hal::delay::TickType;
use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::spi::config::{Config as SpiConfig, DriverConfig};
use esp_idf_svc::hal::spi::SpiDeviceDriver;
use esp_idf_svc::hal::uart::config::Config as UartConfig;
use esp_idf_svc::hal::uart::UartDriver;
use esp_idf_svc::hal::units::Hertz;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use log::{error, info, warn};
use sha2::Digest;

use spine::{
    rx_ceiling_key, AuthFrame, CeilingStore, Framed, LineFramer, Replay, ReplayWindow, SeqCounter,
    StoreError, MAX_AUTH_PAYLOAD,
};
use sx1262::Sx1262;

// ── Keys ────────────────────────────────────────────────────────────────────
//
// SPINE-AUTH.md §3.1: one root secret per deployment; each station's key is
// `HKDF-SHA256(root, salt = "obc-spine-v1", info = "gw-XX")`. The design has
// each *node* flashed with only its own derived key. The Heltecs are not
// nodes: they are the infrastructure stations that verify every frame on the
// air, from any source, so they need every source's key — which is to say the
// root. That is the decision recorded here (and in DECISIONS.md): stations
// hold the root, and a station is provisioned by building it with
// `OBC_SPINE_ROOT` set. Extracting the root from a station's flash is the
// same threat as extracting a node key from a node's (SPINE-REPLAY.md §4, "a
// cloned node"), one station wider.
//
// The secret arrives at build time and never touches the repository. A build
// without it fails here, with this message, rather than producing a station
// that authenticates nothing. Generate one with `openssl rand -hex 32` and
// keep it with the deployment, not the source.
const ROOT_SECRET: &str = env!(
    "OBC_SPINE_ROOT",
    "OBC_SPINE_ROOT is not set. Every spine frame is authenticated (SPINE-AUTH.md \
     step 4); the station needs the deployment's root secret at build time: \
     `$env:OBC_SPINE_ROOT = '<openssl rand -hex 32>'` then rebuild."
);
const _: () = assert!(
    ROOT_SECRET.len() >= 32,
    "OBC_SPINE_ROOT is shorter than 32 characters; use `openssl rand -hex 32`"
);

/// Every source's key, derived from the root as it is first heard from.
struct KeyRing {
    keys: BTreeMap<u8, [u8; 32]>,
}

impl KeyRing {
    fn new() -> Self {
        Self {
            keys: BTreeMap::new(),
        }
    }

    /// The key for frames from station `src` — the same derivation the host
    /// uses (`obc_safety::spine_tag`), with the station's `gw-XX` id as info.
    fn key_for(&mut self, src: u8) -> [u8; 32] {
        *self.keys.entry(src).or_insert_with(|| {
            auth::derive_node_key(ROOT_SECRET.as_bytes(), &format!("gw-{src:02X}"))
        })
    }

    /// Two bytes of SHA-256 over the root: enough to see at a glance, on the
    /// boot log of two boards, whether they were built with the same secret;
    /// far too little to recover it.
    fn fingerprint() -> u16 {
        let d = sha2::Sha256::digest(ROOT_SECRET.as_bytes());
        u16::from_be_bytes([d[0], d[1]])
    }
}

// ── Persistence ─────────────────────────────────────────────────────────────

/// The counter ceilings in NVS: `spine/seq_ceil` for this station's own
/// counter, `spine/rx_XX` for the receive window of each source heard. See
/// `SeqCounter` and `ReplayWindow` in `spine.rs` for why ceilings, not
/// positions, and why the two round in opposite directions.
struct NvsCeiling(EspNvs<NvsDefault>);

impl CeilingStore for NvsCeiling {
    fn read(&mut self, key: &str) -> Result<u32, StoreError> {
        self.0
            .get_u32(key)
            .map(|v| v.unwrap_or(0))
            .map_err(|_| StoreError)
    }
    fn write(&mut self, key: &str, value: u32) -> Result<(), StoreError> {
        self.0.set_u32(key, value).map_err(|_| StoreError)
    }
}

/// Why a received frame was refused. One reason per line on the console, so
/// the bench can count them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refused {
    /// Too short to be a v2 frame (a retired v1 frame, or noise).
    Runt,
    /// The header `seq` is not the low byte of `ctr`: not built by this code.
    SeqMismatch,
    /// The tag did not verify under the key derived for `src`: wrong root,
    /// forged, or corrupted in flight.
    BadTag,
    /// Verified, but the counter was already accepted: a relay duplicate, or
    /// a replay. Silent — duplicates are normal traffic.
    Seen,
    /// Verified, but older than the receive window can judge.
    TooOld,
    /// The receive window's ceiling could not be read or written. Fail
    /// closed: a frame accepted without a durable window is one a reboot
    /// would accept again.
    Store,
}

impl Refused {
    fn as_str(self) -> &'static str {
        match self {
            Refused::Runt => "runt (not a v2 frame)",
            Refused::SeqMismatch => "seq is not the low byte of ctr",
            Refused::BadTag => "bad tag (wrong root, forged, or corrupt)",
            Refused::Seen => "counter already accepted",
            Refused::TooOld => "counter older than the receive window",
            Refused::Store => "NVS refused the receive ceiling",
        }
    }
}

/// Judge a verified frame's counter against its source's window, persisting
/// the window's ceiling when it moves. The window for a source not yet heard
/// is resumed from NVS (0 if never written — the one-frame replay opportunity
/// SPINE-REPLAY.md §4 accepts).
fn judge(
    windows: &mut BTreeMap<u8, ReplayWindow>,
    store: &mut impl CeilingStore,
    src: u8,
    ctr: u32,
) -> Result<(), Refused> {
    let key = rx_ceiling_key(src);
    let w = match windows.entry(src) {
        std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::btree_map::Entry::Vacant(v) => {
            let ceil = store.read(&key).map_err(|_| Refused::Store)?;
            v.insert(ReplayWindow::resume(ceil))
        }
    };
    let (verdict, need) = w.accept(ctr);
    match verdict {
        Replay::Seen => return Err(Refused::Seen),
        Replay::TooOld => return Err(Refused::TooOld),
        Replay::Accept => {}
    }
    if let Some(c) = need {
        store.write(&key, c).map_err(|_| Refused::Store)?;
    }
    Ok(())
}

const FREQ_HZ: u64 = 915_000_000;
const PIN_RST: i32 = 12;
const PIN_BUSY: i32 = 13;
const PIN_DIO1: i32 = 14;
const KEEPALIVE_MS: u64 = 5_000;
/// Up to this much is added to each keepalive interval, derived from the
/// frame counter so it differs per station and per frame. Without it two
/// stations lock step: measured 2026-09-13 after a bridge reset landed its
/// keepalive within 70 ms of the base's, every 5 s, for as long as anyone
/// watched — each transmitting into the other's frame, neither hearing the
/// other, and nothing to break the phase because the periods are the same
/// firmware. Jitter re-rolls the phase every period; a collision cannot
/// persist. (Listen-before-talk would be the fuller answer; this is the
/// one that is measured.)
const KEEPALIVE_JITTER_MS: u64 = 1_500;

/// The keepalive interval to wait after frame `ctr`: the base period plus a
/// counter-derived jitter. Deterministic per station and per frame, so a run
/// is reproducible, and different across stations because their counters
/// are.
fn keepalive_interval_ms(ctr: u32) -> u64 {
    KEEPALIVE_MS + u64::from(ctr.wrapping_mul(0x9E37_79B9) >> 21) % KEEPALIVE_JITTER_MS
}
/// After originating a command from the console, the next keepalive waits at
/// least this long — long enough for a node's reply to come back (see the
/// note at the console-drain step).
const KEEPALIVE_HOLDOFF_AFTER_CMD_MS: u64 = 3_000;
/// Hop budget for flood-relay. A node that hears a *new* frame rebroadcasts it with
/// ttl-1 until it reaches 0; the `SeenSet` de-dup stops it looping. 2 lets a frame
/// reach nodes two hops out. (With two radios you'll see the rebroadcast and the
/// echo being dropped as a dup; a true 3rd hop needs a node out of direct range.)
const SPINE_TTL: u8 = 2;

/// The tag as sixteen lowercase hex digits, for the console line.
fn hex16(mac: &[u8; spine::MAC_LEN]) -> String {
    mac.iter().map(|b| format!("{b:02x}")).collect()
}

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
    info!(
        "spine auth: frame v2 (ctr:u32 + mac:8), payload budget {} B, root fingerprint {:04x}",
        MAX_AUTH_PAYLOAD,
        KeyRing::fingerprint()
    );
    if cfg!(feature = "no-relay") {
        info!("flood relay DISABLED (no-relay build): this station does not re-broadcast");
    }

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
    unsafe {
        esp_idf_svc::sys::esp_efuse_mac_get_default(mac.as_mut_ptr());
    }
    let node = mac[5];
    info!("Gateway {node:02X} — UART1(TX=4,RX=2) ⇄ LoRa. Wire compute TX→GPIO2, GND↔GND.");
    info!("Console origin: type/send a JSON command line here to transmit it over the mesh.");
    info!("──────────────────────────────────────────────");

    // Host-command origin (Phase B outbound). A background thread reads newline/CR-
    // delimited lines from the USB console (UART0 stdin) and hands each to the radio
    // loop, which frames it onto LoRa. This lets a host originate node commands with
    // no extra wiring on this board.
    //
    // The console needs a real RX buffer first. Without a UART0 driver, stdin is
    // served straight from the ROM UART's hardware FIFO, which is 128 bytes: a
    // line of 127 bytes + newline arrives whole, and a line of 128 loses its tail
    // in the FIFO, never sees its newline, and leaves the framer holding a
    // fragment that swallows every following command until the station is
    // reset. Measured 2026-09-13 with `scripts/probe_mesh_frame_size.py`: 126 B
    // crosses, 127 B crosses, 128 B and everything after it is never transmitted.
    // Every `set_limits` the host ever "sent" over the mesh (202–205 B, well
    // inside the 228-byte radio budget the census measured against) died here,
    // and so did `reflex_tick` with four quantities (157 B); `descend` (81–90 B)
    // and `gpio_read` (72–92 B) crossed by luck of size. Installing the driver
    // gives the VFS a 2 KiB ring and makes the framer's 228-byte discipline the
    // only limit, as it was always documented to be. EspLogger output still
    // goes out the same UART; only the path the bytes take changes.
    //
    // SAFETY: plain ESP-IDF calls with valid arguments — port 0, a 2 KiB RX ring,
    // no TX ring (writes stay synchronous, as they were), no event queue.
    unsafe {
        let err = esp_idf_svc::sys::uart_driver_install(0, 2048, 0, 0, core::ptr::null_mut(), 0);
        if err == esp_idf_svc::sys::ESP_OK {
            esp_idf_svc::sys::esp_vfs_dev_uart_use_driver(0);
            info!("console: UART0 driver installed (2 KiB RX ring); lines up to {MAX_AUTH_PAYLOAD} B accepted");
        } else {
            warn!("console: UART0 driver install failed ({err}); lines over 127 B will be lost");
        }
    }
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
                                    MAX_AUTH_PAYLOAD
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

    let mut buf: Vec<u8> = Vec::new();
    let mut last_keepalive = now_ms();
    let uart_read_timeout = TickType::new_millis(20).ticks();
    // Same framer as the console origin: complete lines only, oversized ones
    // discarded whole rather than transmitted as a prefix.
    let mut uart_framer = LineFramer::new();

    let mut keys = KeyRing::new();
    let own_key = keys.key_for(node);
    // One receive window per source heard, resumed from NVS on first sight.
    // This station's own entry is seeded from its counter so its frames,
    // echoed back by a relay, are dropped as already seen.
    let mut windows: BTreeMap<u8, ReplayWindow> = BTreeMap::new();

    // The frame counter, resumed from NVS so a reboot cannot reissue a
    // counter a neighbour's window has already accepted. If NVS is unusable
    // this station neither transmits nor accepts — fail closed, loudly,
    // rather than run on a counter that repeats or a window that forgets.
    let mut ceiling =
        match EspDefaultNvsPartition::take().and_then(|p| EspNvs::new(p, "spine", true)) {
            Ok(nvs) => Some(NvsCeiling(nvs)),
            Err(e) => {
                error!("NVS unavailable ({e}): no counter backing — TRANSMIT AND RECEIVE DISABLED");
                None
            }
        };
    let mut counter = match ceiling.as_mut().map(SeqCounter::boot) {
        Some(Ok(c)) => {
            info!("frame counter resumed at {} (ceiling persisted)", c.count());
            windows.insert(node, ReplayWindow::resume(c.count()));
            Some(c)
        }
        Some(Err(_)) => {
            error!("counter ceiling could not be read/written — TRANSMIT DISABLED");
            None
        }
        None => None,
    };
    // The last seq issued, for the log lines and the keepalive body.
    let mut seq: u8 = counter.as_ref().map(|c| c.count() as u8).unwrap_or(0);

    // TX one authenticated frame originated by this station: next counter
    // (fail closed if there is none), tag under this station's key, and mark
    // the counter in the local window so the relay echo is dropped.
    macro_rules! send_spine {
        ($radio:expr, $seq:expr, $buf:expr, $payload:expr) => {{
            let payload: &[u8] = $payload;
            if payload.len() > MAX_AUTH_PAYLOAD {
                Err(anyhow::anyhow!(
                    "payload is {} B; an authenticated frame carries {} — refused",
                    payload.len(),
                    MAX_AUTH_PAYLOAD
                ))
            } else {
                match (counter.as_mut(), ceiling.as_mut()) {
                    (Some(c), Some(store)) => match c.next(store) {
                        Ok(s) => {
                            $seq = s;
                            let ctr = c.count();
                            windows
                                .entry(node)
                                .or_insert_with(|| ReplayWindow::resume(0))
                                .accept(ctr);
                            let mac = auth::tag(&own_key, node, ctr, payload);
                            AuthFrame {
                                src: node,
                                seq: s,
                                ttl: SPINE_TTL,
                                ctr,
                                payload,
                                mac,
                            }
                            .encode(&mut $buf);
                            $radio.transmit(&$buf)
                        }
                        Err(_) => Err(anyhow::anyhow!(
                            "counter lost its NVS backing — transmit refused (fail closed)"
                        )),
                    },
                    _ => Err(anyhow::anyhow!(
                        "no frame counter — transmit refused (fail closed)"
                    )),
                }
            }
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
                        info!(
                            "uart: dropped non-OBC line ({} B) {}",
                            l.len(),
                            String::from_utf8_lossy(l)
                        )
                    }
                    Framed::Line(l) => {
                        let txt = String::from_utf8_lossy(l).to_string();
                        match send_spine!(radio, seq, buf, l) {
                            Ok(()) => info!("SPINE ► (uart) seq={seq} ({} B) {txt}", buf.len()),
                            Err(e) => info!("SPINE TX error: {e:#}"),
                        }
                    }
                    Framed::Overflow => warn!(
                        "SPINE ► (uart) line longer than {} B — discarded",
                        MAX_AUTH_PAYLOAD
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
            match send_spine!(radio, seq, buf, cmd.as_bytes()) {
                Ok(()) => info!("SPINE ► (console) seq={seq} ({} B) {cmd}", buf.len()),
                Err(e) => info!("SPINE TX error: {e:#}"),
            }
            // A station that has just asked a question stays quiet for the
            // answer. The node replies 1–2 s after a command; a keepalive
            // transmitted in that window makes this radio deaf for exactly
            // the frame it is waiting for. Measured 2026-09-12: with
            // continuous RX in place, the remaining reply losses each lined
            // up with a base keepalive 1.1–1.9 s after the command.
            last_keepalive = now_ms().saturating_sub(KEEPALIVE_MS) + KEEPALIVE_HOLDOFF_AFTER_CMD_MS;
        }

        // ── 2. Keepalive so the link is visible without a compute node wired. ──
        // Saturating: the hold-off after a console command can put
        // `last_keepalive` a little into the future in the first seconds of
        // a boot, and a plain subtraction there wrapped to a huge value and
        // fired a keepalive 120 ms after the command (bench, 2026-09-13).
        let due = keepalive_interval_ms(counter.as_ref().map(|c| c.count()).unwrap_or(0));
        if now_ms().saturating_sub(last_keepalive) >= due {
            last_keepalive = now_ms();
            let hb = format!(
                "{{\"node_id\":\"gw-{node:02X}\",\"type\":\"gw_keepalive\",\"seq\":{}}}",
                seq.wrapping_add(1)
            );
            match send_spine!(radio, seq, buf, hb.as_bytes()) {
                Ok(()) => info!("SPINE ► (keepalive) seq={seq}"),
                Err(e) => info!("SPINE TX error: {e:#}"),
            }
        }

        // ── 3. Listen for spine frames; verify, judge, log + forward. ──
        // Order matters: the tag first, so an attacker cannot move a window
        // with a frame they cannot sign; the counter second, so a genuine
        // frame is accepted once; and only then does the payload leave the
        // radio. Nothing unverified reaches the UART, the log line the host
        // parses, or the relay.
        match radio.receive(600) {
            Ok(Some(rx)) => {
                let verdict: Result<AuthFrame<'_>, (Refused, u8, u32)> =
                    match AuthFrame::decode(&rx.data) {
                        None => Err((Refused::Runt, 0, 0)),
                        Some(f) if f.seq != f.ctr as u8 => {
                            Err((Refused::SeqMismatch, f.src, f.ctr))
                        }
                        Some(f) => {
                            let key = keys.key_for(f.src);
                            if !auth::verify(&key, f.src, f.ctr, f.payload, &f.mac) {
                                Err((Refused::BadTag, f.src, f.ctr))
                            } else {
                                match ceiling.as_mut() {
                                    None => Err((Refused::Store, f.src, f.ctr)),
                                    Some(store) => match judge(&mut windows, store, f.src, f.ctr) {
                                        Ok(()) => Ok(f),
                                        Err(r) => Err((r, f.src, f.ctr)),
                                    },
                                }
                            }
                        }
                    };
                match verdict {
                    Err((Refused::Seen, _, _)) => {
                        // Relay duplicate or replay: normal traffic, silent.
                    }
                    Err((why, src, ctr)) => warn!(
                        "SPINE ◄ REJECTED src={src:02X} ctr={ctr} rssi={} dBm ({} B): {}",
                        rx.rssi_dbm,
                        rx.data.len(),
                        why.as_str()
                    ),
                    Ok(f) => {
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
                        // `ctr=` and `mac=` are on the line so the host can verify
                        // the same tag this station just verified, rather than
                        // trusting this console (`lora_gateway::LoraAuth`). The
                        // host parser reads fields by key, so their position is
                        // free; the payload still follows " : " and precedes the
                        // colour reset, and is printed byte for byte — the host
                        // verifies over exactly what it sees between the two.
                        info!(
                            "SPINE ◄ src={:02X} seq={} ctr={} mac={} rssi={} dBm snr={} dB : {}",
                            f.src,
                            f.seq,
                            f.ctr,
                            hex16(&f.mac),
                            rx.rssi_dbm,
                            rx.snr_db,
                            txt
                        );
                        // Forward the payload to the wired compute node.
                        let _ = uart.write(f.payload);
                        let _ = uart.write(b"\n");
                        // Flood-relay onward if hops remain. Keep the ORIGINAL src, ctr
                        // and tag — the tag does not cover ttl, so the frame re-encodes
                        // with one fewer hop and still verifies at the next station,
                        // whose window de-dups it identically. That is what stops loops.
                        if f.ttl > 0 && !cfg!(feature = "no-relay") {
                            AuthFrame {
                                ttl: f.ttl - 1,
                                ..f
                            }
                            .encode(&mut buf);
                            match radio.transmit(&buf) {
                                Ok(()) => info!(
                                    "SPINE ⇒ relay src={:02X} seq={} ttl={}",
                                    f.src,
                                    f.seq,
                                    f.ttl - 1
                                ),
                                Err(e) => info!("relay TX error: {e:#}"),
                            }
                        }
                    }
                }
            }
            Ok(None) => {}
            Err(e) => info!("SPINE RX error: {e:#}"),
        }
    }
}
