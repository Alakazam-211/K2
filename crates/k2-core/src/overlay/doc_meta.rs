//! A Thread doc's read-state facts, kept beside its body (speed pass P6).
//!
//! The unread summary needs only a few facts about each line past a
//! person's cursor: who wrote it, whether it is history (imported or
//! collected), whether it is an action card, whether it is a card still
//! waiting and until when, and a permission card's approver floor. The
//! store writes these in a small `doc_meta` row (`doc id → encoded
//! DocMeta`) in the SAME redb transaction as every body write, so a count
//! reads a few short strings instead of parsing whole documents.
//!
//! Keyed by doc id, not by seq: a conversation move or a fork-tab split
//! renumbers pointers but never rewrites a body, so the row stays right
//! without following them. A doc with no row (written before the index,
//! not yet backfilled) is read the old way, so a count never depends on
//! the backfill having finished.

use super::doc::OverlayDoc;

/// The facts [`crate::read_state`] counts from. Derived only from the doc
/// ([`DocMeta::of`]); never edited on its own.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DocMeta {
    /// The credential stamp (`author.kind`, `author.login`), verbatim.
    pub author: Option<(String, String)>,
    /// Imported history or a collected line (never unread).
    pub history: bool,
    /// [`OverlayDoc::is_action_card`].
    pub action_card: bool,
    /// [`super::store::is_waiting_card`]: a pending choice / secret card
    /// that is not history.
    pub waiting_card: bool,
    /// When a pending card's time runs out (the time
    /// [`OverlayDoc::is_due`] reads), `None` for anything else.
    pub expires_at: Option<i64>,
    /// A permission card's approver floor (`owner` or `admin`), `None`
    /// for any other doc.
    pub permission_floor: Option<String>,
}

/// Encoding version. A row in another version reads as missing (the doc
/// is read the old way), so a later format never miscounts.
const VERSION: &str = "1";

impl DocMeta {
    /// The facts of one doc.
    pub fn of(doc: &OverlayDoc) -> Self {
        let expires_at = if doc.is_pending_choice() {
            doc.choice.as_ref().and_then(|c| c.expires_at)
        } else if doc.is_pending_secret() {
            doc.secret.as_ref().and_then(|s| s.expires_at)
        } else {
            None
        };
        Self {
            author: doc.author.as_ref().map(|a| (a.kind.clone(), a.login.clone())),
            history: doc.imported.is_some() || doc.external.is_some() || doc.via.as_deref() == Some("import"),
            action_card: doc.is_action_card(),
            waiting_card: super::store::is_waiting_card(doc),
            expires_at,
            permission_floor: permission_floor(doc).map(str::to_string),
        }
    }

    /// [`OverlayDoc::is_due`] for the doc this was taken from.
    pub fn is_due(&self, now: i64) -> bool {
        self.expires_at.is_some_and(|t| t <= now)
    }

    /// `overlay::is_waiting_now` for the doc this was taken from.
    pub fn is_waiting_now(&self, now: i64) -> bool {
        self.waiting_card && !self.is_due(now)
    }

    /// `1\t{flags}\t{expires}\t{floor}\t{kind}\t{login}`; the login is
    /// last so it may hold any character.
    pub fn encode(&self) -> String {
        let mut flags = String::new();
        if self.history {
            flags.push('h');
        }
        if self.action_card {
            flags.push('a');
        }
        if self.waiting_card {
            flags.push('w');
        }
        if self.author.is_some() {
            flags.push('u');
        }
        let expires = self.expires_at.map(|t| t.to_string()).unwrap_or_default();
        let floor = self.permission_floor.as_deref().unwrap_or("");
        let (kind, login) = match &self.author {
            Some((k, l)) => (k.as_str(), l.as_str()),
            None => ("", ""),
        };
        format!("{VERSION}\t{flags}\t{expires}\t{floor}\t{kind}\t{login}")
    }

    /// [`DocMeta::encode`]'s inverse; `None` for a row this build can't
    /// read (the caller falls back to the doc).
    pub fn decode(s: &str) -> Option<Self> {
        let mut it = s.splitn(6, '\t');
        if it.next()? != VERSION {
            return None;
        }
        let flags = it.next()?;
        if flags.chars().any(|c| !matches!(c, 'h' | 'a' | 'w' | 'u')) {
            return None;
        }
        let expires = it.next()?;
        let expires_at = if expires.is_empty() { None } else { Some(expires.parse().ok()?) };
        let floor = it.next()?;
        let kind = it.next()?;
        let login = it.next()?;
        Some(Self {
            author: flags.contains('u').then(|| (kind.to_string(), login.to_string())),
            history: flags.contains('h'),
            action_card: flags.contains('a'),
            waiting_card: flags.contains('w'),
            expires_at,
            permission_floor: (!floor.is_empty()).then(|| floor.to_string()),
        })
    }
}

/// A permission card's floor (`choice.action.view.approvers.floor`), or
/// `None` for any other doc. A permission card without a readable floor
/// waits for the owner only (the strictest floor).
pub fn permission_floor(doc: &OverlayDoc) -> Option<&'static str> {
    let action = doc.choice.as_ref()?.action.as_ref()?;
    if action.id != crate::permission::CARD_ACTION_PERMISSION && action.id != crate::permission::CARD_ACTION_BATCH {
        return None;
    }
    let floor = action
        .view
        .as_ref()
        .and_then(|v| v.get("approvers"))
        .and_then(|a| a.get("floor"))
        .and_then(serde_json::Value::as_str);
    Some(match floor {
        Some("admin") => "admin",
        _ => "owner",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_round_trips_every_field_and_odd_logins() {
        let cases = [
            DocMeta::default(),
            DocMeta {
                author: Some(("user".into(), "Al\tice\nx".into())),
                history: true,
                action_card: true,
                waiting_card: true,
                expires_at: Some(-5),
                permission_floor: Some("admin".into()),
            },
            DocMeta { author: Some((String::new(), String::new())), expires_at: Some(1_900_000_000), ..Default::default() },
        ];
        for m in cases {
            assert_eq!(DocMeta::decode(&m.encode()), Some(m.clone()), "{:?}", m.encode());
        }
    }

    #[test]
    fn rows_in_another_version_or_shape_read_as_missing() {
        assert_eq!(DocMeta::decode("2\t\t\t\t\t"), None);
        assert_eq!(DocMeta::decode("1\tz\t\t\t\t"), None);
        assert_eq!(DocMeta::decode("1\t\tsoon\t\t\t"), None);
        assert_eq!(DocMeta::decode("1\t\t"), None);
        assert_eq!(DocMeta::decode(""), None);
    }
}
