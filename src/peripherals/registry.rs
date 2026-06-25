//! Hardware board registry.
//!
//! Maps USB VID/PID pairs to known board descriptions, transport types, and
//! capability lists. Used by the peripheral subsystem to auto-identify boards
//! when they are plugged in via USB.
//!
//! # Capability Tokens
//! | Token | Description |
//! |---|---|
//! | `gpio` | Digital GPIO read/write |
//! | `analog_read` | Analog-to-digital conversion |
//! | `analog_write` | PWM output via analogWrite |
//! | `i2c` | I2C master bus |
//! | `spi` | SPI master bus |
//! | `pwm` | Hardware PWM channels |
//! | `camera_capture` | Camera still image capture |
//! | `audio_sample` | Microphone / audio sampling |
//! | `sensor_read` | Environmental sensor (temp, humidity, pressure) |
//! | `rtt` | SEGGER RTT debug channel (probe-rs) |
//! | `flash` | Firmware flash via debug probe |
//! | `ble` | Bluetooth Low Energy |
//! | `wifi` | Wi-Fi networking |
//! | `can` | CAN bus interface |
//! | `dac` | Digital-to-analog conversion |
//! | `cuda` | NVIDIA CUDA GPU compute |
//! | `display` | Integrated display output |
//! | `touch` | Capacitive or resistive touch input |
//! | `lora` | LoRa / LoRaWAN long-range radio |
//! | `gps` | GNSS / GPS receiver |
//! | `nfc` | Near-field communication reader |
//! | `rfid` | RFID reader (125 kHz / 13.56 MHz) |
//! | `subghz` | Sub-GHz ISM-band radio |
//! | `infrared` | Infrared transceiver |
//! | `imu` | Inertial measurement unit (accel + gyro) |
//! | `microsd` | microSD card storage |
//! | `actuate` | Servo / motor actuation (Movement suite) |
//! | `audio_output` | Speaker / audio output (Audio suite act side) |
//! | `cellular` | Cellular (LTE/4G) modem link (Comms suite) |
//!
//! Additional capability tokens (AI accelerators, radios, etc.) are tracked in
//! `docs/V2-HARDWARE-ECOSYSTEM.md` and added as boards that use them land.
//!
//! # Connector Ecosystems
//!
//! Vendors make hardware composable through standard connectors. The
//! [`Connector`] enum lets a board advertise which ports it exposes and an
//! accessory declare how it attaches, so the deployment advisor can match
//! accessories to boards by physical connector — not just by capability.
//!
//! Note that **Qwiic** (SparkFun) and **STEMMA QT** (Adafruit) are the same
//! 4-pin JST-SH I2C connector and are cross-compatible; [`Connector::mates_with`]
//! encodes that equivalence.
//!
//! # Export
//!
//! The whole registry serializes to a stable, language-agnostic JSON document
//! via [`registry_json`] (canonical generator: the `emit-registry` binary).
//! That JSON is the single source of truth consumed by sibling projects (the
//! OBC deployment generator, Accelerapp) so the hardware catalog is never
//! re-typed in another language. Only `Serialize` is derived — the registry's
//! `&'static` fields cannot be `Deserialize`d into borrowed data.

use serde::Serialize;

/// A physical connector / expansion ecosystem exposed by a board or used by an
/// accessory.
///
/// Used to match accessories to boards. Most matching is exact, with one
/// electrical equivalence: `Qwiic` and `StemmaQt` are the same I2C connector
/// (see [`Connector::mates_with`]). Serializes to a stable lowercase token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Connector {
    /// Seeed / M5Stack 4-pin Grove connector.
    #[serde(rename = "grove")]
    Grove,
    /// SparkFun Qwiic — 4-pin JST-SH I2C (electrically == STEMMA QT).
    #[serde(rename = "qwiic")]
    Qwiic,
    /// Adafruit STEMMA QT — 4-pin JST-SH I2C (electrically == Qwiic).
    #[serde(rename = "stemma_qt")]
    StemmaQt,
    /// Adafruit STEMMA — 3-pin JST analog/digital/PWM.
    #[serde(rename = "stemma")]
    Stemma,
    /// M5Stack stacking M-Bus.
    #[serde(rename = "mbus")]
    MBus,
    /// Adafruit Feather / FeatherWing header.
    #[serde(rename = "featherwing")]
    FeatherWing,
    /// Digilent Pmod.
    #[serde(rename = "pmod")]
    Pmod,
    /// Raspberry Pi 40-pin HAT header.
    #[serde(rename = "hat_pi")]
    HatPi,
    /// Bare header pins / castellated pads / solder (no standard connector).
    #[serde(rename = "bare")]
    Bare,
}

impl Connector {
    /// Whether this connector carries an I2C bus on the standard 4-pin pinout.
    ///
    /// `Qwiic` and `StemmaQt` are the same connector, so both return `true`.
    pub const fn is_i2c(self) -> bool {
        matches!(self, Connector::Qwiic | Connector::StemmaQt)
    }

    /// Whether an accessory using `self` can physically attach to a board port
    /// of type `port`.
    ///
    /// Exact match, plus the Qwiic ≡ STEMMA QT equivalence.
    pub const fn mates_with(self, port: Connector) -> bool {
        // `==` is not const for enums on all toolchains; compare via is_i2c +
        // discriminant match.
        (self.is_i2c() && port.is_i2c()) || self.same_kind(port)
    }

    const fn same_kind(self, other: Connector) -> bool {
        matches!(
            (self, other),
            (Connector::Grove, Connector::Grove)
                | (Connector::Qwiic, Connector::Qwiic)
                | (Connector::StemmaQt, Connector::StemmaQt)
                | (Connector::Stemma, Connector::Stemma)
                | (Connector::MBus, Connector::MBus)
                | (Connector::FeatherWing, Connector::FeatherWing)
                | (Connector::Pmod, Connector::Pmod)
                | (Connector::HatPi, Connector::HatPi)
                | (Connector::Bare, Connector::Bare)
        )
    }
}

/// Describes a known hardware board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoardInfo {
    /// USB Vendor ID.
    pub vid: u16,
    /// USB Product ID.
    pub pid: u16,
    /// Short board name used in config files (e.g., `"nucleo-f401re"`).
    pub name: &'static str,
    /// Human-readable architecture description.
    pub architecture: Option<&'static str>,
    /// Transport type: `"serial"`, `"native"`, `"probe"`, or `"bridge"`.
    pub transport: &'static str,
    /// Capability tokens supported by this board.
    pub capabilities: &'static [&'static str],
    /// Manufacturer / brand (e.g., `"Espressif"`, `"Seeed Studio"`).
    pub vendor: &'static str,
    /// Product family / line within the vendor (e.g., `"XIAO"`, `"Nucleo-64"`).
    pub ecosystem: &'static str,
    /// Expansion connectors this board exposes (for accessory matching).
    pub connectors: &'static [Connector],
}

/// Complete registry of known boards.
///
/// VID assignments:
/// - `0x0483` = STMicroelectronics
/// - `0x2341` = Arduino
/// - `0x10c4` = Silicon Labs (CP210x)
/// - `0x1a86` = WCH (CH340/CH343)
/// - `0x303a` = Espressif Systems
/// - `0x2207` = Rockchip
/// - `0x0403` = FTDI
/// - `0x2e8a` = Raspberry Pi Foundation
/// - `0x2109` = VIA Labs (used by RPi 4/5 USB hub)
/// - `0x1366` = SEGGER (J-Link / nRF DK)
/// - `0x0955` = NVIDIA
/// - `0x1d6b` = Linux Foundation (gadget devices)
/// - `0x16c0` = PJRC (Teensy)
/// - `0x2886` = Seeed Studio
/// - `0x2b04` = Sipeed
pub static KNOWN_BOARDS: &[BoardInfo] = &[
    // ── STM32 Nucleo ──────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x0483,
        pid: 0x374b,
        name: "nucleo-f401re",
        architecture: Some("ARM Cortex-M4 @ 84 MHz (STM32F401RE)"),
        transport: "probe",
        capabilities: &[
            "gpio",
            "analog_read",
            "analog_write",
            "i2c",
            "spi",
            "pwm",
            "rtt",
            "flash",
        ],
        vendor: "STMicroelectronics",
        ecosystem: "Nucleo-64",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x0483,
        pid: 0x374e,
        name: "nucleo-f411re",
        architecture: Some("ARM Cortex-M4 @ 100 MHz (STM32F411RE)"),
        transport: "probe",
        capabilities: &[
            "gpio",
            "analog_read",
            "analog_write",
            "i2c",
            "spi",
            "pwm",
            "rtt",
            "flash",
        ],
        vendor: "STMicroelectronics",
        ecosystem: "Nucleo-64",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x0483,
        pid: 0x374f,
        name: "nucleo-l476rg",
        architecture: Some("ARM Cortex-M4 @ 80 MHz (STM32L476RG, ultra-low-power)"),
        transport: "probe",
        capabilities: &[
            "gpio",
            "analog_read",
            "analog_write",
            "i2c",
            "spi",
            "pwm",
            "rtt",
            "flash",
        ],
        vendor: "STMicroelectronics",
        ecosystem: "Nucleo-64",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x0483,
        pid: 0x374d,
        name: "nucleo-h743zi",
        architecture: Some("ARM Cortex-M7 @ 480 MHz (STM32H743ZI)"),
        transport: "probe",
        capabilities: &[
            "gpio",
            "analog_read",
            "analog_write",
            "i2c",
            "spi",
            "pwm",
            "rtt",
            "flash",
        ],
        vendor: "STMicroelectronics",
        ecosystem: "Nucleo-144",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x0483,
        pid: 0x374c,
        name: "nucleo-g474re",
        architecture: Some("ARM Cortex-M4 @ 170 MHz (STM32G474RE, HRTIM)"),
        transport: "probe",
        capabilities: &[
            "gpio",
            "analog_read",
            "analog_write",
            "i2c",
            "spi",
            "pwm",
            "rtt",
            "flash",
        ],
        vendor: "STMicroelectronics",
        ecosystem: "Nucleo-64",
        connectors: &[Connector::Bare],
    },
    // ── Arduino ───────────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x2341,
        pid: 0x0043,
        name: "arduino-uno",
        architecture: Some("AVR ATmega328P @ 16 MHz"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "analog_write"],
        vendor: "Arduino",
        ecosystem: "Uno",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x2341,
        pid: 0x0001,
        name: "arduino-uno",
        architecture: Some("AVR ATmega328P @ 16 MHz (legacy)"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "analog_write"],
        vendor: "Arduino",
        ecosystem: "Uno",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x2341,
        pid: 0x0078,
        name: "arduino-uno-q",
        architecture: Some("Arduino Uno Q / ATmega328P"),
        transport: "bridge",
        capabilities: &["gpio", "analog_read", "analog_write"],
        vendor: "Arduino",
        ecosystem: "Uno",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x2341,
        pid: 0x0042,
        name: "arduino-mega",
        architecture: Some("AVR ATmega2560 @ 16 MHz"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "analog_write"],
        vendor: "Arduino",
        ecosystem: "Mega",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x2341,
        pid: 0x0036,
        name: "arduino-leonardo",
        architecture: Some("AVR ATmega32U4 @ 16 MHz (native USB)"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "analog_write"],
        vendor: "Arduino",
        ecosystem: "Leonardo",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x2341,
        pid: 0x0058,
        name: "arduino-nano-every",
        architecture: Some("AVR ATmega4809 @ 20 MHz"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "analog_write"],
        vendor: "Arduino",
        ecosystem: "Nano",
        connectors: &[Connector::Bare],
    },
    // Arduino Nano clone with CH340 USB-UART (extremely common)
    BoardInfo {
        vid: 0x1a86,
        pid: 0x7523,
        name: "arduino-nano",
        architecture: Some("AVR ATmega328P @ 16 MHz (CH340 USB-UART)"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "analog_write"],
        vendor: "Arduino",
        ecosystem: "Nano",
        connectors: &[Connector::Bare],
    },
    // ── USB-UART Bridges ──────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x10c4,
        pid: 0xea60,
        name: "cp2102",
        architecture: Some("Silicon Labs CP2102 USB-UART bridge"),
        transport: "serial",
        capabilities: &[],
        vendor: "Silicon Labs",
        ecosystem: "USB-UART",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x10c4,
        pid: 0xea70,
        name: "cp2102n",
        architecture: Some("Silicon Labs CP2102N USB-UART bridge"),
        transport: "serial",
        capabilities: &[],
        vendor: "Silicon Labs",
        ecosystem: "USB-UART",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x0403,
        pid: 0x6001,
        name: "ftdi-ft232",
        architecture: Some("FTDI FT232 USB-UART bridge"),
        transport: "serial",
        capabilities: &[],
        vendor: "FTDI",
        ecosystem: "USB-UART",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x0403,
        pid: 0x6015,
        name: "ftdi-ft231x",
        architecture: Some("FTDI FT231X USB-UART bridge"),
        transport: "serial",
        capabilities: &[],
        vendor: "FTDI",
        ecosystem: "USB-UART",
        connectors: &[Connector::Bare],
    },
    // ── ESP32 ─────────────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x1a86,
        pid: 0x55d4,
        name: "esp32",
        architecture: Some("ESP32 Xtensa LX6 @ 240 MHz (CH340)"),
        transport: "serial",
        capabilities: &["gpio"],
        vendor: "Espressif",
        ecosystem: "ESP32",
        connectors: &[Connector::Bare],
    },
    // ── ESP32-S3 ──────────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "esp32-s3",
        architecture: Some("ESP32-S3 Xtensa LX7 @ 240 MHz (native USB)"),
        transport: "serial",
        capabilities: &["gpio", "camera_capture", "audio_sample", "sensor_read"],
        vendor: "Espressif",
        ecosystem: "ESP32-S3",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x10c4,
        pid: 0xea60,
        name: "esp32-s3",
        architecture: Some("ESP32-S3 (CP2102 USB-UART)"),
        transport: "serial",
        capabilities: &["gpio", "camera_capture", "audio_sample", "sensor_read"],
        vendor: "Espressif",
        ecosystem: "ESP32-S3",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x1a86,
        pid: 0x55d3,
        name: "esp32-s3",
        architecture: Some("ESP32-S3 (CH343 USB-UART)"),
        transport: "serial",
        capabilities: &["gpio", "camera_capture", "audio_sample", "sensor_read"],
        vendor: "Espressif",
        ecosystem: "ESP32-S3",
        connectors: &[Connector::Bare],
    },
    // ── NanoPi Neo3 ───────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x2207,
        pid: 0x330c,
        name: "nanopi-neo3",
        architecture: Some("Rockchip RK3328 quad-core ARM Cortex-A53 @ 1.5 GHz (AArch64)"),
        transport: "native",
        capabilities: &["gpio", "i2c", "spi", "pwm"],
        vendor: "FriendlyELEC",
        ecosystem: "NanoPi",
        connectors: &[Connector::Bare],
    },
    // ── Raspberry Pi ──────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x2e8a,
        pid: 0x0003,
        name: "raspberry-pi-pico",
        architecture: Some("RP2040 dual-core ARM Cortex-M0+ @ 133 MHz"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "pwm"],
        vendor: "Raspberry Pi",
        ecosystem: "Pico",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x2e8a,
        pid: 0x000a,
        name: "raspberry-pi-pico-w",
        architecture: Some("RP2040 dual-core ARM Cortex-M0+ @ 133 MHz (Wi-Fi)"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "pwm"],
        vendor: "Raspberry Pi",
        ecosystem: "Pico",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x2e8a,
        pid: 0x0004,
        name: "raspberry-pi-pico2",
        architecture: Some("RP2350 dual-core ARM Cortex-M33 @ 150 MHz"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "pwm"],
        vendor: "Raspberry Pi",
        ecosystem: "Pico",
        connectors: &[Connector::Bare],
    },
    // Raspberry Pi 4 / 5 (USB hub VID when used as USB device / OTG)
    BoardInfo {
        vid: 0x2109,
        pid: 0x0817,
        name: "raspberry-pi-4",
        architecture: Some("BCM2711 quad-core ARM Cortex-A72 @ 1.8 GHz (AArch64)"),
        transport: "native",
        capabilities: &["gpio", "i2c", "spi", "pwm", "camera_capture"],
        vendor: "Raspberry Pi",
        ecosystem: "Raspberry Pi",
        connectors: &[Connector::HatPi],
    },
    BoardInfo {
        vid: 0x2109,
        pid: 0x0820,
        name: "raspberry-pi-5",
        architecture: Some("BCM2712 quad-core ARM Cortex-A76 @ 2.4 GHz (AArch64)"),
        transport: "native",
        capabilities: &["gpio", "i2c", "spi", "pwm", "camera_capture"],
        vendor: "Raspberry Pi",
        ecosystem: "Raspberry Pi",
        connectors: &[Connector::HatPi],
    },
    // ── ESP32-C3 ──────────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "esp32-c3",
        architecture: Some("ESP32-C3 RISC-V single-core @ 160 MHz (native USB)"),
        transport: "serial",
        capabilities: &["gpio", "i2c", "spi", "wifi", "ble"],
        vendor: "Espressif",
        ecosystem: "ESP32-C3",
        connectors: &[Connector::Bare],
    },
    // ── nRF52840 DK ───────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x1366,
        pid: 0x1015,
        name: "nrf52840-dk",
        architecture: Some("Nordic nRF52840 ARM Cortex-M4F @ 64 MHz (BLE 5.0)"),
        transport: "serial",
        capabilities: &["gpio", "i2c", "spi", "ble", "pwm"],
        vendor: "Nordic Semiconductor",
        ecosystem: "nRF DK",
        connectors: &[Connector::Bare],
    },
    // ── Arduino Nano 33 BLE ───────────────────────────────────────────────────
    BoardInfo {
        vid: 0x2341,
        pid: 0x805a,
        name: "arduino-nano-33-ble",
        architecture: Some("nRF52840 ARM Cortex-M4F @ 64 MHz (BLE, IMU)"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "ble", "sensor_read"],
        vendor: "Arduino",
        ecosystem: "Nano",
        connectors: &[Connector::Bare],
    },
    // ── Teensy 4.1 ────────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x16c0,
        pid: 0x0483,
        name: "teensy-4.1",
        architecture: Some("NXP i.MX RT1062 ARM Cortex-M7 @ 600 MHz"),
        transport: "serial",
        capabilities: &[
            "gpio",
            "analog_read",
            "analog_write",
            "i2c",
            "spi",
            "pwm",
            "can",
        ],
        vendor: "PJRC",
        ecosystem: "Teensy",
        connectors: &[Connector::Bare],
    },
    // ── BeagleBone Black ──────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x1d6b,
        pid: 0x0104,
        name: "beaglebone-black",
        architecture: Some("TI AM3358 ARM Cortex-A8 @ 1 GHz"),
        transport: "native",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "pwm", "can"],
        vendor: "BeagleBoard",
        ecosystem: "BeagleBone",
        connectors: &[Connector::Bare],
    },
    // ── NVIDIA Jetson Nano ────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x0955,
        pid: 0x7020,
        name: "jetson-nano",
        architecture: Some(
            "NVIDIA Tegra X1 quad-core ARM Cortex-A57 @ 1.43 GHz (128-core Maxwell GPU)",
        ),
        transport: "native",
        capabilities: &["gpio", "i2c", "spi", "pwm", "camera_capture", "cuda"],
        vendor: "NVIDIA",
        ecosystem: "Jetson",
        connectors: &[Connector::Bare],
    },
    // ── STM32 Discovery (H7) ─────────────────────────────────────────────────
    BoardInfo {
        vid: 0x0483,
        pid: 0x3758,
        name: "stm32h7-discovery",
        architecture: Some("ARM Cortex-M7 @ 480 MHz (STM32H750, external flash)"),
        transport: "probe",
        capabilities: &[
            "gpio",
            "analog_read",
            "analog_write",
            "i2c",
            "spi",
            "pwm",
            "rtt",
            "flash",
            "dac",
        ],
        vendor: "STMicroelectronics",
        ecosystem: "Discovery",
        connectors: &[Connector::Bare],
    },
    // ── Waveshare ESP32-S3-Touch-LCD-2.1 ─────────────────────────────────────
    // 2.1-inch round touch display with ESP32-S3, integrated speaker, and
    // capacitive multi-touch.  Acts as the display/sound interface node.
    // Native USB VID=0x303a; PID=0x8135 (Waveshare Touch LCD 2.1 variant).
    BoardInfo {
        vid: 0x303a,
        pid: 0x8135,
        name: "waveshare-esp32-s3-touch-lcd-2.1",
        architecture: Some(
            "ESP32-S3 Xtensa LX7 dual-core @ 240 MHz, 2.1\" round touch LCD, I2S speaker",
        ),
        transport: "serial",
        capabilities: &[
            "gpio",
            "i2c",
            "spi",
            "wifi",
            "ble",
            "audio_sample",
            "display",
            "touch",
        ],
        vendor: "Waveshare",
        ecosystem: "ESP32-S3 Touch LCD",
        connectors: &[Connector::Bare],
    },
    // ── Seeed XIAO ESP32S3-Sense ──────────────────────────────────────────────
    // Compact ESP32-S3 module with OV2640 camera, PDM microphone, and
    // expandable microSD.  Used as the primary vision node.
    // USB VID=0x2886 (Seeed Studio), PID=0x0058 (XIAO ESP32-S3 Sense).
    // The bare module uses castellated pads; the XIAO Expansion Board adds
    // Grove and Qwiic ports (see scout proposals for those as separate entries).
    BoardInfo {
        vid: 0x2886,
        pid: 0x0058,
        name: "xiao-esp32s3-sense",
        architecture: Some(
            "ESP32-S3 Xtensa LX7 dual-core @ 240 MHz, OV2640 camera, PDM microphone",
        ),
        transport: "serial",
        capabilities: &[
            "gpio",
            "analog_read",
            "i2c",
            "spi",
            "wifi",
            "ble",
            "camera_capture",
            "audio_sample",
            "sensor_read",
        ],
        vendor: "Seeed Studio",
        ecosystem: "XIAO",
        connectors: &[Connector::Bare],
    },
    // ── Sipeed 6+1 Mic Array ──────────────────────────────────────────────────
    // Circular microphone array with 6 peripheral mics and 1 center mic,
    // powered by an STM32F103 MCU.  Appears as a USB audio device and
    // provides far-field voice capture for the listening node.
    // USB VID=0x2b04 (Sipeed), PID=0x00fe (Mic Array v2).
    BoardInfo {
        vid: 0x2b04,
        pid: 0x00fe,
        name: "sipeed-6plus1-mic-array",
        architecture: Some("STM32F103 @ 72 MHz, 6+1 MEMS microphone array with USB audio (UAC1)"),
        transport: "serial",
        capabilities: &["audio_sample", "gpio"],
        vendor: "Sipeed",
        ecosystem: "MaixSense",
        connectors: &[Connector::Bare],
    },
    // ── Accelerapp-sourced hardware (v2.0 seed) ───────────────────────────────
    // Boards harvested from the sibling Accelerapp project. NOTE: most ESP32
    // boards share either a USB-bridge VID/PID (CP210x 0x10c4:0xea60, CH340
    // 0x1a86:0x7523) or the native-USB ESP32-S3 id (0x303a:0x1001), so USB
    // auto-identification cannot uniquely distinguish them. These entries are
    // selected primarily by `name` in deployment config (DeploymentHardwareConfig);
    // the shared VID/PID is recorded for reference, and `lookup_board` returns the
    // first VID/PID match. Flipper Zero is the one entry with a unique VID/PID.
    BoardInfo {
        vid: 0x0483,
        pid: 0x5740,
        name: "flipper-zero",
        architecture: Some("STM32WB55 ARM Cortex-M4 @ 64 MHz (multi-tool; USB CDC)"),
        transport: "serial",
        capabilities: &["gpio", "ble", "nfc", "rfid", "subghz", "infrared"],
        vendor: "Flipper Devices",
        ecosystem: "Flipper Zero",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x10c4,
        pid: 0xea60,
        name: "m5stack-core2",
        architecture: Some(
            "ESP32-D0WDQ6 @ 240 MHz, 2.0\" ILI9342C touch LCD, MPU6886 IMU (CP2104; shared VID/PID)",
        ),
        transport: "serial",
        capabilities: &[
            "gpio", "i2c", "spi", "wifi", "ble", "display", "touch", "imu", "microsd",
        ],
        vendor: "M5Stack",
        ecosystem: "M5 Core",
        connectors: &[Connector::Grove, Connector::MBus],
    },
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "m5stack-atom-s3",
        architecture: Some("ESP32-S3 @ 240 MHz, 0.85\" LCD, compact (native USB; shared VID/PID)"),
        transport: "serial",
        capabilities: &["gpio", "i2c", "spi", "wifi", "ble", "display"],
        vendor: "M5Stack",
        ecosystem: "M5 Atom",
        connectors: &[Connector::Grove],
    },
    BoardInfo {
        vid: 0x1a86,
        pid: 0x7523,
        name: "esp32-cam",
        architecture: Some(
            "ESP32 + OV2640 camera, microSD (AI-Thinker; flashed via external USB-UART; VID/PID is a common CH340 programmer)",
        ),
        transport: "serial",
        capabilities: &["gpio", "wifi", "camera_capture", "microsd"],
        vendor: "AI-Thinker",
        ecosystem: "ESP32-CAM",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "esp32-s3-cam",
        architecture: Some(
            "ESP32-S3 + OV2640/OV5640 camera, microSD, PSRAM (native USB; shared VID/PID)",
        ),
        transport: "serial",
        capabilities: &["gpio", "i2c", "wifi", "ble", "camera_capture", "microsd"],
        vendor: "Espressif",
        ecosystem: "ESP32-S3",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x1a86,
        pid: 0x7523,
        name: "cyd-esp32-2432s028r",
        architecture: Some(
            "ESP32 'Cheap Yellow Display' (ESP32-2432S028R): ILI9341 320x240 + XPT2046 resistive touch, microSD (CH340; shared VID/PID)",
        ),
        transport: "serial",
        capabilities: &[
            "gpio", "i2c", "spi", "wifi", "ble", "display", "touch", "microsd",
        ],
        vendor: "Sunton",
        ecosystem: "Cheap Yellow Display",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x10c4,
        pid: 0xea60,
        name: "lilygo-t-beam",
        architecture: Some(
            "ESP32 + SX1276/SX1262 LoRa + NEO-6M GPS (Meshtastic-compatible; CP210x; shared VID/PID)",
        ),
        transport: "serial",
        capabilities: &["gpio", "i2c", "wifi", "ble", "lora", "gps"],
        vendor: "LILYGO",
        ecosystem: "T-Beam",
        connectors: &[Connector::Bare],
    },
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "heltec-wifi-lora-32-v3",
        architecture: Some(
            "ESP32-S3 + SX1262 LoRa + 0.96\" OLED (Meshtastic-compatible; native USB; shared VID/PID)",
        ),
        transport: "serial",
        capabilities: &["gpio", "i2c", "spi", "wifi", "ble", "lora", "display"],
        vendor: "Heltec",
        ecosystem: "WiFi LoRa 32",
        connectors: &[Connector::Bare],
    },
];

/// Look up a board by USB VID and PID.
pub fn lookup_board(vid: u16, pid: u16) -> Option<&'static BoardInfo> {
    KNOWN_BOARDS.iter().find(|b| b.vid == vid && b.pid == pid)
}

/// Return all known board entries.
pub fn known_boards() -> &'static [BoardInfo] {
    KNOWN_BOARDS
}

/// Return all boards that support a given capability.
pub fn boards_with_capability(capability: &str) -> Vec<&'static BoardInfo> {
    KNOWN_BOARDS
        .iter()
        .filter(|b| b.capabilities.contains(&capability))
        .collect()
}

/// Return all boards that use a given transport.
pub fn boards_with_transport(transport: &str) -> Vec<&'static BoardInfo> {
    KNOWN_BOARDS
        .iter()
        .filter(|b| b.transport == transport)
        .collect()
}

/// Return all boards from a given vendor (case-insensitive).
pub fn boards_by_vendor(vendor: &str) -> Vec<&'static BoardInfo> {
    KNOWN_BOARDS
        .iter()
        .filter(|b| b.vendor.eq_ignore_ascii_case(vendor))
        .collect()
}

/// Return all boards that expose a given connector.
pub fn boards_with_connector(connector: Connector) -> Vec<&'static BoardInfo> {
    KNOWN_BOARDS
        .iter()
        .filter(|b| b.connectors.contains(&connector))
        .collect()
}

// ── Accessory Registry ────────────────────────────────────────────────────────

/// Describes a known I2C/SPI accessory or add-on module.
///
/// Accessories are peripheral devices that attach to a host board via I2C or SPI.
/// They don't have their own USB VID/PID — they are discovered by scanning the bus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccessoryInfo {
    /// Short name used in config and sensor references (e.g., `"bme280"`).
    pub name: &'static str,
    /// Human-readable description.
    pub description: &'static str,
    /// Required bus: `"i2c"` or `"spi"`.
    pub bus: &'static str,
    /// Default I2C address (if I2C), `None` for SPI-only devices.
    pub default_i2c_addr: Option<u8>,
    /// Capability tokens this accessory provides.
    pub capabilities: &'static [&'static str],
    /// Compatible host boards (empty = universal / works with any board that has the bus).
    pub compatible_boards: &'static [&'static str],
    /// Physical connector the accessory attaches through. `Bare` for raw
    /// chips/breakouts wired by hand; `Qwiic`/`StemmaQt` for plug-in I2C modules.
    pub connector: Connector,
}

/// Registry of known I2C/SPI accessories and sensor modules.
pub static KNOWN_ACCESSORIES: &[AccessoryInfo] = &[
    // ── Environmental Sensors ─────────────────────────────────────────────────
    AccessoryInfo {
        name: "bme280",
        description: "Bosch BME280 — temperature, humidity, and pressure sensor",
        bus: "i2c",
        default_i2c_addr: Some(0x76),
        capabilities: &["sensor_read"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    AccessoryInfo {
        name: "bmp388",
        description: "Bosch BMP388 — high-accuracy barometric pressure and temperature",
        bus: "i2c",
        default_i2c_addr: Some(0x77),
        capabilities: &["sensor_read"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    AccessoryInfo {
        name: "sht31",
        description: "Sensirion SHT31 — high-accuracy temperature and humidity",
        bus: "i2c",
        default_i2c_addr: Some(0x44),
        capabilities: &["sensor_read"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    AccessoryInfo {
        name: "aht20",
        description: "ASAIR AHT20 — temperature and humidity sensor",
        bus: "i2c",
        default_i2c_addr: Some(0x38),
        capabilities: &["sensor_read"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    // ── Motion / IMU Sensors ──────────────────────────────────────────────────
    AccessoryInfo {
        name: "mpu6050",
        description: "InvenSense MPU-6050 — 6-axis accelerometer + gyroscope",
        bus: "i2c",
        default_i2c_addr: Some(0x68),
        capabilities: &["sensor_read"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    AccessoryInfo {
        name: "lsm6ds3",
        description: "ST LSM6DS3 — 6-axis IMU (accelerometer + gyroscope)",
        bus: "i2c",
        default_i2c_addr: Some(0x6A),
        capabilities: &["sensor_read"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    // ── ADC / DAC ─────────────────────────────────────────────────────────────
    AccessoryInfo {
        name: "ads1115",
        description: "TI ADS1115 — 16-bit 4-channel ADC with programmable gain",
        bus: "i2c",
        default_i2c_addr: Some(0x48),
        capabilities: &["analog_read"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    AccessoryInfo {
        name: "mcp4725",
        description: "Microchip MCP4725 — 12-bit single-channel DAC",
        bus: "i2c",
        default_i2c_addr: Some(0x60),
        capabilities: &["dac"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    // ── GPIO Expanders ────────────────────────────────────────────────────────
    AccessoryInfo {
        name: "pcf8574",
        description: "NXP PCF8574 — 8-bit I2C GPIO expander",
        bus: "i2c",
        default_i2c_addr: Some(0x20),
        capabilities: &["gpio"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    AccessoryInfo {
        name: "mcp23017",
        description: "Microchip MCP23017 — 16-bit I2C GPIO expander",
        bus: "i2c",
        default_i2c_addr: Some(0x21),
        capabilities: &["gpio"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    // ── Thermocouple / Temperature ────────────────────────────────────────────
    AccessoryInfo {
        name: "max31855",
        description: "Maxim MAX31855 — thermocouple-to-digital converter (K-type)",
        bus: "spi",
        default_i2c_addr: None,
        capabilities: &["sensor_read"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    AccessoryInfo {
        name: "ds18b20",
        description: "Maxim DS18B20 — 1-Wire digital temperature sensor",
        bus: "onewire",
        default_i2c_addr: None,
        capabilities: &["sensor_read"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    // ── Power Monitoring ──────────────────────────────────────────────────────
    AccessoryInfo {
        name: "ina260",
        description: "TI INA260 — high/low-side current, voltage, and power monitor",
        bus: "i2c",
        default_i2c_addr: Some(0x40),
        capabilities: &["sensor_read"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    // ── Display ───────────────────────────────────────────────────────────────
    AccessoryInfo {
        name: "ssd1306",
        description: "Solomon SSD1306 — 128x64 OLED display",
        bus: "i2c",
        default_i2c_addr: Some(0x3C),
        capabilities: &[],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    // ── Single-Wire / GPIO-Protocol Sensors ───────────────────────────────────
    AccessoryInfo {
        name: "dht22",
        description: "AOSONG DHT22 (AM2302) — capacitive humidity and temperature sensor, single-wire protocol",
        bus: "gpio",
        default_i2c_addr: None,
        capabilities: &["sensor_read"],
        compatible_boards: &[
            "nanopi-neo3",
            "raspberry-pi-4",
            "raspberry-pi-5",
            "esp32-s3",
            "xiao-esp32s3-sense",
            "waveshare-esp32-s3-touch-lcd-2.1",
        ],
        connector: Connector::Bare,
    },
    AccessoryInfo {
        name: "dht11",
        description: "AOSONG DHT11 — basic humidity and temperature sensor, single-wire protocol",
        bus: "gpio",
        default_i2c_addr: None,
        capabilities: &["sensor_read"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    // ── Accelerapp-sourced accessories (v2.0 seed) ────────────────────────────
    AccessoryInfo {
        name: "ov2640",
        description: "OmniVision OV2640 — 2 MP camera image sensor (SCCB control bus)",
        bus: "sccb",
        default_i2c_addr: None,
        capabilities: &["camera_capture"],
        compatible_boards: &["esp32-cam", "esp32-s3-cam", "xiao-esp32s3-sense"],
        connector: Connector::Bare,
    },
    AccessoryInfo {
        name: "ili9341",
        description: "ILITEK ILI9341 — 240x320 SPI TFT LCD controller",
        bus: "spi",
        default_i2c_addr: None,
        capabilities: &["display"],
        compatible_boards: &["cyd-esp32-2432s028r"],
        connector: Connector::Bare,
    },
    AccessoryInfo {
        name: "xpt2046",
        description: "XPT2046 — resistive touch-screen controller (SPI)",
        bus: "spi",
        default_i2c_addr: None,
        capabilities: &["touch"],
        compatible_boards: &["cyd-esp32-2432s028r"],
        connector: Connector::Bare,
    },
    // ── Subsystem-suite hardware (Movement / Audio / Power / Comms) ────────────
    // The actuators, transducers, gauges, and modems the v2.x capability suites
    // drive. Tagged with the suite capability tokens (`actuate`, `audio_output`,
    // `cellular`) so the deployment advisor can match them to suite-enabled nodes.
    AccessoryInfo {
        name: "sg90",
        description: "TowerPro SG90 — micro servo (PWM position control)",
        bus: "pwm",
        default_i2c_addr: None,
        capabilities: &["actuate"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    AccessoryInfo {
        name: "tb6612fng",
        description: "Toshiba TB6612FNG — dual DC motor driver (PWM + direction)",
        bus: "gpio",
        default_i2c_addr: None,
        capabilities: &["actuate"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    AccessoryInfo {
        name: "pca9685",
        description: "NXP PCA9685 — 16-channel 12-bit I2C PWM / servo driver",
        bus: "i2c",
        default_i2c_addr: Some(0x40),
        capabilities: &["actuate", "pwm"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    AccessoryInfo {
        name: "inmp441",
        description: "InvenSense INMP441 — I2S MEMS microphone (audio capture)",
        bus: "i2s",
        default_i2c_addr: None,
        capabilities: &["audio_sample"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    AccessoryInfo {
        name: "max98357a",
        description: "Maxim MAX98357A — I2S Class-D amplifier (speaker output)",
        bus: "i2s",
        default_i2c_addr: None,
        capabilities: &["audio_output"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    AccessoryInfo {
        name: "max17048",
        description: "Maxim MAX17048 — LiPo battery fuel gauge (state of charge)",
        bus: "i2c",
        default_i2c_addr: Some(0x36),
        capabilities: &["sensor_read"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
    AccessoryInfo {
        name: "sim7600",
        description: "SIMCom SIM7600 — LTE Cat-4 cellular modem with GNSS (UART/USB)",
        bus: "uart",
        default_i2c_addr: None,
        capabilities: &["cellular", "gps"],
        compatible_boards: &[],
        connector: Connector::Bare,
    },
];

/// Look up an accessory by name.
pub fn lookup_accessory(name: &str) -> Option<&'static AccessoryInfo> {
    KNOWN_ACCESSORIES.iter().find(|a| a.name == name)
}

/// Return all known accessories.
pub fn known_accessories() -> &'static [AccessoryInfo] {
    KNOWN_ACCESSORIES
}

/// Look up an accessory by its default I2C address.
pub fn accessories_at_address(addr: u8) -> Vec<&'static AccessoryInfo> {
    KNOWN_ACCESSORIES
        .iter()
        .filter(|a| a.default_i2c_addr == Some(addr))
        .collect()
}

/// Return all accessories that provide a given capability.
pub fn accessories_with_capability(capability: &str) -> Vec<&'static AccessoryInfo> {
    KNOWN_ACCESSORIES
        .iter()
        .filter(|a| a.capabilities.contains(&capability))
        .collect()
}

/// Whether `accessory` can physically attach to `board` via any of the board's
/// connectors (honoring the Qwiic ≡ STEMMA QT equivalence).
///
/// `Bare` accessories are treated as universally wireable to any board that has
/// the matching bus, so this returns `true` for `Bare`-on-anything.
pub fn board_accepts_accessory(board: &BoardInfo, accessory: &AccessoryInfo) -> bool {
    if accessory.connector == Connector::Bare {
        return true;
    }
    board
        .connectors
        .iter()
        .any(|&port| accessory.connector.mates_with(port))
}

/// Return all accessories that can attach to a board (by connector match).
pub fn accessories_for_board(board: &BoardInfo) -> Vec<&'static AccessoryInfo> {
    KNOWN_ACCESSORIES
        .iter()
        .filter(|a| board_accepts_accessory(board, a))
        .collect()
}

// ── Registry export (single source of truth) ───────────────────────────────────

/// Schema version of the exported `registry.json` document. Bump on any
/// breaking change to the serialized shape so older consumers fail loudly.
pub const REGISTRY_SCHEMA_VERSION: u32 = 1;

/// A serializable snapshot of the entire hardware registry.
///
/// This is the canonical, language-agnostic representation consumed by sibling
/// projects (deployment generator, Accelerapp). Generate it with the
/// `emit-registry` binary: `cargo run --bin emit-registry > registry.json`.
#[derive(Debug, Clone, Serialize)]
pub struct RegistrySnapshot {
    /// Schema version (see [`REGISTRY_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// All known boards.
    pub boards: &'static [BoardInfo],
    /// All known accessories.
    pub accessories: &'static [AccessoryInfo],
}

/// Build a snapshot of the current registry.
pub fn registry_snapshot() -> RegistrySnapshot {
    RegistrySnapshot {
        schema_version: REGISTRY_SCHEMA_VERSION,
        boards: KNOWN_BOARDS,
        accessories: KNOWN_ACCESSORIES,
    }
}

/// Serialize the entire registry to a pretty-printed JSON document — the single
/// source of truth other projects consume instead of re-typing the catalog.
pub fn registry_json() -> serde_json::Result<String> {
    serde_json::to_string_pretty(&registry_snapshot())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_nucleo_f401re() {
        let b = lookup_board(0x0483, 0x374b).unwrap();
        assert_eq!(b.name, "nucleo-f401re");
        assert!(b.architecture.unwrap().contains("Cortex-M4"));
        assert!(b.capabilities.contains(&"rtt"));
        assert!(b.capabilities.contains(&"flash"));
        assert_eq!(b.transport, "probe");
    }

    #[test]
    fn lookup_nucleo_h743zi() {
        let b = lookup_board(0x0483, 0x374d).unwrap();
        assert_eq!(b.name, "nucleo-h743zi");
        assert!(b.architecture.unwrap().contains("Cortex-M7"));
        assert_eq!(b.transport, "probe");
    }

    #[test]
    fn lookup_nucleo_g474re() {
        let b = lookup_board(0x0483, 0x374c).unwrap();
        assert_eq!(b.name, "nucleo-g474re");
        assert!(b.architecture.unwrap().contains("HRTIM"));
    }

    #[test]
    fn lookup_arduino_nano_ch340() {
        let b = lookup_board(0x1a86, 0x7523).unwrap();
        assert_eq!(b.name, "arduino-nano");
        assert!(b.capabilities.contains(&"analog_read"));
    }

    #[test]
    fn lookup_arduino_leonardo() {
        let b = lookup_board(0x2341, 0x0036).unwrap();
        assert_eq!(b.name, "arduino-leonardo");
    }

    #[test]
    fn lookup_rpi_pico2() {
        let b = lookup_board(0x2e8a, 0x0004).unwrap();
        assert_eq!(b.name, "raspberry-pi-pico2");
        assert!(b.architecture.unwrap().contains("Cortex-M33"));
    }

    #[test]
    fn lookup_rpi_5() {
        let b = lookup_board(0x2109, 0x0820).unwrap();
        assert_eq!(b.name, "raspberry-pi-5");
        assert!(b.capabilities.contains(&"camera_capture"));
    }

    #[test]
    fn lookup_unknown_returns_none() {
        assert!(lookup_board(0x0000, 0x0000).is_none());
    }

    #[test]
    fn known_boards_not_empty() {
        assert!(!known_boards().is_empty());
    }

    #[test]
    fn lookup_esp32_s3_native_usb() {
        let b = lookup_board(0x303a, 0x1001).unwrap();
        assert_eq!(b.name, "esp32-s3");
        assert!(b.capabilities.contains(&"camera_capture"));
        assert!(b.capabilities.contains(&"audio_sample"));
    }

    #[test]
    fn lookup_nanopi_neo3() {
        let b = lookup_board(0x2207, 0x330c).unwrap();
        assert_eq!(b.name, "nanopi-neo3");
        assert!(b.architecture.unwrap().contains("RK3328"));
        assert!(b.capabilities.contains(&"gpio"));
    }

    #[test]
    fn boards_with_camera_capability() {
        let boards = boards_with_capability("camera_capture");
        assert!(!boards.is_empty());
        assert!(boards.iter().any(|b| b.name == "esp32-s3"));
        assert!(boards.iter().any(|b| b.name.starts_with("raspberry-pi")));
    }

    #[test]
    fn boards_with_probe_transport() {
        let boards = boards_with_transport("probe");
        assert!(!boards.is_empty());
        assert!(boards
            .iter()
            .all(|b| b.name.starts_with("nucleo") || b.name.starts_with("stm32")));
    }

    #[test]
    fn boards_with_rtt_capability() {
        let boards = boards_with_capability("rtt");
        assert!(!boards.is_empty());
        assert!(boards.iter().all(|b| b.transport == "probe"));
    }

    #[test]
    fn all_probe_boards_have_flash_capability() {
        for board in boards_with_transport("probe") {
            assert!(
                board.capabilities.contains(&"flash"),
                "Board {} missing flash capability",
                board.name
            );
        }
    }

    // ── New board tests ───────────────────────────────────────────────────────

    #[test]
    fn lookup_esp32_c3() {
        let boards: Vec<_> = KNOWN_BOARDS
            .iter()
            .filter(|b| b.name == "esp32-c3")
            .collect();
        assert!(!boards.is_empty());
        let b = boards[0];
        assert!(b.architecture.unwrap().contains("RISC-V"));
        assert!(b.capabilities.contains(&"wifi"));
        assert!(b.capabilities.contains(&"ble"));
    }

    #[test]
    fn lookup_nrf52840_dk() {
        let b = lookup_board(0x1366, 0x1015).unwrap();
        assert_eq!(b.name, "nrf52840-dk");
        assert!(b.capabilities.contains(&"ble"));
        assert!(b.architecture.unwrap().contains("nRF52840"));
    }

    #[test]
    fn lookup_arduino_nano_33_ble() {
        let b = lookup_board(0x2341, 0x805a).unwrap();
        assert_eq!(b.name, "arduino-nano-33-ble");
        assert!(b.capabilities.contains(&"ble"));
        assert!(b.capabilities.contains(&"sensor_read"));
    }

    #[test]
    fn lookup_teensy_41() {
        let b = lookup_board(0x16c0, 0x0483).unwrap();
        assert_eq!(b.name, "teensy-4.1");
        assert!(b.architecture.unwrap().contains("Cortex-M7"));
        assert!(b.capabilities.contains(&"can"));
    }

    #[test]
    fn lookup_beaglebone_black() {
        let b = lookup_board(0x1d6b, 0x0104).unwrap();
        assert_eq!(b.name, "beaglebone-black");
        assert!(b.capabilities.contains(&"can"));
        assert_eq!(b.transport, "native");
    }

    #[test]
    fn lookup_jetson_nano() {
        let b = lookup_board(0x0955, 0x7020).unwrap();
        assert_eq!(b.name, "jetson-nano");
        assert!(b.capabilities.contains(&"cuda"));
        assert_eq!(b.transport, "native");
    }

    #[test]
    fn lookup_stm32h7_discovery() {
        let b = lookup_board(0x0483, 0x3758).unwrap();
        assert_eq!(b.name, "stm32h7-discovery");
        assert!(b.architecture.unwrap().contains("Cortex-M7"));
        assert!(b.capabilities.contains(&"dac"));
        assert_eq!(b.transport, "probe");
    }

    #[test]
    fn boards_with_ble_capability() {
        let boards = boards_with_capability("ble");
        assert!(boards.len() >= 3);
        let names: Vec<_> = boards.iter().map(|b| b.name).collect();
        assert!(names.contains(&"nrf52840-dk"));
        assert!(names.contains(&"arduino-nano-33-ble"));
        assert!(names.contains(&"esp32-c3"));
    }

    // ── Accessory tests ───────────────────────────────────────────────────────

    #[test]
    fn accessory_registry_not_empty() {
        assert!(!known_accessories().is_empty());
    }

    #[test]
    fn lookup_bme280_accessory() {
        let a = lookup_accessory("bme280").unwrap();
        assert_eq!(a.bus, "i2c");
        assert_eq!(a.default_i2c_addr, Some(0x76));
        assert!(a.capabilities.contains(&"sensor_read"));
    }

    #[test]
    fn lookup_ads1115_accessory() {
        let a = lookup_accessory("ads1115").unwrap();
        assert_eq!(a.default_i2c_addr, Some(0x48));
        assert!(a.capabilities.contains(&"analog_read"));
    }

    #[test]
    fn lookup_max31855_spi_accessory() {
        let a = lookup_accessory("max31855").unwrap();
        assert_eq!(a.bus, "spi");
        assert_eq!(a.default_i2c_addr, None);
    }

    #[test]
    fn accessories_at_i2c_address_0x76() {
        let accs = accessories_at_address(0x76);
        assert!(!accs.is_empty());
        assert!(accs.iter().any(|a| a.name == "bme280"));
    }

    #[test]
    fn accessories_with_sensor_read_capability() {
        let accs = accessories_with_capability("sensor_read");
        assert!(accs.len() >= 5);
        let names: Vec<_> = accs.iter().map(|a| a.name).collect();
        assert!(names.contains(&"bme280"));
        assert!(names.contains(&"mpu6050"));
        assert!(names.contains(&"bmp388"));
    }

    // ── Subsystem-suite accessory tests ───────────────────────────────────────

    #[test]
    fn movement_actuators_present() {
        let actuators = accessories_with_capability("actuate");
        let names: Vec<_> = actuators.iter().map(|a| a.name).collect();
        assert!(names.contains(&"sg90"));
        assert!(names.contains(&"tb6612fng"));
        assert!(names.contains(&"pca9685"));
    }

    #[test]
    fn audio_output_and_cellular_present() {
        assert!(accessories_with_capability("audio_output")
            .iter()
            .any(|a| a.name == "max98357a"));
        assert!(accessories_with_capability("cellular")
            .iter()
            .any(|a| a.name == "sim7600"));
    }

    #[test]
    fn max17048_fuel_gauge_is_i2c() {
        let a = lookup_accessory("max17048").unwrap();
        assert_eq!(a.bus, "i2c");
        assert_eq!(a.default_i2c_addr, Some(0x36));
        assert!(a.capabilities.contains(&"sensor_read"));
    }

    #[test]
    fn i2s_audio_accessories_have_no_i2c_addr() {
        for name in ["inmp441", "max98357a"] {
            let a = lookup_accessory(name).unwrap();
            assert_eq!(a.bus, "i2s");
            assert_eq!(a.default_i2c_addr, None);
        }
    }

    #[test]
    fn all_i2c_accessories_on_correct_bus() {
        for accessory in known_accessories() {
            if accessory.default_i2c_addr.is_some() {
                assert_eq!(
                    accessory.bus, "i2c",
                    "Accessory {} has I2C address but bus is '{}'",
                    accessory.name, accessory.bus
                );
            }
        }
    }

    #[test]
    fn unknown_accessory_returns_none() {
        assert!(lookup_accessory("nonexistent-sensor").is_none());
    }

    // ── New hardware tests (Phase 13) ─────────────────────────────────────────

    #[test]
    fn lookup_waveshare_esp32s3_touch_lcd() {
        let b = lookup_board(0x303a, 0x8135).unwrap();
        assert_eq!(b.name, "waveshare-esp32-s3-touch-lcd-2.1");
        assert!(b.architecture.unwrap().contains("ESP32-S3"));
        assert!(b.capabilities.contains(&"display"));
        assert!(b.capabilities.contains(&"touch"));
        assert!(b.capabilities.contains(&"audio_sample"));
        assert!(b.capabilities.contains(&"wifi"));
        assert_eq!(b.transport, "serial");
    }

    #[test]
    fn lookup_xiao_esp32s3_sense() {
        let b = lookup_board(0x2886, 0x0058).unwrap();
        assert_eq!(b.name, "xiao-esp32s3-sense");
        assert!(b.architecture.unwrap().contains("OV2640"));
        assert!(b.capabilities.contains(&"camera_capture"));
        assert!(b.capabilities.contains(&"audio_sample"));
        assert!(b.capabilities.contains(&"wifi"));
        assert!(b.capabilities.contains(&"ble"));
        assert_eq!(b.transport, "serial");
    }

    #[test]
    fn lookup_sipeed_mic_array() {
        let b = lookup_board(0x2b04, 0x00fe).unwrap();
        assert_eq!(b.name, "sipeed-6plus1-mic-array");
        assert!(b.architecture.unwrap().contains("STM32F103"));
        assert!(b.capabilities.contains(&"audio_sample"));
        assert_eq!(b.transport, "serial");
    }

    #[test]
    fn lookup_dht22_accessory() {
        let a = lookup_accessory("dht22").unwrap();
        assert_eq!(a.bus, "gpio");
        assert_eq!(a.default_i2c_addr, None);
        assert!(a.capabilities.contains(&"sensor_read"));
        assert!(a.compatible_boards.contains(&"nanopi-neo3"));
        assert!(a.compatible_boards.contains(&"xiao-esp32s3-sense"));
    }

    #[test]
    fn boards_with_display_capability() {
        let boards = boards_with_capability("display");
        assert!(!boards.is_empty());
        assert!(boards
            .iter()
            .any(|b| b.name == "waveshare-esp32-s3-touch-lcd-2.1"));
    }

    #[test]
    fn boards_with_touch_capability() {
        let boards = boards_with_capability("touch");
        assert!(!boards.is_empty());
        assert!(boards
            .iter()
            .any(|b| b.name == "waveshare-esp32-s3-touch-lcd-2.1"));
    }

    #[test]
    fn all_new_boards_are_in_known_boards() {
        let names: Vec<_> = KNOWN_BOARDS.iter().map(|b| b.name).collect();
        assert!(names.contains(&"waveshare-esp32-s3-touch-lcd-2.1"));
        assert!(names.contains(&"xiao-esp32s3-sense"));
        assert!(names.contains(&"sipeed-6plus1-mic-array"));
    }

    // ── Connector ecosystem tests ─────────────────────────────────────────────

    #[test]
    fn connector_exact_match_mates() {
        assert!(Connector::Grove.mates_with(Connector::Grove));
        assert!(Connector::MBus.mates_with(Connector::MBus));
        assert!(Connector::HatPi.mates_with(Connector::HatPi));
        assert!(Connector::Bare.mates_with(Connector::Bare));
    }

    #[test]
    fn qwiic_and_stemma_qt_are_equivalent() {
        // The whole point: SparkFun Qwiic ≡ Adafruit STEMMA QT (same I2C connector).
        assert!(Connector::Qwiic.mates_with(Connector::StemmaQt));
        assert!(Connector::StemmaQt.mates_with(Connector::Qwiic));
        assert!(Connector::Qwiic.is_i2c());
        assert!(Connector::StemmaQt.is_i2c());
    }

    #[test]
    fn incompatible_connectors_do_not_mate() {
        assert!(!Connector::Grove.mates_with(Connector::Qwiic));
        assert!(!Connector::Grove.mates_with(Connector::MBus));
        assert!(!Connector::HatPi.mates_with(Connector::FeatherWing));
        assert!(!Connector::Qwiic.mates_with(Connector::Grove));
    }

    #[test]
    fn every_board_has_vendor_and_connectors() {
        for b in known_boards() {
            assert!(!b.vendor.is_empty(), "board {} missing vendor", b.name);
            assert!(!b.ecosystem.is_empty(), "board {} missing ecosystem", b.name);
            assert!(
                !b.connectors.is_empty(),
                "board {} must declare at least one connector (use Bare)",
                b.name
            );
        }
    }

    #[test]
    fn boards_by_vendor_finds_espressif() {
        let boards = boards_by_vendor("espressif");
        assert!(!boards.is_empty());
        assert!(boards.iter().any(|b| b.name == "esp32-s3"));
    }

    #[test]
    fn boards_with_hatpi_connector() {
        let boards = boards_with_connector(Connector::HatPi);
        let names: Vec<_> = boards.iter().map(|b| b.name).collect();
        assert!(names.contains(&"raspberry-pi-4"));
        assert!(names.contains(&"raspberry-pi-5"));
    }

    #[test]
    fn bare_accessory_attaches_to_any_board() {
        let board = lookup_board(0x2886, 0x0058).unwrap(); // xiao
        let bme = lookup_accessory("bme280").unwrap();
        assert_eq!(bme.connector, Connector::Bare);
        assert!(board_accepts_accessory(board, bme));
    }

    #[test]
    fn accessories_for_board_includes_bare_modules() {
        let board = lookup_board(0x2109, 0x0820).unwrap(); // rpi-5
        let accs = accessories_for_board(board);
        // All current accessories are Bare, so all should be wireable.
        assert_eq!(accs.len(), known_accessories().len());
    }

    // ── Accelerapp-seeded hardware tests (v2.0) ───────────────────────────────

    #[test]
    fn lookup_flipper_zero() {
        let b = lookup_board(0x0483, 0x5740).unwrap();
        assert_eq!(b.name, "flipper-zero");
        assert_eq!(b.vendor, "Flipper Devices");
        assert!(b.capabilities.contains(&"subghz"));
        assert!(b.capabilities.contains(&"nfc"));
        assert!(b.capabilities.contains(&"rfid"));
        assert!(b.capabilities.contains(&"infrared"));
    }

    #[test]
    fn seeded_boards_present_by_name() {
        let names: Vec<_> = KNOWN_BOARDS.iter().map(|b| b.name).collect();
        for n in [
            "flipper-zero",
            "m5stack-core2",
            "m5stack-atom-s3",
            "esp32-cam",
            "esp32-s3-cam",
            "cyd-esp32-2432s028r",
            "lilygo-t-beam",
            "heltec-wifi-lora-32-v3",
        ] {
            assert!(names.contains(&n), "missing seeded board {n}");
        }
    }

    #[test]
    fn m5stack_core2_connectors_and_caps() {
        let b = KNOWN_BOARDS
            .iter()
            .find(|b| b.name == "m5stack-core2")
            .unwrap();
        assert_eq!(b.vendor, "M5Stack");
        assert!(b.connectors.contains(&Connector::Grove));
        assert!(b.connectors.contains(&Connector::MBus));
        assert!(b.capabilities.contains(&"imu"));
        assert!(b.capabilities.contains(&"display"));
        assert!(b.capabilities.contains(&"touch"));
    }

    #[test]
    fn boards_with_lora_capability() {
        let boards = boards_with_capability("lora");
        let names: Vec<_> = boards.iter().map(|b| b.name).collect();
        assert!(names.contains(&"lilygo-t-beam"));
        assert!(names.contains(&"heltec-wifi-lora-32-v3"));
    }

    #[test]
    fn t_beam_has_gps_and_lora() {
        let b = KNOWN_BOARDS
            .iter()
            .find(|b| b.name == "lilygo-t-beam")
            .unwrap();
        assert_eq!(b.vendor, "LILYGO");
        assert!(b.capabilities.contains(&"gps"));
        assert!(b.capabilities.contains(&"lora"));
    }

    #[test]
    fn cyd_has_display_and_touch() {
        let b = KNOWN_BOARDS
            .iter()
            .find(|b| b.name == "cyd-esp32-2432s028r")
            .unwrap();
        assert!(b.capabilities.contains(&"display"));
        assert!(b.capabilities.contains(&"touch"));
        assert!(b.capabilities.contains(&"microsd"));
    }

    #[test]
    fn seeded_accessories_present() {
        assert!(lookup_accessory("ov2640").is_some());
        let ili = lookup_accessory("ili9341").unwrap();
        assert!(ili.capabilities.contains(&"display"));
        let touch = lookup_accessory("xpt2046").unwrap();
        assert!(touch.capabilities.contains(&"touch"));
    }

    #[test]
    fn camera_boards_include_seeded_cams() {
        let boards = boards_with_capability("camera_capture");
        let names: Vec<_> = boards.iter().map(|b| b.name).collect();
        assert!(names.contains(&"esp32-cam"));
        assert!(names.contains(&"esp32-s3-cam"));
    }

    // ── Registry export (single source of truth) ──────────────────────────────

    #[test]
    fn registry_json_serializes_and_includes_key_entries() {
        let j = registry_json().expect("registry serializes to JSON");
        assert!(j.contains("\"schema_version\""));
        assert!(j.contains("\"boards\""));
        assert!(j.contains("\"accessories\""));
        // a unique board, a seeded board, and a connector token serialize through
        assert!(j.contains("\"flipper-zero\""));
        assert!(j.contains("\"cyd-esp32-2432s028r\""));
        assert!(j.contains("\"grove\"")); // Connector::Grove serializes to its token
        assert!(j.contains("\"bare\""));
    }

    #[test]
    fn registry_snapshot_counts_match_tables() {
        let snap = registry_snapshot();
        assert_eq!(snap.schema_version, REGISTRY_SCHEMA_VERSION);
        assert_eq!(snap.boards.len(), KNOWN_BOARDS.len());
        assert_eq!(snap.accessories.len(), KNOWN_ACCESSORIES.len());
    }
}
