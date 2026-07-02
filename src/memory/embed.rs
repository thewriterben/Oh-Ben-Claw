//! Local embedding backend for episode retrieval (`semantic` cargo feature).
//!
//! Wraps [fastembed](https://github.com/Anush008/fastembed-rs) — ONNX
//! inference on CPU, model downloaded once to the local cache and fully
//! offline thereafter. Implements [`Embedder`](crate::memory::trajectory::Embedder),
//! the dense leg of the hybrid episode retrieval
//! (`TrajectoryStore::similar`). See `AI-Agents-Research-July2026.md` for the
//! selection rationale.

#[cfg(feature = "semantic")]
pub use backend::FastEmbedder;

#[cfg(feature = "semantic")]
mod backend {
    use crate::memory::trajectory::Embedder;
    use anyhow::Result;
    use std::sync::Mutex;

    /// fastembed-backed [`Embedder`]. Interior-mutex because fastembed's
    /// `embed` takes `&mut self`; calls are short and the record path is
    /// already serialized per store.
    pub struct FastEmbedder(Mutex<fastembed::TextEmbedding>);

    impl FastEmbedder {
        /// Initialize with fastembed's default model (downloads to the local
        /// HF cache on first use; offline afterwards).
        pub fn try_default() -> Result<Self> {
            let model = fastembed::TextEmbedding::try_new(Default::default())?;
            Ok(Self(Mutex::new(model)))
        }
    }

    impl Embedder for FastEmbedder {
        fn embed(&self, text: &str) -> Result<Vec<f32>> {
            let mut model = self.0.lock().unwrap_or_else(|p| p.into_inner());
            let mut out = model.embed(vec![text], None)?;
            out.pop()
                .ok_or_else(|| anyhow::anyhow!("embedding model returned no vector"))
        }
    }
}
