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

- Rust stable (see `rust-toolchain.toml` for the pinned version)
- An MQTT broker for integration testing (e.g., `mosquitto`)
- Node.js 20+ and `npm` for GUI work

### Building

```bash
# Core agent
cargo build --workspace --exclude obc-esp32-s3

# GUI (Tauri)
cd gui && npm ci && npm run build
```

### Running Tests

```bash
cargo test --workspace --exclude obc-esp32-s3
```

## Style Guide

- Follow standard Rust conventions (`rustfmt` and `clippy` are enforced in CI).
- Keep public APIs documented with `///` doc comments.
- Prefer `anyhow` for application-level errors and `thiserror` for library-level errors.
- New features should have corresponding tests in the same module.

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
