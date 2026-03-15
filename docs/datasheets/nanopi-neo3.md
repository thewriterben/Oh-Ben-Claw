# NanoPi Neo3 — Oh-Ben-Claw Reference

## Board Overview

| Property | Value |
|---|---|
| SoC | Rockchip RK3328 (quad-core Cortex-A53, up to 1.5 GHz) |
| RAM | 1 GB or 3 GB LPDDR4 |
| OS | FriendlyCore (Ubuntu), Armbian, or OpenWrt (AArch64 Linux) |
| GPIO | 40-pin header (3.3 V logic, not 5 V tolerant) |
| Interfaces | I2C, SPI, UART, PWM, USB 3.0 Host, Gigabit Ethernet |
| Form factor | 40 × 40 mm (smaller than Raspberry Pi) |

## Oh-Ben-Claw Configuration

Add to `~/.oh-ben-claw/config.toml`:

```toml
[peripherals]
enabled = true

[[peripherals.boards]]
board = "nanopi-neo3"
transport = "native"
```

Build with: `cargo build --features hardware,peripheral-nanopi`

## Oh-Ben-Claw Tools

| Tool | Description |
|---|---|
| `gpio_read` | Read sysfs GPIO pin (0 or 1) |
| `gpio_write` | Set sysfs GPIO pin high or low |

## GPIO Numbering

NanoPi Neo3 uses sysfs GPIO numbering:

```
gpio_number = 32 * bank + group * 8 + bit
```

| Bank | Group | GPIO Name | Formula |
|---|---|---|---|
| 0 | A (0) | GPIO0_A0 | 0 |
| 0 | A (0) | GPIO0_A7 | 7 |
| 0 | B (1) | GPIO0_B0 | 8 |
| 1 | A (0) | GPIO1_A0 | 32 |
| 2 | A (0) | GPIO2_A0 | 64 |
| 3 | A (0) | GPIO3_A0 | 96 |

## Pin Aliases (40-pin header)

| Alias | sysfs GPIO | Physical Pin | Direction |
|---|---|---|---|
| status_led | 0 | — | output |
| i2c1_sda | 64 | 3 | bidirec |
| i2c1_scl | 65 | 5 | bidirec |
| spi1_mosi | 68 | 19 | output |
| spi1_miso | 69 | 21 | input |
| spi1_sck | 70 | 23 | output |
| uart1_tx | 73 | 8 | output |
| uart1_rx | 74 | 10 | input |

## GPIO Access

```bash
# Add user to gpio group
sudo usermod -aG gpio $USER

# Or for immediate access (less secure)
sudo chmod a+rw /sys/class/gpio/export /sys/class/gpio/unexport
```

## Building on NanoPi Neo3

Cross-compile from host:

```bash
cargo build --target aarch64-unknown-linux-gnu --features hardware,peripheral-nanopi --release
```

Or build natively on the board:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
cargo build --features hardware,peripheral-nanopi --release
```
