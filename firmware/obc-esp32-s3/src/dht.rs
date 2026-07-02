//! DHT22 / AM2302 single-wire temperature + humidity driver.
//!
//! The DHT22 speaks a bit-banged one-wire protocol with microsecond-level
//! timing. The host pulls the data line low for ≥1 ms to start; the sensor
//! replies with an ~80 µs low / ~80 µs high preamble, then streams 40 bits.
//! Each bit is a ~50 µs low followed by a high pulse whose *length* encodes the
//! value (~26 µs high = 0, ~70 µs high = 1). The 40 bits are 5 bytes:
//! `humidity[2]`, `temperature[2]`, `checksum[1]` (MSB first).
//!
//! Reliability notes (mirrors the I2C-timeout hardening):
//! - The bit read runs inside an **interrupt-free** critical section (~5 ms) so
//!   RTOS/WiFi preemption can't corrupt the pulse-width measurements.
//! - **Every** edge-wait is bounded, so a missing/dead sensor returns an error
//!   instead of hanging the node.
//! - No heap allocation happens inside the critical section (the closure returns
//!   a `&'static str` code; the `anyhow::Error` is built afterwards).
//! - Result is checksum- and range-validated before it's trusted.
//!
//! The DHT22 needs ~2 s between reads; callers must rate-limit.
//!
//! **Untested on metal by its author** — the timing constants are from the
//! AM2302 datasheet; expect to fine-tune `HIGH_BIT_THRESHOLD_US` on the bench.

use anyhow::{anyhow, bail, Result};
use esp_idf_svc::hal::delay::Ets;
use esp_idf_svc::hal::interrupt;
use esp_idf_svc::sys::*;

/// A high pulse longer than this many µs is a `1` bit (0 ≈ 26 µs, 1 ≈ 70 µs).
const HIGH_BIT_THRESHOLD_US: i64 = 45;

#[inline]
fn now_us() -> i64 {
    // SAFETY: reads the always-initialised esp_timer; valid with interrupts off.
    unsafe { esp_timer_get_time() }
}

#[inline]
fn level_of(pin: i32) -> i32 {
    // SAFETY: single GPIO level read via the raw sys API.
    unsafe { gpio_get_level(pin) }
}

/// Busy-wait until `pin` reads `level`, returning the elapsed microseconds.
/// Bounded by `timeout_us` so a silent line can never spin forever.
/// `Err(())` on timeout — kept allocation-free for use inside the critical section.
fn wait_for(pin: i32, level: i32, timeout_us: i64) -> Result<i64, ()> {
    let start = now_us();
    loop {
        let elapsed = now_us() - start;
        if level_of(pin) == level {
            return Ok(elapsed);
        }
        if elapsed > timeout_us {
            return Err(());
        }
    }
}

/// Read one DHT22 / AM2302 on `pin` (raw GPIO number). Returns
/// `(temperature_c, humidity_pct)`. Blocks ~5 ms with interrupts disabled.
pub fn read_dht22(pin: i32) -> Result<(f32, f32)> {
    // ── Start signal: idle high, pull low ≥1 ms, release, switch to input. ──
    // SAFETY: raw GPIO on the DHT data pin, which nothing else drives.
    unsafe {
        gpio_set_direction(pin, gpio_mode_t_GPIO_MODE_OUTPUT_OD);
        gpio_set_pull_mode(pin, gpio_pull_mode_t_GPIO_PULLUP_ONLY);
        gpio_set_level(pin, 1);
        Ets::delay_us(40); // settle high
        gpio_set_level(pin, 0);
        Ets::delay_us(1_200); // start pulse (DHT22 needs ≥1 ms)
        gpio_set_level(pin, 1); // release → pull-up brings it high
        gpio_set_direction(pin, gpio_mode_t_GPIO_MODE_INPUT);
    }

    // ── Timing-critical read, interrupts off, allocation-free. ──
    let bytes: [u8; 5] = interrupt::free(|| -> Result<[u8; 5], &'static str> {
        // Preamble: sensor pulls low ~80 µs, then high ~80 µs, then bit 0.
        wait_for(pin, 0, 250).map_err(|_| "no response (check wiring/power)")?;
        wait_for(pin, 1, 250).map_err(|_| "preamble low too long")?;
        wait_for(pin, 0, 250).map_err(|_| "preamble high too long")?;

        let mut data = [0u8; 5];
        for i in 0..40 {
            // Skip the ~50 µs low, then the high-pulse length is the bit value.
            wait_for(pin, 1, 120).map_err(|_| "bit low timeout")?;
            let high_us = wait_for(pin, 0, 150).map_err(|_| "bit high timeout")?;
            if high_us > HIGH_BIT_THRESHOLD_US {
                data[i / 8] |= 1 << (7 - (i % 8));
            }
        }
        Ok(data)
    })
    .map_err(|e| anyhow!("DHT22 read failed: {e}"))?;

    // ── Checksum. ──
    let sum = bytes[0]
        .wrapping_add(bytes[1])
        .wrapping_add(bytes[2])
        .wrapping_add(bytes[3]);
    if sum != bytes[4] {
        bail!(
            "DHT22 checksum mismatch (got {:#04x}, computed {:#04x})",
            bytes[4],
            sum
        );
    }

    // ── Decode (humidity ×10, temperature ×10 with sign bit). ──
    let humidity = (((bytes[0] as u16) << 8) | bytes[1] as u16) as f32 / 10.0;
    let raw_t = (((bytes[2] & 0x7F) as u16) << 8) | bytes[3] as u16;
    let mut temperature = raw_t as f32 / 10.0;
    if bytes[2] & 0x80 != 0 {
        temperature = -temperature;
    }

    // ── Plausibility gate (DHT22 range: −40..80 °C, 0..100 %RH). Guards against
    //    a checksum-passing-but-garbage frame from marginal timing. ──
    if !(-40.0..=80.0).contains(&temperature) || !(0.0..=100.0).contains(&humidity) {
        bail!("DHT22 reading out of range (t={temperature} °C, h={humidity} %)");
    }

    Ok((temperature, humidity))
}
