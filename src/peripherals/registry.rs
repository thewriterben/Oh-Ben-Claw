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
/// - `0x1366` = SEGGER (J-Link / nRF DK)
/// - `0x0955` = NVIDIA
/// - `0x1d6b` = Linux Foundation (gadget devices)
/// - `0x16c0` = PJRC (Teensy)
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
    // ── ESP32-C3 ──────────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x303a,
        pid: 0x1001,
        name: "esp32-c3",
        architecture: Some("ESP32-C3 RISC-V single-core @ 160 MHz (native USB)"),
        transport: "serial",
        capabilities: &["gpio", "i2c", "spi", "wifi", "ble"],
    },
    // ── nRF52840 DK ───────────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x1366,
        pid: 0x1015,
        name: "nrf52840-dk",
        architecture: Some("Nordic nRF52840 ARM Cortex-M4F @ 64 MHz (BLE 5.0)"),
        transport: "serial",
        capabilities: &["gpio", "i2c", "spi", "ble", "pwm"],
    },
    // ── Arduino Nano 33 BLE ───────────────────────────────────────────────────
    BoardInfo {
        vid: 0x2341,
        pid: 0x805a,
        name: "arduino-nano-33-ble",
        architecture: Some("nRF52840 ARM Cortex-M4F @ 64 MHz (BLE, IMU)"),
        transport: "serial",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "ble", "sensor_read"],
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
    },
    // ── BeagleBone Black ──────────────────────────────────────────────────────
    BoardInfo {
        vid: 0x1d6b,
        pid: 0x0104,
        name: "beaglebone-black",
        architecture: Some("TI AM3358 ARM Cortex-A8 @ 1 GHz"),
        transport: "native",
        capabilities: &["gpio", "analog_read", "i2c", "spi", "pwm", "can"],
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
    },
    // ── Seeed XIAO ESP32S3-Sense ──────────────────────────────────────────────
    // Compact ESP32-S3 module with OV2640 camera, PDM microphone, and
    // expandable microSD.  Used as the primary vision node.
    // USB VID=0x2886 (Seeed Studio), PID=0x0058 (XIAO ESP32-S3 Sense).
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

// ── Accessory Registry ────────────────────────────────────────────────────────

/// Describes a known I2C/SPI accessory or add-on module.
///
/// Accessories are peripheral devices that attach to a host board via I2C or SPI.
/// They don't have their own USB VID/PID — they are discovered by scanning the bus.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    },
    AccessoryInfo {
        name: "bmp388",
        description: "Bosch BMP388 — high-accuracy barometric pressure and temperature",
        bus: "i2c",
        default_i2c_addr: Some(0x77),
        capabilities: &["sensor_read"],
        compatible_boards: &[],
    },
    AccessoryInfo {
        name: "sht31",
        description: "Sensirion SHT31 — high-accuracy temperature and humidity",
        bus: "i2c",
        default_i2c_addr: Some(0x44),
        capabilities: &["sensor_read"],
        compatible_boards: &[],
    },
    AccessoryInfo {
        name: "aht20",
        description: "ASAIR AHT20 — temperature and humidity sensor",
        bus: "i2c",
        default_i2c_addr: Some(0x38),
        capabilities: &["sensor_read"],
        compatible_boards: &[],
    },
    // ── Motion / IMU Sensors ──────────────────────────────────────────────────
    AccessoryInfo {
        name: "mpu6050",
        description: "InvenSense MPU-6050 — 6-axis accelerometer + gyroscope",
        bus: "i2c",
        default_i2c_addr: Some(0x68),
        capabilities: &["sensor_read"],
        compatible_boards: &[],
    },
    AccessoryInfo {
        name: "lsm6ds3",
        description: "ST LSM6DS3 — 6-axis IMU (accelerometer + gyroscope)",
        bus: "i2c",
        default_i2c_addr: Some(0x6A),
        capabilities: &["sensor_read"],
        compatible_boards: &[],
    },
    // ── ADC / DAC ─────────────────────────────────────────────────────────────
    AccessoryInfo {
        name: "ads1115",
        description: "TI ADS1115 — 16-bit 4-channel ADC with programmable gain",
        bus: "i2c",
        default_i2c_addr: Some(0x48),
        capabilities: &["analog_read"],
        compatible_boards: &[],
    },
    AccessoryInfo {
        name: "mcp4725",
        description: "Microchip MCP4725 — 12-bit single-channel DAC",
        bus: "i2c",
        default_i2c_addr: Some(0x60),
        capabilities: &["dac"],
        compatible_boards: &[],
    },
    // ── GPIO Expanders ────────────────────────────────────────────────────────
    AccessoryInfo {
        name: "pcf8574",
        description: "NXP PCF8574 — 8-bit I2C GPIO expander",
        bus: "i2c",
        default_i2c_addr: Some(0x20),
        capabilities: &["gpio"],
        compatible_boards: &[],
    },
    AccessoryInfo {
        name: "mcp23017",
        description: "Microchip MCP23017 — 16-bit I2C GPIO expander",
        bus: "i2c",
        default_i2c_addr: Some(0x21),
        capabilities: &["gpio"],
        compatible_boards: &[],
    },
    // ── Thermocouple / Temperature ────────────────────────────────────────────
    AccessoryInfo {
        name: "max31855",
        description: "Maxim MAX31855 — thermocouple-to-digital converter (K-type)",
        bus: "spi",
        default_i2c_addr: None,
        capabilities: &["sensor_read"],
        compatible_boards: &[],
    },
    AccessoryInfo {
        name: "ds18b20",
        description: "Maxim DS18B20 — 1-Wire digital temperature sensor",
        bus: "onewire",
        default_i2c_addr: None,
        capabilities: &["sensor_read"],
        compatible_boards: &[],
    },
    // ── Power Monitoring ──────────────────────────────────────────────────────
    AccessoryInfo {
        name: "ina260",
        description: "TI INA260 — high/low-side current, voltage, and power monitor",
        bus: "i2c",
        default_i2c_addr: Some(0x40),
        capabilities: &["sensor_read"],
        compatible_boards: &[],
    },
    // ── Display ───────────────────────────────────────────────────────────────
    AccessoryInfo {
        name: "ssd1306",
        description: "Solomon SSD1306 — 128x64 OLED display",
        bus: "i2c",
        default_i2c_addr: Some(0x3C),
        capabilities: &[],
        compatible_boards: &[],
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
    },
    AccessoryInfo {
        name: "dht11",
        description: "AOSONG DHT11 — basic humidity and temperature sensor, single-wire protocol",
        bus: "gpio",
        default_i2c_addr: None,
        capabilities: &["sensor_read"],
        compatible_boards: &[],
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
}
