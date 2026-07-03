//! Minimal SX1262 LoRa driver for the Heltec WiFi LoRa 32 V3.
//!
//! Just enough of the Semtech SX126x command set to bring the radio up, transmit
//! a buffer, and receive one with RSSI/SNR — the Phase B point-to-point link test.
//!
//! Transport: ESP-IDF SPI (mode 0, 8 MHz) with hardware-managed NSS on GPIO8.
//! Control lines RST/BUSY/DIO1 are driven via the raw sys GPIO API. Every command
//! waits for BUSY low first (the SX126x handshake), and every wait is bounded so a
//! miswired/silent radio errors instead of hanging (same principle as the node's
//! I2C timeout fix).
//!
//! **Untested on metal by its author** — opcodes/params are from the SX1262
//! datasheet; the TCXO voltage, PA config, and bit-timing may need a bench tweak.
//!
//! Heltec V3 pins: NSS=8 SCK=9 MOSI=10 MISO=11 · RST=12 BUSY=13 DIO1=14 ·
//! TCXO on the chip's DIO3 (1.8 V) · RF switch on DIO2 (chip-controlled).

use anyhow::{bail, Result};
use esp_idf_svc::hal::delay::Ets;
use esp_idf_svc::hal::spi::{SpiDeviceDriver, SpiDriver};
use esp_idf_svc::sys::*;
use log::info;

// ── SX126x opcodes ────────────────────────────────────────────────────────────
const OP_SET_STANDBY: u8 = 0x80;
const OP_SET_REGULATOR_MODE: u8 = 0x96;
const OP_SET_PACKET_TYPE: u8 = 0x8A;
const OP_SET_RF_FREQUENCY: u8 = 0x86;
const OP_SET_DIO3_AS_TCXO: u8 = 0x97;
const OP_CALIBRATE: u8 = 0x89;
const OP_SET_DIO2_AS_RFSWITCH: u8 = 0x9D;
const OP_SET_PA_CONFIG: u8 = 0x95;
const OP_SET_TX_PARAMS: u8 = 0x8E;
const OP_SET_BUFFER_BASE: u8 = 0x8F;
const OP_SET_MODULATION_PARAMS: u8 = 0x8B;
const OP_SET_PACKET_PARAMS: u8 = 0x8C;
const OP_SET_DIO_IRQ_PARAMS: u8 = 0x08;
const OP_WRITE_REGISTER: u8 = 0x0D;
const OP_READ_REGISTER: u8 = 0x1D;
const OP_WRITE_BUFFER: u8 = 0x0E;
const OP_READ_BUFFER: u8 = 0x1E;
const OP_SET_TX: u8 = 0x83;
const OP_SET_RX: u8 = 0x82;
const OP_GET_STATUS: u8 = 0xC0;
const OP_GET_IRQ_STATUS: u8 = 0x12;
const OP_CLEAR_IRQ_STATUS: u8 = 0x02;
const OP_GET_RX_BUFFER_STATUS: u8 = 0x13;
const OP_GET_PACKET_STATUS: u8 = 0x14;

// LoRa sync-word register (private network = 0x1424).
const REG_LORA_SYNCWORD_MSB: u16 = 0x0740;

// IRQ bits.
const IRQ_TX_DONE: u16 = 0x0001;
const IRQ_RX_DONE: u16 = 0x0002;
const IRQ_CRC_ERR: u16 = 0x0040;
const IRQ_TIMEOUT: u16 = 0x0200;

#[inline]
fn now_us() -> i64 {
    unsafe { esp_timer_get_time() }
}

/// A received LoRa frame.
pub struct RxFrame {
    pub data: Vec<u8>,
    pub rssi_dbm: i16,
    pub snr_db: i8,
}

pub struct Sx1262 {
    spi: SpiDeviceDriver<'static, SpiDriver<'static>>,
    rst: i32,
    busy: i32,
    #[allow(dead_code)]
    dio1: i32,
}

impl Sx1262 {
    /// Wrap an SPI device + the RST/BUSY/DIO1 GPIOs. Configures pin directions.
    pub fn new(
        spi: SpiDeviceDriver<'static, SpiDriver<'static>>,
        rst: i32,
        busy: i32,
        dio1: i32,
    ) -> Self {
        // SAFETY: raw GPIO config on the radio control lines.
        unsafe {
            gpio_set_direction(rst, gpio_mode_t_GPIO_MODE_OUTPUT);
            gpio_set_direction(busy, gpio_mode_t_GPIO_MODE_INPUT);
            gpio_set_direction(dio1, gpio_mode_t_GPIO_MODE_INPUT);
        }
        Self { spi, rst, busy, dio1 }
    }

    /// Wait for BUSY low (chip ready), bounded so a dead radio can't hang us.
    fn wait_busy(&self) -> Result<()> {
        let start = now_us();
        loop {
            if unsafe { gpio_get_level(self.busy) } == 0 {
                return Ok(());
            }
            if now_us() - start > 1_000_000 {
                bail!("SX1262 BUSY stuck high (>1 s) — radio not responding");
            }
        }
    }

    /// Issue a command (opcode + params), no response read.
    fn cmd(&mut self, opcode: u8, params: &[u8]) -> Result<()> {
        self.wait_busy()?;
        let mut buf = Vec::with_capacity(1 + params.len());
        buf.push(opcode);
        buf.extend_from_slice(params);
        self.spi.write(&buf)?;
        Ok(())
    }

    /// Issue a command that returns data: sends `opcode ++ params ++ n·NOP` and
    /// returns the last `n` received bytes (the response is byte-aligned after the
    /// opcode/params on the SX126x SPI protocol).
    fn read_cmd(&mut self, opcode: u8, params: &[u8], n: usize) -> Result<Vec<u8>> {
        self.wait_busy()?;
        let total = 1 + params.len() + n;
        let mut tx = Vec::with_capacity(total);
        tx.push(opcode);
        tx.extend_from_slice(params);
        tx.resize(total, 0x00);
        let mut rx = vec![0u8; total];
        self.spi.transfer(&mut rx, &tx)?;
        Ok(rx[total - n..].to_vec())
    }

    fn write_register(&mut self, addr: u16, data: &[u8]) -> Result<()> {
        let mut params = Vec::with_capacity(2 + data.len());
        params.push((addr >> 8) as u8);
        params.push(addr as u8);
        params.extend_from_slice(data);
        self.cmd(OP_WRITE_REGISTER, &params)
    }

    /// Read `n` register bytes at `addr`. Layout: opcode, addr_hi, addr_lo, then a
    /// status byte, then the data — so we skip one leading byte.
    fn read_register(&mut self, addr: u16, n: usize) -> Result<Vec<u8>> {
        let params = [(addr >> 8) as u8, addr as u8, 0x00];
        self.read_cmd(OP_READ_REGISTER, &params, n)
    }

    /// GetStatus opcode — returns the raw status byte (chip mode / command status).
    pub fn status(&mut self) -> Result<u8> {
        Ok(self.read_cmd(OP_GET_STATUS, &[], 1)?[0])
    }

    fn irq_status(&mut self) -> Result<u16> {
        // [status, irq_hi, irq_lo]
        let r = self.read_cmd(OP_GET_IRQ_STATUS, &[], 3)?;
        Ok(((r[1] as u16) << 8) | r[2] as u16)
    }

    fn clear_irq(&mut self) -> Result<()> {
        self.cmd(OP_CLEAR_IRQ_STATUS, &[0xFF, 0xFF])
    }

    /// Hardware reset then bring the radio up for 915 MHz LoRa (SF7/BW125/CR4-5).
    /// Returns the sync-word register readback so the caller can self-test SPI.
    pub fn init(&mut self, freq_hz: u64) -> Result<[u8; 2]> {
        // ── Hardware reset ──
        // SAFETY: raw GPIO on RST.
        unsafe {
            gpio_set_level(self.rst, 0);
            Ets::delay_us(2_000);
            gpio_set_level(self.rst, 1);
        }
        Ets::delay_us(5_000);
        self.wait_busy()?;

        self.cmd(OP_SET_STANDBY, &[0x00])?; // STDBY_RC
        self.cmd(OP_SET_REGULATOR_MODE, &[0x01])?; // DC-DC + LDO (SX1262 has DC-DC)
        self.cmd(OP_SET_PACKET_TYPE, &[0x01])?; // LoRa

        // TCXO on DIO3 at 1.8 V, ~5 ms startup (320 × 15.625 µs = 0x000140).
        self.cmd(OP_SET_DIO3_AS_TCXO, &[0x02, 0x00, 0x01, 0x40])?;
        // Calibrate all blocks now that the reference clock is up.
        self.cmd(OP_CALIBRATE, &[0x7F])?;
        Ets::delay_us(5_000);
        self.wait_busy()?;

        // The SX1262 drives the antenna RF switch via DIO2.
        self.cmd(OP_SET_DIO2_AS_RFSWITCH, &[0x01])?;

        // RF frequency: frf = freq · 2^25 / 32 MHz.
        let frf = ((freq_hz << 25) / 32_000_000) as u32;
        self.cmd(
            OP_SET_RF_FREQUENCY,
            &[(frf >> 24) as u8, (frf >> 16) as u8, (frf >> 8) as u8, frf as u8],
        )?;

        // PA config for the SX1262 (+22 dBm capable) and TX power.
        self.cmd(OP_SET_PA_CONFIG, &[0x04, 0x07, 0x00, 0x01])?;
        self.cmd(OP_SET_TX_PARAMS, &[0x16, 0x04])?; // +22 dBm, 200 µs ramp

        self.cmd(OP_SET_BUFFER_BASE, &[0x00, 0x00])?; // TX/RX base = 0

        // Modulation: SF7, BW125 kHz, CR 4/5, low-data-rate-optimize off.
        self.cmd(OP_SET_MODULATION_PARAMS, &[0x07, 0x04, 0x01, 0x00])?;
        // Packet: 8-symbol preamble, explicit header, len 0xFF (per-TX), CRC on, std IQ.
        self.cmd(OP_SET_PACKET_PARAMS, &[0x00, 0x08, 0x00, 0xFF, 0x01, 0x00])?;

        // Private-network sync word 0x1424 (both nodes must match).
        self.write_register(REG_LORA_SYNCWORD_MSB, &[0x14, 0x24])?;

        // Route TxDone | RxDone | Timeout to the IRQ register (and DIO1).
        let mask = IRQ_TX_DONE | IRQ_RX_DONE | IRQ_TIMEOUT;
        self.cmd(
            OP_SET_DIO_IRQ_PARAMS,
            &[
                (mask >> 8) as u8, mask as u8, // IRQ mask
                (mask >> 8) as u8, mask as u8, // DIO1 mask
                0x00, 0x00, // DIO2
                0x00, 0x00, // DIO3
            ],
        )?;

        // Self-test: read the sync word back. 0x1424 ⇒ SPI read+write+BUSY all good.
        let sync = self.read_register(REG_LORA_SYNCWORD_MSB, 2)?;
        Ok([sync[0], sync[1]])
    }

    /// Transmit a buffer (blocks until TxDone or ~3 s timeout).
    pub fn transmit(&mut self, data: &[u8]) -> Result<()> {
        self.cmd(OP_SET_STANDBY, &[0x00])?;
        // Set payload length for this frame.
        self.cmd(
            OP_SET_PACKET_PARAMS,
            &[0x00, 0x08, 0x00, data.len() as u8, 0x01, 0x00],
        )?;
        // Write payload at buffer offset 0.
        let mut params = Vec::with_capacity(1 + data.len());
        params.push(0x00);
        params.extend_from_slice(data);
        self.cmd(OP_WRITE_BUFFER, &params)?;
        self.clear_irq()?;
        self.cmd(OP_SET_TX, &[0x00, 0x00, 0x00])?; // no chip timeout

        let start = now_us();
        loop {
            let irq = self.irq_status()?;
            if irq & IRQ_TX_DONE != 0 {
                self.clear_irq()?;
                return Ok(());
            }
            if now_us() - start > 3_000_000 {
                bail!("SX1262 TxDone timeout");
            }
            Ets::delay_us(1_000);
        }
    }

    /// Listen for a frame for up to `timeout_ms`. `Ok(None)` on timeout/CRC error.
    pub fn receive(&mut self, timeout_ms: u32) -> Result<Option<RxFrame>> {
        self.cmd(OP_SET_STANDBY, &[0x00])?;
        self.cmd(OP_SET_PACKET_PARAMS, &[0x00, 0x08, 0x00, 0xFF, 0x01, 0x00])?;
        self.clear_irq()?;
        // SetRx timeout in 15.625 µs steps (≈ ms × 64).
        let t = ((timeout_ms as u64) * 64).min(0x00FF_FFFE);
        self.cmd(OP_SET_RX, &[(t >> 16) as u8, (t >> 8) as u8, t as u8])?;

        let start = now_us();
        let hard_deadline_us = (timeout_ms as i64 + 500) * 1_000;
        loop {
            let irq = self.irq_status()?;
            if irq & IRQ_RX_DONE != 0 {
                self.clear_irq()?;
                if irq & IRQ_CRC_ERR != 0 {
                    return Ok(None); // corrupt frame — drop
                }
                let st = self.read_cmd(OP_GET_RX_BUFFER_STATUS, &[], 3)?; // [status,len,start]
                let len = st[1] as usize;
                let start_ofs = st[2];
                let data = self.read_cmd(OP_READ_BUFFER, &[start_ofs, 0x00], len)?;
                let ps = self.read_cmd(OP_GET_PACKET_STATUS, &[], 4)?; // [status,rssi,snr,sig]
                return Ok(Some(RxFrame {
                    data,
                    rssi_dbm: -(ps[1] as i16) / 2,
                    snr_db: (ps[2] as i8) / 4,
                }));
            }
            if irq & IRQ_TIMEOUT != 0 {
                self.clear_irq()?;
                return Ok(None);
            }
            if now_us() - start > hard_deadline_us {
                return Ok(None);
            }
            Ets::delay_us(2_000);
        }
    }
}

/// Log a one-line interpretation of the init self-test.
pub fn log_selftest(status: u8, sync: [u8; 2]) {
    info!(
        "SX1262 self-test: status=0x{:02X}, syncword readback=0x{:02X}{:02X} (expect 0x1424)",
        status, sync[0], sync[1]
    );
    if sync == [0x14, 0x24] {
        info!("  → SPI + BUSY plumbing OK, radio configured.");
    } else if sync == [0x00, 0x00] || sync == [0xFF, 0xFF] {
        info!("  → BAD readback: SPI/BUSY wiring or timing wrong (check NSS/SCK/MOSI/MISO/BUSY).");
    } else {
        info!("  → Unexpected readback — partial comms; investigate.");
    }
}
