//! Mushroom body — a sparse-expansion memory over episode embeddings.
//!
//! The circuit this copies is the insect mushroom body as the fly connectomes
//! describe it: ~50 projection neurons fan out through a sparse random binary
//! matrix onto ~2,000 Kenyon cells (each summing ~6 random inputs), one
//! inhibitory neuron silences all but the top ~5%, and the surviving set is the
//! input's *tag*. Downstream, compartments each learn a valence for a tag by
//! moving only the synapses of the Kenyon cells that were active. Three
//! published algorithms fall out of that wiring and are what this module is:
//!
//! - **FlyHash** (Dasgupta, Stevens, Navlakha 2017, *Science*): the sparse
//!   projection + winner-take-all is a locality-sensitive hash.
//! - **The fly Bloom filter** (Dasgupta et al. 2018, *PNAS*): suppress the
//!   output weights of a tag's active cells when it is seen, let them recover
//!   with time, and the summed weight of a query's cells is a novelty score
//!   that is *distance*-sensitive (partial overlap → partial familiarity) and
//!   *time*-sensitive (familiarity fades).
//! - **FlyModel** (Shen, Dasgupta, Navlakha 2023, *Neural Computation*):
//!   because tags of unrelated inputs barely overlap, reinforcing one input's
//!   compartment weights leaves the others' untouched — an online classifier
//!   that resists catastrophic forgetting without replay.
//!
//! What this is *for* in the agent: before the reasoner sees an objective, say
//! whether it is new (no close precedent in memory) and, when it is not, how
//! objectives like it have tended to end. Both are computed from the same
//! embedding the dense retrieval leg already produces; nothing here calls a
//! model or the network.
//!
//! One step is not from the fly. Sentence embeddings share a large common
//! mean vector, and a random projection of that vector is what wins the
//! top-k — every objective gets nearly the same tag. So the body subtracts
//! the mean of its warm-up episodes from every embedding first (the
//! "all-but-the-top" correction, Mu & Viswanath 2018) — see
//! `MushroomConfig::warmup_episodes` for the measurement that forced it.
//!
//! What this is *not*: an ANN index. FlyHash does not compete with HNSW on
//! recall/QPS and is not used here for retrieval at all — the trajectory
//! store's candidate window is small enough for exact cosine.
//!
//! Everything is deterministic. The projection is drawn from an explicit seed
//! with the same splitmix64 the red-team generator uses; time enters only as
//! the caller's `now_ms`, never the wall clock; ties break by index. Same
//! seed, same inputs, same timestamps → byte-identical tags and scores.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// `[self_improvement.mushroom]` — every parameter of the circuit is explicit;
/// there are no constants in the code. Unknown keys fail the load.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MushroomConfig {
    /// Attach the mushroom body to the trajectory store. Needs the `semantic`
    /// leg (an embedder); without one the store refuses to attach it.
    #[serde(default)]
    pub enabled: bool,
    /// Seed for the random sparse projection. Changing it changes every tag,
    /// so the body is rebuilt from the stored episodes on the next open.
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// Kenyon cells — the expansion width. The fly has ~2,000 for ~50 inputs;
    /// a 384-d sentence embedding needs more room. Measured on the brain's own
    /// episodes (`tests/mushroom_real_episodes.rs`): 2,000 cells at 5% put
    /// first-seen objectives at a median novelty of 0.08 — indistinguishable
    /// from repeats — and 20,000 at 2% with centring put them at 0.28, with
    /// repeats still ≤ 0.002. Neither change did it alone (0.13 and 0.07).
    #[serde(default = "default_kenyon_cells")]
    pub kenyon_cells: usize,
    /// Distinct input dimensions each Kenyon cell sums (the fly: ~6). Clamped
    /// to the embedding dimension.
    #[serde(default = "default_inputs_per_cell")]
    pub inputs_per_cell: usize,
    /// Fraction of Kenyon cells that survive winner-take-all (the fly: ~0.05;
    /// see `kenyon_cells` for why the default is sparser).
    #[serde(default = "default_active_fraction")]
    pub active_fraction: f32,
    /// How long, in ms, until a seen tag is half-way back to novel.
    #[serde(default = "default_novelty_half_life_ms")]
    pub novelty_half_life_ms: u64,
    /// Novelty at or above this is reported as "no close precedent". Set
    /// from the brain's own 94 episodes under the defaults (see
    /// `tests/mushroom_real_episodes.rs`): exact repeats score ≤ 0.002,
    /// rewordings of a seen objective ("Note 2 for the record" after "Note
    /// 1", a second browser task like the first) 0.06–0.24, and objectives
    /// with no precedent 0.31–0.75, so the line sits in the gap. The first
    /// draft shipped 0.7, a guess, at which the body called 0 of 74 novel.
    #[serde(default = "default_novel_threshold")]
    pub novel_threshold: f32,
    /// Step size for compartment reinforcement, in `(0, 1]`.
    #[serde(default = "default_learning_rate")]
    pub learning_rate: f32,
    /// Share of a tag's cells that must carry evidence before an outcome
    /// prior is reported, in `(0, 1]`. Unrelated tags overlap by chance on
    /// about `active_fraction` of their cells; below this share the prior
    /// would be that noise, so it is `None` instead.
    #[serde(default = "default_prior_min_coverage")]
    pub prior_min_coverage: f32,
    /// Episodes the body takes in before it judges anything. They do two
    /// things. Their mean becomes the *centre* — the common component every
    /// later embedding has subtracted before hashing (sentence embeddings
    /// share a large mean vector, and a random projection of it dominates the
    /// top-k so that unrelated objectives get near-identical tags; removing
    /// the mean is the "all-but-the-top" correction of Mu & Viswanath 2018,
    /// and on the brain's own episodes it is what separates a first-seen
    /// objective from a repeat). And until they are in, novelty is not
    /// reported: a body that has seen nothing finds everything novel, which
    /// is true and useless. The centre is fixed once, from these episodes, so
    /// every tag the body holds was made the same way — it is a pure function
    /// of the first `warmup_episodes` stored vectors. `0` = no centring and
    /// no warm-up (the synthetic tests; not for real embeddings). The mean of
    /// one vector is that vector, so anything real should be well above 1.
    #[serde(default = "default_warmup_episodes")]
    pub warmup_episodes: usize,
}

fn default_seed() -> u64 {
    0x4D55_5348_524F_4F4D // "MUSHROOM"
}
fn default_kenyon_cells() -> usize {
    20_000
}
fn default_inputs_per_cell() -> usize {
    6
}
fn default_active_fraction() -> f32 {
    0.02
}
fn default_novelty_half_life_ms() -> u64 {
    7 * 24 * 60 * 60 * 1000
}
fn default_novel_threshold() -> f32 {
    0.25
}
fn default_learning_rate() -> f32 {
    0.2
}
fn default_warmup_episodes() -> usize {
    20
}
fn default_prior_min_coverage() -> f32 {
    0.5
}

impl Default for MushroomConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            seed: default_seed(),
            kenyon_cells: default_kenyon_cells(),
            inputs_per_cell: default_inputs_per_cell(),
            active_fraction: default_active_fraction(),
            novelty_half_life_ms: default_novelty_half_life_ms(),
            novel_threshold: default_novel_threshold(),
            learning_rate: default_learning_rate(),
            warmup_episodes: default_warmup_episodes(),
            prior_min_coverage: default_prior_min_coverage(),
        }
    }
}

impl MushroomConfig {
    /// Reject a configuration the circuit cannot run with, naming the key.
    pub fn validate(&self) -> Result<()> {
        if self.kenyon_cells == 0 {
            bail!("[self_improvement.mushroom] kenyon_cells must be > 0");
        }
        if self.inputs_per_cell == 0 {
            bail!("[self_improvement.mushroom] inputs_per_cell must be > 0");
        }
        if !(self.active_fraction > 0.0 && self.active_fraction <= 1.0) {
            bail!("[self_improvement.mushroom] active_fraction must be in (0, 1]");
        }
        if self.novelty_half_life_ms == 0 {
            bail!("[self_improvement.mushroom] novelty_half_life_ms must be > 0");
        }
        if !(self.novel_threshold >= 0.0 && self.novel_threshold <= 1.0) {
            bail!("[self_improvement.mushroom] novel_threshold must be in [0, 1]");
        }
        if !(self.learning_rate > 0.0 && self.learning_rate <= 1.0) {
            bail!("[self_improvement.mushroom] learning_rate must be in (0, 1]");
        }
        if !(self.prior_min_coverage > 0.0 && self.prior_min_coverage <= 1.0) {
            bail!("[self_improvement.mushroom] prior_min_coverage must be in (0, 1]");
        }
        Ok(())
    }

    /// Kenyon cells kept after winner-take-all: at least one.
    fn active_cells(&self) -> usize {
        ((self.kenyon_cells as f32 * self.active_fraction).round() as usize)
            .clamp(1, self.kenyon_cells)
    }
}

/// splitmix64 — the same dependency-free PRNG as `obc_safety::redteam`, so a
/// seed is reproducible across crates and machines.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The sparse tag of one input: the indices of its active Kenyon cells,
/// ascending. Two tags of similar inputs share most indices; of unrelated
/// inputs, about `active_fraction` of them by chance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag(Vec<u32>);

impl Tag {
    /// Active Kenyon cell indices, ascending.
    pub fn active(&self) -> &[u32] {
        &self.0
    }
    /// Fraction of this tag's cells also active in `other`, in `[0, 1]`.
    pub fn overlap(&self, other: &Tag) -> f32 {
        if self.0.is_empty() {
            return 0.0;
        }
        let mut shared = 0usize;
        let (mut i, mut j) = (0, 0);
        while i < self.0.len() && j < other.0.len() {
            match self.0[i].cmp(&other.0[j]) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Equal => {
                    shared += 1;
                    i += 1;
                    j += 1;
                }
            }
        }
        shared as f32 / self.0.len() as f32
    }
}

/// Which way a compartment is pushed when a tag's episode ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Valence {
    /// The run reached its objective.
    Rewarded,
    /// The run did not.
    Punished,
}

/// What the body says about an objective before the reasoner sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Assessment {
    /// `1.0` = nothing like it has been seen; `0.0` = seen exactly, just now.
    pub novelty: f32,
    /// `novelty >= novel_threshold` — and only once the body is past warm-up.
    pub novel: bool,
    /// Share of reinforcement on this tag's cells that was rewarded, when any
    /// compartment has evidence for it. `None` = no evidence, not "50%".
    pub success_prior: Option<f32>,
}

/// The fixed sparse binary projection PN → KC, drawn once per (seed, dim).
struct Projection {
    dim: usize,
    per_cell: usize,
    /// `kenyon_cells × per_cell` input indices, row-major.
    inputs: Vec<u32>,
}

impl Projection {
    fn draw(seed: u64, dim: usize, kenyon_cells: usize, inputs_per_cell: usize) -> Self {
        let per_cell = inputs_per_cell.min(dim);
        let mut state = seed ^ 0xA5A5_5A5A_DEAD_BEEF;
        let mut inputs = Vec::with_capacity(kenyon_cells * per_cell);
        let mut chosen: Vec<u32> = Vec::with_capacity(per_cell);
        for _ in 0..kenyon_cells {
            chosen.clear();
            // Distinct inputs per cell; per_cell ≤ dim so this terminates.
            while chosen.len() < per_cell {
                let j = (splitmix64(&mut state) % dim as u64) as u32;
                if !chosen.contains(&j) {
                    chosen.push(j);
                }
            }
            inputs.extend_from_slice(&chosen);
        }
        Self {
            dim,
            per_cell,
            inputs,
        }
    }

    /// FlyHash step 1–3: centre the input, sum each cell's inputs, keep the
    /// top `k` (ties → lower index), report them ascending.
    fn tag(&self, x: &[f32], k: usize) -> Tag {
        let mean = x.iter().sum::<f32>() / x.len() as f32;
        let cells = self.inputs.len() / self.per_cell.max(1);
        let mut act: Vec<(f32, u32)> = (0..cells)
            .map(|c| {
                let row = &self.inputs[c * self.per_cell..(c + 1) * self.per_cell];
                let a = row.iter().map(|&j| x[j as usize] - mean).sum::<f32>();
                (if a.is_nan() { f32::MIN } else { a }, c as u32)
            })
            .collect();
        act.sort_by(|p, q| {
            q.0.partial_cmp(&p.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(p.1.cmp(&q.1))
        });
        let mut ids: Vec<u32> = act.into_iter().take(k).map(|(_, c)| c).collect();
        ids.sort_unstable();
        Tag(ids)
    }
}

/// The fly Bloom filter: one output weight per Kenyon cell, `1.0` when never
/// seen, driven to `0.0` by exposure, recovering toward `1.0` with a half-life.
struct NoveltyFilter {
    weight: Vec<f32>,
    touched_ms: Vec<u64>,
    half_life_ms: u64,
}

impl NoveltyFilter {
    fn new(cells: usize, half_life_ms: u64) -> Self {
        Self {
            weight: vec![1.0; cells],
            touched_ms: vec![0; cells],
            half_life_ms,
        }
    }
    fn current(&self, i: usize, now_ms: u64) -> f32 {
        let w = self.weight[i];
        if w >= 1.0 {
            return 1.0;
        }
        let dt = now_ms.saturating_sub(self.touched_ms[i]) as f64;
        let remaining = 0.5f64.powf(dt / self.half_life_ms as f64);
        (1.0 - (1.0 - w as f64) * remaining) as f32
    }
    fn novelty(&self, tag: &Tag, now_ms: u64) -> f32 {
        if tag.0.is_empty() {
            return 1.0;
        }
        tag.0
            .iter()
            .map(|&i| self.current(i as usize, now_ms))
            .sum::<f32>()
            / tag.0.len() as f32
    }
    fn observe(&mut self, tag: &Tag, now_ms: u64) {
        for &i in &tag.0 {
            self.weight[i as usize] = 0.0;
            self.touched_ms[i as usize] = now_ms;
        }
    }
}

/// FlyModel compartments: one weight vector per valence; only the active
/// cells of the reinforced valence move, everything else is frozen.
struct Compartments {
    rewarded: Vec<f32>,
    punished: Vec<f32>,
    learning_rate: f32,
}

impl Compartments {
    fn new(cells: usize, learning_rate: f32) -> Self {
        Self {
            rewarded: vec![0.0; cells],
            punished: vec![0.0; cells],
            learning_rate,
        }
    }
    fn reinforce(&mut self, tag: &Tag, valence: Valence) {
        let w = match valence {
            Valence::Rewarded => &mut self.rewarded,
            Valence::Punished => &mut self.punished,
        };
        for &i in &tag.0 {
            let cur = w[i as usize];
            // Saturating Hebbian step: bounded in [0, 1], repeated evidence
            // deepens the trace, never overflows it.
            w[i as usize] = cur + self.learning_rate * (1.0 - cur);
        }
    }
    fn success_prior(&self, tag: &Tag, min_coverage: f32) -> Option<f32> {
        if tag.0.is_empty() {
            return None;
        }
        let (mut r, mut p, mut covered) = (0.0f32, 0.0f32, 0usize);
        for &i in &tag.0 {
            let (ri, pi) = (self.rewarded[i as usize], self.punished[i as usize]);
            if ri + pi > 0.0 {
                covered += 1;
            }
            r += ri;
            p += pi;
        }
        let coverage = covered as f32 / tag.0.len() as f32;
        (coverage >= min_coverage && r + p > 0.0).then(|| r / (r + p))
    }
}

/// The circuit. Holds the projection (drawn lazily at the first embedding, so
/// the embedding dimension is learned from data, not configured twice), the
/// centre (fixed from the warm-up episodes), the novelty filter and the
/// valence compartments.
///
/// Episodes enter through [`MushroomBody::experience`] in time order, and
/// the body's state is a pure function of that sequence and the config —
/// which is why it needs no persistence: the store replays it on open.
pub struct MushroomBody {
    cfg: MushroomConfig,
    k: usize,
    projection: Option<Projection>,
    /// Mean of the warm-up episodes, subtracted from every embedding before
    /// hashing. `None` until `warmup_episodes` have been experienced.
    centre: Option<Vec<f32>>,
    /// Warm-up episodes held back until the centre is fixed, then tagged
    /// with it in their original order.
    pending: Vec<(Vec<f32>, u64, Option<Valence>)>,
    novelty: NoveltyFilter,
    compartments: Compartments,
    seen: usize,
}

impl MushroomBody {
    /// Build an empty body from a validated config.
    pub fn new(cfg: MushroomConfig) -> Result<Self> {
        cfg.validate()?;
        let k = cfg.active_cells();
        Ok(Self {
            novelty: NoveltyFilter::new(cfg.kenyon_cells, cfg.novelty_half_life_ms),
            compartments: Compartments::new(cfg.kenyon_cells, cfg.learning_rate),
            projection: None,
            centre: None,
            pending: Vec::new(),
            seen: 0,
            k,
            cfg,
        })
    }

    /// The config this body was built from.
    pub fn config(&self) -> &MushroomConfig {
        &self.cfg
    }

    /// Episodes experienced so far (replayed + recorded), warm-up included.
    pub fn seen(&self) -> usize {
        self.seen
    }

    /// Whether the warm-up is over: the centre is fixed and novelty is
    /// reported.
    pub fn warm(&self) -> bool {
        self.centre.is_some()
    }

    /// Fix the input dimension at first use (drawing the projection) and
    /// reject a later vector of another size — an error, not a guess. With
    /// no warm-up configured there is nothing to centre on, so the centre is
    /// the zero vector from the start.
    fn admit(&mut self, x: &[f32]) -> Result<()> {
        if x.is_empty() {
            bail!("mushroom body: cannot tag an empty embedding");
        }
        let (seed, cells, per_cell) = (
            self.cfg.seed,
            self.cfg.kenyon_cells,
            self.cfg.inputs_per_cell,
        );
        let proj = self
            .projection
            .get_or_insert_with(|| Projection::draw(seed, x.len(), cells, per_cell));
        if proj.dim != x.len() {
            bail!(
                "mushroom body: embedding dimension changed from {} to {} — \
                 the projection is fixed at first use; rebuild the body",
                proj.dim,
                x.len()
            );
        }
        if self.centre.is_none() && self.cfg.warmup_episodes == 0 {
            self.centre = Some(vec![0.0; x.len()]);
        }
        Ok(())
    }

    /// Hash an embedding to its sparse tag: subtract the centre, then the
    /// FlyHash steps. An error while the body is still warming up — there is
    /// no centre to tag against yet, and a tag made without one would not
    /// match the tags of anything the body holds.
    pub fn tag(&mut self, x: &[f32]) -> Result<Tag> {
        self.admit(x)?;
        let Some(centre) = &self.centre else {
            bail!(
                "mushroom body: still warming up ({} of {} episodes) — no centre to tag against",
                self.seen,
                self.cfg.warmup_episodes
            );
        };
        let centred: Vec<f32> = x.iter().zip(centre).map(|(v, c)| v - c).collect();
        let proj = self.projection.as_ref().expect("admitted");
        Ok(proj.tag(&centred, self.k))
    }

    /// Novelty of a tag at `now_ms`, in `[0, 1]`. Does not record the tag.
    pub fn novelty(&self, tag: &Tag, now_ms: u64) -> f32 {
        self.novelty.novelty(tag, now_ms)
    }

    /// Rewarded share of the evidence on this tag's cells, once at least
    /// `prior_min_coverage` of them carry any.
    pub fn success_prior(&self, tag: &Tag) -> Option<f32> {
        self.compartments
            .success_prior(tag, self.cfg.prior_min_coverage)
    }

    /// The one way an episode enters the body: observed at `now_ms` (its
    /// tag's cells go familiar) and, when it had a valence, reinforced in
    /// that compartment. During warm-up the episode is held; when the
    /// `warmup_episodes`-th arrives, their mean becomes the centre and all of
    /// them are tagged with it, in order. Callers feed episodes oldest first.
    pub fn experience(&mut self, x: &[f32], now_ms: u64, valence: Option<Valence>) -> Result<()> {
        self.admit(x)?;
        self.seen += 1;
        if self.centre.is_some() {
            let tag = self.tag(x)?;
            self.take_in(&tag, now_ms, valence);
            return Ok(());
        }
        self.pending.push((x.to_vec(), now_ms, valence));
        if self.pending.len() >= self.cfg.warmup_episodes {
            let n = self.pending.len() as f64;
            let dim = x.len();
            let mut sum = vec![0f64; dim];
            for (v, _, _) in &self.pending {
                for (s, &vi) in sum.iter_mut().zip(v) {
                    *s += vi as f64;
                }
            }
            self.centre = Some(sum.iter().map(|s| (s / n) as f32).collect());
            let held = std::mem::take(&mut self.pending);
            for (v, ts, val) in held {
                let tag = self.tag(&v)?;
                self.take_in(&tag, ts, val);
            }
        }
        Ok(())
    }

    fn take_in(&mut self, tag: &Tag, now_ms: u64, valence: Option<Valence>) {
        self.novelty.observe(tag, now_ms);
        if let Some(v) = valence {
            self.compartments.reinforce(tag, v);
        }
    }

    /// Everything the agent asks before a turn, from one embedding. Reads
    /// only — the episode is experienced when it is recorded, not when
    /// planned. While the body is warming up it has no opinion: novelty
    /// `1.0`, not reported as novel, no prior.
    pub fn assess(&mut self, x: &[f32], now_ms: u64) -> Result<Assessment> {
        self.admit(x)?;
        if !self.warm() {
            return Ok(Assessment {
                novelty: 1.0,
                novel: false,
                success_prior: None,
            });
        }
        let tag = self.tag(x)?;
        let novelty = self.novelty(&tag, now_ms);
        Ok(Assessment {
            novelty,
            novel: novelty >= self.cfg.novel_threshold,
            success_prior: self.success_prior(&tag),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_MS: u64 = 24 * 60 * 60 * 1000;

    fn cfg() -> MushroomConfig {
        MushroomConfig {
            enabled: true,
            warmup_episodes: 0,
            ..MushroomConfig::default()
        }
    }

    /// Uniform in [-1, 1) from splitmix — the test's own generator, so the
    /// synthetic data is as reproducible as the body.
    struct Gen(u64);
    impl Gen {
        fn unit(&mut self) -> f32 {
            (splitmix64(&mut self.0) >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
        }
        fn vec(&mut self, dim: usize) -> Vec<f32> {
            (0..dim).map(|_| self.unit()).collect()
        }
        fn below(&mut self, n: usize) -> usize {
            (splitmix64(&mut self.0) % n as u64) as usize
        }
    }

    /// A "family" is a centroid; a sample is centroid + noise.
    fn sample(g: &mut Gen, centroid: &[f32], noise: f32) -> Vec<f32> {
        centroid.iter().map(|c| c + noise * g.unit()).collect()
    }

    #[test]
    fn same_seed_same_input_same_tag_and_a_different_seed_differs() {
        let x: Vec<f32> = (0..64).map(|i| (i as f32 * 0.37).sin()).collect();
        let mut a = MushroomBody::new(cfg()).unwrap();
        let mut b = MushroomBody::new(cfg()).unwrap();
        assert_eq!(a.tag(&x).unwrap(), b.tag(&x).unwrap());
        let mut c = MushroomBody::new(MushroomConfig { seed: 99, ..cfg() }).unwrap();
        assert_ne!(a.tag(&x).unwrap(), c.tag(&x).unwrap());
    }

    #[test]
    fn a_tag_has_exactly_the_configured_number_of_active_cells_ascending() {
        let mut mb = MushroomBody::new(cfg()).unwrap();
        let x: Vec<f32> = (0..384).map(|i| ((i * 7919) % 97) as f32 / 97.0).collect();
        let t = mb.tag(&x).unwrap();
        assert_eq!(t.active().len(), 400, "2% of 20000");
        assert!(t.active().windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn a_changed_embedding_dimension_is_an_error_not_a_guess() {
        let mut mb = MushroomBody::new(cfg()).unwrap();
        mb.tag(&[0.1; 8]).unwrap();
        let err = mb.tag(&[0.1; 9]).unwrap_err().to_string();
        assert!(err.contains("8 to 9"), "{err}");
        assert!(mb.tag(&[]).is_err());
    }

    #[test]
    fn related_inputs_share_most_cells_and_unrelated_ones_share_few() {
        let mut g = Gen(1);
        let mut mb = MushroomBody::new(cfg()).unwrap();
        let a = g.vec(32);
        let b = g.vec(32);
        let ta = mb.tag(&a).unwrap();
        let ta2 = mb.tag(&sample(&mut g, &a, 0.2)).unwrap();
        let tb = mb.tag(&b).unwrap();
        assert!(ta.overlap(&ta2) > 0.6, "same family: {}", ta.overlap(&ta2));
        assert!(
            ta.overlap(&tb) < 0.25,
            "different family: {}",
            ta.overlap(&tb)
        );
    }

    #[test]
    fn novelty_is_one_before_anything_is_seen_and_zero_for_an_exact_repeat() {
        let mut g = Gen(2);
        let mut mb = MushroomBody::new(cfg()).unwrap();
        let x = g.vec(32);
        let t = mb.tag(&x).unwrap();
        assert_eq!(mb.novelty(&t, 1_000), 1.0);
        mb.experience(&x, 1_000, None).unwrap();
        assert_eq!(mb.novelty(&t, 1_000), 0.0);
    }

    #[test]
    fn novelty_is_distance_sensitive_partial_overlap_is_partial_familiarity() {
        let mut g = Gen(3);
        let mut mb = MushroomBody::new(cfg()).unwrap();
        let a = g.vec(32);
        mb.experience(&a, 1_000, None).unwrap();
        let near = mb.tag(&sample(&mut g, &a, 0.2)).unwrap();
        let far = mb.tag(&g.vec(32)).unwrap();
        let n_near = mb.novelty(&near, 1_000);
        let n_far = mb.novelty(&far, 1_000);
        assert!(n_near < 0.5, "near sample novelty {n_near}");
        assert!(n_far > 0.75, "far sample novelty {n_far}");
        assert!(n_near < n_far);
    }

    #[test]
    fn novelty_recovers_with_the_configured_half_life() {
        let mut g = Gen(4);
        let mut mb = MushroomBody::new(MushroomConfig {
            novelty_half_life_ms: DAY_MS,
            ..cfg()
        })
        .unwrap();
        let x = g.vec(32);
        let t = mb.tag(&x).unwrap();
        mb.experience(&x, 0, None).unwrap();
        let one = mb.novelty(&t, DAY_MS);
        let three = mb.novelty(&t, 3 * DAY_MS);
        assert!((one - 0.5).abs() < 1e-4, "one half-life: {one}");
        assert!((three - 0.875).abs() < 1e-4, "three half-lives: {three}");
        // Seeing it again resets the clock.
        mb.experience(&x, 3 * DAY_MS, None).unwrap();
        assert_eq!(mb.novelty(&t, 3 * DAY_MS), 0.0);
    }

    #[test]
    fn success_prior_is_none_without_evidence_and_tracks_reinforcement() {
        let mut g = Gen(5);
        let mut mb = MushroomBody::new(cfg()).unwrap();
        let x = g.vec(32);
        let t = mb.tag(&x).unwrap();
        assert_eq!(mb.success_prior(&t), None, "no evidence is not 50%");
        mb.experience(&x, 0, Some(Valence::Rewarded)).unwrap();
        assert_eq!(mb.success_prior(&t), Some(1.0));
        mb.experience(&x, 0, Some(Valence::Punished)).unwrap();
        let p = mb.success_prior(&t).unwrap();
        assert!((p - 0.5).abs() < 1e-5, "{p}");
        for _ in 0..10 {
            mb.experience(&x, 0, Some(Valence::Punished)).unwrap();
        }
        assert!(mb.success_prior(&t).unwrap() < 0.25);
    }

    #[test]
    fn chance_overlap_with_a_seen_tag_does_not_manufacture_a_prior() {
        let mut g = Gen(8);
        let mut mb = MushroomBody::new(cfg()).unwrap();
        let x = g.vec(32);
        let seen = mb.tag(&x).unwrap();
        for _ in 0..5 {
            mb.experience(&x, 0, Some(Valence::Rewarded)).unwrap();
        }
        // An unrelated tag shares a few cells by chance (see the overlap
        // test) — under the coverage floor, that is not evidence.
        let unrelated = mb.tag(&g.vec(32)).unwrap();
        assert!(unrelated.overlap(&seen) < 0.25);
        assert_eq!(mb.success_prior(&unrelated), None);
    }

    #[test]
    fn a_near_neighbour_clears_the_coverage_floor_and_inherits_the_prior() {
        let mut g = Gen(9);
        let mut mb = MushroomBody::new(cfg()).unwrap();
        let a = g.vec(32);
        let seen = mb.tag(&a).unwrap();
        mb.experience(&a, 0, Some(Valence::Rewarded)).unwrap();
        let near = mb.tag(&sample(&mut g, &a, 0.2)).unwrap();
        assert!(near.overlap(&seen) > 0.6);
        assert_eq!(mb.success_prior(&near), Some(1.0));
    }

    #[test]
    fn assess_reports_novel_only_after_warm_up() {
        let mut g = Gen(6);
        let mut mb = MushroomBody::new(MushroomConfig {
            warmup_episodes: 2,
            ..cfg()
        })
        .unwrap();
        let x = g.vec(32);
        let a0 = mb.assess(&x, 0).unwrap();
        assert_eq!(a0.novelty, 1.0);
        assert!(
            !a0.novel,
            "cold body: everything is novel, so nothing is reported"
        );
        assert!(!mb.warm());
        for _ in 0..2 {
            mb.experience(&g.vec(32), 0, None).unwrap();
        }
        assert!(mb.warm());
        let a1 = mb.assess(&x, 0).unwrap();
        assert!(a1.novel, "warm body, unseen input: {}", a1.novelty);
        mb.experience(&x, 0, None).unwrap();
        assert!(!mb.assess(&x, 0).unwrap().novel);
    }

    #[test]
    fn warm_up_episodes_are_held_then_taken_in_with_the_fixed_centre() {
        let mut g = Gen(10);
        let mut mb = MushroomBody::new(MushroomConfig {
            warmup_episodes: 3,
            ..cfg()
        })
        .unwrap();
        let held: Vec<Vec<f32>> = (0..3).map(|_| g.vec(32)).collect();
        for (i, v) in held.iter().enumerate().take(2) {
            mb.experience(v, 100 + i as u64, Some(Valence::Rewarded))
                .unwrap();
            // No centre yet: no tag, no opinion.
            let err = mb.tag(v).unwrap_err().to_string();
            assert!(err.contains("warming up"), "{err}");
            assert_eq!(mb.assess(v, 200).unwrap().success_prior, None);
        }
        assert_eq!(mb.seen(), 2);
        mb.experience(&held[2], 102, Some(Valence::Punished))
            .unwrap();
        assert!(mb.warm());
        assert_eq!(mb.seen(), 3);
        // Every held episode is now in: familiar at its own time, with its
        // own valence.
        for v in held.iter().take(2) {
            let a = mb.assess(v, 102).unwrap();
            assert!(a.novelty < 1e-6, "{}", a.novelty);
            assert_eq!(a.success_prior, Some(1.0));
        }
        assert_eq!(mb.assess(&held[2], 102).unwrap().success_prior, Some(0.0));
    }

    #[test]
    fn the_same_sequence_gives_the_same_body_however_it_was_split() {
        // Attach-time replay and live recording must agree: the body is a
        // function of the ordered episode sequence, not of when it was built.
        let mut g = Gen(11);
        let seq: Vec<(Vec<f32>, u64, Option<Valence>)> = (0..12)
            .map(|i| {
                (
                    g.vec(32),
                    1_000 + i,
                    match i % 3 {
                        0 => Some(Valence::Rewarded),
                        1 => Some(Valence::Punished),
                        _ => None,
                    },
                )
            })
            .collect();
        let c = MushroomConfig {
            warmup_episodes: 5,
            ..cfg()
        };
        let mut a = MushroomBody::new(c.clone()).unwrap();
        let mut b = MushroomBody::new(c).unwrap();
        for (x, ts, v) in &seq {
            a.experience(x, *ts, *v).unwrap();
        }
        for (x, ts, v) in &seq {
            b.experience(x, *ts, *v).unwrap();
            // Interleaved reads must not change what the body becomes.
            let _ = b.assess(x, *ts);
        }
        let probe = g.vec(32);
        for x in seq.iter().map(|s| &s.0).chain(std::iter::once(&probe)) {
            assert_eq!(a.assess(x, 2_000).unwrap(), b.assess(x, 2_000).unwrap());
        }
    }

    #[test]
    fn a_shared_component_swamps_the_raw_tags_and_the_centre_removes_it() {
        // The failure the real episodes showed: sentence embeddings carry a
        // large common vector. Unrelated inputs that share it get nearly the
        // same raw tag; subtracting the warm-up mean gives them back their
        // own. Per-vector scalar centring cannot do this — the common part
        // is a direction, not an offset.
        let mut g = Gen(12);
        let common: Vec<f32> = g.vec(32).iter().map(|v| v * 5.0).collect();
        let with_common =
            |v: &[f32]| -> Vec<f32> { v.iter().zip(&common).map(|(a, c)| a + c).collect() };
        let a = with_common(&g.vec(32));
        let b = with_common(&g.vec(32));

        let mut raw = MushroomBody::new(cfg()).unwrap();
        let raw_overlap = raw.tag(&a).unwrap().overlap(&raw.tag(&b).unwrap());

        let mut centred = MushroomBody::new(MushroomConfig {
            warmup_episodes: 10,
            ..cfg()
        })
        .unwrap();
        for i in 0..10 {
            centred
                .experience(&with_common(&g.vec(32)), i, None)
                .unwrap();
        }
        let centred_overlap = centred.tag(&a).unwrap().overlap(&centred.tag(&b).unwrap());
        println!("unrelated inputs sharing a common vector: raw overlap {raw_overlap:.3}, centred {centred_overlap:.3}");
        assert!(raw_overlap > 0.5, "raw tags should collide: {raw_overlap}");
        assert!(
            centred_overlap < 0.25,
            "centred tags should not: {centred_overlap}"
        );
    }

    #[test]
    fn config_rejects_what_the_circuit_cannot_run_with() {
        for (name, c) in [
            (
                "kenyon_cells",
                MushroomConfig {
                    kenyon_cells: 0,
                    ..cfg()
                },
            ),
            (
                "active_fraction",
                MushroomConfig {
                    active_fraction: 1.5,
                    ..cfg()
                },
            ),
            (
                "learning_rate",
                MushroomConfig {
                    learning_rate: 0.0,
                    ..cfg()
                },
            ),
            (
                "novelty_half_life_ms",
                MushroomConfig {
                    novelty_half_life_ms: 0,
                    ..cfg()
                },
            ),
        ] {
            let err = match MushroomBody::new(c) {
                Ok(_) => panic!("{name}: invalid config accepted"),
                Err(e) => e.to_string(),
            };
            assert!(err.contains(name), "{err}");
        }
    }

    // ── The measurement ─────────────────────────────────────────────────
    //
    // FlyModel's claim: sparse tags + reinforcing only the active cells of
    // the current valence retain what was learned about early inputs after a
    // long run of later, unrelated ones. The comparison is an online
    // perceptron on the same dense vectors, trained on the same chronological
    // stream. Both are scored on the *old* families after the stream ends.
    //
    // Thresholds were fixed before the first run and are asserted; the
    // perceptron's number is printed so a regression in either direction is
    // visible in `cargo test -- --nocapture`.

    struct Perceptron {
        w: Vec<f32>,
        b: f32,
    }
    impl Perceptron {
        fn predict(&self, x: &[f32]) -> bool {
            self.w.iter().zip(x).map(|(a, b)| a * b).sum::<f32>() + self.b >= 0.0
        }
        fn train(&mut self, x: &[f32], rewarded: bool) {
            if self.predict(x) != rewarded {
                let y = if rewarded { 1.0 } else { -1.0 };
                for (w, xi) in self.w.iter_mut().zip(x) {
                    *w += y * xi;
                }
                self.b += y;
            }
        }
    }

    /// Returns (mushroom, perceptron) accuracy on the old families.
    fn retention_after_drift(seed: u64) -> (f32, f32) {
        const DIM: usize = 32;
        const FAMILIES: usize = 8;
        const OLD: usize = 4;
        const PER_FAMILY: usize = 40;
        const NOISE: f32 = 0.25;
        let mut g = Gen(seed);
        let centroids: Vec<Vec<f32>> = (0..FAMILIES).map(|_| g.vec(DIM)).collect();
        // Label by family, alternating, so neither valence is a majority.
        let rewarded = |f: usize| f.is_multiple_of(2);

        // Fly-scale circuit: the claim is about the mechanism, not the width,
        // and 20 seeds at the deployed 20,000 cells take over a minute in
        // debug builds.
        let mut mb = MushroomBody::new(MushroomConfig {
            kenyon_cells: 2000,
            active_fraction: 0.05,
            ..cfg()
        })
        .unwrap();
        let mut pc = Perceptron {
            w: vec![0.0; DIM],
            b: 0.0,
        };
        // Chronological stream: the old families first, then only new ones.
        for block in [0..OLD, OLD..FAMILIES] {
            let fams: Vec<usize> = block.collect();
            for _ in 0..PER_FAMILY * fams.len() {
                let f = fams[g.below(fams.len())];
                let x = sample(&mut g, &centroids[f], NOISE);
                mb.experience(
                    &x,
                    0,
                    Some(if rewarded(f) {
                        Valence::Rewarded
                    } else {
                        Valence::Punished
                    }),
                )
                .unwrap();
                pc.train(&x, rewarded(f));
            }
        }
        // Score on fresh samples of the old families only.
        let mut mb_ok = 0usize;
        let mut pc_ok = 0usize;
        let mut n = 0usize;
        for (f, centroid) in centroids.iter().enumerate().take(OLD) {
            for _ in 0..25 {
                let x = sample(&mut g, centroid, NOISE);
                let t = mb.tag(&x).unwrap();
                let mb_pred = mb.success_prior(&t).map(|p| p >= 0.5).unwrap_or(false);
                mb_ok += (mb_pred == rewarded(f)) as usize;
                pc_ok += (pc.predict(&x) == rewarded(f)) as usize;
                n += 1;
            }
        }
        (mb_ok as f32 / n as f32, pc_ok as f32 / n as f32)
    }

    #[test]
    fn old_families_are_retained_after_a_block_of_unrelated_ones() {
        const SEEDS: u64 = 20;
        let mut mb_sum = 0.0;
        let mut pc_sum = 0.0;
        let mut mb_min: f32 = 1.0;
        for seed in 0..SEEDS {
            let (m, p) = retention_after_drift(seed);
            mb_sum += m;
            pc_sum += p;
            mb_min = mb_min.min(m);
        }
        let (mb_mean, pc_mean) = (mb_sum / SEEDS as f32, pc_sum / SEEDS as f32);
        println!(
            "retention on old families over {SEEDS} seeds — mushroom mean {mb_mean:.3} \
             (min {mb_min:.3}), perceptron mean {pc_mean:.3}"
        );
        assert!(mb_mean >= 0.9, "mushroom mean retention {mb_mean}");
        assert!(mb_min >= 0.8, "mushroom worst-seed retention {mb_min}");
        assert!(
            mb_mean >= pc_mean,
            "the FlyModel claim did not hold on this stream: mushroom {mb_mean} < perceptron {pc_mean}"
        );
    }

    #[test]
    fn the_measurement_is_deterministic() {
        assert_eq!(retention_after_drift(7), retention_after_drift(7));
    }
}
