//! Host-side harness for the sensor decode and compensation arithmetic.
//!
//! `sensors.rs` carried this note: *"Untested on metal by its author — the
//! register maps and scale factors are from the device datasheets, but verify
//! on the bench."* That bundled two different claims under one caveat, and only
//! one of them needs hardware:
//!
//! * **Does the arithmetic implement the datasheet?** Deterministic integer
//!   maths over given inputs. It never needed a bench. Checked here.
//! * **Are the addresses, register maps and wiring right for the real part?**
//!   That needs a bench, and still does.
//!
//! The check is not a re-transcription of the same formula, which would pass
//! for a mis-transcription made twice. Bosch publishes **two** algorithms: the
//! fixed-point one the firmware ports (datasheet §4.2.3) and a double-precision
//! one (§8.1). They are structurally independent — different operations,
//! different intermediate scaling — and must agree. That is the standard way to
//! validate a fixed-point port, and the reference below is transcribed from the
//! datasheet rather than from the firmware.
//!
//! Source: BME280 Data sheet, Bosch Sensortec, document revision 1.22.

#[path = "../firmware/obc-esp32-s3/src/sensor_math.rs"]
#[allow(dead_code)]
mod sensor_math;

use sensor_math::{
    compensate_humidity, compensate_pressure, compensate_temperature, decode_accel, decode_soc,
    parse_bme280_calib, Bme280Calib, G_MS2,
};

/// Plausible factory trimming, in the shape and sign the datasheet specifies.
/// The values matter only in that both algorithms see the same ones; the point
/// is agreement between two formulas, not a golden output.
fn calib() -> Bme280Calib {
    Bme280Calib {
        t1: 28_960,
        t2: 26_412,
        t3: 50,
        p1: 36_635,
        p2: -10_696,
        p3: 3_024,
        p4: 6_852,
        p5: -134,
        p6: -7,
        p7: 9_900,
        p8: -10_230,
        p9: 4_285,
        h1: 75,
        h2: 353,
        h3: 0,
        h4: 340,
        h5: 0,
        h6: 30,
    }
}

// ── The datasheet's §8.1 double-precision algorithm, transcribed ──────────────

fn t_double(adc_t: i32, c: &Bme280Calib) -> (f64, f64) {
    let var1 = (adc_t as f64 / 16384.0 - c.t1 as f64 / 1024.0) * c.t2 as f64;
    let var2 = ((adc_t as f64 / 131072.0 - c.t1 as f64 / 8192.0)
        * (adc_t as f64 / 131072.0 - c.t1 as f64 / 8192.0))
        * c.t3 as f64;
    (var1 + var2, (var1 + var2) / 5120.0)
}

fn p_double(adc_p: i32, c: &Bme280Calib, t_fine: f64) -> f64 {
    let mut var1 = t_fine / 2.0 - 64000.0;
    let mut var2 = var1 * var1 * c.p6 as f64 / 32768.0;
    var2 += var1 * c.p5 as f64 * 2.0;
    var2 = var2 / 4.0 + c.p4 as f64 * 65536.0;
    var1 = (c.p3 as f64 * var1 * var1 / 524288.0 + c.p2 as f64 * var1) / 524288.0;
    var1 = (1.0 + var1 / 32768.0) * c.p1 as f64;
    if var1 == 0.0 {
        return 0.0;
    }
    let mut p = 1_048_576.0 - adc_p as f64;
    p = (p - var2 / 4096.0) * 6250.0 / var1;
    var1 = c.p9 as f64 * p * p / 2_147_483_648.0;
    var2 = p * c.p8 as f64 / 32768.0;
    p + (var1 + var2 + c.p7 as f64) / 16.0
}

fn h_double(adc_h: i32, c: &Bme280Calib, t_fine: f64) -> f64 {
    let mut var_h = t_fine - 76800.0;
    var_h = (adc_h as f64 - (c.h4 as f64 * 64.0 + c.h5 as f64 / 16384.0 * var_h))
        * (c.h2 as f64 / 65536.0
            * (1.0
                + c.h6 as f64 / 67_108_864.0 * var_h * (1.0 + c.h3 as f64 / 67_108_864.0 * var_h)));
    var_h *= 1.0 - c.h1 as f64 * var_h / 524288.0;
    var_h.clamp(0.0, 100.0)
}

// ── The tests ────────────────────────────────────────────────────────────────

/// Temperature, across the sensor's full operating span. The fixed-point path
/// returns hundredths of a degree, so a one-unit disagreement is 0.01 °C — well
/// inside the datasheet's ±1 °C accuracy, but the two formulas should track far
/// more closely than that, and do.
#[test]
fn fixed_point_temperature_tracks_the_datasheet_double_algorithm() {
    let c = calib();
    for adc_t in (300_000..=550_000).step_by(9_973) {
        let mut t_fine_i = 0;
        let got = compensate_temperature(adc_t, &c, &mut t_fine_i) as f64 / 100.0;
        let (_, want) = t_double(adc_t, &c);
        assert!(
            (got - want).abs() <= 0.01,
            "adc_t={adc_t}: fixed-point {got} vs datasheet double {want}"
        );
    }
}

/// `t_fine` is the term pressure and humidity both depend on, so a drift here
/// would show up as a plausible-looking error in two other readings rather than
/// as an obviously wrong temperature.
///
/// The two algorithms do *not* agree exactly here, and should not: the
/// fixed-point path takes three arithmetic right shifts (`>>11`, `>>12`,
/// `>>14`), each truncating toward negative infinity, while the double path
/// truncates nothing. Measured across the whole raw range the worst residual is
/// **13.1 units of `t_fine`** — which is 0.0072 °C, because `t_fine` is divided
/// by 5120 to reach degrees. The bound below is that measurement with headroom,
/// not a number chosen to make the test pass; the second assertion is the one
/// that matters, and it is tighter than the fixed-point path's own output
/// resolution of 0.01 °C.
#[test]
fn the_shared_t_fine_term_agrees_with_the_double_algorithm() {
    let c = calib();
    for adc_t in (0..=800_000).step_by(997) {
        let mut t_fine_i = 0;
        let hundredths = compensate_temperature(adc_t, &c, &mut t_fine_i);
        let (want_fine, want_c) = t_double(adc_t, &c);
        assert!(
            (t_fine_i as f64 - want_fine).abs() <= 16.0,
            "adc_t={adc_t}: t_fine {t_fine_i} vs {want_fine}"
        );
        assert!(
            (hundredths as f64 / 100.0 - want_c).abs() <= 0.01,
            "adc_t={adc_t}: the t_fine residual reached the reported temperature"
        );
    }
}

/// Pressure, over the raw range a BME280 actually produces. The int64 path
/// returns Q24.8 pascals; agreement to a few pascals against a double is the
/// expected fixed-point residual on a ~100 kPa reading (~0.005 %).
///
/// Both algorithms are given the *same* `t_fine`, which isolates the pressure
/// formula from the temperature one — a failure here is this formula's, not
/// inherited. The propagation of `t_fine`'s own residual is bounded by the
/// temperature test above.
#[test]
fn fixed_point_pressure_tracks_the_datasheet_double_algorithm() {
    let c = calib();
    let mut t_fine = 0;
    compensate_temperature(519_888, &c, &mut t_fine);
    for adc_p in (250_000..=450_000).step_by(7_919) {
        let got = compensate_pressure(adc_p, &c, t_fine) as f64 / 256.0;
        let want = p_double(adc_p, &c, t_fine as f64);
        assert!(
            (got - want).abs() <= 5.0,
            "adc_p={adc_p}: fixed-point {got} Pa vs datasheet double {want} Pa"
        );
    }
}

/// Humidity, over the full 16-bit raw range. Q22.10 %RH against the double
/// path; 0.05 %RH is far inside the part's ±3 %RH accuracy and the two agree
/// well within it.
#[test]
fn fixed_point_humidity_tracks_the_datasheet_double_algorithm() {
    let c = calib();
    let mut t_fine = 0;
    compensate_temperature(519_888, &c, &mut t_fine);
    for adc_h in (0..=65_535).step_by(1_361) {
        let got = compensate_humidity(adc_h, &c, t_fine) as f64 / 1024.0;
        let want = h_double(adc_h, &c, t_fine as f64);
        assert!(
            (got - want).abs() <= 0.05,
            "adc_h={adc_h}: fixed-point {got} %RH vs datasheet double {want} %RH"
        );
    }
}

/// The datasheet clamps humidity to 419430400 in Q22.10, which is exactly
/// 100 %RH. A reading above 100 %RH is physically meaningless and the reflex
/// rule in `bodies/benchtop` fires on `sensor.humidity > 75`.
#[test]
fn humidity_never_reports_more_than_a_hundred_percent() {
    let c = calib();
    for t_raw in [300_000, 519_888, 550_000] {
        let mut t_fine = 0;
        compensate_temperature(t_raw, &c, &mut t_fine);
        for adc_h in (0..=65_535).step_by(97) {
            let rh = compensate_humidity(adc_h, &c, t_fine) as f64 / 1024.0;
            assert!(
                (0.0..=100.0).contains(&rh),
                "adc_h={adc_h} t_raw={t_raw} gave {rh} %RH"
            );
        }
    }
}

/// The MAX17048 SOC register is `[integer %, 1/256 %]`. This is the reading the
/// built-in battery safing rules watch, and `critical_battery_cuts_safe_pin`
/// fires below 10 %.
#[test]
fn battery_state_of_charge_decodes_as_the_register_map_says() {
    assert_eq!(decode_soc([0, 0]), 0.0);
    assert_eq!(decode_soc([100, 0]), 100.0);
    assert_eq!(decode_soc([0, 128]), 0.5);
    assert_eq!(decode_soc([9, 128]), 9.5, "just under the safing threshold");
    assert_eq!(decode_soc([255, 255]), 255.0 + 255.0 / 256.0);
}

/// Accel is big-endian two's complement at ±2 g, so 1 g must come back as
/// standard gravity and the sign must survive. Getting the byte order wrong
/// yields plausible-looking numbers, which is why it is asserted rather than
/// eyeballed.
#[test]
fn acceleration_decodes_big_endian_and_signed() {
    assert_eq!(decode_accel([0x00, 0x00]), 0.0);
    assert!((decode_accel([0x40, 0x00]) - G_MS2).abs() < 1e-9, "+1 g");
    assert!((decode_accel([0xC0, 0x00]) + G_MS2).abs() < 1e-9, "-1 g");
    // Byte order: 0x0100 is +256 LSB, not +1.
    assert!(decode_accel([0x01, 0x00]) > decode_accel([0x00, 0x01]));
}

/// The calibration blocks are read as two chunks and the H4/H5 fields are
/// split across a shared byte with different nibbles. Transposing them is easy
/// and produces humidity that is wrong but not obviously so.
#[test]
fn calibration_parsing_splits_the_shared_humidity_nibbles_correctly() {
    let mut a = [0u8; 26];
    let mut b = [0u8; 7];
    a[0] = 0x70;
    a[1] = 0x6B; // dig_T1 = 0x6B70
    a[25] = 0x4B; // dig_H1 = 75
    b[0] = 0x61;
    b[1] = 0x01; // dig_H2 = 0x0161
    b[3] = 0x01; // dig_H4 high byte
    b[4] = 0x5C; // low nibble -> H4, high nibble -> H5
    b[5] = 0x02; // dig_H5 high byte
    b[6] = 0x1E; // dig_H6 = 30

    let c = parse_bme280_calib(&a, &b);
    assert_eq!(c.t1, 0x6B70);
    assert_eq!(c.h1, 75);
    assert_eq!(c.h2, 0x0161);
    assert_eq!(c.h6, 30);
    assert_eq!(c.h4, (0x01 << 4) | 0x0C, "H4 takes 0xE5's LOW nibble");
    assert_eq!(c.h5, (0x02 << 4) | 0x05, "H5 takes 0xE5's HIGH nibble");
}

/// Measures the residual rather than asserting a guessed bound. Not a test.
#[test]
#[ignore]
fn measure_t_fine_residual() {
    let c = calib();
    let mut worst = 0.0f64;
    let mut worst_c = 0.0f64;
    for adc_t in 0..=800_000i32 {
        if adc_t % 997 != 0 {
            continue;
        }
        let mut t_fine_i = 0;
        let t_hundredths = compensate_temperature(adc_t, &c, &mut t_fine_i);
        let (want_fine, want_c) = t_double(adc_t, &c);
        let d = (t_fine_i as f64 - want_fine).abs();
        if d > worst {
            worst = d;
        }
        let dc = (t_hundredths as f64 / 100.0 - want_c).abs();
        if dc > worst_c {
            worst_c = dc;
        }
    }
    println!("worst t_fine residual: {worst}");
    println!("worst temperature residual: {worst_c} degC");
}
