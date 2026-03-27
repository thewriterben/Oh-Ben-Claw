# Oh-Ben-Claw Deployment Guide Generator

An interactive, chat-style web application that generates step-by-step deployment guides for [Oh-Ben-Claw](https://github.com/thewriterben/Oh-Ben-Claw) and [OBC-deployment-generator](https://github.com/thewriterben/OBC-deployment-generator).

## Live App

**[Launch the Guide Generator](https://thewriterben.github.io/Oh-Ben-Claw/guide/)**

## Features

- Chat-style wizard — walks users through goal, OS, hardware, features, toolchain, and LLM provider selection
- Zero-experience friendly — every command and code block is included with plain-English explanations
- All hardware scenarios — PC (Linux/macOS/Windows), Raspberry Pi, NanoPi Neo3, Jetson Nano, ESP32-S3, ESP32-C3, Arduino, STM32, Raspberry Pi Pico, Teensy, and more
- All toolchains — Rust/Cargo (recommended), Arduino IDE, VS Code + PlatformIO, ESP-IDF, probe-rs
- OTA update workflows — over-the-air firmware update instructions for ESP32, Raspberry Pi, and STM32
- Live repo data — fetches the latest release tag and commit info from GitHub at runtime
- PDF export — download your generated guide as a PDF
- Markdown export — download your generated guide as a Markdown file

## Development

```bash
pnpm install
pnpm dev
pnpm build
```

## Deployment

Automatically deployed to GitHub Pages on every push to `main` that modifies files in `guide-generator/`.
See `.github/workflows/deploy-guide.yml`.
