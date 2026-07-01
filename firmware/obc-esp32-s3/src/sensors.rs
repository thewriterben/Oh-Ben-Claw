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
            .write(MPU6050_ADDR, &[MPU6050_REG_PWR_MGMT_1, 0x00], BLOCK);
        bus.bme280 = bus.probe_bme280();
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

    /// Probe both possible BME280 addresses; on a chip-id match, read the factory
    /// calibration. Returns `None` if no BME280 answers (⇒ stub fallback).
    fn probe_bme280(&mut self) -> Option<Bme280State> {
        for addr in [BME280_ADDR_PRIMARY, BME280_ADDR_SECONDARY] {
            let mut id = [0u8; 1];
            if self
                .i2c
                .write_read(addr, &[BME280_REG_CHIP_ID], &mut id, BLOCK)
                .is_ok()
                && id[0] == BME280_CHIP_ID
            {
                if let Ok(calib) = self.read_bme280_calib(addr) {
                    // Filter off, forced mode is driven per-read.
                    let _ = self.i2c.write(addr, &[BME280_REG_CONFIG, 0x00], BLOCK);
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
            .write_read(addr, &[BME280_REG_CALIB_00], &mut a, BLOCK)
            .context("BME280 calib 0x88")?;
        let mut b = [0u8; 7];
        self.i2c
            .write_read(addr, &[BME280_REG_CALIB_26], &mut b, BLOCK)
            .context("BME280 calib 0xE1")?;
        Ok(parse_bme280_calib(&a, &b))
    }

    /// Trigger a forced measurement and return the requested compensated field
    /// (temperature °C, humidity %RH, pressure hPa — the host conventions).
    fn read_bme280(&mut self, st: Bme280State, field: &str) -> anyhow::Result<f64> {
        // Forced mode: set humidity oversampling, then ctrl_meas re-arms one shot.
        self.i2c
            .write(st.addr, &[BME280_REG_CTRL_HUM, BME280_CTRL_HUM_X1], BLOCK)
            .context("BME280 ctrl_hum")?;
        self.i2c
            .write(st.addr, &[BME280_REG_CTRL_MEAS, BME280_CTRL_MEAS_FORCED], BLOCK)
            .context("BME280 ctrl_meas")?;
        // Wait for the measurement to complete (status.measuring clears), bounded.
        for _ in 0..64 {
            let mut s = [0u8; 1];
            self.i2c
                .write_read(st.addr, &[BME280_REG_STATUS], &mut s, BLOCK)
                .context("BME280 status")?;
            if s[0] & 0x08 == 0 {
                break;
            }
        }
        // Burst-read press[3] temp[3] hum[2].
        let mut d = [0u8; 8];
        self.i2c
            .write_read(st.addr, &[BME280_REG_RAW_DATA], &mut d, BLOCK)
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

/// Decode the MAX17048 SOC register (`[msb, lsb]`) to a percentage.
fn decode_soc(bytes: [u8; 2]) -> f64 {
    bytes[0] as f64 + bytes[1] as f64 / 256.0
}

/// Decode a big-endian signed 16-bit MPU6050 accel sample to m/s².
fn decode_accel(bytes: [u8; 2]) -> f64 {
    let raw = i16::from_be_bytes(bytes);
    (raw as f64 / MPU6050_ACCEL_LSB_PER_G) * G_MS2
}

// ── BME280 factory calibration + Bosch fixed-point compensation ──────────────────
//
// Transcribed from the BME280 datasheet reference algorithm (Bosch Sensortec,
// §4.2.3 / §8.1): the int32 paths for temperature and humidity and the int64 path
// for pressure. The three `compensate_*` functions are pure so the fixed-point math
// is auditable in isolation; only the I2C traffic above is device-bound.

/// Factory trimming coefficients (BME280 datasheet §4.2.2). Signedness matters —
/// each field's type mirrors the datasheet.
#[derive(Clone, Copy)]
struct Bme280Calib {
    t1: u16,
    t2: i16,
    t3: i16,
    p1: u16,
    p2: i16,
    p3: i16,
    p4: i16,
    p5: i16,
    p6: i16,
    p7: i16,
    p8: i16,
    p9: i16,
    h1: u8,
    h2: i16,
    h3: u8,
    h4: i16,
    h5: i16,
    h6: i8,
}

/// Parse the two calibration blocks: `a` = 26 bytes from 0x88, `b` = 7 bytes from 0xE1.
fn parse_bme280_calib(a: &[u8; 26], b: &[u8; 7]) -> Bme280Calib {
    let u16le = |lo: u8, hi: u8| u16::from_le_bytes([lo, hi]);
    let i16le = |lo: u8, hi: u8| i16::from_le_bytes([lo, hi]);
    Bme280Calib {
        t1: u16le(a[0], a[1]),
        t2: i16le(a[2], a[3]),
        t3: i16le(a[4], a[5]),
        p1: u16le(a[6], a[7]),
        p2: i16le(a[8], a[9]),
        p3: i16le(a[10], a[11]),
        p4: i16le(a[12], a[13]),
        p5: i16le(a[14], a[15]),
        p6: i16le(a[16], a[17]),
        p7: i16le(a[18], a[19]),
        p8: i16le(a[20], a[21]),
        p9: i16le(a[22], a[23]),
        // a[24] is reserved (0xA0); a[25] is dig_H1 (0xA1).
        h1: a[25],
        h2: i16le(b[0], b[1]),      // 0xE1/0xE2
        h3: b[2],                    // 0xE3
        // dig_H4: 0xE4[11:4] (sign) | 0xE5[3:0]
        h4: ((b[3] as i8 as i16) << 4) | ((b[4] & 0x0F) as i16),
        // dig_H5: 0xE6[11:4] (sign) | 0xE5[7:4]
        h5: ((b[5] as i8 as i16) << 4) | ((b[4] >> 4) as i16),
        h6: b[6] as i8,              // 0xE7
    }
}

/// Compensated temperature in hundredths of °C; also produces `t_fine`, the shared
/// fine-resolution term the pressure and humidity formulas need.
fn compensate_temperature(adc_t: i32, c: &Bme280Calib, t_fine: &mut i32) -> i32 {
    let var1 = (((adc_t >> 3) - ((c.t1 as i32) << 1)) * (c.t2 as i32)) >> 11;
    let var2 = ((((adc_t >> 4) - (c.t1 as i32)) * ((adc_t >> 4) - (c.t1 as i32))) >> 12)
        * (c.t3 as i32)
        >> 14;
    *t_fine = var1 + var2;
    (*t_fine * 5 + 128) >> 8
}

/// Compensated pressure in Q24.8 pascals (value / 256 = Pa). Returns 0 on the
/// degenerate `var1 == 0` case, per the reference algorithm.
fn compensate_pressure(adc_p: i32, c: &Bme280Calib, t_fine: i32) -> u32 {
    let mut var1 = (t_fine as i64) - 128_000;
    let mut var2 = var1 * var1 * (c.p6 as i64);
    var2 += (var1 * (c.p5 as i64)) << 17;
    var2 += (c.p4 as i64) << 35;
    var1 = ((var1 * var1 * (c.p3 as i64)) >> 8) + ((var1 * (c.p2 as i64)) << 12);
    var1 = (((1i64 << 47) + var1) * (c.p1 as i64)) >> 33;
    if var1 == 0 {
        return 0;
    }
    let mut p: i64 = 1_048_576 - (adc_p as i64);
    p = (((p << 31) - var2) * 3125) / var1;
    var1 = ((c.p9 as i64) * (p >> 13) * (p >> 13)) >> 25;
    var2 = ((c.p8 as i64) * p) >> 19;
    p = ((p + var1 + var2) >> 8) + ((c.p7 as i64) << 4);
    p as u32
}

/// Compensated relative humidity in Q22.10 %RH (value / 1024 = %RH).
fn compensate_humidity(adc_h: i32, c: &Bme280Calib, t_fine: i32) -> u32 {
    let mut v = t_fine - 76_800;
    v = (((((adc_h << 14) - ((c.h4 as i32) << 20) - ((c.h5 as i32) * v)) + 16_384) >> 15)
        * (((((((v * (c.h6 as i32)) >> 10) * (((v * (c.h3 as i32)) >> 11) + 32_768)) >> 10)
            + 2_097_152)
            * (c.h2 as i32)
            + 8_192)
            >> 14));
    v -= (((((v >> 15) * (v >> 15)) >> 7) * (c.h1 as i32)) >> 4);
    v = v.clamp(0, 419_430_400);
    (v >> 12) as u32
}
