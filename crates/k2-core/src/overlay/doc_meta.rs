//! A Thread doc's read-state facts, kept beside its body (speed pass P6).
//!
//! The unread summary needs only a few facts about each line past a
//! person's cursor: who wrote it, whether it is history (imported or
//! collected), whether it is an action card, whether it is a card still
//! waiting and until when, a permission card's approver floor, and on
//! K2's own lines whether it is a quiet notice (0.46.18). The
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
    /// K2's own line (thread-system-notice-v1, 0.46.18): `Some(true)` for
    /// a quiet notice ([`is_quiet_notice`]: an info notice, never unread,
    /// nothing for a person to do), `Some(false)` for any other K2 line
    /// (action, delivery, a later kind, a VIP mail notice, or no
    /// `systemNotice` at all: they count).
    /// `None` on every doc that is not K2's, and on a K2 line whose row an
    /// older build wrote ([`DocMeta::notice_unknown`]: read from the doc).
    pub quiet_notice: Option<bool>,
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
            quiet_notice: (doc.by_system() || doc.system_notice.is_some()).then(|| is_quiet_notice(doc)),
        }
    }

    /// A row an older build wrote for one of K2's own lines: it doesn't say
    /// whether the line is a quiet notice, so the reader takes the facts
    /// from the doc itself (only K2's lines; every other row stands).
    pub fn notice_unknown(&self) -> bool {
        self.quiet_notice.is_none()
            && self.author.as_ref().is_some_and(|(kind, _)| kind == super::doc::AUTHOR_SYSTEM)
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
    /// last so it may hold any character. Flags `i` / `n` (0.46.18) are a
    /// quiet notice / another K2 line; a build before them reads a row
    /// with either as unreadable and parses that doc, so a downgrade
    /// never miscounts.
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
        match self.quiet_notice {
            Some(true) => flags.push('i'),
            Some(false) => flags.push('n'),
            None => {}
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
        if flags.chars().any(|c| !matches!(c, 'h' | 'a' | 'w' | 'u' | 'i' | 'n')) {
            return None;
        }
        let quiet_notice = match (flags.contains('i'), flags.contains('n')) {
            (true, true) => return None,
            (true, false) => Some(true),
            (false, true) => Some(false),
            (false, false) => None,
        };
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
            quiet_notice,
        })
    }
}

/// Info notices that still count as unread (0.46.18): a VIP mail notice
/// ([`super::doc::NOTICE_CODE_MAIL_VIP`]) is the person's important mail,
/// the reason they turned VIP on. Its kind stays `info` on the wire (how a
/// client draws it); only the count differs.
pub const COUNTED_INFO_CODES: &[&str] = &[super::doc::NOTICE_CODE_MAIL_VIP];

/// K2's quiet notice: never unread (0.46.18). An info notice
/// (thread-system-notice-v1 kind [`super::doc::NOTICE_INFO`], exactly)
/// whose code is not in [`COUNTED_INFO_CODES`]. Action and delivery
/// notices, a later kind, a counted info code and a K2 line without
/// `systemNotice` all count.
pub fn is_quiet_notice(doc: &OverlayDoc) -> bool {
    doc.system_notice
        .as_ref()
        .is_some_and(|n| n.kind == super::doc::NOTICE_INFO && !COUNTED_INFO_CODES.contains(&n.code.as_str()))
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
                quiet_notice: Some(true),
            },
            DocMeta { author: Some(("system".into(), "k2".into())), quiet_notice: Some(false), ..Default::default() },
            DocMeta { author: Some(("system".into(), "k2".into())), quiet_notice: None, ..Default::default() },
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
        assert_eq!(DocMeta::decode("1\tin\t\t\t\t"), None, "info and not-info at once");
        assert_eq!(DocMeta::decode(""), None);
    }

    /// 0.46.18: K2's lines say whether they are quiet notices; nobody
    /// else's row changes (an older build still reads it).
    #[test]
    fn k2_lines_record_their_notice_kind_and_other_rows_stay_as_they_were() {
        use super::super::doc::{PostAuthor, SystemNotice};
        let sys = |n: Option<SystemNotice>| {
            let mut d = OverlayDoc::system_text("d".into(), "a".into(), "K2: x".into(), "k2", SystemNotice::info("x"));
            d.system_notice = n;
            DocMeta::of(&d)
        };
        assert_eq!(sys(Some(SystemNotice::info("job.added"))).quiet_notice, Some(true));
        assert_eq!(sys(Some(SystemNotice::action("clone.proposed"))).quiet_notice, Some(false));
        assert_eq!(sys(Some(SystemNotice::delivery("delivery.not_taken.retry_failed"))).quiet_notice, Some(false));
        assert_eq!(sys(Some(SystemNotice::new("later", "x"))).quiet_notice, Some(false), "a later kind counts");
        assert_eq!(sys(None).quiet_notice, Some(false), "a K2 line without systemNotice counts");
        // A VIP mail notice is info on the wire and still counts; a
        // changed-files card is quiet.
        let vip = super::super::doc::ThreadNotice {
            source: "mail".into(),
            inbox: "a@x".into(),
            from_name: None,
            from_addr: "b@y".into(),
            auth: "DMARC pass".into(),
            subject: "s".into(),
            snippet: None,
            read_command: "k2 mail read m_1".into(),
            delivered: true,
            reason: None,
            notice_id: 1,
        };
        let vip = OverlayDoc::mail_notice_card("v".into(), "a".into(), "VIP mail from b@y".into(), vip);
        assert_eq!(vip.system_notice.as_ref().map(|n| n.kind.as_str()), Some(super::super::doc::NOTICE_INFO));
        assert_eq!(DocMeta::of(&vip).quiet_notice, Some(false), "a VIP mail notice counts");
        let changes = OverlayDoc::changes_card("c".into(), "k2".into(), "a".into(), "2 files".into(), Default::default());
        assert_eq!(DocMeta::of(&changes).quiet_notice, Some(true), "a changed-files card is quiet");
        assert!(!sys(None).notice_unknown());

        let mut person = OverlayDoc::text("p".into(), "owner".into(), "a".into(), "hi".into(), "compose");
        person.author = Some(PostAuthor::owner());
        let m = DocMeta::of(&person);
        assert_eq!(m.quiet_notice, None);
        assert!(!m.encode().split('\t').nth(1).expect("flags").contains(['i', 'n']), "{}", m.encode());

        // A row an older build wrote for a K2 line: the reader asks the doc.
        let old = DocMeta::decode("1\tu\t\t\tsystem\tk2").expect("v1 row");
        assert!(old.notice_unknown());
        assert!(!DocMeta::decode("1\tu\t\t\tagent\tx").expect("v1 row").notice_unknown());
    }
}
