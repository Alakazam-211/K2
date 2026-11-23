//! The sim stream: one WebSocket per session, dialed by k2-node to
//! [`super::STREAM_PATH`], encrypted end to end (review IR-1, IR-2).
//!
//! **Why encrypt, not only sign.** TLS ends at the relay (see
//! [`crate::secret`]), and this stream carries screens, typed text, the
//! `type --secret` value and uploaded apps. **Why per connection.** A sim
//! session survives a redial, so anything bound only to the session could
//! be replayed into the next socket.
//!
//! **Handshake** (text JSON, before any binary message):
//! 1. node → server [`StreamHello`]: node fp, session, `nonce_n`, a fresh
//!    X25519 public key `epk_n`.
//! 2. server → node [`StreamChallenge`]: server fp, `nonce_c`, its own
//!    fresh `epk_c`, and `sig_c(DOMAIN_CONTROLLER, [session, nonce_n,
//!    epk_n, node_fp, nonce_c, epk_c])`.
//! 3. node → server [`StreamProof`]: `sig_n(DOMAIN_NODE, [session, nonce_c,
//!    epk_c, controller_fp, nonce_n, epk_n])`.
//!
//! Both signatures cover both fresh keys, so nothing on the path can swap
//! one. The connection id is `sha256(signed_bytes(DOMAIN_CONN, [nonce_n,
//! nonce_c]))`, like attach's session id. Keys: HKDF-SHA256 (salt
//! [`DOMAIN`]) over the X25519 secret with info `[DOMAIN, conn, session,
//! controller_fp, node_fp]`, expanded once per direction (`c2n`, `n2c`).
//!
//! **Messages** (binary): `seq u64 BE ‖ channel u8 ‖ AES-256-GCM(ct ‖ tag)`
//! with nonce = `0^4 ‖ seq` and AAD = `direction ‖ channel ‖ seq`. `seq`
//! starts at 0 and must be exactly the next one; any failure closes the
//! socket. A message from another connection fails (other keys), and a
//! message reflected back fails (the other direction's key).

use aws_lc_rs::{aead, agreement, hkdf, rand};
use serde::{Deserialize, Serialize};

use crate::crypto::{self, SigningKey};

pub const DOMAIN: &str = "k2-compute-sim-v1";
pub const DOMAIN_CONTROLLER: &str = "k2-compute-sim-v1/controller";
pub const DOMAIN_NODE: &str = "k2-compute-sim-v1/node";
pub const DOMAIN_CONN: &str = "k2-compute-sim-v1/conn";

/// Largest binary message (a full-resolution PNG screenshot fits).
pub const MAX_MESSAGE: usize = 16 << 20;
/// seq (8) + channel (1).
pub const HEADER: usize = 9;
/// AES-GCM tag.
pub const TAG: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamHello {
    pub node_fp: String,
    pub session: String,
    pub nonce_n: String,
    pub epk_n: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamChallenge {
    pub controller_fp: String,
    pub nonce_c: String,
    pub epk_c: String,
    pub sig: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamProof {
    pub sig: String,
}

/// The text frames of the handshake. Either side may send `refused`, then close.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum StreamHandshake {
    Hello(StreamHello),
    Challenge(StreamChallenge),
    Proof(StreamProof),
    Refused(crate::frames::Refused),
}

/// Which end seals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Controller,
    Node,
}

fn eph() -> Result<(agreement::EphemeralPrivateKey, Vec<u8>), String> {
    let rng = rand::SystemRandom::new();
    let k = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng).map_err(|_| "X25519 key generation failed".to_string())?;
    let p = k.compute_public_key().map_err(|_| "X25519 public key failed".to_string())?.as_ref().to_vec();
    Ok((k, p))
}

fn conn_id(nonce_n: &str, nonce_c: &str) -> String {
    crypto::sha256_hex(&crypto::signed_bytes(DOMAIN_CONN, &[nonce_n.as_bytes(), nonce_c.as_bytes()]))
}

/// The node's half, between hello and proof.
pub struct NodeHandshake {
    key: agreement::EphemeralPrivateKey,
    hello: StreamHello,
}

impl NodeHandshake {
    /// A fresh hello for `session`, signed later by [`Self::finish`].
    pub fn start(node_key: &SigningKey, session: &str) -> Result<(Self, StreamHello), String> {
        let (key, epk) = eph()?;
        let hello = StreamHello { node_fp: node_key.fingerprint(), session: session.to_string(), nonce_n: crypto::nonce_b64()?, epk_n: crypto::b64(&epk) };
        Ok((Self { key, hello: hello.clone() }, hello))
    }

    /// Check the server's challenge against its pinned key, then prove and
    /// derive the keys.
    pub fn finish(self, node_key: &SigningKey, controller_spki_der: &[u8], ch: &StreamChallenge) -> Result<(StreamProof, StreamKeys), String> {
        let h = &self.hello;
        if ch.controller_fp != crypto::fingerprint_of_spki_der(controller_spki_der) {
            return Err("the sim stream answered with another server's key".into());
        }
        let ok = crypto::verify_domain(
            controller_spki_der,
            DOMAIN_CONTROLLER,
            &[h.session.as_bytes(), h.nonce_n.as_bytes(), h.epk_n.as_bytes(), h.node_fp.as_bytes(), ch.nonce_c.as_bytes(), ch.epk_c.as_bytes()],
            &ch.sig,
        );
        if !ok {
            return Err("bad sim stream challenge signature".into());
        }
        let sig = node_key.sign_domain(
            DOMAIN_NODE,
            &[h.session.as_bytes(), ch.nonce_c.as_bytes(), ch.epk_c.as_bytes(), ch.controller_fp.as_bytes(), h.nonce_n.as_bytes(), h.epk_n.as_bytes()],
        )?;
        let keys = derive(self.key, &ch.epk_c, &h.session, &h.nonce_n, &ch.nonce_c, &ch.controller_fp, &h.node_fp)?;
        Ok((StreamProof { sig }, keys))
    }
}

/// The server's half, between challenge and proof.
pub struct ControllerHandshake {
    key: agreement::EphemeralPrivateKey,
    hello: StreamHello,
    challenge: StreamChallenge,
    node_spki: Vec<u8>,
}

impl ControllerHandshake {
    /// Answer a hello from the node whose enrolled key is `node_spki_der`.
    /// The caller has already checked that `hello.session` is a session it
    /// opened on exactly that node.
    pub fn answer(controller_key: &SigningKey, node_spki_der: &[u8], hello: &StreamHello) -> Result<(Self, StreamChallenge), String> {
        if hello.node_fp != crypto::fingerprint_of_spki_der(node_spki_der) {
            return Err("the hello names another node".into());
        }
        if !super::valid_session(&hello.session) {
            return Err("bad session id".into());
        }
        let epk_n = crypto::unb64(&hello.epk_n).map_err(|_| "bad epk_n".to_string())?;
        if epk_n.len() != 32 || crypto::unb64(&hello.nonce_n).map(|n| n.len()) != Ok(32) {
            return Err("bad hello key or nonce".into());
        }
        let (key, epk) = eph()?;
        let nonce_c = crypto::nonce_b64()?;
        let epk_c = crypto::b64(&epk);
        let fp = controller_key.fingerprint();
        let sig = controller_key.sign_domain(
            DOMAIN_CONTROLLER,
            &[hello.session.as_bytes(), hello.nonce_n.as_bytes(), hello.epk_n.as_bytes(), hello.node_fp.as_bytes(), nonce_c.as_bytes(), epk_c.as_bytes()],
        )?;
        let challenge = StreamChallenge { controller_fp: fp, nonce_c, epk_c, sig };
        Ok((Self { key, hello: hello.clone(), challenge: challenge.clone(), node_spki: node_spki_der.to_vec() }, challenge))
    }

    pub fn finish(self, proof: &StreamProof) -> Result<StreamKeys, String> {
        let (h, c) = (&self.hello, &self.challenge);
        let ok = crypto::verify_domain(
            &self.node_spki,
            DOMAIN_NODE,
            &[h.session.as_bytes(), c.nonce_c.as_bytes(), c.epk_c.as_bytes(), c.controller_fp.as_bytes(), h.nonce_n.as_bytes(), h.epk_n.as_bytes()],
            &proof.sig,
        );
        if !ok {
            return Err("bad sim stream proof signature".into());
        }
        derive(self.key, &h.epk_n, &h.session, &h.nonce_n, &c.nonce_c, &c.controller_fp, &h.node_fp)
    }
}

/// Both directions' keys for one connection.
pub struct StreamKeys {
    pub conn_id: String,
    c2n: [u8; 32],
    n2c: [u8; 32],
}

impl std::fmt::Debug for StreamKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StreamKeys({})", self.conn_id)
    }
}

impl Drop for StreamKeys {
    fn drop(&mut self) {
        crate::secret::wipe(&mut self.c2n);
        crate::secret::wipe(&mut self.n2c);
    }
}

struct Len(usize);
impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn derive(
    key: agreement::EphemeralPrivateKey,
    peer_epk_b64: &str,
    session: &str,
    nonce_n: &str,
    nonce_c: &str,
    controller_fp: &str,
    node_fp: &str,
) -> Result<StreamKeys, String> {
    let peer = crypto::unb64(peer_epk_b64).map_err(|_| "bad peer key".to_string())?;
    if peer.len() != 32 {
        return Err("bad peer key".into());
    }
    let conn = conn_id(nonce_n, nonce_c);
    let info = crypto::signed_bytes(DOMAIN, &[conn.as_bytes(), session.as_bytes(), controller_fp.as_bytes(), node_fp.as_bytes()]);
    agreement::agree_ephemeral(key, agreement::UnparsedPublicKey::new(&agreement::X25519, &peer), (), |shared| {
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, DOMAIN.as_bytes()).extract(shared);
        let mut c2n = [0u8; 32];
        let mut n2c = [0u8; 32];
        prk.expand(&[&info, b"c2n"], Len(32)).map_err(|_| ())?.fill(&mut c2n).map_err(|_| ())?;
        prk.expand(&[&info, b"n2c"], Len(32)).map_err(|_| ())?.fill(&mut n2c).map_err(|_| ())?;
        Ok(StreamKeys { conn_id: conn.clone(), c2n, n2c })
    })
    .map_err(|_: ()| "X25519 agreement failed".to_string())
}

/// Why a message was refused. Every one closes the socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamError {
    TooLarge,
    Short,
    Sequence { expected: u64, got: u64 },
    /// Wrong key (another connection, the other direction) or tampered.
    Open,
    Seal,
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamError::TooLarge => write!(f, "sim stream message over {MAX_MESSAGE} bytes"),
            StreamError::Short => write!(f, "sim stream message too short"),
            StreamError::Sequence { expected, got } => write!(f, "sim stream sequence {got}, expected {expected}"),
            StreamError::Open => write!(f, "sim stream message doesn't open (wrong connection, wrong direction, or tampered)"),
            StreamError::Seal => write!(f, "sim stream seal failed"),
        }
    }
}

fn dir_byte(c2n: bool) -> u8 {
    if c2n {
        b'c'
    } else {
        b'n'
    }
}

fn nonce(seq: u64) -> aead::Nonce {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&seq.to_be_bytes());
    aead::Nonce::assume_unique_for_key(n)
}

fn aad(c2n: bool, channel: u8, seq: u64) -> [u8; 10] {
    let mut a = [0u8; 10];
    a[0] = dir_byte(c2n);
    a[1] = channel;
    a[2..].copy_from_slice(&seq.to_be_bytes());
    a
}

fn less_safe(k: &[u8; 32]) -> aead::LessSafeKey {
    aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, k).expect("a 32-byte AES-256 key"))
}

/// Seals this side's outgoing messages.
pub struct Sealer {
    key: aead::LessSafeKey,
    c2n: bool,
    next: u64,
}

/// Opens the peer's messages, strictly in order.
pub struct Opener {
    key: aead::LessSafeKey,
    c2n: bool,
    expected: u64,
}

impl StreamKeys {
    /// This side's sealer and opener.
    pub fn split(self, side: Side) -> (Sealer, Opener) {
        let c2n = less_safe(&self.c2n);
        let n2c = less_safe(&self.n2c);
        match side {
            Side::Controller => (Sealer { key: c2n, c2n: true, next: 0 }, Opener { key: n2c, c2n: false, expected: 0 }),
            Side::Node => (Sealer { key: n2c, c2n: false, next: 0 }, Opener { key: c2n, c2n: true, expected: 0 }),
        }
    }
}

impl Sealer {
    pub fn seal(&mut self, channel: u8, plain: &[u8]) -> Result<Vec<u8>, StreamError> {
        if HEADER + plain.len() + TAG > MAX_MESSAGE {
            return Err(StreamError::TooLarge);
        }
        let seq = self.next;
        let mut out = Vec::with_capacity(HEADER + plain.len() + TAG);
        out.extend_from_slice(&seq.to_be_bytes());
        out.push(channel);
        let mut body = plain.to_vec();
        self.key.seal_in_place_append_tag(nonce(seq), aead::Aad::from(aad(self.c2n, channel, seq)), &mut body).map_err(|_| StreamError::Seal)?;
        out.extend_from_slice(&body);
        self.next += 1;
        Ok(out)
    }
}

impl Opener {
    /// `(channel, plaintext)`. Any error is final: close the socket.
    pub fn open(&mut self, msg: &[u8]) -> Result<(u8, Vec<u8>), StreamError> {
        if msg.len() > MAX_MESSAGE {
            return Err(StreamError::TooLarge);
        }
        if msg.len() < HEADER + TAG {
            return Err(StreamError::Short);
        }
        let seq = u64::from_be_bytes(msg[..8].try_into().expect("8 bytes"));
        if seq != self.expected {
            return Err(StreamError::Sequence { expected: self.expected, got: seq });
        }
        let channel = msg[8];
        let mut body = msg[HEADER..].to_vec();
        let n = self.key.open_in_place(nonce(seq), aead::Aad::from(aad(self.c2n, channel, seq)), &mut body).map_err(|_| StreamError::Open)?.len();
        body.truncate(n);
        self.expected += 1;
        Ok((channel, body))
    }
}

/// Run both handshake halves in memory (tests on both crates, and the
/// fake node). Returns `(controller keys, node keys)`.
pub fn handshake_in_memory(controller: &SigningKey, node: &SigningKey, session: &str) -> Result<(StreamKeys, StreamKeys), String> {
    let (nh, hello) = NodeHandshake::start(node, session)?;
    let (ch, challenge) = ControllerHandshake::answer(controller, node.spki_der(), &hello)?;
    let (proof, node_keys) = nh.finish(node, controller.spki_der(), &challenge)?;
    let ctl_keys = ch.finish(&proof)?;
    Ok((ctl_keys, node_keys))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "0123456789abcdef0123456789abcdef";
    const CANARY: &[u8] = b"k2-canary-hunter2-SECRET";

    fn keys() -> (SigningKey, SigningKey) {
        (SigningKey::generate().unwrap(), SigningKey::generate().unwrap())
    }

    #[test]
    fn round_trip_both_ways_and_no_plaintext_on_the_wire() {
        let (c, n) = keys();
        let (ck, nk) = handshake_in_memory(&c, &n, SESSION).unwrap();
        assert_eq!(ck.conn_id, nk.conn_id);
        let (mut cs, mut co) = ck.split(Side::Controller);
        let (mut ns, mut no) = nk.split(Side::Node);
        for i in 0..3u8 {
            let m = cs.seal(i, CANARY).unwrap();
            assert!(!m.windows(CANARY.len()).any(|w| w == CANARY), "the relay leg carries the canary in plain");
            assert_eq!(no.open(&m).unwrap(), (i, CANARY.to_vec()));
            let r = ns.seal(1, b"reply").unwrap();
            assert_eq!(co.open(&r).unwrap(), (1, b"reply".to_vec()));
        }
    }

    #[test]
    fn a_message_replayed_into_the_next_connection_is_refused() {
        let (c, n) = keys();
        let (ck1, _nk1) = handshake_in_memory(&c, &n, SESSION).unwrap();
        let (mut cs1, _) = ck1.split(Side::Controller);
        let recorded = cs1.seal(1, br#"{"op":"tap","x":1,"y":2}"#).unwrap();
        // Same session, a redial: new keys.
        let (_ck2, nk2) = handshake_in_memory(&c, &n, SESSION).unwrap();
        let (_, mut no2) = nk2.split(Side::Node);
        assert_eq!(no2.open(&recorded).unwrap_err(), StreamError::Open);
    }

    #[test]
    fn a_reflected_message_is_refused() {
        let (c, n) = keys();
        let (ck, nk) = handshake_in_memory(&c, &n, SESSION).unwrap();
        let (mut cs, mut co) = ck.split(Side::Controller);
        let (_ns, _no) = nk.split(Side::Node);
        let m = cs.seal(0, b"run").unwrap();
        // Bounced straight back at the controller: wrong direction's key.
        assert_eq!(co.open(&m).unwrap_err(), StreamError::Open);
    }

    #[test]
    fn flipped_bytes_wrong_seq_and_wrong_channel_are_refused() {
        let (c, n) = keys();
        let (ck, nk) = handshake_in_memory(&c, &n, SESSION).unwrap();
        let (mut cs, _) = ck.split(Side::Controller);
        let (_, mut no) = nk.split(Side::Node);
        let m0 = cs.seal(0, b"a").unwrap();
        let m1 = cs.seal(0, b"b").unwrap();
        assert_eq!(no.open(&m1).unwrap_err(), StreamError::Sequence { expected: 0, got: 1 });
        let mut flipped = m0.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 1;
        assert_eq!(no.open(&flipped).unwrap_err(), StreamError::Open);
        let mut chan = m0.clone();
        chan[8] = 5;
        assert_eq!(no.open(&chan).unwrap_err(), StreamError::Open, "the channel is authenticated");
        assert_eq!(no.open(&m0[..10]).unwrap_err(), StreamError::Short);
        assert_eq!(no.open(&m0).unwrap(), (0, b"a".to_vec()));
        assert_eq!(no.open(&m0).unwrap_err(), StreamError::Sequence { expected: 1, got: 0 }, "a replay inside the connection");
    }

    #[test]
    fn the_handshake_checks_both_keys() {
        let (c, n) = keys();
        let stranger = SigningKey::generate().unwrap();
        // A hello claiming another node.
        let (_, hello) = NodeHandshake::start(&stranger, SESSION).unwrap();
        assert!(ControllerHandshake::answer(&c, n.spki_der(), &hello).is_err());
        // A challenge from someone other than the pinned server.
        let (nh, hello) = NodeHandshake::start(&n, SESSION).unwrap();
        let (_, challenge) = ControllerHandshake::answer(&stranger, n.spki_der(), &hello).unwrap();
        assert!(nh.finish(&n, c.spki_der(), &challenge).is_err());
        // A swapped server key in the challenge.
        let (nh, hello) = NodeHandshake::start(&n, SESSION).unwrap();
        let (_, mut challenge) = ControllerHandshake::answer(&c, n.spki_der(), &hello).unwrap();
        let (_, other) = eph().unwrap();
        challenge.epk_c = crypto::b64(&other);
        assert!(nh.finish(&n, c.spki_der(), &challenge).is_err());
        // A proof signed by another key.
        let (nh, hello) = NodeHandshake::start(&n, SESSION).unwrap();
        let (ch, challenge) = ControllerHandshake::answer(&c, n.spki_der(), &hello).unwrap();
        let (_, _) = nh.finish(&n, c.spki_der(), &challenge).unwrap();
        let forged = StreamProof { sig: stranger.sign_domain(DOMAIN_NODE, &[b"x"]).unwrap() };
        assert!(ch.finish(&forged).is_err());
    }

    #[test]
    fn oversized_messages_are_refused() {
        let (c, n) = keys();
        let (ck, _) = handshake_in_memory(&c, &n, SESSION).unwrap();
        let (mut cs, mut co) = ck.split(Side::Controller);
        assert_eq!(cs.seal(0, &vec![0u8; MAX_MESSAGE]).unwrap_err(), StreamError::TooLarge);
        assert_eq!(co.open(&vec![0u8; MAX_MESSAGE + 1]).unwrap_err(), StreamError::TooLarge);
    }
}
