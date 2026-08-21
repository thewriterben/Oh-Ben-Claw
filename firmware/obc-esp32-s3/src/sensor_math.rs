//! Pure decode and compensation arithmetic for the I2C sensors.
//!
//! Split out of `sensors.rs` on 2026-08-21 so it can be executed. That file
//! carried the note *"Untested on metal by its author — the register maps and
//! scale factors are from the device datasheets, but verify on the bench"*,
//! which quietly bundled two different claims together:
//!
//! * **Does the arithmetic implement the datasheet?** Deterministic integer
//!   maths over given inputs. It never needed a bench, and it is checked by
//!   `tests/firmware_sensor_math.rs` against the datasheet's own
//!   double-precision algorithm — a second, structurally independent formula
//!   Bosch publishes in §8.1 precisely so a fixed-point port can be validated.
//! * **Are the register maps, addresses and wiring right for the real part?**
//!   That does need a bench, and still does.
//!
//! Only the first is settled here. Nothing in this module touches esp-idf; the
//! I2C traffic stays in `sensors.rs` and only exists on the chip.

/// Accel sensitivity at the ±2 g default range (LSB per g).
pub const MPU6050_ACCEL_LSB_PER_G: f64 = 16_384.0;
/// Standard gravity (m/s² per g) — the host convention for `sensor.accel_*`.
pub const G_MS2: f64 = 9.806_65;

/// Decode the MAX17048 SOC register (`[msb, lsb]`) to a percentage.
pub fn decode_soc(bytes: [u8; 2]) -> f64 {
    bytes[0] as f64 + bytes[1] as f64 / 256.0
}

/// Decode a big-endian signed 16-bit MPU6050 accel sample to m/s².
pub fn decode_accel(bytes: [u8; 2]) -> f64 {
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
pub struct Bme280Calib {
    pub t1: u16,
    pub t2: i16,
    pub t3: i16,
    pub p1: u16,
    pub p2: i16,
    pub p3: i16,
    pub p4: i16,
    pub p5: i16,
    pub p6: i16,
    pub p7: i16,
    pub p8: i16,
    pub p9: i16,
    pub h1: u8,
    pub h2: i16,
    pub h3: u8,
    pub h4: i16,
    pub h5: i16,
    pub h6: i8,
}

/// Parse the two calibration blocks: `a` = 26 bytes from 0x88, `b` = 7 bytes from 0xE1.
pub fn parse_bme280_calib(a: &[u8; 26], b: &[u8; 7]) -> Bme280Calib {
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
        h2: i16le(b[0], b[1]), // 0xE1/0xE2
        h3: b[2],              // 0xE3
        // dig_H4: 0xE4[11:4] (sign) | 0xE5[3:0]
        h4: ((b[3] as i8 as i16) << 4) | ((b[4] & 0x0F) as i16),
        // dig_H5: 0xE6[11:4] (sign) | 0xE5[7:4]
        h5: ((b[5] as i8 as i16) << 4) | ((b[4] >> 4) as i16),
        h6: b[6] as i8, // 0xE7
    }
}

/// Compensated temperature in hundredths of °C; also produces `t_fine`, the shared
/// fine-resolution term the pressure and humidity formulas need.
pub fn compensate_temperature(adc_t: i32, c: &Bme280Calib, t_fine: &mut i32) -> i32 {
    let var1 = (((adc_t >> 3) - ((c.t1 as i32) << 1)) * (c.t2 as i32)) >> 11;
    // The parentheses around the multiply are clippy's `precedence` lint, and
    // they change nothing: `*` already binds tighter than `>>`. Worth having
    // explicit in transcribed datasheet arithmetic, where a reader checking the
    // port against §4.2.3 should not have to hold Rust's precedence table too.
    let var2 = (((((adc_t >> 4) - (c.t1 as i32)) * ((adc_t >> 4) - (c.t1 as i32))) >> 12)
        * (c.t3 as i32))
        >> 14;
    *t_fine = var1 + var2;
    (*t_fine * 5 + 128) >> 8
}

/// Compensated pressure in Q24.8 pascals (value / 256 = Pa). Returns 0 on the
/// degenerate `var1 == 0` case, per the reference algorithm.
pub fn compensate_pressure(adc_p: i32, c: &Bme280Calib, t_fine: i32) -> u32 {
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
pub fn compensate_humidity(adc_h: i32, c: &Bme280Calib, t_fine: i32) -> u32 {
    let mut v = t_fine - 76_800;
    v = ((((adc_h << 14) - ((c.h4 as i32) << 20) - ((c.h5 as i32) * v)) + 16_384) >> 15)
        * (((((((v * (c.h6 as i32)) >> 10) * (((v * (c.h3 as i32)) >> 11) + 32_768)) >> 10)
            + 2_097_152)
            * (c.h2 as i32)
            + 8_192)
            >> 14);
    v -= ((((v >> 15) * (v >> 15)) >> 7) * (c.h1 as i32)) >> 4;
    v = v.clamp(0, 419_430_400);
    (v >> 12) as u32
}
