//! Real I2C sensor drivers — replaces the `sensor_read` placeholder for the
//! sensors that feed the on-MCU reflex/safing loop.
//!
//! This increment implements the three highest-impact, lowest-risk devices:
//!
//! - **MAX17048** fuel gauge → `sensor.battery_soc`. This is the reading the
//!   built-in battery safing rules watch, so making it real is what turns on
//!   genuine self-protection (critical-battery load shed) on hardware.
//! - **MPU6050** accelerometer → `sensor.accel_{x,y,z}` (m/s²).
//! - **BME280** environment → `sensor.{temperature,pressure,humidity}`, with the
//!   Bosch fixed-point compensation. (This module's header called that "the
//!   deliberate next step" until 2026-08-21, by which time it was implemented
//!   forty lines below.)
//!
//! Anything this driver does not handle (SHT31, …) returns `None` from
//! [`SensorBus::read`], and the caller falls back to the stub — so a board
//! without those parts still boots and reacts.
//!
//! All of the decode and compensation arithmetic lives in
//! [`crate::sensor_math`], which has no ESP dependencies; the I2C traffic here
//! is `esp-idf-hal` and only exists on the chip.
//!
//! **Still untested on metal**, but that caveat used to cover more than it
//! should have. It read "the register maps and scale factors are from the
//! device datasheets, but verify on the bench", which bundled two claims:
//!
//! * *Does the arithmetic implement the datasheet?* Deterministic integer maths
//!   over given inputs — it never needed a bench, and `tests/firmware_sensor_math.rs`
//!   now checks it against the datasheet's own double-precision algorithm.
//! * *Are the addresses, register maps and wiring right for the real part?*
//!   That does need a bench (`BRINGUP.md` §4/§6), and still has not had one.

use crate::sensor_math::{
    compensate_humidity, compensate_pressure, compensate_temperature, decode_accel,
    decode_soc, parse_bme280_calib, Bme280Calib,
};
use anyhow::Context;
use esp_idf_svc::hal::delay::{TickType, TickType_t};
use esp_idf_svc::hal::i2c::I2cDriver;

/// Per-transaction I2C timeout. Critical: without a finite bound, a stuck bus
/// (e.g. a half-wired sensor holding SDA low) makes `write_read` block *forever*,
/// which freezes the single-threaded main loop and hangs the whole node until a
/// physical power-cycle. A finite timeout turns a bad/absent sensor into a clean
/// read error instead — the node keeps ticking. 50 ms is generous for a 100 kHz
/// bus (each transaction is well under 1 ms).
const I2C_TIMEOUT: TickType_t = TickType::new_millis(50).ticks();

// ── MAX17048 fuel gauge ─────────────────────────────────────────────────────────
const MAX17048_ADDR: u8 = 0x36;
/// SOC register: high byte = integer %, low byte = 1/256 %.
const MAX17048_REG_SOC: u8 = 0x04;

// ── MPU6050 IMU ──────────────────────────────────────────────────────────────────
const MPU6050_ADDR: u8 = 0x68;
const MPU6050_REG_PWR_MGMT_1: u8 = 0x6B;
const MPU6050_REG_ACCEL_XOUT_H: u8 = 0x3B;

// ── BME280 environment sensor ────────────────────────────────────────────────────
/// Primary I2C address (SDO→GND). `0x77` (SDO→VDD) is tried as a fallback.
const BME280_ADDR_PRIMARY: u8 = 0x76;
const BME280_ADDR_SECONDARY: u8 = 0x77;
const BME280_REG_CHIP_ID: u8 = 0xD0;
const BME280_CHIP_ID: u8 = 0x60;
const BME280_REG_CTRL_HUM: u8 = 0xF2;
const BME280_REG_STATUS: u8 = 0xF3;
const BME280_REG_CTRL_MEAS: u8 = 0xF4;
const BME280_REG_CONFIG: u8 = 0xF5;
const BME280_REG_CALIB_00: u8 = 0x88; // dig_T1 … dig_H1 (26 bytes)
const BME280_REG_CALIB_26: u8 = 0xE1; // dig_H2 … dig_H6 (7 bytes)
const BME280_REG_RAW_DATA: u8 = 0xF7; // press[3] temp[3] hum[2]
/// ctrl_meas: temp ×1, press ×1, forced mode (one shot then sleep).
const BME280_CTRL_MEAS_FORCED: u8 = 0b001_001_01;
/// ctrl_hum: humidity ×1 (takes effect on the next ctrl_meas write).
const BME280_CTRL_HUM_X1: u8 = 0x01;

/// Owns the I2C bus and exposes typed sensor reads. Single-threaded on the node,
/// so reads take `&mut self` (each I2C transaction mutates the peripheral).
pub struct SensorBus {
    i2c: I2cDriver<'static>,
    /// BME280 state (address + factory calibration), if one was detected at boot.
    /// `None` ⇒ bme280 reads fall back to the stub.
    bme280: Option<Bme280State>,
}

/// A detected BME280: its bus address and factory trimming coefficients.
#[derive(Clone, Copy)]
struct Bme280State {
    addr: u8,
    calib: Bme280Calib,
}

impl SensorBus {
    /// Wrap an initialised I2C driver, wake the IMU (the MPU6050 powers up in sleep
    /// mode), and probe for a BME280 (read its calibration). Both are best-effort so
    /// a board missing either part still works.
    pub fn new(i2c: I2cDriver<'static>) -> Self {
        let mut bus = Self { i2c, bme280: None };
        let _ = bus
            .i2c
            .write(MPU6050_ADDR, &[MPU6050_REG_PWR_MGMT_1, 0x00], I2C_TIMEOUT);
        bus.bme280 = bus.probe_bme280();
        bus
    }

    /// Probe every 7-bit address (0x08–0x77) and return those that ACK. Pure
    /// bench diagnostic: a 1-byte read is side-effect-free, and the finite
    /// `I2C_TIMEOUT` means a stuck bus can't hang the scan. Expected hits:
    /// MPU6050 = 0x68 (0x69 if AD0 high), BME280 = 0x76/0x77, MAX17048 = 0x36.
    pub fn scan(&mut self) -> Vec<u8> {
        // Short per-probe timeout: a real device ACKs in well under 1 ms, so a
        // tight bound keeps a full 112-address sweep quick even on a stuck bus
        // (vs. ~5.6 s at the 50 ms read timeout).
        let probe = TickType::new_millis(10).ticks();
        let mut found = Vec::new();
        let mut byte = [0u8; 1];
        for addr in 0x08u8..=0x77 {
            if self.i2c.read(addr, &mut byte, probe).is_ok() {
                found.push(addr);
            }
        }
        found
    }

    /// Real read for supported `(sensor, field)` pairs.
    ///
    /// - `None` — this driver does not handle the pair; the caller falls back to
    ///   the stub (so unwired/unsupported sensors degrade gracefully).
    /// - `Some(Ok(v))` — a real reading.
    /// - `Some(Err(_))` — a real read was attempted but the I2C transaction failed
    ///   (missing device, bus fault); surfaced honestly rather than faked.
    pub fn read(&mut self, sensor: &str, field: &str) -> Option<anyhow::Result<f64>> {
        match (sensor, field) {
            ("max17048", "soc") => Some(self.read_soc()),
            ("mpu6050", "accel_x") => Some(self.read_accel(0)),
            ("mpu6050", "accel_y") => Some(self.read_accel(1)),
            ("mpu6050", "accel_z") => Some(self.read_accel(2)),
            ("bme280", "temperature") | ("bme280", "humidity") | ("bme280", "pressure") => {
                // Only claim the read if a BME280 was detected; else fall back to stub.
                match self.bme280 {
                    Some(st) => Some(self.read_bme280(st, field)),
                    None => None,
                }
            }
            _ => None,
        }
    }

    fn read_soc(&mut self) -> anyhow::Result<f64> {
        let mut buf = [0u8; 2];
        self.i2c
            .write_read(MAX17048_ADDR, &[MAX17048_REG_SOC], &mut buf, I2C_TIMEOUT)
            .context("MAX17048 SoC read")?;
        Ok(decode_soc(buf))
    }

    fn read_accel(&mut self, axis: usize) -> anyhow::Result<f64> {
        // Burst-read the 6 accel bytes (XH,XL,YH,YL,ZH,ZL) from ACCEL_XOUT_H.
        let mut buf = [0u8; 6];
        self.i2c
            .write_read(MPU6050_ADDR, &[MPU6050_REG_ACCEL_XOUT_H], &mut buf, I2C_TIMEOUT)
            .context("MPU6050 accel read")?;
        Ok(decode_accel([buf[axis * 2], buf[axis * 2 + 1]]))
    }

    /// Probe both possible BME280 addresses; on a chip-id match, read the factory
    /// calibration. Returns `None` if no BME280 answers (⇒ stub fallback).
    fn probe_bme280(&mut self) -> Option<Bme280State> {
        for addr in [BME280_ADDR_PRIMARY, BME280_ADDR_SECONDARY] {
            let mut id = [0u8; 1];
            if self
                .i2c
                .write_read(addr, &[BME280_REG_CHIP_ID], &mut id, I2C_TIMEOUT)
                .is_ok()
                && id[0] == BME280_CHIP_ID
            {
                if let Ok(calib) = self.read_bme280_calib(addr) {
                    // Filter off, forced mode is driven per-read.
                    let _ = self.i2c.write(addr, &[BME280_REG_CONFIG, 0x00], I2C_TIMEOUT);
                    return Some(Bme280State { addr, calib });
                }
            }
        }
        None
    }

    /// Read and parse the 33 calibration bytes (two blocks).
    fn read_bme280_calib(&mut self, addr: u8) -> anyhow::Result<Bme280Calib> {
        let mut a = [0u8; 26];
        self.i2c
            .write_read(addr, &[BME280_REG_CALIB_00], &mut a, I2C_TIMEOUT)
            .context("BME280 calib 0x88")?;
        let mut b = [0u8; 7];
        self.i2c
            .write_read(addr, &[BME280_REG_CALIB_26], &mut b, I2C_TIMEOUT)
            .context("BME280 calib 0xE1")?;
        Ok(parse_bme280_calib(&a, &b))
    }

    /// Trigger a forced measurement and return the requested compensated field
    /// (temperature °C, humidity %RH, pressure hPa — the host conventions).
    fn read_bme280(&mut self, st: Bme280State, field: &str) -> anyhow::Result<f64> {
        // Forced mode: set humidity oversampling, then ctrl_meas re-arms one shot.
        self.i2c
            .write(st.addr, &[BME280_REG_CTRL_HUM, BME280_CTRL_HUM_X1], I2C_TIMEOUT)
            .context("BME280 ctrl_hum")?;
        self.i2c
            .write(st.addr, &[BME280_REG_CTRL_MEAS, BME280_CTRL_MEAS_FORCED], I2C_TIMEOUT)
            .context("BME280 ctrl_meas")?;
        // Wait for the measurement to complete (status.measuring clears), bounded.
        for _ in 0..64 {
            let mut s = [0u8; 1];
            self.i2c
                .write_read(st.addr, &[BME280_REG_STATUS], &mut s, I2C_TIMEOUT)
                .context("BME280 status")?;
            if s[0] & 0x08 == 0 {
                break;
            }
        }
        // Burst-read press[3] temp[3] hum[2].
        let mut d = [0u8; 8];
        self.i2c
            .write_read(st.addr, &[BME280_REG_RAW_DATA], &mut d, I2C_TIMEOUT)
            .context("BME280 raw read")?;
        let adc_p = ((d[0] as i32) << 12) | ((d[1] as i32) << 4) | ((d[2] as i32) >> 4);
        let adc_t = ((d[3] as i32) << 12) | ((d[4] as i32) << 4) | ((d[5] as i32) >> 4);
        let adc_h = ((d[6] as i32) << 8) | (d[7] as i32);

        // Temperature must be compensated first (it sets t_fine for P and H).
        let mut t_fine = 0i32;
        let temp_centi = compensate_temperature(adc_t, &st.calib, &mut t_fine);
        match field {
            "temperature" => Ok(temp_centi as f64 / 100.0),
            "pressure" => {
                let p_q24_8 = compensate_pressure(adc_p, &st.calib, t_fine);
                Ok(p_q24_8 as f64 / 256.0 / 100.0) // Q24.8 Pa → Pa → hPa
            }
            "humidity" => {
                let h_q22_10 = compensate_humidity(adc_h, &st.calib, t_fine);
                Ok(h_q22_10 as f64 / 1024.0) // Q22.10 %RH → %RH
            }
            other => anyhow::bail!("unsupported BME280 field {other}"),
        }
    }
}
