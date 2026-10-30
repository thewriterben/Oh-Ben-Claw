//! Per-node Ed25519 identities for the P2P mesh: a frame says *which* node
//! sent it, not only that the sender holds the mesh secret.
//!
//! [`spine_tag`](crate::spine_tag) derives every node's frame key from the one
//! `pairing_secret`, so on the P2P mesh — where every node holds that secret —
//! a valid tag proves membership and nothing more: any member can derive any
//! other member's key and sign as it. A served call is judged as `p2p:<peer>`
//! for trust, approval and audit, so that label has to be proven, not claimed.
//!
//! ## Why asymmetric
//!
//! The alternative was per-node symmetric keys issued by the brain at pairing.
//! It loses on three counts:
//!
//! - **The verifier can forge.** With a symmetric key, whoever checks a tag can
//!   make one. Node C holding A's key to check A's calls can sign as A to
//!   anyone else holding the same key, so it takes a key per *pair* to name a
//!   sender, and the brain becomes a key distribution centre for the mesh.
//! - **Issuing needs a secret channel.** Discovery is a cleartext UDP broadcast
//!   and MQTT is cleartext; handing out a key means a key exchange, which is an
//!   asymmetric primitive anyway.
//! - **The brain stops being optional.** Two edge nodes could no longer call
//!   each other without it.
//!
//! With Ed25519 each node keeps its own secret and publishes the public half in
//! its announcement. A peer pins that key the first time it admits the node
//! (pairing already proved mesh membership), and from then on accepts frames
//! claiming to be that node only if they verify under the pinned key. Nothing
//! secret travels. `ed25519-dalek` is already here for the action audit
//! ([`audit_sign`](crate::audit_sign)).
//!
//! ## What pinning proves, and what it does not
//!
//! - **Trust on first use, unless configured.** The first paired announcement
//!   for a node id fixes its key. A mesh member that announces a node id before
//!   the real node ever has can claim it. `[security] peer_keys` pins keys by
//!   hand, and those are never replaced by what the mesh says; a deployment
//!   that cares names its nodes there. Learned pins can be persisted
//!   ([`PeerKeys::with_store`]) so a restart does not reopen the race.
//! - **No revocation, no rotation.** A node with a new key is refused by every
//!   peer that pinned the old one until an operator removes the pin. That is
//!   the safe failure: the alternative is accepting a changed key, which is the
//!   attack.
//! - **Not the firmware's path.** The Heltec stations and MQTT nodes still use
//!   the HKDF-derived HMAC keys; on MQTT the brain is the only peer and holds
//!   every key by design. This is for nodes that call *each other*.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

/// Domain separator for frame signatures. A signature made for one purpose
/// must not verify for another, so each purpose prefixes its own label.
const FRAME_DOMAIN: &[u8] = b"obc-p2p-frame-v1";

/// Domain separator for announcement signatures.
const ANNOUNCE_DOMAIN: &[u8] = b"obc-p2p-announce-v1";

/// Which direction a signed frame travels. Covered by the signature, so a
/// result cannot be replayed as a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    Request,
    Result,
}

impl FrameKind {
    fn byte(self) -> u8 {
        match self {
            FrameKind::Request => 1,
            FrameKind::Result => 2,
        }
    }
}

/// The signed bytes of a frame:
/// `domain ‖ kind ‖ len(from) ‖ from ‖ len(to) ‖ to ‖ ctr ‖ payload`.
///
/// `to` is in it so a frame one node received cannot be replayed to another:
/// replay windows are per receiver, and without the recipient a call from A to
/// B would be fresh at C. Lengths are 32-bit big-endian, so two ids cannot
/// slide into each other.
fn frame_region(kind: FrameKind, from: &str, to: &str, ctr: u32, payload: &[u8]) -> Vec<u8> {
    let mut buf =
        Vec::with_capacity(FRAME_DOMAIN.len() + 13 + from.len() + to.len() + payload.len());
    buf.extend_from_slice(FRAME_DOMAIN);
    buf.push(kind.byte());
    for id in [from, to] {
        buf.extend_from_slice(&(id.len() as u32).to_be_bytes());
        buf.extend_from_slice(id.as_bytes());
    }
    buf.extend_from_slice(&ctr.to_be_bytes());
    buf.extend_from_slice(payload);
    buf
}

fn announce_region(body: &[u8]) -> Vec<u8> {
    [ANNOUNCE_DOMAIN, body].concat()
}

/// A public key from hex, or `None` if it is not one.
pub fn parse_public(public_hex: &str) -> Option<VerifyingKey> {
    let bytes: [u8; 32] = hex::decode(public_hex).ok()?.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

fn parse_signature(sig_hex: &str) -> Option<Signature> {
    let bytes: [u8; 64] = hex::decode(sig_hex).ok()?.try_into().ok()?;
    Some(Signature::from_bytes(&bytes))
}

/// Verify a frame signature. `false` for anything malformed.
///
/// `verify_strict`: it refuses the small-order keys and non-canonical
/// signatures that plain `verify` lets through, which matters here because the
/// key came off the network.
pub fn verify_frame(
    key: &VerifyingKey,
    kind: FrameKind,
    from: &str,
    to: &str,
    ctr: u32,
    payload: &[u8],
    sig_hex: &str,
) -> bool {
    let Some(sig) = parse_signature(sig_hex) else {
        return false;
    };
    key.verify_strict(&frame_region(kind, from, to, ctr, payload), &sig)
        .is_ok()
}

/// Verify an announcement's signature under the key it carries.
pub fn verify_announcement(public_hex: &str, body: &[u8], sig_hex: &str) -> bool {
    let (Some(key), Some(sig)) = (parse_public(public_hex), parse_signature(sig_hex)) else {
        return false;
    };
    key.verify_strict(&announce_region(body), &sig).is_ok()
}

/// This node's signing key.
#[derive(Clone)]
pub struct NodeIdentity {
    key: SigningKey,
}

impl std::fmt::Debug for NodeIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeIdentity")
            .field("public", &self.public_hex())
            .finish_non_exhaustive()
    }
}

impl NodeIdentity {
    /// A fresh key from the OS CSPRNG. Peers pin it, so a node that generates
    /// a new one at every start is refused after its first restart: use
    /// [`NodeIdentity::load_or_create`] for anything that runs for real.
    pub fn generate() -> Self {
        Self {
            key: SigningKey::generate(&mut rand::rngs::OsRng),
        }
    }

    /// From a 32-byte secret key, hex.
    pub fn from_hex(secret_hex: &str) -> anyhow::Result<Self> {
        let bytes: [u8; 32] = hex::decode(secret_hex.trim())?
            .try_into()
            .map_err(|_| anyhow::anyhow!("an ed25519 secret key is 32 bytes"))?;
        Ok(Self {
            key: SigningKey::from_bytes(&bytes),
        })
    }

    /// The secret key, hex. Store it where only this node can read it.
    pub fn secret_hex(&self) -> String {
        hex::encode(self.key.to_bytes())
    }

    /// The public key, hex: what peers pin.
    pub fn public_hex(&self) -> String {
        hex::encode(self.key.verifying_key().to_bytes())
    }

    /// The key in `path`, or a new one written there (owner-only on Unix).
    pub fn load_or_create(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(hex) => Self::from_hex(&hex)
                .map_err(|e| anyhow::anyhow!("{} is not a P2P identity key: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let identity = Self::generate();
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                write_atomic(path, identity.secret_hex().as_bytes())?;
                Ok(identity)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Sign a frame from this node to `to`.
    pub fn sign_frame(
        &self,
        kind: FrameKind,
        from: &str,
        to: &str,
        ctr: u32,
        payload: &[u8],
    ) -> String {
        hex::encode(
            self.key
                .sign(&frame_region(kind, from, to, ctr, payload))
                .to_bytes(),
        )
    }

    /// Sign an announcement body (the announcement without its signature).
    pub fn sign_announcement(&self, body: &[u8]) -> String {
        hex::encode(self.key.sign(&announce_region(body)).to_bytes())
    }
}

/// Write `bytes` to `path` so that a crash leaves either the old file or the
/// new one, never half of either: a sibling temp file (owner-only on Unix),
/// flushed to disk, renamed into place. A torn key or pin file would stop the
/// node starting, since both refuse to load what does not parse.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    // The rename itself is durable once the directory is.
    #[cfg(unix)]
    if let Some(dir) = path.parent().and_then(|d| std::fs::File::open(d).ok()) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// The epoch this start announces under: one more than the last start's, or
/// the clock's seconds if those are ahead. A node's announcement sequence is
/// `epoch << 32 | count`, and peers accept only a sequence higher than the
/// last one they took from it, so a node without a clock must still go up
/// across restarts. Written back before it is returned; if the file cannot be
/// written the clock alone is used.
pub fn next_boot_epoch(path: &Path) -> u32 {
    let clock = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX))
        .unwrap_or_default();
    let last = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    let epoch = last.map_or(clock, |last| clock.max(last.saturating_add(1)));
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| write_atomic(path, epoch.to_string().as_bytes()));
    if let Err(e) = written {
        tracing::warn!(path = %path.display(), error = %e, "Could not record the P2P announce epoch");
    }
    epoch
}

/// What a node keeps across restarts to stay the same peer on the mesh: its
/// identity key, the pins it learned, and the epoch its announcements count
/// from this start, each in its own file in one directory.
pub struct P2pState {
    pub identity: NodeIdentity,
    pub peer_keys: PeerKeys,
    pub announce_epoch: u32,
}

impl P2pState {
    /// The state kept in `dir`, created where missing, with `peer_keys` (the
    /// configured pins) on top of the pins learned there.
    pub fn load(dir: &Path, peer_keys: PeerKeys) -> anyhow::Result<Self> {
        Ok(Self {
            identity: NodeIdentity::load_or_create(&dir.join("p2p_identity.key"))?,
            peer_keys: peer_keys.with_store(dir.join("p2p_peer_keys.json"))?,
            announce_epoch: next_boot_epoch(&dir.join("p2p_announce_epoch")),
        })
    }
}

/// What pinning an announced key decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinVerdict {
    /// No key was pinned for this node; this one now is.
    Pinned,
    /// The key matches the one already pinned.
    Known,
    /// A different key is pinned for this node. The announcement is not it.
    Mismatch,
}

/// The public keys this node accepts for each peer.
#[derive(Debug, Default)]
pub struct PeerKeys {
    /// From `[security] peer_keys`. They win, and are never written out: the
    /// config is where they live, so removing one there unpins the node.
    configured: HashMap<String, VerifyingKey>,
    /// Pinned on first announcement, and kept in `store`.
    learned: Mutex<HashMap<String, VerifyingKey>>,
    /// Where learned pins are kept across restarts, if anywhere.
    store: Option<PathBuf>,
    /// Held across a write of `store`, and the snapshot taken under it, so the
    /// last write to land is the one with every pin in it.
    writing: Mutex<()>,
}

impl PeerKeys {
    /// No pins, nothing persisted.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pins from `[security] peer_keys` (node id → public key, hex). A
    /// malformed key is an error rather than a skipped entry: a pin the
    /// operator wrote and the node ignored would leave that id open to the
    /// first claimant.
    pub fn configured(keys: &BTreeMap<String, String>) -> anyhow::Result<Self> {
        let mut configured = HashMap::new();
        for (node, hex) in keys {
            let key = parse_public(hex).ok_or_else(|| {
                anyhow::anyhow!("[security] peer_keys.{node} is not an ed25519 public key (64 hex)")
            })?;
            configured.insert(node.clone(), key);
        }
        Ok(Self {
            configured,
            ..Self::default()
        })
    }

    /// Keep learned pins in `path` (JSON, node id → public key hex), and load
    /// the ones already there. A configured pin wins over the file, and the
    /// file's entry for that node is dropped at the next write, so it cannot
    /// come back if the configured one is removed. A file that does not parse
    /// is an error: starting with no pins would hand every id back to whoever
    /// announces it first.
    pub fn with_store(self, path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let path = path.into();
        let stored: BTreeMap<String, String> = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| anyhow::anyhow!("{} does not parse: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e.into()),
        };
        {
            let mut learned = self.learned.lock().unwrap_or_else(|p| p.into_inner());
            for (node, hex) in stored {
                let key = parse_public(&hex).ok_or_else(|| {
                    anyhow::anyhow!("{}: the key for {node} is malformed", path.display())
                })?;
                if !self.configured.contains_key(&node) {
                    learned.entry(node).or_insert(key);
                }
            }
        }
        Ok(Self {
            store: Some(path),
            ..self
        })
    }

    /// The key pinned for `node_id`.
    pub fn key_for(&self, node_id: &str) -> Option<VerifyingKey> {
        self.configured.get(node_id).copied().or_else(|| {
            self.learned
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(node_id)
                .copied()
        })
    }

    /// Pin `key` for `node_id` unless another key already is. Call only for an
    /// announcement from a paired node whose signature verifies under `key`:
    /// pinning is how mesh membership becomes an identity.
    ///
    /// A new pin is written to the store, which is blocking file I/O: from
    /// async code, call this on a blocking thread.
    pub fn pin(&self, node_id: &str, key: VerifyingKey) -> PinVerdict {
        let held = self
            .configured
            .get(node_id)
            .copied()
            .map(|pinned| pinned == key);
        if let Some(same) = held {
            return if same {
                PinVerdict::Known
            } else {
                PinVerdict::Mismatch
            };
        }
        {
            let mut learned = self.learned.lock().unwrap_or_else(|p| p.into_inner());
            match learned.get(node_id) {
                Some(pinned) if *pinned == key => return PinVerdict::Known,
                Some(_) => return PinVerdict::Mismatch,
                None => {
                    learned.insert(node_id.to_string(), key);
                }
            }
        }
        self.persist();
        PinVerdict::Pinned
    }

    /// Write the learned pins to the store. Held in memory either way; a
    /// failed write only means a pin is learned again after a restart.
    fn persist(&self) {
        let Some(path) = &self.store else { return };
        let _writing = self.writing.lock().unwrap_or_else(|p| p.into_inner());
        let snapshot: BTreeMap<String, String> = self
            .learned
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|(n, k)| (n.clone(), hex::encode(k.to_bytes())))
            .collect();
        if let Err(e) = serde_json::to_vec_pretty(&snapshot)
            .map_err(std::io::Error::other)
            .and_then(|bytes| write_atomic(path, &bytes))
        {
            tracing::warn!(path = %path.display(), error = %e, "Could not persist a P2P peer key pin");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "obc-node-a";
    const B: &str = "obc-node-b";
    const C: &str = "obc-node-c";
    const BODY: &[u8] = br#"{"call_id":"1","tool_name":"gpio_write","args":{}}"#;

    fn key(identity: &NodeIdentity) -> VerifyingKey {
        parse_public(&identity.public_hex()).unwrap()
    }

    #[test]
    fn a_frame_verifies_under_its_signers_key() {
        let a = NodeIdentity::generate();
        let sig = a.sign_frame(FrameKind::Request, A, B, 7, BODY);
        assert!(verify_frame(
            &key(&a),
            FrameKind::Request,
            A,
            B,
            7,
            BODY,
            &sig
        ));
    }

    /// The point of this module: A holds the mesh secret and its own key, and
    /// nothing it can sign verifies as B — not under B's key, and not by
    /// claiming B's id over its own key.
    #[test]
    fn node_a_cannot_sign_a_frame_that_verifies_as_node_b() {
        let a = NodeIdentity::generate();
        let b = NodeIdentity::generate();
        let forged = a.sign_frame(FrameKind::Request, B, C, 7, BODY);
        assert!(!verify_frame(
            &key(&b),
            FrameKind::Request,
            B,
            C,
            7,
            BODY,
            &forged
        ));
    }

    #[test]
    fn every_bound_field_changes_what_verifies() {
        let a = NodeIdentity::generate();
        let k = key(&a);
        let sig = a.sign_frame(FrameKind::Request, A, B, 7, BODY);
        assert!(
            !verify_frame(&k, FrameKind::Result, A, B, 7, BODY, &sig),
            "kind"
        );
        assert!(
            !verify_frame(&k, FrameKind::Request, C, B, 7, BODY, &sig),
            "from"
        );
        assert!(
            !verify_frame(&k, FrameKind::Request, A, C, 7, BODY, &sig),
            "to"
        );
        assert!(
            !verify_frame(&k, FrameKind::Request, A, B, 8, BODY, &sig),
            "ctr"
        );
        assert!(
            !verify_frame(&k, FrameKind::Request, A, B, 7, b"{}", &sig),
            "payload"
        );
    }

    /// Length prefixes keep `("ab", "c")` and `("a", "bc")` apart.
    #[test]
    fn ids_cannot_slide_into_each_other() {
        assert_ne!(
            frame_region(FrameKind::Request, "ab", "c", 1, b""),
            frame_region(FrameKind::Request, "a", "bc", 1, b"")
        );
    }

    #[test]
    fn a_frame_signature_is_not_an_announcement_signature() {
        let a = NodeIdentity::generate();
        let region = frame_region(FrameKind::Request, A, B, 7, BODY);
        let sig = hex::encode(a.key.sign(&region).to_bytes());
        assert!(!verify_announcement(&a.public_hex(), &region, &sig));
        let body = b"announcement";
        assert!(verify_announcement(
            &a.public_hex(),
            body,
            &a.sign_announcement(body)
        ));
    }

    #[test]
    fn malformed_input_is_refused_not_an_error() {
        let a = NodeIdentity::generate();
        assert!(!verify_frame(
            &key(&a),
            FrameKind::Request,
            A,
            B,
            7,
            BODY,
            "zz"
        ));
        assert!(!verify_frame(
            &key(&a),
            FrameKind::Request,
            A,
            B,
            7,
            BODY,
            ""
        ));
        assert!(!verify_announcement("00", BODY, "00"));
        assert!(parse_public("not hex").is_none());
    }

    #[test]
    fn the_first_key_is_pinned_and_a_different_one_is_refused() {
        let keys = PeerKeys::new();
        let (a, imposter) = (NodeIdentity::generate(), NodeIdentity::generate());
        assert_eq!(keys.pin(A, key(&a)), PinVerdict::Pinned);
        assert_eq!(keys.pin(A, key(&a)), PinVerdict::Known);
        assert_eq!(keys.pin(A, key(&imposter)), PinVerdict::Mismatch);
        assert_eq!(keys.key_for(A), Some(key(&a)));
    }

    #[test]
    fn a_configured_pin_is_not_replaced_by_the_mesh() {
        let (a, imposter) = (NodeIdentity::generate(), NodeIdentity::generate());
        let keys =
            PeerKeys::configured(&BTreeMap::from([(A.to_string(), a.public_hex())])).unwrap();
        assert_eq!(keys.pin(A, key(&imposter)), PinVerdict::Mismatch);
        assert!(PeerKeys::configured(&BTreeMap::from([(A.to_string(), "00".into())])).is_err());
    }

    #[test]
    fn learned_pins_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.json");
        let (a, imposter) = (NodeIdentity::generate(), NodeIdentity::generate());
        let keys = PeerKeys::new().with_store(&path).unwrap();
        assert_eq!(keys.pin(A, key(&a)), PinVerdict::Pinned);
        let reloaded = PeerKeys::new().with_store(&path).unwrap();
        assert_eq!(reloaded.pin(A, key(&imposter)), PinVerdict::Mismatch);
    }

    #[test]
    fn an_identity_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id.key");
        let first = NodeIdentity::load_or_create(&path).unwrap();
        let again = NodeIdentity::load_or_create(&path).unwrap();
        assert_eq!(first.public_hex(), again.public_hex());
    }

    #[test]
    fn a_configured_pin_removed_from_the_config_is_not_kept_by_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.json");
        let (a, b) = (NodeIdentity::generate(), NodeIdentity::generate());
        let configured = BTreeMap::from([(A.to_string(), a.public_hex())]);
        let keys = PeerKeys::configured(&configured)
            .unwrap()
            .with_store(&path)
            .unwrap();
        // A learned pin is what triggers a write; A's configured one is not in it.
        assert_eq!(keys.pin(B, key(&b)), PinVerdict::Pinned);
        let stored: BTreeMap<String, String> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored.keys().collect::<Vec<_>>(), [B]);

        // The operator removes A's entry: A is unpinned, B still is.
        let reloaded = PeerKeys::new().with_store(&path).unwrap();
        assert_eq!(reloaded.key_for(A), None);
        assert_eq!(reloaded.key_for(B), Some(key(&b)));
    }

    #[test]
    fn a_learned_pin_overridden_by_the_config_is_dropped_from_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.json");
        let (old, new, c) = (
            NodeIdentity::generate(),
            NodeIdentity::generate(),
            NodeIdentity::generate(),
        );
        let keys = PeerKeys::new().with_store(&path).unwrap();
        assert_eq!(keys.pin(A, key(&old)), PinVerdict::Pinned);
        // A re-keyed; the operator pins the new key by hand.
        let keys = PeerKeys::configured(&BTreeMap::from([(A.to_string(), new.public_hex())]))
            .unwrap()
            .with_store(&path)
            .unwrap();
        assert_eq!(keys.key_for(A), Some(key(&new)));
        assert_eq!(keys.pin(C, key(&c)), PinVerdict::Pinned);
        // With the entry gone again, the stale key does not come back.
        let reloaded = PeerKeys::new().with_store(&path).unwrap();
        assert_eq!(reloaded.key_for(A), None);
    }

    #[test]
    fn a_restarted_node_is_the_same_peer() {
        let dir = tempfile::tempdir().unwrap();
        let first = P2pState::load(dir.path(), PeerKeys::new()).unwrap();
        let b = NodeIdentity::generate();
        assert_eq!(first.peer_keys.pin(B, key(&b)), PinVerdict::Pinned);

        let again = P2pState::load(dir.path(), PeerKeys::new()).unwrap();
        assert_eq!(again.identity.public_hex(), first.identity.public_hex());
        assert_eq!(again.peer_keys.key_for(B), Some(key(&b)));
        assert!(again.announce_epoch > first.announce_epoch);
    }

    #[test]
    fn the_boot_epoch_goes_up_even_when_the_clock_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p2p_announce_epoch");
        // A previous start far ahead of this clock, as on a board whose clock reset.
        std::fs::write(&path, (u32::MAX - 5).to_string()).unwrap();
        assert_eq!(next_boot_epoch(&path), u32::MAX - 4);
        assert_eq!(next_boot_epoch(&path), u32::MAX - 3);
    }

    #[test]
    fn files_are_replaced_whole_and_a_stale_temp_file_is_harmless() {
        let dir = tempfile::tempdir().unwrap();
        let (id_path, pins_path) = (dir.path().join("id.key"), dir.path().join("peers.json"));
        // What a crash mid-write used to leave, now only ever in the temp file.
        std::fs::write(dir.path().join("id.key.tmp"), "0123").unwrap();
        std::fs::write(dir.path().join("peers.json.tmp"), "{\"obc-no").unwrap();

        let identity = NodeIdentity::load_or_create(&id_path).unwrap();
        let keys = PeerKeys::new().with_store(&pins_path).unwrap();
        assert_eq!(keys.pin(A, key(&identity)), PinVerdict::Pinned);

        assert_eq!(
            NodeIdentity::load_or_create(&id_path).unwrap().public_hex(),
            identity.public_hex()
        );
        let reloaded = PeerKeys::new().with_store(&pins_path).unwrap();
        assert_eq!(reloaded.key_for(A), Some(key(&identity)));
        for leftover in ["id.key.tmp", "peers.json.tmp"] {
            assert!(
                !dir.path().join(leftover).exists(),
                "{leftover} left behind"
            );
        }
    }
}
