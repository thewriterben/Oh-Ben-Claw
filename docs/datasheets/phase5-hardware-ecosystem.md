# Phase 5 Hardware Ecosystem

This document describes all hardware drivers added in Phase 5 of Oh-Ben-Claw,
covering the Raspberry Pi GPIO/camera driver, Arduino serial driver, STM32 Nucleo
probe-rs driver, and the shared Linux bus tools (I2C, SPI, PWM).

---

## Raspberry Pi GPIO & Camera (`src/peripherals/rpi.rs`)

**Feature flag:** `--features peripheral-rpi`
**Transport:** `native`
**Supported boards:** Raspberry Pi 3 Model B/B+, Raspberry Pi 4 Model B, Raspberry Pi 5, Raspberry Pi Zero 2 W, Raspberry Pi Compute Module 4

### GPIO Interface

Uses the Linux sysfs GPIO interface (`/sys/class/gpio/`) for compatibility with all Pi models and all kernel versions. BCM GPIO numbering is used throughout.

| Tool | Description |
|---|---|
| `rpi_gpio_read` | Read digital value (0/1) of a BCM GPIO pin |
| `rpi_gpio_write` | Set a BCM GPIO pin HIGH or LOW |
| `rpi_pwm_write` | Generate hardware PWM via `/sys/class/pwm/` |
| `rpi_camera_capture` | Capture a still image using `libcamera-still` |
| `rpi_system_info` | Read CPU temperature, throttle status, memory, and model |

### Camera Prerequisites

```bash
# Enable camera interface
sudo raspi-config  # Interface Options → Camera → Enable

# Install libcamera
sudo apt-get install -y libcamera-apps

# Verify
libcamera-still --list-cameras
```

### Hardware PWM Channels

| Channel | GPIO pins |
|---|---|
| PWM0 | GPIO12 (pin 32) or GPIO18 (pin 12) |
| PWM1 | GPIO13 (pin 33) or GPIO19 (pin 35) |

Enable hardware PWM by adding to `/boot/config.txt`:
```
dtoverlay=pwm-2chan
```

---

## Arduino Serial (`src/peripherals/arduino.rs`)

**Feature flag:** `--features hardware` (default)
**Transport:** `serial`
**Supported boards:** Arduino Uno, Mega, Leonardo, Nano, Nano Every

### Protocol

Newline-delimited JSON over USB serial at 115200 baud:

```json
// Host → Arduino
{"cmd": "gpio_read", "pin": 13}
{"cmd": "gpio_write", "pin": 13, "value": 1}
{"cmd": "analog_read", "pin": 0}
{"cmd": "analog_write", "pin": 9, "value": 128}
{"cmd": "ping"}

// Arduino → Host
{"ok": true, "value": 1}
{"ok": false, "error": "pin not available"}
{"ok": true, "version": "1.0.0", "board": "arduino-uno"}
```

| Tool | Description |
|---|---|
| `arduino_gpio_read` | Read digital pin value (HIGH=1, LOW=0) |
| `arduino_gpio_write` | Set digital pin HIGH or LOW |
| `arduino_analog_read` | Read analog pin (0–1023, maps to 0–5V on Uno) |
| `arduino_analog_write` | Write PWM value (0–255) to a PWM-capable pin |
| `arduino_ping` | Verify connection and read firmware version |

### Companion Firmware

Flash the `firmware/obc-arduino/` sketch using the Arduino IDE or `arduino-cli`:

```bash
arduino-cli compile --fqbn arduino:avr:uno firmware/obc-arduino
arduino-cli upload --fqbn arduino:avr:uno --port /dev/ttyUSB0 firmware/obc-arduino
```

### Config Example

```toml
[[peripherals.boards]]
board = "arduino-uno"
transport = "serial"
path = "/dev/ttyUSB0"
baud = 115200
```

---

## STM32 Nucleo via probe-rs (`src/peripherals/stm32.rs`)

**Feature flag:** `--features peripheral-stm32`
**Transport:** `probe`
**Supported boards:** Nucleo-F401RE, F411RE, L476RG, H743ZI, G474RE

### Prerequisites

```bash
# Install probe-rs
cargo install probe-rs-tools --locked

# Install udev rules (Linux)
sudo probe-rs complete install

# Verify probe is detected
probe-rs list
```

### Tools

| Tool | Description |
|---|---|
| `stm32_flash` | Flash a compiled .elf/.bin to the board via ST-Link |
| `stm32_rtt_read` | Read RTT output from the running firmware |
| `stm32_rtt_write` | Send a command to the firmware via RTT down-channel |
| `stm32_reset` | Hard or soft reset the board |
| `stm32_list_probes` | List all connected debug probes |
| `stm32_mem_read` | Read 32-bit words from the STM32 memory map |

### Chip Auto-Detection

The driver infers the `probe-rs` chip name from the board name in config:

| Config `board` | probe-rs chip |
|---|---|
| `nucleo-f401re` | `STM32F401RETx` |
| `nucleo-f411re` | `STM32F411RETx` |
| `nucleo-l476rg` | `STM32L476RGTx` |
| `nucleo-h743zi` | `STM32H743ZITx` |
| `nucleo-g474re` | `STM32G474RETx` |

### Config Example

```toml
[[peripherals.boards]]
board = "nucleo-f401re"
transport = "probe"
```

---

## Shared Linux Bus Tools (`src/peripherals/bus_tools.rs`)

**Feature flag:** none (always available on Linux)
**Transport:** host (no board config needed)

These tools operate directly on the host Linux SBC and are registered automatically
when `target_os = "linux"`. They work on Raspberry Pi, NanoPi Neo3, and any other
Linux SBC with I2C, SPI, and PWM kernel interfaces.

### I2C Tools

| Tool | Description |
|---|---|
| `i2c_scan` | Scan a bus for devices (wraps `i2cdetect -y -r`) |
| `i2c_read` | Read a register byte/word from a device (`i2cget`) |
| `i2c_write` | Write a byte to a device register (`i2cset`) |

**Prerequisites:**
```bash
sudo apt-get install -y i2c-tools
# Enable I2C: sudo raspi-config → Interface Options → I2C → Enable
```

**Common I2C device addresses:**
| Address | Device |
|---|---|
| `0x3C` | SSD1306 OLED display |
| `0x48` | ADS1115 16-bit ADC |
| `0x68` | MPU-6050 IMU (gyro + accel) |
| `0x76` | BME280 temperature/humidity/pressure |
| `0x27` | PCF8574 8-bit I/O expander |
| `0x60` | MCP4725 12-bit DAC |

### SPI Tool

| Tool | Description |
|---|---|
| `spi_transfer` | Full-duplex SPI transfer via `/dev/spidevN.M` |

**Prerequisites:**
```bash
# Enable SPI: sudo raspi-config → Interface Options → SPI → Enable
pip3 install spidev
```

### PWM Tool

| Tool | Description |
|---|---|
| `pwm_control` | Set frequency, duty cycle, and enable/disable via `/sys/class/pwm/` |

**Prerequisites (Raspberry Pi):**
```bash
# Add to /boot/config.txt:
dtoverlay=pwm-2chan
# Then reboot
```

---

## Updated Board Registry

Phase 5 adds 10 new board entries:

| Board | VID:PID | Transport | New Capabilities |
|---|---|---|---|
| Nucleo-F411RE | `0483:374e` | probe | rtt, flash |
| Nucleo-L476RG | `0483:374f` | probe | rtt, flash |
| Nucleo-H743ZI | `0483:374d` | probe | rtt, flash |
| Nucleo-G474RE | `0483:374c` | probe | rtt, flash |
| Arduino Leonardo | `2341:0036` | serial | analog_write |
| Arduino Nano Every | `2341:0058` | serial | analog_write |
| Arduino Nano (CH340) | `1a86:7523` | serial | analog_write |
| FTDI FT231X | `0403:6015` | serial | — |
| RPi Pico W | `2e8a:000a` | serial | pwm |
| RPi Pico 2 | `2e8a:0004` | serial | pwm |
| Raspberry Pi 4 | `2109:0817` | native | camera_capture |
| Raspberry Pi 5 | `2109:0820` | native | camera_capture |
