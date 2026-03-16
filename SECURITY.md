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

## Security Considerations

Oh-Ben-Claw handles:

- **API keys** — stored encrypted via `argon2` + `aes-gcm` in `~/.oh-ben-claw/`. Never committed to source control.
- **MQTT communication** — supports TLS via `rumqttc`'s `rustls` backend.
- **Peripheral pairing** — nodes authenticate via HMAC-SHA256 tokens.
- **Tool sandboxing** — shell commands are restricted by the policy engine in `src/security/`.

Please review `src/security/` for the current implementation details.
