//! The `[tunnel]` block - which tunnel provider, and the credentials it needs.
//!
//! Lives with the providers that read it, the way `obc_planner` owns
//! `DeploymentConfig`, `obc_conscience` owns `ConscienceConfig` and `obc_cost`
//! owns `CostConfig`. The root `Config` re-exports it, so
//! `crate::config::TunnelConfig` is unchanged.

use serde::{Deserialize, Serialize};
/// Configuration for the network tunnel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelConfig {
    /// Whether the tunnel is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// The tunnel backend: "cloudflare" or "tailscale".
    #[serde(default = "default_tunnel_backend")]
    pub backend: String,
    /// The local port the gateway listens on.
    #[serde(default = "default_tunnel_port")]
    pub local_port: u16,
    /// Named Cloudflare tunnel name (for persistent custom domains).
    #[serde(default)]
    pub named_tunnel: Option<String>,
    /// Cloudflare tunnel token (for named tunnels).
    #[serde(default)]
    pub token: Option<String>,
    /// Whether to enable Tailscale Funnel for public access.
    #[serde(default)]
    pub tailscale_funnel: bool,
}

fn default_tunnel_backend() -> String {
    "cloudflare".to_string()
}

fn default_tunnel_port() -> u16 {
    8080
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: default_tunnel_backend(),
            local_port: default_tunnel_port(),
            named_tunnel: None,
            token: None,
            tailscale_funnel: false,
        }
    }
}
