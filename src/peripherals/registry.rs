//! Hardware board registry — maps USB VID/PID to known board names and architectures.
//!
//! This registry is used during USB device discovery to automatically identify
//! connected hardware and suggest appropriate configurations.

/// Information about a known hardware board.
#[derive(Debug, Clone)]
pub struct BoardInfo {
    /// USB Vendor ID.
    pub vid: u16,
    /// USB Product ID.
    pub pid: u16,
    /// Human-readable board name.
    pub name: &'static str,
    /// CPU architecture (e.g., "ARM Cortex-M4", "ESP32-S3", "AArch64").
    pub architecture: Option<&'static str>,
    /// Recommended transport for this board.
    pub transport: &'static str,
    /// Recommended Oh-Ben-Claw capabilities for this board.
    pub capabilities: &'static [&'static str],
}

/// Known USB VID/PID to board mappings.
///
/// VID assignments:
/// - `0x0483` = STMicroelectronics
/// - `0x2341` = Arduino
/// - `0x10c4` = Silicon Labs (CP210x)
/// - `0x1a86` = WCH (CH340/CH343)
/// - `0x303a` = Espressif Systems
/// - `0x2207` = Rockchip
/// - `0x0403` = FTDI
const KNOWN_BOARDS: &[BoardInfo] = &[
    // ── STM32 Nucleo ──────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x0483,
        pid: 0x374b,
        name: "nucleo-f401re",
        architecture: Some("ARM Cortex-M4"),
        transport: "serial",
        capabilities: &["gpio", "adc", "flash", "memory_map"],
    },
    BoardInfo {
        vid: 0x0483,
        pid: 0x3748,
        name: "nucleo-f411re",
        architecture: Some("ARM Cortex-M4"),
        transport: "serial",
        capabilities: &["gpio", "adc", "flash", "memory_map"],
    },
    // ── Arduino ───────────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x2341,
        pid: 0x0043,
        name: "arduino-uno",
        architecture: Some("AVR ATmega328P"),
        transport: "serial",
        capabilities: &["gpio", "analog_read"],
    },
    BoardInfo {
        vid: 0x2341,
        pid: 0x0078,
        name: "arduino-uno-q",
        architecture: Some("Arduino Uno Q / ATmega328P"),
        transport: "bridge",
        capabilities: &["gpio", "analog_read"],
    },
    BoardInfo {
        vid: 0x2341,
        pid: 0x0042,
        name: "arduino-mega",
        architecture: Some("AVR ATmega2560"),
        transport: "serial",
        capabilities: &["gpio", "analog_read"],
    },
    // ── USB-UART Bridges ──────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x10c4,
        pid: 0xea60,
        name: "cp2102",
        architecture: Some("USB-UART bridge"),
        transport: "serial",
        capabilities: &[],
    },
    BoardInfo {
        vid: 0x10c4,
        pid: 0xea70,
        name: "cp2102n",
        architecture: Some("USB-UART bridge"),
        transport: "serial",
        capabilities: &[],
    },
    // ── ESP32 ─────────────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x1a86,
        pid: 0x7523,
        name: "esp32",
        architecture: Some("ESP32 (CH340)"),
        transport: "serial",
        capabilities: &["gpio"],
    },
    BoardInfo {
        vid: 0x1a86,
        pid: 0x55d4,
        name: "esp32",
        architecture: Some("ESP32 (CH340)"),
        transport: "serial",
        capabilities: &["gpio"],
    },
    // ── ESP32-S3 ──────────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "esp32-s3",
        architecture: Some("ESP32-S3 (native USB)"),
        transport: "serial",
        capabilities: &["gpio", "camera_capture", "audio_sample", "sensor_read"],
    },
    BoardInfo {
        vid: 0x10c4,
        pid: 0xea60,
        name: "esp32-s3",
        architecture: Some("ESP32-S3 (CP2102)"),
        transport: "serial",
        capabilities: &["gpio", "camera_capture", "audio_sample", "sensor_read"],
    },
    BoardInfo {
        vid: 0x1a86,
        pid: 0x55d3,
        name: "esp32-s3",
        architecture: Some("ESP32-S3 (CH343)"),
        transport: "serial",
        capabilities: &["gpio", "camera_capture", "audio_sample", "sensor_read"],
    },
    // ── NanoPi Neo3 ───────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x2207,
        pid: 0x330c,
        name: "nanopi-neo3",
        architecture: Some("Rockchip RK3328 (AArch64)"),
        transport: "native",
        capabilities: &["gpio", "i2c", "spi"],
    },
    // ── Raspberry Pi ──────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x2e8a,
        pid: 0x0003,
        name: "raspberry-pi-pico",
        architecture: Some("RP2040 (ARM Cortex-M0+)"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi"],
    },
    // ── FTDI ──────────────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x0403,
        pid: 0x6001,
        name: "ftdi-ft232",
        architecture: Some("USB-UART bridge (FTDI)"),
        transport: "serial",
        capabilities: &[],
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_nucleo_f401re() {
        let b = lookup_board(0x0483, 0x374b).unwrap();
        assert_eq!(b.name, "nucleo-f401re");
        assert_eq!(b.architecture, Some("ARM Cortex-M4"));
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
    }
}
