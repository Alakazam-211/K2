//! The binary structures inside call credentials, certificates, binding tables and recorder
//! releases (prd-sew-50 §9.5, prd-sew-51 §7.2, §7.3, §7.6, §15.1), in the TLS presentation
//! language as MLS uses it (RFC 9420 §2.1): big-endian integers and `opaque x<V>` vectors
//! with the MLS variable-length length prefix (§2.1.2).
//!
//! Hand-written because `sew-proto` takes no codec dependency (rev 26 G19). S50 (the server) and
//! S51 (the client library) both use it; signature checks live outside `sew-proto`. Decoding is
//! total: malformed input gives a [`TlsError`], never a panic, and allocation is bounded by the
//! input length. [`TlsCodec::from_bytes`] refuses trailing bytes.

use std::fmt;

/// The MLS exporter label for per-sender frame keys (S51 §10.1).
pub const EXPORTER_LABEL: &str = "sew call frame key v1";
/// The HPKE `info` prefix of a recorder release (§9.5, S51 §15.1).
pub const RELEASE_INFO_LABEL: &str = "sew call recorder release v1";
/// The CA signature label of a device certificate.
pub const DEVICE_CERT_LABEL: &str = "sew call device cert v1";
/// The CA signature label of a recorder certificate.
pub const RECORDER_CERT_LABEL: &str = "sew call recorder cert v1";
/// The device signature label inside a `SewCallIdentity` (version 2, rev 78 D2).
pub const LEAF_SIG_LABEL: &str = "sew call leaf v2";
/// The proof-of-possession label of a device enrolment (K11).
pub const ENROLL_LABEL: &str = "sew call device enroll v1";
/// The DS signature label of a `LeafBindingTable`.
pub const LEAF_BINDINGS_LABEL: &str = "sew call leaf binding v1";

/// The largest value an MLS variable-length integer can carry (2^30 − 1).
pub const VARINT_MAX: u64 = (1 << 30) - 1;

/// Why bytes did not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsError {
    /// The input ended inside a value.
    Truncated,
    /// Bytes remained after the value.
    Trailing,
    /// A length prefix used more bytes than needed (RFC 9420 §2.1.2 requires the minimum).
    VarintNotMinimal,
    /// A length prefix with the reserved `11` top bits.
    VarintInvalid,
    /// A value that is out of range for its field (the field is named).
    BadValue(&'static str),
    /// A value too long to encode (over 2^30 − 1 bytes).
    TooLong,
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TlsError::Truncated => f.write_str("truncated"),
            TlsError::Trailing => f.write_str("trailing bytes"),
            TlsError::VarintNotMinimal => f.write_str("non-minimal length prefix"),
            TlsError::VarintInvalid => f.write_str("invalid length prefix"),
            TlsError::BadValue(field) => write!(f, "bad value for {field}"),
            TlsError::TooLong => f.write_str("value too long"),
        }
    }
}

impl std::error::Error for TlsError {}

/// A cursor over input bytes.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Starts at the beginning of `buf`.
    pub fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf, pos: 0 }
    }

    /// Bytes not yet read.
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Whether everything was read.
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Takes `n` bytes.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], TlsError> {
        if self.remaining() < n {
            return Err(TlsError::Truncated);
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    /// A `uint8`.
    pub fn u8(&mut self) -> Result<u8, TlsError> {
        Ok(self.take(1)?[0])
    }

    /// A big-endian `uint32`.
    pub fn u32(&mut self) -> Result<u32, TlsError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A big-endian `uint64`.
    pub fn u64(&mut self) -> Result<u64, TlsError> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_be_bytes(a))
    }

    /// An MLS variable-length integer (RFC 9420 §2.1.2), minimal encoding only.
    pub fn varint(&mut self) -> Result<u64, TlsError> {
        let first = self.u8()?;
        let (len, mut v) = match first >> 6 {
            0 => return Ok(u64::from(first)),
            1 => (2usize, u64::from(first & 0x3f)),
            2 => (4usize, u64::from(first & 0x3f)),
            _ => return Err(TlsError::VarintInvalid),
        };
        for &b in self.take(len - 1)? {
            v = (v << 8) | u64::from(b);
        }
        let minimal = match len {
            2 => v >= 64,
            _ => v >= 16_384,
        };
        if !minimal {
            return Err(TlsError::VarintNotMinimal);
        }
        Ok(v)
    }

    /// An `opaque x<V>`.
    pub fn opaque(&mut self) -> Result<&'a [u8], TlsError> {
        let n = self.varint()?;
        let n = usize::try_from(n).map_err(|_| TlsError::TooLong)?;
        self.take(n)
    }

    /// An `opaque x<V>` as an owned vector.
    pub fn opaque_vec(&mut self) -> Result<Vec<u8>, TlsError> {
        Ok(self.opaque()?.to_vec())
    }

    /// A fixed-size `opaque x[N]`.
    pub fn fixed<const N: usize>(&mut self) -> Result<[u8; N], TlsError> {
        let b = self.take(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(b);
        Ok(a)
    }

    /// Fails unless everything was read.
    pub fn finish(&self) -> Result<(), TlsError> {
        if self.is_empty() {
            Ok(())
        } else {
            Err(TlsError::Trailing)
        }
    }
}

/// Appends MLS-encoded values to a buffer.
#[derive(Debug, Default, Clone)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// An empty writer.
    pub fn new() -> Writer {
        Writer::default()
    }

    /// The bytes written.
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// A `uint8`.
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    /// A big-endian `uint32`.
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    /// A big-endian `uint64`.
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    /// Raw bytes (a fixed-size field).
    pub fn raw(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    /// An MLS variable-length integer, minimal encoding. Values over [`VARINT_MAX`] are an
    /// error.
    pub fn varint(&mut self, v: u64) -> Result<(), TlsError> {
        if v < 64 {
            self.buf.push(v as u8);
        } else if v < 16_384 {
            self.buf
                .extend_from_slice(&((v as u16) | 0x4000).to_be_bytes());
        } else if v <= VARINT_MAX {
            self.buf
                .extend_from_slice(&((v as u32) | 0x8000_0000).to_be_bytes());
        } else {
            return Err(TlsError::TooLong);
        }
        Ok(())
    }

    /// An `opaque x<V>`.
    pub fn opaque(&mut self, b: &[u8]) -> Result<(), TlsError> {
        self.varint(b.len() as u64)?;
        self.buf.extend_from_slice(b);
        Ok(())
    }
}

/// A §9.5 structure.
pub trait TlsCodec: Sized {
    /// Appends the encoding.
    fn encode(&self, w: &mut Writer) -> Result<(), TlsError>;
    /// Reads one value.
    fn decode(r: &mut Reader<'_>) -> Result<Self, TlsError>;

    /// The encoding as bytes.
    fn to_bytes(&self) -> Result<Vec<u8>, TlsError> {
        let mut w = Writer::new();
        self.encode(&mut w)?;
        Ok(w.into_bytes())
    }

    /// Decodes exactly one value from `b` (trailing bytes refused).
    fn from_bytes(b: &[u8]) -> Result<Self, TlsError> {
        let mut r = Reader::new(b);
        let v = Self::decode(&mut r)?;
        r.finish()?;
        Ok(v)
    }
}

/// `label || tbs`: the bytes a CA, device or DS signature covers.
pub fn signing_input(label: &str, tbs: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(label.len() + tbs.len());
    v.extend_from_slice(label.as_bytes());
    v.extend_from_slice(tbs);
    v
}

/// The bytes a K11 enrolment `proof` signs: `ENROLL_LABEL || u64 install_id || u64 member_id ||
/// pubkey` (prd-sew-50 §10.2).
pub fn enroll_proof_input(install_id: u64, member_id: u64, pubkey: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(ENROLL_LABEL.len() + 16 + pubkey.len());
    v.extend_from_slice(ENROLL_LABEL.as_bytes());
    v.extend_from_slice(&install_id.to_be_bytes());
    v.extend_from_slice(&member_id.to_be_bytes());
    v.extend_from_slice(pubkey);
    v
}

/// Device certificate platforms (`DeviceCertTbs.platform`).
pub mod platform {
    /// Web.
    pub const WEB: u8 = 1;
    /// Desktop.
    pub const DESKTOP: u8 = 2;
    /// iOS.
    pub const IOS: u8 = 3;
    /// Android.
    pub const ANDROID: u8 = 4;
    /// Other.
    pub const OTHER: u8 = 5;

    /// The wire name of a platform code, if known.
    pub fn name(code: u8) -> Option<&'static str> {
        Some(match code {
            WEB => "web",
            DESKTOP => "desktop",
            IOS => "ios",
            ANDROID => "android",
            OTHER => "other",
            _ => return None,
        })
    }

    /// The code of a wire name, if known.
    pub fn code(name: &str) -> Option<u8> {
        Some(match name {
            "web" => WEB,
            "desktop" => DESKTOP,
            "ios" => IOS,
            "android" => ANDROID,
            "other" => OTHER,
            _ => return None,
        })
    }
}

/// `DeviceCertTbs` (§9.5): what the install CA signs for a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCertTbs {
    /// Always 1.
    pub version: u8,
    /// UTF-8 decimal install id.
    pub install_id: Vec<u8>,
    /// The member (Sew id).
    pub member_id: u64,
    /// The device (Sew id).
    pub device_id: u64,
    /// 65-byte SEC1 uncompressed P-256 public key.
    pub device_pubkey: Vec<u8>,
    /// [`platform`] code.
    pub platform: u8,
    /// UTF-8, ≤ 64 bytes.
    pub label: Vec<u8>,
    /// Unique per install; the CRL names serials.
    pub serial: u64,
    /// Unix seconds.
    pub not_before: u64,
    /// Unix seconds.
    pub not_after: u64,
}

impl TlsCodec for DeviceCertTbs {
    fn encode(&self, w: &mut Writer) -> Result<(), TlsError> {
        w.u8(self.version);
        w.opaque(&self.install_id)?;
        w.u64(self.member_id);
        w.u64(self.device_id);
        w.opaque(&self.device_pubkey)?;
        w.u8(self.platform);
        w.opaque(&self.label)?;
        w.u64(self.serial);
        w.u64(self.not_before);
        w.u64(self.not_after);
        Ok(())
    }

    fn decode(r: &mut Reader<'_>) -> Result<Self, TlsError> {
        let version = r.u8()?;
        if version != 1 {
            return Err(TlsError::BadValue("DeviceCertTbs.version"));
        }
        Ok(DeviceCertTbs {
            version,
            install_id: r.opaque_vec()?,
            member_id: r.u64()?,
            device_id: r.u64()?,
            device_pubkey: r.opaque_vec()?,
            platform: r.u8()?,
            label: r.opaque_vec()?,
            serial: r.u64()?,
            not_before: r.u64()?,
            not_after: r.u64()?,
        })
    }
}

/// `SewDeviceCert`: the TBS and the CA's Ed25519 signature over
/// `DEVICE_CERT_LABEL || TLS(tbs)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SewDeviceCert {
    /// What was signed.
    pub tbs: DeviceCertTbs,
    /// 64-byte Ed25519 signature.
    pub ca_sig: Vec<u8>,
}

impl SewDeviceCert {
    /// The bytes `ca_sig` covers.
    pub fn signed_bytes(&self) -> Result<Vec<u8>, TlsError> {
        Ok(signing_input(DEVICE_CERT_LABEL, &self.tbs.to_bytes()?))
    }
}

impl TlsCodec for SewDeviceCert {
    fn encode(&self, w: &mut Writer) -> Result<(), TlsError> {
        self.tbs.encode(w)?;
        w.opaque(&self.ca_sig)
    }

    fn decode(r: &mut Reader<'_>) -> Result<Self, TlsError> {
        Ok(SewDeviceCert {
            tbs: DeviceCertTbs::decode(r)?,
            ca_sig: r.opaque_vec()?,
        })
    }
}

/// `SewCallIdentity.kind` for a member leaf; anything else is refused (the recorder is not a
/// leaf).
pub const IDENTITY_KIND_MEMBER: u8 = 1;
/// The only `SewCallIdentity.version` (rev 78 shape).
pub const IDENTITY_VERSION: u8 = 2;

/// `SewCallIdentity` version 2 (§9.5, S51 §7.3): the Basic credential's identity bytes. No
/// `join_id`, no room (rev 75, rev 78 D2); `scope` is mandatory (`call:<channel_id>`, rev 88
/// D3) — the codec carries an empty one so S50 and S51 can refuse it by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SewCallIdentity {
    /// Always [`IDENTITY_VERSION`].
    pub version: u8,
    /// [`IDENTITY_KIND_MEMBER`]; other values decode but are refused by the checks.
    pub kind: u8,
    /// `call:<channel_id>`.
    pub scope: Vec<u8>,
    /// `TLS(SewDeviceCert)`.
    pub device_cert: Vec<u8>,
    /// 64-byte raw r‖s ECDSA-P256-SHA256 by the device key over
    /// [`DeviceSigInput`] with [`LEAF_SIG_LABEL`].
    pub device_sig: Vec<u8>,
}

impl TlsCodec for SewCallIdentity {
    fn encode(&self, w: &mut Writer) -> Result<(), TlsError> {
        w.u8(self.version);
        w.u8(self.kind);
        w.opaque(&self.scope)?;
        w.opaque(&self.device_cert)?;
        w.opaque(&self.device_sig)
    }

    fn decode(r: &mut Reader<'_>) -> Result<Self, TlsError> {
        let version = r.u8()?;
        if version != IDENTITY_VERSION {
            return Err(TlsError::BadValue("SewCallIdentity.version"));
        }
        Ok(SewCallIdentity {
            version,
            kind: r.u8()?,
            scope: r.opaque_vec()?,
            device_cert: r.opaque_vec()?,
            device_sig: r.opaque_vec()?,
        })
    }
}

/// What `SewCallIdentity.device_sig` signs (after [`LEAF_SIG_LABEL`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSigInput {
    /// UTF-8 decimal install id.
    pub install_id: Vec<u8>,
    /// The member.
    pub member_id: u64,
    /// The device.
    pub device_id: u64,
    /// `call:<channel_id>`.
    pub scope: Vec<u8>,
    /// The leaf's MLS signature public key.
    pub leaf_signature_key: Vec<u8>,
}

impl DeviceSigInput {
    /// `LEAF_SIG_LABEL || TLS(self)`: the bytes the device signs.
    pub fn signed_bytes(&self) -> Result<Vec<u8>, TlsError> {
        Ok(signing_input(LEAF_SIG_LABEL, &self.to_bytes()?))
    }
}

impl TlsCodec for DeviceSigInput {
    fn encode(&self, w: &mut Writer) -> Result<(), TlsError> {
        w.opaque(&self.install_id)?;
        w.u64(self.member_id);
        w.u64(self.device_id);
        w.opaque(&self.scope)?;
        w.opaque(&self.leaf_signature_key)
    }

    fn decode(r: &mut Reader<'_>) -> Result<Self, TlsError> {
        Ok(DeviceSigInput {
            install_id: r.opaque_vec()?,
            member_id: r.u64()?,
            device_id: r.u64()?,
            scope: r.opaque_vec()?,
            leaf_signature_key: r.opaque_vec()?,
        })
    }
}

/// The key of one binding entry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BindingKey {
    /// `key_type` 1: a leaf index (commit, welcome, K3 reply, recorder tables).
    Leaf(u32),
    /// `key_type` 2: the `key_package_ref` of an Add (proposals).
    KeyPackageRef(Vec<u8>),
}

impl BindingKey {
    /// The wire `key_type`.
    pub fn key_type(&self) -> u8 {
        match self {
            BindingKey::Leaf(_) => 1,
            BindingKey::KeyPackageRef(_) => 2,
        }
    }
}

/// `LeafBindingEntry` (§9.5; the encoding S51 §7.6 leaves open).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeafBindingEntry {
    /// The leaf or the Add's ref.
    pub key: BindingKey,
    /// The Sew-minted join id (its `lk_identity` is derived: [`lk_identity_for`]).
    pub join_id: [u8; 16],
}

impl TlsCodec for LeafBindingEntry {
    fn encode(&self, w: &mut Writer) -> Result<(), TlsError> {
        w.u8(self.key.key_type());
        match &self.key {
            BindingKey::Leaf(l) => w.u32(*l),
            BindingKey::KeyPackageRef(r) => w.opaque(r)?,
        }
        w.raw(&self.join_id);
        Ok(())
    }

    fn decode(r: &mut Reader<'_>) -> Result<Self, TlsError> {
        let key = match r.u8()? {
            1 => BindingKey::Leaf(r.u32()?),
            2 => BindingKey::KeyPackageRef(r.opaque_vec()?),
            _ => return Err(TlsError::BadValue("LeafBindingEntry.key_type")),
        };
        Ok(LeafBindingEntry {
            key,
            join_id: r.fixed::<16>()?,
        })
    }
}

/// Why a binding table's content was refused ([`LeafBindingTableTbs::check`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableError {
    /// Entries mix key types.
    MixedKeyTypes,
    /// Entries are not strictly ascending by key (a duplicate leaf or ref included).
    NotAscending,
    /// Two entries share a `join_id`.
    DuplicateJoinId,
}

impl fmt::Display for TableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TableError::MixedKeyTypes => "binding table mixes key types",
            TableError::NotAscending => "binding table entries not strictly ascending",
            TableError::DuplicateJoinId => "binding table repeats a join_id",
        })
    }
}

impl std::error::Error for TableError {}

/// `LeafBindingTableTbs` (§9.5): what the DS signs with `sew-call-ds` after
/// [`LEAF_BINDINGS_LABEL`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeafBindingTableTbs {
    /// The `r_…` room (UTF-8).
    pub call_id: Vec<u8>,
    /// The 16-byte group id.
    pub group_id: Vec<u8>,
    /// The proposal epoch for key type 2; the new epoch for key type 1.
    pub epoch: u64,
    /// Ascending by leaf or by ref; one key type per table.
    pub entries: Vec<LeafBindingEntry>,
}

impl LeafBindingTableTbs {
    /// `LEAF_BINDINGS_LABEL || TLS(self)`: the bytes the DS signs.
    pub fn signed_bytes(&self) -> Result<Vec<u8>, TlsError> {
        Ok(signing_input(LEAF_BINDINGS_LABEL, &self.to_bytes()?))
    }

    /// The content rules: one key type, strictly ascending keys, unique join ids.
    pub fn check(&self) -> Result<(), TableError> {
        let mut types = self.entries.iter().map(|e| e.key.key_type());
        if let Some(first) = types.next()
            && types.any(|t| t != first)
        {
            return Err(TableError::MixedKeyTypes);
        }
        if self.entries.windows(2).any(|w| w[0].key >= w[1].key) {
            return Err(TableError::NotAscending);
        }
        let mut ids: Vec<&[u8; 16]> = self.entries.iter().map(|e| &e.join_id).collect();
        ids.sort_unstable();
        if ids.windows(2).any(|w| w[0] == w[1]) {
            return Err(TableError::DuplicateJoinId);
        }
        Ok(())
    }
}

impl TlsCodec for LeafBindingTableTbs {
    fn encode(&self, w: &mut Writer) -> Result<(), TlsError> {
        w.opaque(&self.call_id)?;
        w.opaque(&self.group_id)?;
        w.u64(self.epoch);
        let mut inner = Writer::new();
        for e in &self.entries {
            e.encode(&mut inner)?;
        }
        w.opaque(&inner.into_bytes())
    }

    fn decode(r: &mut Reader<'_>) -> Result<Self, TlsError> {
        let call_id = r.opaque_vec()?;
        let group_id = r.opaque_vec()?;
        let epoch = r.u64()?;
        let mut inner = Reader::new(r.opaque()?);
        let mut entries = Vec::new();
        while !inner.is_empty() {
            entries.push(LeafBindingEntry::decode(&mut inner)?);
        }
        Ok(LeafBindingTableTbs {
            call_id,
            group_id,
            epoch,
            entries,
        })
    }
}

/// `RecorderCertTbs` (§9.5): what the CA signs for one recording's recorder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecorderCertTbs {
    /// Always 1.
    pub version: u8,
    /// UTF-8 decimal install id.
    pub install_id: Vec<u8>,
    /// The `r_…` room.
    pub call_id: Vec<u8>,
    /// The recording (Sew id).
    pub recording_id: u64,
    /// X25519 public key made by the recorder for this recording.
    pub hpke_pub: [u8; 32],
    /// Unix seconds.
    pub not_before: u64,
    /// Unix seconds, ≤ `not_before` + 24 h.
    pub not_after: u64,
}

impl TlsCodec for RecorderCertTbs {
    fn encode(&self, w: &mut Writer) -> Result<(), TlsError> {
        w.u8(self.version);
        w.opaque(&self.install_id)?;
        w.opaque(&self.call_id)?;
        w.u64(self.recording_id);
        w.raw(&self.hpke_pub);
        w.u64(self.not_before);
        w.u64(self.not_after);
        Ok(())
    }

    fn decode(r: &mut Reader<'_>) -> Result<Self, TlsError> {
        let version = r.u8()?;
        if version != 1 {
            return Err(TlsError::BadValue("RecorderCertTbs.version"));
        }
        Ok(RecorderCertTbs {
            version,
            install_id: r.opaque_vec()?,
            call_id: r.opaque_vec()?,
            recording_id: r.u64()?,
            hpke_pub: r.fixed::<32>()?,
            not_before: r.u64()?,
            not_after: r.u64()?,
        })
    }
}

/// `RecorderCert`: the TBS and the CA's Ed25519 signature over
/// `RECORDER_CERT_LABEL || TLS(tbs)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecorderCert {
    /// What was signed.
    pub tbs: RecorderCertTbs,
    /// 64-byte Ed25519 signature.
    pub ca_sig: Vec<u8>,
}

impl RecorderCert {
    /// The bytes `ca_sig` covers.
    pub fn signed_bytes(&self) -> Result<Vec<u8>, TlsError> {
        Ok(signing_input(RECORDER_CERT_LABEL, &self.tbs.to_bytes()?))
    }
}

impl TlsCodec for RecorderCert {
    fn encode(&self, w: &mut Writer) -> Result<(), TlsError> {
        self.tbs.encode(w)?;
        w.opaque(&self.ca_sig)
    }

    fn decode(r: &mut Reader<'_>) -> Result<Self, TlsError> {
        Ok(RecorderCert {
            tbs: RecorderCertTbs::decode(r)?,
            ca_sig: r.opaque_vec()?,
        })
    }
}

/// The structure inside a recorder release's HPKE `info` (§9.5, S51 §15.1; rev 88 D4 binds the
/// leaf index).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    /// The `r_…` room.
    pub call_id: Vec<u8>,
    /// The recording.
    pub recording_id: u64,
    /// The group.
    pub group_id: Vec<u8>,
    /// The epoch whose key is released.
    pub epoch: u64,
    /// The sender's leaf index in that epoch.
    pub leaf_index: u32,
    /// `slot(epoch)`.
    pub slot: u8,
    /// The sender's `p_…` identity.
    pub sender_identity: Vec<u8>,
}

impl ReleaseInfo {
    /// The full HPKE `info`: `RELEASE_INFO_LABEL || TLS(self)`.
    pub fn info(&self) -> Result<Vec<u8>, TlsError> {
        Ok(signing_input(RELEASE_INFO_LABEL, &self.to_bytes()?))
    }
}

impl TlsCodec for ReleaseInfo {
    fn encode(&self, w: &mut Writer) -> Result<(), TlsError> {
        w.opaque(&self.call_id)?;
        w.u64(self.recording_id);
        w.opaque(&self.group_id)?;
        w.u64(self.epoch);
        w.u32(self.leaf_index);
        w.u8(self.slot);
        w.opaque(&self.sender_identity)
    }

    fn decode(r: &mut Reader<'_>) -> Result<Self, TlsError> {
        Ok(ReleaseInfo {
            call_id: r.opaque_vec()?,
            recording_id: r.u64()?,
            group_id: r.opaque_vec()?,
            epoch: r.u64()?,
            leaf_index: r.u32()?,
            slot: r.u8()?,
            sender_identity: r.opaque_vec()?,
        })
    }
}

const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// Lowercase RFC 4648 base32 without padding.
pub fn base32_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() * 8).div_ceil(5));
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &b in bytes {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(BASE32[((acc >> bits) & 0x1f) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(BASE32[((acc << (5 - bits)) & 0x1f) as usize]));
    }
    out
}

/// The engine identity of a join: `"p_"` + lowercase unpadded base32 of the 16 `join_id`
/// bytes (26 characters; 28 in all). Equal to S37's `identity_for`.
pub fn lk_identity_for(join_id: &[u8; 16]) -> String {
    format!("p_{}", base32_lower(join_id))
}

/// The key ring size (rev 75): the only accepted value.
pub const KEY_RING_SIZE: u8 = 64;

/// `slot(e) = (slot_base + e) mod ring` (prd-sew-50 §6). `ring` must be non-zero.
pub fn slot(slot_base: u8, epoch: u64, ring: u8) -> u8 {
    let ring = u64::from(ring.max(1));
    ((u64::from(slot_base) + epoch % ring) % ring) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_minimal_round_trip() {
        for v in [0u64, 1, 63, 64, 16_383, 16_384, VARINT_MAX] {
            let mut w = Writer::new();
            w.varint(v).ok();
            let b = w.into_bytes();
            let mut r = Reader::new(&b);
            assert_eq!(r.varint(), Ok(v));
            assert!(r.is_empty());
        }
        let mut w = Writer::new();
        assert_eq!(w.varint(VARINT_MAX + 1), Err(TlsError::TooLong));
        // 5 written in two bytes is not minimal.
        assert_eq!(
            Reader::new(&[0x40, 0x05]).varint(),
            Err(TlsError::VarintNotMinimal)
        );
        assert_eq!(Reader::new(&[0xc0]).varint(), Err(TlsError::VarintInvalid));
    }

    #[test]
    fn identity_matches_base32_rule() {
        let id = lk_identity_for(&[0u8; 16]);
        assert_eq!(id.len(), 28);
        assert_eq!(id, "p_aaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(base32_lower(b"foobar"), "mzxw6ytboi");
    }

    #[test]
    fn slot_formula() {
        assert_eq!(slot(0, 0, 64), 0);
        assert_eq!(slot(0, 65, 64), 1);
        assert_eq!(slot(63, 1, 64), 0);
        assert_eq!(slot(10, u64::MAX, 64), ((10 + u64::MAX % 64) % 64) as u8);
    }

    #[test]
    fn table_rules() {
        let e = |l: u32, j: u8| LeafBindingEntry {
            key: BindingKey::Leaf(l),
            join_id: [j; 16],
        };
        let mut t = LeafBindingTableTbs {
            call_id: b"r_x".to_vec(),
            group_id: vec![1; 16],
            epoch: 3,
            entries: vec![e(0, 1), e(2, 2)],
        };
        assert_eq!(t.check(), Ok(()));
        let bytes = t.to_bytes().unwrap_or_default();
        assert_eq!(LeafBindingTableTbs::from_bytes(&bytes), Ok(t.clone()));
        t.entries.push(e(1, 3));
        assert_eq!(t.check(), Err(TableError::NotAscending));
        t.entries = vec![e(0, 1), e(1, 1)];
        assert_eq!(t.check(), Err(TableError::DuplicateJoinId));
        t.entries = vec![
            e(0, 1),
            LeafBindingEntry {
                key: BindingKey::KeyPackageRef(vec![9; 32]),
                join_id: [2; 16],
            },
        ];
        assert_eq!(t.check(), Err(TableError::MixedKeyTypes));
    }

    #[test]
    fn trailing_and_truncated_refused() {
        let info = ReleaseInfo {
            call_id: b"r_a".to_vec(),
            recording_id: 7,
            group_id: vec![2; 16],
            epoch: 9,
            leaf_index: 4,
            slot: 9,
            sender_identity: b"p_b".to_vec(),
        };
        let mut b = info.to_bytes().unwrap_or_default();
        for n in 0..b.len() {
            assert!(ReleaseInfo::from_bytes(&b[..n]).is_err());
        }
        b.push(0);
        assert_eq!(ReleaseInfo::from_bytes(&b), Err(TlsError::Trailing));
    }
}
