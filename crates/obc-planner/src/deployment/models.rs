//! On-device models the planner can put on an edge host (Phase 20 plan P4).
//!
//! The planner writes `[edge].local_model` and one `[[edge.models]]` entry when
//! the inventory asks for [`FeatureDesire::EdgeInference`] on a single-board
//! host. Only the host runs a model today (the sub-agents sit on
//! microcontrollers), so "per node role" means the orchestrator's board.
//!
//! # The RAM rule
//!
//! A model fits when [`EdgeModel::ram_mb`] is at most the host's budget:
//! its RAM ([`HardwareItem::ram_mb`], or [`board_ram_mb`]'s floor) less
//! [`HOST_RESERVE_MB`] for Linux, the agent and the model server. A model that
//! does not fit is refused, never swapped for another one: the operator asked
//! for it by name, and a different model would answer differently.
//!
//! [`FeatureDesire::EdgeInference`]: crate::deployment::inventory::FeatureDesire::EdgeInference
//! [`HardwareItem::ram_mb`]: crate::deployment::inventory::HardwareItem::ram_mb

/// RAM kept back on the host for Linux, the agent and the model server, in MB.
pub const HOST_RESERVE_MB: u32 = 384;

/// What a loaded model needs beyond its weights at a 2k-token context: the KV
/// cache and the server's own working set, in MB. A rule of thumb, not a
/// measurement; [`EdgeModel::ram_mb`] adds a quarter of the weights on top.
pub const RUNTIME_OVERHEAD_MB: u32 = 256;

/// One model in the planner's catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeModel {
    /// The Ollama tag, which is also the name `[edge].local_model` takes.
    pub name: &'static str,
    /// Parameter count, as the model's publisher states it.
    pub params: &'static str,
    /// Quantisation of the default tag.
    pub quantisation: &'static str,
    /// Download size in MB, from the Ollama library listing (2026-11-01).
    pub size_mb: u32,
    /// Whether the planner picks it on its own for the reflex tier. Larger
    /// models are in the catalogue so an operator can ask for them, but a reflex
    /// call has a 2 s ceiling (`[edge].reflex_timeout_ms`) and a single-board
    /// CPU does not answer from a 4B model inside it.
    pub reflex: bool,
}

impl EdgeModel {
    /// Estimated resident RAM in MB: the weights, a quarter more for the
    /// context and activations, and [`RUNTIME_OVERHEAD_MB`].
    pub fn ram_mb(&self) -> u32 {
        self.size_mb + self.size_mb / 4 + RUNTIME_OVERHEAD_MB
    }

    /// Where `obc fleet models pull` gets it from.
    pub fn source(&self) -> String {
        format!("ollama://{}", self.name)
    }
}

/// The catalogue, smallest first. Sizes and quantisations are the default
/// tags' as listed at ollama.com/library on 2026-11-01.
pub static EDGE_MODELS: &[EdgeModel] = &[
    EdgeModel {
        name: "gemma3:270m",
        params: "270M",
        quantisation: "Q8_0",
        size_mb: 292,
        reflex: true,
    },
    EdgeModel {
        name: "qwen3:0.6b",
        params: "0.6B",
        quantisation: "Q4_K_M",
        size_mb: 523,
        reflex: true,
    },
    EdgeModel {
        name: "gemma3:1b",
        params: "1B",
        quantisation: "Q4_K_M",
        size_mb: 806,
        reflex: true,
    },
    EdgeModel {
        name: "qwen3:1.7b",
        params: "1.7B",
        quantisation: "Q4_K_M",
        size_mb: 1434,
        reflex: true,
    },
    EdgeModel {
        name: "qwen3:4b",
        params: "4B",
        quantisation: "Q4_K_M",
        size_mb: 2560,
        reflex: false,
    },
    EdgeModel {
        name: "gemma3n:e2b",
        params: "E2B",
        quantisation: "Q4_K_M",
        size_mb: 7987,
        reflex: false,
    },
];

/// Look a model up by its Ollama tag. `name` and `name:latest` are the same.
pub fn find(name: &str) -> Option<&'static EdgeModel> {
    let name = name.trim().trim_end_matches(":latest");
    EDGE_MODELS.iter().find(|m| m.name == name)
}

/// The RAM the planner assumes for a host board when the inventory does not
/// say, in MB. A conservative floor (the smallest variant the planner knows
/// of), not the spec sheet: set `ram_mb` on the host item for a bigger board.
pub fn board_ram_mb(board: &str) -> Option<u32> {
    match board {
        "nanopi-neo3" | "raspberry-pi-4" | "raspberry-pi-5" => Some(1024),
        _ => None,
    }
}

/// The host's model budget in MB: its RAM less [`HOST_RESERVE_MB`].
pub fn budget_mb(board: &str, ram_mb: Option<u32>) -> Option<u32> {
    ram_mb
        .or_else(|| board_ram_mb(board))
        .map(|ram| ram.saturating_sub(HOST_RESERVE_MB))
}

/// The model for the host, or why there is none. `Err` is a warning for the
/// operator, and no model is written to the config.
///
/// With `requested`, that model is checked against the budget and refused if
/// it does not fit or is not in the catalogue. Without it, the largest reflex
/// model that fits is chosen.
pub fn choose(
    requested: Option<&str>,
    board: &str,
    ram_mb: Option<u32>,
) -> Result<(&'static EdgeModel, u32), String> {
    let Some(budget) = budget_mb(board, ram_mb) else {
        return Err(format!(
            "Edge inference desired, but the planner does not know how much RAM \
             '{board}' has. Set ram_mb on the host item."
        ));
    };
    let largest_fit = || {
        EDGE_MODELS
            .iter()
            .rfind(|m| m.reflex && m.ram_mb() <= budget)
    };

    if let Some(name) = requested {
        let Some(model) = find(name) else {
            let known: Vec<&str> = EDGE_MODELS.iter().map(|m| m.name).collect();
            return Err(format!(
                "Local model '{name}' is not in the planner's catalogue, so its RAM \
                 need is unknown; it was not written to the config. Known models: {}.",
                known.join(", ")
            ));
        };
        if model.ram_mb() > budget {
            let instead = largest_fit()
                .map(|m| format!(" '{}' ({} MB) fits.", m.name, m.ram_mb()))
                .unwrap_or_default();
            return Err(format!(
                "Local model '{}' needs about {} MB of RAM; '{board}' has a {budget} MB \
                 budget ({HOST_RESERVE_MB} MB kept for the system). It was not written \
                 to the config.{instead}",
                model.name,
                model.ram_mb(),
            ));
        }
        return Ok((model, budget));
    }

    largest_fit().map(|m| (m, budget)).ok_or_else(|| {
        format!(
            "Edge inference desired, but no model in the planner's catalogue fits \
             '{board}''s {budget} MB budget ({HOST_RESERVE_MB} MB kept for the system)."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_is_smallest_first_and_names_are_unique() {
        for pair in EDGE_MODELS.windows(2) {
            assert!(pair[0].ram_mb() < pair[1].ram_mb(), "{pair:?}");
        }
        let mut names: Vec<&str> = EDGE_MODELS.iter().map(|m| m.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), EDGE_MODELS.len());
    }

    #[test]
    fn the_largest_reflex_model_that_fits_is_chosen() {
        // 1 GB floor: 640 MB budget, only the 270M model fits.
        let (m, budget) = choose(None, "nanopi-neo3", None).unwrap();
        assert_eq!((m.name, budget), ("gemma3:270m", 640));
        // 2 GB: 1664 MB budget.
        assert_eq!(
            choose(None, "nanopi-neo3", Some(2048)).unwrap().0.name,
            "gemma3:1b"
        );
        // 8 GB: a 4B model would fit, but it is not a reflex model.
        assert_eq!(
            choose(None, "raspberry-pi-5", Some(8192)).unwrap().0.name,
            "qwen3:1.7b"
        );
    }

    #[test]
    fn a_requested_model_that_does_not_fit_is_refused_not_swapped() {
        let err = choose(Some("qwen3:4b"), "raspberry-pi-4", Some(2048)).unwrap_err();
        assert!(err.contains("'qwen3:4b' needs about 3456 MB"), "{err}");
        assert!(err.contains("1664 MB budget"), "{err}");
        assert!(err.contains("'gemma3:1b' (1263 MB) fits"), "{err}");

        // It fits on the 8 GB board, and a requested model need not be a reflex one.
        let (m, _) = choose(Some("qwen3:4b:latest"), "raspberry-pi-5", Some(8192)).unwrap();
        assert_eq!(m.name, "qwen3:4b");
    }

    #[test]
    fn an_unknown_model_or_board_is_refused_with_the_reason() {
        let err = choose(Some("mystery:7b"), "nanopi-neo3", None).unwrap_err();
        assert!(err.contains("not in the planner's catalogue"), "{err}");
        let err = choose(None, "jetson-orin-nano", None).unwrap_err();
        assert!(err.contains("Set ram_mb on the host item"), "{err}");
        // With the RAM given, any board has a budget.
        assert!(choose(None, "jetson-orin-nano", Some(8192)).is_ok());
        // A board too small for anything.
        let err = choose(None, "nanopi-neo3", Some(512)).unwrap_err();
        assert!(
            err.contains("no model in the planner's catalogue fits"),
            "{err}"
        );
    }
}
