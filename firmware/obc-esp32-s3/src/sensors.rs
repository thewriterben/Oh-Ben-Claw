//! Real I2C sensor drivers — replaces the `sensor_read` placeholder for the
//! sensors that feed the on-MCU reflex/safing loop.
//!
//! This increment implements the two highest-impact, lowest-risk devices:
//!
//! - **MAX17048** fuel gauge → `sensor.battery_soc`. This is the reading the
//!   built-in battery safing rules watch, so making it real is what turns on
//!   genuine self-protection (critical-battery load shed) on hardware.
//! - **MPU6050** accelerometer → `sensor.accel_{x,y,z}` (m/s²).
//!
//! Anything this driver does not handle (BME280 environment, SHT31, …) returns
//! `None` from [`SensorBus::read`], and the caller falls back to the stub — so a
//! board without those parts still boots and reacts. The BME280 environment read
//! (with its Bosch fixed-point compensation) is the deliberate next step.
//!
//! Register decode is factored into pure functions ([`decode_soc`],
//! [`decode_accel`]) so the wire math is obvious and reviewable; the I2C traffic
//! itself is `esp-idf-hal` and only exists on the chip.
//!
//! **Untested on metal by its author** — the register maps and scale factors are
//! from the device datasheets, but verify on the bench (see `BRINGUP.md` §4/§6).

use anyhow::Context;
use esp_idf_svc::hal::delay::BLOCK;
use esp_idf_svc::hal::i2c::I2cDriver;

// ── MAX17048 fuel gauge ─────────────────────────────────────────────────────────
const MAX17048_ADDR: u8 = 0x36;
/// SOC register: high byte = integer %, low byte = 1/256 %.
const MAX17048_REG_SOC: u8 = 0x04;

// ── MPU6050 IMU ──────────────────────────────────────────────────────────────────
const MPU6050_ADDR: u8 = 0x68;
const MPU6050_REG_PWR_MGMT_1: u8 = 0x6B;
const MPU6050_REG_ACCEL_XOUT_H: u8 = 0x3B;
/// Accel sensitivity at the ±2 g default range (LSB per g).
const MPU6050_ACCEL_LSB_PER_G: f64 = 16_384.0;
/// Standard gravity (m/s² per g) — the host convention for `sensor.accel_*`.
const G_MS2: f64 = 9.806_65;

/// Owns the I2C bus and exposes typed sensor reads. Single-threaded on the node,
/// so reads take `&mut self` (each I2C transaction mutates the peripheral).
pub struct SensorBus {
    i2c: I2cDriver<'static>,
}

impl SensorBus {
    /// Wrap an initialised I2C driver and wake the IMU (the MPU6050 powers up in
    /// sleep mode; the write is best-effort so a board without one still works).
    pub fn new(i2c: I2cDriver<'static>) -> Self {
        let mut bus = Self { i2c };
        let _ = bus
            .i2c
            .write(MPU6050_ADDR, &[MPU6050_REG_PWR_MGMT_1, 0x00], BLOCK);
        bus
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
            _ => None,
        }
    }

    fn read_soc(&mut self) -> anyhow::Result<f64> {
        let mut buf = [0u8; 2];
        self.i2c
            .write_read(MAX17048_ADDR, &[MAX17048_REG_SOC], &mut buf, BLOCK)
            .context("MAX17048 SoC read")?;
        Ok(decode_soc(buf))
    }

    fn read_accel(&mut self, axis: usize) -> anyhow::Result<f64> {
        // Burst-read the 6 accel bytes (XH,XL,YH,YL,ZH,ZL) from ACCEL_XOUT_H.
        let mut buf = [0u8; 6];
        self.i2c
            .write_read(MPU6050_ADDR, &[MPU6050_REG_ACCEL_XOUT_H], &mut buf, BLOCK)
            .context("MPU6050 accel read")?;
        Ok(decode_accel([buf[axis * 2], buf[axis * 2 + 1]]))
    }
}

/// Decode the MAX17048 SOC register (`[msb, lsb]`) to a percentage.
fn decode_soc(bytes: [u8; 2]) -> f64 {
    bytes[0] as f64 + bytes[1] as f64 / 256.0
}

/// Decode a big-endian signed 16-bit MPU6050 accel sample to m/s².
fn decode_accel(bytes: [u8; 2]) -> f64 {
    let raw = i16::from_be_bytes(bytes);
    (raw as f64 / MPU6050_ACCEL_LSB_PER_G) * G_MS2
}
