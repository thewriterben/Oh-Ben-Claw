// ── Hardware Data ─────────────────────────────────────────────────────────────
// Derived from OBC-deployment-generator/lib/obc-data.ts and
// Oh-Ben-Claw/src/peripherals/mod.rs
// This module is the single source of truth for all hardware scenarios.

export type HostOS = 'windows' | 'macos' | 'linux';
export type Architecture = 'x86_64' | 'aarch64' | 'riscv' | 'arm32';
export type Toolchain = 'rust-cargo' | 'arduino-ide' | 'vscode-platformio' | 'esp-idf' | 'probe-rs';
export type BoardCategory = 'host' | 'esp32' | 'rpi' | 'arduino' | 'stm32' | 'other';
export type TransportType = 'native' | 'serial' | 'mqtt' | 'probe';

export interface BoardInfo {
  id: string;
  displayName: string;
  architecture: string;
  archType: Architecture;
  transport: TransportType;
  capabilities: string[];
  category: BoardCategory;
  description: string;
  recommendedToolchains: Toolchain[];
  firmwareSupported: boolean; // Can generate OBC firmware for this board
  otaSupported: boolean;
  otaMethod?: string;
  usbDriverNote?: string;
  purchaseNote?: string;
}

export const BOARDS: BoardInfo[] = [
  // ── Host Boards ────────────────────────────────────────────────────────────
  {
    id: 'pc-linux',
    displayName: 'PC / Laptop (Linux)',
    architecture: 'x86_64 / AMD64',
    archType: 'x86_64',
    transport: 'native',
    capabilities: ['shell', 'file', 'browser', 'http', 'memory'],
    category: 'host',
    description: 'A standard Linux PC or laptop running Ubuntu, Debian, Fedora, or Arch. This is the recommended host for Oh-Ben-Claw. It runs the core brain agent and connects to peripheral nodes over USB or MQTT.',
    recommendedToolchains: ['rust-cargo'],
    firmwareSupported: false,
    otaSupported: false,
  },
  {
    id: 'pc-macos',
    displayName: 'Mac (macOS)',
    architecture: 'Apple Silicon (AArch64) or Intel (x86_64)',
    archType: 'aarch64',
    transport: 'native',
    capabilities: ['shell', 'file', 'browser', 'http', 'memory'],
    category: 'host',
    description: 'A Mac running macOS 12 (Monterey) or later. Both Apple Silicon (M1/M2/M3/M4) and Intel Macs are supported. Runs the core brain agent and the native Tauri GUI.',
    recommendedToolchains: ['rust-cargo'],
    firmwareSupported: false,
    otaSupported: false,
  },
  {
    id: 'pc-windows',
    displayName: 'PC / Laptop (Windows)',
    architecture: 'x86_64 / AMD64',
    archType: 'x86_64',
    transport: 'native',
    capabilities: ['shell', 'file', 'browser', 'http', 'memory'],
    category: 'host',
    description: 'A Windows 10/11 PC or laptop. Requires Windows Subsystem for Linux 2 (WSL2) for the best experience, or native Windows build tools. Runs the core brain agent and the native Tauri GUI.',
    recommendedToolchains: ['rust-cargo'],
    firmwareSupported: false,
    otaSupported: false,
  },
  {
    id: 'raspberry-pi',
    displayName: 'Raspberry Pi (3, 4, 5, Zero 2 W)',
    architecture: 'AArch64 (ARM Cortex-A)',
    archType: 'aarch64',
    transport: 'native',
    capabilities: ['gpio', 'camera_capture', 'audio_sample', 'i2c', 'spi', 'pwm'],
    category: 'host',
    description: 'The Raspberry Pi family running Raspberry Pi OS (64-bit recommended). Can act as both the host brain AND a peripheral node. Uses rppal for GPIO and libcamera for camera capture.',
    recommendedToolchains: ['rust-cargo'],
    firmwareSupported: false,
    otaSupported: true,
    otaMethod: 'apt/pip + service restart via SSH',
    purchaseNote: 'Raspberry Pi 4 (4GB) or Pi 5 recommended for running the full brain agent. Pi Zero 2 W is suitable as a lightweight peripheral node only.',
  },
  {
    id: 'nanopi-neo3',
    displayName: 'NanoPi Neo3',
    architecture: 'AArch64 (RK3328 ARM Cortex-A53 @ 1 GHz)',
    archType: 'aarch64',
    transport: 'native',
    capabilities: ['gpio', 'i2c', 'spi', 'pwm'],
    category: 'host',
    description: 'The NanoPi Neo3 is a compact, low-power ARM SBC from FriendlyElec running Armbian. It is the reference host board for the Phase 13 deployment scenario. Uses Linux sysfs for GPIO.',
    recommendedToolchains: ['rust-cargo'],
    firmwareSupported: false,
    otaSupported: true,
    otaMethod: 'apt + service restart',
    purchaseNote: 'Available from FriendlyElec. Requires Armbian OS. A great low-power alternative to Raspberry Pi.',
  },
  {
    id: 'jetson-nano',
    displayName: 'NVIDIA Jetson Nano',
    architecture: 'AArch64 (NVIDIA Tegra X1)',
    archType: 'aarch64',
    transport: 'native',
    capabilities: ['gpio', 'i2c', 'spi', 'pwm', 'camera_capture', 'cuda'],
    category: 'host',
    description: 'The NVIDIA Jetson Nano is a powerful AI-focused SBC with CUDA GPU acceleration. Ideal for vision-heavy deployments. Runs JetPack OS (Ubuntu-based).',
    recommendedToolchains: ['rust-cargo'],
    firmwareSupported: false,
    otaSupported: true,
    otaMethod: 'apt + service restart',
  },

  // ── ESP32 Family ──────────────────────────────────────────────────────────
  {
    id: 'waveshare-esp32-s3-touch-lcd-2.1',
    displayName: 'Waveshare ESP32-S3 Touch LCD 2.1"',
    architecture: 'ESP32-S3 Xtensa LX7 @ 240 MHz',
    archType: 'x86_64', // Xtensa, but we use this for toolchain purposes
    transport: 'serial',
    capabilities: ['gpio', 'display', 'touch', 'audio_sample', 'wifi', 'camera_capture'],
    category: 'esp32',
    description: 'A feature-rich ESP32-S3 board with a 2.1" round capacitive touch LCD, I2S microphone support, and OV2640 camera connector. The reference display/sound node in the Phase 13 deployment. Connects via USB-C.',
    recommendedToolchains: ['rust-cargo', 'esp-idf', 'vscode-platformio'],
    firmwareSupported: true,
    otaSupported: true,
    otaMethod: 'HTTP OTA via esp-idf-svc',
    usbDriverNote: 'Uses a CH343P USB-to-serial chip. Install the CH343 driver on Windows/macOS.',
    purchaseNote: 'Available from Waveshare. Search for "Waveshare ESP32-S3 Touch LCD 2.1".',
  },
  {
    id: 'xiao-esp32s3-sense',
    displayName: 'Seeed XIAO ESP32S3-Sense',
    architecture: 'ESP32-S3 Xtensa LX7 @ 240 MHz',
    archType: 'x86_64',
    transport: 'serial',
    capabilities: ['gpio', 'camera_capture', 'audio_sample', 'wifi', 'ble'],
    category: 'esp32',
    description: 'A tiny but powerful ESP32-S3 module from Seeed Studio with a built-in OV2640 camera and PDM microphone. The reference vision node. Connects via USB-C.',
    recommendedToolchains: ['rust-cargo', 'esp-idf', 'vscode-platformio', 'arduino-ide'],
    firmwareSupported: true,
    otaSupported: true,
    otaMethod: 'HTTP OTA via esp-idf-svc',
    usbDriverNote: 'Uses native USB. No driver needed on most systems. On Windows, install the CP210x driver if not detected.',
    purchaseNote: 'Available from Seeed Studio and distributors like Mouser, DigiKey.',
  },
  {
    id: 'esp32-s3',
    displayName: 'Generic ESP32-S3 DevKit',
    architecture: 'ESP32-S3 Xtensa LX7 @ 240 MHz',
    archType: 'x86_64',
    transport: 'serial',
    capabilities: ['gpio', 'camera_capture', 'audio_sample', 'sensor_read', 'wifi'],
    category: 'esp32',
    description: 'A generic ESP32-S3 development board. Compatible with all Oh-Ben-Claw ESP32-S3 firmware. Specific pin assignments may vary from the reference boards.',
    recommendedToolchains: ['rust-cargo', 'esp-idf', 'vscode-platformio', 'arduino-ide'],
    firmwareSupported: true,
    otaSupported: true,
    otaMethod: 'HTTP OTA via esp-idf-svc',
    usbDriverNote: 'Most use CP2102 or CH340. Install the appropriate driver for your board.',
  },
  {
    id: 'esp32-c3',
    displayName: 'ESP32-C3 (RISC-V)',
    architecture: 'ESP32-C3 RISC-V single-core @ 160 MHz',
    archType: 'riscv',
    transport: 'serial',
    capabilities: ['gpio', 'i2c', 'spi', 'wifi', 'ble'],
    category: 'esp32',
    description: 'The ESP32-C3 is a low-cost RISC-V based microcontroller from Espressif with Wi-Fi and BLE. It does NOT support camera or I2S microphone. Good for sensor nodes and GPIO control.',
    recommendedToolchains: ['rust-cargo', 'esp-idf', 'vscode-platformio', 'arduino-ide'],
    firmwareSupported: false, // OBC firmware is ESP32-S3 specific
    otaSupported: true,
    otaMethod: 'HTTP OTA via esp-idf-svc',
    usbDriverNote: 'Uses native USB or CP2102. Install the CP210x driver on Windows/macOS if needed.',
  },
  {
    id: 'esp32',
    displayName: 'ESP32 (Original, Xtensa LX6)',
    architecture: 'ESP32 Xtensa LX6 dual-core @ 240 MHz',
    archType: 'x86_64',
    transport: 'serial',
    capabilities: ['gpio', 'wifi', 'ble'],
    category: 'esp32',
    description: 'The original ESP32 with dual Xtensa LX6 cores. Supported for basic GPIO and WiFi. Does not support the full OBC ESP32-S3 firmware but can run custom Arduino/ESP-IDF sketches.',
    recommendedToolchains: ['arduino-ide', 'vscode-platformio', 'esp-idf'],
    firmwareSupported: false,
    otaSupported: true,
    otaMethod: 'ArduinoOTA or HTTP OTA',
    usbDriverNote: 'Most boards use CP2102 or CH340. Install the appropriate driver.',
  },

  // ── Raspberry Pi Pico Family ───────────────────────────────────────────────
  {
    id: 'raspberry-pi-pico-w',
    displayName: 'Raspberry Pi Pico W',
    architecture: 'RP2040 dual-core ARM Cortex-M0+ @ 133 MHz (Wi-Fi)',
    archType: 'arm32',
    transport: 'serial',
    capabilities: ['gpio', 'analog_read', 'i2c', 'spi', 'pwm', 'wifi'],
    category: 'rpi',
    description: 'The Raspberry Pi Pico W adds Wi-Fi to the original Pico. Can connect to the Oh-Ben-Claw MQTT spine wirelessly. Programmed via MicroPython or C/C++ SDK.',
    recommendedToolchains: ['arduino-ide', 'vscode-platformio'],
    firmwareSupported: false,
    otaSupported: false,
    usbDriverNote: 'No driver needed. Appears as a USB mass storage device when in bootloader mode.',
  },
  {
    id: 'raspberry-pi-pico2',
    displayName: 'Raspberry Pi Pico 2',
    architecture: 'RP2350 dual-core ARM Cortex-M33 @ 150 MHz',
    archType: 'arm32',
    transport: 'serial',
    capabilities: ['gpio', 'analog_read', 'i2c', 'spi', 'pwm'],
    category: 'rpi',
    description: 'The Raspberry Pi Pico 2 features the newer RP2350 chip with improved performance and security features. Programmed via C/C++ SDK or MicroPython.',
    recommendedToolchains: ['arduino-ide', 'vscode-platformio'],
    firmwareSupported: false,
    otaSupported: false,
    usbDriverNote: 'No driver needed. Appears as a USB mass storage device when in bootloader mode.',
  },

  // ── Arduino Family ────────────────────────────────────────────────────────
  {
    id: 'arduino-uno',
    displayName: 'Arduino Uno (Rev3 / Rev4)',
    architecture: 'AVR ATmega328P @ 16 MHz (Rev3) / Renesas RA4M1 @ 48 MHz (Rev4)',
    archType: 'arm32',
    transport: 'serial',
    capabilities: ['gpio', 'analog_read', 'analog_write'],
    category: 'arduino',
    description: 'The classic Arduino Uno. Connects via USB and communicates with the Oh-Ben-Claw host over serial JSON-RPC. Flash the companion Arduino sketch from the Oh-Ben-Claw firmware directory.',
    recommendedToolchains: ['arduino-ide', 'vscode-platformio'],
    firmwareSupported: false,
    otaSupported: false,
    usbDriverNote: 'Rev3 uses ATmega16U2 USB bridge (no driver needed on most systems). Rev4 uses native USB.',
    purchaseNote: 'The most beginner-friendly board. Rev4 Minima is recommended for new users.',
  },
  {
    id: 'arduino-mega',
    displayName: 'Arduino Mega 2560',
    architecture: 'AVR ATmega2560 @ 16 MHz',
    archType: 'arm32',
    transport: 'serial',
    capabilities: ['gpio', 'analog_read', 'analog_write'],
    category: 'arduino',
    description: 'The Arduino Mega has more GPIO pins and serial ports than the Uno. Ideal for projects requiring many connections.',
    recommendedToolchains: ['arduino-ide', 'vscode-platformio'],
    firmwareSupported: false,
    otaSupported: false,
    usbDriverNote: 'Uses ATmega16U2 USB bridge. No driver needed on most systems.',
  },
  {
    id: 'arduino-nano-33-ble',
    displayName: 'Arduino Nano 33 BLE Sense',
    architecture: 'nRF52840 ARM Cortex-M4F @ 64 MHz',
    archType: 'arm32',
    transport: 'serial',
    capabilities: ['gpio', 'analog_read', 'i2c', 'spi', 'ble', 'sensor_read'],
    category: 'arduino',
    description: 'A compact Arduino with Bluetooth 5.0, IMU, microphone, and multiple environmental sensors built in. Good for portable sensor nodes.',
    recommendedToolchains: ['arduino-ide', 'vscode-platformio'],
    firmwareSupported: false,
    otaSupported: false,
    usbDriverNote: 'Uses native USB. No driver needed.',
  },

  // ── STM32 Family ──────────────────────────────────────────────────────────
  {
    id: 'nucleo-f401re',
    displayName: 'STM32 Nucleo-F401RE',
    architecture: 'ARM Cortex-M4 @ 84 MHz (STM32F401RE)',
    archType: 'arm32',
    transport: 'probe',
    capabilities: ['gpio', 'analog_read', 'analog_write', 'i2c', 'spi', 'pwm', 'dac'],
    category: 'stm32',
    description: 'An STM32 Nucleo development board with an embedded ST-Link V2 debug probe. Communicates with the Oh-Ben-Claw host via RTT (SEGGER Real-Time Transfer) over the ST-Link USB connection. Requires probe-rs.',
    recommendedToolchains: ['probe-rs', 'vscode-platformio'],
    firmwareSupported: false,
    otaSupported: true,
    otaMethod: 'probe-rs flash via ST-Link',
    usbDriverNote: 'Requires ST-Link USB drivers. On Windows, install the ST-Link driver from STMicroelectronics. On Linux/macOS, install udev rules.',
  },
  {
    id: 'nucleo-h743zi',
    displayName: 'STM32 Nucleo-H743ZI',
    architecture: 'ARM Cortex-M7 @ 480 MHz (STM32H743ZI)',
    archType: 'arm32',
    transport: 'probe',
    capabilities: ['gpio', 'analog_read', 'analog_write', 'i2c', 'spi', 'pwm', 'dac', 'can'],
    category: 'stm32',
    description: 'A high-performance STM32H7 Nucleo board with embedded ST-Link V3. Suitable for demanding real-time control applications.',
    recommendedToolchains: ['probe-rs', 'vscode-platformio'],
    firmwareSupported: false,
    otaSupported: true,
    otaMethod: 'probe-rs flash via ST-Link',
    usbDriverNote: 'Requires ST-Link USB drivers. See Nucleo-F401RE note above.',
  },

  // ── Other ─────────────────────────────────────────────────────────────────
  {
    id: 'sipeed-6plus1-mic-array',
    displayName: 'Sipeed 6+1 Mic Array',
    architecture: '7-microphone circular array with DSP',
    archType: 'x86_64',
    transport: 'serial',
    capabilities: ['audio_sample'],
    category: 'other',
    description: 'A USB audio device with 6 MEMS microphones in a circular array plus one central microphone. Provides far-field audio capture. Appears as a standard USB audio device (UAC1) — no special firmware needed.',
    recommendedToolchains: [],
    firmwareSupported: false,
    otaSupported: false,
    usbDriverNote: 'No driver needed. Plug in via USB and it appears as a standard audio input device.',
    purchaseNote: 'Available from Sipeed. A great companion for the NanoPi Neo3 or Raspberry Pi.',
  },
  {
    id: 'teensy-4.1',
    displayName: 'Teensy 4.1',
    architecture: 'NXP i.MX RT1062 ARM Cortex-M7 @ 600 MHz',
    archType: 'arm32',
    transport: 'serial',
    capabilities: ['gpio', 'analog_read', 'analog_write', 'i2c', 'spi', 'pwm', 'dac', 'can'],
    category: 'other',
    description: 'The Teensy 4.1 is an extremely fast microcontroller board from PJRC. Excellent for high-speed signal processing and real-time control. Programmed via the Teensyduino add-on for Arduino IDE.',
    recommendedToolchains: ['arduino-ide'],
    firmwareSupported: false,
    otaSupported: false,
    usbDriverNote: 'Requires the Teensy loader application. Install Teensyduino from pjrc.com.',
  },
];

export const BOARD_BY_ID = Object.fromEntries(BOARDS.map(b => [b.id, b]));

// ── Accessories ────────────────────────────────────────────────────────────────
export interface AccessoryInfo {
  id: string;
  displayName: string;
  bus: string;
  defaultAddress?: string;
  capabilities: string[];
  description: string;
}

export const ACCESSORIES: AccessoryInfo[] = [
  { id: 'bme280', displayName: 'BME280', bus: 'I2C', defaultAddress: '0x76', capabilities: ['sensor_read'], description: 'Temperature, Humidity, and Barometric Pressure sensor. Very popular for weather stations.' },
  { id: 'bmp388', displayName: 'BMP388', bus: 'I2C', defaultAddress: '0x77', capabilities: ['sensor_read'], description: 'High-precision Pressure and Altitude sensor from Bosch.' },
  { id: 'aht20', displayName: 'AHT20', bus: 'I2C', defaultAddress: '0x38', capabilities: ['sensor_read'], description: 'Temperature and Humidity sensor. Low cost and easy to use.' },
  { id: 'mpu6050', displayName: 'MPU-6050', bus: 'I2C', defaultAddress: '0x68', capabilities: ['sensor_read'], description: '6-axis IMU: 3-axis Accelerometer and 3-axis Gyroscope.' },
  { id: 'ssd1306', displayName: 'SSD1306 OLED', bus: 'I2C', defaultAddress: '0x3C', capabilities: ['display'], description: '128×64 pixel monochrome OLED display. Common small display for sensor readouts.' },
  { id: 'dht22', displayName: 'DHT22', bus: 'GPIO', capabilities: ['sensor_read'], description: 'Temperature and Humidity sensor with single-wire protocol. Slightly more accurate than DHT11.' },
  { id: 'dht11', displayName: 'DHT11', bus: 'GPIO', capabilities: ['sensor_read'], description: 'Basic Temperature and Humidity sensor. Lower accuracy than DHT22 but very low cost.' },
  { id: 'ds18b20', displayName: 'DS18B20', bus: '1-Wire', capabilities: ['sensor_read'], description: 'Waterproof digital temperature sensor. Great for liquid temperature measurement.' },
  { id: 'ina260', displayName: 'INA260', bus: 'I2C', defaultAddress: '0x40', capabilities: ['sensor_read'], description: 'Voltage, Current, and Power monitor. Useful for battery monitoring.' },
  { id: 'ads1115', displayName: 'ADS1115', bus: 'I2C', defaultAddress: '0x48', capabilities: ['analog_read'], description: '16-bit 4-channel ADC. Adds precise analog inputs to boards that lack them (e.g., Raspberry Pi).' },
];

// ── Feature Desires ────────────────────────────────────────────────────────────
export interface FeatureDesireInfo {
  id: string;
  label: string;
  description: string;
  icon: string;
  requiredCapabilities: string[];
  suggestedBoards: string[];
}

export const FEATURE_DESIRES: FeatureDesireInfo[] = [
  {
    id: 'Vision',
    label: 'Vision (Camera)',
    description: 'See the world through a camera. Capture images and analyse them with AI.',
    icon: '📷',
    requiredCapabilities: ['camera_capture'],
    suggestedBoards: ['xiao-esp32s3-sense', 'waveshare-esp32-s3-touch-lcd-2.1', 'raspberry-pi'],
  },
  {
    id: 'Listening',
    label: 'Listening (Microphone)',
    description: 'Hear audio from the environment. Enables voice commands and ambient sound detection.',
    icon: '🎤',
    requiredCapabilities: ['audio_sample'],
    suggestedBoards: ['sipeed-6plus1-mic-array', 'xiao-esp32s3-sense', 'waveshare-esp32-s3-touch-lcd-2.1'],
  },
  {
    id: 'Speech',
    label: 'Speech (Speaker / TTS)',
    description: 'Speak responses aloud via text-to-speech. Requires a speaker connected to an I2S output.',
    icon: '🔊',
    requiredCapabilities: ['audio_sample'],
    suggestedBoards: ['waveshare-esp32-s3-touch-lcd-2.1', 'raspberry-pi'],
  },
  {
    id: 'DisplayOutput',
    label: 'Display Output (Screen)',
    description: 'Show information on a screen. Render text, images, and UI elements.',
    icon: '🖥️',
    requiredCapabilities: ['display'],
    suggestedBoards: ['waveshare-esp32-s3-touch-lcd-2.1'],
  },
  {
    id: 'EnvironmentalSensing',
    label: 'Environmental Sensing',
    description: 'Monitor temperature, humidity, pressure, and other environmental conditions.',
    icon: '🌡️',
    requiredCapabilities: ['sensor_read'],
    suggestedBoards: ['raspberry-pi', 'nanopi-neo3', 'arduino-uno'],
  },
  {
    id: 'WirelessMesh',
    label: 'Wireless Mesh (MQTT)',
    description: 'Connect multiple nodes wirelessly over Wi-Fi using the MQTT spine.',
    icon: '📡',
    requiredCapabilities: ['wifi'],
    suggestedBoards: ['esp32-s3', 'xiao-esp32s3-sense', 'raspberry-pi'],
  },
  {
    id: 'EdgeInference',
    label: 'Edge AI (On-Device LLM)',
    description: 'Run a local LLM on the device without a cloud connection. Requires a capable host board.',
    icon: '🧠',
    requiredCapabilities: [],
    suggestedBoards: ['jetson-nano', 'raspberry-pi'],
  },
  {
    id: 'PersistentMemory',
    label: 'Persistent Memory',
    description: 'Remember conversations and context across sessions using a local SQLite database.',
    icon: '💾',
    requiredCapabilities: [],
    suggestedBoards: ['pc-linux', 'pc-macos', 'pc-windows', 'raspberry-pi', 'nanopi-neo3'],
  },
  {
    id: 'OTA',
    label: 'OTA Firmware Updates',
    description: 'Update firmware on peripheral nodes over-the-air without physical access.',
    icon: '🔄',
    requiredCapabilities: ['wifi'],
    suggestedBoards: ['esp32-s3', 'xiao-esp32s3-sense', 'waveshare-esp32-s3-touch-lcd-2.1'],
  },
];
