//! Permission bits and masks (C12, gaps G23, G24; contracts rev 23).
//!
//! Bits 0–31 are channel-scoped and may appear in overwrites. Bits 32–61 are space-scoped;
//! only 32 and 33 may appear in overwrites. Bits 24–31, 45–61 and 63 are reserved. Compute
//! lives in `sew-perms` (S5).

use std::borrow::Cow;
use std::fmt;
use std::ops::{BitAnd, BitOr, BitOrAssign, Not};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::ValidationError;

/// A permission bitfield. JSON: a decimal string such as `"6734509711871"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Perms(pub u64);

macro_rules! perm_bits {
    ($( $(#[$m:meta])* $name:ident = $bit:literal ),+ $(,)?) => {
        impl Perms {
            $( $(#[$m])* pub const $name: Perms = Perms(1 << $bit); )+
        }

        /// Every defined bit and its name, in bit order (C12).
        pub const REGISTRY: &[(u8, &str)] = &[ $( ($bit, stringify!($name)) ),+ ];
    };
}

perm_bits! {
    /// See the channel and read its messages as they arrive.
    VIEW_CHANNEL = 0,
    /// Read messages sent before the member could view the channel.
    READ_MESSAGE_HISTORY = 1,
    /// Post top-level messages.
    SEND_MESSAGES = 2,
    /// Reply in threads.
    SEND_IN_THREADS = 3,
    /// Start threads.
    CREATE_THREADS = 4,
    /// Link previews on own messages.
    EMBED_LINKS = 5,
    /// Upload files.
    ATTACH_FILES = 6,
    /// Add new reactions.
    ADD_REACTIONS = 7,
    /// Use custom emoji from other spaces.
    USE_EXTERNAL_EMOJI = 8,
    /// `@channel`, `@here` and non-mentionable roles.
    MENTION_EVERYONE = 9,
    /// Delete or hide others' messages.
    MANAGE_MESSAGES = 10,
    /// Pin and unpin messages.
    PIN_MESSAGES = 11,
    /// Manage threads.
    MANAGE_THREADS = 12,
    /// Create polls.
    CREATE_POLLS = 13,
    /// Manage channel bookmarks.
    MANAGE_BOOKMARKS = 14,
    /// Not limited by slow mode.
    BYPASS_SLOW_MODE = 15,
    /// Join voice.
    CONNECT = 16,
    /// Speak in voice.
    SPEAK = 17,
    /// Send video.
    VIDEO = 18,
    /// Share a screen.
    SCREEN_SHARE = 19,
    /// Server-mute others in voice.
    MUTE_MEMBERS = 20,
    /// Server-deafen others in voice.
    DEAFEN_MEMBERS = 21,
    /// Move others between voice channels.
    MOVE_MEMBERS = 22,
    /// Record calls.
    RECORD_CALLS = 23,
    /// Create, edit and archive channels.
    MANAGE_CHANNELS = 32,
    /// Manage roles below one's own.
    MANAGE_ROLES = 33,
    /// Edit space settings.
    MANAGE_SPACE = 34,
    /// Read the space audit log.
    VIEW_AUDIT_LOG = 35,
    /// Manage custom emoji.
    MANAGE_EMOJI = 36,
    /// Add members.
    ADD_MEMBERS = 37,
    /// Remove members.
    REMOVE_MEMBERS = 38,
    /// Time members out.
    TIMEOUT_MEMBERS = 39,
    /// Manage user groups.
    MANAGE_USER_GROUPS = 40,
    /// Create public channels.
    CREATE_PUBLIC_CHANNELS = 41,
    /// Create private channels.
    CREATE_PRIVATE_CHANNELS = 42,
    /// Archive channels.
    ARCHIVE_CHANNELS = 43,
    /// Invite guests.
    INVITE_GUESTS = 44,
    /// Every bit.
    ADMINISTRATOR = 62,
}

const fn bits_range(lo: u32, hi: u32) -> u64 {
    let mut v = 0u64;
    let mut b = lo;
    while b <= hi {
        v |= 1 << b;
        b += 1;
    }
    v
}

/// Default `@everyone` role: bits 0–8, 13, 16–19, 37, 41, 42 (`6734509711871`).
pub const DEFAULT_EVERYONE: Perms =
    Perms(bits_range(0, 8) | 1 << 13 | bits_range(16, 19) | 1 << 37 | 1 << 41 | 1 << 42);
/// Channel-scoped bits 0–23 (`16777215`).
pub const CHANNEL_SCOPED: Perms = Perms(bits_range(0, 23));
/// Space-scoped bits 32–44 and 62.
pub const SPACE_SCOPED: Perms = Perms(bits_range(32, 44) | 1 << 62);
/// Every defined bit (`4611721198521286655`).
pub const ALL_DEFINED: Perms = Perms(CHANNEL_SCOPED.0 | SPACE_SCOPED.0);
/// Bits an overwrite may hold: 0–23, 32, 33 (`12901679103`).
pub const OVERWRITABLE: Perms = Perms(bits_range(0, 23) | 1 << 32 | 1 << 33);
/// A guest's ceiling: bits 0–9, 11, 13, 16–19 (`994303`, contracts rev 23).
pub const GUEST_MASK: Perms = Perms(bits_range(0, 9) | 1 << 11 | 1 << 13 | bits_range(16, 19));
/// A meeting link guest's ceiling (Meet MC2a, CC3, prd-meet-01 §6.5.2): VIEW_CHANNEL,
/// SEND_MESSAGES, ADD_REACTIONS, CONNECT, SPEAK and VIDEO (`458885`). No READ_MESSAGE_HISTORY
/// (the history floor), no threads, links, files, pins, polls or moderation. Applied after
/// every role and overwrite, like [`GUEST_MASK`].
pub const LINK_GUEST_MASK: Perms = Perms(1 << 0 | 1 << 2 | 1 << 7 | 1 << 16 | 1 << 17 | 1 << 18);
/// [`LINK_GUEST_MASK`] plus SCREEN_SHARE (bit 19), when the meeting's host allows
/// `guest_screen_share` (`983173`).
pub const LINK_GUEST_MASK_SCREEN: Perms = Perms(LINK_GUEST_MASK.0 | 1 << 19);
// CC3: both link-guest masks are subsets of the guest mask.
const _: () = assert!(LINK_GUEST_MASK_SCREEN.0 & !GUEST_MASK.0 == 0);
const _: () = assert!(LINK_GUEST_MASK.0 == 458_885 && LINK_GUEST_MASK_SCREEN.0 == 983_173);
/// An assistant's ceiling: bits 0–23 minus 10, 12, 14, 20, 21, 22 (`9415679`, gap G23).
pub const ASSISTANT_CEILING: Perms =
    Perms(bits_range(0, 23) & !(1 << 10 | 1 << 12 | 1 << 14 | 1 << 20 | 1 << 21 | 1 << 22));
/// SW11: the manager role a mgmt-created Space gives its listed managers (persons or coworker
/// agents): everything a community manager needs, never ADMINISTRATOR. Bits 0–22 (every
/// channel bit but RECORD_CALLS), MANAGE_CHANNELS, MANAGE_ROLES, VIEW_AUDIT_LOG,
/// MANAGE_EMOJI, ADD_MEMBERS, REMOVE_MEMBERS, TIMEOUT_MEMBERS, CREATE_PUBLIC_CHANNELS,
/// CREATE_PRIVATE_CHANNELS and ARCHIVE_CHANNELS (`16471207968767`). Not MANAGE_SPACE (settings,
/// transfer, delete), MANAGE_USER_GROUPS, INVITE_GUESTS, RECORD_CALLS or ADMINISTRATOR. S12's
/// ladder still applies: a manager acts only below the role, never on the owner.
pub const SPACE_MANAGER: Perms = Perms(
    bits_range(0, 22)
        | 1 << 32
        | 1 << 33
        | 1 << 35
        | 1 << 36
        | 1 << 37
        | 1 << 38
        | 1 << 39
        | 1 << 41
        | 1 << 42
        | 1 << 43,
);
/// Bits cleared when SEND_MESSAGES is missing: 5, 6, 9, 13 (`8800`).
pub const NO_SEND_CLEARS: Perms = Perms(1 << 5 | 1 << 6 | 1 << 9 | 1 << 13);
/// Announcement channels' default `@everyone` deny: SEND_MESSAGES (`4`).
pub const ANNOUNCEMENT_EVERYONE_DENY: Perms = Perms::SEND_MESSAGES;
/// Announcement channels' default `@everyone` allow: SEND_IN_THREADS (`8`).
pub const ANNOUNCEMENT_EVERYONE_ALLOW: Perms = Perms::SEND_IN_THREADS;

// ---- S10 (spaces, channels) masks, prd-sew-10 §6.2 ----

/// What a space member who has not joined a public channel may use there: VIEW_CHANNEL,
/// READ_MESSAGE_HISTORY, MANAGE_CHANNELS, MANAGE_ROLES, ARCHIVE_CHANNELS (bits 0, 1, 32, 33, 43;
/// `8808977924099`). Taking part needs a join (S10 gap G3). Voice channels also keep the voice
/// bits 16–23 so public voice is drop-in (contracts rev 24).
pub const NONMEMBER_MASK: Perms = Perms(1 | 1 << 1 | 1 << 32 | 1 << 33 | 1 << 43);
/// What `my_perms` keeps in an archived channel: the same bits as [`NONMEMBER_MASK`]
/// (`8808977924099`).
pub const ARCHIVED_MASK: Perms = NONMEMBER_MASK;
/// Read-only: VIEW_CHANNEL and READ_MESSAGE_HISTORY (`3`).
pub const READ_ONLY: Perms = Perms(1 | 1 << 1);

impl Perms {
    /// No bits.
    pub const NONE: Perms = Perms(0);

    /// Whether every bit of `other` is set.
    pub const fn contains(self, other: Perms) -> bool {
        self.0 & other.0 == other.0
    }

    /// The names of the defined bits that are set, in bit order.
    pub fn names(self) -> Vec<&'static str> {
        REGISTRY
            .iter()
            .filter(|(bit, _)| self.0 & (1 << bit) != 0)
            .map(|(_, name)| *name)
            .collect()
    }

    /// The set bits that no contract defines.
    pub const fn undefined_bits(self) -> Perms {
        Perms(self.0 & !ALL_DEFINED.0)
    }

    /// The bit set by name.
    pub fn from_name(name: &str) -> Option<Perms> {
        REGISTRY
            .iter()
            .find(|(_, n)| *n == name)
            .map(|(bit, _)| Perms(1 << bit))
    }

    /// Rejects bits outside [`ALL_DEFINED`] in a request (`validation_failed` on `field`).
    pub fn validate_request(self, field: &str) -> Result<(), ValidationError> {
        let extra = self.undefined_bits();
        if extra.0 != 0 {
            return Err(ValidationError::invalid(
                field,
                format!("{field} sets undefined permission bits {}", extra.0),
            ));
        }
        Ok(())
    }

    /// Rejects bits outside [`OVERWRITABLE`] (`validation_failed` on `field`).
    pub fn validate_overwrite(self, field: &str) -> Result<(), ValidationError> {
        let extra = self.0 & !OVERWRITABLE.0;
        if extra != 0 {
            return Err(ValidationError::invalid(
                field,
                format!("{field} sets bits {extra} that overwrites may not hold"),
            ));
        }
        Ok(())
    }
}

impl BitOr for Perms {
    type Output = Perms;
    fn bitor(self, rhs: Perms) -> Perms {
        Perms(self.0 | rhs.0)
    }
}

impl BitOrAssign for Perms {
    fn bitor_assign(&mut self, rhs: Perms) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for Perms {
    type Output = Perms;
    fn bitand(self, rhs: Perms) -> Perms {
        Perms(self.0 & rhs.0)
    }
}

impl Not for Perms {
    type Output = Perms;
    fn not(self) -> Perms {
        Perms(!self.0)
    }
}

impl fmt::Display for Perms {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl Serialize for Perms {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Perms {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = Perms;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a permission bitfield as a decimal string")
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Perms, E> {
                let b = s.as_bytes();
                let canonical = !b.is_empty()
                    && b.len() <= 20
                    && b.iter().all(u8::is_ascii_digit)
                    && (b[0] != b'0' || b.len() == 1);
                if !canonical {
                    return Err(E::custom(format!(
                        "invalid perms {s:?}: expected a decimal u64 string"
                    )));
                }
                s.parse::<u64>()
                    .map(Perms)
                    .map_err(|e| E::custom(format!("invalid perms {s:?}: {e}")))
            }
        }
        deserializer.deserialize_str(V)
    }
}

impl JsonSchema for Perms {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        "Perms".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^(0|[1-9][0-9]{0,19})$",
            "description": "A permission bitfield (u64) as a decimal string."
        })
    }
}
