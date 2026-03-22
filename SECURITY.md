# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

If you discover a security vulnerability, please send an email to the maintainer at the address listed in the GitHub profile, or open a [GitHub Security Advisory](https://github.com/thewriterben/Oh-Ben-Claw/security/advisories/new).

Include as much of the following information as possible:

- A description of the vulnerability and its potential impact
- Steps to reproduce or proof-of-concept code
- Affected versions
- Any suggested mitigations

You should receive an acknowledgement within 48 hours. If the issue is confirmed, a patch will be released as soon as possible, typically within 7 days for critical issues.

## Security Architecture

Oh-Ben-Claw applies defence in depth across multiple layers:

### API Key Protection

All API keys and credentials are stored in an AES-256-GCM encrypted vault
(`~/.oh-ben-claw/vault.db`), never written to `config.toml` in plain text.
The vault uses Argon2id key derivation with a random 16-byte salt and requires
the master password to unlock at startup.

### MQTT Spine Security

The MQTT spine supports TLS (via `rumqttc`'s `rustls` backend) for all
broker connections. MQTT username/password credentials are stored in the
encrypted vault, not in the config file.

### Peripheral Node Pairing

Before a peripheral node's tools are accepted into the brain's registry, the
node must complete a pairing handshake:

1. The brain generates a random 256-bit pairing secret.
2. The node signs its `NodeAnnouncement` with an HMAC-SHA256 tag using the shared secret.
3. The brain verifies the HMAC tag and enforces a 5-minute replay window.
4. Nodes that fail verification are quarantined and their tools are rejected.

Pairing secrets must be at least 16 characters; `NodePairingManager::validate_secret()`
enforces this at startup. Poisoned `Mutex` states are recovered gracefully rather
than panicking.

### Tool Policy Engine

All tool calls are evaluated against a configurable policy before execution
(`src/security/policy.rs`):

- Rules match tool names via glob patterns (`shell*`, `gpio_write`, …).
- Rules can inspect argument values with `arg_contains` filters.
- Actions: `allow`, `deny`, or `audit` (log and allow).
- The glob matcher uses a maximum recursion depth of 64 to prevent ReDoS
  attacks on adversarial glob patterns.

### Sandboxed Tool Execution

The `[runtime]` config section selects the tool execution runtime:

- **`native`** (default) — runs shell commands directly on the host.
- **`docker`** — wraps every shell command in a fresh, ephemeral Docker
  container with configurable memory limits and network isolation. Recommended
  for untrusted skill manifests downloaded from ClawHub.

### Human-in-the-Loop Approval

In `supervised` or `manual` autonomy mode, every tool call is presented to the
operator for explicit approval before execution. A session-scoped allowlist
remembers previous decisions. All approvals and denials are written to an
immutable audit log.

## Known Advisory Status

| Advisory | Crate | Status |
|---|---|---|
| RUSTSEC-2025-0134 | `rustls-pemfile 2.2.0` (via `rumqttc 0.24`) | Acknowledged — unmaintained classification only, no exploitable vulnerability. Tracked for upgrade when `rumqttc` adopts `rustls-pki-types`. |
| RUSTSEC-2024-0436 | `paste` (via `ratatui`) | **Resolved** — `ratatui` upgraded to 0.30, `paste` no longer a transitive dependency. |
| RUSTSEC-2026-0002 | `lru 0.12.5` (via `ratatui`) | **Resolved** — `ratatui` 0.30 uses `lru 0.16.3` which is not affected. |

Please review `src/security/` for full implementation details.
