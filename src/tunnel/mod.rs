//! Oh-Ben-Claw network tunnel subsystem.
//!
//! Provides secure tunnels for exposing the agent and peripheral nodes to the
//! internet, enabling remote access and control.
//!
//! # Supported Tunnels
//!
//! | Tunnel       | Status  | Notes                                        |
//! |--------------|---------|----------------------------------------------|
//! | Cloudflare   | Planned | Cloudflare Tunnel (cloudflared)              |
//! | ngrok        | Planned | ngrok tunnels                                |
//! | Tailscale    | Planned | WireGuard-based mesh VPN                     |
//! | Custom       | Planned | User-defined SSH or reverse proxy tunnel     |
