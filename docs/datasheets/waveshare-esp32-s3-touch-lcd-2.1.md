# Waveshare ESP32-S3 Touch LCD 2.1 — Oh-Ben-Claw Reference

> Corrected 2026-07-16 against the Waveshare wiki + schematic. The previous
> revision of this file described a 40-pin header, LED on GPIO46, and an I2C bus
> on GPIO4/5 — **none of which exist on this board**. Pin truth below.

## Board Overview

| Property | Value |
|---|---|
| SoC | ESP32-S3R8 (dual-core Xtensa LX7, 240 MHz, octal PSRAM) |
| Flash / PSRAM | 16 MB QSPI flash · 8 MB PSRAM |
| Display | 2.1" round IPS 480×480, ST7701 (3-wire SPI init + RGB565 parallel data) |
| Touch | CST820 capacitive (I2C, interrupt) |
| IMU | QMI8658 6-axis (onboard, I2C) |
| RTC | PCF85063 (onboard, I2C, battery header) |
| USB | **Two Type-C ports**: native USB (USB-Serial-JTAG, GPIO19/20) + UART Type-C (CH343 → UART0 43/44, auto-download) |
| Expander | TCA9554 (EXIO0–7: LCD reset/CS, SD CS, buzzer, IMU/RTC INT — internal only, **not broken out**) |
| Storage | TF card slot (shares GPIO1/2 with LCD init SPI; CS via expander) |
| Battery | MX1.25 2-pin LiPo header + charge manager; BAT voltage sense on GPIO4 |

## What is actually exposed (all of it)

**12-pin header** — the only general-purpose I/O on the board. *(Mapping
bench-verified 2026-07-16 via the meter fingerprint below — GND pair, BOOT→IO0
continuity, and rail voltages all matched on real hardware; header has no
silkscreen.)*

| # | Pin | GPIO | Notes |
|---|---|---|---|
| 1, 5 | GND | — | |
| 2 | VBus | — | 5 V from USB |
| 3 | D− | 19 | native-USB pair; GPIO only if you give up native USB |
| 4 | D+ | 20 | native-USB pair; GPIO only if you give up native USB |
| 6 | 3V3 | — | 800 mA LDO |
| 7 | SCL | 7 | **I2C only** — cannot be remapped as GPIO |
| 8 | SDA | 15 | **I2C only** — cannot be remapped as GPIO |
| 9 | TXD | 43 | UART0 TX, or plain GPIO |
| 10 | RXD | 44 | UART0 RX, or plain GPIO |
| 11 | NC | — | |
| 12 | IO0 | 0 | The one true spare. BOOT strapping pin — keep high at reset |

### No silkscreen on the 12-pin header? Fingerprint it with a meter

1. **Power off, continuity vs the USB-C shell (GND):** exactly two pins beep —
   GNDs, positions **1 and 5**. They're asymmetric, so this fixes orientation:
   the end where a GND is the *outermost* pin is the pin-1 end.
2. **Hold the BOOT button** and re-probe: one new pin gains continuity to GND
   while held — that's **IO0 (GPIO0), pin 12**, at the far end. Definitive, and
   it's the DHT22 pin.
3. **Power on, volts:** pin 2 ≈ 5 V (VBus, beside the end GND) · pin 6 = 3.3 V
   (beside the middle GND) · pins 7/8 idle ≈ 3.3 V (I2C pull-ups) · pin 11
   floats · pins 9/10 (TXD/RXD → safe outputs 43/44) idle high under firmware.

Any disagreement → stop and check the schematic PDF (wiki §Resources) before wiring.

**I2C connector** (4-pin): GND / 3V3 / SCL=GPIO7 / SDA=GPIO15 — same bus as the
header pins 7–8, shared with the onboard CST820 + QMI8658 + PCF85063 + expander.
**Addresses in use: 0x15, 0x20, 0x51, 0x6B, 0x7E** — BME280 (0x76) and MPU-6050
(0x68) coexist fine.

**UART connector** (4-pin): GND / 3V3 / TXD=43 / RXD=44 — disabled while the
UART Type-C port is plugged in (FSUSB42 mux).

**Everything else is consumed by the LCD.** RGB data + control use GPIO
1, 2, 3, 5, 6, 8, 9, 10, 11, 12, 13, 14, 16, 17, 18, 21, 38, 39, 40, 41, 45,
46, 47, 48. Driving any of these as GPIO corrupts the display. There is **no
camera connector** and no wirable I2S mic (GPIO0/1/2 are spoken for).

## Oh-Ben-Claw firmware — build with the board feature

```bash
cd firmware/obc-esp32-s3
cargo run --release --features board-waveshare-21   # espflash runner
```

Pin map under `board-waveshare-21`:

| Function | Pins | Notes |
|---|---|---|
| Track 0 safe outputs | **43, 44** | UART1 spine uplink is disabled on this build |
| DHT22 data | **0** | 10 kΩ pull-up to 3V3 (doubles as BOOT strap hold-high) |
| I2C sensor bus | SDA **15** · SCL **7** | the hardwired connector |
| Command I/O | native USB (19/20) | USB-Serial-JTAG, newline-delimited JSON |
| `camera_capture` / `audio_sample` | stubs | no camera connector / no mic pins on this board |

The default (no-feature) build is the **XIAO ESP32-S3** pin map — outputs
21/3/6/7/8, DHT22 on 9, I2C on 4/5. Flashing a default build onto the Waveshare
drives LCD lines as GPIO outputs; don't.

## Bench wiring quick-reference (Station A)

```
DHT22:  + → 3V3 (hdr 6) · out → IO0 (hdr 12) + 10 kΩ→3V3 · − → GND (hdr 1)
LED:    GPIO43 (hdr 9) → 330 Ω → LED → GND        (gpio_write pin 43)
BME280 / MPU-6050: I2C connector (SDA15/SCL7, 3V3, GND) — Qwiic chain OK
Console/flash: either Type-C port (native USB or CH343 UART, auto-download)
```

## Oh-Ben-Claw Configuration

```toml
[peripherals]
enabled = true

[[peripherals.boards]]
board = "waveshare-esp32-s3-touch-lcd-2.1"
transport = "serial"
path = "COM7"           # /dev/ttyACM* (native USB) or /dev/ttyUSB* (CH343)
baud = 115200
```

## Oh-Ben-Claw Tools

| Tool | On this board |
|---|---|
| `gpio_read` / `gpio_write` | GPIO 43/44 (Track 0 allow-list) |
| `sensor_read` | DHT22 (GPIO0), BME280/MPU-6050 + onboard QMI8658 on I2C 15/7 |
| `capabilities` | reports commands + the active GPIO map |
| `camera_capture` / `audio_sample` | stub responses (hardware absent) |

*Sources: Waveshare wiki `ESP32-S3-Touch-LCD-2.1` (interfaces, internal
connection tables, I2C address FAQ) + board schematic PDF;
`firmware/obc-esp32-s3` (`board-waveshare-21` feature).*
