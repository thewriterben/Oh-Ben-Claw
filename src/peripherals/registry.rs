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

/// Describes a known hardware board.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    },
    // ── Arduino ───────────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x2341,
        pid: 0x0043,
        name: "arduino-uno",
        architecture: Some("AVR ATmega328P @ 16 MHz"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "analog_write"],
    },
    BoardInfo {
        vid: 0x2341,
        pid: 0x0001,
        name: "arduino-uno",
        architecture: Some("AVR ATmega328P @ 16 MHz (legacy)"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "analog_write"],
    },
    BoardInfo {
        vid: 0x2341,
        pid: 0x0078,
        name: "arduino-uno-q",
        architecture: Some("Arduino Uno Q / ATmega328P"),
        transport: "bridge",
        capabilities: &["gpio", "analog_read", "analog_write"],
    },
    BoardInfo {
        vid: 0x2341,
        pid: 0x0042,
        name: "arduino-mega",
        architecture: Some("AVR ATmega2560 @ 16 MHz"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "analog_write"],
    },
    BoardInfo {
        vid: 0x2341,
        pid: 0x0036,
        name: "arduino-leonardo",
        architecture: Some("AVR ATmega32U4 @ 16 MHz (native USB)"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "analog_write"],
    },
    BoardInfo {
        vid: 0x2341,
        pid: 0x0058,
        name: "arduino-nano-every",
        architecture: Some("AVR ATmega4809 @ 20 MHz"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "analog_write"],
    },
    // Arduino Nano clone with CH340 USB-UART (extremely common)
    BoardInfo {
        vid: 0x1a86,
        pid: 0x7523,
        name: "arduino-nano",
        architecture: Some("AVR ATmega328P @ 16 MHz (CH340 USB-UART)"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "analog_write"],
    },
    // ── USB-UART Bridges ──────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x10c4,
        pid: 0xea60,
        name: "cp2102",
        architecture: Some("Silicon Labs CP2102 USB-UART bridge"),
        transport: "serial",
        capabilities: &[],
    },
    BoardInfo {
        vid: 0x10c4,
        pid: 0xea70,
        name: "cp2102n",
        architecture: Some("Silicon Labs CP2102N USB-UART bridge"),
        transport: "serial",
        capabilities: &[],
    },
    BoardInfo {
        vid: 0x0403,
        pid: 0x6001,
        name: "ftdi-ft232",
        architecture: Some("FTDI FT232 USB-UART bridge"),
        transport: "serial",
        capabilities: &[],
    },
    BoardInfo {
        vid: 0x0403,
        pid: 0x6015,
        name: "ftdi-ft231x",
        architecture: Some("FTDI FT231X USB-UART bridge"),
        transport: "serial",
        capabilities: &[],
    },
    // ── ESP32 ─────────────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x1a86,
        pid: 0x55d4,
        name: "esp32",
        architecture: Some("ESP32 Xtensa LX6 @ 240 MHz (CH340)"),
        transport: "serial",
        capabilities: &["gpio"],
    },
    // ── ESP32-S3 ──────────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "esp32-s3",
        architecture: Some("ESP32-S3 Xtensa LX7 @ 240 MHz (native USB)"),
        transport: "serial",
        capabilities: &["gpio", "camera_capture", "audio_sample", "sensor_read"],
    },
    BoardInfo {
        vid: 0x10c4,
        pid: 0xea60,
        name: "esp32-s3",
        architecture: Some("ESP32-S3 (CP2102 USB-UART)"),
        transport: "serial",
        capabilities: &["gpio", "camera_capture", "audio_sample", "sensor_read"],
    },
    BoardInfo {
        vid: 0x1a86,
        pid: 0x55d3,
        name: "esp32-s3",
        architecture: Some("ESP32-S3 (CH343 USB-UART)"),
        transport: "serial",
        capabilities: &["gpio", "camera_capture", "audio_sample", "sensor_read"],
    },
    // ── NanoPi Neo3 ───────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x2207,
        pid: 0x330c,
        name: "nanopi-neo3",
        architecture: Some("Rockchip RK3328 quad-core ARM Cortex-A53 @ 1.5 GHz (AArch64)"),
        transport: "native",
        capabilities: &["gpio", "i2c", "spi", "pwm"],
    },
    // ── Raspberry Pi ──────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x2e8a,
        pid: 0x0003,
        name: "raspberry-pi-pico",
        architecture: Some("RP2040 dual-core ARM Cortex-M0+ @ 133 MHz"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "pwm"],
    },
    BoardInfo {
        vid: 0x2e8a,
        pid: 0x000a,
        name: "raspberry-pi-pico-w",
        architecture: Some("RP2040 dual-core ARM Cortex-M0+ @ 133 MHz (Wi-Fi)"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "pwm"],
    },
    BoardInfo {
        vid: 0x2e8a,
        pid: 0x0004,
        name: "raspberry-pi-pico2",
        architecture: Some("RP2350 dual-core ARM Cortex-M33 @ 150 MHz"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "pwm"],
    },
    // Raspberry Pi 4 / 5 (USB hub VID when used as USB device / OTG)
    BoardInfo {
        vid: 0x2109,
        pid: 0x0817,
        name: "raspberry-pi-4",
        architecture: Some("BCM2711 quad-core ARM Cortex-A72 @ 1.8 GHz (AArch64)"),
        transport: "native",
        capabilities: &["gpio", "i2c", "spi", "pwm", "camera_capture"],
    },
    BoardInfo {
        vid: 0x2109,
        pid: 0x0820,
        name: "raspberry-pi-5",
        architecture: Some("BCM2712 quad-core ARM Cortex-A76 @ 2.4 GHz (AArch64)"),
        transport: "native",
        capabilities: &["gpio", "i2c", "spi", "pwm", "camera_capture"],
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
        assert!(boards.iter().all(|b| b.name.starts_with("nucleo")));
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
}
