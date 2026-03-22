# Contributing to Oh-Ben-Claw

Thank you for your interest in contributing! This document explains how to get involved.

## Code of Conduct

By participating in this project, you agree to abide by our [Code of Conduct](CODE_OF_CONDUCT.md).

## How to Contribute

### Reporting Bugs

1. Search [existing issues](https://github.com/thewriterben/Oh-Ben-Claw/issues) to avoid duplicates.
2. Open a new issue using the **Bug Report** template.
3. Include your OS, Rust version (`rustc --version`), and reproduction steps.

### Suggesting Features

1. Search [existing issues](https://github.com/thewriterben/Oh-Ben-Claw/issues) to avoid duplicates.
2. Open a new issue using the **Feature Request** template.
3. Describe the use case and expected behaviour clearly.

### Submitting Pull Requests

1. Fork the repository and create a branch from `main`:
   ```bash
   git checkout -b feat/my-feature
   ```
2. Make your changes, following the style guide below.
3. Add or update tests as appropriate.
4. Ensure all CI checks pass locally:
   ```bash
   cargo build --workspace --exclude obc-esp32-s3
   cargo test --workspace --exclude obc-esp32-s3
   cargo clippy --workspace --exclude obc-esp32-s3 -- -D warnings
   cargo fmt --all --check
   ```
5. Open a pull request against `main` and fill out the PR template.

## Development Setup

### Prerequisites

- **Rust** (stable, see `rust-toolchain.toml` for the pinned version):
  `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **MQTT broker** for integration testing (e.g. `mosquitto`):
  `brew install mosquitto` or `apt install mosquitto`
- **Node.js 20+** and **pnpm** for GUI work:
  `npm install -g pnpm`

### Building

```bash
# Core agent (default features: hardware + mqtt-spine)
cargo build --workspace --exclude obc-esp32-s3

# With terminal dashboard
cargo build --workspace --exclude obc-esp32-s3 --features dashboard

# NanoPi Neo3 cross-compile (from Linux/macOS)
cargo build \
  --target aarch64-unknown-linux-gnu \
  --features hardware,peripheral-nanopi \
  --exclude obc-esp32-s3

# GUI (Tauri 2 + React 18)
cd gui && pnpm install && pnpm tauri dev
```

### Running Tests

```bash
cargo test --workspace --exclude obc-esp32-s3
```

### ESP32-S3 Firmware

```bash
# Install the ESP toolchain (once)
cargo install espup && espup install && source ~/export-esp.sh

# Build and flash (from firmware/obc-esp32-s3)
cd firmware/obc-esp32-s3
cargo build --release
cargo espflash flash --monitor
```

### Deployment Planner

```bash
# Run the static planner against the NanoPi reference scenario
cargo run -- deployment plan --scenario nanopi

# Run with the full LLM swarm (requires OPENAI_API_KEY)
cargo run -- deployment plan --scenario nanopi --llm-swarm
```

## Style Guide

- Follow standard Rust conventions (`rustfmt` and `clippy` are enforced in CI).
- Keep public APIs documented with `///` doc comments.
- Prefer `anyhow` for application-level errors and `thiserror` for library-level errors.
- New features should have corresponding tests in the same module.
- Use `&Path` instead of `&PathBuf` in function signatures (`clippy::ptr_arg`).
- Use `format!(...)` instead of `&format!(...)` when a value is expected (`clippy::needless_borrows_for_generic_args`).

## Commit Messages

Use conventional commits:

```
feat: add BME680 sensor support
fix: correct MQTT reconnect backoff
docs: update hardware setup guide
chore: bump tokio to 1.43
```

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
