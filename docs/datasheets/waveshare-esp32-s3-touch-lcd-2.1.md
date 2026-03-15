# Waveshare ESP32-S3 Touch LCD 2.1 — Oh-Ben-Claw Reference

## Board Overview

| Property | Value |
|---|---|
| SoC | ESP32-S3 (dual-core Xtensa LX7, 240 MHz) |
| Flash | 16 MB (QSPI) |
| PSRAM | 8 MB (PSRAM8) |
| Display | 2.1" LCD 480×480 IPS, ST7701 driver (3-wire SPI) |
| Touch | GT911 capacitive multi-touch controller (I2C) |
| USB | USB Type-C (ESP32-S3 native USB + CH343 USB-UART bridge) |
| Battery | IP5306 power management (I2C, optional LiPo 3.7 V) |
| Expansion | 40-pin header (compatible with many RPi HATs for non-SPI peripherals) |

## Oh-Ben-Claw Configuration

Add to `~/.oh-ben-claw/config.toml`:

```toml
[peripherals]
enabled = true

[[peripherals.boards]]
board = "waveshare-esp32-s3-touch-lcd-2.1"
transport = "serial"
path = "/dev/ttyUSB0"   # or /dev/ttyACM0 on Linux; /dev/cu.usbmodem* on macOS
baud = 115200
```

For MQTT-based (wireless) connection:

```toml
[[peripherals.boards]]
board = "waveshare-esp32-s3-touch-lcd-2.1"
transport = "mqtt"
node_id = "esp32-s3-kitchen"
```

## Oh-Ben-Claw Tools

| Tool | Description |
|---|---|
| `gpio_read` | Read a GPIO pin value (0 or 1) |
| `gpio_write` | Set a GPIO pin high or low |
| `camera_capture` | Capture JPEG from OV2640 (returns base64 JPEG) |
| `audio_sample` | Sample I2S microphone (returns RMS level or PCM samples) |
| `sensor_read` | Read I2C/SPI sensor field (bme280, mpu6050, sht31, etc.) |
| `capabilities` | Report supported commands and GPIO map |

## Pin Aliases

| Alias | GPIO |
|---|---|
| builtin_led | 46 |
| lcd_backlight | 46 |
| touch_sda | 4 |
| touch_scl | 5 |
| touch_int | 7 |
| touch_rst | 6 |
| lcd_mosi | 11 |
| lcd_sck | 12 |
| lcd_cs | 10 |
| lcd_dc | 8 |
| lcd_rst | 9 |
| uart_tx | 43 |
| uart_rx | 44 |

## Camera Wiring (OV2640 via FPC connector)

| Signal | GPIO |
|---|---|
| XCLK | 15 |
| SIOD | 4 |
| SIOC | 5 |
| D0–D7 | 39–42, 16–19 |
| VSYNC | 21 |
| HREF | 38 |
| PCLK | 13 |

## I2S Microphone Wiring (INMP441 / SPH0645)

| Signal | GPIO |
|---|---|
| SCK | 0 |
| WS | 1 |
| SD | 2 |

## I2C Sensor Bus (BME280, MPU6050, SHT31, etc.)

| Signal | GPIO |
|---|---|
| SDA | 4 |
| SCL | 5 |

The I2C bus is shared with the GT911 touch controller. Sensors with unique I2C addresses coexist on this bus without conflict.

## Firmware Build & Flash

```bash
# Install ESP toolchain
cargo install espup && espup install && source ~/export-esp.sh

# Build and flash
cd firmware/obc-esp32-s3
cargo build --release
cargo espflash flash --monitor
```

## sdkconfig.defaults (required for camera and I2S)

```ini
CONFIG_ESP32S3_DEFAULT_CPU_FREQ_240=y
CONFIG_SPIRAM=y
CONFIG_SPIRAM_SPEED_80M=y
CONFIG_ESP32_CAMERA=y
CONFIG_I2S_ENABLE=y
```
