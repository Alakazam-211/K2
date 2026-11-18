//! Who a reply goes to: `k2 mail draft <id>` and `k2 mail reply <id>` with
//! `--reply-all`, `--cc` and `--bcc` (0.46.15).
//!
//! A plain reply goes to the reply's locked recipient only (the original's
//! sender, or its Reply-To where the backend already uses it). `--reply-all`
//! adds the original's To to the reply's To and the original's Cc to its Cc,
//! minus every address of the replying inbox (case-insensitive) and minus
//! repeats. `--cc` / `--bcc` add on top. One pure planner serves every
//! backend (hosted, linked IMAP/Gmail, Microsoft 365), so the rule can't
//! drift between them.

use std::collections::HashSet;

use crate::mail::addresses;
use crate::mail::jmap::MailAddr;

/// What the caller asked for beyond the locked recipient. `cc` / `bcc` are
/// already checked addresses (the routes run them through
/// `send::normalize_recipients`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplyAsk {
    /// Copy everyone on the original (To + Cc), minus the inbox itself.
    pub all: bool,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
}

impl ReplyAsk {
    /// Nothing beyond the locked recipient (a plain reply, as before).
    pub fn is_plain(&self) -> bool {
        !self.all && self.cc.is_empty() && self.bcc.is_empty()
    }
}

/// The original message's parties, as a reply-all needs them.
#[derive(Debug, Clone)]
pub struct SourceParties {
    /// The reply's locked recipient (the backend's own rule).
    pub primary: MailAddr,
    pub to: Vec<MailAddr>,
    pub cc: Vec<MailAddr>,
    pub subject: String,
}

/// The reply's recipients. `to[0]` is always the locked recipient.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplyParties {
    pub to: Vec<MailAddr>,
    pub cc: Vec<MailAddr>,
    pub bcc: Vec<MailAddr>,
}

impl ReplyParties {
    pub fn to_emails(&self) -> Vec<String> {
        self.to.iter().map(|a| a.email.clone()).collect()
    }
    pub fn cc_emails(&self) -> Vec<String> {
        self.cc.iter().map(|a| a.email.clone()).collect()
    }
    pub fn bcc_emails(&self) -> Vec<String> {
        self.bcc.iter().map(|a| a.email.clone()).collect()
    }
    /// The To recipients after the locked one (what a provider that picks
    /// the locked recipient itself must add).
    pub fn extra_to_emails(&self) -> Vec<String> {
        self.to.iter().skip(1).map(|a| a.email.clone()).collect()
    }
    pub fn total(&self) -> usize {
        self.to.len() + self.cc.len() + self.bcc.len()
    }
}

/// The comparable form of an address: normalized (lowercase, punycode), or
/// `None` when it isn't a replyable address at all (a group label, junk).
fn key(email: &str) -> Option<String> {
    addresses::normalize_address(email).ok()
}

/// Plan the reply's recipients. `own` = every address of the replying inbox
/// (any case). The locked recipient stays first and as given; addresses the
/// original carried that aren't replyable are left out; explicit `cc` /
/// `bcc` are kept even when they name the inbox itself (the caller asked
/// for them). Refuses (usage words) past [`crate::mail::send::MAX_RECIPIENTS`].
pub fn plan(
    primary: &MailAddr,
    orig_to: &[MailAddr],
    orig_cc: &[MailAddr],
    own: &[String],
    ask: &ReplyAsk,
) -> Result<ReplyParties, String> {
    let own: HashSet<String> = own.iter().filter_map(|a| key(a)).collect();
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(key(&primary.email).unwrap_or_else(|| primary.email.trim().to_ascii_lowercase()));
    let mut to = vec![primary.clone()];
    let mut cc = Vec::new();
    let mut bcc = Vec::new();
    let take_original = |list: &[MailAddr], out: &mut Vec<MailAddr>, seen: &mut HashSet<String>| {
        for a in list {
            let Some(k) = key(&a.email) else { continue };
            if own.contains(&k) || !seen.insert(k.clone()) {
                continue;
            }
            out.push(MailAddr { name: a.name.clone(), email: k });
        }
    };
    if ask.all {
        take_original(orig_to, &mut to, &mut seen);
        take_original(orig_cc, &mut cc, &mut seen);
    }
    for (list, out) in [(&ask.cc, &mut cc), (&ask.bcc, &mut bcc)] {
        for e in list {
            let k = key(e).unwrap_or_else(|| e.trim().to_ascii_lowercase());
            if seen.insert(k.clone()) {
                out.push(MailAddr { name: None, email: k });
            }
        }
    }
    let parties = ReplyParties { to, cc, bcc };
    let max = crate::mail::send::MAX_RECIPIENTS;
    if parties.total() > max {
        return Err(format!(
            "too many recipients ({}) — max {max} per message, counting to + cc + bcc. Reply to \
             fewer people (drop --reply-all or some --cc/--bcc)",
            parties.total()
        ));
    }
    Ok(parties)
}

/// Every address of a linked inbox: its address, and its login name when
/// that is a different address.
pub fn linked_own_addresses(email_address: &str, username: &str) -> Vec<String> {
    let mut out = vec![email_address.to_string()];
    if username.contains('@') && !username.eq_ignore_ascii_case(email_address) {
        out.push(username.to_string());
    }
    out
}

/// Every address of a hosted mailbox: its address plus its aliases on the
/// mail server. The alias read is best effort: when the mail server can't
/// say, the mailbox's own address is still left out (logged).
pub fn hosted_own_addresses(
    client: &crate::mail::jmap::StalwartClient,
    address: &str,
    account_id: &str,
) -> Vec<String> {
    let mut out = vec![address.to_string()];
    let domain = address.split_once('@').map(|(_, d)| d).unwrap_or("");
    match client.account_get_user(account_id) {
        Ok(user) => {
            for a in &user.aliases {
                out.push(format!("{}@{domain}", a.name));
            }
        }
        Err(e) => k2_core::log_debug!(
            "[mail/reply-all] {address}'s aliases unread ({e}); only its own address is left out"
        ),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(email: &str) -> MailAddr {
        MailAddr { name: None, email: email.to_string() }
    }
    fn named(name: &str, email: &str) -> MailAddr {
        MailAddr { name: Some(name.to_string()), email: email.to_string() }
    }
    fn emails(l: &[MailAddr]) -> Vec<&str> {
        l.iter().map(|x| x.email.as_str()).collect()
    }

    #[test]
    fn plain_reply_is_the_locked_recipient_only() {
        let p = plan(
            &a("sender@example.com"),
            &[a("bot@example.org"), a("ada@example.com")],
            &[a("bo@example.com")],
            &["bot@example.org".to_string()],
            &ReplyAsk::default(),
        )
        .expect("plan");
        assert_eq!(emails(&p.to), ["sender@example.com"]);
        assert!(p.cc.is_empty() && p.bcc.is_empty(), "{p:?}");
    }

    #[test]
    fn reply_all_carries_to_and_three_ccs_and_drops_our_own_addresses() {
        let own = vec!["Bot@Example.org".to_string(), "help@example.org".to_string()];
        let p = plan(
            &named("Sender", "sender@example.com"),
            &[a("BOT@example.org"), named("Ada", "Ada@Example.com")],
            &[a("bo@example.com"), a("help@EXAMPLE.org"), a("cy@example.com"), a("di@example.com")],
            &own,
            &ReplyAsk { all: true, ..Default::default() },
        )
        .expect("plan");
        assert_eq!(emails(&p.to), ["sender@example.com", "ada@example.com"]);
        assert_eq!(p.to[1].name.as_deref(), Some("Ada"), "names ride along");
        assert_eq!(emails(&p.cc), ["bo@example.com", "cy@example.com", "di@example.com"]);
        assert!(p.bcc.is_empty());
    }

    #[test]
    fn repeats_collapse_across_to_cc_and_explicit_copies() {
        let p = plan(
            &a("sender@example.com"),
            &[a("ada@example.com"), a("SENDER@example.com"), a("ada@example.com")],
            &[a("Ada@example.com"), a("bo@example.com"), a("bo@example.com")],
            &[],
            &ReplyAsk {
                all: true,
                cc: vec!["bo@example.com".into(), "cy@example.com".into(), "cy@example.com".into()],
                bcc: vec!["cy@example.com".into(), "di@example.com".into(), "sender@example.com".into()],
            },
        )
        .expect("plan");
        assert_eq!(emails(&p.to), ["sender@example.com", "ada@example.com"]);
        assert_eq!(emails(&p.cc), ["bo@example.com", "cy@example.com"]);
        assert_eq!(emails(&p.bcc), ["di@example.com"]);
    }

    #[test]
    fn explicit_cc_and_bcc_add_on_a_plain_reply() {
        let p = plan(
            &a("sender@example.com"),
            &[a("ada@example.com")],
            &[a("bo@example.com")],
            &[],
            &ReplyAsk { all: false, cc: vec!["a@example.com".into(), "b@example.com".into()], bcc: vec!["c@example.com".into()] },
        )
        .expect("plan");
        assert_eq!(emails(&p.to), ["sender@example.com"], "no reply-all: the original's parties stay off");
        assert_eq!(emails(&p.cc), ["a@example.com", "b@example.com"]);
        assert_eq!(emails(&p.bcc), ["c@example.com"]);
        assert_eq!(p.extra_to_emails(), Vec::<String>::new());
    }

    #[test]
    fn unreplyable_original_addresses_are_left_out() {
        let p = plan(
            &a("sender@example.com"),
            &[a("undisclosed-recipients"), a("ada@example.com")],
            &[a("not an address")],
            &[],
            &ReplyAsk { all: true, ..Default::default() },
        )
        .expect("plan");
        assert_eq!(emails(&p.to), ["sender@example.com", "ada@example.com"]);
        assert!(p.cc.is_empty(), "{p:?}");
    }

    #[test]
    fn past_the_recipient_cap_is_refused() {
        let many: Vec<MailAddr> = (0..crate::mail::send::MAX_RECIPIENTS).map(|i| a(&format!("p{i}@example.com"))).collect();
        let err = plan(&a("sender@example.com"), &many, &[], &[], &ReplyAsk { all: true, ..Default::default() })
            .expect_err("one over the cap");
        assert!(err.contains("too many recipients"), "{err}");
    }

    #[test]
    fn linked_own_addresses_include_a_distinct_login() {
        assert_eq!(linked_own_addresses("a@example.com", "a@example.com"), vec!["a@example.com"]);
        assert_eq!(linked_own_addresses("a@example.com", "A@EXAMPLE.COM"), vec!["a@example.com"]);
        assert_eq!(
            linked_own_addresses("a@example.com", "login@example.net"),
            vec!["a@example.com", "login@example.net"]
        );
        assert_eq!(linked_own_addresses("a@example.com", "plainuser"), vec!["a@example.com"]);
    }
}
