//! Time-sortable 64-bit ids (C3, gap G32).
//!
//! Layout, high bit to low bit: `0` | 42 bits ms since the Sew epoch | 10 bits node | 11 bits
//! sequence. Ids run `1 ..= i64::MAX`; `0` is never valid. JSON form is a decimal string.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;
use std::sync::Mutex;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use crate::clock::{Clock, SystemClock};
use crate::time::Timestamp;

/// The Sew epoch, 2026-01-01T00:00:00Z, in Unix milliseconds.
pub const SEW_EPOCH_MS: u64 = 1_767_225_600_000;
/// Bits holding milliseconds since the Sew epoch.
pub const TIMESTAMP_BITS: u32 = 42;
/// Bits holding the node number.
pub const NODE_BITS: u32 = 10;
/// Bits holding the per-millisecond sequence.
pub const SEQUENCE_BITS: u32 = 11;
/// Highest node number.
pub const MAX_NODE: u16 = 1023;
/// Highest sequence number within one millisecond.
pub const MAX_SEQUENCE: u16 = 2047;
/// Longest the generator waits for a clock that moved backward before it panics.
pub const CLOCK_REGRESSION_MAX_WAIT_MS: u64 = 5_000;
/// Largest millisecond offset the 42-bit field holds.
pub const MAX_MS_SINCE_EPOCH: u64 = (1 << TIMESTAMP_BITS) - 1;

const REGRESSION_STEP_MS: u64 = 100;

/// A Sew entity id. Invariant: `1 ..= i64::MAX`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(u64);

/// Space id (same [`Id`] type; the alias is for reading only).
pub type SpaceId = Id;
/// Channel id.
pub type ChannelId = Id;
/// Message id.
pub type MessageId = Id;
/// Member id.
pub type MemberId = Id;
/// Role id. `@everyone`'s role id equals its space id (gap G24).
pub type RoleId = Id;
/// Category id.
pub type CategoryId = Id;
/// Job id.
pub type JobId = Id;

impl Id {
    /// Wraps a raw value. `None` for `0` or values above `i64::MAX`.
    pub const fn new(v: u64) -> Option<Id> {
        if v == 0 || v > i64::MAX as u64 {
            None
        } else {
            Some(Id(v))
        }
    }

    /// Wraps a constant. Panics (a compile error in const context) when the value is invalid.
    pub const fn new_const(v: u64) -> Id {
        match Id::new(v) {
            Some(id) => id,
            None => panic!("sew id: constant out of range"),
        }
    }

    /// The raw value.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Converts from a Postgres `bigint`. `None` for values below 1.
    pub const fn from_i64(v: i64) -> Option<Id> {
        if v < 1 { None } else { Some(Id(v as u64)) }
    }

    /// Converts to a Postgres `bigint`. Always positive.
    pub const fn to_i64(self) -> i64 {
        self.0 as i64
    }

    /// Builds an id from its parts. `None` if a part is out of range or the result is `0`.
    pub const fn from_parts(ms_since_epoch: u64, node: NodeId, seq: u16) -> Option<Id> {
        if ms_since_epoch > MAX_MS_SINCE_EPOCH || seq > MAX_SEQUENCE {
            return None;
        }
        Id::new(
            (ms_since_epoch << (NODE_BITS + SEQUENCE_BITS))
                | ((node.0 as u64) << SEQUENCE_BITS)
                | seq as u64,
        )
    }

    /// Milliseconds since the Sew epoch.
    pub const fn ms_since_epoch(self) -> u64 {
        self.0 >> (NODE_BITS + SEQUENCE_BITS)
    }

    /// The node that made this id.
    pub const fn node(self) -> NodeId {
        NodeId(((self.0 >> SEQUENCE_BITS) & MAX_NODE as u64) as u16)
    }

    /// The sequence within its millisecond.
    pub const fn sequence(self) -> u16 {
        (self.0 & MAX_SEQUENCE as u64) as u16
    }

    /// The smallest id that can exist at `t` (for time-based cursors). Times before the epoch
    /// give `Id(1)`; times past the id range give the largest id.
    pub fn min_for_time(t: Timestamp) -> Id {
        let ms = clamp_ms(t);
        match Id::from_parts(ms, NodeId(0), 0) {
            Some(id) => id,
            None => Id(1),
        }
    }

    /// The largest id that can exist at `t`.
    pub fn max_for_time(t: Timestamp) -> Id {
        let ms = clamp_ms(t);
        match Id::from_parts(ms, NodeId(MAX_NODE), MAX_SEQUENCE) {
            Some(id) => id,
            None => Id(i64::MAX as u64),
        }
    }
}

fn clamp_ms(t: Timestamp) -> u64 {
    let unix = t.unix_ms();
    let since = unix.saturating_sub(SEW_EPOCH_MS as i64);
    if since <= 0 {
        0
    } else {
        (since as u64).min(MAX_MS_SINCE_EPOCH)
    }
}

/// The ms-precision UTC time an id was made (C3).
pub fn id_to_time(id: Id) -> Timestamp {
    Timestamp::from_unix_ms_saturating(SEW_EPOCH_MS + id.ms_since_epoch())
}

/// Error parsing an [`Id`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdParseError(String);

impl fmt::Display for IdParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid id {:?}: expected a decimal string [1-9][0-9]{{0,18}} no larger than {}",
            self.0,
            i64::MAX
        )
    }
}

impl std::error::Error for IdParseError {}

/// Parses the strict decimal form: `[1-9][0-9]{0,18}`, at most `i64::MAX`.
pub(crate) fn parse_id_str(s: &str) -> Option<Id> {
    let b = s.as_bytes();
    if b.is_empty() || b.len() > 19 || b[0] == b'0' || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    s.parse::<u64>().ok().and_then(Id::new)
}

impl FromStr for Id {
    type Err = IdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_id_str(s).ok_or_else(|| IdParseError(s.to_owned()))
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = Id;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an id as a decimal string")
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Id, E> {
                s.parse().map_err(E::custom)
            }
        }
        deserializer.deserialize_str(V)
    }
}

impl JsonSchema for Id {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "Id".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^[1-9][0-9]{0,18}$",
            "description": "A Sew id: a decimal string of a 64-bit time-sortable id (1 to 9223372036854775807)."
        })
    }
}

/// A node number, `0 ..= 1023`. A single-node install uses [`NodeId::SINGLE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(u16);

impl NodeId {
    /// Node 0, used by every single-node install.
    pub const SINGLE: NodeId = NodeId(0);

    /// `None` for values above 1023.
    pub const fn new(n: u16) -> Option<NodeId> {
        if n > MAX_NODE { None } else { Some(NodeId(n)) }
    }

    /// The raw value.
    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Debug)]
struct GenState {
    last_ms: u64,
    seq: u16,
    issued: bool,
}

/// The clock reads behind the last issued id ([`IdGenerator::try_next`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockBehind {
    /// How far the clock reads behind the last issued id's millisecond.
    pub behind_ms: u64,
}

impl fmt::Display for ClockBehind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "sew id: the clock reads {} ms behind the last issued id",
            self.behind_ms
        )
    }
}

impl std::error::Error for ClockBehind {}

/// Makes unique, strictly increasing ids for one node (C3).
///
/// - Same millisecond: the sequence goes up. After 2,048 ids in one millisecond it yields
///   through [`Clock::sleep_ms`]`(0)` until the clock shows a later millisecond.
/// - Clock behind the last id: [`IdGenerator::next`] waits in steps of at most 100 ms and, after
///   5,000 ms of waiting, panics with `sew id: clock moved backward`; [`IdGenerator::try_next`]
///   returns [`ClockBehind`] at once. Neither ever emits a duplicate or smaller id.
/// - Clock outside the id range (before the Sew epoch or past 42 bits): it panics with
///   `sew id: clock outside id range`.
#[derive(Debug)]
pub struct IdGenerator<C: Clock = SystemClock> {
    node: NodeId,
    state: Mutex<GenState>,
    clock: C,
}

impl IdGenerator<SystemClock> {
    /// A generator on the system clock.
    pub fn new(node: NodeId) -> Self {
        IdGenerator::with_clock(node, SystemClock)
    }
}

impl<C: Clock> IdGenerator<C> {
    /// A generator on the given clock.
    pub fn with_clock(node: NodeId, clock: C) -> Self {
        IdGenerator {
            node,
            state: Mutex::new(GenState {
                last_ms: 0,
                seq: 0,
                issued: false,
            }),
            clock,
        }
    }

    /// The clock this generator reads.
    pub fn clock(&self) -> &C {
        &self.clock
    }

    /// The next id.
    ///
    /// # Panics
    ///
    /// When the clock stays behind the last id for 5 s, or reads outside the id range.
    pub fn next(&self) -> Id {
        // A panic while holding the lock only happens on the fatal paths below; a poisoned
        // state is still consistent (it holds the last issued id).
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut waited: u64 = 0;
        loop {
            let gap = match self.issue(&mut st) {
                Ok(id) => return id,
                Err(ClockBehind { behind_ms }) => behind_ms,
            };
            // The clock moved backward.
            if waited >= CLOCK_REGRESSION_MAX_WAIT_MS {
                panic!(
                    "sew id: clock moved backward by {gap} ms and did not recover within {CLOCK_REGRESSION_MAX_WAIT_MS} ms"
                );
            }
            let step = gap
                .min(REGRESSION_STEP_MS)
                .min(CLOCK_REGRESSION_MAX_WAIT_MS - waited);
            self.clock.sleep_ms(step);
            waited += step;
        }
    }

    /// The next id without waiting for a clock that moved backward: [`ClockBehind`] at once,
    /// with the gap to the last issued id (F5, contracts rev 104). A caller that must not block
    /// (a server request thread) refuses the work and retries later instead. The state is
    /// unchanged by a refusal, so ids stay unique and increasing. Same-millisecond overflow
    /// still yields (through [`Clock::sleep_ms`]`(0)`) until the next millisecond.
    ///
    /// # Panics
    ///
    /// When the clock reads outside the id range.
    pub fn try_next(&self) -> Result<Id, ClockBehind> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.issue(&mut st)
    }

    /// One attempt: an id, or the gap when the clock reads behind the last id.
    fn issue(&self, st: &mut GenState) -> Result<Id, ClockBehind> {
        loop {
            let ms = self.ms_since_epoch_now();
            if !st.issued || ms > st.last_ms {
                st.last_ms = ms;
                st.seq = 0;
                st.issued = true;
                if ms == 0 && self.node.0 == 0 {
                    // Id 0 is invalid (gap G32); skip sequence 0 at the epoch millisecond.
                    st.seq = 1;
                }
                break;
            }
            if ms == st.last_ms {
                if st.seq < MAX_SEQUENCE {
                    st.seq += 1;
                    break;
                }
                self.clock.sleep_ms(0);
                continue;
            }
            return Err(ClockBehind {
                behind_ms: st.last_ms - ms,
            });
        }
        match Id::from_parts(st.last_ms, self.node, st.seq) {
            Some(id) => Ok(id),
            None => panic!("sew id: clock outside id range"),
        }
    }

    fn ms_since_epoch_now(&self) -> u64 {
        let now = self.clock.now_unix_ms();
        if now < SEW_EPOCH_MS || now - SEW_EPOCH_MS > MAX_MS_SINCE_EPOCH {
            panic!("sew id: clock outside id range (unix ms {now})");
        }
        now - SEW_EPOCH_MS
    }
}

/// A comma-separated list of 1–100 distinct ids, as in `GET /api/v1/messages?ids=1,2,3`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdList(Vec<Id>);

/// Error parsing an [`IdList`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdListError(String);

impl fmt::Display for IdListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid id list: {}", self.0)
    }
}

impl std::error::Error for IdListError {}

impl IdList {
    /// Builds a list, checking the 1–100 length and uniqueness rules.
    pub fn new(ids: Vec<Id>) -> Result<Self, IdListError> {
        if ids.is_empty() {
            return Err(IdListError("empty".into()));
        }
        if ids.len() > crate::limits::IDS_PER_QUERY_MAX {
            return Err(IdListError(format!(
                "{} ids; at most {}",
                ids.len(),
                crate::limits::IDS_PER_QUERY_MAX
            )));
        }
        let mut seen = std::collections::HashSet::with_capacity(ids.len());
        for id in &ids {
            if !seen.insert(*id) {
                return Err(IdListError(format!("duplicate id {id}")));
            }
        }
        Ok(IdList(ids))
    }

    /// The ids in request order.
    pub fn ids(&self) -> &[Id] {
        &self.0
    }

    /// Consumes the list.
    pub fn into_vec(self) -> Vec<Id> {
        self.0
    }
}

impl FromStr for IdList {
    type Err = IdListError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(IdListError("empty".into()));
        }
        let mut ids = Vec::new();
        for part in s.split(',') {
            let id = parse_id_str(part).ok_or_else(|| IdListError(format!("bad id {part:?}")))?;
            ids.push(id);
        }
        IdList::new(ids)
    }
}

impl fmt::Display for IdList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, id) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(",")?;
            }
            fmt::Display::fmt(id, f)?;
        }
        Ok(())
    }
}

impl Serialize for IdList {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for IdList {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for IdList {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "IdList".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^[1-9][0-9]{0,18}(,[1-9][0-9]{0,18}){0,99}$",
            "description": "1 to 100 distinct ids, comma-separated."
        })
    }
}
