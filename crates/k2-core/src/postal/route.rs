//! Where a Postal message lands (prd-postal-in-k2-v1 §5.2): K2's routing
//! rules for its delivery hook.
//!
//! A rule is `{"targets": [...], "wake": bool}` with targets `terminal`,
//! `inbox`, `inbox+pointer` and `thread:<person>`. The friend's rule wins,
//! then the agent's, then the default (`terminal`, wake on). [`plan`] then
//! makes the route quieter, never louder, for what the message is:
//! - over 4 KiB or with files: `terminal` becomes `inbox+pointer`;
//! - not valid UTF-8 `text/plain`: `terminal` and Thread targets become
//!   `inbox` (with a pointer where the terminal was);
//! - the sender asked for the tray (`mode = tray`): `terminal` becomes
//!   `inbox`; the sender's `wake = false` turns the wake off.
//!
//! A busy or unreachable agent is the delivery step's (`notify`): the
//! message is stored in the inbox and the pointer waits for the agent.

use serde::{Deserialize, Serialize};

/// Bigger than this goes to the inbox with a pointer.
pub const LONG_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Target {
    Terminal,
    Inbox { pointer: bool },
    /// A Postal line on that person's Thread with the agent.
    Thread(String),
}

impl Target {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        match s {
            "terminal" => Some(Self::Terminal),
            "inbox" => Some(Self::Inbox { pointer: false }),
            "inbox+pointer" => Some(Self::Inbox { pointer: true }),
            _ => {
                let p = s.strip_prefix("thread:")?.trim();
                let ok = !p.is_empty() && p.len() <= 64 && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'));
                ok.then(|| Self::Thread(p.to_string()))
            }
        }
    }

    pub fn as_string(&self) -> String {
        match self {
            Self::Terminal => "terminal".into(),
            Self::Inbox { pointer: false } => "inbox".into(),
            Self::Inbox { pointer: true } => "inbox+pointer".into(),
            Self::Thread(p) => format!("thread:{p}"),
        }
    }

    /// How loud: terminal 3, inbox with a pointer 2, inbox 1 (a Thread is
    /// a person's, counted apart).
    fn loudness(&self) -> u8 {
        match self {
            Self::Terminal => 3,
            Self::Inbox { pointer: true } => 2,
            Self::Inbox { pointer: false } => 1,
            Self::Thread(_) => 0,
        }
    }
}

impl Serialize for Target {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.as_string())
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Target::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("unknown Postal route target {s}")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub targets: Vec<Target>,
    #[serde(default = "yes")]
    pub wake: bool,
}

fn yes() -> bool {
    true
}

impl Default for Rule {
    fn default() -> Self {
        Self { targets: vec![Target::Terminal], wake: true }
    }
}

impl Rule {
    /// A stored rule (`postal_handles.route` / `postal_friends.route`).
    pub fn from_json(s: Option<&str>) -> Option<Self> {
        let r: Rule = serde_json::from_str(s?).ok()?;
        (!r.targets.is_empty()).then_some(r)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    /// Parse `terminal inbox+pointer thread:rosson`; `None` on an unknown word.
    pub fn parse_targets(words: &[&str], wake: bool) -> Option<Self> {
        let mut targets = Vec::new();
        for w in words {
            let t = Target::parse(w)?;
            if !targets.contains(&t) {
                targets.push(t);
            }
        }
        (!targets.is_empty()).then_some(Self { targets, wake })
    }

    fn loudest(&self) -> u8 {
        self.targets.iter().map(Target::loudness).max().unwrap_or(0)
    }

    fn threads(&self) -> Vec<&str> {
        self.targets.iter().filter_map(|t| if let Target::Thread(p) = t { Some(p.as_str()) } else { None }).collect()
    }

    /// Is `self` no louder than `before` (Rosson's 11-08 answer 6: an agent
    /// may only make its own routing quieter)? Never a louder target, never
    /// wake on where it was off, never a new person's Thread.
    pub fn is_quieter_than(&self, before: &Rule) -> bool {
        let threads = before.threads();
        self.loudest() <= before.loudest() && (!self.wake || before.wake) && self.threads().iter().all(|t| threads.contains(t))
    }
}

/// What the message is, for [`plan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    pub body_len: usize,
    pub has_files: bool,
    /// The body is valid UTF-8 and its content type is `text/plain`.
    pub plain_text: bool,
    /// The sender asked for the tray (`mode = tray`).
    pub tray: bool,
    /// The sender's signed `wake`.
    pub wake: bool,
}

/// Where it lands and whether it wakes, with why it was made quieter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub targets: Vec<Target>,
    pub wake: bool,
    #[serde(default)]
    pub adjusted: Vec<String>,
}

/// Is a content type `text/plain` (any parameters)?
pub fn is_text_plain(content_type: &str) -> bool {
    content_type.split(';').next().is_some_and(|t| t.trim().eq_ignore_ascii_case("text/plain"))
}

pub fn plan(agent: Option<&Rule>, friend: Option<&Rule>, f: &Facts) -> Plan {
    let base = friend.or(agent).cloned().unwrap_or_default();
    let mut adjusted = Vec::new();
    let mut out: Vec<Target> = Vec::new();
    let mut push = |t: Target| {
        if !out.contains(&t) {
            out.push(t);
        }
    };
    for t in base.targets {
        let t = match t {
            Target::Terminal if !f.plain_text => {
                adjusted.push("not_text".into());
                Target::Inbox { pointer: true }
            }
            Target::Thread(_) if !f.plain_text => {
                adjusted.push("not_text".into());
                Target::Inbox { pointer: false }
            }
            Target::Terminal if f.tray => {
                adjusted.push("sender_tray".into());
                Target::Inbox { pointer: false }
            }
            Target::Terminal if f.body_len > LONG_BYTES || f.has_files => {
                adjusted.push(if f.has_files { "files".into() } else { "long".into() });
                Target::Inbox { pointer: true }
            }
            t => t,
        };
        push(t);
    }
    // One inbox item: a pointer wins over a plain inbox target.
    if out.contains(&Target::Inbox { pointer: true }) {
        out.retain(|t| *t != Target::Inbox { pointer: false });
    }
    if !f.wake && base.wake {
        adjusted.push("sender_no_wake".into());
    }
    adjusted.dedup();
    Plan { targets: out, wake: base.wake && f.wake, adjusted }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts { body_len: 20, has_files: false, plain_text: true, tray: false, wake: true }
    }

    #[test]
    fn the_default_is_the_terminal_with_a_wake() {
        let p = plan(None, None, &facts());
        assert_eq!((p.targets, p.wake), (vec![Target::Terminal], true));
    }

    #[test]
    fn the_friend_rule_wins_then_the_agent_rule() {
        let agent = Rule::parse_targets(&["inbox"], true).unwrap();
        let friend = Rule::parse_targets(&["thread:rosson", "terminal"], false).unwrap();
        assert_eq!(plan(Some(&agent), None, &facts()).targets, vec![Target::Inbox { pointer: false }]);
        let p = plan(Some(&agent), Some(&friend), &facts());
        assert_eq!(p.targets, vec![Target::Thread("rosson".into()), Target::Terminal]);
        assert!(!p.wake);
    }

    #[test]
    fn long_filed_tray_and_binary_messages_only_get_quieter() {
        let long = Facts { body_len: LONG_BYTES + 1, ..facts() };
        assert_eq!(plan(None, None, &long).targets, vec![Target::Inbox { pointer: true }]);
        let files = Facts { has_files: true, ..facts() };
        assert_eq!(plan(None, None, &files).targets, vec![Target::Inbox { pointer: true }]);
        let tray = Facts { tray: true, wake: false, ..facts() };
        let p = plan(None, None, &tray);
        assert_eq!((p.targets, p.wake), (vec![Target::Inbox { pointer: false }], false));
        let bin = Facts { plain_text: false, ..facts() };
        let r = Rule::parse_targets(&["thread:rosson", "terminal"], true).unwrap();
        assert_eq!(plan(Some(&r), None, &bin).targets, vec![Target::Inbox { pointer: true }]);
        // A sender can't make it louder: wake stays off when the rule says off.
        let quiet = Rule::parse_targets(&["inbox"], false).unwrap();
        assert!(!plan(Some(&quiet), None, &facts()).wake);
    }

    #[test]
    fn an_agent_may_only_make_its_routing_quieter() {
        let def = Rule::default();
        let inbox = Rule::parse_targets(&["inbox"], false).unwrap();
        assert!(inbox.is_quieter_than(&def));
        assert!(!def.is_quieter_than(&inbox));
        let woke_inbox = Rule::parse_targets(&["inbox"], true).unwrap();
        assert!(!woke_inbox.is_quieter_than(&inbox), "wake on where it was off");
        let thread = Rule::parse_targets(&["thread:rosson"], true).unwrap();
        assert!(!thread.is_quieter_than(&def), "a new person's Thread is a person's call");
        assert!(Target::parse("thread:").is_none() && Target::parse("bell").is_none());
        assert_eq!(Rule::from_json(Some(&inbox.to_json())), Some(inbox));
    }
}
