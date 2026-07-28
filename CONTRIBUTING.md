# Contributing to Oh-Ben-Claw

## Where work happens

Oh-Ben-Claw is the **upstream core**: the Rust agent, the world model, the
reflex/System-2 layer, the tool and channel surface, the MQTT/LoRa spine.

[**OBC-Prime**](https://github.com/thewriterben/OBC-Prime) is the **public
project**: the reference bodies you can actually run, the board registry, the
tri-implementation planner parity harness, and the documentation a newcomer
meets first.

Which repo a change belongs in:

| Change | Repo |
|---|---|
| Agent behaviour, memory, tools, channels, spine, safety | **Oh-Ben-Claw** |
| Board registry, golden fixtures, planner WASM | **Oh-Ben-Claw** — they are emitted from here and vendored downstream |
| Reference bodies, quickstarts, deployment docs | **OBC-Prime** |
| Anything a first-time user reads | **OBC-Prime** |

Registry, fixtures and the planner WASM exist in more than one repository and
are kept in step by hash, not by convention: `scripts/sync_upstream.py` in
OBC-Prime plus a CI parity gate. Edit them **here**; never hand-copy them
downstream.

## Code of Conduct

By participating you agree to abide by our [Code of Conduct](CODE_OF_CONDUCT.md).

## Reporting bugs

1. Search [existing issues](https://github.com/thewriterben/Oh-Ben-Claw/issues).
2. Open one with the **Bug Report** template.
3. Include your OS, `rustc --version`, and reproduction steps.

For anything the agent believes that it should not — a stale sensor reading, a
belief that survived its source going away — please include the output of
`oh-ben-claw doctor` and, if you can, the relevant rows from `world_facts`.
Belief-revision bugs are close to unreproducible without the provenance.

## Submitting pull requests

```bash
git checkout -b feat/my-feature
```

Then, before opening the PR, run exactly what CI runs:

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

All four pass on a clean checkout of `main`. If one fails and you did not
expect it to, that is a bug worth reporting on its own — a check that fails by
default teaches everyone who meets it to ignore it.

Two notes on the clippy line, both learned the hard way:

- `--all-targets` matters. Without it, clippy never looks at `tests/`, and a
  crate-level survey that skipped `tests/` is exactly how a module with
  thirteen live references nearly got deleted.
- The crate carries **no blanket `#![allow(...)]`**, and must not gain one. If
  a lint is wrong about your code, suppress it at the item or the file, with a
  comment saying why. `src/channels/slack.rs` is the worked example: its
  structs mirror a vendor's webhook payload, so fields this code never reads
  still earn their place.

## Development setup

- **Rust** stable (see `rust-toolchain.toml`):
  `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **MQTT broker** for spine integration tests, e.g. `mosquitto`
- **Node.js 20+** and **pnpm** for GUI work

Recommended once per clone, so the first-ever `cargo fmt --all` does not sit on
top of every `git blame`:

```bash
git config blame.ignoreRevsFile .git-blame-ignore-revs
```

### Building

```bash
# Core agent (default features: hardware + mqtt-spine + world-anchor)
cargo build --workspace

# NanoPi Neo3 cross-compile (from Linux/macOS)
cargo build --target aarch64-unknown-linux-gnu --features hardware,peripheral-nanopi

# GUI (Tauri 2 + React)
cd gui && pnpm install && pnpm tauri dev
```

The two firmware crates are **not** workspace members — they live in
`[workspace] exclude` because they need the Espressif toolchain and their own
`Cargo.lock`. `cargo build --workspace` will not touch them, and you do not
need `--exclude obc-esp32-s3` to keep it from trying.

### ESP32-S3 / Heltec LoRa firmware

```bash
cargo install espup && espup install && source ~/export-esp.sh
cd firmware/obc-esp32-s3
cargo build --release
cargo espflash flash --monitor
```

### Deployment planner

There is **no `deployment` subcommand** — an earlier version of this file said
there was, and I copied the claim forward without running it. The planner is a
library surface, reached from tests and from the two emitters:

```bash
cargo run --bin emit-registry              # registry/registry.json
cargo run --bin emit-firmware-templates    # firmware scaffolds
cargo test --test planner_parity           # planner vs the committed goldens
```

The interactive way to drive it is the deployment generator app, whose
TypeScript port is byte-identical to this one and enforced so by
`parity/MANIFEST.json` in OBC-Prime.

## Style

- `rustfmt` defaults, `clippy` at `-D warnings`. Both enforced in CI.
- Public items get `///` docs. Say what it is *for*, not what it is called.
- `anyhow` for application errors, `thiserror` for library errors.
- `&Path`, not `&PathBuf`, in signatures.
- New behaviour gets a test in the same module. A fix with no test is a claim.
- Comments should say *why*, especially where the obvious thing is wrong. The
  code can already be read; the reasoning cannot.

### On adding config

A config key that parses cleanly and changes nothing is worse than a missing
feature, because the config file is the surface a user actually reads. If you
add a key, wire it in the same PR and test that it has an effect. Several keys
have been deleted for failing exactly this.

## Commit messages

Conventional prefixes — `feat:`, `fix:`, `docs:`, `chore:`, `style:`, `lint:`,
`curation:`. The body matters more than the prefix: say what was wrong, what
you changed, and what you are *not* claiming. Known weaknesses stated in the
message are worth more than a clean one that hides them.

## License

By contributing you agree that your contributions are licensed under the
[MIT License](LICENSE).
